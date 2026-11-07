//! OS integration: opening files/URLs, notifications, launch at login,
//! console attachment and migrating settings from the old app name.

use crate::paths;
use std::path::Path;

/// Open a folder or file with the default handler; `text` forces a text editor.
pub fn open_path(path: &Path, text: bool) {
    #[cfg(target_os = "macos")]
    let result = {
        let mut cmd = std::process::Command::new("/usr/bin/open");
        if text {
            cmd.arg("-t");
        }
        cmd.arg(path).spawn()
    };
    #[cfg(windows)]
    let result = if text {
        std::process::Command::new("notepad.exe").arg(path).spawn()
    } else {
        std::process::Command::new("explorer.exe").arg(path).spawn()
    };
    #[cfg(not(any(target_os = "macos", windows)))]
    let result = {
        let _ = text;
        std::process::Command::new("xdg-open").arg(path).spawn()
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, path = %path.display(), "could not open");
    }
}

/// Open an https URL in the default browser.
pub fn open_url(url: &str) {
    if !url.starts_with("https://") {
        return;
    }
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("/usr/bin/open").arg(url).spawn();
    #[cfg(windows)]
    let result = std::process::Command::new("explorer.exe").arg(url).spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = result {
        tracing::warn!(error = %e, url, "could not open URL");
    }
}

/// Tell the user something (best effort, never blocks the caller).
pub fn notify(message: &str) {
    tracing::info!(message, "notification");
    let message = &crate::style::text(message);
    let app_name = crate::style::app_name();
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification {} with title \"{}\"",
            serde_json::to_string(message).unwrap_or_default(),
            app_name
        );
        let _ = std::process::Command::new("/usr/bin/osascript").args(["-e", &script]).spawn();
    }
    #[cfg(windows)]
    {
        // Only sent in response to a menu action, so a small dialog is appropriate.
        let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
        let title: Vec<u16> = app_name.encode_utf16().chain(std::iter::once(0)).collect();
        std::thread::spawn(move || unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MessageBoxW};
            use windows::core::PCWSTR;
            MessageBoxW(
                None,
                PCWSTR(text.as_ptr()),
                PCWSTR(title.as_ptr()),
                MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
            );
        });
    }
}

// ---------------------------------------------------------------- login item

#[cfg(target_os = "macos")]
fn launch_agent_file() -> std::path::PathBuf {
    paths::home().join("Library/LaunchAgents").join(format!("{}.plist", paths::BUNDLE_ID))
}

pub fn launch_at_login_enabled() -> bool {
    #[cfg(target_os = "macos")]
    {
        launch_agent_file().exists()
    }
    #[cfg(windows)]
    {
        winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
            .and_then(|k| k.get_value::<String, _>(paths::APP_NAME))
            .is_ok()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        false
    }
}

pub fn set_launch_at_login(on: bool) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    #[cfg(target_os = "macos")]
    {
        let file = launch_agent_file();
        if !on {
            if file.exists() {
                std::fs::remove_file(&file)?;
            }
        } else {
            let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
            let plist = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
</dict>
</plist>
"#,
                label = paths::BUNDLE_ID,
                exe = esc(&exe.to_string_lossy()),
            );
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&file, plist)?;
        }
    }
    #[cfg(windows)]
    {
        let (run, _) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")?;
        if on {
            run.set_value(paths::APP_NAME, &format!("\"{}\"", exe.display()))?;
        } else {
            let _ = run.delete_value(paths::APP_NAME);
        }
    }
    tracing::info!(enabled = on, exe = %exe.display(), "launch at login changed");
    Ok(())
}

// ---------------------------------------------------------------- misc

/// The Windows build is a GUI app (no console window). When it is started
/// from a terminal with arguments, re-attach so `--diagnose` etc. can print.
pub fn attach_console() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Move settings from the old "IINA Presence" name (first launch after the rename).
pub fn migrate_legacy() -> Vec<String> {
    #[allow(unused_mut)]
    let mut notes = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let home = paths::home();
        let moves = [
            (home.join("Library/Application Support/IINA Presence"), paths::support_dir()),
            (home.join("Library/Caches/IINA Presence"), paths::cache_dir()),
        ];
        for (old, new) in moves {
            if old.exists() && !new.exists() {
                match std::fs::rename(&old, &new) {
                    Ok(()) => notes.push(format!("moved {} → {}", old.display(), new.display())),
                    Err(e) => notes.push(format!("could not move {}: {e}", old.display())),
                }
            }
        }
        let old_agent = home.join("Library/LaunchAgents/io.github.iina-presence.plist");
        if old_agent.exists() {
            let _ = std::fs::remove_file(&old_agent);
            match set_launch_at_login(true) {
                Ok(()) => notes.push("re-created the launch-at-login item under the new name".into()),
                Err(e) => notes.push(format!("could not re-create launch at login: {e}")),
            }
        }
    }
    notes
}
