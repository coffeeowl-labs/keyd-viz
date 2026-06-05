//! Client for the `keydviz-helperd` broker socket.
//!
//! When the helper daemon is running, the GUI gets its live signals from the helper's
//! [`LiveEvent`] stream over a unix socket instead of spawning `keyd` itself — the
//! shipped, zero-permission path (ROADMAP §1, `docs/helper-design.md`). This module is
//! the read-only client half: connect, parse one JSON event per line, and hand each
//! event to a callback. The GUI never writes to the socket.
//!
//! If the helper isn't present the app falls back to spawning `keyd listen`/`keyd
//! monitor` directly (see [`crate::layer`]/[`crate::monitor`]); both produce the same
//! [`keydviz_core::live`] types, so the UI wiring is identical either way.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use keydviz_core::live::LiveEvent;

/// Where the packaged system service binds its socket (its unit's `RuntimeDirectory=`
/// gives `keyd-viz` write access under `/run`). Must match `keydviz-helperd.service`.
const SYSTEM_SOCKET: &str = "/run/keyd-viz/keyd-viz.sock";

/// The helper socket path. In priority order: `$KEYDVIZ_HELPER_SOCKET`, else a *live*
/// per-user dev daemon at `$XDG_RUNTIME_DIR/keyd-viz.sock`, else the system service
/// socket [`SYSTEM_SOCKET`]. Mirrors the daemon so they meet with no config: a dev
/// `keydviz-helperd` (no flags) binds the per-user path; the installed service binds the
/// system path. "Live" means a daemon is actually listening (see [`is_live`]) — a stale
/// socket file from a crashed dev daemon is skipped rather than latched onto.
pub fn socket_path() -> String {
    resolve_socket(
        std::env::var("KEYDVIZ_HELPER_SOCKET").ok().as_deref(),
        std::env::var("XDG_RUNTIME_DIR").ok().as_deref(),
        is_live,
    )
}

/// Pure resolver behind [`socket_path`] (env + liveness probe injected for testability).
fn resolve_socket(helper_env: Option<&str>, xdg: Option<&str>, live: impl Fn(&str) -> bool) -> String {
    if let Some(p) = helper_env {
        if !p.is_empty() {
            return p.to_string();
        }
    }
    if let Some(dir) = xdg {
        if !dir.is_empty() {
            let user = format!("{dir}/keyd-viz.sock");
            if live(&user) {
                return user;
            }
        }
    }
    SYSTEM_SOCKET.to_string()
}

/// True if a daemon is actually *listening* on `socket` — tested by a real `connect()`.
/// This is what distinguishes a live broker from a stale socket *file* left behind by a
/// crashed daemon: the file still exists (so an existence check would be fooled) but
/// `connect()` fails with `ECONNREFUSED`. Used to choose broker-vs-direct so the GUI can't
/// latch onto a dead socket and spin forever instead of falling back. The probe
/// connection is immediately dropped.
pub fn is_live(socket: &str) -> bool {
    UnixStream::connect(socket).is_ok()
}

/// Connect to the helper and invoke `on_event` for each [`LiveEvent`]. `on_connect`
/// fires `true` when the stream opens and `false` when it drops. Blocks and retries
/// forever, so run it on a background thread (mirrors [`crate::layer::run_listen`]).
pub fn run_helper_client(
    socket: &str,
    mut on_connect: impl FnMut(bool),
    mut on_event: impl FnMut(LiveEvent),
) {
    loop {
        if let Ok(stream) = UnixStream::connect(socket) {
            on_connect(true);
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if let Some(ev) = LiveEvent::from_line(&line) {
                    on_event(ev);
                }
            }
        }
        on_connect(false);
        std::thread::sleep(Duration::from_secs(3));
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_socket, SYSTEM_SOCKET};

    #[test]
    fn socket_path_precedence() {
        let yes = |_: &str| true;
        let no = |_: &str| false;
        // explicit override always wins, regardless of liveness
        assert_eq!(resolve_socket(Some("/tmp/x.sock"), Some("/run/user/1000"), no), "/tmp/x.sock");
        // a *live* per-user dev socket is preferred
        assert_eq!(
            resolve_socket(None, Some("/run/user/1000"), yes),
            "/run/user/1000/keyd-viz.sock"
        );
        // a dead/stale per-user socket → fall through to the system service socket
        assert_eq!(resolve_socket(None, Some("/run/user/1000"), no), SYSTEM_SOCKET);
        // empty values are ignored, fall through to the system default
        assert_eq!(resolve_socket(Some(""), Some(""), no), SYSTEM_SOCKET);
        assert_eq!(resolve_socket(None, None, no), SYSTEM_SOCKET);
    }
}
