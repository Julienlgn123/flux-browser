#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod store;
mod suggest;
mod wv2;

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::Sender,
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::json;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::{Theme, Window, WindowBuilder},
};
use wry::{
    dpi::{LogicalPosition, LogicalSize as WryLogicalSize},
    MemoryUsageLevel, NewWindowResponse, PageLoadEvent, Rect, WebContext, WebView, WebViewBuilder,
    WebViewBuilderExtWindows, WebViewExtWindows,
};

use store::{Session, SessionTab, Store};

/// Hauteur de la barre (onglets + barre d'adresse), en px CSS.
const BAR_H: f64 = 78.0;
const TICK: Duration = Duration::from_secs(3);
/// Un onglet caché passe en mode mémoire réduite après ce délai.
const LOW_MEMORY_AFTER: Duration = Duration::from_secs(30);

const UI_HTML: &str = include_str!("../ui/index.html");
const BLOCKLIST: &str = include_str!("blocklist.txt");

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

enum UserEvent {
    Ui(String),
    Page(u32, String),
    Title(u32, String),
    Load(u32, bool, String),
    OpenTab(String),
    Suggest(u64, Vec<String>),
    Tick,
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
    Suggest { gen: u64, q: String },
    Settings { s: store::Settings },
    ClearHistory,
    Reopen,
    Devtools,
    FocusPage,
}

struct Tab {
    id: u32,
    view: Option<WebView>,
    url: String,
    title: String,
    loading: bool,
    hidden_since: Instant,
    low: bool,
    suspended: bool,
}

struct App {
    window: Window,
    ui: WebView,
    ctx: WebContext,
    proxy: EventLoopProxy<UserEvent>,
    tabs: Vec<Tab>,
    active: u32,
    next_id: u32,
    closed: Vec<SessionTab>,
    store: Store,
    args: String,
    blocklist: Vec<&'static str>,
    blocker_on: Arc<AtomicBool>,
    blocked: Arc<AtomicU64>,
    suggest: Sender<suggest::Request>,
    ui_h: Option<f64>,
    ticks: u64,
    last_stats: (u64, u64, usize),
}

impl App {
    fn logical_size(&self) -> (f64, f64) {
        let s = self.window.inner_size().to_logical::<f64>(self.window.scale_factor());
        (s.width, s.height)
    }

