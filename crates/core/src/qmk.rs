//! Import physical [`Geometry`] from QMK data.
//!
//! QMK `info.json` carries **geometry only** (`layouts.<NAME>.layout[]` with
//! `x/y/w/h/r/rx/ry`, a `matrix` position, and an optional human `label`) — no key
//! identity. The identity lives in the board's default **keymap**, whose layer-0
//! array is **index-aligned** with the `LAYOUT` macro, i.e. with the `info.json`
//! layout array for the matching variant. So we zip by index: `layout[i]` (geometry)
//! with `keymap.layers[0][i]` (a `KC_*` keycode) → a [`Slot`] labeled with the keyd key
//! name. Keycodes that aren't a plain key (`MO(..)`, `LT(..)`, `KC_NO`, `KC_TRNS`,
//! custom `QK_*`/macros) become unmapped slots (`key: None`), which render blank and
//! can be hand-labeled later. (See ROADMAP §4.5 and the research in the change log.)

use std::collections::HashMap;

use serde::Deserialize;

use crate::geometry::{Geometry, Slot};

/// The result of importing a QMK board: its geometry, the chosen layout-variant name,
/// and how many slots couldn't be labeled (for a "needs cleanup" hint).
#[derive(Debug, Clone, PartialEq)]
pub struct QmkImport {
    pub geometry: Geometry,
    pub layout_name: String,
    pub unmapped: usize,
}

// ---- info.json (geometry) -----------------------------------------------------

#[derive(Debug, Deserialize)]
struct InfoJson {
    #[serde(default)]
    layouts: HashMap<String, InfoLayout>,
}

#[derive(Debug, Deserialize)]
struct InfoLayout {
    #[serde(default)]
    layout: Vec<InfoKey>,
}

fn one() -> f32 {
    1.0
}

#[derive(Debug, Deserialize)]
struct InfoKey {
    x: f32,
    y: f32,
    #[serde(default = "one")]
    w: f32,
    #[serde(default = "one")]
    h: f32,
    #[serde(default)]
    r: f32,
    #[serde(default)]
    rx: f32,
    #[serde(default)]
    ry: f32,
    #[serde(default)]
    label: Option<String>,
}

// ---- keymap.json (identity) ---------------------------------------------------

#[derive(Debug, Deserialize)]
struct KeymapJson {
    #[serde(default)]
    layout: Option<String>,
    #[serde(default)]
    layers: Vec<Vec<String>>,
}

/// The layout-variant names available in an `info.json` (e.g. `LAYOUT_60_ansi`,
/// `LAYOUT_60_iso`) — for offering a picker when a board defines several.
pub fn layout_names(info_json: &str) -> Result<Vec<String>, String> {
    let info: InfoJson = serde_json::from_str(info_json).map_err(|e| format!("info.json: {e}"))?;
    let mut names: Vec<String> = info.layouts.into_keys().collect();
    names.sort();
    Ok(names)
}

