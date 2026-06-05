//! Live keypresses from `keyd monitor` (the I/O + view-transition half).
//!
//! The record format and parser now live in [`keydviz_core::live`] (shared with the
//! helper daemon); this module keeps the process-spawning loop and the
//! follow-keyboard / pressed-set transition the GUI needs. The moved event types are
//! re-exported so existing call sites keep resolving `monitor::MonitorEvent`, etc.
//!
//! Unlike `keyd listen` (layer names, `keyd` group), `keyd monitor` reads `/dev/input`
//! (typically the `input` group). The shipped product routes this through the
//! privileged helper so even that group isn't required (ROADMAP §1); the parser here is
//! source-agnostic and unchanged by that move.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

pub use keydviz_core::live::{parse_monitor_line, KeyAction, KeyEvent, MonitorEvent};

/// Run `keyd monitor` and invoke `on_event` for each parsed record. Blocks forever
/// (retries every few seconds on exit/failure), so call it from a background thread.
/// `on_connect(true)` fires when the stream opens, `on_connect(false)` when it drops —
/// mirroring [`crate::layer::run_listen`] so the UI can show a "live" indicator.
pub fn run_monitor(mut on_connect: impl FnMut(bool), mut on_event: impl FnMut(MonitorEvent)) {
    loop {
        match Command::new("keyd")
            .arg("monitor")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                if let Some(out) = child.stdout.take() {
                    on_connect(true);
                    for line in BufReader::new(out).lines().map_while(Result::ok) {
                        if let Some(ev) = parse_monitor_line(&line) {
                            on_event(ev);
                        }
                    }
                }
                let _ = child.wait();
            }
            Err(_) => { /* keyd not found / not spawnable */ }
        }
        on_connect(false);
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// The keypress-driven UI transition, factored out of the Slint layer so the
/// follow-keyboard + pressed-set logic is testable without a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Press {
    /// `Some(i)` if the shown sheet should switch to index `i` (the last-pressed
    /// keyboard changed); `None` to keep the current sheet.
    pub switch_to: Option<i32>,
    /// The full pressed-key set after this event (deduped, order-preserving).
    pub pressed: Vec<String>,
}

/// Decide how one key event changes the view, given the `vendor:product → sheet`
/// map, the currently shown sheet index, and the currently pressed keys.
///
/// keyd grabs configured keyboards (`EVIOCGRAB`) and re-emits *all* of them through a
/// single virtual device, so most real keystrokes arrive under that virtual id rather
/// than a physical one — i.e. not in the map. We attribute those to the board already
/// shown and glow without switching (keyd has discarded which physical keyboard the
/// keystroke came from, so following the last-pressed keyboard isn't possible from the
/// keypress stream). A device that *is* in the map (e.g. one keyd doesn't grab) selects
/// its sheet and can switch the view.
///
/// Pure: switching keyboards clears the glow (we don't know the new board's held keys);
/// down/repeat add a key (idempotent); up removes it.
pub fn next_press_state(
    ev: &KeyEvent,
    map: &[(String, i32)],
    active_idx: i32,
    current_pressed: &[String],
) -> Press {
    let (idx, switching) = match map.iter().find(|(d, _)| d == &ev.devid) {
        Some(&(_, i)) => (i, i != active_idx),
        None => (active_idx, false),
    };
    let mut pressed: Vec<String> =
        if switching { Vec::new() } else { current_pressed.to_vec() };
    match ev.action {
        KeyAction::Down | KeyAction::Repeat => {
            if !pressed.iter().any(|k| k == &ev.key) {
                pressed.push(ev.key.clone());
            }
        }
        KeyAction::Up => pressed.retain(|k| k != &ev.key),
    }
    Press { switch_to: switching.then_some(idx), pressed }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(devid: &str, key: &str, action: KeyAction) -> KeyEvent {
        KeyEvent { devid: devid.into(), device: "kb".into(), key: key.into(), action }
    }

    #[test]
    fn unmapped_device_glows_on_active_sheet() {
        let map = vec![("04fe:0021".to_string(), 0)];
        // keyd re-emits keystrokes through its virtual device (e.g. 0fac:0ade), which
        // isn't in the map → glow on the shown board, no switch (not ignored).
        assert_eq!(
            next_press_state(&ev("0fac:0ade", "a", KeyAction::Down), &map, 0, &[]),
            Press { switch_to: None, pressed: vec!["a".into()] }
        );
    }

    #[test]
    fn down_up_maintains_pressed_set() {
        let map = vec![("1:1".to_string(), 0)];
        let a = next_press_state(&ev("1:1", "a", KeyAction::Down), &map, 0, &[]);
        assert_eq!(a, Press { switch_to: None, pressed: vec!["a".into()] });
        let b = next_press_state(&ev("1:1", "b", KeyAction::Down), &map, 0, &["a".into()]);
        assert_eq!(b, Press { switch_to: None, pressed: vec!["a".into(), "b".into()] });
        let r = next_press_state(&ev("1:1", "a", KeyAction::Repeat), &map, 0, &["a".into(), "b".into()]);
        assert_eq!(r, Press { switch_to: None, pressed: vec!["a".into(), "b".into()] });
        let u = next_press_state(&ev("1:1", "a", KeyAction::Up), &map, 0, &["a".into(), "b".into()]);
        assert_eq!(u, Press { switch_to: None, pressed: vec!["b".into()] });
    }

    #[test]
    fn mapped_keyboard_switches_view() {
        let map = vec![("aa:aa".to_string(), 0), ("bb:bb".to_string(), 1)];
        let out = next_press_state(&ev("bb:bb", "j", KeyAction::Down), &map, 0, &["a".into()]);
        assert_eq!(out, Press { switch_to: Some(1), pressed: vec!["j".into()] });
    }
}
