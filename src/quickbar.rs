//! Barre de recherche rapide : s'ouvre en haut de l'écran avec le raccourci global,
//! même quand le navigateur est fermé (Flux reste dans la zone de notification).

use tao::{
    dpi::{LogicalSize, PhysicalPosition},
    event_loop::{EventLoopProxy, EventLoopWindowTarget},
    platform::windows::{WindowBuilderExtWindows, WindowExtWindows},
    window::{Window, WindowBuilder},
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
    pub view: WebView,
    height: f64,
}

impl QuickBar {
    pub fn new(
        target: &EventLoopWindowTarget<UserEvent>,
        ctx: &mut WebContext,
        args: &str,
        html: &str,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Self {
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
        let view = WebViewBuilder::new_with_web_context(ctx)
            .with_html(html)
            .with_bounds(Rect {
                position: WryLogicalPosition::new(0.0, 0.0).into(),
                size: WryLogicalSize::new(WIDTH, BASE_H).into(),
            })
            .with_additional_browser_args(args)
            .with_hotkeys_zoom(false)
            .with_default_context_menus(false)
            .with_background_color((24, 24, 27, 255))
            .with_ipc_handler(move |r| {
                let _ = proxy.send_event(UserEvent::Bar(r.body().clone()));
            })
            .build_as_child(&window)
            .expect("création de la vue de la barre rapide");
        Self { window, view, height: BASE_H }
    }

    pub fn visible(&self) -> bool {
        self.window.is_visible()
    }

    /// Affiche la barre en haut de l'écran où se trouve la souris.
    pub fn show(&self) {
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
        self.window.set_visible(true);
        self.window.set_focus();
        let _ = self.view.focus();
        let _ = self.view.evaluate_script("openBar()");
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
        let _ = self.view.evaluate_script("closeBar()");
    }

    pub fn resize(&mut self, h: f64) {
        let h = h.clamp(BASE_H, 640.0);
        if (h - self.height).abs() < 0.5 {
            return;
        }
        self.height = h;
        self.window.set_inner_size(LogicalSize::new(WIDTH, h));
        let _ = self.view.set_bounds(Rect {
            position: WryLogicalPosition::new(0.0, 0.0).into(),
            size: WryLogicalSize::new(WIDTH, h).into(),
        });
    }

    pub fn send(&self, msg: &serde_json::Value) {
        let _ = self.view.evaluate_script(&format!("flux({msg})"));
    }
}
