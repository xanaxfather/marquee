//! Minimal Discord RPC client over Discord's local IPC endpoint: a Unix
//! socket (`$TMPDIR/discord-ipc-N`) on macOS, a named pipe
//! (`\\.\pipe\discord-ipc-N`) on Windows.
//!
//! Frame format: little-endian u32 opcode, u32 length, JSON payload.

use crate::ipc::LocalStream;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;
const MAX_FRAME: usize = 1 << 20;

#[derive(Debug)]
pub enum DiscordError {
    NotRunning,
    NoClientId,
    Io(std::io::Error),
    /// Discord closed the connection (e.g. 4000 = invalid client id).
    Closed {
        code: i64,
        message: String,
    },
    /// Discord answered the command with an error; the connection stays usable.
    Rejected(String),
    Protocol(String),
}

impl std::fmt::Display for DiscordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscordError::NotRunning => write!(f, "Discord is not running"),
            DiscordError::NoClientId => write!(f, "discord_client_id is not configured"),
            DiscordError::Io(e) => write!(f, "I/O error: {e}"),
            DiscordError::Closed { code, message } => write!(f, "Discord closed the connection ({code}: {message})"),
            DiscordError::Rejected(m) => write!(f, "Discord rejected the activity: {m}"),
            DiscordError::Protocol(m) => write!(f, "protocol error: {m}"),
        }
    }
}

impl From<std::io::Error> for DiscordError {
    fn from(e: std::io::Error) -> Self {
        DiscordError::Io(e)
    }
}

pub struct Discord {
    client_id: String,
    conn: Option<LocalStream>,
    nonce: u64,
    pub user: Option<String>,
    /// Name of the Discord application ("Watching <name>"), when Discord reports it.
    pub app_name: Option<String>,
}

#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Option<PathBuf> {
    let mut buf = vec![0u8; 1024];
    let n = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr().cast(), buf.len()) };
    if n == 0 || n > buf.len() {
        return None;
    }
    std::ffi::CStr::from_bytes_until_nul(&buf).ok().map(|s| PathBuf::from(s.to_string_lossy().into_owned()))
}

/// Every place a Discord client (stable, PTB, Canary) may listen.
#[cfg(unix)]
pub fn socket_candidates() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    for var in ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"] {
        if let Some(v) = std::env::var_os(var) {
            dirs.push(PathBuf::from(v));
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(d) = darwin_user_temp_dir() {
        dirs.push(d);
    }
    dirs.push(PathBuf::from("/tmp"));
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.canonicalize().unwrap_or_else(|_| d.clone())));
    dirs.iter().flat_map(|d| (0..10).map(move |i| d.join(format!("discord-ipc-{i}")))).filter(|p| p.exists()).collect()
}

#[cfg(windows)]
pub fn socket_candidates() -> Vec<PathBuf> {
    (0..10).map(|i| PathBuf::from(format!(r"\\.\pipe\discord-ipc-{i}"))).collect()
}

impl Discord {
    pub fn new(client_id: &str) -> Self {
        Self { client_id: client_id.to_string(), conn: None, nonce: 0, user: None, app_name: None }
    }

    pub fn is_connected(&self) -> bool {
        self.conn.is_some()
    }

    pub fn disconnect(&mut self) {
        if self.conn.take().is_some() {
            tracing::info!("disconnected from Discord");
        }
        self.user = None;
    }

