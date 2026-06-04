//! keyd-viz — native GUI cheatsheet for keyd.
//!
//! Parses keyd config(s), builds the semantic board model in `keydviz-core`, and
//! renders it with Slint. Input selection (Phase 0): CLI args, else `/etc/keyd/*.conf`,
//! else the bundled example configs so it runs out of the box.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use keydviz_core::board::{KeyCap, KeyState};
use keydviz_core::{layout_for, parse_file, Layout, Sheet};
use slint::{Brush, Color, ModelRc, VecModel};

slint::include_modules!();

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
fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

fn to_keycap(k: &KeyCap) -> KeyCapData {
    let badge = |b: &Option<keydviz_core::Badge>| {
        b.as_ref()
            .map(|x| (x.text.clone(), x.color.clone()))
            .unwrap_or_default()
    };
    let (bl_text, bl_color) = badge(&k.badge_left);
    let (br_text, br_color) = badge(&k.badge_right);

    KeyCapData {
        width: k.width,
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
        badge_left: bl_text.into(),
        badge_left_color: brush(if bl_color.is_empty() { "#000000" } else { &bl_color }),
        has_badge_left: k.badge_left.is_some(),
        badge_right: br_text.into(),
        badge_right_color: brush(if br_color.is_empty() { "#000000" } else { &br_color }),
        has_badge_right: k.badge_right.is_some(),
    }
}

fn to_sheet_data(sheet: &Sheet) -> SheetData {
    let boards = sheet
        .boards
        .iter()
        .map(|b| {
            let rows = b
                .rows
                .iter()
                .map(|row| RowData { keys: model(row.iter().map(to_keycap).collect()) })
                .collect();
            BoardData {
                is_base: b.is_base,
                title: b.title.clone().into(),
                accent: brush(if b.accent.is_empty() { "#000000" } else { &b.accent }),
                has_accent: !b.accent.is_empty(),
                how: b.how.clone().into(),
                hint: b.hint.clone().into(),
                rows: model(rows),
            }
        })
        .collect();

    let ids = if sheet.ids.is_empty() { "\u{2014}".to_string() } else { sheet.ids.join(", ") };
    let name = Path::new(&sheet.source)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| sheet.source.clone());

    SheetData {
        name: name.into(),
        path: sheet.source.clone().into(),
        profile: sheet.profile.clone().into(),
        ids: ids.into(),
        boards: model(boards),
    }
}

/// Resolve which config files to render: CLI args, else `/etc/keyd/*.conf`, else
/// the bundled examples.
fn collect_conf_paths() -> Vec<PathBuf> {
    let args: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if !args.is_empty() {
        return args;
    }
    let mut system = conf_files_in(Path::new("/etc/keyd"));
    system.sort();
    if !system.is_empty() {
        return system;
    }
    // Fallback: bundled examples next to the workspace.
    let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut bundled = conf_files_in(&examples);
    bundled.sort();
    bundled
}

/// All `*.conf` files directly inside `dir` (empty if unreadable).
fn conf_files_in(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "conf"))
        .collect()
}

/// Register the bundled JetBrains Mono faces so typography is identical on every
/// machine, regardless of installed fonts. Must be called after the Slint platform
/// is initialized (i.e. after the first window is constructed).
fn register_fonts() {
    use std::sync::Arc;
    let mut collection = slint::fontique_08::shared_collection();
    for data in [
        include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf").as_slice(),
        include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf").as_slice(),
    ] {
        let blob = slint::fontique_08::fontique::Blob::new(Arc::new(data.to_vec()));
        collection.register_fonts(blob, None);
    }
    if collection.family_id("JetBrains Mono").is_none() {
        eprintln!("warning: bundled font 'JetBrains Mono' did not register; using fallback");
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let paths = collect_conf_paths();

    let mut sheets = Vec::new();
    for path in &paths {
        match parse_file(path) {
            Ok(cfg) => {
                let path_str = path.to_string_lossy();
                let (layout, profile): (Layout, &str) = layout_for(&path_str);
                let sheet = Sheet::build(&cfg, &path_str, layout, profile);
                sheets.push(to_sheet_data(&sheet));
            }
            Err(e) => eprintln!("warning: skipping {}: {e}", path.display()),
        }
    }

    let n = sheets.len();
    let win = MainWindow::new()?;
    register_fonts(); // after MainWindow::new() so the platform is initialized
    win.set_subtitle(format!("{n} keyboard(s) \u{2014} the config is the source of truth").into());
    win.set_sheets(model(sheets));
    win.run()
}
