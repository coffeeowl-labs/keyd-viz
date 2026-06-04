//! `[ids]` device matching — replicates keyd's logic for deciding which config
//! governs a given input device, so we can show only the connected keyboard's
//! config instead of every file.
//!
//! Faithful to keyd's `config_check_match` / `lookup_config_ent` (see ROADMAP §4.3):
//!   - entries are prefix-matched against the device id (`vendor:product` matches
//!     the full `vendor:product:uid`),
//!   - `k:` restricts to keyboards, `m:` to mice, a leading `-` excludes,
//!   - `*` is a wildcard (keyboards only),
//!   - an explicit match beats a wildcard match.
//!
//! This module is pure: device *enumeration* (reading `/proc` or `/sys`) lives in
//! the app's runtime layer; here we only decide matches.

/// Whether an entry restricts to a device type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeFilter {
    Any,
    Keyboard,
    Mouse,
}

impl TypeFilter {
    fn matches(self, is_keyboard: bool) -> bool {
        match self {
            TypeFilter::Any => true,
            TypeFilter::Keyboard => is_keyboard,
            TypeFilter::Mouse => !is_keyboard,
        }
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
    /// wildcard beats none).
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
    filter: TypeFilter,
    /// The `vendor:product` prefix; empty for a wildcard entry.
    pattern: String,
    wildcard: bool,
}

/// A parsed `[ids]` section, ready to match against device ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ids {
    entries: Vec<Entry>,
}

impl Ids {
    /// Parse raw `[ids]` lines (as collected into [`crate::Config::ids`]).
    pub fn parse(lines: &[String]) -> Ids {
        let entries = lines.iter().map(|l| parse_entry(l)).collect();
        Ids { entries }
    }

    /// Decide how this config matches a device. `devid` is `vendor:product`
    /// (lowercase hex); entries prefix-match it.
    pub fn match_device(&self, devid: &str, is_keyboard: bool) -> MatchKind {
        // Explicit/exclude prefix rules first, in order — first hit wins.
        for e in self.entries.iter().filter(|e| !e.wildcard) {
            if e.filter.matches(is_keyboard) && devid.starts_with(&e.pattern) {
                return if e.exclude { MatchKind::None } else { MatchKind::Explicit };
            }
        }
        // Otherwise fall back to a wildcard, if one applies to this device type.
        for e in self.entries.iter().filter(|e| e.wildcard && !e.exclude) {
            if e.filter.matches(is_keyboard) {
                return MatchKind::Wildcard;
            }
        }
        MatchKind::None
    }

    /// True if any wildcard entry is present.
    pub fn has_wildcard(&self) -> bool {
        self.entries.iter().any(|e| e.wildcard)
    }
}

fn parse_entry(line: &str) -> Entry {
    let mut s = line.trim();
    let mut exclude = false;
    if let Some(rest) = s.strip_prefix('-') {
        exclude = true;
        s = rest.trim();
    }
    let mut filter = TypeFilter::Any;
    if let Some(rest) = s.strip_prefix("k:") {
        filter = TypeFilter::Keyboard;
        s = rest;
    } else if let Some(rest) = s.strip_prefix("m:") {
        filter = TypeFilter::Mouse;
        s = rest;
    }
    let s = s.trim();
    if s == "*" {
        // keyd: a bare wildcard applies to keyboards only.
        if filter == TypeFilter::Any {
            filter = TypeFilter::Keyboard;
        }
        Entry { exclude, filter, pattern: String::new(), wildcard: true }
    } else {
        Entry { exclude, filter, pattern: s.to_string(), wildcard: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(lines: &[&str]) -> Ids {
        Ids::parse(&lines.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn explicit_vendor_product() {
        let i = ids(&["04fe:0021", "04fe:0202"]);
        assert_eq!(i.match_device("04fe:0021", true), MatchKind::Explicit);
        // prefix-matches the full vendor:product:uid form too
        assert_eq!(i.match_device("04fe:0021:deadbeef", true), MatchKind::Explicit);
        assert_eq!(i.match_device("1234:5678", true), MatchKind::None);
    }

    #[test]
    fn wildcard_is_keyboard_only() {
        let i = ids(&["*"]);
        assert!(i.has_wildcard());
        assert_eq!(i.match_device("04fe:0021", true), MatchKind::Wildcard);
        assert_eq!(i.match_device("046d:c52b", false), MatchKind::None); // a mouse
    }

    #[test]
    fn exclude_beats_wildcard() {
        let i = ids(&["*", "-04fe:0021"]);
        // the excluded keyboard does not match this config…
        assert_eq!(i.match_device("04fe:0021", true), MatchKind::None);
        // …but another keyboard falls through to the wildcard.
        assert_eq!(i.match_device("0001:0002", true), MatchKind::Wildcard);
    }

    #[test]
    fn type_filters() {
        let i = ids(&["k:*", "m:046d:c52b"]);
        assert_eq!(i.match_device("0001:0002", true), MatchKind::Wildcard); // any kbd
        assert_eq!(i.match_device("046d:c52b", false), MatchKind::Explicit); // the mouse
        // same id but a keyboard: the m: rule's filter rejects it, no wildcard for mice
        assert_eq!(i.match_device("046d:c52b", true), MatchKind::Wildcard); // k:* still
        // a mouse with no matching rule
        assert_eq!(i.match_device("dead:beef", false), MatchKind::None);
    }

    #[test]
    fn explicit_precedence_within_section() {
        // exclude rule appears before wildcard fallback regardless of order
        let i = ids(&["-dead:beef", "*"]);
        assert_eq!(i.match_device("dead:beef", true), MatchKind::None);
        assert_eq!(i.match_device("aaaa:bbbb", true), MatchKind::Wildcard);
    }
}
