//! "Choose Artwork…" window.

use crate::artwork::{Scope, Target};
use crate::tray::UserEvent;
use crate::webwin::{Kind, Spec, WebWindow};
use serde::Deserialize;
use tao::event_loop::{EventLoopProxy, EventLoopWindowTarget};

/// Messages posted by the page.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PickerMsg {
    Ready,
    Apply { scope: Scope, url: String },
    Reset { scope: Scope },
    Open { url: String },
    Close,
}

pub struct Picker {
    pub win: WebWindow,
    pub target: Target,
}

impl Picker {
    pub fn open(
        el: &EventLoopWindowTarget<UserEvent>,
        proxy: EventLoopProxy<UserEvent>,
        target: Target,
    ) -> anyhow::Result<Self> {
        let win = WebWindow::open(
            el,
            proxy,
            Spec {
                kind: Kind::Artwork,
                title: "Choose Artwork — Marquee",
                size: (1060.0, 700.0),
                min_size: (820.0, 560.0),
                html: include_str!("picker.html"),
                ctx: serde_json::to_value(&target)?,
            },
        )?;
        tracing::info!(tmdb_id = target.tmdb_id, kind = ?target.kind, "artwork picker opened");
        Ok(Self { win, target })
    }

    pub fn deliver_options(&mut self, result: Result<serde_json::Value, String>) {
        match result {
            Ok(v) => self.win.call("__options", &v),
            Err(e) => self.win.call("__error", &serde_json::json!(e)),
        }
    }
}
