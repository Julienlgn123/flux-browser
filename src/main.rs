#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod filters;
mod media;
mod monitor;
mod quickbar;
mod secure;
mod store;
mod suggest;
mod system;
mod wv2;

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{channel, Sender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use adblock::Engine;
use serde::Deserialize;
use serde_json::json;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::windows::{IconExtWindows, WindowExtWindows},
    window::{Icon, Theme, Window, WindowBuilder},
};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use wry::{
    dpi::{LogicalPosition, LogicalSize as WryLogicalSize},
    MemoryUsageLevel, NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder,
    WebViewBuilderExtWindows, WebViewExtWindows,
};

use quickbar::QuickBar;
use store::{Bookmark, Session, SessionTab, Settings, Store};

const TICK: Duration = Duration::from_secs(1);
/// Un onglet caché passe en mode mémoire réduite après ce délai.
const LOW_MEMORY_AFTER: Duration = Duration::from_secs(15);
const MB: u64 = 1 << 20;

const UI_HTML: &str = include_str!("../ui/index.html");
const BAR_HTML: &str = include_str!("../ui/bar.html");
/// Logique de recherche partagée par la barre d'adresse et la barre rapide.
const OMNI_JS: &str = include_str!("../ui/omni.js");
/// Refus automatique des bannières de cookies, injecté dans chaque page et cadre.
const CONSENT_JS: &str = include_str!("../ui/consent.js");

/// Raccourcis clavier quand le focus est dans une page web.
const PAGE_JS: &str = r#"
addEventListener('keydown', e => {
  const k = e.key.toLowerCase(), c = e.ctrlKey || e.metaKey;
  let cmd = null;
  if (c && !e.shiftKey && k === 't') cmd = 'new';
  else if (c && e.shiftKey && k === 't') cmd = 'reopen';
  else if (c && k === 'w') cmd = 'close';
  else if ((c && k === 'l') || (e.altKey && k === 'd') || k === 'f6') cmd = 'focus';
  else if (c && k === 'tab') cmd = e.shiftKey ? 'prev' : 'next';
  else if (c && k >= '1' && k <= '9') cmd = 'n' + k;
  if (cmd) { e.preventDefault(); e.stopPropagation(); window.ipc.postMessage(cmd); }
}, true);
"#;

/// Filtrage cosmétique : masque les emplacements publicitaires (règles `##` des listes).
/// Les règles propres au site arrivent dès la création du document ; les règles génériques
/// sont demandées pour les classes / id réellement présents dans la page.
const COSMETIC_JS: &str = r#"
(() => {
  if (window.top !== window || !/^https?:$/.test(location.protocol)) return;
  const post = m => window.ipc.postMessage(JSON.stringify(m));
  const sheet = new CSSStyleSheet();
  let css = '', attached = false;
  const seenC = new Set(), seenI = new Set();
  let pc = [], pi = [], timer = null;
  const collect = el => {
    if (el.id && !seenI.has(el.id)) { seenI.add(el.id); pi.push(el.id); }
    if (el.classList) for (const c of el.classList) if (!seenC.has(c)) { seenC.add(c); pc.push(c); }
  };
  const flush = () => { timer = null; if (pc.length || pi.length) { post({ c: 'ids', classes: pc, ids: pi }); pc = []; pi = []; } };
  const queue = () => { if (!timer) timer = setTimeout(flush, 100); };
  const scan = () => {
    document.querySelectorAll('[id],[class]').forEach(collect); queue();
    new MutationObserver(ms => {
      for (const m of ms) {
        if (m.type === 'attributes') collect(m.target);
        else for (const n of m.addedNodes) if (n.nodeType === 1) { collect(n); n.querySelectorAll('[id],[class]').forEach(collect); }
      }
      queue();
    }).observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ['id', 'class'] });
  };
  window.__fluxHide = (more, generic) => {
    if (more) {
      css += more;
      sheet.replaceSync(css);
      if (!attached) { attached = true; document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet]; }
    }
    if (generic) document.readyState === 'loading' ? addEventListener('DOMContentLoaded', scan, { once: true }) : scan();
  };
  post({ c: 'cosm', url: location.href });
})();
"#;

