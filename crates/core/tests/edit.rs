//! Round-trip tests for the Edit Mode line model (design doc §5.1, tier T0).
//!
//! The model makes `serialize(parse(f)) == f` identity-by-construction; these tests
//! are the soundness net that keeps it that way — over the real-config corpus, the
//! EOL edge cases `str::lines()` would have eaten, and a deterministic fuzz sweep.

use keydviz_core::edit::{round_trips, EditConfig, EntryKind};

/// Every committed example config round-trips byte-for-byte.
#[test]
fn corpus_round_trips() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "conf") {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(round_trips(&text), "round-trip failed for {}", path.display());
            checked += 1;
        }
    }
    assert!(checked >= 2, "expected the example corpus, found {checked} file(s)");
}

/// This machine's live configs round-trip too, when present (skips silently on a
/// box with no /etc/keyd — keeps CI hermetic while catching real-world files locally).
#[test]
fn etc_keyd_round_trips_if_present() {
    let Ok(entries) = std::fs::read_dir("/etc/keyd") else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "conf") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                assert!(round_trips(&text), "round-trip failed for {}", path.display());
            }
        }
    }
}

// ------------------------------------------------------------------ EOL fidelity
// The one construct that can silently break round-trip (§5.1): CR/LF state and the
// final-newline distinction.

#[test]
fn eol_edge_cases_round_trip() {
    let cases = [
        "",                                  // empty file
        "\n",                                // single blank line
        "[main]\na = b\n",                   // plain LF
        "[main]\r\na = b\r\n",               // CRLF
        "[main]\na = b",                     // no final newline
        "[main]\r\na = b",                   // CRLF then unterminated last line
        "[main]\na = b\r\nc = d\n",          // mixed EOLs
        "a = b",                             // bare assignment, no section at all
        "[main]\na = b\r",                   // trailing CR, no LF (CR stays in raw)
        "   \n\t\n",                         // whitespace-only lines
        "# comment only\n",                  //
        "[ids]\n0123:4567\n*\n",             // valueless entries
        "[main]\n= = a\n==x\n",              // '=' key special cases
        "[main]\nkey =\n",                   // empty value
        "[foo\n",                            // unterminated bracket (an entry!)
        "[a]b]\nx = y\n",                    // ']' inside a section name
        "  [main]  \na = b\n",               // header with surrounding whitespace
    ];
    for case in cases {
        assert!(round_trips(case), "round-trip failed for {case:?}");
    }
}

/// Deterministic fuzz: pseudo-random soups of config-ish bytes must round-trip.
/// (Fixed-seed LCG, so failures reproduce.)
#[test]
fn fuzz_round_trips() {
    let alphabet: Vec<char> =
        "abz09_ =+:#[]()\t\r\n-,.*\\/".chars().collect();
    let mut state: u64 = 0x9e3779b97f4a7c15;
    let mut next = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (state >> 33) as usize
    };
    for _ in 0..500 {
        let len = next() % 200;
        let text: String = (0..len).map(|_| alphabet[next() % alphabet.len()]).collect();
        assert!(round_trips(&text), "round-trip failed for {text:?}");
    }
}

// ----------------------------------------------------------------- structure sanity

#[test]
fn real_example_structure() {
    let text = include_str!("../../../examples/hhkb.conf");
    let cfg = EditConfig::parse(text);
    assert!(cfg.section("ids").is_some());
    assert!(cfg.section("main").is_some());
    // Total lines are conserved: every input line lands in exactly one entry.
    let n_lines = text.split('\n').count() - usize::from(text.ends_with('\n'));
    let n_entries = cfg.preamble.len()
        + cfg.sections.iter().map(|s| 1 + s.entries.len()).sum::<usize>();
    assert_eq!(n_entries, n_lines);
}

#[test]
fn comments_and_blanks_are_first_class_entries() {
    let cfg = EditConfig::parse("# top\n\n[main]\n# mid\na = b\n");
    assert!(matches!(cfg.preamble[0].kind, EntryKind::Comment));
    assert!(matches!(cfg.preamble[1].kind, EntryKind::Blank));
    assert!(matches!(cfg.sections[0].entries[0].kind, EntryKind::Comment));
}
