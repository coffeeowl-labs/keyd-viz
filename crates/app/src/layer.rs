//! Live layer state from `keyd listen` (the I/O half).
//!
//! The stream format, parser, and the active-layer reducer now live in
//! [`keydviz_core::live`] so the helper daemon shares identical parsing; this module
//! keeps only the process-spawning loop and the [`LiveState`] the UI consumes.
//!
//! Access requires membership in the `keyd` group (the socket is `root:keyd 0660`).
//! That socket exposes only layer *names*, never keystrokes — so it's low-risk,
//! unlike `/dev/input`. The shipped path routes this through the privileged helper so
//! even that group isn't required; the parser is source-agnostic and unchanged.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

use keydviz_core::live::{parse_listen_line, ActiveLayers};

/// The live state pushed to the UI on each update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveState {
    /// Whether we're currently connected to the `keyd listen` stream.
    pub connected: bool,
    /// Active layers in activation order (most recent last); empty = base.
    pub active: Vec<String>,
}

/// Run `keyd listen` and invoke `on_update` with each new [`LiveState`]. Blocks
/// forever (retries every few seconds), so call it from a background thread. State
/// resets on every (re)connect, since keyd replays a fresh snapshot.
pub fn run_listen(mut on_update: impl FnMut(LiveState)) {
    loop {
        match Command::new("keyd")
            .arg("listen")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(out) = child.stdout.take() {
                    let mut active = ActiveLayers::default();
                    on_update(LiveState { connected: true, active: Vec::new() });
                    for line in BufReader::new(out).lines().map_while(Result::ok) {
                        if let Some(ev) = parse_listen_line(&line) {
                            active.apply(&ev);
                            on_update(LiveState { connected: true, active: active.active() });
                        }
                    }
                }
                let _ = child.wait();
            }
            Err(_) => { /* keyd not found / not spawnable */ }
        }
        // Stream ended or failed to start: mark offline, then retry.
        on_update(LiveState { connected: false, active: Vec::new() });
        std::thread::sleep(Duration::from_secs(3));
    }
}
