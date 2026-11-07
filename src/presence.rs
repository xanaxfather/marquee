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

fn media_lines(cfg: &Config, m: &Media) -> Lines {
    let mut buttons = Vec::new();
    if cfg.show_buttons {
        buttons.push(("View on TMDB".to_string(), m.tmdb_url()));
        if let Some(imdb) = m.imdb_url() {
            buttons.push(("View on IMDb".to_string(), imdb));
        }
    }
    match (m.kind, &m.episode) {
        (MediaKind::Episode, Some(ep)) => {
            let mut state = ep.number.label();
            if let Some(name) = &ep.name {
                state = format!("{state} · {name}");
            }
            let mut large = with_year(&m.title, m.year);
            if cfg.show_extras
                && let Some(date) = ep.air_date.as_deref().and_then(|d| d.get(..4))
            {
                large = format!("{large} · aired {date}");
            }
            Lines { details: m.title.clone(), state, large_text: large, image: m.poster.clone(), buttons }
        }
        _ => {
            let mut parts: Vec<String> = Vec::new();
            if let Some(y) = m.year {
                parts.push(y.to_string());
            }
            if cfg.show_extras {
                if !m.genres.is_empty() {
                    parts.push(m.genres.iter().take(2).cloned().collect::<Vec<_>>().join(", "));
                }
                if let Some(r) = m.rating {
                    parts.push(format!("★ {r:.1}"));
                }
            }
            let state = if parts.is_empty() { "Movie".into() } else { parts.join(" · ") };
            let large = match (&m.tagline, cfg.show_extras) {
                (Some(t), true) => t.clone(),
                _ => with_year(&m.title, m.year),
            };
            Lines { details: m.title.clone(), state, large_text: large, image: m.poster.clone(), buttons }
        }
    }
}

fn guess_lines(g: &Guess, snap: &Snapshot) -> Lines {
    let title = if g.title.is_empty() {
        snap.media_title.as_deref().unwrap_or("Something").to_string()
    } else {
        g.title.clone()
    };
    let state = match (&g.episode, &g.episode_title, g.year) {
        (Some(ep), Some(name), _) => format!("{} · {name}", ep.label()),
        (Some(ep), None, _) => ep.label(),
        (None, _, Some(y)) => y.to_string(),
        (None, _, None) if !snap.player.is_empty() => snap.player.clone(),
        (None, _, None) => "Video".to_string(),
    };
    Lines { large_text: with_year(&title, g.year), details: title, state, image: None, buttons: vec![] }
}

/// The activity to show, or `None` to clear it.
pub fn build(
    cfg: &Config,
    snap: &Snapshot,
    resolution: Option<&Resolution>,
    image: Option<String>,
    now: Instant,
) -> Value {
    let mut lines = match resolution {
        Some(Resolution { media: Some(m), .. }) => media_lines(cfg, m),
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

    if image.is_some() {
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
    if let Some(img) = lines.image.or_else(|| (!cfg.fallback_image.is_empty()).then(|| cfg.fallback_image.clone())) {
        assets.insert("large_image".into(), json!(img));
        assets.insert("large_text".into(), json!(text(&case(lines.large_text))));
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
            match &g.episode {
                Some(e) => format!("{} · {} (unmatched)", g.title, e.label()),
                None => format!("{} (unmatched)", with_year(&g.title, g.year)),
            }
        }
        None => "Identifying…".into(),
    }
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
            transient: false,
            error: None,
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
