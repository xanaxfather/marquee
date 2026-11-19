//! One running copy at a time. The first launch holds a lock for its whole
//! life; a later launch asks it to show Settings, then exits. That also gives
//! people a way in when the menu bar icon is hidden: open Marquee again.
//!
//! macOS: `flock` on a file in Application Support, plus a Unix socket for the
//! "show yourself" nudge. Windows: a named mutex, plus a named event.

/// Held by the running copy.
pub struct Primary(imp::Primary);

/// `Some` if this is the only copy; `None` if another copy is running (it has
/// been told to show its Settings window).
pub fn acquire() -> Option<Primary> {
    imp::acquire().map(Primary)
}

impl Primary {
    /// Call `f` whenever another launch nudges this copy. Keeps the lock until
    /// the process exits.
    pub fn on_activate(self, f: impl Fn() + Send + 'static) {
        imp::listen(self.0, Box::new(f));
    }
}

#[cfg(unix)]
mod imp {
    use crate::paths;
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::time::Duration;

    pub struct Primary {
        lock: Option<File>,
        listener: Option<UnixListener>,
    }

    /// Unix socket paths are capped at 104 bytes on macOS; fall back to the
    /// (per-user) temp folder for a very long home path.
    fn socket() -> PathBuf {
        let p = paths::support_dir().join("marquee.sock");
        if p.as_os_str().len() < 100 { p } else { std::env::temp_dir().join("marquee.sock") }
    }

    pub fn acquire() -> Option<Primary> {
        let dir = paths::support_dir();
        let _ = std::fs::create_dir_all(&dir);
        let lock =
            match OpenOptions::new().create(true).truncate(false).read(true).write(true).open(dir.join("marquee.lock"))
            {
                Ok(f) => f,
                // Can't tell; better to run twice than not at all.
                Err(_) => return Some(Primary { lock: None, listener: None }),
            };
        // SAFETY: a valid open fd; flock has no other preconditions.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            if std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK) {
                nudge();
                return None;
            }
            return Some(Primary { lock: None, listener: None });
        }
        // We own the lock, so any socket file left behind is stale.
        let path = socket();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).ok();
        Some(Primary { lock: Some(lock), listener })
    }

    fn nudge() {
        if let Ok(mut s) = UnixStream::connect(socket()) {
            let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
            let _ = s.write_all(b"show\n");
        }
    }

    pub fn listen(p: Primary, f: Box<dyn Fn() + Send>) {
        let Some(listener) = p.listener else {
            std::mem::forget(p.lock);
            return;
        };
        let lock = p.lock;
        let spawned = std::thread::Builder::new().name("instance".into()).spawn(move || {
            let _lock = lock;
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut buf = [0u8; 16];
                if matches!(stream.read(&mut buf), Ok(n) if buf[..n].starts_with(b"show")) {
                    tracing::info!("another launch asked to show Marquee");
                    f();
                }
            }
        });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not listen for other launches");
        }
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent, WaitForSingleObject,
    };
    use windows::core::w;

    pub struct Primary {
        /// The "show yourself" event, as a raw handle value (HANDLE isn't Send).
        event: Option<isize>,
    }

    pub fn acquire() -> Option<Primary> {
        // SAFETY: plain Win32 calls with static, NUL-terminated names. The
        // mutex handle is deliberately never closed: it lives as long as we do.
        unsafe {
            let mutex = CreateMutexW(None, false, w!("Local\\Marquee.SingleInstance"));
            if mutex.is_ok() && GetLastError() == ERROR_ALREADY_EXISTS {
                if let Ok(ev) = OpenEventW(EVENT_MODIFY_STATE, false, w!("Local\\Marquee.Show")) {
                    let _ = SetEvent(ev);
                }
                return None;
            }
            let event = CreateEventW(None, false, false, w!("Local\\Marquee.Show")).ok().map(|h| h.0 as isize);
            Some(Primary { event })
        }
    }

    pub fn listen(p: Primary, f: Box<dyn Fn() + Send>) {
        let Some(raw) = p.event else { return };
        let spawned = std::thread::Builder::new().name("instance".into()).spawn(move || {
            let ev = HANDLE(raw as *mut _);
            // SAFETY: `ev` is the event created in `acquire`, never closed.
            while unsafe { WaitForSingleObject(ev, INFINITE) } == WAIT_OBJECT_0 {
                tracing::info!("another launch asked to show Marquee");
                f();
            }
        });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not listen for other launches");
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    pub struct Primary;
    pub fn acquire() -> Option<Primary> {
        Some(Primary)
    }
    pub fn listen(_: Primary, _: Box<dyn Fn() + Send>) {}
}
