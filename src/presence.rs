//! Build the Discord activity payload from playback state + metadata.

use crate::config::{Config, StatusDisplay};
use crate::metadata::{Media, MediaKind, Resolution};
use crate::parse::Guess;
use crate::players::Snapshot;
use serde_json::{Map, Value, json};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const ACTIVITY_WATCHING: u8 = 3;
const MAX_TEXT: usize = 128;

/// Discord requires 2..=128 characters for text fields.
fn text(s: &str) -> String {
    let s = s.trim();
    let mut out: String = if s.chars().count() > MAX_TEXT {
        s.chars().take(MAX_TEXT - 1).chain(std::iter::once('…')).collect()
    } else {
        s.to_string()
    };
    while out.chars().count() < 2 {
        out.push('\u{200b}');
    }
    out
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn with_year(title: &str, year: Option<u16>) -> String {
    match year {
        Some(y) => format!("{title} ({y})"),
        None => title.to_string(),
    }
}

struct Lines {
    details: String,
    state: String,
    large_text: String,
    image: Option<String>,
    buttons: Vec<(String, String)>,
}

/// Placeholders a template line may use (listed in Settings).
pub const PLACEHOLDERS: &[&str] = &[
    "title",
    "year",
    "label",
    "season",
    "episode",
    "episode_title",
    "air_date",
    "air_year",
    "genres",
    "rating",
    "tagline",
    "player",
];

pub const DEFAULT_EPISODE_DETAILS: &str = "{title}";
pub const DEFAULT_EPISODE_STATE: &str = "{label} · {episode_title}";
pub const DEFAULT_EPISODE_IMAGE: &str = "{title} ({year}) · aired {air_year}";
pub const DEFAULT_MOVIE_DETAILS: &str = "{title}";
pub const DEFAULT_MOVIE_STATE: &str = "{year} · {genres} · ★ {rating}";
pub const DEFAULT_MOVIE_IMAGE: &str = "{tagline|title}";

fn or_default<'a>(t: &'a str, d: &'a str) -> &'a str {
    if t.trim().is_empty() { d } else { t }
}

/// Fill a template. A " · " part whose placeholders are all empty is
/// dropped, and brackets left empty ("()") disappear with their space.
/// `{a|b}` uses `b` when `a` is empty.
pub fn render(template: &str, vars: &dyn Fn(&str) -> Option<String>) -> String {
    let lookup = |name: &str| name.split('|').find_map(|n| vars(n.trim()).filter(|v| !v.trim().is_empty()));
    let mut parts = Vec::new();
    for part in template.split(" · ") {
        let mut out = String::new();
        let (mut total, mut filled) = (0, 0);
        let mut rest = part;
        while let Some(open) = rest.find('{') {
            let Some(close) = rest[open..].find('}') else { break };
            out.push_str(&rest[..open]);
            total += 1;
            if let Some(v) = lookup(&rest[open + 1..open + close]) {
                filled += 1;
                out.push_str(&v);
            }
            rest = &rest[open + close + 1..];
        }
        out.push_str(rest);
        if total > 0 && filled == 0 {
            continue;
        }
        let mut cleaned = out;
        for empty in ["()", "[]", "“”", "\"\""] {
            cleaned = cleaned.replace(&format!(" {empty}"), "").replace(empty, "");
        }
        let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        if !cleaned.is_empty() {
            parts.push(cleaned);
        }
    }
    parts.join(" · ")
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Placeholders in a URL are percent-encoded.
fn render_url(template: &str, vars: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else { break };
        out.push_str(&rest[..open]);
        out.push_str(&encode(&vars(&rest[open + 1..open + close]).unwrap_or_default()));
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

/// Built-in buttons: (config key, label). Settings lists these.
pub const BUTTONS: &[(&str, &str)] = &[
    ("tmdb", "View on TMDB"),
    ("imdb", "View on IMDb"),
    ("letterboxd", "View on Letterboxd"),
    ("trakt", "View on Trakt"),
    ("trailer", "Watch Trailer"),
    ("wikipedia", "Wikipedia"),
];

fn media_buttons(cfg: &Config, m: &Media, vars: &dyn Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    if !cfg.show_buttons {
        return vec![];
    }
    let query = match (m.kind, m.year) {
        (MediaKind::Movie, Some(y)) => format!("{} {y}", m.title),
        _ => m.title.clone(),
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for b in &cfg.buttons {
        let key = b.trim().to_ascii_lowercase();
        let url = match key.as_str() {
            "tmdb" => Some(m.tmdb_url()),
            "imdb" => m.imdb_url(),
            "letterboxd" => (m.kind == MediaKind::Movie).then(|| format!("https://letterboxd.com/tmdb/{}/", m.tmdb_id)),
            "trakt" => Some(format!(
                "https://trakt.tv/search/tmdb/{}?id_type={}",
                m.tmdb_id,
                if m.kind == MediaKind::Movie { "movie" } else { "show" }
            )),
            "trailer" => {
                Some(format!("https://www.youtube.com/results?search_query={}", encode(&format!("{query} trailer"))))
            }
            "wikipedia" => Some(format!(
                "https://en.wikipedia.org/w/index.php?search={}",
                encode(&match m.kind {
                    MediaKind::Movie => format!("{query} film"),
                    MediaKind::Episode => format!("{} TV series", m.title),
                })
            )),
            _ => None,
        };
        if let Some(url) = url {
            let label = BUTTONS.iter().find(|(id, _)| *id == key).map(|(_, l)| l.to_string()).unwrap_or_default();
            out.push((label, url));
            continue;
        }
        // Custom: "Label | https://example.com/?q={title}"
        if let Some((l, u)) = b.split_once('|') {
            let url = render_url(u.trim(), vars);
            let label = render(l.trim(), vars);
            if !label.is_empty() && url.starts_with("https://") {
                out.push((label, url));
            }
        }
    }
    out.truncate(2);
    out
}

fn media_vars<'a>(cfg: &'a Config, m: &'a Media, player: &'a str) -> impl Fn(&str) -> Option<String> + 'a {
    move |name: &str| {
        let ep = m.episode.as_ref();
        match name {
            "title" => Some(m.title.clone()),
            "year" => m.year.map(|y| y.to_string()),
            "label" => ep.map(|e| {
                // A dated episode TMDB doesn't list keeps its date as the label.
                let dated = e.number.is_year_season() && e.number.episode == 0;
                match (&e.air_date, dated) {
                    (Some(d), true) => d.clone(),
                    _ => e.number.label(),
                }
            }),
            "season" => ep.map(|e| e.number.season.to_string()),
            "episode" => ep.filter(|e| e.number.episode > 0).map(|e| e.number.episode.to_string()),
            "episode_title" => ep.and_then(|e| e.name.clone()),
            "air_date" => ep.and_then(|e| e.air_date.clone()),
            "air_year" if cfg.show_extras => ep.and_then(|e| e.air_date.as_deref()?.get(..4).map(String::from)),
            "genres" if cfg.show_extras => {
                (!m.genres.is_empty()).then(|| m.genres.iter().take(2).cloned().collect::<Vec<_>>().join(", "))
            }
            "rating" if cfg.show_extras => m.rating.map(|r| format!("{r:.1}")),
            "tagline" if cfg.show_extras => m.tagline.clone(),
            "player" => (!player.is_empty()).then(|| player.to_string()),
            _ => None,
        }
    }
}

fn media_lines(cfg: &Config, m: &Media, player: &str) -> Lines {
    let vars = media_vars(cfg, m, player);
    let (d, s, i) = match m.kind {
        MediaKind::Episode => (
            or_default(&cfg.episode_details, DEFAULT_EPISODE_DETAILS),
            or_default(&cfg.episode_state, DEFAULT_EPISODE_STATE),
            or_default(&cfg.episode_image_text, DEFAULT_EPISODE_IMAGE),
        ),
        MediaKind::Movie => (
            or_default(&cfg.movie_details, DEFAULT_MOVIE_DETAILS),
            or_default(&cfg.movie_state, DEFAULT_MOVIE_STATE),
            or_default(&cfg.movie_image_text, DEFAULT_MOVIE_IMAGE),
        ),
    };
    let or = |s: String, fallback: String| if s.trim().is_empty() { fallback } else { s };
    let title_year = with_year(&m.title, m.year);
    let state_fallback = if m.kind == MediaKind::Movie { "Movie".to_string() } else { title_year.clone() };
    Lines {
        details: or(render(d, &vars), m.title.clone()),
        state: or(render(s, &vars), state_fallback),
        large_text: or(render(i, &vars), title_year),
        image: m.poster.clone(),
        buttons: media_buttons(cfg, m, &vars),
    }
}

fn guess_lines(g: &Guess, snap: &Snapshot) -> Lines {
    let title = if g.title.is_empty() {
        snap.media_title.as_deref().unwrap_or("Something").to_string()
    } else {
        g.title.clone()
    };
    let state = match (g.episode_label(), &g.episode_title, g.year) {
        (Some(ep), Some(name), _) => format!("{ep} · {name}"),
        (Some(ep), None, _) => ep,
        (None, _, Some(y)) => y.to_string(),
        (None, _, None) if !snap.player.is_empty() => snap.player.clone(),
        (None, _, None) => "Video".to_string(),
    };
    Lines { large_text: with_year(&title, g.year), details: title, state, image: None, buttons: vec![] }
}

/// Whether the privacy lists cover this file or title.
pub fn is_private(cfg: &Config, path: Option<&str>, resolution: Option<&Resolution>) -> bool {
    use crate::matching::normalize;
    let titles: Vec<&str> = match resolution {
        Some(Resolution { media: Some(m), guess, .. }) => vec![m.title.as_str(), guess.title.as_str()],
        Some(r) => vec![r.guess.title.as_str()],
        None => vec![],
    };
    let title_hit =
        cfg.hide_titles.iter().map(|h| h.trim()).filter(|h| !h.is_empty()).any(|h| match h.strip_suffix('*') {
            Some(prefix) => titles.iter().any(|t| !t.is_empty() && normalize(t).starts_with(&normalize(prefix))),
            None => titles.iter().any(|t| !t.is_empty() && normalize(t) == normalize(h)),
        });
    let folder_hit = path.is_some_and(|p| {
        let p = p.replace('\\', "/").to_lowercase();
        cfg.hide_folders.iter().map(|f| f.trim()).filter(|f| !f.is_empty()).any(|f| {
            let f = match f.strip_prefix("~/") {
                Some(rest) => crate::paths::home().join(rest).to_string_lossy().into_owned(),
                None => f.to_string(),
            };
            let f = f.replace('\\', "/").to_lowercase();
            let f = f.trim_end_matches('/');
            p == f || p.starts_with(&format!("{f}/"))
        })
    });
    let genre_hit = match resolution {
        Some(Resolution { media: Some(m), .. }) => {
            m.genres.iter().any(|g| cfg.hide_genres.iter().any(|h| h.trim().eq_ignore_ascii_case(g)))
        }
        _ => false,
    };
    title_hit || folder_hit || genre_hit
}

/// The activity to show, with the privacy lists deciding whether it's generic.
pub fn build(
    cfg: &Config,
    snap: &Snapshot,
    resolution: Option<&Resolution>,
    image: Option<String>,
    now: Instant,
) -> Value {
    let private = is_private(cfg, snap.path.as_deref(), resolution);
    build_with(cfg, snap, resolution, image, now, private)
}

/// The activity to show; `private` = "Watching something" with no title,
/// artwork or links (the caller has applied rules and privacy lists).
pub fn build_with(
    cfg: &Config,
    snap: &Snapshot,
    resolution: Option<&Resolution>,
    image: Option<String>,
    now: Instant,
    private: bool,
) -> Value {
    let mut lines = match resolution {
        _ if private => Lines {
            details: "Something".into(),
            state: "Private".into(),
            large_text: String::new(),
            image: None,
            buttons: vec![],
        },
        Some(Resolution { media: Some(m), .. }) => media_lines(cfg, m, &snap.player),
        Some(r) => guess_lines(&r.guess, snap),
        None => {
            // Resolution in progress (first ~second of a new file).
            let path = snap.path.as_deref().unwrap_or("");
            let g = crate::parse::display_fallback(path);
            guess_lines(&g, snap)
        }
    };
    // Lowercase look: every visible string, never URLs or asset keys.
    let case = |s: String| if cfg.lowercase { s.to_lowercase() } else { s };

    if image.is_some() && !private {
        lines.image = image;
    }

    let mut a = Map::new();
    a.insert("type".into(), json!(ACTIVITY_WATCHING));
    a.insert(
        "status_display_type".into(),
        json!(match cfg.status_display {
            StatusDisplay::Name => 0,
            StatusDisplay::State => 1,
            StatusDisplay::Details => 2,
        }),
    );
    a.insert("details".into(), json!(text(&case(lines.details))));

    let paused = snap.paused;
    let state = if paused { format!("Paused · {}", lines.state) } else { lines.state };
    a.insert("state".into(), json!(text(&case(state))));

    let mut assets = Map::new();
    let fallback = (!cfg.fallback_image.is_empty()).then(|| cfg.fallback_image.clone());
    if let Some(img) = lines.image.or(fallback) {
        assets.insert("large_image".into(), json!(img));
        if !lines.large_text.is_empty() {
            assets.insert("large_text".into(), json!(text(&case(lines.large_text))));
        }
    }
    let small = if paused { &cfg.small_image_paused } else { &cfg.small_image_playing };
    if !small.is_empty() {
        assets.insert("small_image".into(), json!(small));
        assets.insert("small_text".into(), json!(case(if paused { "Paused" } else { "Playing" }.to_string())));
    }
    if !assets.is_empty() {
        a.insert("assets".into(), Value::Object(assets));
    }

    // Progress bar: Discord animates it from start/end; nothing to update while playing.
    if !paused && let Some(pos) = snap.position_now(now) {
        let speed = if snap.speed > 0.0 { snap.speed } else { 1.0 };
        let start = now_ms() - (pos / speed * 1000.0) as i64;
        let mut ts = Map::new();
        ts.insert("start".into(), json!(start));
        if let Some(d) = snap.duration {
            ts.insert("end".into(), json!(start + (d / speed * 1000.0) as i64));
        }
        a.insert("timestamps".into(), Value::Object(ts));
    }

    // Clickable title/artwork (newer Discord clients), shown to everyone incl. yourself.
    if let Some((_, url)) = lines.buttons.first() {
        a.insert("details_url".into(), json!(url));
        if let Some(assets) = a.get_mut("assets").and_then(Value::as_object_mut)
            && assets.contains_key("large_image")
        {
            assets.insert("large_url".into(), json!(url));
        }
    }

    if !lines.buttons.is_empty() {
        let b: Vec<Value> =
            lines.buttons.into_iter().take(2).map(|(label, url)| json!({"label": case(label), "url": url})).collect();
        a.insert("buttons".into(), Value::Array(b));
    }
    Value::Object(a)
}

/// Equal, ignoring timestamp jitter below two seconds.
pub fn same(a: &Value, b: &Value) -> bool {
    let strip = |v: &Value| {
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("timestamps");
        }
        v
    };
    if strip(a) != strip(b) {
        return false;
    }
    let ts = |v: &Value, k: &str| v["timestamps"][k].as_i64();
    let close = |x: Option<i64>, y: Option<i64>| match (x, y) {
        (Some(x), Some(y)) => (x - y).abs() < 2000,
        (None, None) => true,
        _ => false,
    };
    close(ts(a, "start"), ts(b, "start")) && close(ts(a, "end"), ts(b, "end"))
}

/// Short human summary for the menu bar (the menu item's icon shows play/pause).
pub fn summary(resolution: Option<&Resolution>) -> String {
    match resolution {
        Some(Resolution { media: Some(m), .. }) => match &m.episode {
            Some(e) => format!("{} · {}", m.title, e.number.label()),
            None => with_year(&m.title, m.year),
        },
        Some(r) => {
            let g = &r.guess;
            match g.episode_label() {
                Some(e) => format!("{} · {e} (unmatched)", g.title),
                None => format!("{} (unmatched)", with_year(&g.title, g.year)),
            }
        }
        None => "Identifying…".into(),
    }
}

/// Example titles for the Settings preview when nothing is playing.
pub fn samples() -> (Media, Media) {
    use crate::metadata::EpisodeInfo;
    use crate::parse::EpisodeNo;
    let episode = Media {
        kind: MediaKind::Episode,
        tmdb_id: 1396,
        title: "Breaking Bad".into(),
        year: Some(2008),
        poster: Some("https://image.tmdb.org/t/p/w500/anFx9aTOOYqgS3v7x3R84Kz67ly.jpg".into()),
        backdrop: None,
        season_poster: None,
        genres: vec!["Drama".into(), "Crime".into()],
        rating: Some(8.9),
        tagline: None,
        imdb_id: Some("tt0903747".into()),
        episode: Some(EpisodeInfo {
            number: EpisodeNo { season: 2, episode: 3, last: None },
            tmdb: None,
            name: Some("Bit by a Dead Bee".into()),
            air_date: Some("2009-03-22".into()),
            still: None,
        }),
    };
    let movie = Media {
        kind: MediaKind::Movie,
        tmdb_id: 949,
        title: "Heat".into(),
        year: Some(1995),
        poster: Some("https://image.tmdb.org/t/p/w500/umSVjVdbVwtx5ryCA2QXL44Durm.jpg".into()),
        backdrop: None,
        season_poster: None,
        genres: vec!["Crime".into(), "Drama".into(), "Action".into()],
        rating: Some(7.9),
        tagline: Some("A Los Angeles crime saga.".into()),
        imdb_id: Some("tt0113277".into()),
        episode: None,
    };
    (episode, movie)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::EpisodeInfo;
    use crate::parse::EpisodeNo;

    fn snap(paused: bool) -> Snapshot {
        Snapshot {
            path: Some("/x/Breaking.Bad.S02E03.mkv".into()),
            duration: Some(2820.0),
            position: Some(600.0),
            paused,
            idle: false,
            ..Default::default()
        }
    }

    fn episode() -> Resolution {
        Resolution {
            path: "/x".into(),
            guess: crate::parse::parse_name("Breaking.Bad.S02E03.mkv"),
            media: Some(Media {
                kind: MediaKind::Episode,
                tmdb_id: 1396,
                title: "Breaking Bad".into(),
                year: Some(2008),
                poster: Some("https://image.tmdb.org/t/p/w500/x.jpg".into()),
                backdrop: None,
                season_poster: None,
                genres: vec!["Drama".into()],
                rating: Some(8.9),
                tagline: None,
                imdb_id: Some("tt0903747".into()),
                episode: Some(EpisodeInfo {
                    number: EpisodeNo { season: 2, episode: 3, last: None },
                    tmdb: None,
                    name: Some("Bit by a Fly".into()),
                    air_date: Some("2009-03-22".into()),
                    still: None,
                }),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn episode_activity() {
        let cfg = Config::default();
        let r = episode();
        let now = Instant::now();
        let a = build(&cfg, &snap(false), Some(&r), None, now);
        assert_eq!(a["type"], 3);
        assert_eq!(a["details"], "Breaking Bad");
        assert_eq!(a["state"], "S2 - E3 · Bit by a Fly");
        assert_eq!(a["assets"]["large_image"], "https://image.tmdb.org/t/p/w500/x.jpg");
        assert_eq!(a["assets"]["large_text"], "Breaking Bad (2008) · aired 2009");
        let ts = &a["timestamps"];
        assert_eq!(ts["end"].as_i64().unwrap() - ts["start"].as_i64().unwrap(), 2_820_000);
        assert_eq!(a["buttons"].as_array().unwrap().len(), 2);

        let p = build(&cfg, &snap(true), Some(&r), None, now);
        assert_eq!(p["state"], "Paused · S2 - E3 · Bit by a Fly");
        assert!(p.get("timestamps").is_none());
        assert!(!same(&a, &p));
        assert!(same(&a, &build(&cfg, &snap(false), Some(&r), None, now)));
    }

    #[test]
    fn movie_defaults_match_the_old_layout() {
        let (_, movie) = samples();
        let cfg = Config::default();
        let r = Resolution { path: "/x".into(), media: Some(movie), ..Default::default() };
        let a = build(&cfg, &snap(false), Some(&r), None, Instant::now());
        assert_eq!(a["details"], "Heat");
        assert_eq!(a["state"], "1995 · Crime, Drama · ★ 7.9");
        assert_eq!(a["assets"]["large_text"], "A Los Angeles crime saga.");
        let plain = Config { show_extras: false, ..Config::default() };
        let a = build(&plain, &snap(false), Some(&r), None, Instant::now());
        assert_eq!(a["state"], "1995");
        assert_eq!(a["assets"]["large_text"], "Heat");
    }

    #[test]
    fn templates() {
        let vars = |n: &str| match n {
            "title" => Some("Heat".to_string()),
            "year" => None,
            "label" => Some("S1 - E2".to_string()),
            _ => None,
        };
        assert_eq!(render("{title} ({year})", &vars), "Heat");
        assert_eq!(render("{label} · {episode_title}", &vars), "S1 - E2");
        assert_eq!(render("watching {title} · aired {air_year}", &vars), "watching Heat");
        assert_eq!(render("{tagline|title}", &vars), "Heat");
        assert_eq!(render("Now: {title}!", &vars), "Now: Heat!");

        let cfg = Config {
            episode_details: "{title} · {label}".into(),
            episode_state: "{episode_title} on {player}".into(),
            ..Config::default()
        };
        let mut s = snap(false);
        s.player = "IINA".into();
        let a = build(&cfg, &s, Some(&episode()), None, Instant::now());
        assert_eq!(a["details"], "Breaking Bad · S2 - E3");
        assert_eq!(a["state"], "Bit by a Fly on IINA");
    }

    #[test]
    fn buttons_in_order_with_custom() {
        let cfg = Config {
            buttons: vec!["letterboxd".into(), "trakt".into(), "Search | https://example.com/?q={title}".into()],
            ..Config::default()
        };
        let a = build(&cfg, &snap(false), Some(&episode()), None, Instant::now());
        // Letterboxd is movies-only, so Trakt and the custom button remain.
        assert_eq!(a["buttons"][0]["label"], "View on Trakt");
        assert_eq!(a["buttons"][0]["url"], "https://trakt.tv/search/tmdb/1396?id_type=show");
        assert_eq!(a["buttons"][1]["url"], "https://example.com/?q=Breaking+Bad");
        assert_eq!(a["details_url"], "https://trakt.tv/search/tmdb/1396?id_type=show");
        let none = Config { buttons: vec![], ..Config::default() };
        let a = build(&none, &snap(false), Some(&episode()), None, Instant::now());
        assert!(a.get("buttons").is_none() && a.get("details_url").is_none());
    }

    #[test]
    fn privacy() {
        let r = episode();
        for cfg in [
            Config { hide_titles: vec!["breaking bad".into()], ..Config::default() },
            Config { hide_titles: vec!["Breaking*".into()], ..Config::default() },
            Config { hide_folders: vec!["/x/".into()], ..Config::default() },
            Config { hide_genres: vec!["drama".into()], ..Config::default() },
        ] {
            assert!(is_private(&cfg, Some("/x/Breaking.Bad.S02E03.mkv"), Some(&r)));
            let a = build(&cfg, &snap(false), Some(&r), Some("https://img".into()), Instant::now());
            assert_eq!(a["details"], "Something");
            assert!(a["assets"].get("large_image").is_none() && a.get("buttons").is_none());
        }
        let cfg =
            Config { hide_titles: vec!["Breaking".into()], hide_folders: vec!["/xy".into()], ..Config::default() };
        assert!(!is_private(&cfg, Some("/x/Breaking.Bad.S02E03.mkv"), Some(&r)));
    }

    #[test]
    fn lowercase_activity() {
        let cfg = Config { lowercase: true, ..Config::default() };
        let r = episode();
        let a = build(&cfg, &snap(true), Some(&r), None, Instant::now());
        assert_eq!(a["details"], "breaking bad");
        assert_eq!(a["state"], "paused · s2 - e3 · bit by a fly");
        assert_eq!(a["assets"]["large_text"], "breaking bad (2008) · aired 2009");
        assert_eq!(a["assets"]["small_text"], "paused");
        assert_eq!(a["buttons"][0]["label"], "view on tmdb");
        // URLs keep their case.
        assert_eq!(a["assets"]["large_image"], "https://image.tmdb.org/t/p/w500/x.jpg");
        assert_eq!(a["assets"]["small_image"], crate::config::BADGE_PAUSED);
    }

    #[test]
    fn text_limits() {
        assert_eq!(text("x").chars().count(), 2);
        assert_eq!(text(&"a".repeat(300)).chars().count(), 128);
    }
}
