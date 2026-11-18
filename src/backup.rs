//! Export and import of settings: the config, Fix Match overrides, artwork
//! choices and rules, in one JSON file. The TMDB key goes in only when asked
//! for; machine-specific values (the generated VLC password) never do.
//!
//! An import is checked completely before anything is written, and the files
//! are swapped in only after every new version has been written beside them,
//! so a bad file can't leave a half-imported setup behind.

use crate::config::{self, Config};
use crate::matches::MatchOverride;
use crate::rules::Rule;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// File format version. Bump it when an old Marquee could misread a new file.
pub const FORMAT: u32 = 1;
const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ITEMS: usize = 50_000;
const MAX_TEXT: usize = 4096;

/// Never exported or imported: generated for this machine / this install.
const MACHINE_ONLY: &[&str] = &["vlc_password", "setup_complete"];
/// Player socket paths only make sense on the OS they were written on.
const PLATFORM_PATHS: &[&str] = &["ipc_socket", "mpv_ipc"];
/// List settings that a merge combines instead of replacing.
const UNION_LISTS: &[&str] = &["hide_titles", "hide_folders", "hide_genres", "smtc_ignore", "disabled_players"];
const SECRET: &str = "tmdb_api_key";

fn platform() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "other"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub marquee_export: u32,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub exported_at: u64,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub config: Map<String, Value>,
    #[serde(default)]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub matches: BTreeMap<String, MatchOverride>,
    #[serde(default)]
    pub artwork: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Imported values win; lists, rules, matches and artwork are combined.
    Merge,
    /// Everything becomes what the file says (defaults where it says nothing),
    /// except the TMDB key when the file has none, and machine-only values.
    Replace,
}

/// What's on disk now.
struct Current {
    cfg: Config,
    matches: BTreeMap<String, MatchOverride>,
    artwork: BTreeMap<String, String>,
}

fn read_map<T: serde::de::DeserializeOwned>(path: &Path) -> BTreeMap<String, T> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn current() -> Result<Current> {
    // A broken config can't be merged into safely; say so instead of guessing.
    let cfg = Config::load().context("the current config file has an error; fix or reload it first")?;
    Ok(Current {
        cfg,
        matches: read_map(&crate::paths::matches_file()),
        artwork: read_map(&crate::paths::artwork_file()),
    })
}

// ------------------------------------------------------------------ export

fn bundle_from(cur: &Current, include_key: bool, now: u64) -> Result<Bundle> {
    let mut config = match serde_json::to_value(&cur.cfg)? {
        Value::Object(m) => m,
        _ => bail!("config is not an object"),
    };
    config.remove("rules");
    for k in MACHINE_ONLY {
        config.remove(*k);
    }
    if !include_key || cur.cfg.tmdb_api_key.is_empty() {
        config.remove(SECRET);
    }
    Ok(Bundle {
        marquee_export: FORMAT,
        app_version: env!("CARGO_PKG_VERSION").into(),
        exported_at: now,
        platform: platform().into(),
        config,
        rules: cur.cfg.rules.clone(),
        matches: cur.matches.clone(),
        artwork: cur.artwork.clone(),
    })
}

/// Write everything to a new file in Downloads and return its path.
/// `date` ("2026-11-18", the user's local date) goes in the file name.
pub fn export(include_key: bool, date: &str) -> Result<PathBuf> {
    let cur = current()?;
    let bundle = bundle_from(&cur, include_key, crate::history::now())?;
    let bytes = serde_json::to_vec_pretty(&bundle)?;
    let date: String = date.chars().filter(|c| c.is_ascii_digit() || *c == '-').take(10).collect();
    let stem = if date.is_empty() { "marquee-settings".to_string() } else { format!("marquee-settings-{date}") };
    let dir = crate::paths::downloads_dir();
    for n in 1..1000 {
        let name = if n == 1 { format!("{stem}.json") } else { format!("{stem}-{n}.json") };
        let path = dir.join(name);
        // Never overwrite: an earlier export with the same name stays.
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                use std::io::Write;
                if let Err(e) = f.write_all(&bytes).and_then(|_| f.sync_all()) {
                    let _ = std::fs::remove_file(&path);
                    return Err(e).with_context(|| format!("writing {}", path.display()));
                }
                tracing::info!(path = %path.display(), include_key, "settings exported");
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
        }
    }
    bail!("too many exports in {} already", dir.display())
}

