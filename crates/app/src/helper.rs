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

/// The helper socket path: `$KEYDVIZ_HELPER_SOCKET`, else `$XDG_RUNTIME_DIR/keyd-viz.sock`,
/// else `/run/keyd-viz.sock`. Mirrors the daemon's default so they meet with no config.
pub fn socket_path() -> String {
    resolve_socket(
        std::env::var("KEYDVIZ_HELPER_SOCKET").ok().as_deref(),
        std::env::var("XDG_RUNTIME_DIR").ok().as_deref(),
    )
}

/// Pure resolver behind [`socket_path`] (env read out for testability).
fn resolve_socket(helper_env: Option<&str>, xdg: Option<&str>) -> String {
    match helper_env {
        Some(p) if !p.is_empty() => return p.to_string(),
        _ => {}
    }
    match xdg {
        Some(dir) if !dir.is_empty() => format!("{dir}/keyd-viz.sock"),
        _ => "/run/keyd-viz.sock".to_string(),
    }
}

/// True if the helper socket exists — used to prefer the broker over direct `keyd`.
pub fn is_present(socket: &str) -> bool {
    std::fs::metadata(socket).is_ok()
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
    use super::resolve_socket;

    #[test]
    fn socket_path_precedence() {
        // explicit override wins
        assert_eq!(resolve_socket(Some("/tmp/x.sock"), Some("/run/user/1000")), "/tmp/x.sock");
        // else XDG_RUNTIME_DIR
        assert_eq!(resolve_socket(None, Some("/run/user/1000")), "/run/user/1000/keyd-viz.sock");
        // empty values are ignored, fall through to the system default
        assert_eq!(resolve_socket(Some(""), Some("")), "/run/keyd-viz.sock");
        assert_eq!(resolve_socket(None, None), "/run/keyd-viz.sock");
    }
}
