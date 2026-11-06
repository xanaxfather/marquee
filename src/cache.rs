//! Tiny persistent JSON cache for metadata lookups.
//!
//! Positive results live for 30 days, "not found" results for 12 hours so a
//! newly added title on TMDB gets picked up. Network failures are never cached.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const POSITIVE_TTL: u64 = 30 * 24 * 3600;
const NEGATIVE_TTL: u64 = 12 * 3600;
const MAX_ENTRIES: usize = 5000;
const VERSION: u32 = 3;

#[derive(Serialize, Deserialize)]
struct Entry {
    stored: u64,
    value: Value,
}

#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    version: u32,
    entries: HashMap<String, Entry>,
}

pub struct Cache {
    path: PathBuf,
    data: CacheFile,
    dirty: bool,
}

pub enum Lookup<T> {
    Hit(T),
    /// Previously looked up and not found.
    Negative,
    Miss,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn expired(e: &Entry, t: u64) -> bool {
    let ttl = if e.value.is_null() { NEGATIVE_TTL } else { POSITIVE_TTL };
    t.saturating_sub(e.stored) > ttl
}

impl Cache {
    pub fn open(path: PathBuf) -> Self {
        let mut data = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<CacheFile>(&bytes) {
                Ok(d) if d.version == VERSION => d,
                Ok(_) => {
                    tracing::info!("metadata cache format changed; starting fresh");
                    CacheFile::default()
                }
                Err(e) => {
                    tracing::warn!(error = %e, path = %path.display(), "metadata cache unreadable; starting fresh");
                    CacheFile::default()
                }
            },
            Err(_) => CacheFile::default(),
        };
        data.version = VERSION;
        let t = now();
        let before = data.entries.len();
        data.entries.retain(|_, e| !expired(e, t));
        tracing::debug!(entries = data.entries.len(), pruned = before - data.entries.len(), "metadata cache loaded");
        let dirty = before != data.entries.len();
        Self { path, data, dirty }
    }

    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Lookup<T> {
        match self.data.entries.get(key) {
            Some(e) if !expired(e, now()) => {
                if e.value.is_null() {
                    Lookup::Negative
                } else {
                    match serde_json::from_value(e.value.clone()) {
                        Ok(v) => Lookup::Hit(v),
                        Err(_) => Lookup::Miss,
                    }
                }
            }
            _ => Lookup::Miss,
        }
    }

    pub fn put<T: Serialize>(&mut self, key: &str, value: Option<&T>) {
        let value = value.and_then(|v| serde_json::to_value(v).ok()).unwrap_or(Value::Null);
        self.data.entries.insert(key.to_string(), Entry { stored: now(), value });
        if self.data.entries.len() > MAX_ENTRIES {
            // Drop the oldest tenth.
            let mut ages: Vec<(u64, String)> = self.data.entries.iter().map(|(k, e)| (e.stored, k.clone())).collect();
            ages.sort_unstable();
            for (_, k) in ages.into_iter().take(MAX_ENTRIES / 10) {
                self.data.entries.remove(&k);
            }
        }
        self.dirty = true;
    }

    pub fn len(&self) -> usize {
        self.data.entries.len()
    }

    pub fn clear(&mut self) {
        self.data.entries.clear();
        self.dirty = true;
        self.flush();
    }

    /// Atomically write to disk if anything changed.
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = self.path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let tmp = self.path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec(&self.data)?)?;
            std::fs::rename(&tmp, &self.path)
        })();
        match result {
            Ok(()) => self.dirty = false,
            Err(e) => tracing::warn!(error = %e, path = %self.path.display(), "could not write metadata cache"),
        }
    }
}