// ------------------------------------------------------------------ import

fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect::<String>().trim().to_string()
}

/// Parse and check an exported file. Errors are worded for the user.
pub fn parse(text: &str) -> Result<Bundle> {
    ensure!(text.len() <= MAX_FILE_BYTES, "This file is too large to be a Marquee settings file.");
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|_| anyhow::anyhow!("This isn't a Marquee settings file (it isn't valid JSON)."))?;
    let Some(format) = v.get("marquee_export") else {
        bail!("This isn't a Marquee settings file.");
    };
    let Some(format) = format.as_u64() else {
        bail!("This settings file is damaged (its format version is unreadable).")
    };
    if format < u64::from(FORMAT) {
        bail!("This file was exported in an older format (version {format}) that this Marquee can't import.");
    }
    if format > u64::from(FORMAT) {
        bail!("This file was made by a newer Marquee (format {format}). Update Marquee, then import it again.");
    }
    let mut b: Bundle =
        serde_json::from_value(v).map_err(|e| anyhow::anyhow!("This settings file is damaged: {e}."))?;

    // Config: every key must be a setting this version knows, of the right type.
    let defaults = match serde_json::to_value(Config::default())? {
        Value::Object(m) => m,
        _ => bail!("config is not an object"),
    };
    for k in MACHINE_ONLY.iter().chain(["rules"].iter()) {
        b.config.remove(*k);
    }
    for (k, val) in &b.config {
        ensure!(defaults.contains_key(k), "This file has a setting this Marquee doesn't know: “{k}”.");
        let mut one = defaults.clone();
        one.insert(k.clone(), val.clone());
        ensure!(
            serde_json::from_value::<Config>(Value::Object(one)).is_ok(),
            "The setting “{k}” in this file has the wrong kind of value."
        );
    }
    if let Some(Value::String(key)) = b.config.get(SECRET)
        && key.trim().is_empty()
    {
        b.config.remove(SECRET);
    }

    ensure!(
        b.rules.len() <= MAX_ITEMS && b.matches.len() <= MAX_ITEMS && b.artwork.len() <= MAX_ITEMS,
        "This file has more entries than Marquee can import."
    );
    for r in &mut b.rules {
        r.player = clean(&r.player);
        r.folder = clean(&r.folder);
        ensure!(r.player.len() <= MAX_TEXT && r.folder.len() <= MAX_TEXT, "A rule in this file is too long.");
    }
    b.rules.retain(|r| !(r.player.is_empty() && r.folder.is_empty()));
    for (k, m) in &b.matches {
        ensure!(
            k.len() <= MAX_TEXT && (k.starts_with("file:") || k.starts_with("dir:") || k.starts_with("name:")),
            "A Fix Match entry in this file is damaged."
        );
        ensure!(m.title.len() <= MAX_TEXT, "A Fix Match entry in this file is damaged.");
    }
    for (k, url) in &b.artwork {
        ensure!(
            !k.is_empty() && k.len() <= MAX_TEXT && url.len() <= MAX_TEXT && url.starts_with("https://"),
            "An artwork choice in this file is damaged."
        );
    }
    Ok(b)
}

/// The settings to write (`{key: value}`, only keys that are written) and the
/// new Fix Match / artwork maps.
struct Plan {
    values: Map<String, Value>,
    matches: BTreeMap<String, MatchOverride>,
    artwork: BTreeMap<String, String>,
}

