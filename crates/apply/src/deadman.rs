//! The dead-man's switch (design doc §5.4).
//!
//! After write + reload, the privileged tool blocks waiting for a positive `keep`
//! line on its private fd (stdin of the pkexec invocation). **Confirm = keep;
//! anything else — timeout, EOF (GUI crash), garbage — reverts.** Revert authority
//! must live in this process: the unprivileged GUI cannot write `/etc/keyd`, and
//! "keep" is the action that requires working input, so its *absence* is the safe
//! state. (`keyd check` can't catch logical lockouts — a config that disables every
//! key is syntactically fine — which is exactly the case this switch exists for.)

use std::io::Read;
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

/// The verdict after waiting on the confirmation fd.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// A `keep` line arrived in time.
    Keep,
    /// The deadline passed with no (complete) line.
    TimedOut,
    /// The fd closed (caller died) before confirming.
    Eof,
    /// A complete line arrived but it wasn't `keep` — treated as a revert request.
    Refused,
}

/// Block until `keep\n` arrives on `fd`, the deadline passes, or the fd closes.
/// Only an exact `keep` line keeps; every other outcome is a revert.
pub fn await_keep(fd: &(impl AsRawFd + Read), timeout: Duration) -> Verdict {
    let deadline = Instant::now() + timeout;
    let raw = fd.as_raw_fd();
    let mut buf = Vec::new();

    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Verdict::TimedOut;
        }
        let mut pfd = libc::pollfd { fd: raw, events: libc::POLLIN, revents: 0 };
        let ms = left.as_millis().min(i32::MAX as u128) as libc::c_int;
        let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
        match rc {
            0 => return Verdict::TimedOut,
            r if r < 0 => {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue; // EINTR: re-poll with the remaining time
                }
                return Verdict::Eof; // unpollable fd: fail safe (revert)
            }
            _ => {}
        }

        let mut chunk = [0u8; 256];
        let n = unsafe { libc::read(raw, chunk.as_mut_ptr().cast(), chunk.len()) };
        match n {
            0 => return Verdict::Eof,
            n if n < 0 => {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Verdict::Eof;
            }
            n => buf.extend_from_slice(&chunk[..n as usize]),
        }

        if let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line = &buf[..pos];
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            return if line == b"keep" { Verdict::Keep } else { Verdict::Refused };
        }
        // Cap garbage accumulation: nothing legitimate sends more than one word.
        if buf.len() > 1024 {
            return Verdict::Refused;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use std::os::fd::FromRawFd;

    /// A unix pipe as (reader, writer) Files.
    fn pipe() -> (File, File) {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) }
    }

    #[test]
    fn keep_line_keeps() {
        let (r, mut w) = pipe();
        w.write_all(b"keep\n").unwrap();
        assert_eq!(await_keep(&r, Duration::from_secs(5)), Verdict::Keep);
    }

    #[test]
    fn crlf_keep_keeps() {
        let (r, mut w) = pipe();
        w.write_all(b"keep\r\n").unwrap();
        assert_eq!(await_keep(&r, Duration::from_secs(5)), Verdict::Keep);
    }

    #[test]
    fn timeout_reverts() {
        let (r, _w) = pipe();
        assert_eq!(await_keep(&r, Duration::from_millis(50)), Verdict::TimedOut);
    }

    #[test]
    fn eof_reverts() {
        let (r, w) = pipe();
        drop(w);
        assert_eq!(await_keep(&r, Duration::from_secs(5)), Verdict::Eof);
    }

    #[test]
    fn wrong_line_reverts() {
        let (r, mut w) = pipe();
        w.write_all(b"yes please\n").unwrap();
        assert_eq!(await_keep(&r, Duration::from_secs(5)), Verdict::Refused);
    }

    #[test]
    fn split_arrival_still_keeps() {
        let (r, mut w) = pipe();
        let t = std::thread::spawn(move || {
            w.write_all(b"ke").unwrap();
            std::thread::sleep(Duration::from_millis(30));
            w.write_all(b"ep\n").unwrap();
        });
        assert_eq!(await_keep(&r, Duration::from_secs(5)), Verdict::Keep);
        t.join().unwrap();
    }
}
