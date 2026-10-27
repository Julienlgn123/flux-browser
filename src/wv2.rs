//! Accès direct aux API WebView2 que wry n'expose pas : bloqueur réseau,
//! mise en veille des onglets, mesure mémoire, ordre d'empilement.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
};

use adblock::{request::Request, Engine};
use webview2_com::{
    take_pwstr,
    Microsoft::Web::WebView2::Win32::*,
    TrySuspendCompletedHandler, WebResourceRequestedEventHandler,
};
use windows::{
    core::{Interface, BOOL, HSTRING, PWSTR},
    Win32::{
        Foundation::{CloseHandle, HWND},
        System::{
            Com::IStream,
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX},
            Threading::{GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        },
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

/// Mémoire privée totale : processus Flux + tous les processus WebView2 associés.
pub fn total_memory(view: &WebView) -> u64 {
    unsafe {
        let mut pids = vec![GetCurrentProcessId()];
        if let Ok(env8) = view.environment().cast::<ICoreWebView2Environment8>() {
            if let Ok(infos) = env8.GetProcessInfos() {
                let mut n = 0u32;
                let _ = infos.Count(&mut n);
                for i in 0..n {
                    if let Ok(info) = infos.GetValueAtIndex(i) {
                        let mut pid = 0i32;
                        if info.ProcessId(&mut pid).is_ok() {
                            pids.push(pid as u32);
                        }
                    }
                }
            }
        }
        pids.into_iter().map(|pid| private_bytes(pid)).sum()
    }
}

unsafe fn private_bytes(pid: u32) -> u64 {
    let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
        return 0;
    };
    let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
    let ok = GetProcessMemoryInfo(
        h,
        &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
        std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
    )
    .is_ok();
    let _ = CloseHandle(h);
    if ok {
        c.PrivateUsage as u64
    } else {
        0
    }
}
