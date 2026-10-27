//! Historique, session et réglages, persistés en JSON dans %LOCALAPPDATA%\Flux.

use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

const MAX_HISTORY: usize = 5000;

#[derive(Serialize, Deserialize, Clone)]
pub struct Visit {
    pub url: String,
    pub title: String,
    pub visits: u32,
    pub last: u64,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub engine: String,
    pub suggestions: bool,
    pub blocker: bool,
    /// Minutes avant gel d'un onglet caché.
    pub sleep_min: u64,
    /// Minutes avant déchargement complet (0 = jamais).
    pub discard_min: u64,
    /// Limite le nombre de processus de rendu (redémarrage requis).
    pub eco: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { engine: "google".into(), suggestions: true, blocker: true, sleep_min: 5, discard_min: 60, eco: false }
    }
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
    pub settings: Settings,
    pub dirty: bool,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn load<T: DeserializeOwned + Default>(path: PathBuf) -> T {
    fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save<T: Serialize>(path: PathBuf, value: &T) {
    if let Ok(b) = serde_json::to_vec(value) {
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, b).is_ok() {
            let _ = fs::rename(tmp, path);
        }
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
        let dir = base.join("Flux");
        let _ = fs::create_dir_all(&dir);
        let list: Vec<Visit> = load(dir.join("history.json"));
        let settings = load(dir.join("settings.json"));
        let history = list.into_iter().map(|v| (v.url.clone(), v)).collect();
        Self { dir, history, settings, dirty: false }
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

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.dirty = true;
    }

    pub fn load_session(&self) -> Session {
        load(self.dir.join("session.json"))
    }

    pub fn save_session(&self, s: &Session) {
        save(self.dir.join("session.json"), s);
    }

    pub fn save_settings(&self) {
        save(self.dir.join("settings.json"), &self.settings);
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
        save(self.dir.join("history.json"), &list);
        self.dirty = false;
    }
}
