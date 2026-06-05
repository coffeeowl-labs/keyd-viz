//! `keydviz-core` — pure, dependency-free keyd logic for keyd-viz.
//!
//! Parsing ([`parse_text`]), value prettifying ([`prettify`]), physical layouts
//! ([`layout_for`]), and display constants ([`style`]). No I/O beyond an optional
//! file read; all keyd-domain knowledge lives here so the GUI stays a thin layer.

pub mod board;
pub mod catalog;
pub mod geometry;
pub mod ids;
pub mod live;
pub mod layout;
pub mod model;
pub mod parser;
pub mod prettify;
pub mod qmk;
pub mod style;

pub use board::{Badge, Board, KeyCap, KeyState, Sheet};
pub use catalog::BoardKind;
pub use geometry::{Geometry, Slot};
pub use qmk::{import as import_qmk, QmkImport};
pub use ids::{Ids, MatchKind, TypeFilter};
pub use live::{
    parse_listen_line, parse_monitor_line, ActiveLayers, KeyAction, KeyEvent, LayerEvent,
    LiveEvent, MonitorEvent,
};
pub use layout::layout_for;
pub use model::{Config, Hold, HoldKind, Layer};
pub use parser::{parse_file, parse_text};
pub use prettify::{base_legend, prettify};
