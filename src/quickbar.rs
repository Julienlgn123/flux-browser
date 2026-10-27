//! Barre de recherche rapide : s'ouvre en haut de l'écran avec le raccourci global,
//! même quand le navigateur est fermé (Flux reste dans la zone de notification).
//!
//! Sa vue web n'existe que pendant qu'elle est affichée : fermée, elle ne coûte rien.

use tao::{
    dpi::{LogicalSize, PhysicalPosition},
    event_loop::{EventLoopProxy, EventLoopWindowTarget},
    platform::windows::{WindowBuilderExtWindows, WindowExtWindows},
    window::{Theme, Window, WindowBuilder},
};
use windows::Win32::{
    Foundation::HWND,
    Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWM_WINDOW_CORNER_PREFERENCE},
};
use wry::{
    dpi::{LogicalPosition as WryLogicalPosition, LogicalSize as WryLogicalSize},
    Rect, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows,
};

use crate::UserEvent;

pub const WIDTH: f64 = 760.0;
pub const BASE_H: f64 = 64.0;
/// Distance au bord supérieur de l'écran.
const TOP: f64 = 18.0;

pub struct QuickBar {
    pub window: Window,
    pub view: Option<WebView>,
    html: String,
    args: String,
    proxy: EventLoopProxy<UserEvent>,
    height: f64,
}

impl QuickBar {
    pub fn new(target: &EventLoopWindowTarget<UserEvent>, args: &str, html: String, proxy: EventLoopProxy<UserEvent>) -> Self {
        let window = WindowBuilder::new()
            .with_title("Flux — recherche rapide")
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top(true)
            .with_skip_taskbar(true)
            .with_visible(false)
            .with_inner_size(LogicalSize::new(WIDTH, BASE_H))
            .build(target)
            .expect("création de la barre rapide");
        unsafe {
            let pref: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(
                HWND(window.hwnd() as _),
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &pref as *const _ as *const _,
                std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
            );
        }
        Self { window, view: None, html, args: args.to_string(), proxy, height: BASE_H }
    }

    pub fn visible(&self) -> bool {
        self.window.is_visible()
    }

    /// Affiche la barre en haut de l'écran où se trouve la souris.
    pub fn show(&mut self, ctx: &mut WebContext, theme: Option<Theme>) {
        let monitor = self
            .window
            .cursor_position()
            .ok()
            .and_then(|p| self.window.monitor_from_point(p.x, p.y))
            .or_else(|| self.window.primary_monitor());
        if let Some(m) = monitor {
            let scale = m.scale_factor();
            let (pos, size) = (m.position(), m.size());
            let x = pos.x as f64 + (size.width as f64 - WIDTH * scale) / 2.0;
            let y = pos.y as f64 + TOP * scale;
            self.window.set_outer_position(PhysicalPosition::new(x, y));
        }
        self.height = BASE_H;
        self.window.set_inner_size(LogicalSize::new(WIDTH, BASE_H));
        self.window.set_theme(theme);
        if self.view.is_none() {
            let proxy = self.proxy.clone();
            let bg = if theme == Some(Theme::Light) { (255, 255, 255, 255) } else { (24, 24, 27, 255) };
            self.view = WebViewBuilder::new_with_web_context(ctx)
                .with_html(&self.html)
                .with_bounds(Rect {
                    position: WryLogicalPosition::new(0.0, 0.0).into(),
                    size: WryLogicalSize::new(WIDTH, BASE_H).into(),
                })
                .with_additional_browser_args(&self.args)
                .with_hotkeys_zoom(false)
                .with_default_context_menus(false)
                .with_background_color(bg)
                .with_ipc_handler(move |r| {
                    let _ = proxy.send_event(UserEvent::Bar(r.body().clone()));
                })
                .build_as_child(&self.window)
                .ok();
        }
        self.window.set_visible(true);
        self.window.set_focus();
        if let Some(v) = &self.view {
            let _ = v.focus();
        }
    }

    /// Masque la barre et libère sa vue web.
    pub fn hide(&mut self) {
        self.window.set_visible(false);
        self.view = None;
    }

    pub fn resize(&mut self, h: f64) {
        let h = h.clamp(BASE_H, 640.0);
        if (h - self.height).abs() < 0.5 {
            return;
        }
        self.height = h;
        self.window.set_inner_size(LogicalSize::new(WIDTH, h));
        if let Some(v) = &self.view {
            let _ = v.set_bounds(Rect {
                position: WryLogicalPosition::new(0.0, 0.0).into(),
                size: WryLogicalSize::new(WIDTH, h).into(),
            });
        }
    }

    pub fn send(&self, msg: &serde_json::Value) {
        if let Some(v) = &self.view {
            let _ = v.evaluate_script(&format!("flux({msg})"));
        }
    }
}