    pub fn connect(&mut self) -> Result<(), DiscordError> {
        if self.client_id.is_empty() {
            return Err(DiscordError::NoClientId);
        }
        self.disconnect();
        let mut last = DiscordError::NotRunning;
        for path in socket_candidates() {
            let mut stream = match LocalStream::connect(&path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(socket = %path.display(), error = %e, "Discord socket not accepting");
                    if e.kind() != std::io::ErrorKind::NotFound {
                        last = DiscordError::Io(e);
                    }
                    continue;
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            self.conn = Some(stream);
            match self.handshake() {
                Ok(user) => {
                    tracing::info!(socket = %path.display(), user = user.as_deref().unwrap_or("?"), "connected to Discord");
                    self.user = user;
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(socket = %path.display(), error = %e, "Discord handshake failed");
                    self.conn = None;
                    // An invalid client id fails identically on every socket.
                    if matches!(e, DiscordError::Closed { .. }) {
                        return Err(e);
                    }
                    last = e;
                }
            }
        }
        Err(last)
    }

    fn handshake(&mut self) -> Result<Option<String>, DiscordError> {
        self.write_frame(OP_HANDSHAKE, &json!({"v": 1, "client_id": self.client_id}))?;
        loop {
            let (op, v) = self.read_frame()?;
            match op {
                OP_FRAME if v["evt"] == "READY" => {
                    self.app_name = v["data"]["application"]["name"].as_str().map(String::from);
                    let user = &v["data"]["user"];
                    let name = user["global_name"].as_str().or(user["username"].as_str()).map(String::from);
                    return Ok(name);
                }
                OP_FRAME if v["evt"] == "ERROR" => {
                    return Err(DiscordError::Rejected(v["data"]["message"].as_str().unwrap_or("").into()));
                }
                OP_CLOSE => return Err(closed(&v)),
                OP_PING => self.write_frame(OP_PONG, &v)?,
                _ => {}
            }
        }
    }

    /// Set (or with `None`, clear) the activity.
    pub fn set_activity(&mut self, activity: Option<&Value>) -> Result<(), DiscordError> {
        self.nonce += 1;
        let nonce = self.nonce.to_string();
        let payload = json!({
            "cmd": "SET_ACTIVITY",
            "args": {"pid": std::process::id(), "activity": activity},
            "nonce": nonce,
        });
        let result = self.write_frame(OP_FRAME, &payload).and_then(|_| {
            loop {
                let (op, v) = self.read_frame()?;
                match op {
                    OP_FRAME if v["nonce"].as_str() == Some(nonce.as_str()) => {
                        if v["evt"] == "ERROR" {
                            break Err(DiscordError::Rejected(
                                v["data"]["message"].as_str().unwrap_or("unknown error").into(),
                            ));
                        }
                        tracing::debug!(echo = %v["data"], "Discord accepted activity");
                        break Ok(());
                    }
                    OP_CLOSE => break Err(closed(&v)),
                    OP_PING => self.write_frame(OP_PONG, &v)?,
                    _ => {}
                }
            }
        });
        if let Err(e) = &result
            && !matches!(e, DiscordError::Rejected(_))
        {
            tracing::warn!(error = %e, "Discord connection broken");
            self.disconnect();
        }
        result
    }

    /// Cheap liveness check without blocking: has Discord hung up?
    pub fn check_alive(&mut self) -> bool {
        let alive = self.conn.as_ref().is_some_and(|c| c.is_alive());
        if !alive && self.conn.is_some() {
            tracing::info!("Discord went away");
            self.disconnect();
        }
        alive
    }

    fn write_frame(&mut self, op: u32, v: &Value) -> Result<(), DiscordError> {
        let conn = self.conn.as_mut().ok_or(DiscordError::NotRunning)?;
        let body = serde_json::to_vec(v).map_err(|e| DiscordError::Protocol(e.to_string()))?;
        let mut frame = Vec::with_capacity(8 + body.len());
        frame.extend_from_slice(&op.to_le_bytes());
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(&body);
        conn.write_all(&frame)?;
        Ok(())
    }

    fn read_frame(&mut self) -> Result<(u32, Value), DiscordError> {
        let conn = self.conn.as_mut().ok_or(DiscordError::NotRunning)?;
        let mut header = [0u8; 8];
        conn.read_exact(&mut header)?;
        let op = u32::from_le_bytes(header[..4].try_into().unwrap());
        let len = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        if len > MAX_FRAME {
            return Err(DiscordError::Protocol(format!("frame too large: {len}")));
        }
        let mut body = vec![0u8; len];
        conn.read_exact(&mut body)?;
        let v = serde_json::from_slice(&body).map_err(|e| DiscordError::Protocol(e.to_string()))?;
        Ok((op, v))
    }
}

fn closed(v: &Value) -> DiscordError {
    DiscordError::Closed {
        code: v["code"].as_i64().unwrap_or(0),
        message: v["message"].as_str().unwrap_or("").to_string(),
    }
}
