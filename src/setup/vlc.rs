//! Turn on VLC's web interface (localhost only, random password) in `vlcrc`.

use super::{get_kv, set_kv};
use crate::config::Config;
use std::path::PathBuf;

fn vlcrc() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        crate::paths::home().join("Library/Preferences/org.videolan.vlc/vlcrc")
    }
    #[cfg(windows)]
    {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default().join("vlc").join("vlcrc")
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        crate::paths::home().join(".config/vlc/vlcrc")
    }
}

pub fn is_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::path::Path::new("/Applications/VLC.app").exists()
            || crate::paths::home().join("Applications/VLC.app").exists()
    }
    #[cfg(windows)]
    {
        ["ProgramFiles", "ProgramFiles(x86)"]
            .iter()
            .any(|v| std::env::var_os(v).is_some_and(|p| PathBuf::from(p).join(r"VideoLAN\VLC\vlc.exe").exists()))
            || vlcrc().exists()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        vlcrc().exists()
    }
}

pub fn is_ready(cfg: &Config) -> bool {
    let Ok(text) = std::fs::read_to_string(vlcrc()) else { return false };
    let intf_ok = get_kv(&text, "extraintf").is_some_and(|v| v.split(':').any(|i| i == "http"));
    let pass_ok =
        !cfg.vlc_password.is_empty() && get_kv(&text, "http-password").as_deref() == Some(cfg.vlc_password.as_str());
    let port_ok = get_kv(&text, "http-port").map_or(cfg.vlc_port == 8080, |p| p == cfg.vlc_port.to_string());
    intf_ok && pass_ok && port_ok
}

/// A random password; only used between this app and VLC on localhost.
fn random_password() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut out = String::new();
    while out.len() < 20 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        h.write_u32(std::process::id());
        out.push_str(&format!("{:x}", h.finish()));
    }
    out.truncate(20);
    out
}

pub fn configure(cfg: &mut Config) -> anyhow::Result<()> {
    if cfg.vlc_password.is_empty() {
        cfg.vlc_password = random_password();
        crate::config::persist_value("vlc_password", &format!("\"{}\"", cfg.vlc_password))?;
    }
    let path = vlcrc();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "[core]\n".into());
    let intf = match get_kv(&text, "extraintf").filter(|v| !v.is_empty()) {
        Some(v) if v.split(':').any(|i| i == "http") => v,
        Some(v) => format!("{v}:http"),
        None => "http".into(),
    };
    let mut text = set_kv(&text, "extraintf", &intf);
    text = set_kv(&text, "http-host", "127.0.0.1");
    text = set_kv(&text, "http-port", &cfg.vlc_port.to_string());
    text = set_kv(&text, "http-password", &cfg.vlc_password);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, text)?;
    crate::players::vlc::set_auth(cfg.vlc_port, &cfg.vlc_password);
    Ok(())
}
