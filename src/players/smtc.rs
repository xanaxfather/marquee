//! Windows System Media Transport Controls — the media overlay Windows shows
//! for the volume keys. Windows Media Player, Movies & TV and many other apps
//! publish their title, playback state and timeline here.
//!
//! Only video sessions are used (music and browsers are ignored), and players
//! that have a richer dedicated watcher (VLC, mpv, MPC) are skipped here.

use super::{PlayerEvent, Reporter, Snapshot};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Media::MediaPlaybackType;

const POLL: Duration = Duration::from_secs(2);

/// Never used through SMTC: music apps, browsers, and players with their own watcher.
const ALWAYS_IGNORE: &[&str] = &[
    "spotify",
    "chrome",
    "msedge",
    "firefox",
    "brave",
    "opera",
    "vivaldi",
    "arc",
    "discord",
    "itunes",
    "applemusic",
    "tidal",
    "deezer",
    "vlc",
    "mpv",
    "mpc-hc",
    "mpc-be",
];

/// Apps that only play video, accepted even if they don't declare a playback type.
const VIDEO_APPS: &[&str] = &["zunevideo", "video.ui", "quicklook", "potplayer", "kmplayer", "gom"];

fn friendly_name(aumid: &str) -> String {
    let l = aumid.to_ascii_lowercase();
    if l.contains("zunevideo") || l.contains("video.ui") {
        return "Movies & TV".into();
    }
    if l.contains("zunemusic") || l.contains("media.player") {
        return "Media Player".into();
    }
    if l.contains("quicklook") {
        return "QuickLook".into();
    }
    if l.contains("potplayer") {
        return "PotPlayer".into();
    }
    // "C:\...\Foo.exe" / "Foo.exe" / "Publisher.App_hash!App"
    let base = aumid.rsplit(['\\', '/']).next().unwrap_or(aumid);
    let base = base.split('!').next().unwrap_or(base);
    let base = base.trim_end_matches(".exe").trim_end_matches(".EXE");
    base.split('_').next().unwrap_or(base).rsplit('.').next().unwrap_or(base).to_string()
}

fn filetime_now() -> i64 {
    // 100 ns ticks since 1601-01-01, like WinRT DateTime.
    let unix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    (unix.as_nanos() / 100) as i64 + 116_444_736_000_000_000
}

pub fn spawn(reporter: Reporter, extra_ignore: Vec<String>) {
    std::thread::Builder::new()
        .name("watch-smtc".into())
        .spawn(move || run(&reporter, &extra_ignore))
        .expect("spawn smtc watcher");
}

fn run(reporter: &Reporter, extra_ignore: &[String]) {
    unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
    }
    let manager = loop {
        match Manager::RequestAsync().and_then(|op| op.join()) {
            Ok(m) => break m,
            Err(e) => {
                tracing::warn!(error = %e, "media transport controls unavailable; retrying in 30 s");
                std::thread::sleep(Duration::from_secs(30));
            }
        }
    };
    let ignore: Vec<String> = ALWAYS_IGNORE
        .iter()
        .map(|s| s.to_string())
        .chain(extra_ignore.iter().map(|s| s.to_ascii_lowercase()))
        .collect();

    let mut connected = false;
    let mut last_sent: Option<Snapshot> = None;
    loop {
        let best = match pick(&manager, &ignore) {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!(error = %e, "SMTC query failed");
                None
            }
        };
        match best {
            Some(snap) => {
                if !connected {
                    connected = true;
                    tracing::info!(player = %snap.player, "media session detected");
                    if !reporter.send(PlayerEvent::Connected) {
                        return;
                    }
                }
                if last_sent.as_ref().is_none_or(|p| !p.same_state(&snap)) {
                    last_sent = Some(snap.clone());
                    if !reporter.send(PlayerEvent::Update(snap)) {
                        return;
                    }
                }
            }
            None if connected => {
                connected = false;
                last_sent = None;
                if !reporter.send(PlayerEvent::Disconnected) {
                    return;
                }
            }
            None => {}
        }
        std::thread::sleep(POLL);
    }
}

fn snapshot(session: &Session, ignore: &[String]) -> windows::core::Result<Option<(u8, Snapshot)>> {
    let app = session.SourceAppUserModelId()?.to_string();
    let lower = app.to_ascii_lowercase();
    if ignore.iter().any(|i| lower.contains(i.as_str())) {
        return Ok(None);
    }
    let props = session.TryGetMediaPropertiesAsync()?.join()?;
    let title = props.Title()?.to_string();
    if title.trim().is_empty() {
        return Ok(None);
    }
    let kind = props.PlaybackType().ok().and_then(|r| r.Value().ok());
    let is_video = match kind {
        Some(k) => k == MediaPlaybackType::Video,
        None => VIDEO_APPS.iter().any(|v| lower.contains(v)),
    };
    if !is_video {
        return Ok(None);
    }
    let status = session.GetPlaybackInfo()?.PlaybackStatus()?;
    let (paused, rank) = match status {
        Status::Playing => (false, 2),
        Status::Paused => (true, 1),
        _ => return Ok(None),
    };
    let tl = session.GetTimelineProperties()?;
    let start = tl.StartTime()?.Duration as f64 / 1e7;
    let end = tl.EndTime()?.Duration as f64 / 1e7;
    let pos = tl.Position()?.Duration as f64 / 1e7 - start;
    // The position is as of LastUpdatedTime; age it to "now".
    let age_ticks = (filetime_now() - tl.LastUpdatedTime()?.UniversalTime).max(0);
    let age = Duration::from_nanos((age_ticks as u64).saturating_mul(100)).min(Duration::from_secs(3600));
    let position_at = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
    let duration = (end - start > 1.0).then_some(end - start);
    Ok(Some((
        rank,
        Snapshot {
            player: friendly_name(&app),
            path: Some(title.clone()),
            media_title: Some(title),
            duration,
            position: duration.map(|_| pos.max(0.0)),
            position_at,
            paused,
            buffering: false,
            speed: 1.0,
            idle: false,
        },
    )))
}

fn pick(manager: &Manager, ignore: &[String]) -> windows::core::Result<Option<Snapshot>> {
    let sessions = manager.GetSessions()?;
    let mut best: Option<(u8, Snapshot)> = None;
    for session in sessions {
        if let Ok(Some((rank, snap))) = snapshot(&session, ignore)
            && best.as_ref().is_none_or(|(r, _)| rank > *r)
        {
            best = Some((rank, snap));
        }
    }
    Ok(best.map(|(_, s)| s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(friendly_name("Microsoft.ZuneVideo_8wekyb3d8bbwe!Microsoft.ZuneVideo"), "Movies & TV");
        assert_eq!(friendly_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "Media Player");
        assert_eq!(friendly_name(r"C:\Program Files\Foo\FooPlayer.exe"), "FooPlayer");
    }
}
