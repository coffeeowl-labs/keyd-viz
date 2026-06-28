//! Live event model shared by the GUI and the brokering helper daemon.
//!
//! Two keyd text formats drive the live view, parsed here:
//! - layer transitions (`+name`/`-name`/`/layout`) — read from keyd's control socket (the
//!   helper) or `keyd listen` stdout (the GUI's direct fallback).
//! - keypresses + device hotplug (`keyd monitor` output) — used by the GUI's direct
//!   fallback; the helper reads these from `/dev/input` (evdev) and builds events directly.
//!
//! This module holds the **pure** parsers for both ([`parse_listen_line`],
//! [`parse_monitor_line`]) and the layer-stack reducer ([`ActiveLayers`]) — no I/O, so
//! every source shares identical parsing. It also defines [`LiveEvent`], the
//! one-event-per-line **JSON wire protocol** the helper emits and the GUI consumes
//! (`events out only`, ROADMAP §5 / `docs/helper-design.md`).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Layer stream (`keyd listen`)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Keypress stream (`keyd monitor`)
// ---------------------------------------------------------------------------

/// Whether a key went down, came up, or auto-repeated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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

// ---------------------------------------------------------------------------
// Wire protocol (helper → GUI): one JSON object per line, events out only
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LayerAction {
    On,
    Off,
    Layout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceAction {
    Added,
    Removed,
}

/// One line of the helper's event stream. The GUI never sends anything back — this is
/// the entire surface the unprivileged client sees. Serialized as a single JSON object
/// per line, tagged by `"t"`, e.g.:
///
/// ```json
/// {"t":"hello","keyd":"2.6.0"}
/// {"t":"layer","action":"on","name":"nav"}
/// {"t":"key","devid":"04fe:0021","device":"PFU HHKB","key":"a","action":"down"}
/// {"t":"device","action":"added","devid":"04fe:0021","device":"PFU HHKB"}
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum LiveEvent {
    /// Sent once on connect, so the client can confirm the helper + keyd version.
    Hello { keyd: String },
    Layer { action: LayerAction, name: String },
    Key { devid: String, device: String, key: String, action: KeyAction },
    Device { action: DeviceAction, devid: String, device: String },
}

impl LiveEvent {
    /// Serialize as one newline-terminated JSON line for the wire.
    pub fn to_line(&self) -> String {
        // These types always serialize; fall back to an empty object on the impossible
        // error rather than panicking in the daemon's hot path.
        let mut s = serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string());
        s.push('\n');
        s
    }

    /// Parse one wire line back into an event. `None` on blank/garbage lines.
    pub fn from_line(line: &str) -> Option<LiveEvent> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }
        serde_json::from_str(line).ok()
    }

    /// The layer transition this event represents, for feeding [`ActiveLayers`].
    pub fn as_layer(&self) -> Option<LayerEvent> {
        match self {
            LiveEvent::Layer { action: LayerAction::On, name } => Some(LayerEvent::On(name.clone())),
            LiveEvent::Layer { action: LayerAction::Off, name } => Some(LayerEvent::Off(name.clone())),
            LiveEvent::Layer { action: LayerAction::Layout, name } => {
                Some(LayerEvent::Layout(name.clone()))
            }
            _ => None,
        }
    }

    /// The monitor record this event represents, for the keypress/glow path.
    pub fn as_monitor(&self) -> Option<MonitorEvent> {
        match self {
            LiveEvent::Key { devid, device, key, action } => Some(MonitorEvent::Key(KeyEvent {
                devid: devid.clone(),
                device: device.clone(),
                key: key.clone(),
                action: *action,
            })),
            LiveEvent::Device { action: DeviceAction::Added, devid, device } => {
                Some(MonitorEvent::DeviceAdded { devid: devid.clone(), device: device.clone() })
            }
            LiveEvent::Device { action: DeviceAction::Removed, devid, device } => {
                Some(MonitorEvent::DeviceRemoved { devid: devid.clone(), device: device.clone() })
            }
            _ => None,
        }
    }
}

impl From<&LayerEvent> for LiveEvent {
    fn from(ev: &LayerEvent) -> Self {
        let (action, name) = match ev {
            LayerEvent::On(n) => (LayerAction::On, n.clone()),
            LayerEvent::Off(n) => (LayerAction::Off, n.clone()),
            LayerEvent::Layout(n) => (LayerAction::Layout, n.clone()),
        };
        LiveEvent::Layer { action, name }
    }
}

