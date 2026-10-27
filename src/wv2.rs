//! Accès direct aux API WebView2 que wry n'expose pas : bloqueur réseau,
//! mise en veille des onglets, mesure mémoire, ordre d'empilement.

use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Environment8, ICoreWebView2_3, ICoreWebView2_8,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
    },
    TrySuspendCompletedHandler, WebResourceRequestedEventHandler,
};
use windows::{
    core::{Interface, BOOL, HSTRING},
    Win32::{
        Foundation::{CloseHandle, HWND},
        System::{
            Com::IStream,
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX},
            Threading::{GetCurrentProcessId, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        },
        UI::WindowsAndMessaging::{SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE},
    },
};
use wry::{WebView, WebViewExtWindows};

/// Bloque les domaines publicitaires / traqueurs. Le filtrage par motif est fait
/// par WebView2 côté navigateur : seules les requêtes bloquées remontent ici.
pub fn install_blocker(
    view: &WebView,
    domains: &[&str],
    enabled: Arc<AtomicBool>,
    counter: Arc<AtomicU64>,
) -> windows::core::Result<()> {
    unsafe {
        let wv = view.webview();
        let env = view.environment();
        for d in domains {
            for pat in [format!("*://{d}/*"), format!("*://*.{d}/*")] {
                wv.AddWebResourceRequestedFilter(&HSTRING::from(pat), COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL)?;
            }
        }
        let mut token = 0i64;
        wv.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                if let Some(args) = args {
                    if enabled.load(Ordering::Relaxed) {
                        let resp = env.CreateWebResourceResponse(
                            None::<&IStream>,
                            403,
                            &HSTRING::from("Blocked"),
                            &HSTRING::new(),
                        )?;
                        args.SetResponse(&resp)?;
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                }
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
