//! Intégration Windows : raccourci clavier global, instance unique, lancement au démarrage.

use std::{
    io::{Read, Write},
    sync::mpsc::{channel, Sender},
    thread,
};

use windows::{
    core::HSTRING,
    Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{ReadFile, PIPE_ACCESS_INBOUND},
        System::{
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
                PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
            },
            Registry::{RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ},
            Threading::GetCurrentThreadId,
        },
        UI::{
            Input::KeyboardAndMouse::*,
            WindowsAndMessaging::{GetMessageW, PeekMessageW, PostThreadMessageW, MSG, PM_NOREMOVE, WM_APP, WM_HOTKEY},
        },
    },
};

/* ---------- raccourci global ---------- */

/// Convertit « Ctrl+Alt+Space » en modificateurs + touche virtuelle Windows.
pub fn parse_hotkey(spec: &str) -> Option<(HOT_KEY_MODIFIERS, u32)> {
    let mut mods = MOD_NOREPEAT;
    let mut key = None;
    for part in spec.split('+').map(|p| p.trim().to_ascii_lowercase()) {
        match part.as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" | "maj" => mods |= MOD_SHIFT,
            "win" => mods |= MOD_WIN,
            "space" | "espace" => key = Some(VK_SPACE.0 as u32),
            "enter" | "entrée" => key = Some(VK_RETURN.0 as u32),
            "tab" => key = Some(VK_TAB.0 as u32),
            p if p.len() == 1 && p.as_bytes()[0].is_ascii_alphanumeric() => {
                key = Some(p.to_ascii_uppercase().as_bytes()[0] as u32)
            }
            p if p.starts_with('f') && p[1..].parse::<u32>().is_ok_and(|n| (1..=24).contains(&n)) => {
                key = Some(VK_F1.0 as u32 + p[1..].parse::<u32>().unwrap() - 1)
            }
            _ => return None,
        }
    }
    // Un raccourci global sans modificateur volerait une touche à toutes les applications.
    if mods == MOD_NOREPEAT {
        return None;
    }
    key.map(|k| (mods, k))
}

pub struct Hotkey {
    thread_id: u32,
    tx: Sender<String>,
}

impl Hotkey {
    /// Fil dédié : Windows envoie WM_HOTKEY à la file de messages du fil qui a enregistré.
    pub fn spawn(
        initial: String,
        pressed: impl Fn() + Send + 'static,
        status: impl Fn(bool, String) + Send + 'static,
    ) -> Self {
        let (tx, rx) = channel::<String>();
        let (id_tx, id_rx) = channel();
        thread::spawn(move || unsafe {
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE); // crée la file de messages
            let _ = id_tx.send(GetCurrentThreadId());
            let register = |spec: &str| {
                let _ = UnregisterHotKey(None, 1);
                let ok = parse_hotkey(spec).is_some_and(|(m, k)| RegisterHotKey(None, 1, m, k).is_ok());
                status(ok, spec.to_string());
            };
            register(&initial);
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                match msg.message {
                    WM_HOTKEY => pressed(),
                    m if m == WM_APP => {
                        if let Some(spec) = rx.try_iter().last() {
                            register(&spec);
                        }
                    }
                    _ => {}
                }
            }
        });
        Self { thread_id: id_rx.recv().unwrap_or(0), tx }
    }

    pub fn change(&self, spec: &str) {
        let _ = self.tx.send(spec.to_string());
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_APP, Default::default(), Default::default());
        }
    }
}

/* ---------- instance unique ---------- */

fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    format!(r"\\.\pipe\flux-browser-{user}{}", crate::store::profile_suffix())
}

/// Si Flux tourne déjà, lui transmet la commande et renvoie `true`.
pub fn forward_to_running(command: &str) -> bool {
    match std::fs::OpenOptions::new().write(true).open(pipe_name()) {
        Ok(mut f) => f.write_all(command.as_bytes()).is_ok(),
        Err(_) => false,
    }
}

/// Reçoit les commandes des lancements suivants (raccourci, barre des tâches, lien).
pub fn listen(on_command: impl Fn(String) + Send + 'static) {
    let name = HSTRING::from(pipe_name());
    thread::spawn(move || loop {
        let pipe = unsafe {
            CreateNamedPipeW(
                &name,
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                0,
                4096,
                0,
                None,
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            return;
        }
        // Un client déjà connecté avant l'appel renvoie ERROR_PIPE_CONNECTED : c'est un succès.
        let _ = unsafe { ConnectNamedPipe(pipe, None) };
        let text = read_all(pipe);
        unsafe {
            let _ = DisconnectNamedPipe(pipe);
            let _ = CloseHandle(pipe);
        }
        if !text.is_empty() {
            on_command(text);
        }
    });
}

fn read_all(pipe: HANDLE) -> String {
    struct P(HANDLE);
    impl Read for P {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut n = 0u32;
            match unsafe { ReadFile(self.0, Some(buf), Some(&mut n), None) } {
                Ok(()) => Ok(n as usize),
                Err(_) => Ok(0),
            }
        }
    }
    let mut s = String::new();
    let _ = P(pipe).take(64 * 1024).read_to_string(&mut s);
    s
}

/* ---------- lancement au démarrage ---------- */

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

pub fn set_autostart(enabled: bool) {
    // Un profil de test ne doit jamais remplacer le lancement au démarrage du vrai Flux.
    if !crate::store::profile_suffix().is_empty() {
        return;
    }
    unsafe {
        if enabled {
            let Ok(exe) = std::env::current_exe() else { return };
            let cmd: Vec<u16> = format!("\"{}\" --background", exe.display()).encode_utf16().chain([0]).collect();
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(RUN_KEY),
                &HSTRING::from("Flux"),
                REG_SZ.0,
                Some(cmd.as_ptr() as *const _),
                (cmd.len() * 2) as u32,
            );
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(RUN_KEY), &HSTRING::from("Flux"));
        }
    }
}

/* ---------- choix d'un fichier de fond ---------- */

/// Boîte de dialogue Windows « Ouvrir » limitée aux images, GIF et vidéos.
pub fn pick_media(owner: isize) -> Option<std::path::PathBuf> {
    use windows::{
        core::{PCWSTR, PWSTR},
        Win32::{
            Foundation::HWND,
            UI::Controls::Dialogs::{GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW},
        },
    };
    let filter: Vec<u16> = "Images, GIF et vidéos\0*.png;*.jpg;*.jpeg;*.gif;*.webp;*.avif;*.bmp;*.mp4;*.webm;*.mov;*.m4v\0Tous les fichiers\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "Choisir le fond de la page d'accueil\0".encode_utf16().collect();
    let mut file = vec![0u16; 4096];
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: HWND(owner as _),
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut ofn) }.as_bool() {
        return None;
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&file[..len]).into())
}
