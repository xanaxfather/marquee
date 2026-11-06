//! Marquee — Discord Rich Presence for the movies and TV shows you watch,
//! identified automatically from the file name. macOS and Windows.

// No console window on Windows; `platform::attach_console` re-attaches for CLI use.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod artwork;
mod cache;
mod config;
mod core;
mod discord;
mod icons;
mod ipc;
mod logging;
mod matching;
mod metadata;
mod parse;
mod paths;
mod picker;
mod platform;
mod players;
mod presence;
mod setup;
#[cfg(target_os = "macos")]
mod symbols;
mod tmdb;
mod tray;

use config::Config;

const HELP: &str = "\
Marquee — Discord Rich Presence for what you're watching

USAGE:
    marquee                          Run in the menu bar / system tray (default)
    marquee --headless               Run without a tray icon (logs to the terminal)
    marquee --parse <path>           Show how a file name is interpreted
    marquee --lookup <path>          Identify a file on TMDB and print the activity
    marquee --artwork-picker <path>  Choose artwork for a file without playing it
    marquee --artwork <path>         List the artwork the picker would offer (JSON)
    marquee --setup-players          Configure every installed player
    marquee --diagnose               Check config, players, Discord and TMDB
    marquee --version

Set RUST_LOG=debug for verbose logs.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first().map(String::as_str);
    if first.is_some() {
        platform::attach_console();
    }

    if matches!(first, Some("-h" | "--help")) {
        println!("{HELP}\n\nConfig: {}\nLogs:   {}", paths::config_file().display(), paths::log_dir().display());
        return;
    }
    if matches!(first, Some("-V" | "--version")) {
        println!("marquee {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let migrated = platform::migrate_legacy();
    let (mut cfg, config_error) = Config::load_or_default();
    let _guard = logging::init(&cfg.log_level);
    for note in &migrated {
        tracing::info!(note, "migrated settings from IINA Presence");
    }
    if let Some(e) = &config_error {
        tracing::error!(error = %e, file = %paths::config_file().display(), "config file is invalid; using defaults until it is fixed");
        if first.is_none() {
            platform::notify(
                "Your config file has an error, so Discord and TMDB are off. Fix it, then choose Reload Config.",
            );
        }
    }

    match first {
        Some("--parse") => cli_parse(&args[1..]),
        Some("--lookup") => cli_lookup(&cfg, &args[1..]),
        Some("--diagnose") => diagnose(&cfg),
        Some("--artwork") => cli_artwork(&cfg, &args[1..]),
        Some("--artwork-picker") => {
            let (_, target) = artwork_target(&cfg, &path_arg(&args[1..]));
            tray::run_picker(target, cfg.tmdb_api_key.clone(), cfg.language.clone());
        }
        Some("--setup-players") => {
            let results = setup::configure_all(&mut cfg);
            if results.is_empty() {
                println!("No supported players found.");
            }
            for (name, r) in results {
                match r {
                    Ok(()) => println!("{name}: set up (restart it once)"),
                    Err(e) => println!("{name}: failed: {e:#}"),
                }
            }
        }
        Some("--headless") => run(cfg, config_error, false),
        None => run(cfg, config_error, true),
        Some(other) => {
            eprintln!("unknown argument: {other}\n\n{HELP}");
            std::process::exit(2);
        }
    }
}

fn run(cfg: Config, config_error: Option<String>, with_tray: bool) {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        pid = std::process::id(),
        config = cfg.summary(),
        config_file = %paths::config_file().display(),
        logs = %paths::log_dir().display(),
        "starting Marquee"
    );
    let (tx, rx) = std::sync::mpsc::channel::<core::Msg>();
    players::spawn_all(&cfg, tx.clone());

    if with_tray {
        let core_tx = tx.clone();
        tray::run(core_tx, move |ui| {
            std::thread::Builder::new()
                .name("core".into())
                .spawn(move || core::Core::new(cfg, tx, rx, ui).with_config_error(config_error).run())
                .expect("spawn core thread");
        });
    } else {
        let ui = |ev: core::UiEvent| {
            if let core::UiEvent::Status(s) = ev {
                tracing::info!(now = s.now, discord = s.discord, player = s.player, "status");
            }
        };
        core::Core::new(cfg, tx, rx, ui).with_config_error(config_error).run();
    }
}

fn path_arg(args: &[String]) -> String {
    match args.first() {
        Some(p) => p.clone(),
        None => {
            eprintln!("missing <path>");
            std::process::exit(2);
        }
    }
}

fn cli_parse(args: &[String]) {
    let path = path_arg(args);
    println!("kind: {:?}", parse::file_kind(&path));
    for (i, h) in parse::hypotheses(&path).iter().enumerate() {
        println!("hypothesis {}: {}", i + 1, serde_json::to_string(h).unwrap_or_default());
    }
}

