//! User configuration, stored as TOML in Application Support.

use crate::paths;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What Discord shows after "Watching" in the member list / status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusDisplay {
    /// "Watching <your Discord application name>"
    Name,
    /// "Watching <state line>"
    State,
    /// "Watching <title>" (default)
    Details,
}

/// Default large image when no artwork was picked for a title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtworkMode {
    /// Episode thumbnail (movies: poster).
    Episode,
    /// Season poster (movies: poster).
    Season,
    /// Show / movie poster.
    Poster,
    /// Wide backdrop.
    Backdrop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub discord_client_id: String,
    pub tmdb_api_key: String,
    pub language: String,
    /// IINA's mpv socket (macOS).
    pub ipc_socket: PathBuf,
    /// Standalone mpv / mpv.net socket or named pipe.
    pub mpv_ipc: PathBuf,
    pub vlc_port: u16,
    pub vlc_password: String,
    pub mpc_port: u16,
    /// Extra Windows media-session apps to ignore (substring of the app id).
    pub smtc_ignore: Vec<String>,
    /// Watchers to switch off: "IINA", "mpv", "VLC", "MPC", "SMTC", "Windows".
    pub disabled_players: Vec<String>,
    pub show_when_paused: bool,
    pub show_buttons: bool,
    pub show_extras: bool,
    pub status_display: StatusDisplay,
    pub artwork: ArtworkMode,
    pub small_image_playing: String,
    pub small_image_paused: String,
    pub fallback_image: String,
    pub log_level: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            discord_client_id: String::new(),
            tmdb_api_key: String::new(),
            language: "en-US".into(),
            ipc_socket: PathBuf::from("/tmp/marquee-iina.sock"),
            mpv_ipc: PathBuf::from(if cfg!(windows) { r"\\.\pipe\marquee-mpv" } else { "/tmp/marquee-mpv.sock" }),
            vlc_port: 8080,
            vlc_password: String::new(),
            mpc_port: 13579,
            smtc_ignore: Vec::new(),
            disabled_players: Vec::new(),
            show_when_paused: true,
            show_buttons: true,
            show_extras: true,
            status_display: StatusDisplay::Details,
            artwork: ArtworkMode::Episode,
            small_image_playing: String::new(),
            small_image_paused: String::new(),
            fallback_image: String::new(),
            log_level: "info".into(),
        }
    }
}

const TEMPLATE: &str = r#"# Marquee configuration
# Changes take effect after choosing "Reload Config" in the menu
# (socket/port changes need a restart of Marquee and the player).

# 1. Create an application at https://discord.com/developers/applications
#    Its name is what Discord shows ("Watching <name>") — e.g. "Movies" or "TV".
#    Paste its Application ID here:
discord_client_id = ""

# 2. Free TMDB key: https://www.themoviedb.org/settings/api
#    Either the v3 "API Key" or the v4 "API Read Access Token" works.
tmdb_api_key = ""

# Metadata language (TMDB language code).
language = "en-US"

# ---- Players -------------------------------------------------------------
# "Set Up Players…" in the menu fills these in and configures each player.
# IINA's mpv socket (macOS):
ipc_socket = "/tmp/marquee-iina.sock"
# mpv / mpv.net socket (macOS) or named pipe (Windows: \\.\pipe\marquee-mpv):
# mpv_ipc = "/tmp/marquee-mpv.sock"
# VLC web interface:
vlc_port = 8080
vlc_password = ""
# MPC-HC / MPC-BE web interface (Windows):
mpc_port = 13579
# Windows media sessions to ignore, e.g. ["Netflix"]:
smtc_ignore = []
# Turn watchers off: "IINA", "mpv", "VLC", "MPC", "SMTC", "Windows"
disabled_players = []

# ---- Presence ------------------------------------------------------------
# Keep the presence visible (marked as paused) while playback is paused.
show_when_paused = true

