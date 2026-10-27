//! Accès direct aux API WebView2 que wry n'expose pas : bloqueur réseau,
//! mise en veille des onglets, processus par onglet, réseau (DevTools), comptes.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};

use adblock::{request::Request, Engine};
use webview2_com::{
    take_pwstr,
    CallDevToolsProtocolMethodCompletedHandler, ClearBrowsingDataCompletedHandler, DevToolsProtocolEventReceivedEventHandler,
    GetProcessExtendedInfosCompletedHandler,
    Microsoft::Web::WebView2::Win32::*,
    TrySuspendCompletedHandler, WebResourceRequestedEventHandler,
};
use windows::{
    core::{Interface, BOOL, HSTRING, PWSTR},
    Win32::{
        Foundation::HWND,
        System::Com::IStream,
        UI::{
            Shell::SHCreateMemStream,
            WindowsAndMessaging::{SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE},
        },
    },
};
use wry::{WebView, WebViewExtWindows};

fn request_type(ctx: COREWEBVIEW2_WEB_RESOURCE_CONTEXT) -> &'static str {
    match ctx {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => "subdocument",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => "stylesheet",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => "image",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => "media",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => "font",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => "script",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST => "xmlhttprequest",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH => "fetch",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET => "websocket",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING => "ping",
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_CSP_VIOLATION_REPORT => "csp_report",
        _ => "other",
    }
}

/// Réponse de remplacement pour les règles `$redirect` (ex. un script vide au lieu
/// d'une régie pub), au format `data:<mime>;base64,<contenu>`.
fn redirect_body(data_url: &str) -> Option<(Vec<u8>, String)> {
    use base64::Engine as _;
    let rest = data_url.strip_prefix("data:")?;
    let (head, payload) = rest.split_once(',')?;
    let mime = head.trim_end_matches(";base64").to_string();
    let bytes = if head.ends_with(";base64") {
        base64::engine::general_purpose::STANDARD.decode(payload).ok()?
    } else {
        payload.as_bytes().to_vec()
    };
    Some((bytes, mime))
}

