//! `keydviz-core` — pure, dependency-free keyd logic for keyd-viz.
//!
//! Parsing ([`parse_text`]), value prettifying ([`prettify`]), physical layouts
//! ([`layout_for`]), and display constants ([`style`]). No I/O beyond an optional
//! file read; all keyd-domain knowledge lives here so the GUI stays a thin layer.

pub mod board;
pub mod catalog;
pub mod edit;
pub mod geometry;
pub mod globals;
pub mod humanizer;
pub mod ids;
pub mod keycodes;
pub mod layeraction;
pub mod live;
pub mod layout;
pub mod macros;
pub mod model;
pub mod mods;
pub mod parser;
pub mod prettify;
pub mod qmk;
pub mod style;
pub mod taphold;

pub use board::{Badge, Board, KeyCap, KeyState, Sheet};
pub use catalog::BoardKind;
pub use edit::{round_trips, EditConfig};
pub use geometry::{Geometry, Slot};
pub use globals::{is_known_global, GlobalOption, GLOBAL_OPTIONS};
pub use humanizer::humanize;
pub use qmk::{import as import_qmk, QmkImport};
pub use ids::{find_conflicts, DeviceFlags, IdConflict, Ids, MatchKind};
pub use keycodes::keycode_name;
pub use layeraction::{LayerAction, LayerKind};
pub use live::{
    parse_listen_line, parse_monitor_line, ActiveLayers, KeyAction, KeyEvent, LayerEvent,
    LiveEvent, MonitorEvent,
};
pub use layout::layout_for;
pub use macros::{Macro, MacroToken};
pub use model::{Config, Hold, HoldKind, Layer};
pub use parser::{canonical_chord, is_chord_key, parse_file, parse_text};
pub use prettify::{base_legend, prettify};
pub use taphold::{Behavior, TapHold, MODIFIERS};
