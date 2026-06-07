//! Enumerate connected input devices from `/proc/bus/input/devices` and classify
//! keyboards using keyd's own capability heuristic.
//!
//! keyd marks a device as a keyboard (`CAP_KEYBOARD`) if its `EV_KEY` capability
//! bitmap contains **all** of `KEY_1..KEY_0, KEY_Q..KEY_Y`, **or** any of a few
//! media keys (brightness/volume/touchpad/mic) — see keyd's
//! `resolve_device_capabilities`. That same `EV_KEY` bitmap is exposed verbatim in
//! the `B: KEY=` line of `/proc/bus/input/devices`, so we reproduce the rule here
//! without opening the device (no privilege needed).

use std::path::Path;

use keydviz_core::DeviceFlags;

/// A connected input device, with the data we need to match it against `[ids]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputDevice {
    pub name: String,
    /// 4-hex lowercase vendor id.
    pub vendor: String,
    /// 4-hex lowercase product id.
    pub product: String,
    /// keyd-faithful: a real keyboard or a media-key emitter.
    pub is_keyboard: bool,
    /// A "full" keyboard (has the whole alphanumeric key set) — used to prefer
    /// real keyboards over media-key pseudo-devices when labeling.
    pub full_keyboard: bool,
    /// The full capability set in keyd's flag space — what `[ids]` matching
    /// actually tests (a combo keyboard+mouse carries several bits).
    pub flags: DeviceFlags,
}

impl InputDevice {
    /// The `vendor:product` id used for `[ids]` prefix matching.
    pub fn devid(&self) -> String {
        format!("{}:{}", self.vendor, self.product)
    }
}

// keyd's keyboard key set (device must have ALL): KEY_1..KEY_0, KEY_Q..KEY_Y.
const KEYBOARD_KEYS: [u32; 16] =
    [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 16, 17, 18, 19, 20, 21];
// keyd's media keys (ANY qualifies): BRIGHTNESSUP, VOLUMEUP, TOUCHPAD_TOGGLE,
// TOUCHPAD_OFF, MICMUTE.
const MEDIA_KEYS: [u32; 5] = [225, 115, 530, 532, 248];

/// All connected input devices on this system (empty if `/proc` is unreadable).
pub fn connected_devices() -> Vec<InputDevice> {
    let text = std::fs::read_to_string(Path::new("/proc/bus/input/devices")).unwrap_or_default();
    parse_devices(&text)
}

/// Parse the text of `/proc/bus/input/devices` into devices. Pure (no I/O).
pub fn parse_devices(text: &str) -> Vec<InputDevice> {
    let mut out = Vec::new();
    let mut block = Block::default();
    for line in text.lines() {
        if line.trim().is_empty() {
            if let Some(d) = block.finish() {
                out.push(d);
            }
            block = Block::default();
        } else {
            block.feed(line);
        }
    }
    if let Some(d) = block.finish() {
        out.push(d);
    }
    out
}

#[derive(Default)]
struct Block {
    name: Option<String>,
    vendor: Option<String>,
    product: Option<String>,
    /// `EV_KEY` bitmap words as printed (most-significant word first).
    key_words: Option<Vec<u64>>,
    /// Any relative axes (`B: REL=` nonzero) — a mouse to keyd.
    has_rel: bool,
    /// Any absolute axes (`B: ABS=` nonzero) — a trackpad/abs mouse to keyd.
    has_abs: bool,
}

impl Block {
    fn feed(&mut self, line: &str) {
        if let Some(rest) = line.strip_prefix("I:") {
            for tok in rest.split_whitespace() {
                if let Some(v) = tok.strip_prefix("Vendor=") {
                    self.vendor = Some(v.to_ascii_lowercase());
                } else if let Some(p) = tok.strip_prefix("Product=") {
                    self.product = Some(p.to_ascii_lowercase());
                }
            }
        } else if let Some(rest) = line.strip_prefix("N: Name=") {
            self.name = Some(rest.trim().trim_matches('"').to_string());
        } else if let Some(rest) = line.strip_prefix("B: KEY=") {
            self.key_words =
                Some(rest.split_whitespace().filter_map(|w| u64::from_str_radix(w, 16).ok()).collect());
        } else if let Some(rest) = line.strip_prefix("B: REL=") {
            self.has_rel = bitmap_nonzero(rest);
        } else if let Some(rest) = line.strip_prefix("B: ABS=") {
            self.has_abs = bitmap_nonzero(rest);
        }
    }

    fn finish(self) -> Option<InputDevice> {
        let vendor = self.vendor?;
        let product = self.product?;
        let words = self.key_words.unwrap_or_default();
        let full = has_all_keyboard_keys(&words);
        let media = MEDIA_KEYS.iter().any(|&k| has_key(&words, k));
        // keyd's resolve_device_capabilities → manage_device flag translation:
        // any key → KEY; the keyboard rule → KEYBOARD; rel|abs → MOUSE; abs →
        // TRACKPAD. One device can carry several bits (combo keyboard+mouse).
        let mut flags = DeviceFlags::default();
        if words.iter().any(|&w| w != 0) {
            flags = flags.union(DeviceFlags::KEY);
        }
        if full || media {
            flags = flags.union(DeviceFlags::KEYBOARD);
        }
        if self.has_rel || self.has_abs {
            flags = flags.union(DeviceFlags::MOUSE);
        }
        if self.has_abs {
            flags = flags.union(DeviceFlags::TRACKPAD);
        }
        Some(InputDevice {
            name: self.name.unwrap_or_default(),
            vendor,
            product,
            is_keyboard: full || media,
            full_keyboard: full,
            flags,
        })
    }
}

