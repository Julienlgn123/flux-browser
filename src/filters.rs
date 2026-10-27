//! Listes de filtrage EasyList / uBlock Origin, compilées par adblock-rust (moteur de Brave).
//!
//! Les listes sont téléchargées et compilées dans un thread à part, puis le moteur
//! sérialisé est envoyé au thread principal. Au démarrage, le moteur en cache
//! (`engine.dat`) se recharge en quelques millisecondes, sans réseau.

use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime},
};

use adblock::{lists::ParseOptions, resources::Resource, Engine, FilterSet};
use serde::{Deserialize, Serialize};

pub const UPDATE_EVERY: Duration = Duration::from_secs(24 * 3600);

pub const LISTS: &[(&str, &str)] = &[
    ("easylist", "https://easylist.to/easylist/easylist.txt"),
    ("easyprivacy", "https://easylist.to/easylist/easyprivacy.txt"),
    ("liste-fr", "https://easylist-downloads.adblockplus.org/liste_fr.txt"),
    ("ublock-filters", "https://ublockorigin.github.io/uAssets/filters/filters.txt"),
    ("ublock-privacy", "https://ublockorigin.github.io/uAssets/filters/privacy.txt"),
    ("ublock-badware", "https://ublockorigin.github.io/uAssets/filters/badware.txt"),
    ("ublock-unbreak", "https://ublockorigin.github.io/uAssets/filters/unbreak.txt"),
    ("ublock-quick-fixes", "https://ublockorigin.github.io/uAssets/filters/quick-fixes.txt"),
    ("peter-lowe", "https://pgl.yoyo.org/adservers/serverlist.php?hostformat=adblockplus&showintro=0&mimetype=plaintext"),
];

/// Scriptlets et ressources de redirection (format attendu par adblock-rust).
const RESOURCES_URL: &str = "https://raw.githubusercontent.com/brave/adblock-resources/master/dist/resources.json";

/// Liste minimale intégrée, utilisée tant que les listes complètes n'ont jamais été téléchargées.
const BOOTSTRAP: &str = include_str!("blocklist.txt");

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Meta {
    /// Secondes UNIX de la dernière compilation réussie.
    pub updated: u64,
    pub rules: usize,
    pub lists: usize,
}

pub struct Compiled {
    pub engine: Vec<u8>,
    pub resources: String,
    pub meta: Meta,
}

fn dir(base: &Path) -> PathBuf {
    base.join("filters")
}

fn resources(text: &str) -> Vec<Resource> {
    serde_json::from_str(text).unwrap_or_default()
}

/// Moteur prêt à l'emploi : cache compilé s'il existe, sinon la liste intégrée.
pub fn load(base: &Path) -> (Engine, Meta) {
    let d = dir(base);
    let meta: Meta = fs::read(d.join("meta.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    if let Ok(bytes) = fs::read(d.join("engine.dat")) {
        let mut engine = Engine::default();
        if engine.deserialize(&bytes).is_ok() {
            if let Ok(r) = fs::read_to_string(d.join("resources.json")) {
                engine.use_resources(resources(&r));
            }
            return (engine, meta);
        }
    }
    let rules: Vec<String> = BOOTSTRAP
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|d| format!("||{d}^"))
        .collect();
    let mut set = FilterSet::new(false);
    set.add_filter_list(rules.join("\n"), ParseOptions::default());
    (Engine::new_with_filter_set(set), Meta::default())
}

pub fn is_stale(meta: &Meta) -> bool {
    let age = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_sub(meta.updated);
    meta.updated == 0 || age >= UPDATE_EVERY.as_secs()
}

/// Télécharge et compile toutes les listes. Une liste injoignable est remplacée
/// par sa dernière copie locale ; `None` si rien n'est disponible.
pub fn compile(base: &Path) -> Option<Compiled> {
    let d = dir(base);
    let _ = fs::create_dir_all(&d);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let fetch = |url: &str| -> Option<String> { agent.get(url).call().ok()?.body_mut().with_config().limit(32 << 20).read_to_string().ok() };

    // Téléchargements en parallèle : le temps total est celui de la liste la plus lente.
    let results: Vec<(String, Option<String>)> = thread::scope(|s| {
        let handles: Vec<_> = LISTS
            .iter()
            .map(|(name, url)| (name, s.spawn(|| fetch(url))))
            .collect();
        handles.into_iter().map(|(name, h)| (name.to_string(), h.join().ok().flatten())).collect()
    });

    let mut set = FilterSet::new(false);
    let mut rules = 0;
    let mut lists = 0;
    for (name, fresh) in results {
        let path = d.join(format!("{name}.txt"));
        let text = match fresh.filter(|t| t.len() > 100) {
            Some(t) => {
                let _ = fs::write(&path, &t);
                t
            }
            None => match fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => continue,
            },
        };
        rules += text.lines().filter(|l| !l.is_empty() && !l.starts_with('!') && !l.starts_with('[')).count();
        lists += 1;
        set.add_filter_list(text, ParseOptions::default());
    }
    if lists == 0 {
        return None;
    }

    let resources_text = match fetch(RESOURCES_URL).filter(|t| t.starts_with('[')) {
        Some(t) => {
            let _ = fs::write(d.join("resources.json"), &t);
            t
        }
        None => fs::read_to_string(d.join("resources.json")).unwrap_or_else(|_| "[]".into()),
    };

    let engine = Engine::new_with_filter_set(set).serialize();
    let meta = Meta { updated: crate::store::now(), rules, lists };
    let _ = fs::write(d.join("engine.dat"), &engine);
    let _ = fs::write(d.join("meta.json"), serde_json::to_vec(&meta).unwrap_or_default());
    Some(Compiled { engine, resources: resources_text, meta })
}

/// Reconstruit le moteur à partir du résultat de [`compile`] (thread principal).
pub fn materialize(c: &Compiled) -> Option<Engine> {
    let mut engine = Engine::default();
    engine.deserialize(&c.engine).ok()?;
    engine.use_resources(resources(&c.resources));
    Some(engine)
}

/// Feuille de style qui masque les sélecteurs donnés. Une règle par sélecteur :
/// un sélecteur invalide n'annule ainsi que sa propre règle.
pub fn hide_css<'a>(selectors: impl IntoIterator<Item = &'a String>) -> String {
    let mut css = String::new();
    for s in selectors {
        css.push_str(s);
        css.push_str("{display:none!important}\n");
    }
    css
}
