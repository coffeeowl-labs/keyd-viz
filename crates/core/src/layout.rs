//! Pick a default physical layout for a config file.
//!
//! keyd exposes no board identity, so until the user picks (and we persist) a layout
//! per keyboard, we guess one from the config's file name. The actual geometries live
//! in the curated [`catalog`](crate::catalog); this is just the name-based default.

use crate::catalog;
use crate::geometry::Geometry;

/// Pick a default physical layout from a config file's name (HHKB for `*hhkb*`, ortho
/// for `*planck*`, …, else ANSI-60). Returns the positioned geometry and its display
/// name. See [`catalog::guess`] for the full mapping.
///
/// This is the interim default until the GUI picker lets the user choose per keyboard
/// (the irreducible manual step — keyd carries no physical-layout information).
pub fn layout_for(path: &str) -> (Geometry, &'static str) {
    let id = catalog::guess(path);
    let geom = catalog::geometry(id).expect("catalog::guess returns a known id");
    let name = catalog::name(id).expect("catalog::guess returns a known id");
    (geom, name)
}