impl From<&MonitorEvent> for LiveEvent {
    fn from(ev: &MonitorEvent) -> Self {
        match ev {
            MonitorEvent::Key(k) => LiveEvent::Key {
                devid: k.devid.clone(),
                device: k.device.clone(),
                key: k.key.clone(),
                action: k.action,
            },
            MonitorEvent::DeviceAdded { devid, device } => LiveEvent::Device {
                action: DeviceAction::Added,
                devid: devid.clone(),
                device: device.clone(),
            },
            MonitorEvent::DeviceRemoved { devid, device } => LiveEvent::Device {
                action: DeviceAction::Removed,
                devid: devid.clone(),
                device: device.clone(),
            },
        }
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
        assert_eq!(parse_monitor_line("device added:"), None);
        assert_eq!(parse_monitor_line("name\tid\tkey sideways"), None);
        assert_eq!(parse_monitor_line("only one field"), None);
    }

    #[test]
    fn vendor_product_strips_hash() {
        assert_eq!(vendor_product("04fe:0021:f26878c3"), "04fe:0021");
        assert_eq!(vendor_product("04fe:0021"), "04fe:0021");
        assert_eq!(vendor_product("weird"), "weird");
    }

    #[test]
    fn wire_layer_roundtrip_and_format() {
        let ev: LiveEvent = (&LayerEvent::On("nav".into())).into();
        let line = ev.to_line();
        assert_eq!(line, "{\"t\":\"layer\",\"action\":\"on\",\"name\":\"nav\"}\n");
        let back = LiveEvent::from_line(&line).unwrap();
        assert_eq!(back, ev);
        assert_eq!(back.as_layer(), Some(LayerEvent::On("nav".into())));
    }

    #[test]
    fn wire_key_roundtrip() {
        let mon = MonitorEvent::Key(KeyEvent {
            devid: "04fe:0021".into(),
            device: "PFU HHKB".into(),
            key: "a".into(),
            action: KeyAction::Down,
        });
        let ev: LiveEvent = (&mon).into();
        let back = LiveEvent::from_line(&ev.to_line()).unwrap();
        assert_eq!(back, ev);
        assert_eq!(back.as_monitor(), Some(mon));
    }

    #[test]
    fn wire_device_and_hello() {
        let mon = MonitorEvent::DeviceRemoved { devid: "1:2".into(), device: "kb".into() };
        let ev: LiveEvent = (&mon).into();
        assert_eq!(LiveEvent::from_line(&ev.to_line()).unwrap().as_monitor(), Some(mon));

        let hello = LiveEvent::Hello { keyd: "2.6.0".into() };
        assert_eq!(LiveEvent::from_line(&hello.to_line()), Some(hello));
    }

    #[test]
    fn from_line_rejects_garbage() {
        assert_eq!(LiveEvent::from_line(""), None);
        assert_eq!(LiveEvent::from_line("not json"), None);
        assert_eq!(LiveEvent::from_line("{\"t\":\"bogus\"}"), None);
    }

    // -------------------------------------------------- mutation-gap regressions
    #[test]
    fn key_line_needs_both_device_and_id() {
        assert_eq!(parse_monitor_line("\t04fe:0021:hash\ta down"), None);
        assert_eq!(parse_monitor_line("PFU HHKB\t\ta down"), None);
    }

    #[test]
    fn as_layer_covers_off_and_layout() {
        let off = LiveEvent::Layer { action: LayerAction::Off, name: "nav".into() };
        assert_eq!(off.as_layer(), Some(LayerEvent::Off("nav".into())));
        let layout = LiveEvent::Layer { action: LayerAction::Layout, name: "main".into() };
        assert_eq!(layout.as_layer(), Some(LayerEvent::Layout("main".into())));
    }

    #[test]
    fn as_monitor_covers_device_added() {
        let ev = LiveEvent::Device {
            action: DeviceAction::Added,
            devid: "04fe:0021".into(),
            device: "PFU HHKB".into(),
        };
        assert_eq!(
            ev.as_monitor(),
            Some(MonitorEvent::DeviceAdded { devid: "04fe:0021".into(), device: "PFU HHKB".into() })
        );
    }
}
