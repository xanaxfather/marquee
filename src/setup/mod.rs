//! "Set Up Players…": switch on the integration each installed player needs,
//! leaving the rest of its settings alone.

#[cfg(target_os = "macos")]
pub mod iina;
#[cfg(windows)]
pub mod mpc;
pub mod mpv;
pub mod vlc;

use crate::config::Config;

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerSetup {
    pub name: &'static str,
    pub installed: bool,
    pub ready: bool,
}

/// Which installed players still need setting up.
#[allow(clippy::vec_init_then_push)] // entries depend on the target OS
pub fn status(cfg: &Config) -> Vec<PlayerSetup> {
    let mut v = Vec::new();
    #[cfg(target_os = "macos")]
    v.push(PlayerSetup {
        name: "IINA",
        installed: iina::is_installed(),
        ready: iina::check(&cfg.ipc_socket) == iina::SetupState::Ready,
    });
    v.push(PlayerSetup { name: "VLC", installed: vlc::is_installed(), ready: vlc::is_ready(cfg) });
    v.push(PlayerSetup { name: "mpv", installed: mpv::is_installed(), ready: mpv::is_ready(cfg) });
    #[cfg(windows)]
    v.push(PlayerSetup { name: "MPC-HC", installed: mpc::is_installed(), ready: mpc::is_ready(cfg) });
    v
}

pub fn needing_setup(cfg: &Config) -> Vec<&'static str> {
    status(cfg).into_iter().filter(|p| p.installed && !p.ready).map(|p| p.name).collect()
}

/// Configure every installed player. May update `cfg` (e.g. the VLC password).
/// Returns (player, result) for each one touched.
pub fn configure_all(cfg: &mut Config) -> Vec<(&'static str, anyhow::Result<()>)> {
    let mut out = Vec::new();
    #[cfg(target_os = "macos")]
    if iina::is_installed() {
        out.push(("IINA", iina::configure(&cfg.ipc_socket)));
    }
    if vlc::is_installed() {
        out.push(("VLC", vlc::configure(cfg)));
    }
    if mpv::is_installed() {
        out.push(("mpv", mpv::configure(cfg)));
    }
    #[cfg(windows)]
    if mpc::is_installed() {
        out.push(("MPC-HC", mpc::configure(cfg)));
    }
    for (name, r) in &out {
        match r {
            Ok(()) => tracing::info!(player = name, "player set up"),
            Err(e) => tracing::error!(player = name, error = format!("{e:#}"), "player setup failed"),
        }
    }
    out
}

/// Replace `key=value` lines (also commented `#key=` defaults) or append one.
pub(crate) fn set_kv(text: &str, key: &str, value: &str) -> String {
    let mut found = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| {
            let t = line.trim_start().trim_start_matches('#').trim_start();
            let is_key = t.strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('='));
            if is_key && !found {
                found = true;
                format!("{key}={value}")
            } else {
                line.to_string()
            }
        })
        .collect();
    if !found {
        lines.push(format!("{key}={value}"));
    }
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

/// Value of an uncommented `key=value` line.
pub(crate) fn get_kv(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let t = line.trim();
        let rest = t.strip_prefix(key)?.trim_start().strip_prefix('=')?;
        Some(rest.trim().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_editing() {
        let vlcrc = "[core]\n# Extra interface modules (string)\n#extraintf=\n[lua]\n#http-password=\n";
        let out = set_kv(&set_kv(vlcrc, "extraintf", "http"), "http-password", "abc");
        assert!(out.contains("\nextraintf=http\n"));
        assert!(out.contains("\nhttp-password=abc\n"));
        assert_eq!(get_kv(&out, "extraintf").as_deref(), Some("http"));
        assert_eq!(get_kv("#extraintf=http\n", "extraintf"), None);
        let appended = set_kv("vo=gpu\n", "input-ipc-server", "/tmp/x.sock");
        assert_eq!(appended, "vo=gpu\ninput-ipc-server=/tmp/x.sock\n");
        // Doesn't confuse prefixes.
        assert_eq!(get_kv("http-password-x=1\nhttp-password=2\n", "http-password").as_deref(), Some("2"));
    }
}
