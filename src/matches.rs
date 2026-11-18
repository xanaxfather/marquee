//! "Fix Match…": titles the user picked by hand when automatic matching got
//! it wrong. Stored as JSON in Application Support, for one file, a whole
//! folder, or every file whose name parses to the same title.

use crate::metadata::MediaKind;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchOverride {
    pub kind: MediaKind,
    pub tmdb_id: u64,
    /// For display and logs.
    pub title: String,
    #[serde(default)]
    pub year: Option<u16>,
    /// Exact episode, when the file's numbering is wrong (file scope only).
    #[serde(default)]
    pub season: Option<u32>,
    #[serde(default)]
    pub episode: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchScope {
    File,
    Folder,
    Name,
}

pub fn key(scope: MatchScope, path: &str, parsed_title: &str) -> String {
    match scope {
        MatchScope::File => format!("file:{path}"),
        MatchScope::Folder => {
            format!("dir:{}", Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())
        }
        MatchScope::Name => format!("name:{}", crate::matching::normalize(parsed_title)),
    }
}

pub struct Store {
    path: PathBuf,
    map: HashMap<String, MatchOverride>,
    mtime: Option<std::time::SystemTime>,
}

fn mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Store {
    pub fn open() -> Self {
        Self::at(crate::paths::matches_file())
    }

    fn at(path: PathBuf) -> Self {
        let mut s = Self { path, map: HashMap::new(), mtime: None };
        s.reload();
        s
    }

    fn reload(&mut self) {
        self.mtime = mtime(&self.path);
        self.map = std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    }

    /// Pick up changes saved by another part of the app.
    pub fn refresh(&mut self) {
        if mtime(&self.path) != self.mtime {
            self.reload();
        }
    }

    /// Most specific first: this file, its folder, then the parsed title.
    pub fn lookup(&self, path: &str, parsed_title: &str) -> Option<(MatchScope, &MatchOverride)> {
        [MatchScope::File, MatchScope::Folder, MatchScope::Name]
            .into_iter()
            .find_map(|s| self.map.get(&key(s, path, parsed_title)).map(|m| (s, m)))
    }

    pub fn set(&mut self, key: &str, value: Option<MatchOverride>) {
        self.refresh();
        match value {
            Some(v) => self.map.insert(key.to_string(), v),
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
            Ok(()) => tracing::info!(key, "match override saved"),
            Err(e) => tracing::error!(error = %e, "could not save match override"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_specific_scope_wins() {
        let path = std::env::temp_dir().join(format!("marquee-matches-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut s = Store::at(path.clone());
        let m = |id| MatchOverride {
            kind: MediaKind::Episode,
            tmdb_id: id,
            title: "x".into(),
            year: None,
            season: None,
            episode: None,
        };
        let file = "/tv/Show/Season 1/show.s01e01.mkv";
        s.set(&key(MatchScope::Name, file, "Show"), Some(m(1)));
        assert_eq!(s.lookup("/elsewhere/show.s02e05.mkv", "show").unwrap().1.tmdb_id, 1);
        s.set(&key(MatchScope::Folder, file, "Show"), Some(m(2)));
        assert_eq!(s.lookup("/tv/Show/Season 1/show.s01e02.mkv", "Show").unwrap().1.tmdb_id, 2);
        s.set(&key(MatchScope::File, file, "Show"), Some(m(3)));
        assert_eq!(s.lookup(file, "Show").unwrap(), (MatchScope::File, &m(3)));
        // Survives a reopen.
        assert_eq!(Store::at(path.clone()).lookup(file, "Show").unwrap().1.tmdb_id, 3);
        let _ = std::fs::remove_file(&path);
    }
}
