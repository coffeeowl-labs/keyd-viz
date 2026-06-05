//! Tests for the semantic render model ([`Sheet`]/[`Board`]/[`KeyCap`]).
//! Verifies that keyd bindings translate into the right cap labels, badges,
//! ghosts, colors, and states — the visual semantics the GUI depends on.

use keydviz_core::board::KeyState;
use keydviz_core::{layout_for, parse_text, Sheet};

/// Find a cap by physical key index helper: returns the cap whose label/ghost we
/// can assert on. We locate caps positionally via the known ANSI-60 / HHKB rows.
fn find_cap<'a>(board: &'a keydviz_core::Board, predicate: impl Fn(&KeyCapView) -> bool) -> Option<KeyCapView<'a>> {
    for cap in &board.keys {
        let view = KeyCapView { cap };
        if predicate(&view) {
            return Some(view);
        }
    }
    None
}

struct KeyCapView<'a> {
    cap: &'a keydviz_core::KeyCap,
}

#[test]
fn laptop_base_momentary_and_remap() {
    // laptop.conf: capslock = layer(control) [momentary mod, no tap];
    //              leftcontrol = capslock [plain remap].
    let text = "[ids]\n0b05:19b6\n[main]\ncapslock = layer(control)\nleftcontrol = capslock\n";
    let cfg = parse_text(text);
    let (geom, profile) = layout_for("/etc/keyd/laptop.conf");
    let sheet = Sheet::build(&cfg, "laptop.conf", &geom, profile);

    assert_eq!(sheet.profile, "ANSI 60%");
    let base = &sheet.boards[0];
    assert!(base.is_base);

    // capslock: pure momentary modifier -> emphasized "Ctrl", ghost "Caps",
    // control accent, no tap badge.
    let caps = find_cap(base, |v| v.cap.ghost == "Caps").expect("capslock cap");
    assert_eq!(caps.cap.label, "Ctrl");
    assert!(caps.cap.emphasized);
    assert_eq!(caps.cap.accent, "#ff6b6b");
    assert!(caps.cap.badge_left.is_none());

    // leftcontrol: plain remap to capslock -> emphasized "Caps", ghost "Ctrl",
    // orange remap accent.
    let lc = find_cap(base, |v| v.cap.ghost == "Ctrl" && v.cap.label == "Caps")
        .expect("leftcontrol cap");
    assert!(lc.cap.emphasized);
    assert_eq!(lc.cap.accent, "#ffb454");
    // Glow matches what the key emits: leftcontrol -> capslock, so the cap's match
    // key is the output "capslock", not the physical "leftcontrol".
    assert_eq!(lc.cap.key, "capslock");
}

#[test]
fn hhkb_sheet_structure_and_badges() {
    let cfg = parse_text(include_str!("../../../examples/hhkb.conf"));
    let (geom, profile) = layout_for("hhkb.conf");
    let sheet = Sheet::build(&cfg, "hhkb.conf", &geom, profile);

    // base + nav, num, sym, shift, game = 6 boards, game last.
    let titles: Vec<&str> = sheet.boards.iter().map(|b| b.title.as_str()).collect();
    assert_eq!(titles, ["Base layer", "NAV", "NUM", "SYM", "SHIFT", "GAME"]);
    assert!(sheet.boards.last().unwrap().title == "GAME");

    let base = &sheet.boards[0];
    // f = lettermod(nav, f, ...) -> tap "F" with a "↓nav" hold badge.
    let f = find_cap(base, |v| v.cap.label == "F" && v.cap.badge_left.is_some())
        .expect("f cap");
    let badge = f.cap.badge_left.as_ref().unwrap();
    assert_eq!(badge.text, "\u{2193}nav");
    assert_eq!(badge.color, "#4aa3ff");

    // Both shifts carry the ⇧⇧ chord marker (toggle game).
    let shift_cap = find_cap(base, |v| {
        v.cap.badge_right.as_ref().map(|b| b.text.as_str()) == Some("\u{21e7}\u{21e7}")
    });
    assert!(shift_cap.is_some(), "expected a ⇧⇧ chord badge on base");

    // The NAV board marks F as the held key.
    let nav = sheet.boards.iter().find(|b| b.title == "NAV").unwrap();
    assert_eq!(nav.how, "hold F");
    let held = find_cap(nav, |v| matches!(v.cap.state, KeyState::Hold)).expect("held key");
    assert_eq!(held.cap.label, "F");
    assert_eq!(held.cap.badge_left.as_ref().unwrap().text, "HOLD");

    // GAME board is a toggle, hinted by the chord.
    let game = sheet.boards.iter().find(|b| b.title == "GAME").unwrap();
    assert!(game.how.starts_with("toggle:"));
}

