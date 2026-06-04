//! The semantic render model: turn a [`Config`] + physical [`Layout`] into
//! presentation-agnostic boards of key caps. This is the bridge between keyd
//! logic and any renderer (the Slint UI, or the legacy HTML).
//!
//! Ports `render_base` / `render_layer` / `render_config` from the original
//! Python tool, but emits structured data instead of HTML so the visual
//! semantics stay unit-testable and the GUI stays a thin presentation layer.

use crate::layout::Layout;
use crate::model::{Config, HoldKind, Layer};
use crate::prettify::{base_legend, prettify};
use crate::style::{accent_for, mod_name, REMAP_ACCENT};

/// A small corner badge on a key cap (hold target, `HOLD`, or chord marker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub text: String,
    /// Background color (hex).
    pub color: String,
}

/// Visual state of a key on a board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// Normal cap.
    Normal,
    /// Dimmed (unchanged on this layer).
    Dim,
    /// The key you hold to reach this layer (highlighted).
    Hold,
}

/// One rendered key cap.
#[derive(Debug, Clone, PartialEq)] // not Eq: `width` is f32
pub struct KeyCap {
    /// Width in standard key units.
    pub width: f32,
    /// The keyd key name for this physical position (e.g. `a`, `space`, `leftshift`).
    /// Lets a live keypress from `keyd monitor` (same name namespace) light up the cap.
    pub key: String,
    /// Primary label shown on the cap.
    pub label: String,
    /// `true` => render as a large emphasized glyph (remap/override/momentary mod).
    pub emphasized: bool,
    /// Faint top-right legend showing the key's normal meaning (empty if none).
    pub ghost: String,
    /// Accent color (hex) for the cap text/border; empty => default cap color.
    pub accent: String,
    pub state: KeyState,
    /// Bottom-left badge (hold target / `HOLD`).
    pub badge_left: Option<Badge>,
    /// Bottom-right badge (chord marker).
    pub badge_right: Option<Badge>,
}

/// One board: the base layer or a single layer's overrides.
#[derive(Debug, Clone, PartialEq)] // not Eq: caps carry f32 widths
pub struct Board {
    pub is_base: bool,
    /// `Base layer` or the layer name, uppercased.
    pub title: String,
    /// Layer accent color (hex) for the tag/emphasis; empty for the base board.
    pub accent: String,
    /// How to engage this layer (e.g. `hold F`, `toggle: ⇧ + ⇧`); empty if n/a.
    pub how: String,
    /// One-line descriptive hint.
    pub hint: String,
    pub rows: Vec<Vec<KeyCap>>,
}

/// A full cheatsheet for one config: its source, profile, ids, and boards.
#[derive(Debug, Clone, PartialEq)] // not Eq: boards carry f32 widths
pub struct Sheet {
    pub source: String,
    pub profile: String,
    pub ids: Vec<String>,
    pub boards: Vec<Board>,
}

impl Sheet {
    /// Build the full cheatsheet for one parsed config on a physical layout.
    pub fn build(cfg: &Config, source: &str, layout: Layout, profile: &str) -> Sheet {
        let mut boards = vec![build_base(cfg, layout)];
        // Non-game layers in declaration order, then game last (matches the
        // original render order).
        for layer in &cfg.layers {
            if layer.name != "game" {
                boards.push(build_layer(cfg, layer, layout));
            }
        }
        if let Some(game) = cfg.layer("game") {
            boards.push(build_layer(cfg, game, layout));
        }
        Sheet {
            source: source.to_string(),
            profile: profile.to_string(),
            ids: cfg.ids.clone(),
            boards,
        }
    }
}

/// The last tap/hold (or momentary) binding for a physical key. Last-wins mirrors
/// the original dict comprehension over `cfg.holds`.
fn last_hold_for<'a>(cfg: &'a Config, key: &str) -> Option<&'a crate::model::Hold> {
    cfg.holds.iter().rev().find(|h| h.key == key)
}

/// The chord target whose chord includes `key` (last-wins, mirroring the dict).
fn chord_target_for(cfg: &Config, key: &str) -> Option<String> {
    cfg.chords
        .iter()
        .rev()
        .find(|(chord, _)| chord.split('+').any(|p| p.trim() == key))
        .map(|(_, target)| target.clone())
}

