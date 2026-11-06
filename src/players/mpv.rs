//! IINA and mpv through mpv's JSON IPC (Unix socket / Windows named pipe).
//!
//! We subscribe to property changes (`observe_property`) so mpv *pushes*
//! state changes — no polling during playback. `time-pos` is deliberately
//! not observed (it changes every frame); it is fetched only on
//! pause/seek/speed changes, and Discord extrapolates progress itself.

use super::{PlayerEvent, Reporter, Snapshot};
use crate::ipc::LocalStream;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often to look for the socket while the player isn't running.
const RECONNECT_INTERVAL: Duration = Duration::from_secs(3);
const TIME_POS_REQUEST: u64 = 1000;

const OBSERVED: &[&str] = &["path", "pause", "duration", "speed", "idle-active", "media-title", "paused-for-cache"];
/// Properties that invalidate our idea of the playback position.
const RESYNC_ON: &[&str] = &["pause", "speed", "paused-for-cache", "duration"];

pub fn spawn(reporter: Reporter, socket: PathBuf) {
    std::thread::Builder::new()
        .name(format!("watch-{}", reporter.source))
        .spawn(move || run(&reporter, &socket))
        .expect("spawn mpv watcher");
}

fn run(reporter: &Reporter, socket: &Path) {
    let mut last_error: Option<std::io::ErrorKind> = None;
    loop {
        match LocalStream::connect(socket) {
            Ok(stream) => {
                last_error = None;
                tracing::info!(player = reporter.source, socket = %socket.display(), "player connected");
                if !reporter.send(PlayerEvent::Connected) {
                    return;
                }
                let started = Instant::now();
                match session(stream, reporter) {
                    Ok(()) => {
                        tracing::info!(player = reporter.source, secs = started.elapsed().as_secs(), "player closed")
                    }
                    Err(e) => tracing::warn!(player = reporter.source, error = %e, "player connection lost"),
                }
                if !reporter.send(PlayerEvent::Disconnected) {
                    return;
                }
            }
            Err(e) => {
                // Log state transitions only, not every retry.
                if last_error != Some(e.kind()) {
                    match e.kind() {
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                            tracing::debug!(player = reporter.source, socket = %socket.display(), "waiting for player")
                        }
                        _ => {
                            tracing::warn!(player = reporter.source, socket = %socket.display(), error = %e, "cannot connect to player")
                        }
                    }
                    last_error = Some(e.kind());
                }
            }
        }
        std::thread::sleep(RECONNECT_INTERVAL);
    }
}

fn send(w: &mut LocalStream, cmd: Value) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(&cmd)?;
    line.push(b'\n');
    w.write_all(&line)
}

fn request_time_pos(w: &mut LocalStream) -> std::io::Result<()> {
    send(w, json!({"command": ["get_property", "time-pos"], "request_id": TIME_POS_REQUEST}))
}

fn session(mut stream: LocalStream, reporter: &Reporter) -> std::io::Result<()> {
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut writer = stream.try_clone()?;
    for (i, prop) in OBSERVED.iter().enumerate() {
        send(&mut writer, json!({"command": ["observe_property", i + 1, prop]}))?;
    }
    request_time_pos(&mut writer)?;

    let mut state = Snapshot { player: reporter.source.to_string(), ..Default::default() };
    let mut last_sent: Option<Snapshot> = None;
    let mut reader = BufReader::with_capacity(16 * 1024, stream);
    let mut line = String::new();

    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(()); // EOF: player closed
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            tracing::debug!(line = line.trim(), "unparseable IPC line");
            continue;
        };

        if msg["request_id"].as_u64() == Some(TIME_POS_REQUEST) {
            state.position = msg["data"].as_f64();
            state.position_at = Instant::now();
        } else if let Some(event) = msg["event"].as_str() {
            match event {
                "property-change" => {
                    let name = msg["name"].as_str().unwrap_or("");
                    let data = &msg["data"];
                    match name {
                        "path" => {
                            let path = data.as_str().map(String::from);
                            if path != state.path {
                                state.position = None;
                                state.duration = None;
                            }
                            state.path = path;
                        }
                        "pause" => state.paused = data.as_bool().unwrap_or(false),
                        "duration" => state.duration = data.as_f64().filter(|d| *d > 0.0),
                        "speed" => state.speed = data.as_f64().filter(|s| *s > 0.0).unwrap_or(1.0),
                        "idle-active" => state.idle = data.as_bool().unwrap_or(false),
                        "media-title" => state.media_title = data.as_str().map(String::from),
                        "paused-for-cache" => state.buffering = data.as_bool().unwrap_or(false),
                        _ => {}
                    }
                    if RESYNC_ON.contains(&name) {
                        request_time_pos(&mut writer)?;
                    }
                }
                // Fired after seeks and when a new file starts rendering.
                "playback-restart" => request_time_pos(&mut writer)?,
                "shutdown" => return Ok(()),
                _ => {}
            }
        }

        if last_sent.as_ref().is_none_or(|prev| !prev.same_state(&state)) {
            last_sent = Some(state.clone());
            if !reporter.send(PlayerEvent::Update(state.clone())) {
                return Ok(());
            }
        }
    }
}
