//! The app's brain: one thread that reacts to player events, metadata results
//! and menu commands, and keeps Discord in sync. It sleeps in `recv` whenever
//! nothing is scheduled, so an idle app uses no CPU at all.

use crate::artwork::{self, Target};
use crate::config::Config;
use crate::discord::{Discord, DiscordError};
use crate::metadata::{Resolution, Resolver};
use crate::parse::{self, FileKind, Guess};
use crate::platform::notify;
use crate::players::{PlayerEvent, Snapshot, SourceId};
use crate::{presence, setup};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// Discord allows ~5 activity updates per 20 s; stay well below that.
const MIN_UPDATE_INTERVAL: Duration = Duration::from_secs(2);
/// Coalesce bursts (new file, seek) so the player's position reply lands first.
const DEBOUNCE: Duration = Duration::from_millis(300);
const DISCORD_RETRY_MIN: Duration = Duration::from_secs(5);
const DISCORD_RETRY_MAX: Duration = Duration::from_secs(60);
const DISCORD_ALIVE_CHECK: Duration = Duration::from_secs(30);
const RESOLVE_RETRY_MIN: Duration = Duration::from_secs(30);
const RESOLVE_RETRY_MAX: Duration = Duration::from_secs(600);

// Messages are moved, not copied, and arrive a few times per second at most.
#[allow(clippy::large_enum_variant)]
pub enum Msg {
    Player { source: SourceId, event: PlayerEvent },
    Resolved(Resolution),
    ArtworkOptions { tmdb_id: u64, result: Result<Value, String> },
    Command(Command),
}

