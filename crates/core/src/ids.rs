//! `[ids]` device matching — replicates keyd's logic for deciding which config
//! governs a given input device, so we can show only the connected keyboard's
//! config instead of every file.
//!
//! Faithful to keyd 2.6.0 `config_check_match` (config.c) + the wildcard rule in
//! `lookup_config_ent` (daemon.c), in keyd's own flag space (`ID_*`, config.h):
//!   - entries are prefix-matched against the device id (`vendor:product` matches
//!     the full `vendor:product:uid`), first hit wins; an excluded hit (`-…`)
//!     rejects immediately; a hit whose type flags don't overlap the device's
//!     keeps scanning,
//!   - `m:` entries carry MOUSE; `k:` entries carry KEYBOARD|KEY; a plain id
//!     carries KEYBOARD|KEY|MOUSE. Matching is bitwise overlap, so a combo
//!     keyboard+mouse device matches either filter — and (faithfully) a `k:`
//!     entry matches a button-bearing mouse via the KEY bit,
//!   - only a bare `*` is the wildcard (`parse_id_section` does an exact compare:
//!     `k:*` is a dead entry whose pattern `*` prefix-matches nothing),
//!   - the wildcard matches keyboard-capable, non-trackpad devices only, and an
//!     explicit match beats it (`MatchKind::rank`).
//!
//! This module is pure: device *enumeration* (reading `/proc` or `/sys`) lives in
//! the app's runtime layer; here we only decide matches.

/// Device capability flags, in keyd's `ID_*` bit space (config.h). A device can
/// be several at once (combo keyboard+trackpad); `[ids]` entries carry the set
/// they match and the test is bitwise overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeviceFlags(u8);

impl DeviceFlags {
    pub const MOUSE: DeviceFlags = DeviceFlags(2);
    pub const KEYBOARD: DeviceFlags = DeviceFlags(4);
    pub const TRACKPAD: DeviceFlags = DeviceFlags(8);
    /// Emits keys, but is not necessarily a keyboard (mouse buttons count).
    pub const KEY: DeviceFlags = DeviceFlags(16);

    /// What keyd assigns a plain keyboard (CAP_KEYBOARD|CAP_KEY → ID space).
    pub fn keyboard() -> DeviceFlags {
        Self::KEYBOARD.union(Self::KEY)
    }

    /// A typical mouse: relative axes plus buttons (buttons are keys to keyd).
    pub fn mouse() -> DeviceFlags {
        Self::MOUSE.union(Self::KEY)
    }

    pub fn union(self, other: DeviceFlags) -> DeviceFlags {
        DeviceFlags(self.0 | other.0)
    }

    /// Any bit in common — keyd's `entry.flags & device.flags` match test.
    pub fn intersects(self, other: DeviceFlags) -> bool {
        self.0 & other.0 != 0
    }

    /// All of `other`'s bits present.
    pub fn contains(self, other: DeviceFlags) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// How strongly a config matched a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// No match (or explicitly excluded).
    None,
    /// Matched only via a `*` wildcard.
    Wildcard,
    /// Matched a specific `vendor:product` entry.
    Explicit,
}

impl MatchKind {
    /// Strength rank for picking the best config for a device (explicit beats
    /// wildcard beats none) — keyd's 2/1/0 from `config_check_match`.
    pub fn rank(self) -> u8 {
        match self {
            MatchKind::None => 0,
            MatchKind::Wildcard => 1,
            MatchKind::Explicit => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    exclude: bool,
    /// Device types this entry matches (overlap test); unused when `exclude`.
    flags: DeviceFlags,
    /// The id prefix to match.
    pattern: String,
}

/// A parsed `[ids]` section, ready to match against device ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ids {
    entries: Vec<Entry>,
    wildcard: bool,
}

impl Ids {
    /// Parse raw `[ids]` lines (as collected into [`crate::Config::ids`]),
    /// mirroring keyd's `parse_id_section` prefix order: bare `*` (exact), `m:`,
    /// `k:`, `-` (which does NOT strip a further type prefix), plain id.
    pub fn parse(lines: &[String]) -> Ids {
        let mut ids = Ids::default();
        for line in lines {
            let s = line.trim();
            if s == "*" {
                ids.wildcard = true;
            } else if let Some(rest) = s.strip_prefix("m:") {
                ids.entries.push(Entry {
                    exclude: false,
                    flags: DeviceFlags::MOUSE,
                    pattern: rest.to_string(),
                });
            } else if let Some(rest) = s.strip_prefix("k:") {
                ids.entries.push(Entry {
                    exclude: false,
                    flags: DeviceFlags::KEYBOARD.union(DeviceFlags::KEY),
                    pattern: rest.to_string(),
                });
            } else if let Some(rest) = s.strip_prefix('-') {
                ids.entries.push(Entry {
                    exclude: true,
                    flags: DeviceFlags::default(),
                    pattern: rest.to_string(),
                });
            } else {
                ids.entries.push(Entry {
                    exclude: false,
                    flags: DeviceFlags::KEYBOARD
                        .union(DeviceFlags::KEY)
                        .union(DeviceFlags::MOUSE),
                    pattern: s.to_string(),
                });
            }
        }
        ids
    }

