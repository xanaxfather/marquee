//! Small WebKit / WebView2 windows (artwork picker, Fix Match, Settings).
//! Each exists only while open, so closing it frees the web process.
//!
//! Pages talk to the app with `window.ipc.postMessage(JSON)`; messages are
//! forwarded to the tray event loop tagged with the window kind, and the app
//! answers by evaluating script in the page.

use crate::tray::UserEvent;
use std::rc::Rc;
use tao::dpi::LogicalSize;
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder, WindowId};
use wry::{WebContext, WebView, WebViewBuilder};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Artwork,
    Match,
    Settings,
}

/// Windows open right now; Marquee is a regular app (Dock icon) only while any are.
static OPEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// macOS: make Marquee the active app so its window becomes key. A menu bar
/// (accessory) app is refused activation on macOS 14+, and WebKit shows no
/// hover states or pointer cursors in a window that isn't key, so the pages
/// looked lifeless until clicked. Becoming a regular app while a window is
/// open is what lets the activation through.
#[cfg(target_os = "macos")]
fn activate_app() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, sel};
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: bool = msg_send![app, setActivationPolicy: 0isize]; // Regular
        let modern: bool = msg_send![app, respondsToSelector: sel!(activate)];
        if modern {
            let _: () = msg_send![app, activate];
        } else {
            let _: () = msg_send![app, activateIgnoringOtherApps: true];
        }
    }
}

/// macOS: back to a menu bar app once the last window has closed.
#[cfg(target_os = "macos")]
fn release_app() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: bool = msg_send![app, setActivationPolicy: 1isize]; // Accessory
    }
}

#[cfg(not(target_os = "macos"))]
fn activate_app() {}
#[cfg(not(target_os = "macos"))]
fn release_app() {}

pub struct WebWindow {
    // Field order matters: the webview must drop before its context and window.
    webview: WebView,
    _context: Box<WebContext>,
    window: Rc<Window>,
    pub kind: Kind,
    ready: bool,
    pending: Vec<String>,
}

pub struct Spec<'a> {
    pub kind: Kind,
    pub title: &'a str,
    pub size: (f64, f64),
    pub min_size: (f64, f64),
    /// Page source; `/*__CTX__*/null` is replaced by `ctx`.
    pub html: &'a str,
    pub ctx: serde_json::Value,
}

impl Drop for WebWindow {
    fn drop(&mut self) {
        if OPEN.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) == 1 {
            release_app();
        }
    }
}

impl WebWindow {
    pub fn open(
        el: &EventLoopWindowTarget<UserEvent>,
        proxy: EventLoopProxy<UserEvent>,
        spec: Spec,
    ) -> anyhow::Result<Self> {
        // Ask now, while the menu click that opened us is still the latest input.
        activate_app();
        let built = Self::build(el, proxy, spec);
        if built.is_err() && OPEN.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            release_app();
        }
        built
    }

    fn build(
        el: &EventLoopWindowTarget<UserEvent>,
        proxy: EventLoopProxy<UserEvent>,
        spec: Spec,
    ) -> anyhow::Result<Self> {
        let builder = WindowBuilder::new()
            .with_title(crate::style::text(spec.title))
            .with_inner_size(LogicalSize::new(spec.size.0, spec.size.1))
            .with_min_inner_size(LogicalSize::new(spec.min_size.0, spec.min_size.1))
            .with_visible(false);
        // macOS: content under a transparent title bar (traffic lights float over it).
        #[cfg(target_os = "macos")]
        let builder = {
            use tao::platform::macos::WindowBuilderExtMacOS;
            builder.with_titlebar_transparent(true).with_title_hidden(true).with_fullsize_content_view(true)
        };
        // Windows: the standard title bar, in dark mode to match the pages.
        #[cfg(windows)]
        let builder = {
            use tao::platform::windows::{IconExtWindows, WindowBuilderExtWindows};
            let b = builder.with_theme(Some(tao::window::Theme::Dark));
            match tao::window::Icon::from_resource(1, None) {
                Ok(icon) => b.with_window_icon(Some(icon)).with_taskbar_icon(None),
                Err(_) => b,
            }
        };
        let window = Rc::new(builder.build(el)?);

        let mut ctx = spec.ctx;
        ctx["platform"] = serde_json::json!(if cfg!(windows) { "windows" } else { "macos" });
        ctx["lowercase"] = serde_json::json!(crate::style::lowercase());
        // `</` would end the inline <script> early.
        let ctx = ctx.to_string().replace("</", "<\\/");
        let mut html = spec.html.replacen("/*__CTX__*/null", &ctx, 1);
        if crate::style::lowercase() {
            // Typed text keeps its case.
            html = html.replacen(
                "</head>",
                "<style>body { text-transform: lowercase; } input, textarea, code, .keep-case { text-transform: none; }</style></head>",
                1,
            );
        }

        // WebView2 keeps its profile here (Windows); WebKit ignores it.
        let mut context = Box::new(WebContext::new(Some(crate::paths::webview_dir())));
        let drag_window = window.clone();
        let kind = spec.kind;
        let webview = WebViewBuilder::new_with_web_context(&mut context)
            .with_html(html)
            .with_background_color((13, 14, 18, 255))
            .with_devtools(cfg!(debug_assertions))
            // Pages are self-contained; never navigate away from them.
            .with_navigation_handler(|url| !url.starts_with("http"))
            .with_ipc_handler(move |req| {
                let body = req.body();
                if body.contains("\"type\":\"drag\"") {
                    let _ = drag_window.drag_window();
                    return;
                }
                match serde_json::from_str::<serde_json::Value>(body) {
                    Ok(v) => {
                        let _ = proxy.send_event(UserEvent::Web(kind, v));
                    }
                    Err(e) => tracing::warn!(error = %e, ?kind, "bad message from window"),
                }
            })
            .build(&*window)?;
        tracing::info!(?kind, "window opened");
        OPEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(Self { webview, _context: context, window, kind, ready: false, pending: Vec::new() })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn focus(&self) {
        activate_app();
        self.window.set_visible(true);
        self.window.set_focus();
    }

    /// Page finished loading: show the window (no white flash) and flush queued script.
    pub fn on_ready(&mut self) {
        self.ready = true;
        self.focus();
        for js in std::mem::take(&mut self.pending) {
            self.eval_now(&js);
        }
    }

    /// Run script in the page, queued until it has loaded.
    pub fn eval(&mut self, js: String) {
        if self.ready {
            self.eval_now(&js);
        } else {
            self.pending.push(js);
        }
    }

    /// Call `window.<func>(arg)` in the page.
    pub fn call(&mut self, func: &str, arg: &serde_json::Value) {
        self.eval(format!("window.{func} && window.{func}({})", arg.to_string().replace("</", "<\\/")));
    }

    fn eval_now(&self, js: &str) {
        if let Err(e) = self.webview.evaluate_script(js) {
            tracing::warn!(error = %e, kind = ?self.kind, "window script failed");
        }
    }
}
