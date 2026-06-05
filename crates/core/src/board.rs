//! The semantic render model: turn a [`Config`] + physical [`Geometry`] into
//! presentation-agnostic boards of key caps. This is the bridge between keyd
//! logic and any renderer (the Slint UI, or the legacy HTML).
//!
//! Ports `render_base` / `render_layer` / `render_config` from the original
//! Python tool, but emits structured data instead of HTML so the visual
//! semantics stay unit-testable and the GUI stays a thin presentation layer.

use crate::geometry::{Geometry, Slot};
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

/// One rendered key cap, positioned in key units (see [`crate::geometry::Slot`]).
#[derive(Debug, Clone, PartialEq)] // not Eq: f32 geometry fields
pub struct KeyCap {
    /// Left edge in key units from the board's top-left.
    pub x: f32,
    /// Top edge in key units from the board's top-left.
    pub y: f32,
    /// Width in standard key units.
    pub width: f32,
    /// Height in key units.
    pub height: f32,
    /// Rotation in degrees clockwise about (`rx`, `ry`).
    pub r: f32,
    pub rx: f32,
    pub ry: f32,
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
#[derive(Debug, Clone, PartialEq)] // not Eq: caps carry f32 geometry
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
    /// Positioned key caps (absolute geometry; not grouped into rows).
    pub keys: Vec<KeyCap>,
    /// Board extent `(width, height)` in key units, for sizing the panel.
    pub extent: (f32, f32),
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
    /// Build the full cheatsheet for one parsed config on a physical [`Geometry`].
    pub fn build(cfg: &Config, source: &str, geom: &Geometry, profile: &str) -> Sheet {
        let mut boards = vec![build_base(cfg, geom)];
        // Non-game layers in declaration order, then game last (matches the
        // original render order).
        for layer in &cfg.layers {
            if layer.name != "game" {
                boards.push(build_layer(cfg, layer, geom));
            }
        }
        if let Some(game) = cfg.layer("game") {
            boards.push(build_layer(cfg, game, geom));
        }
        Sheet {
            source: source.to_string(),
            profile: profile.to_string(),
            ids: cfg.ids.clone(),
            boards,
        }
    }
}

/// A blank positioned cap at `slot` (geometry filled in, semantics empty). `key` is
/// the keyd key name for the slot (empty for a decorative/unmapped slot).
fn cap_at(slot: &Slot, key: &str) -> KeyCap {
    KeyCap {
        x: slot.x,
        y: slot.y,
        width: slot.w,
        height: slot.h,
        r: slot.r,
        rx: slot.rx,
        ry: slot.ry,
        key: key.to_string(),
        label: String::new(),
        emphasized: false,
        ghost: String::new(),
        accent: String::new(),
        state: KeyState::Normal,
        badge_left: None,
        badge_right: None,
    }
}

/// The single plain keyd key a binding *emits*, used to match the live keypress glow
/// (`keyd monitor` reports the post-remap output keysym, not the physical key). Returns
/// `None` for actions/combos with no single output — `macro(...)`, `layer(...)`,
/// modifier chords like `C-c` — which can't be matched to one cap.
fn output_key(val: &str) -> Option<String> {
    let v = val.trim();
    if v.is_empty() || v.contains(['(', ' ', '-', '+']) {
        return None;
    }
    Some(v.to_string())
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

fn build_base(cfg: &Config, geom: &Geometry) -> Board {
    let mut keys = Vec::new();
    for slot in &geom.slots {
        // Decorative / unmapped slot: a dim blank cap holding its place.
        let Some(name) = slot.key.as_deref() else {
            let mut blank = cap_at(slot, "");
            blank.state = KeyState::Dim;
            keys.push(blank);
            continue;
        };
        let mut cap = cap_at(slot, name);

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
                    // A tap emits the tap action; glow on that, not the physical key.
                    if let Some(out) = output_key(tap) {
                        cap.key = out;
                    }
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
            // Remapped keys emit the remap target; glow matches that, not the physical key.
            if let Some(out) = output_key(val) {
                cap.key = out;
            }
        } else {
            cap.label = base_legend(name);
        }

        if let Some(target) = chord_target_for(cfg, name) {
            cap.badge_right = Some(Badge {
                text: "\u{21e7}\u{21e7}".to_string(), // ⇧⇧
                color: accent_for(&target).to_string(),
            });
        }

        keys.push(cap);
    }

    Board {
        is_base: true,
        title: "Base layer".to_string(),
        accent: String::new(),
        how: String::new(),
        hint: "tap = legend \u{b7} \u{2193}badge = hold \u{b7} orange = remap".to_string(),
        keys,
        extent: geom.extent(),
    }
}

fn build_layer(cfg: &Config, layer: &Layer, geom: &Geometry) -> Board {
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

    let mut keys = Vec::new();
    for slot in &geom.slots {
        let Some(nm) = slot.key.as_deref() else {
            let mut blank = cap_at(slot, "");
            blank.state = KeyState::Dim;
            keys.push(blank);
            continue;
        };
        let mut cap = cap_at(slot, nm);
        if let Some(val) = layer.get(nm) {
            cap.label = if is_game { base_legend(nm) } else { prettify(val) };
            cap.emphasized = true;
            cap.ghost = if is_game { String::new() } else { base_legend(nm) };
            cap.accent = accent.clone();
            // Glow on what the remapped key emits (a num-layer `j = 4` glows the j-cap
            // when keyd reports `4`). Game/passthrough keys still emit their own key.
            if !is_game {
                cap.key = output_key(val).unwrap_or_default();
            }
        } else if act_key.as_deref() == Some(nm) {
            cap.label = base_legend(nm);
            cap.accent = accent.clone();
            cap.state = KeyState::Hold;
            cap.badge_left = Some(Badge { text: "HOLD".to_string(), color: accent.clone() });
            // Held to reach this layer — keyd emits nothing for it, so never glow it.
            cap.key = String::new();
        } else {
            cap.label = base_legend(nm);
            cap.state = KeyState::Dim;
        }
        keys.push(cap);
    }

    Board {
        is_base: false,
        title: name.to_uppercase(),
        accent,
        how,
        hint,
        keys,
        extent: geom.extent(),
    }
}
