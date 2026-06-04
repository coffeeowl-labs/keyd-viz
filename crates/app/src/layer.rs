//! Live layer state from `keyd listen`.
//!
//! `keyd listen` streams newline-delimited layer transitions over keyd's socket:
//! `+name` (layer activated), `-name` (deactivated), `/name` (layout changed). On
//! connect it replays a snapshot of the current state. We parse that stream and
//! track which layer to highlight.
//!
//! Access requires membership in the `keyd` group (the socket is `root:keyd 0660`).
//! That socket exposes only layer *names*, never keystrokes — so it's low-risk,
//! unlike `/dev/input`. When access is unavailable the GUI shows "live view off".

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

/// A single layer transition from the `keyd listen` stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerEvent {
    On(String),
    Off(String),
    Layout(String),
}

/// Parse one line of `keyd listen` output. Returns `None` for blank/unknown lines.
pub fn parse_listen_line(line: &str) -> Option<LayerEvent> {
    let line = line.trim();
    let mut chars = line.chars();
    let kind = chars.next()?;
    let name = chars.as_str().trim().to_string();
    if name.is_empty() {
        return None;
    }
    match kind {
        '+' => Some(LayerEvent::On(name)),
        '-' => Some(LayerEvent::Off(name)),
        '/' => Some(LayerEvent::Layout(name)),
        _ => None,
    }
}

/// Tracks which layers are currently active (in activation order).
#[derive(Debug, Default, Clone)]
pub struct ActiveLayers {
    stack: Vec<String>,
    layout: String,
}

impl ActiveLayers {
    pub fn apply(&mut self, ev: &LayerEvent) {
        match ev {
            LayerEvent::On(n) => {
                if !self.stack.contains(n) {
                    self.stack.push(n.clone());
                }
            }
            LayerEvent::Off(n) => self.stack.retain(|x| x != n),
            LayerEvent::Layout(n) => self.layout = n.clone(),
        }
    }

    /// The active layers in activation order (most recent last). Lets the caller
    /// resolve the topmost layer that actually has a board to show.
    pub fn active(&self) -> Vec<String> {
        self.stack.clone()
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listen_lines() {
        assert_eq!(parse_listen_line("+nav"), Some(LayerEvent::On("nav".into())));
        assert_eq!(parse_listen_line("-nav"), Some(LayerEvent::Off("nav".into())));
        assert_eq!(parse_listen_line("/main"), Some(LayerEvent::Layout("main".into())));
        assert_eq!(parse_listen_line("  +sym  "), Some(LayerEvent::On("sym".into())));
        assert_eq!(parse_listen_line(""), None);
        assert_eq!(parse_listen_line("garbage"), None);
        assert_eq!(parse_listen_line("+"), None);
    }

    #[test]
    fn tracks_active_layer_stack() {
        let mut a = ActiveLayers::default();
        assert!(a.active().is_empty());
        a.apply(&LayerEvent::On("nav".into()));
        assert_eq!(a.active(), vec!["nav"]);
        a.apply(&LayerEvent::On("sym".into()));
        assert_eq!(a.active(), vec!["nav", "sym"]); // most recent last
        a.apply(&LayerEvent::Off("sym".into()));
        assert_eq!(a.active(), vec!["nav"]);
        a.apply(&LayerEvent::Off("nav".into()));
        assert!(a.active().is_empty()); // back to base
    }

    #[test]
    fn deduplicates_and_ignores_layout() {
        let mut a = ActiveLayers::default();
        a.apply(&LayerEvent::On("nav".into()));
        a.apply(&LayerEvent::On("nav".into())); // duplicate
        a.apply(&LayerEvent::Layout("main".into())); // no effect on the stack
        assert_eq!(a.active(), vec!["nav"]);
        a.apply(&LayerEvent::Off("nav".into()));
        assert!(a.active().is_empty());
    }
}
