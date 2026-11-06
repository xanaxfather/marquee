//! VLC through its built-in web interface (`/requests/status.json`).
//!
//! VLC has no push API, so this is a light poll: a localhost request every
//! 2 s while VLC is open, every 5 s (a refused connection, microseconds)
//! while it isn't. "Set Up Players…" turns the interface on.

use super::{PlayerEvent, Reporter, Snapshot};
use serde_json::Value;
use std::sync::RwLock;
use std::time::{Duration, Instant};

const POLL_ACTIVE: Duration = Duration::from_secs(2);
const POLL_IDLE: Duration = Duration::from_secs(3);
const POLL_ABSENT: Duration = Duration::from_secs(5);

/// Port and password, updatable at runtime (setup / config reload).
static AUTH: RwLock<(u16, String)> = RwLock::new((8080, String::new()));

pub fn set_auth(port: u16, password: &str) {
    if let Ok(mut a) = AUTH.write() {
        *a = (port, password.to_string());
    }
}

fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `file:///C:/Movies/x.mkv` → `C:/Movies/x.mkv`, `file:///Users/x.mkv` → `/Users/x.mkv`.
pub fn uri_to_path(uri: &str) -> String {
    match uri.strip_prefix("file://") {
        Some(rest) => {
            let p = percent_decode(rest);
            let b = p.as_bytes();
            // Windows drive: "/C:/..." → "C:/..."
            if b.len() > 3 && b[0] == b'/' && b[2] == b':' { p[1..].to_string() } else { p }
        }
        None => uri.to_string(),
    }
}

fn find_current(node: &Value) -> Option<String> {
    if node["current"].as_str() == Some("current") {
        return node["uri"].as_str().map(String::from);
    }
    node["children"].as_array()?.iter().find_map(find_current)
}

pub fn spawn(reporter: Reporter, port: u16, password: String) {
    set_auth(port, &password);
    std::thread::Builder::new().name("watch-vlc".into()).spawn(move || run(&reporter)).expect("spawn vlc watcher");
}

enum Poll {
    Absent,
    Unauthorized,
    Status(Value),
}

fn get(agent: &ureq::Agent, port: u16, auth: &str, path: &str) -> Poll {
    match agent.get(&format!("http://127.0.0.1:{port}{path}")).set("Authorization", auth).call() {
        Ok(r) => r.into_json::<Value>().map(Poll::Status).unwrap_or(Poll::Absent),
        Err(ureq::Error::Status(401, _)) => Poll::Unauthorized,
        Err(_) => Poll::Absent,
    }
}

fn run(reporter: &Reporter) {
    let agent =
        ureq::AgentBuilder::new().timeout_connect(Duration::from_millis(500)).timeout(Duration::from_secs(2)).build();
    let mut connected = false;
    let mut warned_auth = false;
    let mut last_sent: Option<Snapshot> = None;
    let mut known_path: Option<(String, String)> = None; // (filename, full path)

    loop {
        let (port, password) = AUTH.read().map(|a| a.clone()).unwrap_or((8080, String::new()));
        let auth = format!("Basic {}", base64(format!(":{password}").as_bytes()));
        let poll = if password.is_empty() { Poll::Absent } else { get(&agent, port, &auth, "/requests/status.json") };
        let delay = match poll {
            Poll::Absent | Poll::Unauthorized => {
                if matches!(poll, Poll::Unauthorized) && !warned_auth {
                    tracing::warn!("VLC web interface rejected the password; run \"Set Up Players…\" again");
                    warned_auth = true;
                }
                if connected {
                    connected = false;
                    last_sent = None;
                    tracing::info!("VLC closed");
                    if !reporter.send(PlayerEvent::Disconnected) {
                        return;
                    }
                }
                POLL_ABSENT
            }
            Poll::Status(v) => {
                warned_auth = false;
                if !connected {
                    connected = true;
                    tracing::info!(port, "VLC connected");
                    if !reporter.send(PlayerEvent::Connected) {
                        return;
                    }
                }
                let state = v["state"].as_str().unwrap_or("stopped");
                let meta = &v["information"]["category"]["meta"];
                let filename = meta["filename"].as_str().map(String::from);
                // Resolve the full path once per file (folder names help matching).
                let path = match (&filename, &known_path) {
                    (Some(f), Some((kf, p))) if f == kf => Some(p.clone()),
                    (Some(f), _) => {
                        let full = match get(&agent, port, &auth, "/requests/playlist.json") {
                            Poll::Status(pl) => find_current(&pl).map(|u| uri_to_path(&u)),
                            _ => None,
                        }
                        .unwrap_or_else(|| f.clone());
                        known_path = Some((f.clone(), full.clone()));
                        Some(full)
                    }
                    (None, _) => None,
                };
                let snap = Snapshot {
                    player: "VLC".into(),
                    path: path.filter(|_| state != "stopped"),
                    media_title: meta["title"].as_str().map(String::from),
                    duration: v["length"].as_f64().filter(|l| *l > 0.0),
                    position: v["time"].as_f64(),
                    position_at: Instant::now(),
                    paused: state == "paused",
                    buffering: false,
                    speed: v["rate"].as_f64().filter(|r| *r > 0.0).unwrap_or(1.0),
                    idle: state == "stopped",
                };
                let delay = if state == "playing" { POLL_ACTIVE } else { POLL_IDLE };
                if last_sent.as_ref().is_none_or(|p| !p.same_state(&snap)) {
                    last_sent = Some(snap.clone());
                    if !reporter.send(PlayerEvent::Update(snap)) {
                        return;
                    }
                }
                delay
            }
        };
        std::thread::sleep(delay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(base64(b":secret"), "OnNlY3JldA==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(uri_to_path("file:///Users/me/Breaking%20Bad/S01E01.mkv"), "/Users/me/Breaking Bad/S01E01.mkv");
        assert_eq!(uri_to_path("file:///C:/Movies/Heat%20(1995).mkv"), "C:/Movies/Heat (1995).mkv");
        let pl: Value = serde_json::from_str(
            r#"{"children":[{"children":[{"name":"a","uri":"file:///a.mkv"},{"current":"current","uri":"file:///b%20c.mkv"}]}]}"#,
        )
        .unwrap();
        assert_eq!(find_current(&pl).as_deref(), Some("file:///b%20c.mkv"));
    }
}
