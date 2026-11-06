//! Which image Discord shows: per-title overrides chosen in the picker,
//! otherwise the default mode from the config.

use crate::config::{ArtworkMode, Config};
use crate::metadata::{Media, MediaKind};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Episode,
    Season,
    /// The whole show, or the movie.
    Show,
}

/// Everything the picker window needs about the current title.
#[derive(Debug, Clone, Serialize)]
pub struct Target {
    pub kind: MediaKind,
    pub tmdb_id: u64,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    /// TMDB's (season, episode) when the file uses different numbering.
    pub tmdb_numbers: Option<(u32, u32)>,
    pub language: String,
    /// Preview text, as Discord shows it.
    pub app_name: String,
    pub details: String,
    pub state: String,
    pub position: Option<f64>,
    pub paused: bool,
    /// Image currently sent to Discord, and what "Reset" would go back to.
    pub current: Option<String>,
    pub default: Option<String>,
    /// Existing overrides per scope.
    pub overrides: HashMap<Scope, String>,
    pub tmdb_url: String,
}

impl Target {
    pub fn key(&self, scope: Scope) -> Option<String> {
        key(self.kind, self.tmdb_id, self.season, self.episode, scope)
    }
}

fn key(kind: MediaKind, id: u64, season: Option<u32>, episode: Option<u32>, scope: Scope) -> Option<String> {
    match (kind, scope) {
        (MediaKind::Movie, _) => Some(format!("movie:{id}")),
        (MediaKind::Episode, Scope::Show) => Some(format!("tv:{id}")),
        (MediaKind::Episode, Scope::Season) => Some(format!("tv:{id}:s{}", season?)),
        (MediaKind::Episode, Scope::Episode) => Some(format!("tv:{id}:s{}:e{}", season?, episode?)),
    }
}

fn media_key(m: &Media, scope: Scope) -> Option<String> {
    let (s, e) = match &m.episode {
        Some(ep) => (Some(ep.number.season), Some(ep.number.episode)),
        None => (None, None),
    };
    key(m.kind, m.tmdb_id, s, e, scope)
}

/// Overrides chosen in the picker, persisted as JSON in Application Support.
pub struct Store {
    path: PathBuf,
    map: HashMap<String, String>,
    mtime: Option<std::time::SystemTime>,
}

fn mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Store {
    pub fn open() -> Self {
        let mut s = Self { path: crate::paths::artwork_file(), map: HashMap::new(), mtime: None };
        s.reload();
        s
    }

    fn reload(&mut self) {
        self.mtime = mtime(&self.path);
        self.map = std::fs::read(&self.path)
            .ok()
            .and_then(|b| match serde_json::from_slice(&b) {
                Ok(m) => Some(m),
                Err(e) => {
                    tracing::warn!(error = %e, path = %self.path.display(), "artwork overrides unreadable; ignoring");
                    None
                }
            })
            .unwrap_or_default();
    }

    /// Pick up changes made by another process (e.g. `--artwork-picker`).
    /// Returns true if the file changed.
    pub fn refresh(&mut self) -> bool {
        if mtime(&self.path) != self.mtime {
            self.reload();
            return true;
        }
        false
    }

    pub fn set(&mut self, key: &str, url: Option<String>) {
        self.refresh();
        match url {
            Some(u) => self.map.insert(key.to_string(), u),
            None => self.map.remove(key),
        };
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = self.path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&self.map)?)?;
            std::fs::rename(&tmp, &self.path)
        })();
        self.mtime = mtime(&self.path);
        match result {
            Ok(()) => tracing::info!(key, "artwork override saved"),
            Err(e) => tracing::error!(error = %e, "could not save artwork override"),
        }
    }

    pub fn overrides(&self, m: &Media) -> HashMap<Scope, String> {
        [Scope::Episode, Scope::Season, Scope::Show]
            .into_iter()
            .filter_map(|s| {
                let k = media_key(m, s)?;
                Some((s, self.map.get(&k)?.clone()))
            })
            .collect()
    }

    /// Most specific override wins: episode → season → show.
    pub fn lookup(&self, m: &Media) -> Option<String> {
        [Scope::Episode, Scope::Season, Scope::Show]
            .into_iter()
            .filter_map(|s| media_key(m, s))
            .find_map(|k| self.map.get(&k).cloned())
    }
}

/// Image used when no override is set.
pub fn default_for(mode: ArtworkMode, m: &Media) -> Option<String> {
    let still = m.episode.as_ref().and_then(|e| e.still.clone());
    let chain: [Option<String>; 4] = match mode {
        ArtworkMode::Episode => [still, m.season_poster.clone(), m.poster.clone(), m.backdrop.clone()],
        ArtworkMode::Season => [m.season_poster.clone(), m.poster.clone(), still, m.backdrop.clone()],
        ArtworkMode::Poster => [m.poster.clone(), m.season_poster.clone(), still, m.backdrop.clone()],
        ArtworkMode::Backdrop => [m.backdrop.clone(), still, m.poster.clone(), None],
    };
    chain.into_iter().flatten().next()
}

pub fn effective(cfg: &Config, store: &Store, m: &Media) -> Option<String> {
    store.lookup(m).or_else(|| default_for(cfg.artwork, m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::EpisodeInfo;
    use crate::parse::EpisodeNo;

    fn ep() -> Media {
        Media {
            kind: MediaKind::Episode,
            tmdb_id: 7,
            title: "Show".into(),
            year: None,
            poster: Some("poster".into()),
            backdrop: Some("backdrop".into()),
            season_poster: Some("season".into()),
            genres: vec![],
            rating: None,
            tagline: None,
            imdb_id: None,
            episode: Some(EpisodeInfo {
                number: EpisodeNo { season: 1, episode: 2, last: None },
                tmdb: None,
                name: None,
                air_date: None,
                still: Some("still".into()),
            }),
        }
    }

    #[test]
    fn defaults_and_fallbacks() {
        let mut m = ep();
        assert_eq!(default_for(ArtworkMode::Episode, &m).as_deref(), Some("still"));
        assert_eq!(default_for(ArtworkMode::Season, &m).as_deref(), Some("season"));
        assert_eq!(default_for(ArtworkMode::Backdrop, &m).as_deref(), Some("backdrop"));
        m.episode.as_mut().unwrap().still = None;
        assert_eq!(default_for(ArtworkMode::Episode, &m).as_deref(), Some("season"));
    }

    #[test]
    fn most_specific_override_wins() {
        let m = ep();
        let mut store = Store {
            path: std::env::temp_dir().join("iina-presence-test-artwork.json"),
            map: HashMap::new(),
            mtime: None,
        };
        store.map.insert("tv:7".into(), "show-pick".into());
        assert_eq!(store.lookup(&m).as_deref(), Some("show-pick"));
        store.map.insert("tv:7:s1".into(), "season-pick".into());
        assert_eq!(store.lookup(&m).as_deref(), Some("season-pick"));
        store.map.insert("tv:7:s1:e2".into(), "ep-pick".into());
        assert_eq!(store.lookup(&m).as_deref(), Some("ep-pick"));
        store.map.insert("tv:7:s1:e3".into(), "other".into());
        assert_eq!(store.overrides(&m).len(), 3);
    }
}
