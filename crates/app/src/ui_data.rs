//! Pure converters from the semantic board model (`keydviz_core`) to the Slint
//! `*Data` structs the UI binds to, plus the tiny color/model helpers they share.
//!
//! Nothing here holds state or touches the window — it's all `core type -> Slint
//! struct`, so it's the natural home for `hex`/`brush`/`model` (used everywhere) and
//! the `to_keycap`/`to_sheet_data` board projection.

use std::path::Path;
use std::rc::Rc;

use slint::{Brush, Color, ModelRc, VecModel};

use keydviz_core::board::{KeyCap, KeyState};
use keydviz_core::Sheet;

use crate::{BoardData, IdTag, KeyCapData, SheetData};

/// Parse `#rrggbb` into a Slint color (black on malformed input).
fn hex(s: &str) -> Color {
    let s = s.trim_start_matches('#');
    if s.len() == 6 {
        let p = |a, b| u8::from_str_radix(&s[a..b], 16).unwrap_or(0);
        Color::from_rgb_u8(p(0, 2), p(2, 4), p(4, 6))
    } else {
        Color::from_rgb_u8(0, 0, 0)
    }
}

fn brush(s: &str) -> Brush {
    Brush::SolidColor(hex(s))
}

/// Wrap a Vec into a Slint model.
pub(crate) fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

fn to_keycap(k: &KeyCap) -> KeyCapData {
    let badge = |b: &Option<keydviz_core::Badge>| {
        b.as_ref().map(|x| (x.text.clone(), x.color.clone())).unwrap_or_default()
    };
    let (bl_text, bl_color) = badge(&k.badge_left);
    let (br_text, br_color) = badge(&k.badge_right);

    KeyCapData {
        x: k.x,
        y: k.y,
        width: k.width,
        height: k.height,
        rotation: k.r,
        rx: k.rx,
        ry: k.ry,
        key: k.key.clone().into(),
        phys: k.phys.clone().into(),
        label: k.label.clone().into(),
        emphasized: k.emphasized,
        ghost: k.ghost.clone().into(),
        has_accent: !k.accent.is_empty(),
        accent: brush(if k.accent.is_empty() { "#000000" } else { &k.accent }),
        state: match k.state {
            KeyState::Normal => 0,
            KeyState::Dim => 1,
            KeyState::Hold => 2,
        },
        pressed: false,
        chord_pick: false,
        badge_left: bl_text.into(),
        badge_left_color: brush(if bl_color.is_empty() { "#000000" } else { &bl_color }),
        has_badge_left: k.badge_left.is_some(),
        badge_right: br_text.into(),
        badge_right_color: brush(if br_color.is_empty() { "#000000" } else { &br_color }),
        has_badge_right: k.badge_right.is_some(),
    }
}

pub(crate) fn to_sheet_data(sheet: &Sheet, device: &str, layout_id: &str, matched_ids: &[String]) -> SheetData {
    let boards = sheet
        .boards
        .iter()
        .map(|b| BoardData {
            is_base: b.is_base,
            title: b.title.clone().into(),
            accent: brush(if b.accent.is_empty() { "#000000" } else { &b.accent }),
            has_accent: !b.accent.is_empty(),
            how: b.how.clone().into(),
            hint: b.hint.clone().into(),
            keys: model(b.keys.iter().map(to_keycap).collect()),
            extent_w: b.extent.0,
            extent_h: b.extent.1,
        })
        .collect();

    let id_tags: Vec<IdTag> = sheet
        .ids
        .iter()
        .map(|id| IdTag {
            text: id.clone().into(),
            matched: matched_ids.iter().any(|d| id_matches(id, d)),
        })
        .collect();
    let name = Path::new(&sheet.source)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| sheet.source.clone());

    SheetData {
        name: name.into(),
        path: sheet.source.clone().into(),
        profile: sheet.profile.clone().into(),
        id_tags: model(id_tags),
        device: device.into(),
        layout_id: layout_id.into(),
        boards: model(boards),
    }
}