#[derive(Debug, Clone)]
pub enum Command {
    Reconnect,
    SetEnabled(bool),
    ReloadConfig,
    ClearCache,
    SetupPlayers,
    OpenArtwork,
    SetArtwork { key: String, url: Option<String> },
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Health {
    Good,
    #[default]
    Waiting,
    Problem,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum NowState {
    Playing,
    Paused,
    #[default]
    Idle,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UiStatus {
    pub now_state: NowState,
    pub discord_health: Health,
    pub player_health: Health,
    pub now: String,
    pub discord: String,
    pub player: String,
    pub enabled: bool,
    pub needs_setup: bool,
    pub can_pick_artwork: bool,
}

pub enum UiEvent {
    Status(UiStatus),
    OpenArtwork(Box<Target>),
    ArtworkOptions { tmdb_id: u64, result: Result<Value, String> },
    Exit,
}

enum Job {
    Resolve(String),
    Clear,
    Reconfigure { key: String, language: String },
    Artwork(Box<Target>),
}

fn spawn_resolver(cfg: &Config, tx: Sender<Msg>) -> Sender<Job> {
    let (job_tx, job_rx) = std::sync::mpsc::channel::<Job>();
    let (key, lang) = (cfg.tmdb_api_key.clone(), cfg.language.clone());
    std::thread::Builder::new()
        .name("metadata".into())
        .spawn(move || {
            let mut resolver = Resolver::new(&key, &lang);
            while let Ok(job) = job_rx.recv() {
                match job {
                    Job::Resolve(path) => {
                        // Skip stale jobs if the user is flicking through a playlist.
                        let res = resolver.resolve(&path);
                        if tx.send(Msg::Resolved(res)).is_err() {
                            return;
                        }
                    }
                    Job::Clear => resolver.clear_cache(),
                    Job::Reconfigure { key, language } => resolver = Resolver::new(&key, &language),
                    Job::Artwork(target) => {
                        let result = resolver.artwork_options(&target).map_err(|e| e.to_string());
                        if let Err(e) = &result {
                            tracing::warn!(error = %e, "could not load artwork options");
                        }
                        if tx.send(Msg::ArtworkOptions { tmdb_id: target.tmdb_id, result }).is_err() {
                            return;
                        }
                    }
                }
            }
        })
        .expect("spawn metadata thread");
    job_tx
}

struct SourceState {
    connected: bool,
    snap: Snapshot,
    /// When it last started playing (new file or resumed) — newest wins.
    since: Instant,
}

pub struct Core<F: Fn(UiEvent)> {
    cfg: Config,
    rx: Receiver<Msg>,
    ui: F,
    resolver: Sender<Job>,
    artwork: artwork::Store,
    discord: Discord,
    discord_retry_at: Option<Instant>,
    discord_backoff: Duration,
    discord_error: Option<String>,
    discord_app_name: Option<String>,
    alive_check_at: Option<Instant>,

    /// Every watcher's latest state; `active` is the one shown on Discord.
    sources: HashMap<SourceId, SourceState>,
    active: Option<SourceId>,
    player_connected: bool,
    snap: Snapshot,
    resolution: Option<Resolution>,
    resolve_retry_at: Option<Instant>,
    resolve_backoff: Duration,
    setup_needed: Vec<&'static str>,
    setup_just_done: bool,

    enabled: bool,
    config_error: Option<String>,
    /// `None` = unknown (fresh connection); `Some(None)` = cleared.
    last_sent: Option<Option<Value>>,
    last_send_at: Option<Instant>,
    flush_at: Option<Instant>,
    last_ui: Option<UiStatus>,
}

impl<F: Fn(UiEvent)> Core<F> {
    pub fn new(cfg: Config, tx: Sender<Msg>, rx: Receiver<Msg>, ui: F) -> Self {
        let resolver = spawn_resolver(&cfg, tx);
        let setup_needed = setup::needing_setup(&cfg);
        if !setup_needed.is_empty() {
            tracing::warn!(players = ?setup_needed, "installed players not set up yet; use \"Set Up Players…\" in the menu");
        }
        Self {
            discord: Discord::new(&cfg.discord_client_id),
            cfg,
            rx,
            ui,
            resolver,
            artwork: artwork::Store::open(),
            discord_retry_at: Some(Instant::now()),
            discord_backoff: DISCORD_RETRY_MIN,
            discord_error: None,
            discord_app_name: None,
            alive_check_at: None,
            sources: HashMap::new(),
            active: None,
            player_connected: false,
            snap: Snapshot::default(),
            resolution: None,
            resolve_retry_at: None,
            resolve_backoff: RESOLVE_RETRY_MIN,
            setup_needed,
            setup_just_done: false,
            enabled: true,
            config_error: None,
            last_sent: None,
            last_send_at: None,
            flush_at: None,
            last_ui: None,
        }
    }

    pub fn with_config_error(mut self, e: Option<String>) -> Self {
        self.config_error = e;
        self
    }

    pub fn run(mut self) {
        self.push_ui();
        loop {
            let now = Instant::now();
            let next = [self.flush_at, self.discord_retry_at, self.alive_check_at, self.resolve_retry_at]
                .into_iter()
                .flatten()
                .min();
            let msg = match next {
                Some(t) => match self.rx.recv_timeout(t.saturating_duration_since(now)) {
                    Ok(m) => Some(m),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match self.rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return,
                },
            };
            if let Some(msg) = msg
                && !self.handle(msg)
            {
                break;
            }
            self.timers();
            self.push_ui();
        }
        // Quit: clear the activity so it doesn't linger.
        if self.discord.is_connected() {
            let _ = self.discord.set_activity(None);
        }
        tracing::info!("shutting down");
        (self.ui)(UiEvent::Exit);
    }

    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Player { source, event } => self.on_player(source, event),
            Msg::Resolved(r) => {
                if self.snap.path.as_deref() == Some(r.path.as_str()) {
                    if r.transient {
                        self.resolve_retry_at = Some(Instant::now() + self.resolve_backoff);
                        tracing::info!(
                            retry_in_secs = self.resolve_backoff.as_secs(),
                            "metadata incomplete; will retry"
                        );
                        self.resolve_backoff = (self.resolve_backoff * 2).min(RESOLVE_RETRY_MAX);
                    } else {
                        self.resolve_retry_at = None;
                        self.resolve_backoff = RESOLVE_RETRY_MIN;
                    }
                    // Don't let a failed retry wipe out metadata we already have.
                    let keep_old =
                        r.media.is_none() && r.transient && self.resolution.as_ref().is_some_and(|o| o.media.is_some());
                    if !keep_old {
                        self.resolution = Some(r);
                    }
                    self.schedule_flush();
                }
            }
            Msg::ArtworkOptions { tmdb_id, result } => {
                (self.ui)(UiEvent::ArtworkOptions { tmdb_id, result });
            }
            Msg::Command(cmd) => {
                tracing::info!(?cmd, "menu command");
                match cmd {
                    Command::Quit => return false,
                    Command::SetEnabled(on) => {
                        self.enabled = on;
                        self.schedule_flush();
                    }
                    Command::Reconnect => {
                        self.discord.disconnect();
                        self.discord_backoff = DISCORD_RETRY_MIN;
                        self.discord_retry_at = Some(Instant::now());
                        self.alive_check_at = None;
                    }
                    Command::ReloadConfig => self.reload_config(),
                    Command::ClearCache => {
                        let _ = self.resolver.send(Job::Clear);
                        self.re_resolve();
                    }
                    Command::SetupPlayers => self.setup_players(),
                    Command::OpenArtwork => self.open_artwork(),
                    Command::SetArtwork { key, url } => {
                        self.artwork.set(&key, url);
                        self.schedule_flush();
                    }
                }
            }
        }
        true
    }

    fn on_player(&mut self, source: SourceId, event: PlayerEvent) {
        let entry = self.sources.entry(source).or_insert_with(|| SourceState {
            connected: false,
            snap: Snapshot::default(),
            since: Instant::now(),
        });
        match event {
            PlayerEvent::Connected => {
                entry.connected = true;
                self.setup_just_done = false;
            }
            PlayerEvent::Disconnected => {
                entry.connected = false;
                entry.snap = Snapshot::default();
                self.setup_needed = setup::needing_setup(&self.cfg);
            }
            PlayerEvent::Update(s) => {
                let started = s.is_playing()
                    && (!entry.snap.is_playing() || entry.snap.path != s.path || (entry.snap.paused && !s.paused));
                if started {
                    entry.since = Instant::now();
                }
                entry.connected = true;
                entry.snap = s;
            }
        }
        self.select_active();
    }

    /// The player shown on Discord: the most recently started one that is
    /// playing, else the most recent paused one.
    fn select_active(&mut self) {
        let best = self
            .sources
            .iter()
            .filter(|(_, st)| st.connected && st.snap.is_playing())
            .max_by_key(|(_, st)| (!st.snap.paused, st.since))
            .map(|(id, _)| *id);
        if best != self.active {
            tracing::info!(from = ?self.active, to = ?best, "active player changed");
            self.active = best;
        }
        self.player_connected = best.is_some();
        let snap = best.map(|id| self.sources[id].snap.clone()).unwrap_or_default();
        self.apply_snapshot(snap);
    }

    fn apply_snapshot(&mut self, s: Snapshot) {
        let path_changed = s.path != self.snap.path;
        let was = (self.snap.paused, self.snap.is_playing());
        self.snap = s;
        if path_changed {
            self.on_new_path();
        }
        let is = (self.snap.paused, self.snap.is_playing());
        if was != is && !path_changed {
            tracing::info!(paused = self.snap.paused, playing = self.snap.is_playing(), "playback state changed");
        }
        self.schedule_flush();
    }

    fn on_new_path(&mut self) {
        self.resolution = None;
        self.resolve_retry_at = None;
        self.resolve_backoff = RESOLVE_RETRY_MIN;
        let Some(path) = self.snap.path.clone() else { return };
        let kind = parse::file_kind(&path);
        let is_url = path.contains("://");
        tracing::info!(path, ?kind, player = %self.snap.player, "now playing");
        match kind {
            FileKind::Audio | FileKind::Image => {
                tracing::info!(path, "not a video; presence will be cleared");
            }
            _ if is_url && kind != FileKind::Video => {
                // Streams (YouTube etc.): use the player's title, no TMDB lookup.
                self.resolution = Some(Resolution {
                    path: path.clone(),
                    guess: Guess {
                        title: String::new(),
                        year: None,
                        episode: None,
                        country: None,
                        episode_title: None,
                    },
                    media: None,
                    transient: false,
                    error: None,
                });
            }
            _ => {
                let _ = self.resolver.send(Job::Resolve(path));
            }
        }
    }

    fn current_media(&self) -> Option<&crate::metadata::Media> {
        if !self.player_connected || !self.snap.is_playing() {
            return None;
        }
        self.resolution.as_ref()?.media.as_ref()
    }

    fn open_artwork(&mut self) {
        let Some(m) = self.current_media() else {
            notify("Play something first — artwork can be chosen once it's identified.");
            return;
        };
        let activity = presence::build(&self.cfg, &self.snap, self.resolution.as_ref(), None, Instant::now());
        let target = Target {
            kind: m.kind,
            tmdb_id: m.tmdb_id,
            season: m.episode.as_ref().map(|e| e.number.season),
            episode: m.episode.as_ref().map(|e| e.number.episode),
            language: self.cfg.language.clone(),
            app_name: self.discord_app_name.clone().unwrap_or_else(|| "Marquee".into()),
            details: activity["details"].as_str().unwrap_or("").to_string(),
            state: activity["state"].as_str().unwrap_or("").trim_start_matches("Paused · ").to_string(),
            position: self.snap.position_now(Instant::now()),
            paused: self.snap.paused,
            current: artwork::effective(&self.cfg, &self.artwork, m),
            default: artwork::default_for(self.cfg.artwork, m),
            overrides: self.artwork.overrides(m),
            tmdb_url: m.tmdb_url(),
        };
        let _ = self.resolver.send(Job::Artwork(Box::new(target.clone())));
        (self.ui)(UiEvent::OpenArtwork(Box::new(target)));
    }

    fn re_resolve(&mut self) {
        if self.snap.path.is_some() {
            self.on_new_path();
            self.schedule_flush();
        }
    }

    fn reload_config(&mut self) {
        let new = match Config::load() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = format!("{e:#}"), "config reload failed; keeping current settings");
                notify("Config not reloaded: the file has an error (see Open Logs Folder).");
                return;
            }
        };
        tracing::info!(config = new.summary(), "config reloaded");
        self.config_error = None;
        if new.ipc_socket != self.cfg.ipc_socket || new.mpv_ipc != self.cfg.mpv_ipc || new.mpc_port != self.cfg.mpc_port
        {
            tracing::warn!("player socket/port changed; restart Marquee (and the player) for it to take effect");
        }
        crate::players::vlc::set_auth(new.vlc_port, &new.vlc_password);
        if new.discord_client_id != self.cfg.discord_client_id {
            self.discord.disconnect();
            self.discord = Discord::new(&new.discord_client_id);
            self.discord_error = None;
            self.discord_backoff = DISCORD_RETRY_MIN;
            self.discord_retry_at = Some(Instant::now());
        }
        if new.tmdb_api_key != self.cfg.tmdb_api_key || new.language != self.cfg.language {
            let _ =
                self.resolver.send(Job::Reconfigure { key: new.tmdb_api_key.clone(), language: new.language.clone() });
            self.cfg = new;
            self.re_resolve();
        } else {
            self.cfg = new;
        }
        self.last_sent = None;
        self.schedule_flush();
    }

    fn setup_players(&mut self) {
        let results = setup::configure_all(&mut self.cfg);
        if results.is_empty() {
            notify("No supported players found. Install IINA, VLC, mpv or MPC-HC, then try again.");
            return;
        }
        let ok: Vec<&str> = results.iter().filter(|(_, r)| r.is_ok()).map(|(n, _)| *n).collect();
        let failed: Vec<&str> = results.iter().filter(|(_, r)| r.is_err()).map(|(n, _)| *n).collect();
        self.setup_needed = setup::needing_setup(&self.cfg);
        self.setup_just_done = !ok.is_empty();
        let mut msg = String::new();
        if !ok.is_empty() {
            msg.push_str(&format!(
                "Set up {}. Quit and reopen {} once.",
                ok.join(", "),
                if ok.len() == 1 { "it" } else { "them" }
            ));
        }
        if !failed.is_empty() {
            msg.push_str(&format!(" Could not set up {} — see the log.", failed.join(", ")));
        }
        notify(msg.trim());
    }

    fn schedule_flush(&mut self) {
        let now = Instant::now();
        let earliest = self.last_send_at.map(|t| t + MIN_UPDATE_INTERVAL).unwrap_or(now).max(now + DEBOUNCE);
        self.flush_at = Some(match self.flush_at {
            Some(t) if t < earliest => t,
            _ => earliest,
        });
    }

    fn desired(&self) -> Option<Value> {
        if !self.enabled || !self.player_connected || !self.snap.is_playing() {
            return None;
        }
        let path = self.snap.path.as_deref()?;
        if matches!(parse::file_kind(path), FileKind::Audio | FileKind::Image) {
            return None;
        }
        if self.snap.paused && !self.cfg.show_when_paused {
            return None;
        }
        let image = self
            .resolution
            .as_ref()
            .and_then(|r| r.media.as_ref())
            .and_then(|m| artwork::effective(&self.cfg, &self.artwork, m));
        Some(presence::build(&self.cfg, &self.snap, self.resolution.as_ref(), image, Instant::now()))
    }

    fn flush(&mut self) {
        self.flush_at = None;
        self.artwork.refresh();
        if !self.discord.is_connected() {
            return;
        }
        let desired = self.desired();
        let unchanged = match (&self.last_sent, &desired) {
            (Some(None), None) => true,
            (Some(Some(a)), Some(b)) => presence::same(a, b),
            _ => false,
        };
        if unchanged {
            return;
        }
        self.last_send_at = Some(Instant::now());
        let mut result = self.discord.set_activity(desired.as_ref());
        // Older Discord builds may not know `status_display_type`; retry without it.
        if let (Err(DiscordError::Rejected(m)), Some(a)) = (&result, &desired)
            && a.get("status_display_type").is_some()
        {
            tracing::warn!(error = %m, "Discord rejected the activity; retrying without status_display_type");
            let mut plain = a.clone();
            plain.as_object_mut().map(|o| o.remove("status_display_type"));
            result = self.discord.set_activity(Some(&plain));
        }
        match result {
            Ok(()) => {
                match &desired {
                    Some(a) => tracing::info!(
                        details = a["details"].as_str().unwrap_or(""),
                        state = a["state"].as_str().unwrap_or(""),
                        image = a["assets"]["large_image"].as_str().unwrap_or(""),
                        "presence updated"
                    ),
                    None => tracing::info!("presence cleared"),
                }
                self.last_sent = Some(desired);
            }
            Err(DiscordError::Rejected(m)) => {
                tracing::error!(error = m, activity = %desired.as_ref().map(|v| v.to_string()).unwrap_or_default(), "Discord rejected the activity");
                self.last_sent = Some(desired); // don't hammer Discord with the same bad payload
            }
            Err(e) => {
                tracing::warn!(error = %e, "presence update failed; reconnecting");
                self.last_sent = None;
                self.alive_check_at = None;
                self.discord_backoff = DISCORD_RETRY_MIN;
                self.discord_retry_at = Some(Instant::now() + Duration::from_secs(1));
            }
        }
    }

    fn timers(&mut self) {
        let now = Instant::now();

        if self.discord_retry_at.is_some_and(|t| t <= now) {
            self.discord_retry_at = None;
            match self.discord.connect() {
                Ok(()) => {
                    self.discord_error = None;
                    self.discord_app_name = self.discord.app_name.clone();
                    self.discord_backoff = DISCORD_RETRY_MIN;
                    self.alive_check_at = Some(now + DISCORD_ALIVE_CHECK);
                    self.last_sent = None;
                    self.last_send_at = None;
                    self.schedule_flush();
                }
                Err(DiscordError::NoClientId) => {
                    if self.discord_error.is_none() {
                        tracing::warn!("discord_client_id is not set in the config; Discord presence disabled");
                    }
                    self.discord_error = Some("set discord_client_id in config".into());
                    // Re-checked on "Reload Config".
                }
                Err(e) => {
                    let msg = match &e {
                        DiscordError::Closed { code: 4000..=4999, message, .. } => {
                            format!("rejected client id ({message})")
                        }
                        DiscordError::NotRunning => "not running".into(),
                        // e.g. 1000 "User logged out" while Discord restarts its RPC server
                        _ => "reconnecting…".into(),
                    };
                    if self.discord_error.as_deref() != Some(msg.as_str()) {
                        tracing::info!(error = %e, retry_in_secs = self.discord_backoff.as_secs(), "Discord unavailable");
                    }
                    self.discord_error = Some(msg);
                    self.discord_retry_at = Some(now + self.discord_backoff);
                    self.discord_backoff = (self.discord_backoff * 2).min(DISCORD_RETRY_MAX);
                }
            }
        }

        if self.alive_check_at.is_some_and(|t| t <= now) {
            if self.discord.check_alive() {
                self.alive_check_at = Some(now + DISCORD_ALIVE_CHECK);
            } else {
                self.alive_check_at = None;
                self.last_sent = None;
                self.discord_error = Some("not running".into());
                self.discord_backoff = DISCORD_RETRY_MIN;
                self.discord_retry_at = Some(now + DISCORD_RETRY_MIN);
            }
        }

        if self.resolve_retry_at.is_some_and(|t| t <= now) {
            self.resolve_retry_at = None;
            if let Some(path) = self.snap.path.clone() {
                tracing::info!(path, "retrying metadata lookup");
                let _ = self.resolver.send(Job::Resolve(path));
            }
        }

        if self.flush_at.is_some_and(|t| t <= now) {
            self.flush();
        }
    }

    fn push_ui(&mut self) {
        let now = if !self.enabled {
            "Presence hidden".to_string()
        } else if self.player_connected && self.snap.is_playing() {
            match self.snap.path.as_deref().map(parse::file_kind) {
                Some(FileKind::Audio | FileKind::Image) => "Not a video (hidden)".to_string(),
                _ => presence::summary(self.resolution.as_ref()),
            }
        } else {
            "Nothing playing".to_string()
        };
        let discord = if self.config_error.is_some() {
            "Discord: config file has an error".to_string()
        } else if self.discord.is_connected() {
            match &self.discord.user {
                Some(u) => format!("Discord: connected ({u})"),
                None => "Discord: connected".into(),
            }
        } else {
            format!("Discord: {}", self.discord_error.as_deref().unwrap_or("connecting…"))
        };
        let needs_setup = !self.setup_needed.is_empty();
        let mut idle: Vec<String> = self
            .sources
            .iter()
            .filter(|(_, st)| st.connected)
            .map(|(id, st)| if st.snap.player.is_empty() { id.to_string() } else { st.snap.player.clone() })
            .collect();
        idle.sort();
        idle.dedup();
        let player = if self.active.is_some() {
            format!("{}: {}", self.snap.player, if self.snap.paused { "paused" } else { "playing" })
        } else if self.setup_just_done {
            "Players set up — restart them once".to_string()
        } else if !idle.is_empty() {
            format!("{}: idle", idle.join(", "))
        } else if needs_setup {
            format!("Needs setup: {}", self.setup_needed.join(", "))
        } else {
            "Waiting for a player…".to_string()
        };
        let can_pick_artwork = self.current_media().is_some();
        let now_state = if !self.enabled {
            NowState::Hidden
        } else if self.player_connected && self.snap.is_playing() {
            if self.snap.paused { NowState::Paused } else { NowState::Playing }
        } else {
            NowState::Idle
        };
        let discord_health = if self.config_error.is_some() {
            Health::Problem
        } else if self.discord.is_connected() {
            Health::Good
        } else if self.discord_error.as_deref().is_some_and(|e| e.contains("client id") || e.contains("config")) {
            Health::Problem
        } else {
            Health::Waiting
        };
        let player_health = if self.player_connected || !idle.is_empty() {
            Health::Good
        } else if needs_setup && !self.setup_just_done {
            Health::Problem
        } else {
            Health::Waiting
        };
        let status = UiStatus {
            now_state,
            discord_health,
            player_health,
            now,
            discord,
            player,
            enabled: self.enabled,
            needs_setup,
            can_pick_artwork,
        };
        if self.last_ui.as_ref() != Some(&status) {
            tracing::debug!(?status, "status");
            self.last_ui = Some(status.clone());
            (self.ui)(UiEvent::Status(status));
        }
    }
}
