//! Create-config flow: scan connected keyboards for ones not yet governed by a
//! specific config and offer them as fresh-config targets.
//!
//! The candidate filter here is deliberately *stricter* than keyd's own
//! "is a keyboard" rule — this is a human-facing picker, so precision beats recall
//! (see [`is_create_candidate`]). All scanning reads the same config directory the
//! one-click apply tool targets, so detection, collision checks, and apply can never
//! disagree.

use std::path::{Path, PathBuf};

use keydviz_core::{DeviceFlags, Ids, MatchKind};

use crate::devices::{self, InputDevice};
use crate::{conf_files_in, parse_configs};

/// A connected keyboard that has no specific config yet — a candidate for
/// spawning a fresh one. We avoid offering a device already governed by a config to
/// prevent spawning a second file with a colliding id. Both unclaimed and wildcard-only
/// keyboards qualify, because a new specific config out-ranks the wildcard.
pub(crate) struct CreateCandidate {
    /// The raw device name (may be empty) — used as the new board's label.
    pub(crate) name: String,
    /// Chip label, e.g. `PFU HHKB (04fe:0021)`.
    pub(crate) label: String,
    /// The `[ids]` entry to seed (the device's `vendor:product`).
    pub(crate) devid: String,
    /// A config name suggested from the device name, sanitised to the apply tool's
    /// allow-list.
    pub(crate) suggested: String,
}

/// The directory create-config reads existing configs from and writes the new one
/// to — the *same* dir the one-click apply tool targets, so candidate detection,
/// collision checks, and the apply path can never disagree. Falls back to the
/// production `/etc/keyd` when one-click isn't available (AppImage / plain source:
/// the new config goes through draft-then-install, but collisions are still checked
/// against the real dir).
pub(crate) fn create_config_dir() -> PathBuf {
    crate::applying::one_click()
        .map(|i| i.config_dir().to_path_buf())
        .unwrap_or_else(|| crate::applying::prod_config_dir().to_path_buf())
}

/// keyd's own virtual output devices carry vendor 0x0FAC (`device.c` sets
/// `is_virtual` on this vendor). They must never be offered as a config target:
/// keyd re-emits every grabbed keyboard *through* them, so a config matching
/// `0fac:*` would point keyd at its own output — a feedback loop, not a keyboard.
pub(crate) const KEYD_VIRTUAL_VENDOR: &str = "0fac";

/// Whether a connected device should be offered as a create-config target. This is
/// deliberately **stricter than keyd's own "is a keyboard" rule** (which also accepts
/// media-key emitters), because the create list is a human-facing picker — precision
/// beats recall. A candidate must:
///   - have the **full alphanumeric key block** (`full_keyboard`) — drops the media-
///     key/system pseudo-devices that share the input bus (Video Bus, WMI hotkeys,
///     lid/power/sleep), which `is_keyboard` would let through;
///   - **not be keyd's own virtual keyboard** (vendor `0fac`) — see above;
///   - **report no pointer motion** (`MOUSE` = any REL/ABS axis) — drops mice that
///     expose a keyboard HID interface (e.g. a Logitech G502 "Keyboard" node) and
///     synthetic pointer devices (e.g. ydotoold).
///
/// Trade-off: a *combo* node that is both a full keyboard and a pointer — some laptops
/// put the keyboard and touchpad on one event node — is excluded too (it's
/// indistinguishable from a mouse's keyboard interface by capabilities alone). In
/// practice such a device is already governed by an explicit config (so it wouldn't be
/// a candidate anyway), and the "All keyboards (\*)" wildcard is the fallback.
pub(crate) fn is_create_candidate(dev: &InputDevice) -> bool {
    dev.full_keyboard
        && dev.vendor != KEYD_VIRTUAL_VENDOR
        && !dev.flags.intersects(DeviceFlags::MOUSE)
}

/// Result of scanning connected keyboards for the create dialog.
pub(crate) struct CreateScan {
    /// Unclaimed / wildcard-only keyboards offered as fresh-config targets.
    pub(crate) candidates: Vec<CreateCandidate>,
    /// Display names of connected keyboards *already* governed by a specific config
    /// (deduped by `vendor:product`). Surfaced as an explainer so the user understands
    /// why their existing keyboards aren't candidates — they edit those configs instead.
    pub(crate) already_configured: Vec<String>,
}

