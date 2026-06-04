//! The curated library of physical layouts, authored compactly as rows of
//! `(keyd-key-name, width-in-units)` and expanded to positioned [`Geometry`].
//!
//! keyd configs carry no physical geometry, so these positions are supplied here
//! (see ROADMAP §4.5 — the QMK importer loads `info.json` for arbitrary boards). The
//! row form is just an authoring convenience for standard staggered boards; the widths
//! alone encode the stagger (a 1.5u Tab pushes `q` right), and [`Geometry::from_rows`]
//! turns them into absolute coordinates the renderer consumes.

use crate::geometry::Geometry;

/// One physical row: a key-name and its width in standard key units.
pub type Row = &'static [(&'static str, f32)];
/// A compact, row-authored layout: an ordered list of rows.
pub type Layout = &'static [Row];

/// HHKB 60% layout (keyed by keyd key-names).
pub static HHKB: Layout = &[
    &[
        ("esc", 1.0), ("1", 1.0), ("2", 1.0), ("3", 1.0), ("4", 1.0), ("5", 1.0),
        ("6", 1.0), ("7", 1.0), ("8", 1.0), ("9", 1.0), ("0", 1.0), ("minus", 1.0),
        ("equal", 1.0), ("backslash", 1.0), ("grave", 1.0),
    ],
    &[
        ("tab", 1.5), ("q", 1.0), ("w", 1.0), ("e", 1.0), ("r", 1.0), ("t", 1.0),
        ("y", 1.0), ("u", 1.0), ("i", 1.0), ("o", 1.0), ("p", 1.0), ("leftbrace", 1.0),
        ("rightbrace", 1.0), ("backspace", 1.5),
    ],
    &[
        ("leftcontrol", 1.75), ("a", 1.0), ("s", 1.0), ("d", 1.0), ("f", 1.0),
        ("g", 1.0), ("h", 1.0), ("j", 1.0), ("k", 1.0), ("l", 1.0), ("semicolon", 1.0),
        ("apostrophe", 1.0), ("enter", 2.25),
    ],
    &[
        ("leftshift", 2.25), ("z", 1.0), ("x", 1.0), ("c", 1.0), ("v", 1.0),
        ("b", 1.0), ("n", 1.0), ("m", 1.0), ("comma", 1.0), ("dot", 1.0),
        ("slash", 1.0), ("rightshift", 1.75), ("fn", 1.0),
    ],
    &[
        ("leftalt", 1.5), ("leftmeta", 1.0), ("space", 7.0), ("rightmeta", 1.0),
        ("rightalt", 1.5),
    ],
];

/// ANSI 60% layout (keyed by keyd key-names).
pub static ANSI60: Layout = &[
    &[
        ("grave", 1.0), ("1", 1.0), ("2", 1.0), ("3", 1.0), ("4", 1.0), ("5", 1.0),
        ("6", 1.0), ("7", 1.0), ("8", 1.0), ("9", 1.0), ("0", 1.0), ("minus", 1.0),
        ("equal", 1.0), ("backspace", 2.0),
    ],
    &[
        ("tab", 1.5), ("q", 1.0), ("w", 1.0), ("e", 1.0), ("r", 1.0), ("t", 1.0),
        ("y", 1.0), ("u", 1.0), ("i", 1.0), ("o", 1.0), ("p", 1.0), ("leftbrace", 1.0),
        ("rightbrace", 1.0), ("backslash", 1.5),
    ],
    &[
        ("capslock", 1.75), ("a", 1.0), ("s", 1.0), ("d", 1.0), ("f", 1.0),
        ("g", 1.0), ("h", 1.0), ("j", 1.0), ("k", 1.0), ("l", 1.0), ("semicolon", 1.0),
        ("apostrophe", 1.0), ("enter", 2.25),
    ],
    &[
        ("leftshift", 2.25), ("z", 1.0), ("x", 1.0), ("c", 1.0), ("v", 1.0),
        ("b", 1.0), ("n", 1.0), ("m", 1.0), ("comma", 1.0), ("dot", 1.0),
        ("slash", 1.0), ("rightshift", 2.75),
    ],
    &[
        ("leftcontrol", 1.25), ("leftmeta", 1.25), ("leftalt", 1.25), ("space", 6.25),
        ("rightalt", 1.25), ("rightmeta", 1.25), ("menu", 1.25), ("rightcontrol", 1.25),
    ],
];

/// Pick a physical layout from a config file's name: HHKB for `*hhkb*`, else ANSI-60.
/// Returns the positioned geometry and a human-readable profile name.
///
/// This name-based guess is the interim until the user can pick/persist a layout per
/// keyboard (the irreducible manual step — keyd exposes no board identity).
pub fn layout_for(path: &str) -> (Geometry, &'static str) {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    if name.contains("hhkb") {
        (Geometry::from_rows(HHKB), "HHKB 60%")
    } else {
        (Geometry::from_rows(ANSI60), "ANSI 60%")
    }
}
