//! `keydviz-core` — pure, dependency-free keyd logic for keyd-viz.
//!
//! Parsing ([`parse_text`]), value prettifying ([`prettify`]), physical layouts
//! ([`layout_for`]), and display constants ([`style`]). No I/O beyond an optional
//! file read; all keyd-domain knowledge lives here so the GUI stays a thin layer.

pub mod layout;
pub mod model;
pub mod parser;
pub mod prettify;
pub mod style;

pub use layout::{layout_for, Layout, Row, ANSI60, HHKB};
pub use model::{Config, Hold, HoldKind, Layer};
pub use parser::{parse_file, parse_text};
pub use prettify::{base_legend, prettify};