/// Filtre chaque requête de la page avec le moteur EasyList / uBlock.
/// `page_url` est l'adresse du document principal (sert aux règles tierces et `domain=`).
pub fn install_blocker(
    view: &WebView,
    engine: Rc<RefCell<Engine>>,
    page_url: Rc<RefCell<String>>,
    enabled: Arc<AtomicBool>,
    counter: Arc<AtomicU64>,
) -> windows::core::Result<()> {
    unsafe {
        let wv = view.webview();
        let env = view.environment();
        wv.AddWebResourceRequestedFilter(&HSTRING::from("*"), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
        let mut token = 0i64;
        wv.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                if !enabled.load(Ordering::Relaxed) {
                    return Ok(());
                }
                let req = args.Request()?;
                let mut uri = PWSTR::null();
                req.Uri(&mut uri)?;
                let uri = take_pwstr(uri);
                if !uri.starts_with("http") {
                    return Ok(());
                }
                let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_OTHER;
                args.ResourceContext(&mut ctx)?;
                let page = page_url.borrow();
                // Ne jamais bloquer la navigation principale elle-même.
                if ctx == COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT && *page == uri {
                    return Ok(());
                }
                let mut method = PWSTR::null();
                req.Method(&mut method)?;
                let method = take_pwstr(method);
                let Ok(request) = Request::new(&uri, &page, request_type(ctx), &method) else {
                    return Ok(());
                };
                let result = engine.borrow().check_network_request(&request);
                if !result.should_block() {
                    return Ok(());
                }
                let resp = match result.redirect.as_deref().and_then(redirect_body) {
                    Some((body, mime)) => env.CreateWebResourceResponse(
                        SHCreateMemStream(Some(&body)).as_ref(),
                        200,
                        &HSTRING::from("OK"),
                        &HSTRING::from(format!("Content-Type: {mime}\r\nAccess-Control-Allow-Origin: *")),
                    )?,
                    None => env.CreateWebResourceResponse(None::<&IStream>, 403, &HSTRING::from("Blocked"), &HSTRING::new())?,
                };
                args.SetResponse(&resp)?;
                counter.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

/// Gèle complètement un onglet caché (CPU à zéro, mémoire compressible).
pub fn try_suspend(view: &WebView) {
    unsafe {
        if let Ok(w3) = view.webview().cast::<ICoreWebView2_3>() {
            let _ = w3.TrySuspend(&TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(()))));
        }
    }
}

pub fn playing_audio(view: &WebView) -> bool {
    unsafe {
        let mut playing = BOOL(0);
        view.webview()
            .cast::<ICoreWebView2_8>()
            .and_then(|w| w.IsDocumentPlayingAudio(&mut playing))
            .is_ok()
            && playing.as_bool()
    }
}

pub fn stop(view: &WebView) {
    unsafe {
        let _ = view.webview().Stop();
    }
}

/// Place la vue au-dessus des autres (l'interface doit recouvrir les pages
/// quand la liste de suggestions est ouverte).
pub fn raise(view: &WebView) {
    unsafe {
        let mut hwnd = HWND::default();
        if view.controller().ParentWindow(&mut hwnd).is_ok() {
            let _ = SetWindowPos(hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

pub struct ProcInfo {
    pub pid: u32,
    pub kind: &'static str,
}

/// Liste des processus du moteur (navigateur, GPU, rendu, utilitaires).
pub fn process_map(view: &WebView, done: impl FnOnce(Vec<ProcInfo>) + 'static) {
    unsafe {
        let Ok(env) = view.environment().cast::<ICoreWebView2Environment13>() else { return };
        let _ = env.GetProcessExtendedInfos(&GetProcessExtendedInfosCompletedHandler::create(Box::new(
            move |_, list| {
                let mut out = Vec::new();
                if let Some(list) = list {
                    let mut n = 0u32;
                    let _ = list.Count(&mut n);
                    for i in 0..n {
                        let Ok(info) = list.GetValueAtIndex(i).and_then(|e| e.ProcessInfo()) else { continue };
                        let (mut pid, mut kind) = (0i32, COREWEBVIEW2_PROCESS_KIND_UTILITY);
                        let _ = info.ProcessId(&mut pid);
                        let _ = info.Kind(&mut kind);
                        out.push(ProcInfo { pid: pid as u32, kind: kind_name(kind) });
                    }
                }
                done(out);
                Ok(())
            },
        )));
    }
}

/// Métriques de la page (DevTools) : taille du tas JS et temps processeur cumulé
/// du fil principal, en secondes. Nécessite `Performance.enable` (fait à la création).
pub fn page_metrics(view: &WebView, done: impl FnOnce(f64, f64) + 'static) {
    unsafe {
        let _ = view.webview().CallDevToolsProtocolMethod(
            &HSTRING::from("Performance.getMetrics"),
            &HSTRING::from("{}"),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |res, json| {
                if res.is_ok() {
                    let v: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                    let get = |name: &str| {
                        v["metrics"].as_array().and_then(|m| m.iter().find(|x| x["name"] == name)).and_then(|x| x["value"].as_f64()).unwrap_or(0.0)
                    };
                    done(get("JSHeapTotalSize"), get("TaskDuration"));
                }
                Ok(())
            })),
        );
    }
}

fn kind_name(k: COREWEBVIEW2_PROCESS_KIND) -> &'static str {
    match k {
        COREWEBVIEW2_PROCESS_KIND_BROWSER => "Navigateur",
        COREWEBVIEW2_PROCESS_KIND_RENDERER => "Rendu",
        COREWEBVIEW2_PROCESS_KIND_GPU => "GPU",
        _ => "Utilitaire",
    }
}

/// Appel d'une méthode du protocole DevTools sur la page.
pub fn cdp(view: &WebView, method: &str, params: &str) {
    unsafe {
        let _ = view.webview().CallDevToolsProtocolMethod(
            &HSTRING::from(method),
            &HSTRING::from(params),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(()))),
        );
    }
}