/// Scan connected keyboards: split them into create candidates (best `[ids]` match is
/// None or Wildcard, *and* a real physical keyboard per [`is_create_candidate`]) and the
/// names of those already governed by a specific config. Both are deduped by
/// `vendor:product` (one keyboard exposes several event nodes); the governed name prefers
/// a full-keyboard node over a media-only sibling. See [`CreateCandidate`].
pub(crate) fn create_scan(config_dir: &Path) -> CreateScan {
    let configs = parse_configs(&conf_files_in(config_dir));
    let matchers: Vec<Ids> = configs.iter().map(|(_, c)| Ids::parse(&c.ids)).collect();
    let mut candidates: Vec<CreateCandidate> = Vec::new();
    // (devid, display name, whether the recorded name came from a full-keyboard node).
    let mut governed: Vec<(String, String, bool)> = Vec::new();
    for dev in devices::connected_devices() {
        let devid = dev.devid();
        let best = matchers
            .iter()
            .map(|ids| ids.match_device(&devid, dev.flags))
            .max_by_key(|m| m.rank())
            .unwrap_or(MatchKind::None);
        if best == MatchKind::Explicit {
            // Already governed → not a candidate; record it for the explainer (any
            // keyd-recognised keyboard, since the laptop's combo node fails the stricter
            // candidate filter but is still a keyboard the user has configured).
            if dev.is_keyboard {
                let name = if dev.name.is_empty() { devid.clone() } else { dev.name.clone() };
                match governed.iter_mut().find(|(d, _, _)| *d == devid) {
                    Some(e) if dev.full_keyboard && !e.2 => {
                        e.1 = name;
                        e.2 = true;
                    }
                    Some(_) => {}
                    None => governed.push((devid, name, dev.full_keyboard)),
                }
            }
            continue;
        }
        // Unclaimed or wildcard-only: offer it only if it's a real physical keyboard.
        if !is_create_candidate(&dev) || candidates.iter().any(|c| c.devid == devid) {
            continue;
        }
        let label =
            if dev.name.is_empty() { devid.clone() } else { format!("{} ({devid})", dev.name) };
        candidates.push(CreateCandidate {
            name: dev.name.clone(),
            label,
            suggested: sanitize_config_name(&dev.name),
            devid,
        });
    }
    CreateScan {
        candidates,
        already_configured: governed.into_iter().map(|(_, name, _)| name).collect(),
    }
}

/// The "already configured" explainer for the create dialog, or `""` when none of the
/// connected keyboards are governed by a specific config. Capped so a busy machine
/// doesn't produce an unreadably long line.
pub(crate) fn governed_line(names: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let shown = names.len().min(4);
    let more = names.len() - shown;
    let tail = if more > 0 { format!(" (+{more} more)") } else { String::new() };
    format!(
        "Already configured \u{2014} edit from the chooser above: {}{tail}",
        names[..shown].join(", ")
    )
}

/// Turn a free-form device name into a config name the apply tool's allow-list
/// accepts ([`keydviz_apply::valid_name`]): lowercased, every run of
/// non-`[a-z0-9_]` collapsed to a single `-`, leading/trailing `-` trimmed, capped
/// at 64. Falls back to `keyboard` when nothing usable survives.
pub(crate) fn sanitize_config_name(name: &str) -> String {
    let mut s = String::new();
    let mut prev_dash = false;
    for c in name.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_alphanumeric() || lc == '_' {
            s.push(lc);
            prev_dash = false;
        } else if !s.is_empty() && !prev_dash {
            s.push('-');
            prev_dash = true;
        }
    }
    let capped: String = s.trim_matches('-').chars().take(64).collect();
    let capped = capped.trim_end_matches('-');
    if capped.is_empty() {
        "keyboard".to_string()
    } else {
        capped.to_string()
    }
}

