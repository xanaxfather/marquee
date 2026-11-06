//! Where the app keeps its files on each OS.
//!
//! | | macOS | Windows |
//! |---|---|---|
//! | config | `~/Library/Application Support/Marquee` | `%APPDATA%\Marquee` |
//! | cache | `~/Library/Caches/Marquee` | `%LOCALAPPDATA%\Marquee\Cache` |
//! | logs | `~/Library/Logs/Marquee` | `%LOCALAPPDATA%\Marquee\Logs` |

use std::path::PathBuf;

pub const APP_NAME: &str = "Marquee";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const BUNDLE_ID: &str = "io.github.xanaxfather.marquee";

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(target_os = "macos")]
mod dirs {
    use super::*;
    pub fn support() -> PathBuf {
        home().join("Library/Application Support").join(APP_NAME)
    }
    pub fn cache() -> PathBuf {
        home().join("Library/Caches").join(APP_NAME)
    }
    pub fn logs() -> PathBuf {
        home().join("Library/Logs").join(APP_NAME)
    }
}

#[cfg(windows)]
mod dirs {
    use super::*;
    fn env_dir(var: &str, fallback: &str) -> PathBuf {
        std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| home().join(fallback))
    }
    pub fn support() -> PathBuf {
        env_dir("APPDATA", "AppData/Roaming").join(APP_NAME)
    }
    fn local() -> PathBuf {
        env_dir("LOCALAPPDATA", "AppData/Local").join(APP_NAME)
    }
    pub fn cache() -> PathBuf {
        local().join("Cache")
    }
    pub fn logs() -> PathBuf {
        local().join("Logs")
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod dirs {
    use super::*;
    pub fn support() -> PathBuf {
        home().join(".config/marquee")
    }
    pub fn cache() -> PathBuf {
        home().join(".cache/marquee")
    }
    pub fn logs() -> PathBuf {
        cache().join("logs")
    }
}

pub fn support_dir() -> PathBuf {
    dirs::support()
}

pub fn cache_dir() -> PathBuf {
    dirs::cache()
}

pub fn log_dir() -> PathBuf {
    dirs::logs()
}

pub fn config_file() -> PathBuf {
    support_dir().join("config.toml")
}

pub fn cache_file() -> PathBuf {
    cache_dir().join("metadata.json")
}

pub fn artwork_file() -> PathBuf {
    support_dir().join("artwork.json")
}

/// WebView2 needs a writable profile folder on Windows.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn webview_dir() -> PathBuf {
    cache_dir().join("WebView")
}