#[test]
fn layer_remap_glows_on_output_key() {
    // num layer binds `j = 4`. keyd reports the *output* keysym `4` when you press
    // physical j, so the j-cap must carry the output key "4" to glow — not "j".
    let cfg = parse_text(include_str!("../../../examples/hhkb.conf"));
    let (geom, profile) = layout_for("hhkb.conf");
    let sheet = Sheet::build(&cfg, "hhkb.conf", &geom, profile);
    let num = sheet.boards.iter().find(|b| b.title == "NUM").unwrap();

    // The remapped cap shows "4" and matches the keyd output "4".
    let four = find_cap(num, |v| v.cap.emphasized && v.cap.label == "4").expect("num j->4 cap");
    assert_eq!(four.cap.key, "4");

    // Symbol remaps: the config uses keyd's alt name (`p = equal`), but monitor prints
    // the primary `=`. The cap must carry the primary so it glows. Likewise `slash = dot`
    // emits `.` and `semicolon = minus` emits `-` — and no cap keeps the alt name.
    let p = find_cap(num, |v| v.cap.ghost == "P" && v.cap.emphasized).expect("num p->equal cap");
    assert_eq!(p.cap.key, "=");
    assert!(find_cap(num, |v| v.cap.key == "-").is_some(), "semicolon->minus glows on -");
    assert!(find_cap(num, |v| v.cap.key == ".").is_some(), "slash->dot glows on .");
    for alt in ["equal", "minus", "dot", "semicolon"] {
        assert!(find_cap(num, |v| v.cap.key == alt).is_none(), "{alt} should canonicalise");
    }

    // Base-layer passthrough also canonicalises: the `=` cap glows on monitor's `=`.
    let base = &sheet.boards[0];
    assert!(find_cap(base, |v| v.cap.key == "=").is_some(), "base = key glows on =");
    assert!(find_cap(base, |v| v.cap.key == "equal").is_none());

    // The key you hold to reach the layer emits nothing, so it never glows.
    let held = find_cap(num, |v| matches!(v.cap.state, KeyState::Hold)).expect("held key");
    assert_eq!(held.cap.key, "");
}

#[test]
fn chord_remaps_emit_full_keysym_set() {
    // Remaps that emit a modifier chord must carry the whole set keyd reports, so the
    // glow matches when all of those keysyms are held — and a more-specific cap can
    // suppress the plain Ctrl / arrow / digit caps it subsumes (handled app-side).
    let cfg = parse_text(include_str!("../../../examples/hhkb.conf"));
    let (geom, profile) = layout_for("hhkb.conf");
    let sheet = Sheet::build(&cfg, "hhkb.conf", &geom, profile);

    // nav: `n = C-left` -> leftcontrol + left (canonical modifier keysym + key).
    let nav = sheet.boards.iter().find(|b| b.title == "NAV").unwrap();
    let n = find_cap(nav, |v| v.cap.ghost == "N" && v.cap.emphasized).expect("nav n->C-left cap");
    assert_eq!(n.cap.key, "leftcontrol+left");

    // sym: `j = S-9` -> leftshift + 9; `m = S-leftbrace` -> leftshift + [ (alt name
    // canonicalised inside the chord); `u = leftbrace` -> plain [.
    let sym = sheet.boards.iter().find(|b| b.title == "SYM").unwrap();
    let j = find_cap(sym, |v| v.cap.ghost == "J" && v.cap.emphasized).expect("sym j->S-9 cap");
    assert_eq!(j.cap.key, "leftshift+9");
    let m = find_cap(sym, |v| v.cap.ghost == "M" && v.cap.emphasized).expect("sym m cap");
    assert_eq!(m.cap.key, "leftshift+[");
    let u = find_cap(sym, |v| v.cap.ghost == "U" && v.cap.emphasized).expect("sym u cap");
    assert_eq!(u.cap.key, "[");
}
