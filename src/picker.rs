//! "Choose Artwork…" window: a WebKit view (system framework, no bundled
//! browser) that exists only while open — closing it frees the web process.

use crate::artwork::{Scope, Target};
use crate::tray::UserEvent;
use serde::Deserialize;
use std::rc::Rc;
use tao::dpi::LogicalSize;
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};
use tao::window::{Window, WindowBuilder, WindowId};
use wry::{WebContext, WebView, WebViewBuilder};

/// Messages posted by the page with `window.ipc.postMessage`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PickerMsg {
    Ready,
    Apply { scope: Scope, url: String },
    Reset { scope: Scope },
    Open { url: String },
    Close,
    Drag,
}

pub struct Picker {
    // Field order matters: the webview must drop before its context and window.
    webview: WebView,
    _context: Box<WebContext>,
    window: Rc<Window>,
    pub target: Target,
    ready: bool,
    pending: Option<String>,
}

impl Picker {
    pub fn open(
        el: &EventLoopWindowTarget<UserEvent>,
        proxy: EventLoopProxy<UserEvent>,
        target: Target,
    ) -> anyhow::Result<Self> {
        let builder = WindowBuilder::new()
            .with_title(crate::style::text("Choose Artwork — Marquee"))
            .with_inner_size(LogicalSize::new(1060.0, 700.0))
            .with_min_inner_size(LogicalSize::new(820.0, 560.0))
            .with_visible(false);
        // macOS: content under a transparent title bar (traffic lights float over it).
        #[cfg(target_os = "macos")]
        let builder = {
            use tao::platform::macos::WindowBuilderExtMacOS;
            builder.with_titlebar_transparent(true).with_title_hidden(true).with_fullsize_content_view(true)
        };
        // Windows: the standard title bar, in dark mode to match the page.
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

        // `</` would end the inline <script> early.
        let mut ctx_json = serde_json::to_value(&target)?;
        ctx_json["platform"] = serde_json::json!(if cfg!(windows) { "windows" } else { "macos" });
        let ctx = ctx_json.to_string().replace("</", "<\\/");
        let mut html = include_str!("picker.html").replacen("/*__CTX__*/null", &ctx, 1);
        if crate::style::lowercase() {
            // Typed URLs keep their case.
            html = html.replacen(
                "</head>",
                "<style>body { text-transform: lowercase; } input { text-transform: none; }</style></head>",
                1,
            );
        }

        // WebView2 keeps its profile here (Windows); WebKit ignores it.
        let mut context = Box::new(WebContext::new(Some(crate::paths::webview_dir())));
        let drag_window = window.clone();
        let webview = WebViewBuilder::new_with_web_context(&mut context)
            .with_html(html)
            .with_background_color((13, 14, 18, 255))
            .with_devtools(cfg!(debug_assertions))
            // The page is self-contained; never navigate away from it.
            .with_navigation_handler(|url| !url.starts_with("http"))
            .with_ipc_handler(move |req| match serde_json::from_str::<PickerMsg>(req.body()) {
                Ok(PickerMsg::Drag) => {
                    let _ = drag_window.drag_window();
                }
                Ok(msg) => {
                    let _ = proxy.send_event(UserEvent::Picker(msg));
                }
                Err(e) => tracing::warn!(error = %e, body = req.body(), "bad message from artwork picker"),
            })
            .build(&*window)?;

        tracing::info!(tmdb_id = target.tmdb_id, kind = ?target.kind, "artwork picker opened");
        Ok(Self { webview, _context: context, window, target, ready: false, pending: None })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn focus(&self) {
        self.window.set_visible(true);
        self.window.set_focus();
    }

    /// Page finished loading: show the window (no white flash) and flush data.
    pub fn on_ready(&mut self) {
        self.ready = true;
        self.focus();
        if let Some(js) = self.pending.take() {
            self.eval(&js);
        }
    }

    pub fn deliver_options(&mut self, result: Result<serde_json::Value, String>) {
        let js = match result {
            Ok(v) => format!("window.__options({v})"),
            Err(e) => format!("window.__error({})", serde_json::to_string(&e).unwrap_or_default()),
        };
        if self.ready {
            self.eval(&js);
        } else {
            self.pending = Some(js);
        }
    }

    fn eval(&self, js: &str) {
        if let Err(e) = self.webview.evaluate_script(js) {
            tracing::warn!(error = %e, "artwork picker script failed");
        }
    }
}