    fn content_rect(&self) -> Rect {
        let (w, h) = self.logical_size();
        Rect {
            position: LogicalPosition::new(0.0, BAR_H).into(),
            size: WryLogicalSize::new(w, (h - BAR_H).max(0.0)).into(),
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

    fn layout(&self) {
        let (w, h) = self.logical_size();
        // Page vide (nouvel onglet) : l'interface occupe toute la fenêtre.
        let empty = self.active_view().is_none();
        let ui_h = if empty { h } else { self.ui_h.map_or(h, |x| x.min(h)) };
        let _ = self.ui.set_bounds(Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: WryLogicalSize::new(w, ui_h).into(),
        });
        let rect = self.content_rect();
        for t in &self.tabs {
            if let Some(v) = &t.view {
                let _ = v.set_bounds(rect);
            }
        }
    }

    fn send(&self, msg: serde_json::Value) {
        let _ = self.ui.evaluate_script(&format!("flux({msg})"));
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
        let view = WebViewBuilder::new_with_web_context(&mut self.ctx)
            .with_url(url)
            .with_bounds(rect)
            .with_additional_browser_args(&self.args)
            .with_initialization_script_for_main_only(PAGE_JS, true)
            .with_devtools(true)
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
                // Les vraies pop-ups (taille imposée, ex. connexion OAuth) gardent leur fenêtre
                // pour conserver window.opener ; le reste s'ouvre dans un onglet.
                if features.size.is_some() {
                    NewWindowResponse::Allow
                } else {
                    let _ = p4.send_event(UserEvent::OpenTab(u));
                    NewWindowResponse::Deny
                }
            })
            .build_as_child(&self.window)
            .expect("création de la vue WebView2");
        let _ = wv2::install_blocker(&view, &self.blocklist, self.blocker_on.clone(), self.blocked.clone());
        wv2::raise(&self.ui);
        view
    }

    fn add_tab(&mut self, url: &str, title: &str) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.tabs.push(Tab {
            id,
            view: None,
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
        let needs_view = self.tab(id).is_some_and(|t| t.view.is_none() && !t.url.is_empty());
        if needs_view {
            let url = self.tab(id).unwrap().url.clone();
            let view = self.create_view(id, &url);
            let t = self.tab_mut(id).unwrap();
            t.view = Some(view);
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
            let _ = self.ui.focus();
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
                let _ = self.ui.focus();
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

    fn reopen(&mut self) {
        if let Some(t) = self.closed.pop() {
            self.new_tab(Some(t.url));
        }
    }

    fn handle_ui(&mut self, raw: &str) {
        let Ok(msg) = serde_json::from_str::<UiMsg>(raw) else {
            // Raccourcis envoyés par l'interface elle-même (même format que les pages).
            self.shortcut(self.active, raw);
            return;
        };
        match msg {
            UiMsg::Ready => {
                self.send(json!({ "t": "settings", "s": self.store.settings }));
                self.send(json!({ "t": "hist", "items": self.store.ranked(3000), "reset": true }));
                self.push_tabs();
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
            UiMsg::Suggest { gen, q } => {
                if self.store.settings.suggestions {
                    let engine = self.store.settings.engine.clone();
                    let _ = self.suggest.send(suggest::Request { gen, query: q, engine });
                }
            }
            UiMsg::Settings { s } => {
                self.blocker_on.store(s.blocker, Ordering::Relaxed);
                self.store.settings = s;
                self.store.save_settings();
            }
            UiMsg::ClearHistory => {
                self.store.clear_history();
                self.store.flush();
                self.send(json!({ "t": "hist", "items": [], "reset": true }));
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
        }
    }

    fn on_load(&mut self, id: u32, started: bool, url: String) {
        let Some(t) = self.tab_mut(id) else { return };
        t.loading = started;
        t.url = url.clone();
        let title = t.title.clone();
        if !started {
            if let Some(v) = self.store.record(&url, &title) {
                self.send(json!({ "t": "hist", "items": [v] }));
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

        let mem = wv2::total_memory(&self.ui);
        let blocked = self.blocked.load(Ordering::Relaxed);
        let alive = self.tabs.iter().filter(|t| t.view.is_some() && !t.suspended).count();
        let stats = (mem >> 20, blocked, alive);
        if stats != self.last_stats {
            self.last_stats = stats;
            self.send(json!({ "t": "stats", "mem": mem, "blocked": blocked, "alive": alive, "total": self.tabs.len() }));
        }
        if self.ticks % 10 == 0 {
            self.persist();
        }
    }

    fn persist(&mut self) {
        let active = self.tabs.iter().position(|t| t.id == self.active).unwrap_or(0);
        let tabs = self
            .tabs
            .iter()
            .map(|t| SessionTab { url: t.url.clone(), title: t.title.clone() })
            .collect();
        self.store.save_session(&Session { tabs, active });
        self.store.flush();
    }
}

fn main() {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let window = WindowBuilder::new()
        .with_title("Flux")
        .with_inner_size(LogicalSize::new(1280.0, 820.0))
        .with_min_inner_size(LogicalSize::new(480.0, 320.0))
        .with_theme(Some(Theme::Dark))
        .build(&event_loop)
        .expect("création de la fenêtre");

    let store = Store::open();
    let mut args = String::from(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,Translate \
         --enable-features=BackForwardCache,ParallelDownloading \
         --process-per-site --disable-background-networking --no-pings",
    );
    if store.settings.eco {
        args.push_str(" --renderer-process-limit=3");
    }
    let mut ctx = WebContext::new(Some(store.webview_dir()));

    let ui = {
        let p = proxy.clone();
        let s = window.inner_size().to_logical::<f64>(window.scale_factor());
        WebViewBuilder::new_with_web_context(&mut ctx)
            .with_html(UI_HTML)
            .with_bounds(Rect {
                position: LogicalPosition::new(0.0, 0.0).into(),
                size: WryLogicalSize::new(s.width, s.height).into(),
            })
            .with_additional_browser_args(&args)
            .with_hotkeys_zoom(false)
            .with_default_context_menus(false)
            .with_background_color((24, 24, 27, 255))
            .with_ipc_handler(move |r| {
                let _ = p.send_event(UserEvent::Ui(r.body().clone()));
            })
            .build_as_child(&window)
            .expect("création de l'interface")
    };

    let suggest = {
        let p = proxy.clone();
        suggest::spawn(move |gen, items| {
            let _ = p.send_event(UserEvent::Suggest(gen, items));
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

    let blocklist = BLOCKLIST
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect();

    let session = store.load_session();
    let blocker_on = Arc::new(AtomicBool::new(store.settings.blocker));
    let mut app = App {
        window,
        ui,
        ctx,
        proxy,
        tabs: Vec::new(),
        active: 0,
        next_id: 1,
        closed: Vec::new(),
        store,
        args,
        blocklist,
        blocker_on,
        blocked: Arc::new(AtomicU64::new(0)),
        suggest,
        ui_h: Some(BAR_H),
        ticks: 0,
        last_stats: (0, 0, 0),
    };

    // Restauration paresseuse : seuls les onglets réellement ouverts chargent leur page.
    let ids: Vec<u32> = session.tabs.iter().map(|t| app.add_tab(&t.url, &t.title)).collect();
    match ids.get(session.active).or(ids.first()) {
        Some(&id) => app.activate(id),
        None => app.new_tab(None),
    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::Resized(_), .. }
            | Event::WindowEvent { event: WindowEvent::ScaleFactorChanged { .. }, .. } => app.layout(),
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                app.persist();
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(e) => match e {
                UserEvent::Ui(m) => app.handle_ui(&m),
                UserEvent::Page(id, cmd) => app.shortcut(id, &cmd),
                UserEvent::Title(id, t) => app.on_title(id, t),
                UserEvent::Load(id, started, url) => app.on_load(id, started, url),
                UserEvent::OpenTab(url) => app.new_tab(Some(url)),
                UserEvent::Suggest(gen, items) => app.send(json!({ "t": "sugg", "gen": gen, "items": items })),
                UserEvent::Tick => app.tick(),
            },
            _ => {}
        }
    });
}