pub enum UserEvent {
    Ui(String),
    Bar(String),
    Page(u32, String),
    Title(u32, String),
    Load(u32, bool, String),
    OpenTab(String),
    Suggest(bool, u64, Vec<String>),
    Tick,
    Procs(Vec<wv2::ProcInfo>),
    Metrics(u32, f64, f64),
    FiltersBusy,
    Filters(Option<filters::Compiled>),
    Hotkey,
    HotkeyStatus(bool, String),
    /// Commande d'un autre lancement de Flux (instance unique).
    Command(String),
    TrayOpen,
    MenuItem(String),
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum UiMsg {
    Ready,
    New { url: Option<String> },
    Close { id: u32 },
    Activate { id: u32 },
    Go { url: String },
    Back,
    Forward,
    Reload,
    Stop,
    Height { h: Option<f64> },
    Bar { h: f64 },
    Suggest { gen: u64, q: String },
    Settings { s: Settings },
    ClearHistory,
    ClearData { what: String },
    Reopen,
    Devtools,
    FocusPage,
    UpdateFilters,
    Freeze { id: u32 },
    Discard { id: u32 },
    QuickBar,
    Bookmarks { items: Vec<Bookmark> },
    PickBackground,
    ClearBackground,
    History,
    HistoryDelete { url: String },
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum BarMsg {
    Ready,
    Go { url: String },
    Height { h: f64 },
    Suggest { gen: u64, q: String },
    Hide,
}

#[derive(Deserialize)]
#[serde(tag = "c", rename_all = "snake_case")]
enum PageMsg {
    Cosm { url: String },
    Ids { classes: Vec<String>, ids: Vec<String> },
}

struct Tab {
    id: u32,
    view: Option<WebView>,
    /// Adresse du document principal, partagée avec le bloqueur réseau.
    page_url: Rc<RefCell<String>>,
    /// Exceptions cosmétiques (`#@#`) du site courant.
    cosmetic_exceptions: HashSet<String>,
    /// Octets reçus depuis la création de la vue (compteur DevTools).
    received: Rc<Cell<u64>>,
    received_prev: u64,
    /// Tas JavaScript (octets) et temps processeur cumulé de la page, via DevTools.
    heap: f64,
    task_prev: Option<(f64, Instant)>,
    cpu: f64,
    url: String,
    title: String,
    loading: bool,
    hidden_since: Instant,
    low: bool,
    suspended: bool,
}

struct App {
    window: Window,
    /// Interface (onglets, barre d'adresse). N'existe que quand la fenêtre est affichée.
    ui: Option<WebView>,
    ui_html: String,
    bar: QuickBar,
    ctx: WebContext,
    proxy: EventLoopProxy<UserEvent>,
    tabs: Vec<Tab>,
    active: u32,
    next_id: u32,
    closed: Vec<SessionTab>,
    store: Store,
    args: String,
    engine: Rc<RefCell<Engine>>,
    filter_meta: filters::Meta,
    filters_busy: bool,
    update_filters: Sender<()>,
    blocker_on: Arc<AtomicBool>,
    blocked: Arc<AtomicU64>,
    suggest: Sender<suggest::Request>,
    hotkey: system::Hotkey,
    hotkey_status: (bool, String),
    sampler: monitor::Sampler,
    cpu_limiter: Option<monitor::CpuLimiter>,
    last_sample: Instant,
    /// Hauteur de la barre d'onglets + barre d'adresse (dépend de la densité choisie).
    bar_h: f64,
    ui_h: Option<f64>,
    /// La fenêtre principale est affichée (sinon Flux tourne en arrière-plan).
    shown: bool,
    minimized: bool,
    ticks: u64,
    _tray: Option<TrayIcon>,
}

fn strip_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

fn theme_of(s: &Settings) -> Option<Theme> {
    match s.theme.as_str() {
        "light" => Some(Theme::Light),
        "dark" => Some(Theme::Dark),
        _ => None,
    }
}

/// Arguments du moteur, communs à toutes les vues (ils doivent être identiques).
fn browser_args(s: &Settings) -> String {
    let mut features = vec!["ParallelDownloading", "HttpsUpgrades"];
    let doh = match s.dns.as_str() {
        "cloudflare" => Some("https://cloudflare-dns.com/dns-query"),
        "google" => Some("https://dns.google/dns-query{?dns}"),
        "quad9" => Some("https://dns.quad9.net/dns-query"),
        _ => None,
    };
    let doh_feature;
    if let Some(t) = doh {
        let escaped: String = t
            .bytes()
            .map(|b| match b {
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' => (b as char).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect();
        doh_feature = format!("DnsOverHttps:Fallback/true/Templates/{escaped}");
        features.push(&doh_feature);
    }
    let mut disabled = vec!["msWebOOUI", "msPdfOOUI", "msSmartScreenProtection", "Translate"];
    if s.eco {
        // Le cache « précédent / suivant » garde les pages quittées en mémoire.
        disabled.push("BackForwardCache");
    }
    // V8 privilégie la mémoire à la vitesse brute : environ −14 % de RAM sur YouTube,
    // sans différence perceptible.
    let mut args = format!(
        "--disable-features={} --enable-features={} --process-per-site --disable-background-networking \
         --no-pings --js-flags=--optimize-for-size",
        disabled.join(","),
        features.join(",")
    );
    if s.eco {
        args.push_str(" --renderer-process-limit=3 --enable-low-end-device-mode");
    }
    if !s.extra_args.trim().is_empty() {
        args.push(' ');
        args.push_str(s.extra_args.trim());
    }
    args
}

impl App {
    fn settings(&self) -> &Settings {
        &self.store.settings
    }

    fn logical_size(&self) -> (f64, f64) {
        let s = self.window.inner_size().to_logical::<f64>(self.window.scale_factor());
        (s.width, s.height)
    }

    fn content_rect(&self) -> Rect {
        let (w, h) = self.logical_size();
        Rect {
            position: LogicalPosition::new(0.0, self.bar_h).into(),
            size: WryLogicalSize::new(w, (h - self.bar_h).max(0.0)).into(),
        }
    }

    fn tab(&self, id: u32) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    fn tab_mut(&mut self, id: u32) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    fn active_view(&self) -> Option<&WebView> {
        self.tab(self.active).and_then(|t| t.view.as_ref())
    }

    fn views(&self) -> impl Iterator<Item = &WebView> {
        self.tabs.iter().filter_map(|t| t.view.as_ref())
    }

    fn layout(&self) {
        let (w, h) = self.logical_size();
        // Page vide (nouvel onglet) : l'interface occupe toute la fenêtre.
        let empty = self.active_view().is_none();
        let ui_h = if empty { h } else { self.ui_h.map_or(h, |x| x.min(h)) };
        if let Some(ui) = &self.ui {
            let _ = ui.set_bounds(Rect {
                position: LogicalPosition::new(0.0, 0.0).into(),
                size: WryLogicalSize::new(w, ui_h).into(),
            });
        }
        let rect = self.content_rect();
        for v in self.views() {
            let _ = v.set_bounds(rect);
        }
    }

    fn send(&self, msg: serde_json::Value) {
        if let Some(ui) = &self.ui {
            let _ = ui.evaluate_script(&format!("flux({msg})"));
        }
    }

    fn push_tabs(&self) {
        let tabs: Vec<_> = self
            .tabs
            .iter()
            .map(|t| {
                json!({
                    "id": t.id, "url": t.url, "title": t.title, "loading": t.loading,
                    "asleep": t.view.is_none() && !t.url.is_empty() || t.suspended,
                })
            })
            .collect();
        self.send(json!({ "t": "tabs", "tabs": tabs, "active": self.active }));
    }

    fn create_view(&mut self, id: u32, url: &str) -> WebView {
        let rect = self.content_rect();
        let p = self.proxy.clone();
        let (p1, p2, p3, p4) = (p.clone(), p.clone(), p.clone(), p);
        let tab = self.tab(id).expect("onglet existant");
        let (page_url, received) = (tab.page_url.clone(), tab.received.clone());
        *page_url.borrow_mut() = strip_fragment(url).to_string();
        let nav_url = page_url.clone();
        let consent = if self.settings().reject_cookies { CONSENT_JS } else { "" };
        let view = WebViewBuilder::new_with_web_context(&mut self.ctx)
            .with_url(url)
            .with_bounds(rect)
            .with_additional_browser_args(&self.args)
            .with_initialization_script_for_main_only(PAGE_JS, true)
            .with_initialization_script_for_main_only(COSMETIC_JS, true)
            .with_initialization_script_for_main_only(consent, false)
            .with_devtools(true)
            .with_navigation_handler(move |u| {
                *nav_url.borrow_mut() = strip_fragment(&u).to_string();
                true
            })
            .with_ipc_handler(move |r| {
                let _ = p1.send_event(UserEvent::Page(id, r.body().clone()));
            })
            .with_document_title_changed_handler(move |t| {
                let _ = p2.send_event(UserEvent::Title(id, t));
            })
            .with_on_page_load_handler(move |e, u| {
                let _ = p3.send_event(UserEvent::Load(id, matches!(e, PageLoadEvent::Started), u));
            })
            .with_new_window_req_handler(move |u, features| {
                // Les vraies pop-ups (taille imposée, ex. connexion Google ou Twitch) gardent leur
                // fenêtre pour conserver window.opener ; le reste s'ouvre dans un onglet.
                if features.size.is_some() {
                    NewWindowResponse::Allow
                } else {
                    let _ = p4.send_event(UserEvent::OpenTab(u));
                    NewWindowResponse::Deny
                }
            })
            .build_as_child(&self.window)
            .expect("création de la vue WebView2");
        let _ = wv2::install_blocker(
            &view,
            self.engine.clone(),
            page_url,
            self.blocker_on.clone(),
            self.blocked.clone(),
        );
        let s = self.settings();
        wv2::set_autofill(&view, s.passwords, s.autofill);
        wv2::watch_network(&view, received);
        if s.net_limit > 0 {
            wv2::set_net_limit(&view, s.net_limit);
        }
        if s.zoom != 100 {
            let _ = view.zoom(s.zoom as f64 / 100.0);
        }
        if let Some(ui) = &self.ui {
            wv2::raise(ui);
        }
        view
    }

    fn add_tab(&mut self, url: &str, title: &str) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab {
            id,
            view: None,
            page_url: Rc::default(),
            cosmetic_exceptions: HashSet::new(),
            received: Rc::default(),
            received_prev: 0,
            heap: 0.0,
            task_prev: None,
            cpu: 0.0,
            url: url.to_string(),
            title: title.to_string(),
            loading: false,
            hidden_since: Instant::now(),
            low: false,
            suspended: false,
        });
        id
    }

    fn activate(&mut self, id: u32) {
        if self.tab(id).is_none() {
            return;
        }
        let prev = self.active;
        if prev != id {
            if let Some(t) = self.tab_mut(prev) {
                if let Some(v) = &t.view {
                    let _ = v.set_visible(false);
                }
                t.hidden_since = Instant::now();
            }
        }
        self.active = id;
        let needs_view = self.shown && self.tab(id).is_some_and(|t| t.view.is_none() && !t.url.is_empty());
        if needs_view {
            let url = self.tab(id).unwrap().url.clone();
            let view = self.create_view(id, &url);
            let t = self.tab_mut(id).unwrap();
            t.view = Some(view);
            t.received_prev = 0;
            t.loading = true;
        }
        let t = self.tab_mut(id).unwrap();
        t.low = false;
        t.suspended = false;
        if let Some(v) = &t.view {
            let _ = v.set_memory_usage_level(MemoryUsageLevel::Normal);
            let _ = v.set_visible(true);
            let _ = v.focus();
        }
        self.layout();
        self.push_tabs();
    }

    fn new_tab(&mut self, url: Option<String>) {
        let url = url.unwrap_or_default();
        let id = self.add_tab(&url, "");
        self.activate(id);
        if url.is_empty() {
            if let Some(ui) = &self.ui {
                let _ = ui.focus();
            }
            self.send(json!({ "t": "focus" }));
        }
    }

    fn close_tab(&mut self, id: u32) {
        let Some(i) = self.tabs.iter().position(|t| t.id == id) else { return };
        let t = self.tabs.remove(i);
        if !t.url.is_empty() {
            self.closed.push(SessionTab { url: t.url, title: t.title });
        }
        drop(t.view);
        if self.tabs.is_empty() {
            self.new_tab(None);
        } else if self.active == id {
            let next = self.tabs[i.min(self.tabs.len() - 1)].id;
            self.activate(next);
        } else {
            self.push_tabs();
        }
    }

    /// Libère toute la mémoire d'un onglet ; il se recharge quand on y revient.
    fn discard(&mut self, id: u32) {
        if id == self.active {
            return;
        }
        let Some(t) = self.tab_mut(id) else { return };
        if let Some(v) = t.view.take() {
            if let Ok(u) = v.url() {
                t.url = u;
            }
        }
        t.suspended = false;
        t.received_prev = 0;
        self.push_tabs();
    }

    fn freeze(&mut self, id: u32) {
        if id == self.active {
            return;
        }
        let Some(t) = self.tab_mut(id) else { return };
        if let Some(v) = &t.view {
            let _ = v.set_memory_usage_level(MemoryUsageLevel::Low);
            wv2::try_suspend(v);
            t.low = true;
            t.suspended = true;
        }
        self.push_tabs();
    }

    fn go(&mut self, url: String) {
        let id = self.active;
        let has_view = self.tab(id).is_some_and(|t| t.view.is_some());
        {
            let t = self.tab_mut(id).unwrap();
            t.url = url.clone();
            t.loading = true;
        }
        if has_view {
            let v = self.active_view().unwrap();
            let _ = v.load_url(&url);
            let _ = v.focus();
        } else {
            let view = self.create_view(id, &url);
            let _ = view.focus();
            self.tab_mut(id).unwrap().view = Some(view);
            self.layout();
        }
        self.push_tabs();
    }

    /// Ouvre une adresse dans la fenêtre principale (réutilise un nouvel onglet vide).
    fn open_url(&mut self, url: String) {
        self.show_window();
        if self.tab(self.active).is_some_and(|t| t.url.is_empty()) {
            self.go(url);
        } else {
            self.new_tab(Some(url));
        }
    }

    fn cycle(&mut self, step: isize) {
        let n = self.tabs.len() as isize;
        let i = self.tabs.iter().position(|t| t.id == self.active).unwrap_or(0) as isize;
        let id = self.tabs[((i + step).rem_euclid(n)) as usize].id;
        self.activate(id);
    }

    fn shortcut(&mut self, source: u32, cmd: &str) {
        match cmd {
            "new" => self.new_tab(None),
            "reopen" => self.reopen(),
            "close" => self.close_tab(source),
            "focus" => {
                if let Some(ui) = &self.ui {
                let _ = ui.focus();
            }
                self.send(json!({ "t": "focus" }));
            }
            "next" => self.cycle(1),
            "prev" => self.cycle(-1),
            n if n.starts_with('n') => {
                let k: usize = n[1..].parse().unwrap_or(1);
                let i = if k == 9 { self.tabs.len() - 1 } else { (k - 1).min(self.tabs.len() - 1) };
                self.activate(self.tabs[i].id);
            }
            _ => {}
        }
    }

    fn page_message(&mut self, id: u32, raw: &str) {
        let Ok(msg) = serde_json::from_str::<PageMsg>(raw) else {
            self.shortcut(id, raw);
            return;
        };
        if !self.blocker_on.load(Ordering::Relaxed) {
            return;
        }
        let engine = self.engine.clone();
        let engine = engine.borrow();
        let Some(tab) = self.tab_mut(id) else { return };
        let script = match msg {
            PageMsg::Cosm { url } => {
                let res = engine.url_cosmetic_resources(&url);
                let css = filters::hide_css(&res.hide_selectors);
                tab.cosmetic_exceptions = res.exceptions;
                let mut js = format!("window.__fluxHide&&__fluxHide({},{});", json!(css), !res.generichide);
                js.push_str(&res.injected_script);
                js
            }
            PageMsg::Ids { classes, ids } => {
                let sel = engine.hidden_class_id_selectors(&classes, &ids, &tab.cosmetic_exceptions);
                if sel.is_empty() {
                    return;
                }
                format!("window.__fluxHide&&__fluxHide({});", json!(filters::hide_css(&sel)))
            }
        };
        if let Some(v) = &tab.view {
            let _ = v.evaluate_script(&script);
        }
    }

    fn push_filters(&self) {
        let m = &self.filter_meta;
        self.send(json!({ "t": "filters", "rules": m.rules, "lists": m.lists, "updated": m.updated, "busy": self.filters_busy }));
    }

    fn on_filters(&mut self, compiled: Option<filters::Compiled>) {
        self.filters_busy = false;
        if let Some(c) = compiled {
            if let Some(engine) = filters::materialize(&c) {
                *self.engine.borrow_mut() = engine;
                self.filter_meta = c.meta;
            }
        }
        self.push_filters();
    }

    fn reopen(&mut self) {
        if let Some(t) = self.closed.pop() {
            self.new_tab(Some(t.url));
        }
    }

    fn push_settings(&self) {
        let msg = json!({ "t": "settings", "s": self.settings(), "hotkey_ok": self.hotkey_status.0 });
        self.send(msg.clone());
        self.bar.send(&msg);
    }

    /// Applique un changement de réglages à tout ce qui tourne déjà.
    fn apply_settings(&mut self, new: Settings) {
        let old = std::mem::replace(&mut self.store.settings, new);
        let s = self.store.settings.clone();
        self.blocker_on.store(s.blocker, Ordering::Relaxed);
        if s.zoom != old.zoom {
            for v in self.views() {
                let _ = v.zoom(s.zoom as f64 / 100.0);
            }
        }
        if s.net_limit != old.net_limit {
            for v in self.views() {
                wv2::set_net_limit(v, s.net_limit);
            }
        }
        if (s.passwords, s.autofill) != (old.passwords, old.autofill) {
            for v in self.views() {
                wv2::set_autofill(v, s.passwords, s.autofill);
            }
        }
        if s.tracking != old.tracking {
            if let Some(ui) = &self.ui {
                wv2::set_tracking(ui, &s.tracking);
            }
        }
        if s.cpu_limit != old.cpu_limit {
            if let Some(l) = &mut self.cpu_limiter {
                l.set(s.cpu_limit);
            }
        }
        if s.hotkey != old.hotkey {
            self.hotkey.change(&s.hotkey);
        }
        if s.autostart != old.autostart {
            system::set_autostart(s.autostart);
        }
        if s.theme != old.theme {
            self.window.set_theme(theme_of(&s));
        }
        self.store.save_settings();
        self.push_settings();
    }

    fn handle_ui(&mut self, raw: &str) {
        let Ok(msg) = serde_json::from_str::<UiMsg>(raw) else {
            // Raccourcis envoyés par l'interface elle-même (même format que les pages).
            self.shortcut(self.active, raw);
            return;
        };
        match msg {
            UiMsg::Ready => {
                self.push_settings();
                self.send(json!({ "t": "hist", "items": self.store.ranked(3000), "reset": true }));
                self.push_tabs();
                self.push_filters();
                self.push_bookmarks();
                if self.active_view().is_none() {
                    self.send(json!({ "t": "focus" }));
                }
            }
            UiMsg::New { url } => self.new_tab(url),
            UiMsg::Close { id } => self.close_tab(id),
            UiMsg::Activate { id } => self.activate(id),
            UiMsg::Go { url } => self.go(url),
            UiMsg::Back => drop(self.active_view().map(|v| v.evaluate_script("history.back()"))),
            UiMsg::Forward => drop(self.active_view().map(|v| v.evaluate_script("history.forward()"))),
            UiMsg::Reload => drop(self.active_view().map(|v| v.reload())),
            UiMsg::Stop => {
                if let Some(v) = self.active_view() {
                    wv2::stop(v);
                }
            }
            UiMsg::Height { h } => {
                self.ui_h = h;
                self.layout();
            }
            UiMsg::Bar { h } => {
                self.bar_h = h;
                self.layout();
            }
            UiMsg::Suggest { gen, q } => self.request_suggestions(false, gen, q),
            UiMsg::Settings { s } => self.apply_settings(s),
            UiMsg::ClearHistory => {
                self.store.clear_history();
                self.store.flush();
                let msg = json!({ "t": "hist", "items": [], "reset": true });
                self.send(msg.clone());
                self.bar.send(&msg);
            }
            UiMsg::ClearData { what } => {
                if let Some(ui) = &self.ui {
                    wv2::clear_data(ui, &what);
                }
            }
            UiMsg::Reopen => self.reopen(),
            UiMsg::Devtools => {
                if let Some(v) = self.active_view() {
                    v.open_devtools();
                }
            }
            UiMsg::FocusPage => {
                if let Some(v) = self.active_view() {
                    let _ = v.focus();
                }
            }
            UiMsg::UpdateFilters => {
                let _ = self.update_filters.send(());
            }
            UiMsg::Freeze { id } => self.freeze(id),
            UiMsg::Discard { id } => self.discard(id),
            UiMsg::QuickBar => self.show_bar(),
            UiMsg::Bookmarks { items } => {
                self.store.bookmarks = items;
                self.store.save_bookmarks();
                self.push_bookmarks();
            }
            UiMsg::History => self.send(json!({ "t": "history", "items": self.store.chronological() })),
            UiMsg::HistoryDelete { url } => {
                self.store.forget(&url);
                self.store.flush();
            }
            UiMsg::PickBackground => {
                if let Some(path) = system::pick_media(self.window.hwnd()) {
                    if let Some(name) = media::install(&self.store.dir, &path) {
                        let mut s = self.settings().clone();
                        s.ntp_media = format!("{name}?{}", store::now());
                        self.apply_settings(s);
                    }
                }
            }
            UiMsg::ClearBackground => {
                media::clear(&self.store.dir);
                let mut s = self.settings().clone();
                s.ntp_media.clear();
                self.apply_settings(s);
            }
        }
    }

    fn handle_bar(&mut self, raw: &str) {
        let Ok(msg) = serde_json::from_str::<BarMsg>(raw) else { return };
        match msg {
            BarMsg::Ready => {
                self.push_settings();
                self.bar.send(&json!({ "t": "hist", "items": self.store.ranked(3000), "reset": true }));
                self.bar.send(&json!({ "t": "favs", "items": self.store.bookmarks }));
            }
            BarMsg::Go { url } => {
                self.bar.hide();
                self.open_url(url);
            }
            BarMsg::Height { h } => self.bar.resize(h),
            BarMsg::Suggest { gen, q } => self.request_suggestions(true, gen, q),
            BarMsg::Hide => self.bar.hide(),
        }
    }

    fn request_suggestions(&self, to_bar: bool, gen: u64, query: String) {
        let s = self.settings();
        if s.suggestions {
            let engine = s.engine.clone();
            let _ = self.suggest.send(suggest::Request { to_bar, gen, query, engine });
        }
    }

    fn on_load(&mut self, id: u32, started: bool, url: String) {
        let Some(t) = self.tab_mut(id) else { return };
        t.loading = started;
        t.url = url.clone();
        let title = t.title.clone();
        if !started {
            if let Some(v) = self.store.record(&url, &title) {
                let msg = json!({ "t": "hist", "items": [v] });
                self.send(msg.clone());
                self.bar.send(&msg);
            }
        }
        self.push_tabs();
    }

    fn on_title(&mut self, id: u32, title: String) {
        let Some(t) = self.tab_mut(id) else { return };
        t.title = title.clone();
        let url = t.url.clone();
        if let Some(v) = self.store.retitle(&url, &title) {
            self.send(json!({ "t": "hist", "items": [v] }));
        }
        self.push_tabs();
    }

    /// Gestion automatique de la mémoire des onglets en arrière-plan.
    fn tick(&mut self) {
        self.ticks += 1;
        let s = &self.store.settings;
        let sleep = Duration::from_secs(s.sleep_min.max(1) * 60);
        let discard = (s.discard_min > 0).then(|| Duration::from_secs(s.discard_min * 60));
        let active = self.active;
        let mut changed = false;
        for t in self.tabs.iter_mut().filter(|t| t.id != active) {
            let Some(v) = &t.view else { continue };
            if wv2::playing_audio(v) {
                t.hidden_since = Instant::now();
                continue;
            }
            let idle = t.hidden_since.elapsed();
            if discard.is_some_and(|d| idle >= d) {
                if let Ok(u) = v.url() {
                    t.url = u;
                }
                t.view = None;
                t.suspended = false;
                t.received_prev = 0;
                changed = true;
            } else if idle >= sleep && !t.suspended {
                wv2::try_suspend(v);
                t.suspended = true;
                changed = true;
            } else if idle >= LOW_MEMORY_AFTER && !t.low {
                let _ = v.set_memory_usage_level(MemoryUsageLevel::Low);
                t.low = true;
            }
        }
        if changed {
            self.push_tabs();
        }
        if self.shown {
            for t in self.tabs.iter().filter(|t| !t.suspended) {
                let Some(v) = &t.view else { continue };
                let (p, id) = (self.proxy.clone(), t.id);
                wv2::page_metrics(v, move |heap, task| {
                    let _ = p.send_event(UserEvent::Metrics(id, heap, task));
                });
            }
            let p = self.proxy.clone();
            if let Some(ui) = &self.ui {
                wv2::process_map(ui, move |list| {
                    let _ = p.send_event(UserEvent::Procs(list));
                });
            }
        }
        if self.ticks % 30 == 0 {
            self.persist();
        }
    }

    /// Mesure en temps réel + limiteurs, à partir de la liste des processus du moteur.
    fn on_procs(&mut self, list: Vec<wv2::ProcInfo>) {
        let elapsed = self.last_sample.elapsed().as_secs_f64().max(0.001);
        self.last_sample = Instant::now();
        let own = std::process::id();

        let mut kinds: HashMap<&str, (u64, f64, u32)> = HashMap::new();
        let (mut mem, mut cpu) = (0u64, 0f64);
        let me = self.sampler.sample(own);
        mem += me.mem;
        cpu += me.cpu;
        kinds.insert("Interface", (me.mem, me.cpu, 1));
        let mut alive = HashSet::from([own]);
        let mut renderer_mem = 0u64;
        for p in &list {
            alive.insert(p.pid);
            let u = self.sampler.sample(p.pid);
            mem += u.mem;
            cpu += u.cpu;
            if p.kind == "Rendu" {
                renderer_mem += u.mem;
            }
            let k = kinds.entry(p.kind).or_default();
            k.0 += u.mem;
            k.1 += u.cpu;
            k.2 += 1;
        }
        self.sampler.retain(&alive);

        // Mémoire réelle des processus de rendu, répartie entre les onglets au prorata de leur tas JS.
        let heap_total: f64 = self.tabs.iter().filter(|t| t.view.is_some()).map(|t| t.heap).sum();
        let mut net_total = 0f64;
        let tabs: Vec<_> = self
            .tabs
            .iter_mut()
            .map(|t| {
                let got = t.received.get();
                let rate = got.saturating_sub(t.received_prev) as f64 / elapsed;
                t.received_prev = got;
                net_total += rate;
                let m = if t.view.is_some() && heap_total > 0.0 { (renderer_mem as f64 * t.heap / heap_total) as u64 } else { 0 };
                let c = if t.view.is_some() && !t.suspended { t.cpu } else { 0.0 };
                let state = if t.view.is_none() {
                    "unloaded"
                } else if t.suspended {
                    "frozen"
                } else {
                    "running"
                };
                json!({ "id": t.id, "title": t.title, "url": t.url, "mem": m, "cpu": c, "net": rate, "state": state })
            })
            .collect();
        let procs: Vec<_> = kinds
            .iter()
            .map(|(k, (m, c, n))| json!({ "kind": k, "mem": m, "cpu": c, "count": n }))
            .collect();

        // Limite de mémoire : décharge l'onglet en arrière-plan inactif depuis le plus longtemps.
        let limit = self.settings().ram_limit * MB;
        if limit > 0 && mem > limit {
            let victim = self
                .tabs
                .iter()
                .filter(|t| t.id != self.active && t.view.as_ref().is_some_and(|v| !wv2::playing_audio(v)))
                .min_by_key(|t| t.hidden_since)
                .map(|t| t.id);
            if let Some(id) = victim {
                self.discard(id);
            }
        }

        let alive_tabs = self.tabs.iter().filter(|t| t.view.is_some() && !t.suspended).count();
        self.send(json!({
            "t": "stats", "mem": mem, "cpu": cpu, "net": net_total,
            "blocked": self.blocked.load(Ordering::Relaxed),
            "alive": alive_tabs, "total": self.tabs.len(), "tabs": tabs, "procs": procs,
        }));
    }

    fn on_metrics(&mut self, id: u32, heap: f64, task: f64) {
        let cores = self.sampler.cores();
        let Some(t) = self.tab_mut(id) else { return };
        t.heap = heap;
        let now = Instant::now();
        if let Some((prev, at)) = t.task_prev {
            let dt = now.duration_since(at).as_secs_f64();
            if dt > 0.0 && task >= prev {
                t.cpu = ((task - prev) / dt / cores * 100.0).min(100.0);
            }
        }
        t.task_prev = Some((task, now));
    }

    fn create_ui(&mut self) {
        if self.ui.is_some() {
            return;
        }
        let p = self.proxy.clone();
        let dir = self.store.dir.clone();
        let (w, h) = self.logical_size();
        let bg = if theme_of(self.settings()) == Some(Theme::Light) { (236, 236, 239, 255) } else { (24, 24, 27, 255) };
        let ui = WebViewBuilder::new_with_web_context(&mut self.ctx)
            .with_html(&self.ui_html)
            .with_bounds(Rect {
                position: LogicalPosition::new(0.0, 0.0).into(),
                size: WryLogicalSize::new(w, h).into(),
            })
            .with_additional_browser_args(&self.args)
            .with_hotkeys_zoom(false)
            .with_default_context_menus(false)
            .with_background_color(bg)
            // Fond personnalisé de la page d'accueil : http://flux.localhost/background.*
            .with_custom_protocol("flux".into(), move |_, req| media::serve(&dir, req))
            .with_ipc_handler(move |r| {
                let _ = p.send_event(UserEvent::Ui(r.body().clone()));
            })
            .build_as_child(&self.window)
            .expect("création de l'interface");
        wv2::set_tracking(&ui, &self.settings().tracking);
        wv2::raise(&ui);
        self.ui = Some(ui);
        self.layout();
    }

    /// Fenêtre réduite : tout passe en mémoire réduite, rien n'est gelé (le son continue).
    fn on_minimized(&mut self, minimized: bool) {
        if minimized == self.minimized {
            return;
        }
        self.minimized = minimized;
        let level = if minimized { MemoryUsageLevel::Low } else { MemoryUsageLevel::Normal };
        for t in &mut self.tabs {
            if let Some(v) = &t.view {
                if t.id == self.active || !minimized {
                    let _ = v.set_memory_usage_level(level);
                    t.low = minimized;
                }
            }
        }
        if let Some(ui) = &self.ui {
            let _ = ui.set_memory_usage_level(level);
        }
    }

    fn show_bar(&mut self) {
        let theme = theme_of(self.settings());
        self.bar.show(&mut self.ctx, theme);
    }

    fn toggle_bar(&mut self) {
        if self.bar.visible() {
            self.bar.hide();
        } else {
            self.show_bar();
        }
    }

    fn push_bookmarks(&self) {
        let msg = json!({ "t": "favs", "items": self.store.bookmarks });
        self.send(msg.clone());
        self.bar.send(&msg);
    }

    fn persist(&mut self) {
        let active = self.tabs.iter().position(|t| t.id == self.active).unwrap_or(0);
        let tabs = self
            .tabs
            .iter()
            .filter(|t| !t.url.is_empty())
            .map(|t| SessionTab { url: t.url.clone(), title: t.title.clone() })
            .collect();
        self.store.save_session(&Session { tabs, active });
        self.store.flush();
    }

    fn show_window(&mut self) {
        if !self.shown {
            self.shown = true;
            self.window.set_visible(true);
            self.create_ui();
            self.activate(self.active);
        }
        self.window.set_minimized(false);
        self.window.set_focus();
    }

    /// Fermer la fenêtre en mode arrière-plan : toutes les pages sont libérées,
    /// seuls l'interface et la barre rapide restent prêtes.
    fn hide_window(&mut self) {
        self.persist();
        self.shown = false;
        self.window.set_visible(false);
        for t in &mut self.tabs {
            if let Some(v) = t.view.take() {
                if let Ok(u) = v.url() {
                    t.url = u;
                }
            }
            t.suspended = false;
            t.received_prev = 0;
        }
        // Plus aucune vue web : le moteur WebView2 s'arrête complètement et rend toute sa
        // mémoire. Seul le petit processus Flux reste, pour le raccourci et l'icône.
        self.ui = None;
        self.bar.hide();
    }

    fn command(&mut self, cmd: &str) {
        let cmd = cmd.trim();
        if cmd == "bar" {
            self.show_bar();
        } else if let Some(url) = cmd.strip_prefix("url ") {
            self.open_url(url.to_string());
        } else {
            self.show_window();
        }
    }
}

fn tray(proxy: &EventLoopProxy<UserEvent>) -> Option<TrayIcon> {
    let menu = Menu::new();
    let open = MenuItem::with_id("open", "Ouvrir Flux", true, None);
    let search = MenuItem::with_id("bar", "Recherche rapide", true, None);
    let quit = MenuItem::with_id("quit", "Quitter Flux", true, None);
    menu.append(&open).ok()?;
    menu.append(&search).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&quit).ok()?;

    let p = Mutex::new(proxy.clone());
    TrayIconEvent::set_event_handler(Some(move |e| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            let _ = p.lock().unwrap().send_event(UserEvent::TrayOpen);
        }
    }));
    let p = Mutex::new(proxy.clone());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        let _ = p.lock().unwrap().send_event(UserEvent::MenuItem(e.id.0));
    }));

    TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("Flux")
        .with_icon(tray_icon::Icon::from_resource(1, None).ok()?)
        .build()
        .ok()
}