# "View on TMDB" / "View on IMDb" buttons. Discord shows them to other
# people, not on your own profile; the title and artwork link to TMDB too.
show_buttons = true

# Genres and rating for movies, air date for episodes.
show_extras = true

# Discord status line: "details" = "Watching Breaking Bad",
# "state" = "Watching S2 - E3 · Bit by a Dead Bee", "name" = "Watching <app name>".
status_display = "details"

# Default artwork: "episode" (episode thumbnail), "season" (season poster),
# "poster" (show/movie poster) or "backdrop". Movies use their poster for
# "episode"/"season". Pick a specific image per title with "Choose Artwork…".
artwork = "episode"

# Optional small corner images: an asset key uploaded to your Discord
# application (Rich Presence → Art Assets) or an https:// image URL.
small_image_playing = ""
small_image_paused = ""

# Large image when no artwork could be found (asset key or URL). Empty = none.
fallback_image = ""

# trace | debug | info | warn | error  (RUST_LOG overrides this)
log_level = "info"
"#;

impl Config {
    /// Load the config file, creating a commented template on first run.
    /// Environment variables override file values (handy for testing).
    pub fn load() -> Result<Self> {
        let path = paths::config_file();
        let mut cfg = if path.exists() {
            let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            toml::from_str::<Config>(&text).with_context(|| format!("parsing {}", path.display()))?
        } else {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            }
            std::fs::write(&path, TEMPLATE).with_context(|| format!("writing {}", path.display()))?;
            Config::default()
        };
        if let Ok(v) = std::env::var("MARQUEE_DISCORD_ID") {
            cfg.discord_client_id = v;
        }
        if let Ok(v) = std::env::var("MARQUEE_TMDB_KEY") {
            cfg.tmdb_api_key = v;
        }
        if let Some(v) = std::env::var_os("MARQUEE_IINA_SOCKET") {
            cfg.ipc_socket = PathBuf::from(v);
        }
        if let Some(v) = std::env::var_os("MARQUEE_MPV_IPC") {
            cfg.mpv_ipc = PathBuf::from(v);
        }
        cfg.discord_client_id = cfg.discord_client_id.trim().to_string();
        cfg.tmdb_api_key = cfg.tmdb_api_key.trim().to_string();
        Ok(cfg)
    }

    /// Load, falling back to defaults if the file is broken. The error is
    /// returned so it can be logged and shown once logging is up.
    pub fn load_or_default() -> (Self, Option<String>) {
        match Self::load() {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(format!("{e:#}"))),
        }
    }

    /// One-line summary for diagnostics with secrets redacted.
    pub fn summary(&self) -> String {
        format!(
            "discord_client_id={} tmdb_api_key={} language={} iina_socket={} mpv_ipc={} vlc_port={} vlc_password={} mpc_port={} disabled={:?} show_when_paused={} status_display={:?}",
            if self.discord_client_id.is_empty() { "<unset>" } else { &self.discord_client_id },
            redact(&self.tmdb_api_key),
            self.language,
            self.ipc_socket.display(),
            self.mpv_ipc.display(),
            self.vlc_port,
            redact(&self.vlc_password),
            self.mpc_port,
            self.disabled_players,
            self.show_when_paused,
            self.status_display,
        )
    }
}

/// Set `key = <toml value>` in the config file, keeping comments and layout.
pub fn persist_value(key: &str, toml_value: &str) -> Result<()> {
    let path = paths::config_file();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut found = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|l| {
            let t = l.trim_start();
            if !found && t.strip_prefix(key).is_some_and(|r| r.trim_start().starts_with('=')) {
                found = true;
                format!("{key} = {toml_value}")
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        lines.push(format!("{key} = {toml_value}"));
    }
    std::fs::write(&path, lines.join("\n") + "\n").with_context(|| format!("writing {}", path.display()))
}

fn redact(s: &str) -> String {
    if s.is_empty() { "<unset>".into() } else { format!("<set, {} chars>", s.len()) }
}
