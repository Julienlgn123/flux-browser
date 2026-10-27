//! Téléchargements : suivi en direct (progression, vitesse, état) à la place de la fenêtre
//! par défaut de WebView2, avec pause / reprise / annulation et ouverture des fichiers.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::Path,
    rc::Rc,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tao::event_loop::EventLoopProxy;
use webview2_com::{
    take_pwstr, BytesReceivedChangedEventHandler, DownloadStartingEventHandler,
    Microsoft::Web::WebView2::Win32::*,
    StateChangedEventHandler,
};
use windows::{
    core::{Interface, BOOL, HSTRING, PWSTR},
    Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
};
use wry::{WebView, WebViewExtWindows};

use crate::UserEvent;

/// Fréquence maximale des mises à jour de progression envoyées à l'interface.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

#[derive(Serialize, Deserialize, Clone)]
pub struct Item {
    pub id: u32,
    pub url: String,
    pub path: String,
    pub name: String,
    /// Taille totale en octets (0 si inconnue).
    pub total: u64,
    pub received: u64,
    /// "progress", "paused", "done", "cancelled" ou "failed".
    pub state: String,
    pub reason: String,
    pub started: u64,
    /// Onglet d'origine : sa vue doit rester en vie tant que le téléchargement tourne.
    #[serde(skip)]
    pub tab: u32,
}

impl Item {
    pub fn active(&self) -> bool {
        matches!(self.state.as_str(), "progress" | "paused")
    }
}

/// Opérations en cours, partagées entre les vues (thread principal uniquement).
pub type Ops = Rc<RefCell<HashMap<u32, ICoreWebView2DownloadOperation>>>;

unsafe fn read(op: &ICoreWebView2DownloadOperation) -> (u64, u64, String) {
    let (mut total, mut received) = (0i64, 0i64);
    let _ = op.TotalBytesToReceive(&mut total);
    let _ = op.BytesReceived(&mut received);
    let mut state = COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS;
    let _ = op.State(&mut state);
    let state = match state {
        COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED => "done",
        COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED => {
            let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE;
            let _ = op.InterruptReason(&mut reason);
            let mut can = BOOL(0);
            let _ = op.CanResume(&mut can);
            if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED {
                "cancelled"
            } else if reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED || can.as_bool() {
                "paused"
            } else {
                "failed"
            }
        }
        _ => "progress",
    };
    (total.max(0) as u64, received.max(0) as u64, state.to_string())
}

fn reason_text(op: &ICoreWebView2DownloadOperation) -> String {
    let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE;
    unsafe {
        let _ = op.InterruptReason(&mut reason);
    }
    match reason {
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_FAILED
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_TIMEOUT
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_DISCONNECTED => "Erreur réseau",
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_NO_SPACE => "Disque plein",
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_ACCESS_DENIED => "Accès refusé",
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_BLOCKED_BY_POLICY | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_SECURITY_CHECK_FAILED => "Bloqué par Windows",
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_MALICIOUS => "Fichier dangereux détecté",
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FAILED
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_NO_RANGE
        | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_BAD_CONTENT => "Erreur du serveur",
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNAUTHORIZED | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FORBIDDEN => {
            "Accès refusé par le site"
        }
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_SHUTDOWN => "Navigateur fermé",
        _ => "Échec",
    }
    .to_string()
}

/// Intercepte les téléchargements de la vue : la fenêtre par défaut de WebView2 est
/// remplacée par le menu de Flux, alimenté par des événements `UserEvent::Download`.
pub fn install(view: &WebView, tab: u32, ops: Ops, next_id: Rc<Cell<u32>>, proxy: EventLoopProxy<UserEvent>) {
    unsafe {
        let Ok(wv4) = view.webview().cast::<ICoreWebView2_4>() else { return };
        let mut token = 0i64;
        let _ = wv4.add_DownloadStarting(
            &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                args.SetHandled(true)?;
                let op = args.DownloadOperation()?;
                let id = next_id.get();
                next_id.set(id + 1);
                ops.borrow_mut().insert(id, op.clone());

                let mut uri = PWSTR::null();
                op.Uri(&mut uri)?;
                let mut path = PWSTR::null();
                op.ResultFilePath(&mut path)?;
                let (url, path) = (take_pwstr(uri), take_pwstr(path));
                let name = Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let (total, received, _) = read(&op);
                let item = Item {
                    id, url, path, name, total, received,
                    state: "progress".into(), reason: String::new(), started: crate::store::now(), tab,
                };
                let _ = proxy.send_event(UserEvent::Download(json!({ "kind": "start", "item": item, "tab": tab })));

                let last = Rc::new(Cell::new(Instant::now() - PROGRESS_EVERY));
                let p = proxy.clone();
                let mut t = 0i64;
                op.add_BytesReceivedChanged(
                    &BytesReceivedChangedEventHandler::create(Box::new(move |op, _| {
                        if last.get().elapsed() < PROGRESS_EVERY {
                            return Ok(());
                        }
                        last.set(Instant::now());
                        if let Some(op) = op {
                            let (total, received, _) = read(&op);
                            let _ = p.send_event(UserEvent::Download(json!({ "kind": "progress", "id": id, "total": total, "received": received })));
                        }
                        Ok(())
                    })),
                    &mut t,
                )?;
                let p = proxy.clone();
                op.add_StateChanged(
                    &StateChangedEventHandler::create(Box::new(move |op, _| {
                        if let Some(op) = op {
                            let (total, received, state) = read(&op);
                            let reason = if state == "failed" { reason_text(&op) } else { String::new() };
                            let _ = p.send_event(UserEvent::Download(json!({
                                "kind": "state", "id": id, "total": total, "received": received, "state": state, "reason": reason,
                            })));
                        }
                        Ok(())
                    })),
                    &mut t,
                )?;
                Ok(())
            })),
            &mut token,
        );
    }
}

pub fn pause(op: &ICoreWebView2DownloadOperation) {
    unsafe {
        let _ = op.Pause();
    }
}

pub fn resume(op: &ICoreWebView2DownloadOperation) {
    unsafe {
        let _ = op.Resume();
    }
}

pub fn cancel(op: &ICoreWebView2DownloadOperation) {
    unsafe {
        let _ = op.Cancel();
    }
}

/// Ouvre le fichier avec l'application associée par Windows.
pub fn open(path: &str) {
    unsafe {
        ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from(path), None, None, SW_SHOWNORMAL);
    }
}

/// Ouvre l'Explorateur sur le dossier du fichier, fichier sélectionné.
pub fn show_in_folder(path: &str) {
    unsafe {
        ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &HSTRING::from("explorer.exe"),
            &HSTRING::from(format!("/select,\"{path}\"")),
            None,
            SW_SHOWNORMAL,
        );
    }
}

/// Dossier Téléchargements de l'utilisateur.
pub fn open_folder(hint: Option<&str>) {
    let dir = hint
        .and_then(|p| Path::new(p).parent().map(|d| d.to_string_lossy().into_owned()))
        .or_else(|| std::env::var("USERPROFILE").ok().map(|u| format!("{u}\\Downloads")))
        .unwrap_or_default();
    open(&dir);
}
