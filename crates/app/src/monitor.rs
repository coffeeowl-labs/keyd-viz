//! Live keypresses from `keyd monitor`.
//!
//! `keyd monitor` prints, to stdout, two kinds of newline-delimited records:
//!
//! ```text
//! device added: 04fe:0021:f26878c3 PFU Limited HHKB-Hybrid Keyboard (/dev/input/event9)
//! device removed: 04fe:0021:f26878c3 PFU Limited HHKB-Hybrid Keyboard (/dev/input/event9)
//! PFU Limited HHKB-Hybrid Keyboard\t04fe:0021:f26878c3\ta down
//! ```
//!
//! i.e. a key event is three tab-separated fields — device *name*, device *id*
//! (`vendor:product:hash`), and `"<key> <action>"` — matching keyd's internal
//! `"%s\t%s\t%s %s"` format string (verified against keyd v2.6.0). The key name is
//! the same namespace keyd configs use (`a`, `space`, `leftshift`, …), so it maps
//! straight onto a board cap's `key`; the id's first two fields are the
//! `vendor:product` used for `[ids]` matching, so a keypress can also tell us *which*
//! keyboard is active.
//!
//! Unlike `keyd listen` (layer names, gated on the `keyd` group), `keyd monitor`
//! reads `/dev/input` — typically the `input` group, which most desktop users are
//! already in. The shipped product routes this through the privileged helper so even
//! that group isn't required (see ROADMAP §1 zero-permission requirement); the parser
//! here is source-agnostic and unchanged by that move.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Whether a key went down, came up, or auto-repeated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Down,
    Up,
    Repeat,
}

/// A single key event from a specific device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    /// `vendor:product` (the `[ids]`-matchable id; the per-device hash is stripped).
    pub devid: String,
    /// Human-readable device name (as keyd reports it).
    pub device: String,
    /// keyd key name (`a`, `space`, `leftshift`, …).
    pub key: String,
    pub action: KeyAction,
}

/// One parsed record from the `keyd monitor` stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorEvent {
    /// A device appeared (`vendor:product`, name).
    DeviceAdded { devid: String, device: String },
    /// A device went away (`vendor:product`, name).
    DeviceRemoved { devid: String, device: String },
    /// A key transition.
    Key(KeyEvent),
}

/// Strip the trailing per-device hash from a `vendor:product:hash` id, yielding the
/// `vendor:product` used for `[ids]` matching. Ids already in `vendor:product` form
/// (or anything else) are returned unchanged.
fn vendor_product(id: &str) -> String {
    let mut it = id.split(':');
    match (it.next(), it.next()) {
        (Some(v), Some(p)) => format!("{v}:{p}"),
        _ => id.to_string(),
    }
}

/// Parse a `device added:` / `device removed:` line of the form
/// `"<id> <name...> (/dev/input/eventN)"`. Returns `(devid, name)`.
fn parse_device_line(rest: &str) -> Option<(String, String)> {
    let rest = rest.trim();
    let (id, after) = rest.split_once(' ')?;
    // Drop the trailing " (/dev/input/eventN)" node path if present.
    let name = match after.rfind(" (") {
        Some(i) => &after[..i],
        None => after,
    };
    Some((vendor_product(id), name.trim().to_string()))
}

/// Parse one line of `keyd monitor` output. Returns `None` for blank/unknown lines.
pub fn parse_monitor_line(line: &str) -> Option<MonitorEvent> {
    let line = line.trim_end_matches(['\n', '\r']);
    if let Some(rest) = line.strip_prefix("device added:") {
        let (devid, device) = parse_device_line(rest)?;
        return Some(MonitorEvent::DeviceAdded { devid, device });
    }
    if let Some(rest) = line.strip_prefix("device removed:") {
        let (devid, device) = parse_device_line(rest)?;
        return Some(MonitorEvent::DeviceRemoved { devid, device });
    }

    // Key event: "name\tvendor:product:hash\tkey action"
    let mut fields = line.split('\t');
    let device = fields.next()?.trim();
    let id = fields.next()?.trim();
    let key_action = fields.next()?.trim();
    if device.is_empty() || id.is_empty() {
        return None;
    }
    let (key, action) = key_action.rsplit_once(' ')?;
    let action = match action {
        "down" => KeyAction::Down,
        "up" => KeyAction::Up,
        "repeat" => KeyAction::Repeat,
        _ => return None,
    };
    Some(MonitorEvent::Key(KeyEvent {
        devid: vendor_product(id),
        device: device.to_string(),
        key: key.trim().to_string(),
        action,
    }))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_events() {
        // The exact form captured from keyd v2.6.0 (ydotool synthetic press).
        assert_eq!(
            parse_monitor_line("ydotoold virtual device\t2333:6666:e7fb73a9\ta down"),
            Some(MonitorEvent::Key(KeyEvent {
                devid: "2333:6666".into(),
                device: "ydotoold virtual device".into(),
                key: "a".into(),
                action: KeyAction::Down,
            }))
        );
        assert_eq!(
            parse_monitor_line("PFU HHKB\t04fe:0021:f26878c3\tleftshift up"),
            Some(MonitorEvent::Key(KeyEvent {
                devid: "04fe:0021".into(),
                device: "PFU HHKB".into(),
                key: "leftshift".into(),
                action: KeyAction::Up,
            }))
        );
        // repeat action
        assert!(matches!(
            parse_monitor_line("KB\t1:2:3\tspace repeat"),
            Some(MonitorEvent::Key(KeyEvent { action: KeyAction::Repeat, .. }))
        ));
    }

    #[test]
    fn parses_device_lines() {
        assert_eq!(
            parse_monitor_line(
                "device added: 04fe:0021:f26878c3 PFU Limited HHKB-Hybrid Keyboard (/dev/input/event9)"
            ),
            Some(MonitorEvent::DeviceAdded {
                devid: "04fe:0021".into(),
                device: "PFU Limited HHKB-Hybrid Keyboard".into(),
            })
        );
        assert_eq!(
            parse_monitor_line(
                "device removed: 046d:c098:0910139a Logitech G502 X (/dev/input/event12)"
            ),
            Some(MonitorEvent::DeviceRemoved {
                devid: "046d:c098".into(),
                device: "Logitech G502 X".into(),
            })
        );
    }

    #[test]
    fn ignores_garbage_and_blanks() {
        assert_eq!(parse_monitor_line(""), None);
        assert_eq!(parse_monitor_line("   "), None);
        assert_eq!(parse_monitor_line("device added:"), None); // no id/name
        assert_eq!(parse_monitor_line("name\tid\tkey sideways"), None); // bad action
        assert_eq!(parse_monitor_line("only one field"), None);
    }

    #[test]
    fn vendor_product_strips_hash() {
        assert_eq!(vendor_product("04fe:0021:f26878c3"), "04fe:0021");
        assert_eq!(vendor_product("04fe:0021"), "04fe:0021");
        assert_eq!(vendor_product("weird"), "weird");
    }
}
