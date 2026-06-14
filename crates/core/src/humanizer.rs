//! Plain-English descriptions of keyd binding values.
//!
//! The raw keyd syntax (`lettermod(nav, f, 150, 200)`, `toggle(game)`, `macro(...)`) is
//! load-bearing in exactly the wrong places — the tap/hold editor headline and the
//! apply-confirmation diff — where a first-timer (or even a QMK power user new to keyd)
//! can't tell what a change actually does. This turns a binding into one scannable line,
//! e.g. `Tap F → F · Hold F → nav layer`. Best-effort: anything it can't model falls back
//! to the raw value, so it never lies, only sometimes stays terse.
//!
//! It reuses the structured parsers (`TapHold`, `Macro`) and `base_legend` rather than
//! re-parsing strings, so it can't drift from how those constructs are actually read.

use crate::macros::{Macro, MacroToken};
use crate::parser::parse_fn;
use crate::prettify::base_legend;
use crate::taphold::TapHold;

/// One-line plain-English description of what `rhs` does when bound to physical `key`.
/// `key` lets the sentence name the key ("Tap F → …"); pass `""` to omit the subject.
pub fn humanize(key: &str, rhs: &str) -> String {
    let k = base_legend(key);
    let subj = if k.is_empty() { String::new() } else { format!("{k} → ") };
    let rhs = rhs.trim();

    // Tap/hold (lettermod / overload / overloadt / overloadt2 / momentary layer).
    if let Some(th) = TapHold::parse(key, rhs) {
        let hold = hold_phrase(&th.target);
        return match &th.tap {
            Some(tap) => format!("Tap {k} → {} \u{00b7} Hold {k} → {hold}", action(tap)),
            None => format!("Hold {k} → {hold}"),
        };
    }

    // Layer / daemon-state calls keyd offers besides tap-hold.
    if let Some((name, args)) = parse_fn(rhs) {
        if let Some(body) = call_phrase(name, &args) {
            return format!("{subj}{body}");
        }
    }

    // Macro: an ordered list of typed steps.
    if let Some(m) = Macro::parse(rhs) {
        return format!("{subj}{}", macro_phrase(&m));
    }

    // command() runs a shell command as the keyd user — keep that explicit, never terse.
    if rhs.starts_with("command(") {
        return format!("{subj}runs a shell command");
    }
    if rhs == "noop" {
        return format!("{subj}does nothing");
    }

    // Plain key or modifier combo (a, escape, C-a, S-9).
    format!("{subj}{}", action(rhs))
}

/// A hold target → phrase: a modifier reads as itself, a layer as "<name> layer".
fn hold_phrase(target: &str) -> String {
    match mod_word(target) {
        Some(w) => w.to_string(),
        None => format!("{target} layer"),
    }
}

/// A tap action / plain remap value → phrase: a modifier combo (`C-a`) or a key legend.
fn action(value: &str) -> String {
    combo(value).unwrap_or_else(|| base_legend(value))
}

/// Render a keyd modifier combo (`C-S-a`) as `Ctrl+Shift+A`, or `None` if not a combo.
/// The leading segments must all be single mod letters; the final segment is the key.
fn combo(value: &str) -> Option<String> {
    let segs: Vec<&str> = value.split('-').collect();
    if segs.len() < 2 {
        return None; // no modifier prefix
    }
    let (mods, key) = segs.split_at(segs.len() - 1);
    let mut parts: Vec<&str> = Vec::with_capacity(mods.len());
    for m in mods {
        parts.push(mod_letter(m)?); // any non-mod prefix → not a clean combo
    }
    let key = key[0];
    if key.is_empty() {
        return None;
    }
    Some(format!("{}+{}", parts.join("+"), base_legend(key)))
}

/// keyd modifier key name (`control`) → display word, for hold targets.
fn mod_word(name: &str) -> Option<&'static str> {
    crate::mods::Mod::from_target(name).map(|m| m.word)
}