/// Whether `<config_dir>/<name>.conf` already exists — a *filename* collision,
/// distinct from an `[ids]` collision: creating over it would overwrite an unrelated
/// config, so the UI blocks it and asks for a different name.
pub(crate) fn config_name_taken(config_dir: &Path, name: &str) -> bool {
    config_dir.join(format!("{name}.conf")).exists()
}

#[cfg(test)]
mod tests {
    use super::{governed_line, is_create_candidate, sanitize_config_name};
    use crate::devices::InputDevice;
    use keydviz_core::DeviceFlags;

    fn dev(vendor: &str, full: bool, flags: DeviceFlags) -> InputDevice {
        InputDevice {
            name: "x".into(),
            vendor: vendor.into(),
            product: "0001".into(),
            is_keyboard: true,
            full_keyboard: full,
            flags,
        }
    }

    #[test]
    fn create_candidate_filter_keeps_only_real_keyboards() {
        // A real external keyboard (HHKB): full key block, no pointer axes.
        assert!(is_create_candidate(&dev("04fe", true, DeviceFlags::keyboard())));

        // keyd's OWN virtual keyboard (vendor 0fac, full key block, no pointer) — the
        // dangerous one: never offer it, or a config would target keyd's own output.
        assert!(!is_create_candidate(&dev("0fac", true, DeviceFlags::keyboard())));

        // Media-key / system pseudo-devices (Video Bus, WMI hotkeys, lid/power): they
        // pass keyd's is_keyboard via media keys but lack the full alphanumeric block.
        assert!(!is_create_candidate(&dev("0000", false, DeviceFlags::KEYBOARD)));

        // A mouse exposing a keyboard HID interface (Logitech G502) or a synthetic
        // pointer (ydotoold): full key block, but reports pointer motion.
        let mousey = DeviceFlags::keyboard().union(DeviceFlags::MOUSE);
        assert!(!is_create_candidate(&dev("046d", true, mousey)));

        // A keyboard+touchpad combo node — excluded too (documented trade-off: it's
        // capability-identical to a mouse's keyboard interface).
        let combo = DeviceFlags::keyboard().union(DeviceFlags::MOUSE).union(DeviceFlags::TRACKPAD);
        assert!(!is_create_candidate(&dev("0b05", true, combo)));
    }

    #[test]
    fn sanitizes_device_names_to_the_allow_list() {
        assert_eq!(sanitize_config_name("PFU HHKB"), "pfu-hhkb");
        assert_eq!(sanitize_config_name("ASUS ROG Zephyrus G14"), "asus-rog-zephyrus-g14");
        // Leading/trailing junk trimmed; runs of symbols collapse to one dash.
        assert_eq!(sanitize_config_name("  ::Keychron K2:: "), "keychron-k2");
        assert_eq!(sanitize_config_name("My_Board-2"), "my_board-2");
        // Every result is a name the apply tool would accept.
        for n in ["PFU HHKB", "  ::Keychron K2:: ", "My_Board-2", ""] {
            let s = sanitize_config_name(n);
            assert!(keydviz_apply::valid_name(&s), "{n:?} → {s:?} should be valid");
        }
    }

    #[test]
    fn falls_back_when_nothing_usable_survives() {
        assert_eq!(sanitize_config_name(""), "keyboard");
        assert_eq!(sanitize_config_name("!!! ###"), "keyboard");
    }

    #[test]
    fn governed_explainer_line() {
        assert_eq!(governed_line(&[]), "");
        let one = governed_line(&["PFU HHKB".to_string()]);
        assert!(one.contains("edit from the chooser above"));
        assert!(one.contains("PFU HHKB"));
        // Capped at 4 with a "+N more" tail.
        let many: Vec<String> = (0..6).map(|i| format!("kbd{i}")).collect();
        let line = governed_line(&many);
        assert!(line.contains("kbd0") && line.contains("kbd3"));
        assert!(!line.contains("kbd4"));
        assert!(line.contains("(+2 more)"));
    }

    #[test]
    fn caps_at_64_and_stays_valid() {
        let long = "a".repeat(200);
        let s = sanitize_config_name(&long);
        assert_eq!(s.len(), 64);
        assert!(keydviz_apply::valid_name(&s));
    }
}
