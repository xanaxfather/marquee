//! Menu bar (macOS) / system tray (Windows) icon and menu, and the app's
//! windows. Runs on the main thread inside the tao event loop, which blocks
//! in the OS run loop when nothing happens (zero CPU while idle).

use crate::core::{Command, Health, MatchOpen, Msg, NowState, RecentMenu, SettingsOpen, UiEvent, UiStatus};
use crate::matches::{MatchOverride, MatchScope};
use crate::metadata::{Media, MediaKind, Resolution};
use crate::picker::{Picker, PickerMsg};
use crate::webwin::{Kind, Spec, WebWindow};
use crate::{config, icons, paths, platform, style};
use serde_json::{Map, Value, json};
use std::sync::mpsc::Sender;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tray_icon::menu::{CheckMenuItem, IconMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub enum UserEvent {
    Ui(UiEvent),
    Menu(MenuEvent),
    /// A message posted by one of the windows.
    Web(Kind, Value),
}

type MenuIcon = tray_icon::menu::Icon;

struct Items {
    now: IconMenuItem,
    discord: IconMenuItem,
    player: IconMenuItem,
    enabled: CheckMenuItem,
    hide: IconMenuItem,
    artwork: IconMenuItem,
    fix: IconMenuItem,
    recent: Submenu,
    setup: IconMenuItem,
    reconnect: IconMenuItem,
    settings: IconMenuItem,
    update: IconMenuItem,
    quit: IconMenuItem,
}

/// Menu glyphs. macOS: SF Symbols (template images). Windows menus
/// conventionally have no glyphs, so only the coloured status dots are used.
#[cfg(target_os = "macos")]
fn glyph(name: &str) -> Option<MenuIcon> {
    crate::symbols::symbol(name)
}
#[cfg(not(target_os = "macos"))]
fn glyph(_: &str) -> Option<MenuIcon> {
    None
}

fn to_menu_icon((rgba, size): (Vec<u8>, u32)) -> MenuIcon {
    MenuIcon::from_rgba(rgba, size, size).expect("valid menu icon")
}

/// Rendered once on the main thread after the event loop starts.
struct Icons {
    playing: Option<MenuIcon>,
    paused: Option<MenuIcon>,
    idle: Option<MenuIcon>,
    hidden: Option<MenuIcon>,
    setup: Option<MenuIcon>,
    warning: Option<MenuIcon>,
    hide: Option<MenuIcon>,
    unhide: Option<MenuIcon>,
    update: Option<MenuIcon>,
    update_ready: Option<MenuIcon>,
    good: MenuIcon,
    waiting: MenuIcon,
    problem: MenuIcon,
}

impl Icons {
    fn load() -> Self {
        Self {
            playing: glyph("play.circle"),
            paused: glyph("pause.circle"),
            idle: glyph("stop.circle"),
            hidden: glyph("eye.slash"),
            setup: glyph("wrench.and.screwdriver"),
            warning: glyph("exclamationmark.triangle"),
            hide: glyph("eye.slash"),
            unhide: glyph("eye"),
            update: glyph("arrow.down.circle"),
            update_ready: glyph("arrow.down.circle.fill"),
            good: to_menu_icon(icons::dot((35, 165, 90))),
            waiting: to_menu_icon(icons::dot((128, 132, 142))),
            problem: to_menu_icon(icons::dot((240, 178, 50))),
        }
    }

    fn health(&self, h: Health) -> MenuIcon {
        match h {
            Health::Good => self.good.clone(),
            Health::Waiting => self.waiting.clone(),
            Health::Problem => self.problem.clone(),
        }
    }
}

fn icon_item(text: &str, enabled: bool) -> IconMenuItem {
    IconMenuItem::new(text, enabled, None, None)
}

/// Template icons only exist on macOS; elsewhere set them as normal icons.
fn set_glyph(item: &IconMenuItem, icon: Option<MenuIcon>) {
    #[cfg(target_os = "macos")]
    item.set_icon_templated(icon);
    #[cfg(not(target_os = "macos"))]
    item.set_icon(icon);
}

fn tray_icon() -> Icon {
    #[cfg(target_os = "macos")]
    let (rgba, size) = icons::tray_template();
    #[cfg(not(target_os = "macos"))]
    let (rgba, size) = icons::tray_color();
    Icon::from_rgba(rgba, size, size).expect("valid tray icon")
}

/// macOS: an invisible app menu so windows get ⌘C/⌘V/⌘A/⌘Z and ⌘W.
#[cfg(target_os = "macos")]
fn install_app_menu() {
    let app_menu = Menu::new();
    let edit = Submenu::new("Edit", true);
    let _ = edit.append_items(&[
        &PredefinedMenuItem::undo(None),
        &PredefinedMenuItem::redo(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::cut(None),
        &PredefinedMenuItem::copy(None),
        &PredefinedMenuItem::paste(None),
        &PredefinedMenuItem::select_all(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::close_window(None),
    ]);
    let app = Submenu::new(paths::APP_NAME, true);
    let _ = app.append(&PredefinedMenuItem::quit(None));
    let _ = app_menu.append_items(&[&app, &edit]);
    app_menu.init_for_nsapp();
    // NSApp keeps using it for the life of the process.
    std::mem::forget(app_menu);
}
#[cfg(not(target_os = "macos"))]
fn install_app_menu() {}

/// Fixed menu labels in the configured case (re-applied so the lowercase
/// look switches without a restart).
fn relabel(items: &Items) {
    let t = style::text;
    items.enabled.set_text(t("Show on Discord"));
    items.artwork.set_text(t("Choose Artwork…"));
    items.fix.set_text(t("Fix Match…"));
    items.recent.set_text(t("Recently Watched"));
    items.reconnect.set_text(t("Reconnect to Discord"));
    items.settings.set_text(t("Settings…"));
    items.quit.set_text(t(&format!("Quit {}", paths::APP_NAME)));
}

fn apply(items: &Items, icons: Option<&Icons>, tray: Option<&TrayIcon>, s: &UiStatus) {
    relabel(items);
    if let Some(ic) = icons {
        let now = match s.now_state {
            NowState::Playing => &ic.playing,
            NowState::Paused => &ic.paused,
            NowState::Idle => &ic.idle,
            NowState::Hidden => &ic.hidden,
        };
        set_glyph(&items.now, now.clone());
        items.discord.set_icon(Some(ic.health(s.discord_health)));
        items.player.set_icon(Some(ic.health(s.player_health)));
        set_glyph(&items.setup, if s.needs_setup { ic.warning.clone() } else { ic.setup.clone() });
        set_glyph(&items.hide, if s.title_hidden { ic.unhide.clone() } else { ic.hide.clone() });
        set_glyph(&items.update, if s.update.is_some() { ic.update_ready.clone() } else { ic.update.clone() });
    }
    items.now.set_text(style::text(&s.now));
    items.discord.set_text(style::text(&s.discord));
    items.player.set_text(style::text(&s.player));
    items.enabled.set_checked(s.enabled);
    items.artwork.set_enabled(s.can_pick_artwork);
    items.fix.set_enabled(s.can_fix_match);
    items.hide.set_enabled(s.can_hide);
    items.hide.set_text(style::text(if s.title_hidden { "Show This Title on Discord" } else { "Hide This Title" }));
    items.setup.set_text(style::text(if s.needs_setup { "Set Up Players…  (needed)" } else { "Set Up Players…" }));
    items.update.set_text(style::text(&match &s.update {
        Some(v) => format!("Install Update {v}…"),
        None => "Check for Updates…".into(),
    }));
    if let Some(t) = tray {
        let _ = t.set_tooltip(Some(style::text(&format!("{}\n{}", paths::APP_NAME, s.now))));
    }
}

/// Recently Watched submenu: what each entry opens.
#[derive(Default)]
struct Recent {
    targets: Vec<(MenuId, String)>,
    all: Option<MenuId>,
    clear: Option<MenuId>,
}

fn fill_recent(sub: &Submenu, recent: &mut Recent, menu: &RecentMenu) {
    while sub.remove_at(0).is_some() {}
    recent.targets.clear();
    let t = style::text;
    if menu.items.is_empty() {
        let _ = sub.append(&MenuItem::new(t("Nothing watched yet"), false, None));
    } else {
        let _ = sub.append(&MenuItem::new(t(&format!("This week: {}", menu.week)), false, None));
        let _ = sub.append(&PredefinedMenuItem::separator());
        for (label, target) in &menu.items {
            let item = MenuItem::new(t(label), !target.is_empty(), None);
            recent.targets.push((item.id().clone(), target.clone()));
            let _ = sub.append(&item);
        }
    }
    let _ = sub.append(&PredefinedMenuItem::separator());
    let all = MenuItem::new(t("Show All History…"), true, None);
    let clear = MenuItem::new(t("Clear History"), !menu.items.is_empty(), None);
    recent.all = Some(all.id().clone());
    recent.clear = Some(clear.id().clone());
    let _ = sub.append_items(&[&all, &clear]);
}

// ------------------------------------------------------------------ windows

struct MatchWin {
    win: WebWindow,
    path: String,
    parsed_title: String,
    override_scope: Option<MatchScope>,
}

struct SettingsWin {
    win: WebWindow,
    cfg: config::Config,
    episode: Media,
    movie: Media,
    player: String,
    /// A checked import file waiting for the user to confirm it.
    import: Option<crate::backup::Bundle>,
    /// The last export, so "Show" can only reveal that file.
    exported: Option<std::path::PathBuf>,
}

#[derive(Default)]
struct Windows {
    picker: Option<Picker>,
    matcher: Option<MatchWin>,
    settings: Option<SettingsWin>,
}

impl Windows {
    fn get(&mut self, kind: Kind) -> Option<&mut WebWindow> {
        match kind {
            Kind::Artwork => self.picker.as_mut().map(|p| &mut p.win),
            Kind::Match => self.matcher.as_mut().map(|m| &mut m.win),
            Kind::Settings => self.settings.as_mut().map(|s| &mut s.win),
        }
    }

    fn close(&mut self, kind: Kind) {
        match kind {
            Kind::Artwork => self.picker = None,
            Kind::Match => self.matcher = None,
            Kind::Settings => self.settings = None,
        }
        tracing::info!(?kind, "window closed");
    }

    fn close_by_id(&mut self, id: tao::window::WindowId) {
        for kind in [Kind::Artwork, Kind::Match, Kind::Settings] {
            if self.get(kind).is_some_and(|w| w.id() == id) {
                self.close(kind);
            }
        }
    }

    fn close_all(&mut self) {
        self.picker = None;
        self.matcher = None;
        self.settings = None;
    }
}

fn handle_picker_msg(
    picker: &mut Option<Picker>,
    core_tx: Option<&Sender<Msg>>,
    store: Option<&mut crate::artwork::Store>,
    msg: PickerMsg,
) -> bool {
    let Some(p) = picker.as_mut() else { return true };
    let apply = |key: String, url: Option<String>, store: Option<&mut crate::artwork::Store>| match (core_tx, store) {
        (Some(tx), _) => {
            let _ = tx.send(Msg::Command(Command::SetArtwork { key, url }));
        }
        (None, Some(store)) => store.set(&key, url),
        _ => {}
    };
    match msg {
        PickerMsg::Ready => p.win.on_ready(),
        PickerMsg::Apply { scope, url } => {
            if let Some(key) = p.target.key(scope) {
                tracing::info!(key, url, "artwork chosen");
                apply(key, Some(url), store);
            }
        }
        PickerMsg::Reset { scope } => {
            if let Some(key) = p.target.key(scope) {
                apply(key, None, store);
            }
        }
        PickerMsg::Open { url } => {
            if url.starts_with("https://www.themoviedb.org/") {
                platform::open_url(&url);
            }
        }
        PickerMsg::Close => {
            *picker = None;
            return false;
        }
    }
    true
}

fn handle_match_msg(windows: &mut Windows, core_tx: &Sender<Msg>, msg: &Value) {
    let Some(m) = windows.matcher.as_mut() else { return };
    let send = |c: Command| {
        let _ = core_tx.send(Msg::Command(c));
    };
    match msg["type"].as_str().unwrap_or("") {
        "ready" => m.win.on_ready(),
        "search" => send(Command::Search {
            query: msg["query"].as_str().unwrap_or("").to_string(),
            kind: msg["kind"].as_str().unwrap_or("multi").to_string(),
        }),
        "seasons" => {
            if let Some(id) = msg["id"].as_u64() {
                send(Command::Seasons(id));
            }
        }
        "apply" => {
            let scope: MatchScope = serde_json::from_value(msg["scope"].clone()).unwrap_or(MatchScope::File);
            let kind: MediaKind = serde_json::from_value(msg["kind"].clone()).unwrap_or(MediaKind::Movie);
            let (Some(id), Some(title)) = (msg["id"].as_u64(), msg["title"].as_str()) else { return };
            let exact = scope == MatchScope::File && kind == MediaKind::Episode;
            let value = MatchOverride {
                kind,
                tmdb_id: id,
                title: title.to_string(),
                year: msg["year"].as_u64().map(|y| y as u16),
                season: msg["season"].as_u64().filter(|_| exact).map(|n| n as u32),
                episode: msg["episode"].as_u64().filter(|_| exact).map(|n| n as u32),
            };
            // A narrower override would keep winning over the new one.
            for narrower in [MatchScope::File, MatchScope::Folder] {
                if narrower == scope {
                    break;
                }
                let key = crate::matches::key(narrower, &m.path, &m.parsed_title);
                send(Command::SetMatch { key, value: None });
            }
            let key = crate::matches::key(scope, &m.path, &m.parsed_title);
            tracing::info!(key, id, title, "match chosen");
            m.override_scope = Some(scope);
            send(Command::SetMatch { key, value: Some(value) });
        }
        "reset" => {
            for scope in [MatchScope::File, MatchScope::Folder, MatchScope::Name] {
                let key = crate::matches::key(scope, &m.path, &m.parsed_title);
                send(Command::SetMatch { key, value: None });
            }
            m.override_scope = None;
        }
        "open" => {
            if let Some(url) = msg["url"].as_str().filter(|u| u.starts_with("https://www.themoviedb.org/")) {
                platform::open_url(url);
            }
        }
        "close" => windows.close(Kind::Match),
        _ => {}
    }
}

/// Discord cards for the Settings preview.
fn previews(s: &SettingsWin, values: &Map<String, Value>) -> Value {
    let cfg = config::with_values(&s.cfg, values);
    let snap = crate::players::Snapshot {
        player: s.player.clone(),
        path: Some("/preview.mkv".into()),
        duration: Some(2820.0),
        position: Some(1234.0),
        paused: false,
        idle: false,
        ..Default::default()
    };
    let card = |m: &Media| {
        let r = Resolution { path: "/preview.mkv".into(), media: Some(m.clone()), ..Default::default() };
        let image = crate::artwork::default_for(cfg.artwork, m);
        let mut a = crate::presence::build(&cfg, &snap, Some(&r), image, std::time::Instant::now());
        a["hidden"] = json!(
            cfg.privacy_mode == config::PrivacyMode::Hide
                && crate::presence::is_private(&cfg, snap.path.as_deref(), Some(&r))
        );
        a
    };
    json!({ "episode": card(&s.episode), "movie": card(&s.movie) })
}

fn handle_settings_msg(windows: &mut Windows, core_tx: &Sender<Msg>, proxy: &EventLoopProxy<UserEvent>, msg: &Value) {
    let Some(s) = windows.settings.as_mut() else { return };
    let send = |c: Command| {
        let _ = core_tx.send(Msg::Command(c));
    };
    let empty = Map::new();
    let values = msg["values"].as_object().unwrap_or(&empty);
    match msg["type"].as_str().unwrap_or("") {
        "ready" => {
            s.win.on_ready();
            let p = previews(s, &empty);
            s.win.call("__preview", &p);
        }
        "preview" => {
            let p = previews(s, values);
            s.win.call("__preview", &p);
        }
        "save" => match config::persist_json(values) {
            Ok(()) => {
                s.cfg = config::with_values(&s.cfg, values);
                send(Command::ReloadConfig);
                s.win.call("__saved", &json!({ "ok": true }));
            }
            Err(e) => {
                tracing::error!(error = format!("{e:#}"), "could not save settings");
                s.win.call("__saved", &json!({ "ok": false, "error": format!("{e:#}") }));
            }
        },
        "login" => {
            let on = msg["on"].as_bool().unwrap_or(false);
            if let Err(e) = platform::set_launch_at_login(on) {
                tracing::error!(error = %e, "could not change launch at login");
            }
            s.win.call("__login", &json!(platform::launch_at_login_enabled()));
        }
        "setup_players" => send(Command::SetupPlayers),
        "test_tmdb" => {
            let key = msg["key"].as_str().unwrap_or("").to_string();
            let proxy = proxy.clone();
            std::thread::spawn(move || {
                let t = crate::tmdb::Tmdb::new(&key, "en-US");
                let arg = match t.get("/configuration", &[]) {
                    Ok(_) => json!({ "ok": true }),
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                };
                let _ =
                    proxy.send_event(UserEvent::Ui(UiEvent::Window { kind: Kind::Settings, func: "__tmdbTest", arg }));
            });
        }
        "check_update" => send(Command::CheckUpdate { manual: true }),
        "install_update" => send(Command::InstallUpdate),
        "clear_history" => send(Command::ClearHistory),
        "remove_history" => {
            if let Some(k) = msg["key"].as_str() {
                send(Command::RemoveHistory(k.to_string()));
            }
        }
        "clear_cache" => send(Command::ClearCache),
        "reconnect" => send(Command::Reconnect),
        "reload" => send(Command::ReloadConfig),
        "open" => {
            if let Some(url) = msg["url"].as_str() {
                platform::open_url(url);
            }
        }
        "open_file" => {
            if let Some(p) = msg["path"].as_str().map(std::path::Path::new).filter(|p| p.is_file()) {
                platform::open_path(p, false);
            }
        }
        "export" => {
            let include_key = msg["include_key"].as_bool().unwrap_or(false);
            let reply = match crate::backup::export(include_key, msg["date"].as_str().unwrap_or("")) {
                Ok(path) => {
                    platform::reveal_path(&path);
                    let shown = path.display().to_string();
                    s.exported = Some(path);
                    json!({ "ok": true, "path": shown })
                }
                Err(e) => {
                    tracing::error!(error = format!("{e:#}"), "could not export settings");
                    json!({ "ok": false, "error": format!("{e:#}") })
                }
            };
            s.win.call("__exported", &reply);
        }
        "reveal_export" => {
            if let Some(p) = s.exported.as_deref().filter(|p| p.is_file()) {
                platform::reveal_path(p);
            }
        }
        "import_check" => {
            s.import = None;
            let text = msg["text"].as_str().unwrap_or("");
            let reply = match crate::backup::parse(text).and_then(|b| crate::backup::preview(&b).map(|p| (b, p))) {
                Ok((bundle, preview)) => {
                    s.import = Some(bundle);
                    json!({ "ok": true, "plan": preview })
                }
                Err(e) => {
                    tracing::warn!(error = format!("{e:#}"), "settings file refused");
                    json!({ "ok": false, "error": format!("{e:#}") })
                }
            };
            s.win.call("__importPlan", &reply);
        }
        "import_cancel" => s.import = None,
        "import_apply" => {
            let mode = serde_json::from_value::<crate::backup::Mode>(msg["mode"].clone());
            let reply = match (s.import.take(), mode) {
                (Some(bundle), Ok(mode)) => match crate::backup::apply(&bundle, mode) {
                    Ok(cfg) => {
                        s.cfg = cfg.clone();
                        send(Command::Imported);
                        json!({ "ok": true, "config": cfg })
                    }
                    Err(e) => {
                        tracing::error!(error = format!("{e:#}"), "could not import settings");
                        json!({ "ok": false, "error": format!("Nothing was changed: {e:#}") })
                    }
                },
                _ => json!({ "ok": false, "error": "Choose the file again." }),
            };
            s.win.call("__imported", &reply);
        }
        "open_config" => platform::open_path(&paths::config_file(), true),
        "open_logs" => platform::open_path(&paths::log_dir(), false),
        "close" => windows.close(Kind::Settings),
        _ => {}
    }
}

fn open_settings(
    el: &EventLoopWindowTarget<UserEvent>,
    proxy: &EventLoopProxy<UserEvent>,
    w: &mut Windows,
    o: SettingsOpen,
) {
    if let Some(s) = w.settings.as_mut() {
        // Already open: just jump to the section.
        if let Some(section) = o.ctx["section"].as_str() {
            s.win.call("__section", &json!(section));
        }
        s.win.focus();
        return;
    }
    let welcome = o.ctx["section"].as_str() == Some("welcome");
    let spec = Spec {
        kind: Kind::Settings,
        title: if welcome { "Welcome to Marquee" } else { "Marquee Settings" },
        size: (980.0, 680.0),
        min_size: (820.0, 560.0),
        html: include_str!("settings.html"),
        ctx: o.ctx,
    };
    match WebWindow::open(el, proxy.clone(), spec) {
        Ok(win) => {
            w.settings = Some(SettingsWin {
                win,
                cfg: o.cfg,
                episode: o.episode,
                movie: o.movie,
                player: o.player,
                import: None,
                exported: None,
            })
        }
        Err(e) => tracing::error!(error = format!("{e:#}"), "could not open settings"),
    }
}

fn open_match(el: &EventLoopWindowTarget<UserEvent>, proxy: &EventLoopProxy<UserEvent>, w: &mut Windows, o: MatchOpen) {
    w.matcher = None;
    let override_scope = serde_json::from_value(o.ctx["override_scope"].clone()).ok();
    let spec = Spec {
        kind: Kind::Match,
        title: "Fix Match — Marquee",
        size: (980.0, 660.0),
        min_size: (780.0, 520.0),
        html: include_str!("match.html"),
        ctx: o.ctx,
    };
    match WebWindow::open(el, proxy.clone(), spec) {
        Ok(win) => w.matcher = Some(MatchWin { win, path: o.path, parsed_title: o.parsed_title, override_scope }),
        Err(e) => tracing::error!(error = format!("{e:#}"), "could not open Fix Match"),
    }
}

/// `--artwork-picker <file>`: just the picker window, for a file that isn't playing.
/// Choices are saved to the same overrides file the tray app uses.
pub fn run_picker(target: crate::artwork::Target, tmdb_key: String, language: String) -> ! {
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Regular);
    }
    let proxy = event_loop.create_proxy();
    {
        let (t, p) = (target.clone(), proxy.clone());
        std::thread::spawn(move || {
            let r = crate::metadata::Resolver::new(&tmdb_key, &language);
            let result = r.artwork_options(&t).map_err(|e| e.to_string());
            let _ = p.send_event(UserEvent::Ui(UiEvent::ArtworkOptions { tmdb_id: t.tmdb_id, result }));
        });
    }
    let mut store = crate::artwork::Store::open();
    let mut picker: Option<Picker> = None;
    event_loop.run(move |event, el, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                install_app_menu();
                match Picker::open(el, proxy.clone(), target.clone()) {
                    Ok(p) => picker = Some(p),
                    Err(e) => {
                        eprintln!("could not open the picker: {e:#}");
                        *control_flow = ControlFlow::Exit;
                    }
                }
            }
            Event::UserEvent(UserEvent::Ui(UiEvent::ArtworkOptions { result, .. })) => {
                if let Some(p) = picker.as_mut() {
                    p.deliver_options(result);
                }
            }
            Event::UserEvent(UserEvent::Web(Kind::Artwork, v)) => match serde_json::from_value::<PickerMsg>(v) {
                Ok(msg) => {
                    if !handle_picker_msg(&mut picker, None, Some(&mut store), msg) {
                        *control_flow = ControlFlow::Exit;
                    }
                }
                Err(e) => tracing::warn!(error = %e, "bad message from artwork picker"),
            },
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    })
}

