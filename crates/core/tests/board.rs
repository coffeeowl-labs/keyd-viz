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
