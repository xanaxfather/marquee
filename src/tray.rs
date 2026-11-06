//! Menu bar (macOS) / system tray (Windows) icon and menu. Runs on the main
//! thread inside the tao event loop, which blocks in the OS run loop when
//! nothing happens (zero CPU while idle).

use crate::core::{Command, Health, Msg, NowState, UiEvent, UiStatus};
use crate::picker::{Picker, PickerMsg};
use crate::{icons, paths, platform};
use std::sync::mpsc::Sender;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tray_icon::menu::{CheckMenuItem, IconMenuItem, Menu, MenuEvent, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub enum UserEvent {
    Ui(UiEvent),
    Menu(MenuEvent),
    Picker(PickerMsg),
}

type MenuIcon = tray_icon::menu::Icon;

struct Items {
    now: IconMenuItem,
    discord: IconMenuItem,
    player: IconMenuItem,
    enabled: CheckMenuItem,
    artwork: IconMenuItem,
    setup: IconMenuItem,
    reconnect: IconMenuItem,
    reload: IconMenuItem,
    open_config: IconMenuItem,
    open_logs: IconMenuItem,
    clear_cache: IconMenuItem,
    login: CheckMenuItem,
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
    use tray_icon::menu::Submenu;
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

fn apply(items: &Items, icons: Option<&Icons>, tray: Option<&TrayIcon>, s: &UiStatus) {
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
    }
    items.now.set_text(&s.now);
    items.discord.set_text(&s.discord);
    items.player.set_text(&s.player);
    items.enabled.set_checked(s.enabled);
    items.artwork.set_enabled(s.can_pick_artwork);
    items.setup.set_text(if s.needs_setup { "Set Up Players…  (needed)" } else { "Set Up Players…" });
    if let Some(t) = tray {
        let _ = t.set_tooltip(Some(format!("{}\n{}", paths::APP_NAME, s.now)));
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
        PickerMsg::Ready => p.on_ready(),
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
        PickerMsg::Drag => {}
    }
    true
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
            Event::UserEvent(UserEvent::Picker(msg)) => {
                if !handle_picker_msg(&mut picker, None, Some(&mut store), msg) {
                    *control_flow = ControlFlow::Exit;
                }
            }
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
    let picker_proxy = event_loop.create_proxy();
    let mut start_core = Some(start_core);

    let items = Items {
        now: icon_item("Nothing playing", false),
        discord: icon_item("Discord: connecting…", false),
        player: icon_item("Waiting for a player…", false),
        enabled: CheckMenuItem::new("Show on Discord", true, true, None),
        artwork: icon_item("Choose Artwork…", false),
        setup: icon_item("Set Up Players…", true),
        reconnect: icon_item("Reconnect to Discord", true),
        reload: icon_item("Reload Config", true),
        open_config: icon_item("Open Config File…", true),
        open_logs: icon_item("Open Logs Folder", true),
        clear_cache: icon_item("Clear Metadata Cache", true),
        login: CheckMenuItem::new(
            if cfg!(windows) { "Start with Windows" } else { "Launch at Login" },
            true,
            platform::launch_at_login_enabled(),
            None,
        ),
        quit: icon_item(&format!("Quit {}", paths::APP_NAME), true),
    };
    let mut icons: Option<Icons> = None;
    let mut tray: Option<TrayIcon> = None;
    let mut picker: Option<Picker> = None;

    event_loop.run(move |event, window_target, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                install_app_menu();

                let ic = Icons::load();
                for (item, name) in [
                    (&items.artwork, "photo.on.rectangle.angled"),
                    (&items.reconnect, "arrow.triangle.2.circlepath"),
                    (&items.reload, "arrow.clockwise"),
                    (&items.open_config, "slider.horizontal.3"),
                    (&items.open_logs, "doc.text.magnifyingglass"),
                    (&items.clear_cache, "trash"),
                    (&items.quit, "power"),
                ] {
                    set_glyph(item, glyph(name));
                }
                set_glyph(&items.now, ic.idle.clone());
                set_glyph(&items.setup, ic.setup.clone());
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
                    &items.artwork,
                    &items.setup,
                    &items.reconnect,
                    &sep(),
                    &items.reload,
                    &items.open_config,
                    &items.open_logs,
                    &items.clear_cache,
                    &items.login,
                    &sep(),
                    &items.quit,
                ]);
                if let Err(e) = result {
                    tracing::error!(error = %e, "could not build menu");
                }
                let builder = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip(paths::APP_NAME);
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
            Event::UserEvent(UserEvent::Ui(UiEvent::Status(s))) => apply(&items, icons.as_ref(), tray.as_ref(), &s),
            Event::UserEvent(UserEvent::Ui(UiEvent::OpenArtwork(target))) => {
                picker = None; // replace any open picker
                match Picker::open(window_target, picker_proxy.clone(), *target) {
                    Ok(p) => picker = Some(p),
                    Err(e) => tracing::error!(error = format!("{e:#}"), "could not open artwork picker"),
                }
            }
            Event::UserEvent(UserEvent::Ui(UiEvent::ArtworkOptions { tmdb_id, result })) => {
                if let Some(p) = picker.as_mut().filter(|p| p.target.tmdb_id == tmdb_id) {
                    p.deliver_options(result);
                }
            }
            Event::UserEvent(UserEvent::Picker(msg)) => {
                handle_picker_msg(&mut picker, Some(&core_tx), None, msg);
            }
            Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. } => {
                if picker.as_ref().is_some_and(|p| p.id() == window_id) {
                    picker = None;
                    tracing::info!("artwork picker closed");
                }
            }
            Event::UserEvent(UserEvent::Ui(UiEvent::Exit)) => {
                picker = None;
                tray.take();
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::Menu(e)) => {
                let id = e.id();
                let send = |c: Command| {
                    let _ = core_tx.send(Msg::Command(c));
                };
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
                } else if id == items.artwork.id() {
                    match picker.as_ref() {
                        Some(p) => p.focus(),
                        None => send(Command::OpenArtwork),
                    }
                } else if id == items.setup.id() {
                    send(Command::SetupPlayers);
                } else if id == items.reconnect.id() {
                    send(Command::Reconnect);
                } else if id == items.reload.id() {
                    send(Command::ReloadConfig);
                } else if id == items.open_config.id() {
                    platform::open_path(&paths::config_file(), true);
                } else if id == items.open_logs.id() {
                    platform::open_path(&paths::log_dir(), false);
                } else if id == items.clear_cache.id() {
                    send(Command::ClearCache);
                } else if id == items.login.id()
                    && let Err(e) = platform::set_launch_at_login(items.login.is_checked())
                {
                    tracing::error!(error = %e, "could not change launch at login");
                    items.login.set_checked(platform::launch_at_login_enabled());
                }
            }
            _ => {}
        }
    })
}
