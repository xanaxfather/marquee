//! The optional all-lowercase look (`lowercase = true` in the config).
//!
//! Applies to text people read (menu, tooltip, notifications, picker); never
//! to paths, URLs or registry names. The Discord card reads `Config::lowercase`
//! directly, see `presence::build`.

use crate::paths;
use std::sync::atomic::{AtomicBool, Ordering};

static LOWERCASE: AtomicBool = AtomicBool::new(false);
static ANIMATED_BACKGROUND: AtomicBool = AtomicBool::new(true);

pub fn set_lowercase(on: bool) {
    LOWERCASE.store(on, Ordering::Relaxed);
}

pub fn lowercase() -> bool {
    LOWERCASE.load(Ordering::Relaxed)
}

/// The faint ASCII pattern behind Marquee's windows (`animated_background`).
pub fn set_animated_background(on: bool) {
    ANIMATED_BACKGROUND.store(on, Ordering::Relaxed);
}

pub fn animated_background() -> bool {
    ANIMATED_BACKGROUND.load(Ordering::Relaxed)
}

/// `s` in the chosen case.
pub fn text(s: &str) -> String {
    if lowercase() { s.to_lowercase() } else { s.to_string() }
}

/// "Marquee", or "marquee".
pub fn app_name() -> String {
    text(paths::APP_NAME)
}