fn plan(b: &Bundle, mode: Mode, cur: &Current) -> Result<Plan> {
    let cur_cfg = match serde_json::to_value(&cur.cfg)? {
        Value::Object(m) => m,
        _ => bail!("config is not an object"),
    };
    let same_os = b.platform == platform();
    let skip = |k: &str| MACHINE_ONLY.contains(&k) || (!same_os && PLATFORM_PATHS.contains(&k));

    let mut values = Map::new();
    match mode {
        Mode::Merge => {
            for (k, v) in &b.config {
                if skip(k) {
                    continue;
                }
                let v = match (UNION_LISTS.contains(&k.as_str()), cur_cfg.get(k), v) {
                    (true, Some(Value::Array(old)), Value::Array(new)) => {
                        let mut all = old.clone();
                        for x in new {
                            if !all.contains(x) {
                                all.push(x.clone());
                            }
                        }
                        Value::Array(all)
                    }
                    _ => v.clone(),
                };
                values.insert(k.clone(), v);
            }
            let mut rules = cur.cfg.rules.clone();
            for r in &b.rules {
                if !rules.contains(r) {
                    rules.push(r.clone());
                }
            }
            values.insert("rules".into(), serde_json::to_value(rules)?);
        }
        Mode::Replace => {
            let defaults = match serde_json::to_value(Config::default())? {
                Value::Object(m) => m,
                _ => bail!("config is not an object"),
            };
            for (k, d) in defaults {
                // Left alone: machine values, the key when the file has none, and
                // socket paths from another OS. Not writing them keeps them as they are.
                if skip(&k) || k == "rules" || (k == SECRET && !b.config.contains_key(SECRET)) {
                    continue;
                }
                values.insert(k.clone(), b.config.get(&k).cloned().unwrap_or(d));
            }
            values.insert("rules".into(), serde_json::to_value(&b.rules)?);
        }
    }

    let (matches, artwork) = match mode {
        Mode::Merge => {
            let mut m = cur.matches.clone();
            m.extend(b.matches.clone());
            let mut a = cur.artwork.clone();
            a.extend(b.artwork.clone());
            (m, a)
        }
        Mode::Replace => (b.matches.clone(), b.artwork.clone()),
    };
    Ok(Plan { values, matches, artwork })
}

fn map_changes<T: PartialEq>(old: &BTreeMap<String, T>, new: &BTreeMap<String, T>) -> Value {
    let added = new.keys().filter(|k| !old.contains_key(*k)).count();
    let removed = old.keys().filter(|k| !new.contains_key(*k)).count();
    let changed = new.iter().filter(|(k, v)| old.get(*k).is_some_and(|o| o != *v)).count();
    json!({ "added": added, "changed": changed, "removed": removed, "total": new.len() })
}

/// What an import would change, for the confirmation step.
fn summarize(b: &Bundle, mode: Mode, cur: &Current) -> Result<Value> {
    let p = plan(b, mode, cur)?;
    let cur_cfg = serde_json::to_value(&cur.cfg)?;
    let mut settings: Vec<&str> = p
        .values
        .iter()
        .filter(|(k, v)| k.as_str() != "rules" && k.as_str() != SECRET && cur_cfg.get(k.as_str()) != Some(*v))
        .map(|(k, _)| k.as_str())
        .collect();
    settings.sort();
    let new_rules: Vec<Rule> = serde_json::from_value(p.values["rules"].clone())?;
    let key = match b.config.get(SECRET).and_then(Value::as_str) {
        None => "kept",
        Some(k) if k.trim() == cur.cfg.tmdb_api_key => "kept",
        Some(_) if cur.cfg.tmdb_api_key.is_empty() => "added",
        Some(_) => "replaced",
    };
    Ok(json!({
        "settings": settings,
        "rules": { "before": cur.cfg.rules.len(), "after": new_rules.len(), "changed": new_rules != cur.cfg.rules },
        "matches": map_changes(&cur.matches, &p.matches),
        "artwork": map_changes(&cur.artwork, &p.artwork),
        "tmdb_key": key,
    }))
}

/// Check a file and describe what merging or replacing would change.
pub fn preview(b: &Bundle) -> Result<Value> {
    let cur = current()?;
    Ok(json!({
        "app_version": b.app_version,
        "exported_at": b.exported_at,
        "other_os": !b.platform.is_empty() && b.platform != platform(),
        "merge": summarize(b, Mode::Merge, &cur)?,
        "replace": summarize(b, Mode::Replace, &cur)?,
    }))
}

/// Write several files all-or-nothing: every new version is written next to
/// its file first; only when all of them are down are they renamed into place.
fn write_all(files: &[(PathBuf, Vec<u8>)]) -> Result<()> {
    let mut temps = Vec::new();
    let result = (|| -> Result<()> {
        for (path, bytes) in files {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            }
            let tmp = path.with_extension("import.tmp");
            std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
            temps.push((tmp, path.clone()));
        }
        Ok(())
    })();
    if let Err(e) = result {
        for (tmp, _) in &temps {
            let _ = std::fs::remove_file(tmp);
        }
        return Err(e);
    }
    for (tmp, path) in &temps {
        std::fs::rename(tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    }
    Ok(())
}

