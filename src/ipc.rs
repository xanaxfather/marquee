//! A local byte stream to another process: a Unix domain socket on macOS,
//! a named pipe (`\\.\pipe\...`) on Windows. Used for Discord and mpv.

use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

pub struct LocalStream {
    #[cfg(unix)]
    inner: std::os::unix::net::UnixStream,
    #[cfg(windows)]
    inner: std::fs::File,
    #[cfg(windows)]
    read_timeout: Option<Duration>,
}

impl LocalStream {
    pub fn connect(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self { inner: std::os::unix::net::UnixStream::connect(path)? })
        }
        #[cfg(windows)]
        {
            let inner = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
            Ok(Self { inner, read_timeout: None })
        }
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            inner: self.inner.try_clone()?,
            #[cfg(windows)]
            read_timeout: self.read_timeout,
        })
    }

    /// Reads fail with `TimedOut` after this long without data (`None` = block).
    pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.inner.set_read_timeout(timeout)
        }
        #[cfg(windows)]
        {
            self.read_timeout = timeout;
            Ok(())
        }
    }

    pub fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.inner.set_write_timeout(timeout)
        }
        #[cfg(windows)]
        {
            let _ = timeout; // pipe writes to a local server complete immediately
            Ok(())
        }
    }

    /// Non-blocking check whether the other side has hung up.
    pub fn is_alive(&self) -> bool {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let mut b = [0u8; 1];
            let n = unsafe {
                libc::recv(self.inner.as_raw_fd(), b.as_mut_ptr().cast(), 1, libc::MSG_PEEK | libc::MSG_DONTWAIT)
            };
            match n {
                0 => false,
                n if n > 0 => true,
                _ => io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock,
            }
        }
        #[cfg(windows)]
        {
            self.available().is_ok()
        }
    }

    #[cfg(windows)]
    fn available(&self) -> io::Result<u32> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Pipes::PeekNamedPipe;
        let mut avail = 0u32;
        unsafe { PeekNamedPipe(HANDLE(self.inner.as_raw_handle()), None, 0, None, Some(&mut avail), None) }
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(avail)
    }
}

impl Read for LocalStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        #[cfg(windows)]
        if let Some(timeout) = self.read_timeout {
            // Named pipes have no read timeout; poll for data instead.
            let deadline = std::time::Instant::now() + timeout;
            loop {
                if self.available()? > 0 {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "pipe read timed out"));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        self.inner.read(buf)
    }
}

impl Write for LocalStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