/// Run the tray app. Never returns.
pub fn run(core_tx: Sender<Msg>, start_core: impl FnOnce(Box<dyn Fn(UiEvent) + Send>) + 'static) -> ! {
    #[allow(unused_mut)]
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        event_loop.set_activation_policy(ActivationPolicy::Accessory);
    }

    let menu_proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = menu_proxy.send_event(UserEvent::Menu(e));
    }));
    let ui_proxy = event_loop.create_proxy();
    let web_proxy = event_loop.create_proxy();
    let mut start_core = Some(start_core);

    let t = style::text;
    let items = Items {
        now: icon_item(&t("Nothing playing"), false),
        discord: icon_item(&t("Discord: connecting…"), false),
        player: icon_item(&t("Waiting for a player…"), false),
        enabled: CheckMenuItem::new(t("Show on Discord"), true, true, None),
        hide: icon_item(&t("Hide This Title"), false),
        artwork: icon_item(&t("Choose Artwork…"), false),
        fix: icon_item(&t("Fix Match…"), false),
        recent: Submenu::new(t("Recently Watched"), true),
        setup: icon_item(&t("Set Up Players…"), true),
        reconnect: icon_item(&t("Reconnect to Discord"), true),
        settings: icon_item(&t("Settings…"), true),
        update: icon_item(&t("Check for Updates…"), true),
        quit: icon_item(&t(&format!("Quit {}", paths::APP_NAME)), true),
    };
    let mut recent = Recent::default();
    fill_recent(&items.recent, &mut recent, &RecentMenu::default());
    let mut update_available = false;
    let mut icons: Option<Icons> = None;
    let mut tray: Option<TrayIcon> = None;
    let mut windows = Windows::default();

    event_loop.run(move |event, window_target, control_flow| {
        *control_flow = ControlFlow::Wait;
        let send = |c: Command| {
            let _ = core_tx.send(Msg::Command(c));
        };
        match event {
            Event::NewEvents(StartCause::Init) => {
                install_app_menu();

                let ic = Icons::load();
                for (item, name) in [
                    (&items.artwork, "photo.on.rectangle.angled"),
                    (&items.fix, "wand.and.stars"),
                    (&items.reconnect, "arrow.triangle.2.circlepath"),
                    (&items.settings, "gearshape"),
                    (&items.quit, "power"),
                ] {
                    set_glyph(item, glyph(name));
                }
                #[cfg(target_os = "macos")]
                items.recent.set_icon_templated(glyph("clock.arrow.circlepath"));
                set_glyph(&items.now, ic.idle.clone());
                set_glyph(&items.setup, ic.setup.clone());
                set_glyph(&items.hide, ic.hide.clone());
                set_glyph(&items.update, ic.update.clone());
                items.discord.set_icon(Some(ic.waiting.clone()));
                items.player.set_icon(Some(ic.waiting.clone()));
                icons = Some(ic);

                let menu = Menu::new();
                let sep = PredefinedMenuItem::separator;
                let result = menu.append_items(&[
                    &items.now,
                    &sep(),
                    &items.discord,
                    &items.player,
                    &sep(),
                    &items.enabled,
                    &items.hide,
                    &items.artwork,
                    &items.fix,
                    &items.recent,
                    &sep(),
                    &items.setup,
                    &items.reconnect,
                    &items.settings,
                    &items.update,
                    &sep(),
                    &items.quit,
                ]);
                if let Err(e) = result {
                    tracing::error!(error = %e, "could not build menu");
                }
                let builder = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip(style::app_name());
                #[cfg(target_os = "macos")]
                let builder = builder.with_icon_templated(tray_icon());
                #[cfg(not(target_os = "macos"))]
                let builder = builder.with_icon(tray_icon());
                match builder.build() {
                    Ok(t) => tray = Some(t),
                    Err(e) => tracing::error!(error = %e, "could not create tray icon"),
                }
                if let Some(start) = start_core.take() {
                    let proxy = ui_proxy.clone();
                    start(Box::new(move |ev| {
                        let _ = proxy.send_event(UserEvent::Ui(ev));
                    }));
                }
            }
            Event::UserEvent(UserEvent::Ui(ev)) => match ev {
                UiEvent::Status(s) => {
                    update_available = s.update.is_some();
                    apply(&items, icons.as_ref(), tray.as_ref(), &s);
                }
                UiEvent::Recent(menu) => fill_recent(&items.recent, &mut recent, &menu),
                UiEvent::OpenArtwork(target) => {
                    windows.picker = None; // replace any open picker
                    match Picker::open(window_target, web_proxy.clone(), *target) {
                        Ok(p) => windows.picker = Some(p),
                        Err(e) => tracing::error!(error = format!("{e:#}"), "could not open artwork picker"),
                    }
                }
                UiEvent::ArtworkOptions { tmdb_id, result } => {
                    if let Some(p) = windows.picker.as_mut().filter(|p| p.target.tmdb_id == tmdb_id) {
                        p.deliver_options(result);
                    }
                }
                UiEvent::OpenSettings(o) => open_settings(window_target, &web_proxy, &mut windows, *o),
                UiEvent::OpenMatch(o) => open_match(window_target, &web_proxy, &mut windows, *o),
                UiEvent::Window { kind, func, arg } => {
                    if let Some(w) = windows.get(kind) {
                        w.call(func, &arg);
                    }
                }
                UiEvent::Exit => {
                    windows.close_all();
                    tray.take();
                    *control_flow = ControlFlow::Exit;
                }
            },
            Event::UserEvent(UserEvent::Web(kind, msg)) => match kind {
                Kind::Artwork => match serde_json::from_value::<PickerMsg>(msg) {
                    Ok(m) => {
                        handle_picker_msg(&mut windows.picker, Some(&core_tx), None, m);
                    }
                    Err(e) => tracing::warn!(error = %e, "bad message from artwork picker"),
                },
                Kind::Match => handle_match_msg(&mut windows, &core_tx, &msg),
                Kind::Settings => handle_settings_msg(&mut windows, &core_tx, &web_proxy, &msg),
            },
            Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. } => {
                windows.close_by_id(window_id);
            }
            Event::UserEvent(UserEvent::Menu(e)) => {
                let id = e.id();
                if id == items.quit.id() {
                    send(Command::Quit);
                    // Safety net in case the core thread is stuck on a network call.
                    let proxy = ui_proxy.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        let _ = proxy.send_event(UserEvent::Ui(UiEvent::Exit));
                    });
                } else if id == items.enabled.id() {
                    send(Command::SetEnabled(items.enabled.is_checked()));
                } else if id == items.hide.id() {
                    send(Command::ToggleHideTitle);
                } else if id == items.artwork.id() {
                    match windows.picker.as_ref() {
                        Some(p) => p.win.focus(),
                        None => send(Command::OpenArtwork),
                    }
                } else if id == items.fix.id() {
                    send(Command::OpenMatch);
                } else if id == items.setup.id() {
                    send(Command::SetupPlayers);
                } else if id == items.reconnect.id() {
                    send(Command::Reconnect);
                } else if id == items.settings.id() {
                    send(Command::OpenSettings { section: None });
                } else if id == items.update.id() {
                    if update_available {
                        send(Command::OpenSettings { section: Some("about".into()) });
                    } else {
                        send(Command::CheckUpdate { manual: true });
                    }
                } else if Some(id) == recent.all.as_ref() {
                    send(Command::OpenSettings { section: Some("history".into()) });
                } else if Some(id) == recent.clear.as_ref() {
                    send(Command::ClearHistory);
                } else if let Some((_, target)) = recent.targets.iter().find(|(i, _)| i == id) {
                    if target.starts_with("https://") {
                        platform::open_url(target);
                    } else {
                        platform::open_path(std::path::Path::new(target), false);
                    }
                }
            }
            _ => {}
        }
    })
}
