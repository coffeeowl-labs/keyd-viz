//! The keyd modifier set, in one place.
//!
//! keyd has five modifiers, and the UI needs four representations of each: the
//! chord-prefix **letter** (the `C` in `C-a`), the keyd **target** name as
//! written in a config (`control`), the display **word** (`Ctrl`), and the
//! compact cap **glyph** (`⌃`). Those four maps used to live in five different
//! files and had already drifted — `meta` rendered as "Super" in one renderer
//! and "Meta" in another, side by side in the same UI. Now every renderer reads
//! from [`MODS`] so they cannot disagree.

/// A keyd modifier and its display forms.
pub struct Mod {
    /// keyd chord-prefix letter: the `C` in `C-a`.
    pub letter: char,
    /// keyd descriptor/target name, as written in a config (`control`, `meta`).
    pub target: &'static str,
    /// Human display word (`Ctrl`, `Meta`).
    pub word: &'static str,
    /// Compact cap glyph; AltGr has no single glyph so it reuses its word.
    pub glyph: &'static str,
}

/// The five keyd modifiers, in keyd's canonical `C S A M G` order.
pub const MODS: [Mod; 5] = [
    Mod { letter: 'C', target: "control", word: "Ctrl", glyph: "\u{2303}" }, // ⌃
    Mod { letter: 'S', target: "shift", word: "Shift", glyph: "\u{21e7}" }, // ⇧
    Mod { letter: 'A', target: "alt", word: "Alt", glyph: "\u{2325}" }, // ⌥
    Mod { letter: 'M', target: "meta", word: "Meta", glyph: "\u{25c7}" }, // ◇
    Mod { letter: 'G', target: "altgr", word: "AltGr", glyph: "AltGr" },
];

impl Mod {
    /// Look up a modifier by its chord-prefix letter (`'C'` → Ctrl).
    pub fn from_letter(letter: char) -> Option<&'static Mod> {
        MODS.iter().find(|m| m.letter == letter)
    }

    /// Look up a modifier by a keyd target name, folding the left/right twins
    /// keyd emits: `leftcontrol`/`rightcontrol` → Ctrl, and crucially
    /// `rightalt` → AltGr (keyd's `MOD_ALT_GR`), *not* Alt.
    pub fn from_target(name: &str) -> Option<&'static Mod> {
        let letter = match name {
            "control" | "leftcontrol" | "rightcontrol" => 'C',
            "shift" | "leftshift" | "rightshift" => 'S',
            "alt" | "leftalt" => 'A',
            "meta" | "leftmeta" | "rightmeta" => 'M',
            "altgr" | "rightalt" => 'G',
            _ => return None,
        };
        Self::from_letter(letter)
    }
}

/// True if `c` is one of keyd's five chord-prefix letters (`C S A M G`).
pub fn is_prefix_letter(c: char) -> bool {
    MODS.iter().any(|m| m.letter == c)
}

#[cfg(test)]
mod tests {
    use super::{is_prefix_letter, Mod};

    #[test]
    fn from_target_folds_left_right_twins() {
        assert_eq!(Mod::from_target("shift").unwrap().letter, 'S');
        assert_eq!(Mod::from_target("leftalt").unwrap().letter, 'A');
        assert_eq!(Mod::from_target("meta").unwrap().letter, 'M');
        assert_eq!(Mod::from_target("rightalt").unwrap().letter, 'G');
        assert!(Mod::from_target("nonsense").is_none());
    }

    #[test]
    fn is_prefix_letter_only_for_mod_letters() {
        assert!(is_prefix_letter('C'));
        assert!(is_prefix_letter('G'));
        assert!(!is_prefix_letter('Z'));
        assert!(!is_prefix_letter('a'));
    }
}