/// Whether a config `[ids]` entry refers to a concrete connected `vendor:product`. Handles
/// a bare `vvvv:pppp` and keyd's `k:`/`m:` type prefixes; wildcards (`*`) never match a
/// specific device, so they stay un-highlighted.
fn id_matches(config_id: &str, devid: &str) -> bool {
    config_id == devid || config_id.ends_with(devid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use keydviz_core::Badge;

    fn crgb(c: Color) -> (u8, u8, u8) {
        (c.red(), c.green(), c.blue())
    }
    fn brgb(b: &Brush) -> (u8, u8, u8) {
        crgb(b.color())
    }

    /// A baseline cap (KeyCap has no Default — f32 geometry fields); tests tweak one field.
    fn cap() -> KeyCap {
        KeyCap {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            r: 0.0,
            rx: 0.0,
            ry: 0.0,
            key: String::new(),
            phys: String::new(),
            label: String::new(),
            emphasized: false,
            ghost: String::new(),
            accent: String::new(),
            state: KeyState::Normal,
            badge_left: None,
            badge_right: None,
        }
    }

    #[test]
    fn hex_parses_rrggbb_with_or_without_hash() {
        assert_eq!(crgb(hex("#ff8800")), (255, 136, 0));
        assert_eq!(crgb(hex("00ff00")), (0, 255, 0)); // leading '#' is optional
    }

    #[test]
    fn hex_is_black_on_malformed_input() {
        assert_eq!(crgb(hex("#fff")), (0, 0, 0)); // wrong length
        assert_eq!(crgb(hex("#gggggg")), (0, 0, 0)); // non-hex digits
        assert_eq!(crgb(hex("")), (0, 0, 0)); // empty
    }

    #[test]
    fn brush_is_a_solid_color_of_the_hex() {
        assert_eq!(brgb(&brush("#0000ff")), (0, 0, 255));
    }

    #[test]
    fn to_keycap_maps_state_enum_to_int() {
        for (st, want) in [(KeyState::Normal, 0), (KeyState::Dim, 1), (KeyState::Hold, 2)] {
            let mut k = cap();
            k.state = st;
            assert_eq!(to_keycap(&k).state, want);
        }
    }

    #[test]
    fn to_keycap_accent_present_vs_absent() {
        let mut k = cap();
        k.accent = "#abcdef".into();
        let kc = to_keycap(&k);
        assert!(kc.has_accent);
        assert_eq!(brgb(&kc.accent), (0xab, 0xcd, 0xef));

        let blank = to_keycap(&cap()); // empty accent => not flagged, brush falls back to black
        assert!(!blank.has_accent);
        assert_eq!(brgb(&blank.accent), (0, 0, 0));
    }

    #[test]
    fn to_keycap_badge_text_color_and_presence() {
        let mut k = cap();
        k.badge_left = Some(Badge { text: "HOLD".into(), color: "#112233".into() });
        let kc = to_keycap(&k);
        assert!(kc.has_badge_left);
        assert_eq!(kc.badge_left.as_str(), "HOLD");
        assert_eq!(brgb(&kc.badge_left_color), (0x11, 0x22, 0x33));
        // The absent right badge: no flag, empty text, black fallback color.
        assert!(!kc.has_badge_right);
        assert_eq!(kc.badge_right.as_str(), "");
        assert_eq!(brgb(&kc.badge_right_color), (0, 0, 0));
    }

    #[test]
    fn to_keycap_badge_empty_color_falls_back_to_black() {
        let mut k = cap();
        k.badge_right = Some(Badge { text: "+".into(), color: String::new() });
        let kc = to_keycap(&k);
        assert!(kc.has_badge_right);
        assert_eq!(kc.badge_right.as_str(), "+");
        assert_eq!(brgb(&kc.badge_right_color), (0, 0, 0));
    }

    #[test]
    fn to_keycap_passes_geometry_and_string_fields_through() {
        let mut k = cap();
        k.x = 1.5;
        k.y = 2.0;
        k.width = 1.25;
        k.emphasized = true;
        k.key = "a".into();
        k.phys = "q".into();
        k.label = "Esc".into();
        k.ghost = "g".into();
        let kc = to_keycap(&k);
        assert_eq!(kc.x, 1.5);
        assert_eq!(kc.y, 2.0);
        assert_eq!(kc.width, 1.25);
        assert!(kc.emphasized);
        assert_eq!(kc.key.as_str(), "a");
        assert_eq!(kc.phys.as_str(), "q");
        assert_eq!(kc.label.as_str(), "Esc");
        assert_eq!(kc.ghost.as_str(), "g");
        // These are always seeded false (set later by live state / picker, not this projector).
        assert!(!kc.pressed);
        assert!(!kc.chord_pick);
    }

    #[test]
    fn id_matches_exact_and_keyd_type_prefixes() {
        assert!(id_matches("04fe:0021", "04fe:0021")); // bare vendor:product
        assert!(id_matches("k:04fe:0021", "04fe:0021")); // keyd keyboard prefix
        assert!(id_matches("m:1234:5678", "1234:5678")); // keyd mouse prefix
    }

    #[test]
    fn id_matches_wildcard_and_mismatch_never_highlight() {
        assert!(!id_matches("*", "04fe:0021")); // wildcard stays un-highlighted
        assert!(!id_matches("dead:beef", "04fe:0021")); // different device
    }
}
