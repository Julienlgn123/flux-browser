//! Historique, session et réglages, dans %LOCALAPPDATA%\Flux.
//! L'historique et la session sont chiffrés (DPAPI, voir `secure`).

use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::secure;

const MAX_HISTORY: usize = 5000;

#[derive(Serialize, Deserialize, Clone)]
pub struct Visit {
    pub url: String,
    pub title: String,
    pub visits: u32,
    pub last: u64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    // Recherche
    pub engine: String,
    /// Moteur personnalisé, `%s` = la requête.
    pub custom_engine: String,
    pub suggestions: bool,
    /// Bangs personnels, une ligne par bang : `clé=https://…%s`.
    pub bangs: String,

    // Apparence
    /// "dark", "light" ou "system".
    pub theme: String,
    pub accent: String,
    /// "compact", "normal" ou "comfort".
    pub density: String,
    pub show_nav: bool,
    pub show_stats: bool,
    pub show_shield: bool,
    /// Fond de la page nouvel onglet : couleur CSS ou adresse d'image.
    pub ntp_bg: String,
    /// Fichier de fond choisi par l'utilisateur (image, GIF ou vidéo), copié dans le dossier Flux.
    pub ntp_media: String,
    /// Assombrissement du fond, en % (lisibilité de la barre de recherche).
    pub ntp_dim: u32,
    pub show_favbar: bool,
    /// Feuille de style appliquée à l'interface (personnalisation libre).
    pub custom_css: String,
    /// Zoom par défaut des pages, en %.
    pub zoom: u32,

    // Démarrage et arrière-plan
    /// "restore", "newtab" ou "home".
    pub startup: String,
    pub home_url: String,
    /// Raccourci global de la barre de recherche, ex. "Ctrl+Shift+X".
    pub hotkey: String,
    /// Fermer la fenêtre laisse Flux dans la zone de notification.
    pub background: bool,
    pub autostart: bool,

    // Comptes et vie privée
    pub passwords: bool,
    pub autofill: bool,
    /// "off", "basic", "balanced" ou "strict".
    pub tracking: String,
    /// Refuse automatiquement les bannières de cookies (Google, YouTube, Didomi, OneTrust…).
    pub reject_cookies: bool,
    /// Domaines dont les cookies sont refusés (jamais envoyés, supprimés dès qu'ils apparaissent).
    pub cookie_block: Vec<String>,
    /// DNS chiffré (DNS-over-HTTPS) : "off", "cloudflare", "google" ou "quad9" (redémarrage requis).
    pub dns: String,
    pub blocker: bool,

