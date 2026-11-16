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

/// How a title matched by the privacy lists appears on Discord.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrivacyMode {
    /// Nothing is shown.
    Hide,
    /// "Watching something", no title, artwork or links.
    Generic,
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
    /// Buttons in order (Discord shows the first two): "tmdb", "imdb",
    /// "letterboxd", "trakt", "trailer", "wikipedia", or "Label | https://…{title}…".
    pub buttons: Vec<String>,
    pub show_extras: bool,
    /// Line templates; see `presence::PLACEHOLDERS`. Empty = built-in default.
    pub episode_details: String,
    pub episode_state: String,
    pub episode_image_text: String,
    pub movie_details: String,
    pub movie_state: String,
    pub movie_image_text: String,
    /// Privacy: titles (exact, or ending in `*` for a prefix), folders and genres to keep off Discord.
    pub hide_titles: Vec<String>,
    pub hide_folders: Vec<String>,
    pub hide_genres: Vec<String>,
    pub privacy_mode: PrivacyMode,
    /// Keep a local watch history (Recently Watched, stats).
    pub history: bool,
    pub check_updates: bool,
    /// The welcome window has been completed.
    pub setup_complete: bool,
    pub status_display: StatusDisplay,
    pub artwork: ArtworkMode,
    pub small_image_playing: String,
    pub small_image_paused: String,
    pub fallback_image: String,
    /// All-lowercase look: Discord card, menu, notifications ("marquee").
    pub lowercase: bool,
    pub log_level: String,
}

/// Marquee's own corner badges (assets/presence/, see scripts/make_presence_icons.py).
pub const BADGE_PLAYING: &str =
    "https://raw.githubusercontent.com/xanaxfather/marquee/main/assets/presence/playing.png";
pub const BADGE_PAUSED: &str = "https://raw.githubusercontent.com/xanaxfather/marquee/main/assets/presence/paused.png";

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
            buttons: vec!["tmdb".into(), "imdb".into()],
            show_extras: true,
            episode_details: String::new(),
            episode_state: String::new(),
            episode_image_text: String::new(),
            movie_details: String::new(),
            movie_state: String::new(),
            movie_image_text: String::new(),
            hide_titles: Vec::new(),
            hide_folders: Vec::new(),
            hide_genres: Vec::new(),
            privacy_mode: PrivacyMode::Hide,
            history: true,
            check_updates: true,
            setup_complete: false,
            status_display: StatusDisplay::Details,
            artwork: ArtworkMode::Episode,
            small_image_playing: BADGE_PLAYING.into(),
            small_image_paused: BADGE_PAUSED.into(),
            fallback_image: String::new(),
            lowercase: false,
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

# Link buttons. Discord shows them to other people, not on your own profile;
# the title and artwork link to the first one too.
show_buttons = true
# In order; Discord shows the first two that apply. Choices: "tmdb", "imdb",
# "letterboxd" (movies), "trakt", "trailer" (YouTube search), "wikipedia",
# or your own: "Label | https://example.com/search?q={title}".
buttons = ["tmdb", "imdb"]

# Genres and rating for movies, air date for episodes.
show_extras = true

# Discord status line: "details" = "Watching Breaking Bad",
# "state" = "Watching S2 - E3 · Bit by a Dead Bee", "name" = "Watching <app name>".
status_display = "details"

# Line templates (empty = default). Placeholders: {title} {year} {label}
# ("S2 - E3") {season} {episode} {episode_title} {air_date} {air_year}
# {genres} {rating} {tagline} {player}. Parts separated by " · " vanish when
# their placeholders are empty; {a|b} uses b when a is empty.
episode_details = ""      # default: {title}
episode_state = ""        # default: {label} · {episode_title}
episode_image_text = ""   # default: {title} ({year}) · aired {air_year}
movie_details = ""        # default: {title}
movie_state = ""          # default: {year} · {genres} · ★ {rating}
movie_image_text = ""     # default: {tagline|title}

# ---- Privacy -------------------------------------------------------------
# Keep titles off Discord: exact names (or "Prefix*"), folders, TMDB genres.
hide_titles = []
hide_folders = []
hide_genres = []
# "hide" = show nothing, "generic" = "Watching something" without details.
privacy_mode = "hide"
# Keep a local watch history (Recently Watched in the menu, stats in Settings).
history = true

# Default artwork: "episode" (episode thumbnail), "season" (season poster),
# "poster" (show/movie poster) or "backdrop". Movies use their poster for
# "episode"/"season". Pick a specific image per title with "Choose Artwork…".
artwork = "episode"

# Small corner badge over the artwork: an asset key uploaded to your Discord
# application (Rich Presence → Art Assets) or an https:// image URL.
# Defaults to Marquee's play/pause badges; set to "" for none.
small_image_playing = "https://raw.githubusercontent.com/xanaxfather/marquee/main/assets/presence/playing.png"
small_image_paused = "https://raw.githubusercontent.com/xanaxfather/marquee/main/assets/presence/paused.png"

# Large image when no artwork could be found (asset key or URL). Empty = none.
fallback_image = ""

# All-lowercase look: the Discord card, menu and notifications ("marquee",
# "breaking bad", "s2 - e3 · bit by a fly"). Your Discord application's name
# ("Watching <name>") comes from the Developer Portal; rename it there too.
lowercase = false

# Check GitHub for new versions once a day.
check_updates = true

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
        crate::style::set_lowercase(cfg.lowercase);
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
            "discord_client_id={} tmdb_api_key={} language={} iina_socket={} mpv_ipc={} vlc_port={} vlc_password={} mpc_port={} disabled={:?} show_when_paused={} status_display={:?} lowercase={}",
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
            self.lowercase,
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

/// Save settings from the Settings window: `{key: json value}`. Unknown keys
/// and values of the wrong type are refused before anything is written.
pub fn persist_json(values: &serde_json::Map<String, serde_json::Value>) -> Result<()> {
    let mut merged = serde_json::to_value(Config::load().unwrap_or_default())?;
    for (k, v) in values {
        let slot = merged.get_mut(k).with_context(|| format!("unknown setting {k}"))?;
        *slot = v.clone();
    }
    // Type check: the merged result must still be a valid config.
    serde_json::from_value::<Config>(merged).context("invalid settings")?;
    for (k, v) in values {
        let tv = toml::Value::try_from(v).with_context(|| format!("setting {k}"))?;
        persist_value(k, &tv.to_string())?;
    }
    Ok(())
}

/// The config with `values` applied (for live previews; nothing is saved).
pub fn with_values(base: &Config, values: &serde_json::Map<String, serde_json::Value>) -> Config {
    let mut v = serde_json::to_value(base).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        for (k, val) in values {
            if o.contains_key(k) {
                o.insert(k.clone(), val.clone());
            }
        }
    }
    serde_json::from_value(v).unwrap_or_else(|_| base.clone())
}

fn redact(s: &str) -> String {
    if s.is_empty() { "<unset>".into() } else { format!("<set, {} chars>", s.len()) }
}
