//! Unbuffered, deadline-aware fd I/O for the apply protocol.
//!
//! Two properties the std primitives don't give us, both load-bearing for the
//! dead-man's switch (review findings, 2026-06-06):
//!
//! - **Reads have a deadline.** A stalled or malicious client that sends a partial
//!   request/payload and holds the pipe open must not wedge a root process forever:
//!   [`FdReader`] polls before every read and fails with `TimedOut` past its
//!   deadline. (The dead-man wait itself has its own polling in [`crate::deadman`].)
//! - **Protocol writes never panic.** `println!` panics on `EPIPE` — and the GUI
//!   being dead (closed pipe) is *exactly* the dead-man scenario, so a panic there
//!   would unwind past the revert. [`say`] writes best-effort and swallows every
//!   error: the GUI that would have read the line is gone; the revert must proceed.
//!
//! Everything reads/writes raw fds (no std buffering): the request line, payload,
//! and the later `keep` line must all come through ONE unbuffered reader, or a
//! buffered wrapper could slurp the `keep` away from the dead-man's `poll`.

use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::time::{Duration, Instant};

/// Unbuffered reader on an inherited fd with an absolute deadline.
pub struct FdReader {
    fd: RawFd,
    deadline: Instant,
}

impl FdReader {
    pub fn new(fd: RawFd, timeout: Duration) -> FdReader {
        FdReader { fd, deadline: Instant::now() + timeout }
    }
}

impl AsRawFd for FdReader {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl Read for FdReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "request deadline passed"));
            }
            let mut pfd = libc::pollfd { fd: self.fd, events: libc::POLLIN, revents: 0 };
            let ms = left.as_millis().min(i32::MAX as u128) as libc::c_int;
            match unsafe { libc::poll(&mut pfd, 1, ms) } {
                0 => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "request deadline passed",
                    ))
                }
                r if r < 0 => {
                    let e = io::Error::last_os_error();
                    if e.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(e);
                }
                _ => {}
            }
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}

/// Best-effort protocol line to stdout. Never panics, never errors — if the peer
/// is gone (`EPIPE`), the message simply has no reader, and the caller's revert
/// logic must keep going regardless.
pub fn say(line: &str) {
    write_line(libc::STDOUT_FILENO, line);
}

/// Best-effort `line + \n` to an fd, swallowing every error (testable core of
/// [`say`]).
pub fn write_line(fd: RawFd, line: &str) {
    let mut buf = Vec::with_capacity(line.len() + 1);
    buf.extend_from_slice(line.as_bytes());
    buf.push(b'\n');
    let mut off = 0;
    while off < buf.len() {
        let n = unsafe { libc::write(fd, buf[off..].as_ptr().cast(), buf.len() - off) };
        if n > 0 {
            off += n as usize;
            continue;
        }
        let e = io::Error::last_os_error();
        if e.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return; // EPIPE, EBADF, … — peer is gone; nothing useful to do.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::{FromRawFd, IntoRawFd};

    fn pipe() -> (File, File) {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
    }

    #[test]
    fn read_times_out_on_silent_pipe() {
        let (r, _w) = pipe(); // writer held open: no EOF, no data
        let mut rd = FdReader::new(r.as_raw_fd(), Duration::from_millis(50));
        let mut buf = [0u8; 4];
        let err = rd.read_exact(&mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn read_delivers_data_within_deadline() {
        let (r, mut w) = pipe();
        w.write_all(b"abcd").unwrap();
        let mut rd = FdReader::new(r.as_raw_fd(), Duration::from_secs(5));
        let mut buf = [0u8; 4];
        rd.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"abcd");
    }

    #[test]
    fn partial_payload_then_stall_times_out() {
        // The F2 scenario: client claims more bytes than it sends, keeps pipe open.
        let (r, mut w) = pipe();
        w.write_all(b"ab").unwrap();
        let mut rd = FdReader::new(r.as_raw_fd(), Duration::from_millis(50));
        let mut buf = [0u8; 8];
        let err = rd.read_exact(&mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn write_line_survives_closed_pipe() {
        // The F1 scenario: the reader is gone; the write must neither panic nor
        // error — the caller's revert continues.
        let (r, w) = pipe();
        drop(r);
        let wfd = w.into_raw_fd();
        write_line(wfd, "applied 20"); // EPIPE swallowed
        write_line(wfd, "reverted Eof");
        unsafe { libc::close(wfd) };
    }

    #[test]
    fn write_line_delivers_when_pipe_is_open() {
        let (mut r, w) = pipe();
        write_line(w.as_raw_fd(), "kept");
        drop(w);
        let mut s = String::new();
        r.read_to_string(&mut s).unwrap();
        assert_eq!(s, "kept\n");
    }
}