/// Compte les octets reçus par l'onglet (réseau réel, en-têtes compris).
pub fn watch_network(view: &WebView, received: Rc<Cell<u64>>) {
    unsafe {
        if let Ok(rx) = view.webview().GetDevToolsProtocolEventReceiver(&HSTRING::from("Network.dataReceived")) {
            let mut token = 0i64;
            let _ = rx.add_DevToolsProtocolEventReceived(
                &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        let mut json = PWSTR::null();
                        args.ParameterObjectAsJson(&mut json)?;
                        let v: serde_json::Value = serde_json::from_str(&take_pwstr(json)).unwrap_or_default();
                        let n = v["encodedDataLength"].as_u64().filter(|&n| n > 0).or(v["dataLength"].as_u64());
                        received.set(received.get() + n.unwrap_or(0));
                    }
                    Ok(())
                })),
                &mut token,
            );
        }
    }
    cdp(view, "Network.enable", "{}");
    cdp(view, "Performance.enable", "{}");
}

/// Débit maximal de l'onglet en Ko/s (0 = illimité).
pub fn set_net_limit(view: &WebView, kbps: u64) {
    let rate = if kbps == 0 { -1.0 } else { kbps as f64 * 1024.0 };
    cdp(
        view,
        "Network.emulateNetworkConditions",
        &format!(r#"{{"offline":false,"latency":0,"downloadThroughput":{rate},"uploadThroughput":{rate}}}"#),
    );
}

/// Mots de passe enregistrés et remplissage automatique (chiffrés par le moteur, comme dans Edge).
pub fn set_autofill(view: &WebView, passwords: bool, autofill: bool) {
    unsafe {
        if let Ok(s) = view.webview().Settings().and_then(|s| s.cast::<ICoreWebView2Settings4>()) {
            let _ = s.SetIsPasswordAutosaveEnabled(passwords);
            let _ = s.SetIsGeneralAutofillEnabled(autofill);
        }
    }
}

/// Niveau de protection contre le pistage du profil (partagé par tous les onglets).
pub fn set_tracking(view: &WebView, level: &str) {
    let level = match level {
        "off" => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_NONE,
        "basic" => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_BASIC,
        "strict" => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_STRICT,
        _ => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_BALANCED,
    };
    unsafe {
        if let Ok(p) = view
            .webview()
            .cast::<ICoreWebView2_13>()
            .and_then(|w| w.Profile())
            .and_then(|p| p.cast::<ICoreWebView2Profile3>())
        {
            let _ = p.SetPreferredTrackingPreventionLevel(level);
        }
    }
}

/// Efface des données du profil : "cookies" (déconnecte de tous les sites),
/// "passwords", "cache" ou "all".
pub fn clear_data(view: &WebView, what: &str) {
    let kinds = match what {
        "cookies" => COREWEBVIEW2_BROWSING_DATA_KINDS_COOKIES,
        "passwords" => COREWEBVIEW2_BROWSING_DATA_KINDS_PASSWORD_AUTOSAVE | COREWEBVIEW2_BROWSING_DATA_KINDS_GENERAL_AUTOFILL,
        "cache" => COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE | COREWEBVIEW2_BROWSING_DATA_KINDS_CACHE_STORAGE,
        _ => COREWEBVIEW2_BROWSING_DATA_KINDS_ALL_PROFILE,
    };
    unsafe {
        if let Ok(p) = view
            .webview()
            .cast::<ICoreWebView2_13>()
            .and_then(|w| w.Profile())
            .and_then(|p| p.cast::<ICoreWebView2Profile2>())
        {
            let _ = p.ClearBrowsingData(kinds, &ClearBrowsingDataCompletedHandler::create(Box::new(|_| Ok(()))));
        }
    }
}