fn cli_lookup(cfg: &Config, args: &[String]) {
    let path = path_arg(args);
    let mut resolver = metadata::Resolver::new(&cfg.tmdb_api_key, &cfg.language);
    let res = resolver.resolve(&path);
    match &res.media {
        Some(m) => println!("match: {}", serde_json::to_string_pretty(m).unwrap_or_default()),
        None => println!("no match (error: {})", res.error.as_deref().unwrap_or("none")),
    }
    let snap = players::Snapshot {
        path: Some(path.clone()),
        duration: Some(2700.0),
        position: Some(600.0),
        paused: false,
        idle: false,
        ..Default::default()
    };
    let image = res.media.as_ref().and_then(|m| artwork::effective(cfg, &artwork::Store::open(), m));
    let activity = presence::build(cfg, &snap, Some(&res), image, std::time::Instant::now());
    println!("activity (45 min file at 10:00): {}", serde_json::to_string_pretty(&activity).unwrap_or_default());
}

/// Identify a file and describe it for the artwork picker.
fn artwork_target(cfg: &Config, path: &str) -> (metadata::Resolver, artwork::Target) {
    let mut resolver = metadata::Resolver::new(&cfg.tmdb_api_key, &cfg.language);
    let res = resolver.resolve(path);
    let Some(m) = res.media else {
        eprintln!("not identified: {}", res.error.as_deref().unwrap_or("no confident match"));
        std::process::exit(1);
    };
    let store = artwork::Store::open();
    let snap = players::Snapshot { path: Some(path.to_string()), idle: false, ..Default::default() };
    let activity = presence::build(cfg, &snap, None, None, std::time::Instant::now());
    let state = match &m.episode {
        Some(e) => match &e.name {
            Some(n) => format!("{} · {n}", e.number.label()),
            None => e.number.label(),
        },
        None => m.year.map(|y| y.to_string()).unwrap_or_default(),
    };
    let target = artwork::Target {
        kind: m.kind,
        tmdb_id: m.tmdb_id,
        season: m.episode.as_ref().map(|e| e.number.season),
        episode: m.episode.as_ref().map(|e| e.number.episode),
        tmdb_numbers: m.episode.as_ref().and_then(|e| e.tmdb),
        language: cfg.language.clone(),
        app_name: "Marquee".into(),
        details: m.title.clone(),
        state: if state.is_empty() { activity["state"].as_str().unwrap_or("").to_string() } else { state },
        position: None,
        paused: false,
        current: artwork::effective(cfg, &store, &m),
        default: artwork::default_for(cfg.artwork, &m),
        overrides: store.overrides(&m),
        tmdb_url: m.tmdb_url(),
    };
    (resolver, target)
}

fn cli_artwork(cfg: &Config, args: &[String]) {
    let (resolver, target) = artwork_target(cfg, &path_arg(args));
    match resolver.artwork_options(&target) {
        Ok(v) => println!("{}", serde_json::json!({"target": target, "options": v})),
        Err(e) => {
            eprintln!("failed: {e}");
            std::process::exit(1);
        }
    }
}

fn diagnose(cfg: &Config) {
    let ok = |b: bool| if b { "ok  " } else { "FAIL" };
    println!("Marquee {} diagnostics\n", env!("CARGO_PKG_VERSION"));
    println!("config file:   {}", paths::config_file().display());
    println!("log folder:    {}", paths::log_dir().display());
    println!("cache file:    {}", paths::cache_file().display());
    println!("config:        {}\n", cfg.summary());

    for p in setup::status(cfg) {
        if p.installed {
            println!("[{}] {} set up{}", ok(p.ready), p.name, if p.ready { "" } else { " (run --setup-players)" });
        } else {
            println!("[ -- ] {} not installed", p.name);
        }
    }

    println!("[{}] discord_client_id set", ok(!cfg.discord_client_id.is_empty()));
    let sockets = discord::socket_candidates();
    println!("Discord endpoints: {:?}", sockets);
    if !cfg.discord_client_id.is_empty() {
        let mut d = discord::Discord::new(&cfg.discord_client_id);
        match d.connect() {
            Ok(()) => println!("[ok  ] Discord handshake as {}", d.user.as_deref().unwrap_or("?")),
            Err(e) => println!("[FAIL] Discord handshake: {e}"),
        }
    }

    let t = tmdb::Tmdb::new(&cfg.tmdb_api_key, &cfg.language);
    if !t.has_key() {
        println!("[FAIL] tmdb_api_key set");
    } else {
        match t.get("/configuration", &[]) {
            Ok(_) => println!("[ok  ] TMDB API key accepted"),
            Err(e) => println!("[FAIL] TMDB: {e}"),
        }
    }
}