fn main() {
    // Instance unique : un second lancement passe la main à celui qui tourne déjà.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let background = args.iter().any(|a| a == "--background");
    let url_arg = args.iter().find(|a| !a.starts_with("--")).cloned();
    let command = if args.iter().any(|a| a == "--bar") {
        "bar".to_string()
    } else if let Some(u) = &url_arg {
        format!("url {u}")
    } else {
        "show".to_string()
    };
    if system::forward_to_running(&command) {
        return;
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let store = Store::open();
    let settings = store.settings.clone();

    let window = WindowBuilder::new()
        .with_title("Flux")
        .with_window_icon(Icon::from_resource(1, None).ok())
        .with_inner_size(LogicalSize::new(1280.0, 820.0))
        .with_min_inner_size(LogicalSize::new(480.0, 320.0))
        .with_theme(theme_of(&settings))
        .with_visible(!background)
        .build(&event_loop)
        .expect("création de la fenêtre");

    // Avant le moteur : ses processus doivent naître dans le job du plafond processeur.
    let mut cpu_limiter = monitor::CpuLimiter::new();
    if let Some(l) = &mut cpu_limiter {
        l.set(settings.cpu_limit);
    }

    let args = browser_args(&settings);
    let ctx = WebContext::new(Some(store.webview_dir()));
    let ui_html = UI_HTML.replace("/*OMNI*/", OMNI_JS);
    let bar = QuickBar::new(&event_loop, &args, BAR_HTML.replace("/*OMNI*/", OMNI_JS), proxy.clone());

    let suggest = {
        let p = proxy.clone();
        suggest::spawn(move |to_bar, gen, items| {
            let _ = p.send_event(UserEvent::Suggest(to_bar, gen, items));
        })
    };

    {
        let p = proxy.clone();
        thread::spawn(move || loop {
            thread::sleep(TICK);
            if p.send_event(UserEvent::Tick).is_err() {
                break;
            }
        });
    }

    let hotkey = {
        let (p1, p2) = (proxy.clone(), proxy.clone());
        system::Hotkey::spawn(
            settings.hotkey.clone(),
            move || {
                let _ = p1.send_event(UserEvent::Hotkey);
            },
            move |ok, spec| {
                let _ = p2.send_event(UserEvent::HotkeyStatus(ok, spec));
            },
        )
    };
    {
        let p = proxy.clone();
        system::listen(move |cmd| {
            let _ = p.send_event(UserEvent::Command(cmd));
        });
    }
    system::set_autostart(settings.autostart);

    let (engine, filter_meta) = filters::load(&store.dir);
    // Mise à jour des listes : au démarrage si elles ont plus de 24 h, puis vérification
    // toutes les heures, ou immédiatement sur demande depuis les réglages.
    let update_filters = {
        let p = proxy.clone();
        let base = store.dir.clone();
        let (tx, rx) = channel::<()>();
        let mut meta = filter_meta.clone();
        thread::spawn(move || loop {
            let forced = match rx.recv_timeout(if meta.updated == 0 { Duration::ZERO } else { Duration::from_secs(3600) }) {
                Ok(()) => true,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
                Err(_) => break,
            };
            if !forced && !filters::is_stale(&meta) {
                continue;
            }
            let _ = p.send_event(UserEvent::FiltersBusy);
            let compiled = filters::compile(&base);
            if let Some(c) = &compiled {
                meta = c.meta.clone();
            } else if meta.updated == 0 {
                // Hors ligne au premier lancement : on réessaie dans une heure.
                meta.updated = 1;
            }
            if p.send_event(UserEvent::Filters(compiled)).is_err() {
                break;
            }
        });
        tx
    };

    let session = store.load_session();
    let blocker_on = Arc::new(AtomicBool::new(settings.blocker));
    let mut app = App {
        window,
        ui: None,
        ui_html,
        bar,
        ctx,
        proxy: proxy.clone(),
        tabs: Vec::new(),
        active: 0,
        next_id: 1,
        closed: Vec::new(),
        store,
        args,
        engine: Rc::new(RefCell::new(engine)),
        filter_meta,
        filters_busy: false,
        update_filters,
        blocker_on,
        blocked: Arc::new(AtomicU64::new(0)),
        suggest,
        hotkey,
        hotkey_status: (false, settings.hotkey.clone()),
        sampler: monitor::Sampler::new(),
        cpu_limiter,
        last_sample: Instant::now(),
        bar_h: 78.0,
        ui_h: Some(78.0),
        shown: !background,
        minimized: false,
        ticks: 0,
        _tray: tray(&proxy),
    };

    if !background {
        app.create_ui();
    }

    // Restauration paresseuse : seuls les onglets réellement ouverts chargent leur page.
    let restore: Vec<SessionTab> = match settings.startup.as_str() {
        "restore" => session.tabs.clone(),
        "home" if !settings.home_url.is_empty() => {
            vec![SessionTab { url: settings.home_url.clone(), title: String::new() }]
        }
        _ => Vec::new(),
    };
    let ids: Vec<u32> = restore.iter().map(|t| app.add_tab(&t.url, &t.title)).collect();
    match ids.get(session.active.min(ids.len().saturating_sub(1))) {
        Some(&id) => app.activate(id),
        None => app.new_tab(None),
    }
    if let Some(u) = url_arg {
        app.open_url(u);
    }

    let main_id = app.window.id();
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { window_id, event, .. } if window_id == main_id => match event {
                WindowEvent::Resized(_) => {
                    app.on_minimized(app.window.is_minimized());
                    app.layout();
                }
                WindowEvent::ScaleFactorChanged { .. } => app.layout(),
                WindowEvent::CloseRequested => {
                    if app.settings().background {
                        app.hide_window();
                    } else {
                        app.persist();
                        *control_flow = ControlFlow::Exit;
                    }
                }
                _ => {}
            },
            Event::UserEvent(e) => match e {
                UserEvent::Ui(m) => app.handle_ui(&m),
                UserEvent::Bar(m) => app.handle_bar(&m),
                UserEvent::Page(id, msg) => app.page_message(id, &msg),
                UserEvent::Title(id, t) => app.on_title(id, t),
                UserEvent::Load(id, started, url) => app.on_load(id, started, url),
                UserEvent::OpenTab(url) => app.new_tab(Some(url)),
                UserEvent::Suggest(to_bar, gen, items) => {
                    let msg = json!({ "t": "sugg", "gen": gen, "items": items });
                    if to_bar {
                        app.bar.send(&msg);
                    } else {
                        app.send(msg);
                    }
                }
                UserEvent::Tick => app.tick(),
                UserEvent::Procs(list) => app.on_procs(list),
                UserEvent::Metrics(id, heap, task) => app.on_metrics(id, heap, task),
                UserEvent::FiltersBusy => {
                    app.filters_busy = true;
                    app.push_filters();
                }
                UserEvent::Filters(c) => app.on_filters(c),
                UserEvent::Hotkey => app.toggle_bar(),
                UserEvent::HotkeyStatus(ok, spec) => {
                    app.hotkey_status = (ok, spec);
                    app.push_settings();
                }
                UserEvent::Command(c) => app.command(&c),
                UserEvent::TrayOpen => app.show_window(),
                UserEvent::MenuItem(id) => match id.as_str() {
                    "open" => app.show_window(),
                    "bar" => app.show_bar(),
                    "quit" => {
                        app.persist();
                        *control_flow = ControlFlow::Exit;
                    }
                    _ => {}
                },
            },
            _ => {}
        }
    });
}
