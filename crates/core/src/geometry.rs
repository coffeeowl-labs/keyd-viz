//! Physical keyboard geometry: positioned key slots in key units.
//!
//! keyd configs carry no geometry (ROADMAP §4.5), so physical positions come from
//! here. A [`Geometry`] is a flat list of [`Slot`]s with absolute `x`/`y` (top-left,
//! in key units) plus size and optional rotation — the same model QMK `info.json` and
//! KLE use, and exactly what an arbitrary board (staggered, ortho, split, rotated)
//! needs. Each slot is labeled with the keyd key *name* at that position (or `None`
//! for a decorative/unmapped slot), so keyd bindings overlay onto it by name.
//!
//! Sources that fill this model:
//! - the bundled curated library (authored compactly via [`Geometry::from_rows`]);
//! - a QMK importer (`info.json` geometry zipped index-wise with the default keymap's
//!   keycodes → keyd names) — added later;
//! - user-imported KLE/`info.json` with manual labeling.

/// One physical key position, in key units (1u = one standard 1×1 key).
#[derive(Debug, Clone, PartialEq)] // not Eq: f32 fields
pub struct Slot {
    /// Left edge, in key units from the board's top-left.
    pub x: f32,
    /// Top edge, in key units from the board's top-left.
    pub y: f32,
    /// Width in key units.
    pub w: f32,
    /// Height in key units (2.0 for a vertical ISO-enter, etc.).
    pub h: f32,
    /// Rotation in degrees clockwise, about (`rx`, `ry`). 0 for the common case.
    pub r: f32,
    /// Rotation origin x, in key units (only meaningful when `r != 0`).
    pub rx: f32,
    /// Rotation origin y, in key units.
    pub ry: f32,
    /// The keyd key name at this position (`a`, `leftshift`, …), or `None` when the
    /// slot is decorative or couldn't be mapped to a keyd key.
    pub key: Option<String>,
}

impl Slot {
    /// A plain 1u-tall, unrotated slot.
    pub fn new(x: f32, y: f32, w: f32, key: Option<String>) -> Self {
        Slot { x, y, w, h: 1.0, r: 0.0, rx: 0.0, ry: 0.0, key }
    }
}

/// A full physical keyboard: an unordered set of positioned slots.
#[derive(Debug, Clone, PartialEq)] // not Eq: slots carry f32
pub struct Geometry {
    pub slots: Vec<Slot>,
}

impl Geometry {
    /// Expand the compact row-authoring format — rows of `(keyd-name, width)` — into
    /// absolute positions: `x` accumulates across each row, `y` is the row index. The
    /// widths alone reproduce a real left-aligned, staggered board (a 1.5u Tab pushes
    /// `q` to x=1.5, etc.), which is how the curated standard layouts are authored.
    pub fn from_rows(rows: &[&[(&str, f32)]]) -> Self {
        let mut slots = Vec::new();
        for (row_idx, row) in rows.iter().enumerate() {
            let mut x = 0.0;
            for &(name, w) in *row {
                slots.push(Slot::new(x, row_idx as f32, w, Some(name.to_string())));
                x += w;
            }
        }
        Geometry { slots }
    }

    /// Overall extent `(width, height)` in key units — for sizing the board panel.
    pub fn extent(&self) -> (f32, f32) {
        let w = self.slots.iter().map(|s| s.x + s.w).fold(0.0, f32::max);
        let h = self.slots.iter().map(|s| s.y + s.h).fold(0.0, f32::max);
        (w, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_rows_positions_keys_left_aligned_and_staggered() {
        let rows: &[&[(&str, f32)]] = &[
            &[("esc", 1.0), ("1", 1.0), ("2", 1.0)],
            &[("tab", 1.5), ("q", 1.0), ("w", 1.0)],
        ];
        let g = Geometry::from_rows(rows);
        assert_eq!(g.slots.len(), 6);
        // row 0 runs 0,1,2 at y=0
        assert_eq!(g.slots[0], Slot::new(0.0, 0.0, 1.0, Some("esc".into())));
        assert_eq!(g.slots[2].x, 2.0);
        // row 1 starts at x=0,y=1; the 1.5u Tab pushes q to x=1.5
        assert_eq!(g.slots[3], Slot::new(0.0, 1.0, 1.5, Some("tab".into())));
        assert_eq!(g.slots[4].x, 1.5);
        assert_eq!(g.slots[4].y, 1.0);
    }

    #[test]
    fn extent_spans_widest_row_and_all_rows() {
        let rows: &[&[(&str, f32)]] = &[
            &[("a", 1.0), ("b", 1.0)],
            &[("space", 3.0)],
        ];
        let g = Geometry::from_rows(rows);
        assert_eq!(g.extent(), (3.0, 2.0));
    }
}