/// Import a board's geometry from its `info.json`, labeling each slot via the default
/// `keymap_json` when given (zipped by index against the matching layout variant).
///
/// Variant selection: the keymap's `layout` field if present and known; otherwise the
/// sole layout; otherwise an error listing the choices (pass the name as `prefer`).
pub fn import(
    info_json: &str,
    keymap_json: Option<&str>,
    prefer: Option<&str>,
) -> Result<QmkImport, String> {
    let info: InfoJson = serde_json::from_str(info_json).map_err(|e| format!("info.json: {e}"))?;
    if info.layouts.is_empty() {
        return Err("info.json has no layouts".into());
    }

    let keymap: Option<KeymapJson> = match keymap_json {
        Some(k) => Some(serde_json::from_str(k).map_err(|e| format!("keymap.json: {e}"))?),
        None => None,
    };

    // Pick the layout variant.
    let wanted = prefer
        .map(str::to_string)
        .or_else(|| keymap.as_ref().and_then(|k| k.layout.clone()));
    let layout_name = match wanted {
        Some(name) if info.layouts.contains_key(&name) => name,
        Some(name) => return Err(format!("layout '{name}' not in info.json")),
        None if info.layouts.len() == 1 => info.layouts.keys().next().unwrap().clone(),
        None => {
            let mut names: Vec<&String> = info.layouts.keys().collect();
            names.sort();
            return Err(format!(
                "info.json defines {} layouts; choose one: {}",
                names.len(),
                names.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
    };
    let layout = &info.layouts[&layout_name].layout;

    // The base layer's keycodes (layer 0), if a keymap was supplied.
    let base: Option<&Vec<String>> = keymap.as_ref().and_then(|k| k.layers.first());

    let mut slots = Vec::with_capacity(layout.len());
    let mut unmapped = 0usize;
    for (i, k) in layout.iter().enumerate() {
        // identity: prefer the keymap keycode, fall back to the info.json label.
        let key = base
            .and_then(|b| b.get(i))
            .and_then(|kc| keycode_to_keyd(kc))
            .or_else(|| label_to_keyd(k.label.as_deref()));
        if key.is_none() {
            unmapped += 1;
        }
        slots.push(Slot { x: k.x, y: k.y, w: k.w, h: k.h, r: k.r, rx: k.rx, ry: k.ry, key });
    }

    let mut geometry = Geometry { slots };
    normalize(&mut geometry);
    Ok(QmkImport { geometry, layout_name, unmapped })
}

/// Shift the geometry so its top-left is at (0, 0) — some boards start at a non-zero
/// origin. Rotation origins shift with it so rotated clusters stay correct.
fn normalize(geom: &mut Geometry) {
    let min_x = geom.slots.iter().map(|s| s.x).fold(f32::INFINITY, f32::min);
    let min_y = geom.slots.iter().map(|s| s.y).fold(f32::INFINITY, f32::min);
    if !min_x.is_finite() || !min_y.is_finite() {
        return;
    }
    for s in &mut geom.slots {
        s.x -= min_x;
        s.y -= min_y;
        s.rx -= min_x;
        s.ry -= min_y;
    }
}

// ---- keycode translation ------------------------------------------------------

/// Translate a QMK keycode string to a keyd key name, or `None` if it isn't a plain
/// key (layer/mod/custom/transparent/no-op). Handles letters, digits, and F-keys
/// algorithmically; everything else via [`NAMED`].
pub fn keycode_to_keyd(kc: &str) -> Option<String> {
    let kc = kc.trim();
    if let Some(rest) = kc.strip_prefix("KC_") {
        // single letter A–Z or digit 0–9
        if rest.len() == 1 {
            let b = rest.as_bytes()[0];
            if b.is_ascii_uppercase() {
                return Some((b as char).to_ascii_lowercase().to_string());
            }
            if b.is_ascii_digit() {
                return Some((b as char).to_string());
            }
        }
        // F1..F24
        if let Some(n) = rest.strip_prefix('F') {
            if let Ok(num) = n.parse::<u8>() {
                if (1..=24).contains(&num) {
                    return Some(format!("f{num}"));
                }
            }
        }
    }
    NAMED.iter().find(|(q, _)| *q == kc).map(|(_, k)| (*k).to_string())
}

/// Best-effort identity from an `info.json` human `label` when no keymap is available.
/// Deliberately conservative — labels are free-form and unreliable, so we only accept
/// unambiguous single letters/digits and a few obvious words.
fn label_to_keyd(label: Option<&str>) -> Option<String> {
    let l = label?.trim();
    if l.len() == 1 {
        let b = l.as_bytes()[0];
        if b.is_ascii_alphabetic() {
            return Some((b as char).to_ascii_lowercase().to_string());
        }
        if b.is_ascii_digit() {
            return Some((b as char).to_string());
        }
    }
    let lower = l.to_ascii_lowercase();
    NAMED_LABELS.iter().find(|(w, _)| *w == lower).map(|(_, k)| (*k).to_string())
}

/// QMK keycode → keyd key name. Every right-hand value is a real keyd key (validated
/// against `keyd list-keys`). Both short (`KC_SCLN`) and a few long aliases included.
#[rustfmt::skip]
static NAMED: &[(&str, &str)] = &[
    // whitespace / control
    ("KC_ENT", "enter"), ("KC_ENTER", "enter"), ("KC_ESC", "esc"), ("KC_ESCAPE", "esc"),
    ("KC_BSPC", "backspace"), ("KC_TAB", "tab"), ("KC_SPC", "space"), ("KC_SPACE", "space"),
    ("KC_CAPS", "capslock"), ("KC_DEL", "delete"), ("KC_INS", "insert"),
    // Grave-Escape: a single key that taps Esc (and shift/gui → grave). For a static
    // cheatsheet its resting identity is Esc — the most useful thing to show.
    ("QK_GESC", "esc"), ("KC_GESC", "esc"),
    // modifiers
    ("KC_LCTL", "leftcontrol"), ("KC_RCTL", "rightcontrol"),
    ("KC_LSFT", "leftshift"), ("KC_RSFT", "rightshift"),
    ("KC_LALT", "leftalt"), ("KC_RALT", "rightalt"), ("KC_ALGR", "rightalt"),
    ("KC_LGUI", "leftmeta"), ("KC_RGUI", "rightmeta"), ("KC_LCMD", "leftmeta"), ("KC_LWIN", "leftmeta"),
    // punctuation / symbols
    ("KC_MINS", "minus"), ("KC_EQL", "equal"), ("KC_LBRC", "leftbrace"), ("KC_RBRC", "rightbrace"),
    ("KC_BSLS", "backslash"), ("KC_SCLN", "semicolon"), ("KC_QUOT", "apostrophe"),
    ("KC_GRV", "grave"), ("KC_COMM", "comma"), ("KC_DOT", "dot"), ("KC_SLSH", "slash"),
    ("KC_NUHS", "backslash"), ("KC_NUBS", "102nd"),
    // navigation / editing
    ("KC_HOME", "home"), ("KC_END", "end"), ("KC_PGUP", "pageup"), ("KC_PGDN", "pagedown"),
    ("KC_RGHT", "right"), ("KC_RIGHT", "right"), ("KC_LEFT", "left"),
    ("KC_DOWN", "down"), ("KC_UP", "up"),
    // system / locks
    ("KC_PSCR", "sysrq"), ("KC_SCRL", "scrolllock"), ("KC_PAUS", "pause"),
    ("KC_NUM", "numlock"), ("KC_APP", "compose"), ("KC_MENU", "menu"),
    // keypad
    ("KC_P0", "kp0"), ("KC_P1", "kp1"), ("KC_P2", "kp2"), ("KC_P3", "kp3"), ("KC_P4", "kp4"),
    ("KC_P5", "kp5"), ("KC_P6", "kp6"), ("KC_P7", "kp7"), ("KC_P8", "kp8"), ("KC_P9", "kp9"),
    ("KC_PDOT", "kpdot"), ("KC_PPLS", "kpplus"), ("KC_PMNS", "kpminus"),
    ("KC_PAST", "kpasterisk"), ("KC_PSLS", "kpslash"), ("KC_PENT", "kpenter"), ("KC_PEQL", "kpequal"),
    // media (common ones keyd knows)
    ("KC_MUTE", "mute"), ("KC_VOLU", "volumeup"), ("KC_VOLD", "volumedown"),
];

/// A tiny human-label → keyd map for the no-keymap fallback (conservative on purpose).
#[rustfmt::skip]
static NAMED_LABELS: &[(&str, &str)] = &[
    ("esc", "esc"), ("escape", "esc"), ("tab", "tab"), ("enter", "enter"), ("return", "enter"),
    ("space", "space"), ("backspace", "backspace"), ("bksp", "backspace"), ("del", "delete"),
    ("caps", "capslock"), ("caps lock", "capslock"),
    ("shift", "leftshift"), ("ctrl", "leftcontrol"), ("control", "leftcontrol"),
    ("alt", "leftalt"), ("super", "leftmeta"), ("win", "leftmeta"), ("gui", "leftmeta"), ("cmd", "leftmeta"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_basic_keycodes() {
        assert_eq!(keycode_to_keyd("KC_A").as_deref(), Some("a"));
        assert_eq!(keycode_to_keyd("KC_Z").as_deref(), Some("z"));
        assert_eq!(keycode_to_keyd("KC_1").as_deref(), Some("1"));
        assert_eq!(keycode_to_keyd("KC_0").as_deref(), Some("0"));
        assert_eq!(keycode_to_keyd("KC_F12").as_deref(), Some("f12"));
        assert_eq!(keycode_to_keyd("KC_SCLN").as_deref(), Some("semicolon"));
        assert_eq!(keycode_to_keyd("KC_LSFT").as_deref(), Some("leftshift"));
        assert_eq!(keycode_to_keyd("  KC_SPC  ").as_deref(), Some("space"));
    }

    #[test]
    fn unmappable_keycodes_are_none() {
        // layer/mod/custom/transparent/no-op → no plain key
        assert_eq!(keycode_to_keyd("MO(1)"), None);
        assert_eq!(keycode_to_keyd("LT(2,KC_X)"), None);
        assert_eq!(keycode_to_keyd("MT(MOD_LCTL,KC_A)"), None);
        assert_eq!(keycode_to_keyd("KC_NO"), None);
        assert_eq!(keycode_to_keyd("KC_TRNS"), None);
        assert_eq!(keycode_to_keyd("QK_BOOT"), None);
        assert_eq!(keycode_to_keyd("F13"), None); // not KC_-prefixed
        // Grave-Escape resolves to its tap identity (Esc), not unmapped.
        assert_eq!(keycode_to_keyd("QK_GESC").as_deref(), Some("esc"));
    }

    const INFO: &str = r#"{
      "layouts": {
        "LAYOUT_ansi": { "layout": [
          {"matrix":[0,0],"x":0,"y":0,"label":"Esc"},
          {"matrix":[0,1],"x":1,"y":0},
          {"matrix":[1,0],"x":0,"y":1,"w":1.5},
          {"matrix":[1,1],"x":1.5,"y":1}
        ]},
        "LAYOUT_iso": { "layout": [ {"matrix":[0,0],"x":0,"y":0} ] }
      }
    }"#;

    const KEYMAP: &str = r#"{
      "layout": "LAYOUT_ansi",
      "layers": [ ["KC_ESC","KC_1","KC_TAB","MO(1)"], ["KC_TRNS","KC_TRNS","KC_TRNS","KC_TRNS"] ]
    }"#;

    #[test]
    fn imports_geometry_and_labels_via_keymap() {
        let imp = import(INFO, Some(KEYMAP), None).unwrap();
        assert_eq!(imp.layout_name, "LAYOUT_ansi");
        let g = &imp.geometry;
        assert_eq!(g.slots.len(), 4);
        // labeled from the keymap, by index
        assert_eq!(g.slots[0].key.as_deref(), Some("esc"));
        assert_eq!(g.slots[1].key.as_deref(), Some("1"));
        assert_eq!(g.slots[2].key.as_deref(), Some("tab"));
        // MO(1) is not a plain key → unmapped
        assert_eq!(g.slots[3].key, None);
        assert_eq!(imp.unmapped, 1);
        // geometry preserved (1.5u key at row 1)
        assert_eq!(g.slots[2].w, 1.5);
        assert_eq!(g.slots[3].x, 1.5);
    }

    #[test]
    fn no_keymap_falls_back_to_labels() {
        // pick the variant explicitly since there are two and no keymap
        let imp = import(INFO, None, Some("LAYOUT_ansi")).unwrap();
        // slot 0 has label "Esc" → esc; the rest have no label → unmapped
        assert_eq!(imp.geometry.slots[0].key.as_deref(), Some("esc"));
        assert_eq!(imp.geometry.slots[1].key, None);
        assert_eq!(imp.unmapped, 3);
    }

    #[test]
    fn ambiguous_variant_without_hint_errors() {
        let err = import(INFO, None, None).unwrap_err();
        assert!(err.contains("LAYOUT_ansi") && err.contains("LAYOUT_iso"), "got: {err}");
    }

    #[test]
    fn normalizes_origin_to_zero() {
        let info = r#"{"layouts":{"L":{"layout":[
          {"x":2,"y":3,"label":"A"}, {"x":3,"y":3,"label":"B"}
        ]}}}"#;
        let imp = import(info, None, None).unwrap();
        assert_eq!((imp.geometry.slots[0].x, imp.geometry.slots[0].y), (0.0, 0.0));
        assert_eq!(imp.geometry.slots[1].x, 1.0);
    }

    #[test]
    fn lists_layout_names_sorted() {
        assert_eq!(layout_names(INFO).unwrap(), vec!["LAYOUT_ansi", "LAYOUT_iso"]);
    }

    // -------------------------------------------------- mutation-gap regressions
    #[test]
    fn missing_width_height_default_to_one_unit() {
        let info = r#"{"layouts":{"L":{"layout":[{"x":0,"y":0}]}}}"#;
        let imp = import(info, None, None).unwrap();
        assert_eq!(imp.geometry.slots[0].w, 1.0);
        assert_eq!(imp.geometry.slots[0].h, 1.0);
    }

    #[test]
    fn unknown_preferred_variant_errors() {
        let err = import(INFO, None, Some("LAYOUT_nope")).unwrap_err();
        assert!(err.contains("LAYOUT_nope"), "got: {err}");
    }

    #[test]
    fn normalizes_rotation_origin_with_geometry() {
        let info = r#"{"layouts":{"L":{"layout":[
          {"x":2,"y":3,"rx":2,"ry":3}, {"x":4,"y":5,"rx":4,"ry":5}
        ]}}}"#;
        let imp = import(info, None, None).unwrap();
        let s0 = &imp.geometry.slots[0];
        assert_eq!((s0.x, s0.y), (0.0, 0.0));
        assert_eq!((s0.rx, s0.ry), (0.0, 0.0));
        let s1 = &imp.geometry.slots[1];
        assert_eq!((s1.rx, s1.ry), (2.0, 2.0));
    }

    #[test]
    fn multiword_label_maps_to_exact_keyd_name() {
        let info = r#"{"layouts":{"L":{"layout":[{"x":0,"y":0,"label":"Tab"}]}}}"#;
        let imp = import(info, None, None).unwrap();
        assert_eq!(imp.geometry.slots[0].key.as_deref(), Some("tab"));
    }
}
