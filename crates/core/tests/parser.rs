//! Port of the original `tests/test_parser.py`. Each test mirrors a pytest case
//! so the Rust parser is provably at parity with the Python one.

use keydviz_core::model::{Hold, HoldKind};
use keydviz_core::{base_legend, layout_for, parse_text, prettify};

fn hold(key: &str, target: &str, kind: HoldKind, tap: Option<&str>) -> Hold {
    Hold {
        key: key.to_string(),
        target: target.to_string(),
        kind,
        tap: tap.map(str::to_string),
    }
}

// ----------------------------------------------------------------------- parse
#[test]
fn lettermod_tap_and_hold() {
    let cfg = parse_text("[main]\nf = lettermod(nav, f, 150, 200)\n");
    assert_eq!(cfg.holds, vec![hold("f", "nav", HoldKind::Layer, Some("f"))]);
}

#[test]
fn lettermod_to_modifier() {
    let cfg = parse_text("[main]\nk = lettermod(control, k, 150, 200)\n");
    assert_eq!(cfg.holds, vec![hold("k", "control", HoldKind::Mod, Some("k"))]);
}

#[test]
fn overload_tap_differs_from_key() {
    // capslock taps Esc, holds Ctrl
    let cfg = parse_text("[main]\ncapslock = overload(control, esc)\n");
    assert_eq!(cfg.holds, vec![hold("capslock", "control", HoldKind::Mod, Some("esc"))]);
}

#[test]
fn layer_is_pure_modifier_no_tap() {
    let cfg = parse_text("[main]\ncapslock = layer(control)\n");
    assert_eq!(cfg.holds, vec![hold("capslock", "control", HoldKind::Mod, None)]);
}

#[test]
fn plain_remap() {
    let cfg = parse_text("[main]\nleftcontrol = capslock\n");
    assert_eq!(cfg.remaps, vec![("leftcontrol".to_string(), "capslock".to_string())]);
    assert!(cfg.holds.is_empty());
}

#[test]
fn toggle_chord() {
    let cfg = parse_text("[main]\nleftshift+rightshift = toggle(game)\n");
    assert_eq!(cfg.chords, vec![("leftshift+rightshift".to_string(), "game".to_string())]);
}

#[test]
fn layer_section_overrides() {
    let cfg = parse_text("[nav]\nh = left\nj = down\n");
    let nav = cfg.layer("nav").expect("nav layer");
    assert_eq!(nav.get("h"), Some("left"));
    assert_eq!(nav.get("j"), Some("down"));
    assert_eq!(nav.keys.len(), 2);
}

#[test]
fn ids_collected() {
    let cfg = parse_text("[ids]\n04fe:0021\n04fe:0202\n");
    assert_eq!(cfg.ids, vec!["04fe:0021".to_string(), "04fe:0202".to_string()]);
}

#[test]
fn full_line_comments_and_blanks_ignored() {
    let text = "# a comment\n\n[main]\n# another\nf = lettermod(nav, f, 1, 2)\n";
    let cfg = parse_text(text);
    assert_eq!(cfg.holds.len(), 1);
}

#[test]
fn empty_layer_section_registered() {
    let cfg = parse_text("[sym]\n");
    assert!(cfg.layer("sym").is_some());
}

#[test]
fn inline_hash_is_literal_not_a_comment() {
    // keyd only treats '#' as a comment at line start; a '#' in a value stays.
    let cfg = parse_text("[main]\n3 = #\n");
    assert_eq!(cfg.remaps, vec![("3".to_string(), "#".to_string())]);
}

#[test]
fn modifier_qualified_layer_merges_into_base() {
    // `[nav:C]` is the nav layer's Control-held bindings — they belong to nav,
    // not to whatever section preceded it.
    let cfg = parse_text("[nav]\nh = left\n[nav:C]\nh = home\n");
    let nav = cfg.layer("nav").expect("nav layer");
    assert_eq!(nav.keys.len(), 2);
    assert!(cfg.layer("nav:C").is_none());
}

#[test]
fn global_and_aliases_are_not_layers() {
    // keyd special-cases [global] and [aliases]; they must not become boards.
    let text = "[ids]\n*\n[global]\noverload_tap_timeout = 200\n\
                [aliases]\ncapslock = esc\n[nav]\nh = left\n";
    let cfg = parse_text(text);
    assert!(cfg.layer("global").is_none(), "[global] should not be a layer");
    assert!(cfg.layer("aliases").is_none(), "[aliases] should not be a layer");
    // a real layer alongside them still parses
    assert!(cfg.layer("nav").is_some());
    assert_eq!(cfg.layer("nav").unwrap().get("h"), Some("left"));
}

#[test]
fn composite_layer_header_parsed() {
    // `[a+b]` is a real (composite) layer name; bindings must not leak into the
    // previous section.
    let cfg = parse_text("[main]\nx = y\n[fn+nav]\nq = w\n");
    let composite = cfg.layer("fn+nav").expect("composite layer");
    assert_eq!(composite.get("q"), Some("w"));
    assert_eq!(cfg.remaps, vec![("x".to_string(), "y".to_string())]);
}

// -------------------------------------------------------------------- prettify
#[test]
fn prettify_cases() {
    let cases = [
        ("S-9", "("),
        ("S-0", ")"),
        ("S-minus", "_"),
        ("S-leftbrace", "{"),
        ("S-rightbrace", "}"),
        ("C-left", "\u{2303}\u{2190}"), // ⌃←
        ("C-right", "\u{2303}\u{2192}"), // ⌃→
        ("leftbrace", "["),
        ("backspace", "\u{232b}"), // ⌫
        ("esc", "Esc"),
        ("a", "A"),
    ];
    for (value, expected) in cases {
        assert_eq!(prettify(value), expected, "prettify({value:?})");
    }
}

#[test]
fn base_legend_basics() {
    assert_eq!(base_legend("a"), "A");
    assert_eq!(base_legend("1"), "1");
    assert_eq!(base_legend("esc"), "Esc");
    assert_eq!(base_legend("space"), "Space");
}

// ---------------------------------------------------------------------- layout
#[test]
fn layout_selection() {
    // HHKB profile, and its top-left key is Esc (vs ANSI-60's grave).
    let (geom, prof) = layout_for("/etc/keyd/hhkb.conf");
    assert_eq!(prof, "HHKB");
    assert_eq!(geom.slots[0].key.as_deref(), Some("esc"));

    let (geom, prof) = layout_for("/etc/keyd/laptop.conf");
    assert_eq!(prof, "ANSI 60%");
    assert_eq!(geom.slots[0].key.as_deref(), Some("grave"));
}