/// True if any word of a `B: <MAP>=` bitmap line is nonzero.
fn bitmap_nonzero(rest: &str) -> bool {
    rest.split_whitespace()
        .filter_map(|w| u64::from_str_radix(w, 16).ok())
        .any(|w| w != 0)
}

/// Is keycode `code` set in the printed bitmap? Words are most-significant first,
/// so the last word covers bits 0..63.
fn has_key(words: &[u64], code: u32) -> bool {
    let group = (code / 64) as usize;
    let bit = code % 64;
    let n = words.len();
    if group >= n {
        return false; // high all-zero words are omitted from /proc output
    }
    (words[n - 1 - group] >> bit) & 1 == 1
}

fn has_all_keyboard_keys(words: &[u64]) -> bool {
    !words.is_empty() && KEYBOARD_KEYS.iter().all(|&k| has_key(words, k))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Low word with KEY_1..KEY_0 (bits 2-11) and KEY_Q..KEY_Y (bits 16-21) set.
    const FULL_KB_LOW: &str = "3f0ffc";

    #[test]
    fn classifies_full_keyboard() {
        let text = format!(
            "I: Bus=0003 Vendor=04fe Product=0021 Version=0111\n\
             N: Name=\"PFU HHKB\"\n\
             H: Handlers=sysrq kbd event3\n\
             B: KEY={FULL_KB_LOW}\n"
        );
        let devs = parse_devices(&text);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].devid(), "04fe:0021");
        assert_eq!(devs[0].name, "PFU HHKB");
        assert!(devs[0].is_keyboard);
        assert!(devs[0].full_keyboard);
    }

    #[test]
    fn power_button_is_not_a_keyboard() {
        // Only a high-ish bit set, none of the alphanumeric keys, no media keys.
        let text = "I: Bus=0019 Vendor=0000 Product=0001 Version=0000\n\
                    N: Name=\"Power Button\"\n\
                    B: KEY=10000000000000\n";
        let devs = parse_devices(text);
        assert_eq!(devs.len(), 1);
        assert!(!devs[0].is_keyboard);
        assert!(!devs[0].full_keyboard);
    }

    #[test]
    fn media_key_device_counts_as_keyboard_but_not_full() {
        // KEY_VOLUMEUP = 115 -> group 1, bit 51. Two words: [high, low].
        let text = "I: Bus=0003 Vendor=1234 Product=5678 Version=0001\n\
                    N: Name=\"Laptop Hotkeys\"\n\
                    B: KEY=8000000000000 0\n";
        let devs = parse_devices(text);
        assert_eq!(devs.len(), 1);
        assert!(devs[0].is_keyboard, "media-key device should count as keyboard");
        assert!(!devs[0].full_keyboard);
    }

    #[test]
    fn parses_multiple_blocks() {
        let text = format!(
            "I: Bus=0003 Vendor=04fe Product=0021\n\
             N: Name=\"KB\"\n\
             B: KEY={FULL_KB_LOW}\n\
             \n\
             I: Bus=0003 Vendor=046d Product=c52b\n\
             N: Name=\"Mouse\"\n\
             B: KEY=0\n"
        );
        let devs = parse_devices(&text);
        assert_eq!(devs.len(), 2);
        assert!(devs[0].is_keyboard);
        assert!(!devs[1].is_keyboard);
    }

    #[test]
    fn capability_flags_cover_mice_and_combos() {
        // A mouse: buttons (KEY bits set) + relative axes, no keyboard keys.
        let text = format!(
            "I: Bus=0003 Vendor=046d Product=c52b\n\
             N: Name=\"Mouse\"\n\
             B: KEY=70000 0 0 0 0\n\
             B: REL=903\n\
             \n\
             I: Bus=0003 Vendor=feed Product=cafe\n\
             N: Name=\"Combo Board\"\n\
             B: KEY={FULL_KB_LOW}\n\
             B: REL=3\n"
        );
        let devs = parse_devices(&text);
        let mouse = &devs[0];
        assert!(!mouse.is_keyboard);
        assert!(mouse.flags.contains(DeviceFlags::MOUSE.union(DeviceFlags::KEY)));
        assert!(!mouse.flags.intersects(DeviceFlags::KEYBOARD));
        // The §12 case: one device that is both keyboard and mouse — a bool
        // can't carry this, the flag set can.
        let combo = &devs[1];
        assert!(combo.is_keyboard);
        assert!(combo.flags.contains(DeviceFlags::keyboard().union(DeviceFlags::MOUSE)));
        assert!(!combo.flags.intersects(DeviceFlags::TRACKPAD));
    }

    #[test]
    fn abs_axes_mark_a_trackpad() {
        let text = "I: Bus=0003 Vendor=04f3 Product=311c\n\
                    N: Name=\"Touchpad\"\n\
                    B: KEY=e520 10000 0 0 0 0\n\
                    B: ABS=260800000000003\n";
        let devs = parse_devices(text);
        assert!(devs[0]
            .flags
            .contains(DeviceFlags::MOUSE.union(DeviceFlags::TRACKPAD)));
    }
}
