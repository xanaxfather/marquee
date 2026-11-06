//! Add `input-ipc-server` to mpv's (and mpv.net's) `mpv.conf`.

use super::{get_kv, set_kv};
use crate::config::Config;
use std::path::PathBuf;

fn conf_files() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
        // mpv.net keeps its own folder; plain mpv uses %APPDATA%\mpv.
        let mut v = vec![appdata.join("mpv").join("mpv.conf")];
        if appdata.join("mpv.net").exists() {
            v.push(appdata.join("mpv.net").join("mpv.conf"));
        }
        v
    }
    #[cfg(not(windows))]
    {
        vec![crate::paths::home().join(".config/mpv/mpv.conf")]
    }
}

pub fn is_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        ["/Applications/mpv.app", "/opt/homebrew/bin/mpv", "/usr/local/bin/mpv"]
            .iter()
            .any(|p| std::path::Path::new(p).exists())
    }
    #[cfg(windows)]
    {
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
        appdata.join("mpv").exists() || appdata.join("mpv.net").exists()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        crate::paths::home().join(".config/mpv").exists()
    }
}

fn want(cfg: &Config) -> String {
    cfg.mpv_ipc.to_string_lossy().into_owned()
}

pub fn is_ready(cfg: &Config) -> bool {
    conf_files().iter().filter(|f| f.parent().is_some_and(|d| d.exists())).all(|f| {
        std::fs::read_to_string(f).ok().and_then(|t| get_kv(&t, "input-ipc-server")).as_deref()
            == Some(want(cfg).as_str())
    })
}

pub fn configure(cfg: &mut Config) -> anyhow::Result<()> {
    for file in conf_files() {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&file, set_kv(&text, "input-ipc-server", &want(cfg)))?;
    }
    Ok(())
}
