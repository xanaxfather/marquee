//! Players without any API: read the file name from their window title.
//! Covers QuickLook (space-bar previews), PotPlayer, KMPlayer and GOM Player.
//! No pause or position information is available this way.

use super::{PlayerEvent, Reporter, Snapshot};
use crate::parse::{FileKind, file_kind};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};
use windows::core::{BOOL, PWSTR};

const POLL: Duration = Duration::from_secs(3);

struct Known {
    exe: &'static [&'static str],
    name: &'static str,
    /// Text before the file name ("QuickLook - Movie.mkv" in QuickLook 4+).
    prefixes: &'static [&'static str],
    suffixes: &'static [&'static str],
    /// The title must end in a video extension (QuickLook also previews documents).
    needs_extension: bool,
}

const KNOWN: &[Known] = &[
    Known {
        exe: &["quicklook.exe"],
        name: "QuickLook",
        prefixes: &["QuickLook - ", "QuickLook – ", "QuickLook — "],
        suffixes: &[" - QuickLook"],
        needs_extension: true,
    },
    Known {
        exe: &["potplayermini64.exe", "potplayermini.exe", "potplayer64.exe", "potplayer.exe"],
        name: "PotPlayer",
        prefixes: &[],
        suffixes: &[" - PotPlayer"],
        needs_extension: false,
    },
    Known {
        exe: &["kmplayer64x.exe", "kmplayer.exe"],
        name: "KMPlayer",
        prefixes: &[],
        suffixes: &[" - KMPlayer"],
        needs_extension: false,
    },
    Known {
        exe: &["gom64.exe", "gom.exe"],
        name: "GOM Player",
        prefixes: &[],
        suffixes: &[" - GOM Player"],
        needs_extension: false,
    },
];

/// File name shown in a known player's title, if any.
pub fn title_to_file(known_name: &str, title: &str) -> Option<String> {
    let k = KNOWN.iter().find(|k| k.name == known_name)?;
    let mut t = title.trim();
    for p in k.prefixes {
        if t.len() > p.len() && t.is_char_boundary(p.len()) && t[..p.len()].eq_ignore_ascii_case(p) {
            t = &t[p.len()..];
            break;
        }
    }
    for s in k.suffixes {
        t = t.strip_suffix(s).unwrap_or(t);
    }
    let t = t.trim();
    if t.is_empty() || t.eq_ignore_ascii_case(k.name) {
        return None;
    }
    if k.needs_extension && file_kind(t) != FileKind::Video {
        return None;
    }
    Some(t.to_string())
}

fn process_exe(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 520];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| {
            let full = String::from_utf16_lossy(&buf[..len as usize]);
            full.rsplit('\\').next().unwrap_or(&full).to_ascii_lowercase()
        })
    }
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let out = unsafe { &mut *(lparam.0 as *mut Vec<(String, String)>) };
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let Some(exe) = process_exe(pid) else { return true.into() };
        let Some(k) = KNOWN.iter().find(|k| k.exe.contains(&exe.as_str())) else { return true.into() };
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        out.push((k.name.to_string(), String::from_utf16_lossy(&buf[..n as usize])));
    }
    true.into()
}

pub fn spawn(reporter: Reporter) {
    std::thread::Builder::new()
        .name("watch-windows".into())
        .spawn(move || run(&reporter))
        .expect("spawn window-title watcher");
}

fn run(reporter: &Reporter) {
    let mut current: Option<(String, String)> = None; // (player, file)
    loop {
        let mut windows: Vec<(String, String)> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect), LPARAM(&mut windows as *mut _ as isize));
        }
        let found = windows.iter().find_map(|(name, title)| title_to_file(name, title).map(|f| (name.clone(), f)));
        if found != current {
            match &found {
                Some((player, file)) => {
                    if current.is_none() && !reporter.send(PlayerEvent::Connected) {
                        return;
                    }
                    let snap = Snapshot {
                        player: player.clone(),
                        path: Some(file.clone()),
                        media_title: Some(file.clone()),
                        position: None,
                        position_at: Instant::now(),
                        idle: false,
                        ..Default::default()
                    };
                    if !reporter.send(PlayerEvent::Update(snap)) {
                        return;
                    }
                }
                None => {
                    if !reporter.send(PlayerEvent::Disconnected) {
                        return;
                    }
                }
            }
            current = found;
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles() {
        assert_eq!(title_to_file("PotPlayer", "Heat (1995).mkv - PotPlayer").as_deref(), Some("Heat (1995).mkv"));
        assert_eq!(title_to_file("PotPlayer", "PotPlayer"), None);
        assert_eq!(title_to_file("QuickLook", "Breaking.Bad.S02E03.mkv").as_deref(), Some("Breaking.Bad.S02E03.mkv"));
        assert_eq!(title_to_file("QuickLook", "report.pdf"), None);
        assert_eq!(
            title_to_file(
                "QuickLook",
                "QuickLook - Slasher (2016) - S01E08 - Soon Your Own Eyes Will See (1080p BluRay x265 RCVR).mkv"
            )
            .as_deref(),
            Some("Slasher (2016) - S01E08 - Soon Your Own Eyes Will See (1080p BluRay x265 RCVR).mkv")
        );
        assert_eq!(title_to_file("QuickLook", "QuickLook - report.pdf"), None);
        assert_eq!(title_to_file("QuickLook", "QuickLook"), None);
    }
}