/// Import `b` and return the new config.
pub fn apply(b: &Bundle, mode: Mode) -> Result<Config> {
    let cur = current()?;
    let p = plan(b, mode, &cur)?;
    // Everything is serialised and validated before the first byte is written.
    let text = config::prepare_json(&p.values)?;
    let files = vec![
        (crate::paths::config_file(), text.into_bytes()),
        (crate::paths::matches_file(), serde_json::to_vec_pretty(&p.matches)?),
        (crate::paths::artwork_file(), serde_json::to_vec_pretty(&p.artwork)?),
    ];
    write_all(&files)?;
    tracing::info!(
        ?mode,
        settings = p.values.len(),
        matches = p.matches.len(),
        artwork = p.artwork.len(),
        "settings imported"
    );
    Config::load()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::MediaKind;
    use crate::rules::RuleAction;

    fn sample() -> Current {
        let cfg = Config {
            tmdb_api_key: "secret-key".into(),
            vlc_password: "vlc-pass".into(),
            hide_titles: vec!["A".into()],
            rules: vec![Rule { player: "VLC".into(), folder: String::new(), action: RuleAction::Always }],
            idle_timeout: 30,
            ..Config::default()
        };
        let m = MatchOverride {
            kind: MediaKind::Movie,
            tmdb_id: 1,
            title: "Heat".into(),
            year: None,
            season: None,
            episode: None,
        };
        Current {
            cfg,
            matches: BTreeMap::from([("file:/a.mkv".to_string(), m)]),
            artwork: BTreeMap::from([("movie:1".to_string(), "https://image.tmdb.org/a.jpg".to_string())]),
        }
    }

    #[test]
    fn export_leaves_secrets_out_unless_asked() {
        let cur = sample();
        let text = serde_json::to_string(&bundle_from(&cur, false, 0).unwrap()).unwrap();
        assert!(!text.contains("secret-key") && !text.contains("vlc-pass") && !text.contains("setup_complete"));
        let with_key = serde_json::to_string(&bundle_from(&cur, true, 0).unwrap()).unwrap();
        assert!(with_key.contains("secret-key") && !with_key.contains("vlc-pass"));
        // And it reads back.
        let b = parse(&with_key).unwrap();
        assert_eq!(b.rules, cur.cfg.rules);
        assert_eq!(b.matches, cur.matches);
    }

    #[test]
    fn bad_files_fail_with_a_clear_message() {
        let err = |t: &str| parse(t).unwrap_err().to_string();
        assert!(err("not json").contains("isn't a Marquee settings file"));
        assert!(err("{}").contains("isn't a Marquee settings file"));
        assert!(err(r#"{"marquee_export":0}"#).contains("older format"));
        assert!(err(r#"{"marquee_export":99}"#).contains("newer Marquee"));
        assert!(err(r#"{"marquee_export":1,"config":{"nope":1}}"#).contains("“nope”"));
        assert!(err(r#"{"marquee_export":1,"config":{"idle_timeout":"x"}}"#).contains("“idle_timeout”"));
        assert!(err(r#"{"marquee_export":1,"artwork":{"k":"javascript:alert(1)"}}"#).contains("artwork"));
        assert!(
            err(r#"{"marquee_export":1,"matches":{"bad":{"kind":"Movie","tmdb_id":1,"title":"x"}}}"#)
                .contains("Fix Match")
        );
        assert!(err(r#"{"marquee_export":1,"rules":"x"}"#).contains("damaged"));
    }

    #[test]
    fn merge_combines_and_replace_resets() {
        let cur = sample();
        let file = r#"{"marquee_export":1,"platform":"elsewhere",
            "config":{"hide_titles":["B"],"lowercase":true,"ipc_socket":"/elsewhere.sock","vlc_password":"x"},
            "rules":[{"player":"","folder":"/Movies\n","action":"never"}],
            "matches":{"file:/b.mkv":{"kind":"Movie","tmdb_id":2,"title":"Ronin"}},
            "artwork":{}}"#;
        let b = parse(file).unwrap();
        assert_eq!(b.rules[0].folder, "/Movies"); // control characters stripped

        let m = plan(&b, Mode::Merge, &cur).unwrap();
        assert_eq!(m.values["hide_titles"], json!(["A", "B"]));
        assert_eq!(m.values["lowercase"], json!(true));
        assert!(!m.values.contains_key("ipc_socket")); // other OS
        assert!(!m.values.contains_key("vlc_password"));
        assert!(!m.values.contains_key("idle_timeout")); // not in the file: untouched
        assert_eq!(m.values["rules"].as_array().unwrap().len(), 2);
        assert_eq!(m.matches.len(), 2);
        assert_eq!(m.artwork.len(), 1);

        let r = plan(&b, Mode::Replace, &cur).unwrap();
        assert_eq!(r.values["hide_titles"], json!(["B"]));
        assert_eq!(r.values["idle_timeout"], json!(15)); // back to the default
        assert!(!r.values.contains_key(SECRET)); // file has no key: keep ours
        assert!(!r.values.contains_key("vlc_password"));
        assert_eq!(r.values["rules"].as_array().unwrap().len(), 1);
        assert_eq!(r.matches.len(), 1);
        assert!(r.artwork.is_empty());

        let s = summarize(&b, Mode::Replace, &cur).unwrap();
        assert_eq!(s["tmdb_key"], "kept");
        assert_eq!(s["matches"]["removed"], 1);
        assert_eq!(s["artwork"]["removed"], 1);
        assert!(s["settings"].as_array().unwrap().contains(&json!("idle_timeout")));
    }

    /// Export, change things, import: real files under a throwaway HOME.
    /// `HOME=$(mktemp -d) cargo test backup -- --ignored` (it refuses to run against a real home).
    #[test]
    #[ignore]
    fn end_to_end_in_a_temp_home() {
        let home = crate::paths::home();
        assert!(
            home.starts_with(std::env::temp_dir()) || home.starts_with("/tmp") || home.starts_with("/private"),
            "set HOME to a temp folder"
        );
        let cfg = Config::load().unwrap(); // writes the template
        assert!(cfg.rules.is_empty());
        let mut v = Map::new();
        v.insert("tmdb_api_key".into(), json!("k-123"));
        v.insert("idle_timeout".into(), json!(30));
        v.insert("rules".into(), json!([{ "player": "VLC", "folder": "~/Kids", "action": "never" }]));
        config::persist_json(&v).unwrap();
        crate::artwork::Store::open().set("movie:1", Some("https://image.tmdb.org/a.jpg".into()));

        let path = export(false, "2026-11-18").unwrap();
        let again = export(false, "2026-11-18").unwrap();
        assert_ne!(path, again); // never overwrites
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("k-123"));

        // Change everything, then bring it back.
        let mut v = Map::new();
        v.insert("idle_timeout".into(), json!(0));
        v.insert("rules".into(), json!([]));
        config::persist_json(&v).unwrap();
        crate::artwork::Store::open().set("movie:1", None);
        let b = parse(&text).unwrap();
        let after = apply(&b, Mode::Replace).unwrap();
        assert_eq!(after.idle_timeout, 30);
        assert_eq!(after.rules.len(), 1);
        assert_eq!(after.tmdb_api_key, "k-123"); // not in the file: kept
        let art: BTreeMap<String, String> = read_map(&crate::paths::artwork_file());
        assert_eq!(art.len(), 1);
        // The config file is still the commented template, edited in place.
        let raw = std::fs::read_to_string(crate::paths::config_file()).unwrap();
        assert!(raw.starts_with("# Marquee configuration"));
        assert!(!crate::paths::config_file().with_extension("import.tmp").exists());

        // A newer file changes nothing.
        let before = std::fs::read(crate::paths::config_file()).unwrap();
        assert!(parse(&text.replace("\"marquee_export\": 1", "\"marquee_export\": 2")).is_err());
        assert_eq!(std::fs::read(crate::paths::config_file()).unwrap(), before);
    }

    #[test]
    fn write_all_is_all_or_nothing() {
        let dir = std::env::temp_dir().join(format!("marquee-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.json");
        std::fs::write(&a, b"old").unwrap();
        // The second target's parent is a file, so its temp file can't be written.
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, b"").unwrap();
        let bad = blocker.join("b.json");
        assert!(write_all(&[(a.clone(), b"new".to_vec()), (bad, b"x".to_vec())]).is_err());
        assert_eq!(std::fs::read(&a).unwrap(), b"old");
        assert!(!a.with_extension("import.tmp").exists());
        write_all(&[(a.clone(), b"new".to_vec())]).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