    // Consommation
    /// Minutes avant gel d'un onglet caché.
    pub sleep_min: u64,
    /// Minutes avant déchargement complet (0 = jamais).
    pub discard_min: u64,
    /// Limite de mémoire totale en Mo (0 = aucune) : les onglets en arrière-plan
    /// les plus anciens sont déchargés au-delà.
    pub ram_limit: u64,
    /// Plafond processeur en % de la machine (0 = aucun).
    pub cpu_limit: u32,
    /// Débit maximal par onglet en Ko/s (0 = illimité).
    pub net_limit: u64,
    /// Limite le nombre de processus de rendu (redémarrage requis).
    pub eco: bool,
    /// Arguments supplémentaires passés au moteur Chromium (redémarrage requis).
    pub extra_args: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            engine: "google".into(),
            custom_engine: String::new(),
            suggestions: true,
            bangs: String::new(),
            theme: "dark".into(),
            accent: "#ff5b2e".into(),
            density: "normal".into(),
            show_nav: true,
            show_stats: true,
            show_shield: true,
            ntp_bg: String::new(),
            ntp_media: String::new(),
            ntp_dim: 25,
            show_favbar: true,
            custom_css: String::new(),
            zoom: 100,
            startup: "restore".into(),
            home_url: String::new(),
            hotkey: "Ctrl+Shift+X".into(),
            background: true,
            autostart: true,
            passwords: true,
            autofill: true,
            tracking: "balanced".into(),
            reject_cookies: true,
            cookie_block: Vec::new(),
            dns: "cloudflare".into(),
            blocker: true,
            sleep_min: 3,
            discard_min: 30,
            ram_limit: 0,
            cpu_limit: 0,
            net_limit: 0,
            eco: false,
            extra_args: String::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Session {
    pub tabs: Vec<SessionTab>,
    pub active: usize,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SessionTab {
    pub url: String,
    pub title: String,
}

pub struct Store {
    pub dir: PathBuf,
    pub history: HashMap<String, Visit>,
    pub bookmarks: Vec<Bookmark>,
    /// Téléchargements récents (100 au plus), du plus récent au plus ancien.
    pub downloads: Vec<crate::downloads::Item>,
    pub settings: Settings,
    pub dirty: bool,
}

/// Profil séparé (variable FLUX_PROFILE) : données et instance indépendantes, pour les tests.
pub fn profile_suffix() -> String {
    std::env::var("FLUX_PROFILE").map(|p| format!("-{p}")).unwrap_or_default()
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn load<T: DeserializeOwned + Default>(path: PathBuf) -> T {
    fs::read(path)
        .ok()
        .and_then(|b| secure::decrypt(&b))
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save<T: Serialize>(path: PathBuf, value: &T, encrypted: bool) {
    let Ok(mut bytes) = serde_json::to_vec(value) else { return };
    if encrypted {
        match secure::encrypt(&bytes) {
            Some(c) => bytes = c,
            None => return,
        }
    }
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, bytes).is_ok() {
        let _ = fs::rename(tmp, path);
    }
}

/// Score « fréquence × récence », le même que celui de l'omnibox.
pub fn frecency(v: &Visit, now: u64) -> f64 {
    let days = now.saturating_sub(v.last) as f64 / 86400.0;
    v.visits as f64 / (1.0 + days / 7.0)
}

impl Store {
    pub fn open() -> Self {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let dir = base.join(format!("Flux{}", profile_suffix()));
        let _ = fs::create_dir_all(&dir);
        let list: Vec<Visit> = load(dir.join("history.json"));
        let settings = load(dir.join("settings.json"));
        let history = list.into_iter().map(|v| (v.url.clone(), v)).collect();
        let bookmarks = load(dir.join("bookmarks.json"));
        let mut downloads: Vec<crate::downloads::Item> = load(dir.join("downloads.json"));
        // Un téléchargement resté « en cours » vient d'une session précédente : il a été interrompu.
        for d in downloads.iter_mut().filter(|d| d.active()) {
            d.state = "failed".into();
            d.reason = "Interrompu".into();
        }
        Self { dir, history, bookmarks, downloads, settings, dirty: true }
    }

    pub fn webview_dir(&self) -> PathBuf {
        self.dir.join("WebView2")
    }

    pub fn record(&mut self, url: &str, title: &str) -> Option<Visit> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return None;
        }
        let v = self.history.entry(url.to_string()).or_insert_with(|| Visit {
            url: url.to_string(),
            title: String::new(),
            visits: 0,
            last: 0,
        });
        v.visits += 1;
        v.last = now();
        if !title.is_empty() {
            v.title = title.to_string();
        }
        self.dirty = true;
        Some(v.clone())
    }

    pub fn retitle(&mut self, url: &str, title: &str) -> Option<Visit> {
        let v = self.history.get_mut(url)?;
        if v.title == title {
            return None;
        }
        v.title = title.to_string();
        self.dirty = true;
        Some(v.clone())
    }

    /// Entrées triées par score décroissant (pour l'omnibox).
    pub fn ranked(&self, limit: usize) -> Vec<Visit> {
        let t = now();
        let mut list: Vec<&Visit> = self.history.values().collect();
        list.sort_by(|a, b| frecency(b, t).total_cmp(&frecency(a, t)));
        list.into_iter().take(limit).cloned().collect()
    }

    /// Tout l'historique, de la visite la plus récente à la plus ancienne.
    pub fn chronological(&self) -> Vec<Visit> {
        let mut list: Vec<Visit> = self.history.values().cloned().collect();
        list.sort_by(|a, b| b.last.cmp(&a.last));
        list
    }

    pub fn forget(&mut self, url: &str) {
        if self.history.remove(url).is_some() {
            self.dirty = true;
        }
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.dirty = true;
    }

    pub fn load_session(&self) -> Session {
        load(self.dir.join("session.json"))
    }

    pub fn save_session(&self, s: &Session) {
        save(self.dir.join("session.json"), s, true);
    }

    pub fn save_downloads(&mut self) {
        self.downloads.truncate(100);
        save(self.dir.join("downloads.json"), &self.downloads, true);
    }

    pub fn save_bookmarks(&self) {
        save(self.dir.join("bookmarks.json"), &self.bookmarks, true);
    }

    pub fn save_settings(&self) {
        save(self.dir.join("settings.json"), &self.settings, false);
    }

    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        if self.history.len() > MAX_HISTORY {
            let keep: Vec<Visit> = self.ranked(MAX_HISTORY);
            self.history = keep.into_iter().map(|v| (v.url.clone(), v)).collect();
        }
        let list: Vec<&Visit> = self.history.values().collect();
        save(self.dir.join("history.json"), &list, true);
        self.dirty = false;
    }
}
