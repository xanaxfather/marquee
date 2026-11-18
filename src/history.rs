//! Local watch history: what was watched, when, for how long and how far.
//! Kept as JSON in Application Support and never sent anywhere.

use crate::metadata::{Media, MediaKind, Resolution};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_ENTRIES: usize = 500;
/// Hour buckets kept per entry (the oldest go first); ~170 days of nonstop play.
const MAX_HOURS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// "movie:603", "tv:1396:s2:e3", or "file:<path>" when unidentified.
    pub key: String,
    pub kind: Option<MediaKind>,
    pub title: String,
    pub year: Option<u16>,
    /// "S2 - E3"
    pub label: Option<String>,
    pub episode_title: Option<String>,
    pub image: Option<String>,
    pub tmdb_url: Option<String>,
    pub path: String,
    pub player: String,
    /// Unix seconds.
    pub first_watched: u64,
    pub last_watched: u64,
    /// Seconds actually spent playing.
    pub watched: f64,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    /// Seconds played in each hour (key: unix seconds / 3600), for Stats.
    /// Missing in files written before Stats existed; older versions ignore it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub hours: BTreeMap<u64, f64>,
}

impl Entry {
    #[cfg(test)]
    pub fn progress(&self) -> Option<f64> {
        match (self.position, self.duration) {
            (Some(p), Some(d)) if d > 0.0 => Some((p / d).clamp(0.0, 1.0)),
            _ => None,
        }
    }

    /// "Breaking Bad · S2 - E3" / "Heat (1995)"
    pub fn display(&self) -> String {
        match (&self.label, self.year) {
            (Some(l), _) => format!("{} · {l}", self.title),
            (None, Some(y)) => format!("{} ({y})", self.title),
            _ => self.title.clone(),
        }
    }
}

/// Spread `seconds` of playback that ended at `end` (unix seconds) over the
/// hours it covered.
fn credit_hours(hours: &mut BTreeMap<u64, f64>, end: u64, seconds: f64) {
    if !seconds.is_finite() {
        return;
    }
    let mut left = seconds.max(0.0);
    let mut t = end;
    // Ticks are a minute or so; the bound only guards against nonsense input.
    for _ in 0..48 {
        if left <= 0.0 || t == 0 {
            break;
        }
        let start = (t - 1) / 3600 * 3600;
        let take = left.min((t - start) as f64);
        *hours.entry(start / 3600).or_insert(0.0) += take;
        left -= take;
        t = start;
    }
    while hours.len() > MAX_HOURS {
        hours.pop_first();
    }
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The history key for what's playing.
pub fn key_for(path: &str, r: &Resolution) -> String {
    match &r.media {
        Some(Media { kind: MediaKind::Movie, tmdb_id, .. }) => format!("movie:{tmdb_id}"),
        Some(m) => match &m.episode {
            Some(e) => format!("tv:{}:s{}:e{}", m.tmdb_id, e.number.season, e.number.episode),
            None => format!("tv:{}", m.tmdb_id),
        },
        None => format!("file:{path}"),
    }
}

pub struct Store {
    path: PathBuf,
    pub entries: Vec<Entry>,
    dirty: bool,
}

impl Store {
    pub fn open() -> Self {
        Self::at(crate::paths::support_dir().join("history.json"))
    }

    fn at(path: PathBuf) -> Self {
        let entries = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Self { path, entries, dirty: false }
    }

    /// Record `seconds` more of playback and the current position.
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &mut self,
        path: &str,
        r: &Resolution,
        image: Option<String>,
        player: &str,
        seconds: f64,
        position: Option<f64>,
        duration: Option<f64>,
    ) {
        let key = key_for(path, r);
        let t = now();
        let idx = match self.entries.iter().position(|e| e.key == key) {
            Some(i) => i,
            None => {
                let (kind, title, year, label, episode_title, tmdb_url) = match &r.media {
                    Some(m) => (
                        Some(m.kind),
                        m.title.clone(),
                        m.year,
                        m.episode.as_ref().map(|e| e.number.label()),
                        m.episode.as_ref().and_then(|e| e.name.clone()),
                        Some(m.tmdb_url()),
                    ),
                    None => (
                        None,
                        r.guess.title.clone(),
                        r.guess.year,
                        r.guess.episode_label(),
                        r.guess.episode_title.clone(),
                        None,
                    ),
                };
                if title.trim().is_empty() {
                    return;
                }
                self.entries.insert(
                    0,
                    Entry {
                        key,
                        kind,
                        title,
                        year,
                        label,
                        episode_title,
                        image: None,
                        tmdb_url,
                        path: String::new(),
                        player: String::new(),
                        first_watched: t,
                        last_watched: t,
                        watched: 0.0,
                        position: None,
                        duration: None,
                        hours: BTreeMap::new(),
                    },
                );
                0
            }
        };
        let mut e = self.entries.remove(idx);
        e.last_watched = t;
        e.watched += seconds.max(0.0);
        credit_hours(&mut e.hours, t, seconds);
        e.path = path.to_string();
        if !player.is_empty() {
            e.player = player.to_string();
        }
        if image.is_some() {
            e.image = image;
        }
        if position.is_some() {
            e.position = position;
        }
        if duration.is_some() {
            e.duration = duration;
        }
        self.entries.insert(0, e);
        self.entries.truncate(MAX_ENTRIES);
        self.dirty = true;
    }

    /// Credit more playing time to an existing entry.
    pub fn add_time(&mut self, key: &str, seconds: f64, position: Option<f64>, duration: Option<f64>) {
        let Some(e) = self.entries.iter_mut().find(|e| e.key == key) else { return };
        e.watched += seconds.max(0.0);
        e.last_watched = now();
        credit_hours(&mut e.hours, e.last_watched, seconds);
        if position.is_some() {
            e.position = position;
        }
        if duration.is_some() {
            e.duration = duration;
        }
        self.dirty = true;
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.dirty = true;
        self.flush();
    }

    pub fn remove(&mut self, key: &str) {
        self.entries.retain(|e| e.key != key);
        self.dirty = true;
        self.flush();
    }

    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = self.path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec(&self.entries)?)?;
            std::fs::rename(&tmp, &self.path)
        })();
        match result {
            Ok(()) => self.dirty = false,
            Err(e) => tracing::warn!(error = %e, "could not save watch history"),
        }
    }

    /// Seconds watched since `since` (unix seconds), by last-watched time.
    pub fn watched_since(&self, since: u64) -> f64 {
        self.entries.iter().filter(|e| e.last_watched >= since).map(|e| e.watched).sum()
    }
}

