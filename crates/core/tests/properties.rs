//! Property-based tests — Component 2 of `docs/testing-harness-design.md`.
//!
//! A grammar-aware generator emits realistic, mostly-keyd-valid configs and the
//! pure invariants that must hold for ANY config are asserted:
//!   - CST round-trip identity (the byte-faithful line model),
//!   - `parse -> derive -> Sheet::build` never panics (totality).
//!
//! Two extra strategies stress the only places round-trip can actually break:
//!   - `arb_messy_text` — lines joined with mixed `\n`/`\r\n`/lone-`\r` and an
//!     optional missing final newline,
//!   - `arb_bytes_text` — arbitrary `char` sequences (a *shrinking* successor to
//!     the fixed-seed LCG fuzz in `tests/edit.rs`).
//!
//! The keyd differential-oracle property (generated configs are keyd-valid) lives
//! in `crates/app/tests/keyd_oracle.rs`, which can reach `EditSession` + keyd.
//!
//! Determinism note: gating CI should run bounded (`PROPTEST_CASES=64`); the
//! committed `.proptest-regressions` replays any past failure deterministically.
//! High-case entropy exploration is for local/nightly runs.

use keydviz_core::parser::derive;
use keydviz_core::{layout_for, parse_text, round_trips, EditConfig, Sheet};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Generator
// ---------------------------------------------------------------------------

const KEYS: &[&str] = &[
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "1", "2", "3", "0",
    "space", "tab", "enter", "esc", "backspace", "capslock", "leftshift", "leftcontrol", "leftalt",
    "leftmeta", "up", "down", "left", "right",
];

const LAYER_NAMES: &[&str] = &["nav", "num", "sym", "fnl", "media", "mouse"];

fn arb_key() -> impl Strategy<Value = String> {
    prop::sample::select(KEYS).prop_map(String::from)
}

/// An RHS action referencing one of the already-defined `layers` (so generated
/// configs stay orphan-free and mostly keyd-valid). Covers every binding shape
/// the editor classifies: remap, noop, layer/toggle/oneshot, overload tap/hold,
/// and a small macro.
fn arb_action(layers: Vec<String>) -> impl Strategy<Value = String> {
    let pick = prop::sample::select(layers);
    prop_oneof![
        4 => arb_key(),
        1 => Just("noop".to_string()),
        2 => pick.clone().prop_map(|l| format!("layer({l})")),
        2 => pick.clone().prop_map(|l| format!("toggle({l})")),
        2 => pick.clone().prop_map(|l| format!("oneshot({l})")),
        2 => (pick, arb_key()).prop_map(|(l, k)| format!("overload({l}, {k})")),
        2 => prop::collection::vec(arb_key(), 1..4).prop_map(|ks| format!("macro({})", ks.join(" "))),
    ]
}

/// A full, orphan-free keyd config: `[ids]` wildcard, `[main]`, 1..=4 normal
/// layer sections, and (sometimes) a composite `[a+b]` overlay of two of them.
fn arb_config() -> impl Strategy<Value = String> {
    prop::sample::subsequence(LAYER_NAMES.to_vec(), 1..=4)
        .prop_flat_map(|picked| {
            let layers: Vec<String> = picked.iter().map(|s| s.to_string()).collect();
            let n = layers.len();
            let section = prop::collection::vec((arb_key(), arb_action(layers.clone())), 0..6);
            // main + one body per layer.
            (
                Just(layers),
                prop::collection::vec(section, n + 1..=n + 1),
                any::<bool>(), // include a composite overlay?
            )
        })
        .prop_map(|(layers, bodies, composite)| {
            let mut out = String::from("[ids]\n*\n");
            out.push_str("\n[main]\n");
            for (k, v) in &bodies[0] {
                out.push_str(&format!("{k} = {v}\n"));
            }
            for (i, layer) in layers.iter().enumerate() {
                out.push_str(&format!("\n[{layer}]\n"));
                for (k, v) in &bodies[i + 1] {
                    out.push_str(&format!("{k} = {v}\n"));
                }
            }
            if composite && layers.len() >= 2 {
                out.push_str(&format!("\n[{}+{}]\n", layers[0], layers[1]));
            }
            out
        })
}

/// Config-ish lines joined with mixed EOLs and an optional missing final newline —
/// the exact shapes `str::lines()` silently corrupts, where the CST splitter can
/// actually break (as opposed to clean content, which round-trips by construction).
fn arb_messy_text() -> impl Strategy<Value = String> {
    let line = prop_oneof![
        (arb_key(), arb_key()).prop_map(|(k, v)| format!("{k} = {v}")),
        Just("[main]".to_string()),
        Just("[nav]".to_string()),
        Just("# keyd-viz: a = Alpha".to_string()),
        Just("# comment".to_string()),
        Just(String::new()),
        Just("  spaced = thing  ".to_string()),
    ];
    let eol = prop_oneof![Just("\n"), Just("\r\n"), Just("\r")];
    (prop::collection::vec((line, eol), 0..12), any::<bool>()).prop_map(|(lines, trailing)| {
        let mut s = String::new();
        let len = lines.len();
        for (i, (l, e)) in lines.into_iter().enumerate() {
            s.push_str(&l);
            if i + 1 < len || trailing {
                s.push_str(e);
            }
        }
        s
    })
}

/// Arbitrary `char` sequences — the shrinking successor to the LCG byte-fuzz.
fn arb_bytes_text() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..200).prop_map(|cs| cs.into_iter().collect())
}

// ---------------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------------

// No explicit `cases:` — ProptestConfig::default() honors the PROPTEST_CASES env
// var (256 locally, bounded to 64 in gating CI). A literal here would silence it.
proptest! {
    /// P2-roundtrip + P2-derive-total: a realistic config round-trips byte-exact,
    /// and parsing -> deriving -> building the board never panics. (The round-trip
    /// is near-tautological for this tame generator — the real round-trip teeth are
    /// in messy_text/arbitrary_text below; here the value is the totality half.)
    #[test]
    fn structured_config_is_total(text in arb_config()) {
        prop_assert!(round_trips(&text), "round-trip failed:\n{text:?}");
        let edit = EditConfig::parse(&text);
        let cfg = derive(&edit);
        let (geom, profile) = layout_for("test.conf");
        let _sheet = Sheet::build(&cfg, "test.conf", &geom, profile); // must not panic
        // parse_text is the viewer's entry point; it must agree it can build too.
        let cfg2 = parse_text(&text);
        let _ = Sheet::build(&cfg2, "test.conf", &geom, profile);
    }

    /// Round-trip fidelity across mixed EOLs / unterminated last line.
    #[test]
    fn messy_text_round_trips(text in arb_messy_text()) {
        prop_assert!(round_trips(&text), "round-trip failed:\n{text:?}");
    }

    /// Totality + round-trip on arbitrary char soup (shrinks to a minimal repro).
    #[test]
    fn arbitrary_text_is_total(text in arb_bytes_text()) {
        prop_assert!(round_trips(&text), "round-trip failed:\n{text:?}");
        let cfg = parse_text(&text);
        let (geom, profile) = layout_for("test.conf");
        let _ = Sheet::build(&cfg, "test.conf", &geom, profile); // must not panic
    }
}