fn build_base(cfg: &Config, layout: Layout) -> Board {
    let mut rows = Vec::new();
    for prow in layout {
        let mut cells = Vec::new();
        for &(name, width) in *prow {
            let mut cap = KeyCap {
                width,
                key: name.to_string(),
                label: String::new(),
                emphasized: false,
                ghost: String::new(),
                accent: String::new(),
                state: KeyState::Normal,
                badge_left: None,
                badge_right: None,
            };

            if let Some(h) = last_hold_for(cfg, name) {
                let col = if h.kind == HoldKind::Mod {
                    accent_for("control")
                } else {
                    accent_for(&h.target)
                };
                let label_text = if h.kind == HoldKind::Mod {
                    mod_name(&h.target).to_string()
                } else {
                    h.target.clone()
                };
                cap.accent = col.to_string();
                match &h.tap {
                    // Pure momentary modifier/layer: the key simply *is* that function.
                    None => {
                        cap.emphasized = true;
                        cap.label = label_text;
                        cap.ghost = base_legend(name);
                    }
                    Some(tap) => {
                        cap.label = prettify(tap);
                        cap.badge_left = Some(Badge {
                            text: format!("\u{2193}{label_text}"), // ↓<target>
                            color: col.to_string(),
                        });
                    }
                }
            } else if let Some(val) = cfg.remap(name) {
                cap.accent = REMAP_ACCENT.to_string();
                cap.emphasized = true;
                cap.label = prettify(val);
                cap.ghost = base_legend(name);
            } else {
                cap.label = base_legend(name);
            }

            if let Some(target) = chord_target_for(cfg, name) {
                cap.badge_right = Some(Badge {
                    text: "\u{21e7}\u{21e7}".to_string(), // ⇧⇧
                    color: accent_for(&target).to_string(),
                });
            }

            cells.push(cap);
        }
        rows.push(cells);
    }

    Board {
        is_base: true,
        title: "Base layer".to_string(),
        accent: String::new(),
        how: String::new(),
        hint: "tap = legend \u{b7} \u{2193}badge = hold \u{b7} orange = remap".to_string(),
        rows,
    }
}

fn build_layer(cfg: &Config, layer: &Layer, layout: Layout) -> Board {
    let name = &layer.name;
    let accent = accent_for(name).to_string();
    let is_game = name == "game";

    // First binding whose hold engages this layer (the key you hold).
    let act_key = cfg.holds.iter().find(|h| &h.target == name).map(|h| h.key.clone());
    let chord = cfg.chords.iter().find(|(_, t)| t == name).map(|(c, _)| c.clone());

    let (how, hint) = if is_game {
        let how = match &chord {
            Some(c) => {
                let keys = c
                    .split('+')
                    .map(|p| base_legend(p.trim()))
                    .collect::<Vec<_>>()
                    .join(" + ");
                format!("toggle: {keys}")
            }
            None => "toggle layer".to_string(),
        };
        (how, "passthrough \u{2014} these revert to plain keys (gaming)".to_string())
    } else {
        let how = match &act_key {
            Some(k) => format!("hold {}", base_legend(k)),
            None => String::new(),
        };
        (how, "highlighted keys change while held".to_string())
    };

    let mut rows = Vec::new();
    for prow in layout {
        let mut cells = Vec::new();
        for &(nm, width) in *prow {
            let cap = if let Some(val) = layer.get(nm) {
                KeyCap {
                    width,
                    key: nm.to_string(),
                    label: if is_game { base_legend(nm) } else { prettify(val) },
                    emphasized: true,
                    ghost: if is_game { String::new() } else { base_legend(nm) },
                    accent: accent.clone(),
                    state: KeyState::Normal,
                    badge_left: None,
                    badge_right: None,
                }
            } else if act_key.as_deref() == Some(nm) {
                KeyCap {
                    width,
                    key: nm.to_string(),
                    label: base_legend(nm),
                    emphasized: false,
                    ghost: String::new(),
                    accent: accent.clone(),
                    state: KeyState::Hold,
                    badge_left: Some(Badge { text: "HOLD".to_string(), color: accent.clone() }),
                    badge_right: None,
                }
            } else {
                KeyCap {
                    width,
                    key: nm.to_string(),
                    label: base_legend(nm),
                    emphasized: false,
                    ghost: String::new(),
                    accent: String::new(),
                    state: KeyState::Dim,
                    badge_left: None,
                    badge_right: None,
                }
            };
            cells.push(cap);
        }
        rows.push(cells);
    }

    Board {
        is_base: false,
        title: name.to_uppercase(),
        accent,
        how,
        hint,
        rows,
    }
}
