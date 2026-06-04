//! keyd-viz — native GUI cheatsheet for keyd.
//!
//! Parses keyd config(s), builds the semantic board model in `keydviz-core`, and
//! renders it with Slint. By default it detects connected keyboards and shows only
//! the config(s) governing them; with explicit path args it shows exactly those.

mod devices;
mod layer;

use std::path::{Path, PathBuf};
use std::rc::Rc;

use devices::InputDevice;
use keydviz_core::board::{KeyCap, KeyState};
use keydviz_core::{layout_for, parse_file, Config, Ids, Sheet};
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
        b.as_ref().map(|x| (x.text.clone(), x.color.clone())).unwrap_or_default()
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

fn to_sheet_data(sheet: &Sheet, device: &str) -> SheetData {
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
        device: device.into(),
        boards: model(boards),
    }
}

/// Build a SheetData from a parsed config and its path, with an optional connected-
/// device label.
fn sheet_from(path: &Path, cfg: &Config, device: &str) -> SheetData {
    let path_str = path.to_string_lossy();
    let (layout, profile) = layout_for(&path_str);
    let sheet = Sheet::build(cfg, &path_str, layout, profile);
    to_sheet_data(&sheet, device)
}

/// All `*.conf` files directly inside `dir` (sorted; empty if unreadable).
fn conf_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "conf"))
        .collect();
    v.sort();
    v
}

/// Parse the given config paths into `(path, Config)`, warning on failures.
fn parse_configs(paths: &[PathBuf]) -> Vec<(PathBuf, Config)> {
    paths
        .iter()
        .filter_map(|p| match parse_file(p) {
            Ok(cfg) => Some((p.clone(), cfg)),
            Err(e) => {
                eprintln!("warning: skipping {}: {e}", p.display());
                None
            }
        })
        .collect()
}

/// A short label for the device(s) that matched a config. One physical keyboard
/// can expose several event nodes (e.g. a "Consumer Control" node) sharing a
/// `vendor:product`; we group by that so each physical keyboard appears once,
/// preferring the full-keyboard node's name.
fn device_label(devices: &[InputDevice], idxs: &[usize]) -> String {
    // (devid, chosen name, name-is-from-full-keyboard)
    let mut groups: Vec<(String, &str, bool)> = Vec::new();
    for &i in idxs {
        let d = &devices[i];
        let devid = d.devid();
        if let Some(g) = groups.iter_mut().find(|g| g.0 == devid) {
            if (d.full_keyboard && !g.2) || g.1.is_empty() {
                g.1 = &d.name;
                g.2 = d.full_keyboard;
            }
        } else {
            groups.push((devid, &d.name, d.full_keyboard));
        }
    }
    let names: Vec<&str> = groups.iter().map(|g| g.1).filter(|n| !n.is_empty()).collect();
    match names.len() {
        0 => String::new(),
        1 => names[0].to_string(),
        _ => format!("{} (+{})", names[0], names.len() - 1),
    }
}

/// Decide which sheets to render, and a subtitle describing the selection.
///
/// - Explicit path args  → render exactly those configs.
/// - Otherwise           → glob `/etc/keyd/*.conf`, detect connected keyboards,
///   and render only the matching configs (labeled with the device). If nothing
///   matches, fall back to showing all configs. If `/etc/keyd` is empty, fall back
///   to the bundled examples.
fn gather_sheets() -> (Vec<SheetData>, String) {
    let args: Vec<PathBuf> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .collect();
    if !args.is_empty() {
        let sheets: Vec<SheetData> =
            parse_configs(&args).iter().map(|(p, c)| sheet_from(p, c, "")).collect();
        let n = sheets.len();
        return (sheets, format!("{n} config(s) from arguments"));
    }

    let conf_paths = conf_files_in(Path::new("/etc/keyd"));
    if conf_paths.is_empty() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let sheets: Vec<SheetData> =
            parse_configs(&conf_files_in(&examples)).iter().map(|(p, c)| sheet_from(p, c, "")).collect();
        let n = sheets.len();
        return (sheets, format!("{n} example keyboard(s) \u{2014} no /etc/keyd configs found"));
    }

    let configs = parse_configs(&conf_paths);
    let matchers: Vec<Ids> = configs.iter().map(|(_, c)| Ids::parse(&c.ids)).collect();
    let devices = devices::connected_devices();

    // Assign each connected device to its best-matching config (explicit > wildcard).
    let mut per_config: Vec<Vec<usize>> = vec![Vec::new(); configs.len()];
    for (di, dev) in devices.iter().enumerate() {
        let devid = dev.devid();
        let mut best: Option<(usize, u8)> = None;
        for (ci, ids) in matchers.iter().enumerate() {
            let rank = ids.match_device(&devid, dev.is_keyboard).rank();
            if rank > 0 && best.is_none_or(|(_, br)| rank > br) {
                best = Some((ci, rank));
            }
        }
        if let Some((ci, _)) = best {
            per_config[ci].push(di);
        }
    }

    let matched_any = per_config.iter().any(|v| !v.is_empty());
    if !matched_any {
        // No connected keyboard matched — show everything rather than nothing.
        let sheets: Vec<SheetData> = configs.iter().map(|(p, c)| sheet_from(p, c, "")).collect();
        let n = sheets.len();
        return (sheets, format!("{n} config(s) \u{2014} no connected keyboard detected"));
    }

    let mut sheets = Vec::new();
    for (ci, (path, cfg)) in configs.iter().enumerate() {
        if per_config[ci].is_empty() {
            continue;
        }
        let label = device_label(&devices, &per_config[ci]);
        sheets.push(sheet_from(path, cfg, &label));
    }
    let n = sheets.len();
    (sheets, format!("{n} connected keyboard(s) detected"))
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
    let (sheets, subtitle) = gather_sheets();

    // `--list`: print the detection result to stdout and exit (no GUI). Useful for
    // debugging device detection and for scripting.
    if std::env::args().any(|a| a == "--list") {
        println!("{subtitle}");
        for s in &sheets {
            let dev = if s.device.is_empty() {
                String::new()
            } else {
                format!("  <- {}", s.device)
            };
            println!("  {} [{}]{dev}", s.name, s.path);
        }
        return Ok(());
    }

    let win = MainWindow::new()?;
    register_fonts(); // after MainWindow::new() so the platform is initialized
    win.set_subtitle(subtitle.into());
    win.set_sheets(model(sheets));

    if std::env::args().any(|a| a == "--demo") {
        spawn_demo(&win);
    } else {
        spawn_live(&win);
    }

    win.run()
}

