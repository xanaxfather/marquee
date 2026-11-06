//! Media player watchers. Each runs on its own thread and reports a
//! [`Snapshot`] whenever something meaningful changes; the core picks the
//! player that is actually playing.
//!
//! | Watcher | macOS | Windows | How |
//! |---|---|---|---|
//! | IINA | ✓ | | mpv JSON IPC (push, no polling) |
//! | mpv / mpv.net | ✓ | ✓ | mpv JSON IPC |
//! | VLC | ✓ | ✓ | VLC web interface (`/requests/status.json`) |
//! | MPC-HC / MPC-BE | | ✓ | web interface (`/variables.html`) |
//! | Media Player, Movies & TV, … | | ✓ | System Media Transport Controls |
//! | QuickLook, PotPlayer, KMPlayer, GOM | | ✓ | window titles |

#[cfg(windows)]
pub mod mpc;
pub mod mpv;
#[cfg(windows)]
pub mod smtc;
pub mod vlc;
#[cfg(windows)]
pub mod wintitle;

use crate::config::Config;
use crate::core::Msg;
use std::sync::mpsc::Sender;
use std::time::Instant;

/// Stable id of a watcher ("IINA", "VLC", …).
pub type SourceId = &'static str;

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Display name of the app actually playing ("VLC", "Media Player", …).
    pub player: String,
    /// File path or URL. For players that only expose a title, the title.
    pub path: Option<String>,
    pub media_title: Option<String>,
    pub duration: Option<f64>,
    /// Position (seconds) as of `position_at`.
    pub position: Option<f64>,
    pub position_at: Instant,
    pub paused: bool,
    pub buffering: bool,
    pub speed: f64,
    pub idle: bool,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            player: String::new(),
            path: None,
            media_title: None,
            duration: None,
            position: None,
            position_at: Instant::now(),
            paused: false,
            buffering: false,
            speed: 1.0,
            idle: true,
        }
    }
}

impl Snapshot {
    pub fn is_playing(&self) -> bool {
        self.path.is_some() && !self.idle
    }

    /// Current position, extrapolated while playing.
    pub fn position_now(&self, now: Instant) -> Option<f64> {
        let p = self.position?;
        if self.paused || self.buffering || self.idle {
            return Some(p);
        }
        let elapsed = now.saturating_duration_since(self.position_at).as_secs_f64() * self.speed;
        Some(match self.duration {
            Some(d) => (p + elapsed).min(d),
            None => p + elapsed,
        })
    }

    /// Same playback state, ignoring position drift below 1.5 s.
    pub fn same_state(&self, other: &Snapshot) -> bool {
        if self.player != other.player
            || self.path != other.path
            || self.media_title != other.media_title
            || self.duration != other.duration
            || self.paused != other.paused
            || self.buffering != other.buffering
            || self.speed != other.speed
            || self.idle != other.idle
        {
            return false;
        }
        let now = Instant::now();
        match (self.position_now(now), other.position_now(now)) {
            (Some(x), Some(y)) => (x - y).abs() < 1.5,
            (None, None) => true,
            _ => false,
        }
    }
}

#[derive(Debug)]
pub enum PlayerEvent {
    Connected,
    Update(Snapshot),
    Disconnected,
}

/// Helper for watchers: send an event tagged with the watcher's id.
#[derive(Clone)]
pub struct Reporter {
    pub source: SourceId,
    tx: Sender<Msg>,
}

impl Reporter {
    pub fn new(source: SourceId, tx: Sender<Msg>) -> Self {
        Self { source, tx }
    }

    /// Returns false once the app is shutting down.
    pub fn send(&self, event: PlayerEvent) -> bool {
        self.tx.send(Msg::Player { source: self.source, event }).is_ok()
    }
}

fn enabled(cfg: &Config, name: &str) -> bool {
    !cfg.disabled_players.iter().any(|d| d.eq_ignore_ascii_case(name))
}

/// Start every watcher available on this OS.
pub fn spawn_all(cfg: &Config, tx: Sender<Msg>) {
    #[cfg(target_os = "macos")]
    if enabled(cfg, "IINA") {
        mpv::spawn(Reporter::new("IINA", tx.clone()), cfg.ipc_socket.clone());
    }
    if enabled(cfg, "mpv") {
        mpv::spawn(Reporter::new("mpv", tx.clone()), cfg.mpv_ipc.clone());
    }
    if enabled(cfg, "VLC") {
        vlc::spawn(Reporter::new("VLC", tx.clone()), cfg.vlc_port, cfg.vlc_password.clone());
    }
    #[cfg(windows)]
    {
        if enabled(cfg, "MPC") {
            mpc::spawn(Reporter::new("MPC", tx.clone()), cfg.mpc_port);
        }
        if enabled(cfg, "SMTC") {
            smtc::spawn(Reporter::new("SMTC", tx.clone()), cfg.smtc_ignore.clone());
        }
        if enabled(cfg, "Windows") {
            wintitle::spawn(Reporter::new("Windows", tx.clone()));
        }
    }
    let _ = tx;
}