    /// Decide how this config matches a device. `devid` is `vendor:product`
    /// (lowercase hex); entries prefix-match it. `flags` is the device's
    /// capability set ([`DeviceFlags::keyboard`] for an ordinary keyboard).
    pub fn match_device(&self, devid: &str, flags: DeviceFlags) -> MatchKind {
        for e in &self.entries {
            if devid.starts_with(&e.pattern) {
                if e.exclude {
                    return MatchKind::None;
                }
                if e.flags.intersects(flags) {
                    return MatchKind::Explicit;
                }
                // Prefix hit but wrong device type: keyd keeps scanning.
            }
        }
        // daemon.c: "The wildcard should not match mice or trackpads" —
        // keyboard-capable and not a trackpad.
        if self.wildcard
            && flags.contains(DeviceFlags::KEYBOARD)
            && !flags.intersects(DeviceFlags::TRACKPAD)
        {
            MatchKind::Wildcard
        } else {
            MatchKind::None
        }
    }

    /// True if the section has the bare-`*` wildcard.
    pub fn has_wildcard(&self) -> bool {
        self.wildcard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(lines: &[&str]) -> Ids {
        Ids::parse(&lines.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn kbd() -> DeviceFlags {
        DeviceFlags::keyboard()
    }

    fn mouse() -> DeviceFlags {
        DeviceFlags::mouse()
    }

    #[test]
    fn explicit_vendor_product() {
        let i = ids(&["04fe:0021", "04fe:0202"]);
        assert_eq!(i.match_device("04fe:0021", kbd()), MatchKind::Explicit);
        // prefix-matches the full vendor:product:uid form too
        assert_eq!(i.match_device("04fe:0021:deadbeef", kbd()), MatchKind::Explicit);
        assert_eq!(i.match_device("1234:5678", kbd()), MatchKind::None);
    }

    #[test]
    fn wildcard_is_keyboardish_only() {
        let i = ids(&["*"]);
        assert!(i.has_wildcard());
        assert_eq!(i.match_device("04fe:0021", kbd()), MatchKind::Wildcard);
        assert_eq!(i.match_device("046d:c52b", mouse()), MatchKind::None);
        // A combo keyboard+mouse is keyboard-capable → wildcard applies…
        let combo = kbd().union(DeviceFlags::MOUSE);
        assert_eq!(i.match_device("aaaa:bbbb", combo), MatchKind::Wildcard);
        // …but a trackpad-bearing combo is excluded by the daemon rule.
        let with_pad = combo.union(DeviceFlags::TRACKPAD);
        assert_eq!(i.match_device("aaaa:bbbb", with_pad), MatchKind::None);
    }

    #[test]
    fn exclude_beats_wildcard() {
        let i = ids(&["*", "-04fe:0021"]);
        // the excluded keyboard does not match this config…
        assert_eq!(i.match_device("04fe:0021", kbd()), MatchKind::None);
        // …but another keyboard falls through to the wildcard.
        assert_eq!(i.match_device("0001:0002", kbd()), MatchKind::Wildcard);
    }

    #[test]
    fn combo_device_matches_either_type_filter() {
        // The §12 motivating case: one device that is both keyboard and mouse.
        // A bool can't represent it; flag overlap matches both filters.
        let combo = kbd().union(DeviceFlags::MOUSE);
        let m = ids(&["m:046d:c548"]);
        let k = ids(&["k:046d:c548"]);
        assert_eq!(m.match_device("046d:c548", combo), MatchKind::Explicit);
        assert_eq!(k.match_device("046d:c548", combo), MatchKind::Explicit);
        // A pure keyboard still doesn't match the m: entry.
        assert_eq!(m.match_device("046d:c548", kbd()), MatchKind::None);
    }

    #[test]
    fn k_entry_matches_key_emitting_mouse() {
        // Faithful keyd oddity: mouse buttons set the KEY bit, and k: entries
        // carry KEYBOARD|KEY, so a k: id matches a button-bearing mouse.
        let i = ids(&["k:046d:c52b"]);
        assert_eq!(i.match_device("046d:c52b", mouse()), MatchKind::Explicit);
    }

    #[test]
    fn k_star_is_a_dead_entry_not_a_wildcard() {
        // keyd's wildcard check is an exact compare against "*": `k:*` parses as
        // a k: entry whose pattern `*` prefix-matches no real id.
        let i = ids(&["k:*"]);
        assert!(!i.has_wildcard());
        assert_eq!(i.match_device("04fe:0021", kbd()), MatchKind::None);
    }

    #[test]
    fn wrong_type_prefix_hit_keeps_scanning() {
        // A prefix hit with non-overlapping flags must not stop the scan.
        let i = ids(&["m:04fe:0021", "04fe:0021"]);
        assert_eq!(i.match_device("04fe:0021", kbd()), MatchKind::Explicit);
    }

    #[test]
    fn explicit_precedence_within_section() {
        // exclude wins on first prefix hit regardless of position
        let i = ids(&["-dead:beef", "*"]);
        assert_eq!(i.match_device("dead:beef", kbd()), MatchKind::None);
        assert_eq!(i.match_device("aaaa:bbbb", kbd()), MatchKind::Wildcard);
    }
}