/// keyd combo modifier letter (`C`) → display word.
fn mod_letter(letter: &str) -> Option<&'static str> {
    let mut chars = letter.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None; // a modifier letter is exactly one char
    }
    crate::mods::Mod::from_letter(c).map(|m| m.word)
}

/// Describe the non-tap-hold keyd calls (`toggle(game)` → "toggle game layer").
fn call_phrase(name: &str, args: &[&str]) -> Option<String> {
    let arg = args.first().copied().unwrap_or("");
    Some(match name {
        "toggle" | "toggle2" => format!("toggle {arg} layer on/off"),
        "oneshot" => format!("one-shot {arg} layer (next key only)"),
        "swap" | "swap2" => format!("swap to {arg} layer"),
        "layer" => format!("{arg} layer while held"),
        "setlayout" => format!("set layout to {arg}"),
        "clear" => "clear active layers".to_string(),
        "clearm" => format!("clear, then {arg} layer while held"),
        _ => return None,
    })
}

/// Describe a macro as its steps: `type "hi", wait 100ms, Ctrl+Enter (repeats while held)`.
fn macro_phrase(m: &Macro) -> String {
    let mut steps: Vec<String> = m.tokens.iter().map(macro_step).collect();
    if steps.is_empty() {
        steps.push("(empty macro)".to_string());
    }
    let mut s = steps.join(", ");
    if m.repeat.is_some() {
        s.push_str(" (repeats while held)");
    }
    s
}

fn macro_step(tok: &MacroToken) -> String {
    match tok {
        MacroToken::Key(k) => base_legend(k),
        MacroToken::Delay(n) => format!("wait {n}ms"),
        MacroToken::Text(t) => format!("type \u{201c}{t}\u{201d}"),
        MacroToken::Chord { mods, keys } => {
            let mut parts: Vec<String> =
                mods.iter().filter_map(|c| mod_letter(&c.to_string()).map(String::from)).collect();
            parts.extend(keys.iter().map(|k| base_legend(k)));
            parts.join("+")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lettermod_tap_and_hold() {
        assert_eq!(humanize("f", "lettermod(nav, f, 150, 200)"), "Tap F → F · Hold F → nav layer");
    }

    #[test]
    fn home_row_mod_hold_reads_as_modifier() {
        assert_eq!(humanize("k", "lettermod(control, k, 150, 200)"), "Tap K → K · Hold K → Ctrl");
    }

    #[test]
    fn overload_short_form_defaults_tap_to_key() {
        assert_eq!(humanize("a", "overload(sym, a)"), "Tap A → A · Hold A → sym layer");
    }

    #[test]
    fn momentary_layer_has_no_tap() {
        assert_eq!(humanize("f", "layer(nav)"), "Hold F → nav layer");
    }

    #[test]
    fn toggle_layer() {
        assert_eq!(humanize("g", "toggle(game)"), "G → toggle game layer on/off");
    }

    #[test]
    fn oneshot_layer() {
        assert_eq!(humanize("a", "oneshot(nav)"), "A → one-shot nav layer (next key only)");
    }

    #[test]
    fn modifier_combo_remap() {
        assert_eq!(humanize("n", "C-left"), "N → Ctrl+←");
        assert_eq!(humanize("x", "C-S-a"), "X → Ctrl+Shift+A");
    }

    #[test]
    fn plain_key_remap() {
        assert_eq!(humanize("capslock", "esc"), "Caps → Esc");
        assert_eq!(humanize("h", "left"), "H → ←");
    }

    #[test]
    fn command_is_always_explicit() {
        assert_eq!(humanize("f1", "command(systemctl suspend)"), "f1 → runs a shell command");
    }

    #[test]
    fn noop() {
        assert_eq!(humanize("a", "noop"), "A → does nothing");
    }

    #[test]
    fn macro_steps() {
        assert_eq!(
            humanize("k", "macro(C-t 100ms google.com enter)"),
            "K → Ctrl+T, wait 100ms, type “google.com”, ⏎",
        );
    }

    #[test]
    fn no_subject_when_key_empty() {
        assert_eq!(humanize("", "toggle(game)"), "toggle game layer on/off");
    }
}
