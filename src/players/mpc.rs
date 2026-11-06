//! MPC-HC / MPC-BE through their web interface (`/variables.html`).

use super::{PlayerEvent, Reporter, Snapshot};
use std::time::{Duration, Instant};

const POLL_ACTIVE: Duration = Duration::from_secs(2);
const POLL_ABSENT: Duration = Duration::from_secs(5);

pub fn spawn(reporter: Reporter, port: u16) {
    std::thread::Builder::new()
        .name("watch-mpc".into())
        .spawn(move || run(&reporter, port))
        .expect("spawn mpc watcher");
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Text of `<p id="name">…</p>`.
pub fn field(html: &str, id: &str) -> Option<String> {
    let start = html.find(&format!("id=\"{id}\">"))? + id.len() + 6;
    let end = html[start..].find('<')? + start;
    Some(unescape(html[start..end].trim()))
}

pub fn parse(html: &str) -> Snapshot {
    // state: -1 = nothing loaded, 0 = stopped, 1 = paused, 2 = playing
    let state: i32 = field(html, "state").and_then(|s| s.parse().ok()).unwrap_or(-1);
    let ms = |id| field(html, id).and_then(|s| s.parse::<f64>().ok()).map(|v| v / 1000.0);
    let path = field(html, "filepath").filter(|p| !p.is_empty());
    let player = match field(html, "version") {
        Some(v) if v.to_ascii_uppercase().contains("BE") => "MPC-BE",
        _ => "MPC-HC",
    };
    Snapshot {
        player: player.into(),
        path: path.filter(|_| state >= 1),
        media_title: field(html, "file"),
        duration: ms("duration").filter(|d| *d > 0.0),
        position: ms("position"),
        position_at: Instant::now(),
        paused: state == 1,
        buffering: false,
        speed: field(html, "playbackrate").and_then(|s| s.parse().ok()).filter(|r: &f64| *r > 0.0).unwrap_or(1.0),
        idle: state < 1,
    }
}

fn run(reporter: &Reporter, port: u16) {
    let agent =
        ureq::AgentBuilder::new().timeout_connect(Duration::from_millis(500)).timeout(Duration::from_secs(2)).build();
    let url = format!("http://127.0.0.1:{port}/variables.html");
    let mut connected = false;
    let mut last_sent: Option<Snapshot> = None;
    loop {
        let delay = match agent.get(&url).call().ok().and_then(|r| r.into_string().ok()) {
            Some(html) => {
                if !connected {
                    connected = true;
                    tracing::info!(port, "MPC connected");
                    if !reporter.send(PlayerEvent::Connected) {
                        return;
                    }
                }
                let snap = parse(&html);
                if last_sent.as_ref().is_none_or(|p| !p.same_state(&snap)) {
                    last_sent = Some(snap.clone());
                    if !reporter.send(PlayerEvent::Update(snap)) {
                        return;
                    }
                }
                POLL_ACTIVE
            }
            None => {
                if connected {
                    connected = false;
                    last_sent = None;
                    tracing::info!("MPC closed");
                    if !reporter.send(PlayerEvent::Disconnected) {
                        return;
                    }
                }
                POLL_ABSENT
            }
        };
        std::thread::sleep(delay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_variables_page() {
        let html = r#"<html><body><p id="file">Heat (1995).mkv</p><p id="filepath">C:\Movies\Heat (1995).mkv</p>
            <p id="state">2</p><p id="position">61000</p><p id="duration">10260000</p>
            <p id="playbackrate">1</p><p id="version">2.1.0</p></body></html>"#;
        let s = parse(html);
        assert_eq!(s.path.as_deref(), Some(r"C:\Movies\Heat (1995).mkv"));
        assert_eq!((s.paused, s.idle), (false, false));
        assert_eq!(s.position, Some(61.0));
        assert_eq!(s.duration, Some(10260.0));
        assert_eq!(s.player, "MPC-HC");
    }
}
