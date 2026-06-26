//! Component 4 — board-model snapshots (docs/testing-harness-design.md).
//!
//! Snapshots a SEMANTIC PROJECTION of the rendered `Sheet` — the fields that
//! determine what a key shows and how it's emphasized (label/ghost/accent/badges/
//! state/glow-key), sorted by physical key. Geometry floats (`x/y/width/...`) are
//! deliberately omitted: they're covered by `tests/board.rs` and would bury the
//! one semantic line that matters under thousands of noise lines on any layout
//! tweak. Review intended changes with `cargo insta review`.

use keydviz_core::{layout_for, parse_text, Board, Sheet};
use serde::Serialize;

#[derive(Serialize)]
struct CapView {
    phys: String,
    /// The keysym the cap glows on (output, not physical key).
    key: String,
    label: String,
    ghost: String,
    emphasized: bool,
    accent: String,
    state: String,
    badges: Vec<String>,
}

#[derive(Serialize)]
struct BoardView {
    title: String,
    is_base: bool,
    accent: String,
    how: String,
    hint: String,
    caps: Vec<CapView>,
}

fn project_board(b: &Board) -> BoardView {
    let mut caps: Vec<CapView> = b
        .keys
        .iter()
        .map(|c| {
            let mut badges = vec![];
            if let Some(bl) = &c.badge_left {
                badges.push(format!("L {} {}", bl.text, bl.color));
            }
            if let Some(br) = &c.badge_right {
                badges.push(format!("R {} {}", br.text, br.color));
            }
            CapView {
                phys: c.phys.clone(),
                key: c.key.clone(),
                label: c.label.clone(),
                ghost: c.ghost.clone(),
                emphasized: c.emphasized,
                accent: c.accent.clone(),
                state: format!("{:?}", c.state),
                badges,
            }
        })
        .collect();
    // Stable, geometry-independent order so reordering caps never churns the diff.
    caps.sort_by(|a, b| a.phys.cmp(&b.phys).then(a.key.cmp(&b.key)));
    BoardView {
        title: b.title.clone(),
        is_base: b.is_base,
        accent: b.accent.clone(),
        how: b.how.clone(),
        hint: b.hint.clone(),
        caps,
    }
}

fn project(sheet: &Sheet) -> Vec<BoardView> {
    sheet.boards.iter().map(project_board).collect()
}

fn sheet_for(text: &str, path: &str) -> Sheet {
    let cfg = parse_text(text);
    let (geom, profile) = layout_for(path);
    Sheet::build(&cfg, path, &geom, profile)
}

#[test]
fn snapshot_hhkb_board() {
    let sheet = sheet_for(include_str!("../../../examples/hhkb.conf"), "hhkb.conf");
    insta::assert_yaml_snapshot!("hhkb", project(&sheet));
}

#[test]
fn snapshot_laptop_board() {
    let sheet = sheet_for(include_str!("../../../examples/laptop.conf"), "laptop.conf");
    insta::assert_yaml_snapshot!("laptop", project(&sheet));
}
