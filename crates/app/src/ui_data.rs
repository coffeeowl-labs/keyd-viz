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