/// "6h 20m", "45m", "under a minute"
pub fn duration_text(secs: f64) -> String {
    let m = (secs / 60.0).round() as u64;
    match (m / 60, m % 60) {
        (0, 0) => "under a minute".into(),
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::Guess;

    #[test]
    fn records_and_orders_by_recency() {
        let path = std::env::temp_dir().join(format!("marquee-history-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut s = Store::at(path.clone());
        let r = |t: &str| Resolution { guess: Guess { title: t.into(), ..Default::default() }, ..Default::default() };
        s.record("/a.mkv", &r("A"), None, "IINA", 60.0, Some(10.0), Some(100.0));
        s.record("/b.mkv", &r("B"), None, "VLC", 30.0, None, None);
        s.record("/a.mkv", &r("A"), None, "", 60.0, Some(50.0), None);
        assert_eq!(s.entries.len(), 2);
        assert_eq!(s.entries[0].title, "A");
        assert_eq!(s.entries[0].watched, 120.0);
        assert_eq!(s.entries[0].player, "IINA");
        assert_eq!(s.entries[0].progress(), Some(0.5));
        s.flush();
        assert_eq!(Store::at(path.clone()).entries, s.entries);
        assert_eq!(duration_text(3600.0 * 6.0 + 1200.0), "6h 20m");
        assert_eq!(duration_text(10.0), "under a minute");
        // Every second credited lands in an hour bucket.
        let total: f64 = s.entries.iter().flat_map(|e| e.hours.values()).sum();
        assert_eq!(total, 150.0);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn hours_split_at_boundaries_and_stay_bounded() {
        let mut h = BTreeMap::new();
        // 30 minutes ending 10 minutes into hour 100.
        credit_hours(&mut h, 100 * 3600 + 600, 1800.0);
        assert_eq!(h.get(&100), Some(&600.0));
        assert_eq!(h.get(&99), Some(&1200.0));
        // Ending exactly on the hour belongs to the hour before.
        let mut h = BTreeMap::new();
        credit_hours(&mut h, 200 * 3600, 60.0);
        assert_eq!(h.keys().copied().collect::<Vec<_>>(), vec![199]);
        // Nonsense in, nothing silly out.
        credit_hours(&mut h, 300 * 3600, f64::NAN);
        credit_hours(&mut h, 300 * 3600, -5.0);
        credit_hours(&mut h, 0, 60.0);
        credit_hours(&mut h, 400 * 3600, 1e12);
        assert!(h.len() <= 1 + 48);
        let mut big = BTreeMap::new();
        for i in 0..(MAX_HOURS as u64 + 10) {
            credit_hours(&mut big, (i + 1) * 3600, 60.0);
        }
        assert_eq!(big.len(), MAX_HOURS);
        assert_eq!(big.keys().next(), Some(&10));
    }

    #[test]
    fn reads_files_from_before_stats() {
        let old = r#"[{"key":"movie:1","kind":"Movie","title":"Heat","year":1995,"label":null,"episode_title":null,"image":null,"tmdb_url":null,"path":"/a.mkv","player":"IINA","first_watched":1,"last_watched":2,"watched":60.0,"position":null,"duration":null}]"#;
        let e: Vec<Entry> = serde_json::from_str(old).unwrap();
        assert!(e[0].hours.is_empty());
        // And an empty map isn't written, so the file stays as it was.
        assert!(!serde_json::to_string(&e).unwrap().contains("hours"));
    }
}