/// Resolve the active-layer stack to a single board title to show, against the live
/// sheet's boards. Walks from the most recently activated layer down, returning the
/// first whose name has a (non-base) board. Returns "" (the base layer) when nothing
/// held maps to a board — e.g. holding a bare `control` mod, which keyd reports as a
/// layer but which has no dedicated board. Must run on the UI thread.
fn resolve_title(win: &MainWindow, active: &[String]) -> slint::SharedString {
    use slint::Model;
    let idx = win.get_live_sheet() as usize;
    let Some(sheet) = win.get_sheets().row_data(idx) else { return Default::default() };
    let boards = sheet.boards;
    for name in active.iter().rev() {
        let upper = name.to_uppercase();
        if let Some(b) = boards.iter().find(|b| !b.is_base && b.title == upper) {
            return b.title;
        }
    }
    Default::default()
}

/// Point the single-board live view at the board for `title` ("" = base layer),
/// updating the connection pill, the active-layer label, and the morphing board.
/// Must run on the UI thread. Falls back to the base board (then the first board)
/// when the title has no match.
fn show_layer(win: &MainWindow, connected: bool, title: slint::SharedString) {
    use slint::Model;
    win.set_live_connected(connected);
    win.set_active_layer(title.clone());
    let idx = win.get_live_sheet() as usize;
    let Some(sheet) = win.get_sheets().row_data(idx) else { return };
    let boards = sheet.boards;
    let chosen = boards
        .iter()
        .find(|b| if title.is_empty() { b.is_base } else { b.title == title })
        .or_else(|| boards.iter().find(|b| b.is_base))
        .or_else(|| boards.row_data(0));
    if let Some(b) = chosen {
        win.set_active_board(b);
    }
}

/// Subscribe to live layer state from `keyd listen` on a background thread, pushing
/// updates onto the UI thread. Degrades gracefully to "live view off" when the keyd
/// socket isn't accessible (e.g. not in the `keyd` group).
fn spawn_live(win: &MainWindow) {
    let weak = win.as_weak();
    std::thread::spawn(move || {
        layer::run_listen(move |state| {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = weak.upgrade() {
                    let title = resolve_title(&win, &state.active);
                    show_layer(&win, state.connected, title);
                }
            });
        });
    });
}

/// `--demo`: cycle the highlighted layer through base + each layer on a timer, so
/// the live single-board view can be seen without a running keyd / keyd-group access.
fn spawn_demo(win: &MainWindow) {
    use slint::Model;
    // Demo the live single-board view: morph one board through the layers.
    win.set_live_mode(true);
    let live_idx = win.get_live_sheet() as usize;
    let mut cycle: Vec<slint::SharedString> = vec!["".into()];
    if let Some(sheet) = win.get_sheets().row_data(live_idx) {
        for board in sheet.boards.iter() {
            if !board.is_base && !cycle.contains(&board.title) {
                cycle.push(board.title.clone());
            }
        }
    }
    let weak = win.as_weak();
    std::thread::spawn(move || {
        let mut i = 0;
        loop {
            let layer = cycle[i % cycle.len()].clone();
            i += 1;
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = weak.upgrade() {
                    show_layer(&win, true, layer);
                }
            });
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }
    });
}
