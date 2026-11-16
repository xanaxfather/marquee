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

impl WebWindow {
    pub fn open(
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
        Ok(Self { webview, _context: context, window, kind, ready: false, pending: Vec::new() })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn focus(&self) {
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
