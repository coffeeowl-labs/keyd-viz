//! keyd-viz — native GUI cheatsheet for keyd.
//!
//! Parses keyd config(s), builds the semantic board model in `keydviz-core`, and
//! renders it with Slint. By default it detects connected keyboards and shows only
//! the config(s) governing them; with explicit path args it shows exactly those.

mod devices;
mod layer;
mod monitor;

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
        key: k.key.clone().into(),
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

/// The result of deciding what to show: the rendered sheets, a `vendor:product →
/// sheet-index` map for following the last-pressed keyboard, and a subtitle.
struct Detection {
    sheets: Vec<SheetData>,
    /// `(vendor:product, index into sheets)` for each matched connected keyboard.
    device_map: Vec<(String, i32)>,
    subtitle: String,
}

impl Detection {
    fn new(sheets: Vec<SheetData>, subtitle: String) -> Self {
        Detection { sheets, device_map: Vec::new(), subtitle }
    }
}

/// Decide which sheets to render, the device→sheet map, and a subtitle.
///
/// - Explicit path args  → render exactly those configs (no device map).
/// - Otherwise           → glob `/etc/keyd/*.conf`, detect connected keyboards,
///   and render only the matching configs (labeled with the device). If nothing
///   matches, fall back to showing all configs. If `/etc/keyd` is empty, fall back
///   to the bundled examples.
fn gather_sheets() -> Detection {
    let args: Vec<PathBuf> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .collect();
    if !args.is_empty() {
        let sheets: Vec<SheetData> =
            parse_configs(&args).iter().map(|(p, c)| sheet_from(p, c, "")).collect();
        let n = sheets.len();
        return Detection::new(sheets, format!("{n} config(s) from arguments"));
    }

    let conf_paths = conf_files_in(Path::new("/etc/keyd"));
    if conf_paths.is_empty() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let sheets: Vec<SheetData> =
            parse_configs(&conf_files_in(&examples)).iter().map(|(p, c)| sheet_from(p, c, "")).collect();
        let n = sheets.len();
        return Detection::new(
            sheets,
            format!("{n} example keyboard(s) \u{2014} no /etc/keyd configs found"),
        );
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
        return Detection::new(sheets, format!("{n} config(s) \u{2014} no connected keyboard detected"));
    }

    let mut sheets = Vec::new();
    let mut device_map: Vec<(String, i32)> = Vec::new();
    for (ci, (path, cfg)) in configs.iter().enumerate() {
        if per_config[ci].is_empty() {
            continue;
        }
        let idx = sheets.len() as i32;
        let label = device_label(&devices, &per_config[ci]);
        sheets.push(sheet_from(path, cfg, &label));
        // Map every device id that matched this config to its sheet index (deduped).
        for &di in &per_config[ci] {
            let devid = devices[di].devid();
            if !device_map.iter().any(|(d, _)| *d == devid) {
                device_map.push((devid, idx));
            }
        }
    }
    let n = sheets.len();
    Detection { sheets, device_map, subtitle: format!("{n} connected keyboard(s) detected") }
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
    let det = gather_sheets();

    // `--list`: print the detection result to stdout and exit (no GUI). Useful for
    // debugging device detection and for scripting.
    if std::env::args().any(|a| a == "--list") {
        println!("{}", det.subtitle);
        for s in &det.sheets {
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
    win.set_subtitle(det.subtitle.into());

    // All detected sheets are kept as storage so a keypress can switch the view to
    // whichever keyboard was last pressed; only one is shown at a time (`active_sheet`).
    let first = det.sheets.first().cloned();
    let device_map: Vec<DeviceMatch> = det
        .device_map
        .iter()
        .map(|(d, i)| DeviceMatch { devid: d.clone().into(), sheet: *i })
        .collect();
    win.set_sheets(model(det.sheets));
    win.set_device_map(model(device_map));
    win.set_active_index(0);
    if let Some(active) = first {
        win.set_active_sheet(active);
    }
    // Seed the base board so the window is never blank before keyd connects.
    render_board(&win);

    if std::env::args().any(|a| a == "--demo") {
        spawn_demo(&win);
    } else {
        spawn_live(&win); // layer stream  (keyd listen)
        spawn_monitor(&win); // keypress stream (keyd monitor)
    }

    win.run()
}

/// Rebuild the single displayed board from the live state held in window properties:
/// `active_sheet` (which keyboard), `active_stack` (the keyd layer stack), and
/// `pressed_keys` (held keys → glow). Resolves the stack (most-recent first) to the
/// topmost layer that actually has a board, falling back to the base board when
/// nothing held maps to one — e.g. a bare `control` mod, which keyd reports as a layer
/// but which has no dedicated board. Then stamps the pressed glow onto matching caps.
///
/// Must run on the UI thread: it reads `Rc`-backed models that aren't `Send`, so the
/// listen/monitor threads only ferry plain data here via `invoke_from_event_loop`.
fn render_board(win: &MainWindow) {
    use slint::Model;
    let boards = win.get_active_sheet().boards;

    // resolve the active layer stack to the title of a board that exists ("" = base)
    let stack: Vec<slint::SharedString> = win.get_active_stack().iter().collect();
    let mut title = slint::SharedString::default();
    for name in stack.iter().rev() {
        let upper = name.to_uppercase();
        if let Some(b) = boards.iter().find(|b| !b.is_base && b.title == upper) {
            title = b.title;
            break;
        }
    }
    win.set_active_layer(title.clone());

    let chosen = boards
        .iter()
        .find(|b| if title.is_empty() { b.is_base } else { b.title == title })
        .or_else(|| boards.iter().find(|b| b.is_base))
        .or_else(|| boards.row_data(0));
    let Some(mut board) = chosen else { return };

    // stamp the live keypress glow onto the caps whose keyd key name is held down
    let pressed: Vec<slint::SharedString> = win.get_pressed_keys().iter().collect();
    if !pressed.is_empty() {
        let rows: Vec<RowData> = board
            .rows
            .iter()
            .map(|row| {
                let keys: Vec<KeyCapData> = row
                    .keys
                    .iter()
                    .map(|mut k| {
                        k.pressed = pressed.iter().any(|p| p == &k.key);
                        k
                    })
                    .collect();
                RowData { keys: model(keys) }
            })
            .collect();
        board.rows = model(rows);
    }
    win.set_active_board(board);
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
                    let stack: Vec<slint::SharedString> =
                        state.active.iter().map(|s| s.clone().into()).collect();
                    win.set_active_stack(model(stack));
                    win.set_live_connected(state.connected);
                    render_board(&win);
                }
            });
        });
    });
}

/// Subscribe to live keypresses from `keyd monitor` on a background thread. Drives the
/// pressed-key glow and follows the last-pressed keyboard. Works wherever `/dev/input`
/// is readable (typically the `input` group); the shipped product routes this through
/// the privileged helper so even that isn't required (ROADMAP §1).
fn spawn_monitor(win: &MainWindow) {
    let weak = win.as_weak();
    let weak_conn = weak.clone();
    std::thread::spawn(move || {
        monitor::run_monitor(
            move |connected| {
                let weak = weak_conn.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = weak.upgrade() {
                        win.set_keys_connected(connected);
                        if !connected {
                            win.set_pressed_keys(model(Vec::new()));
                            render_board(&win);
                        }
                    }
                });
            },
            move |ev| {
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = weak.upgrade() {
                        handle_key_event(&win, ev);
                    }
                });
            },
        );
    });
}

/// Apply one `keyd monitor` key event on the UI thread: follow the last-pressed
/// keyboard (switch the shown sheet), maintain the pressed-key set, and re-render.
/// The decision logic lives in [`monitor::next_press_state`] (pure, tested); this
/// just reads the current state from the window and writes the result back.
fn handle_key_event(win: &MainWindow, ev: monitor::MonitorEvent) {
    use slint::Model;

    let monitor::MonitorEvent::Key(k) = ev else { return }; // ignore device add/remove

    let map: Vec<(String, i32)> =
        win.get_device_map().iter().map(|m| (m.devid.to_string(), m.sheet)).collect();
    let pressed_now: Vec<String> = win.get_pressed_keys().iter().map(|s| s.to_string()).collect();

    match monitor::next_press_state(&k, &map, win.get_active_index(), &pressed_now) {
        monitor::KeyOutcome::Ignore => {}
        monitor::KeyOutcome::Apply { switch_to, pressed } => {
            if let Some(idx) = switch_to {
                if let Some(sheet) = win.get_sheets().row_data(idx as usize) {
                    win.set_active_index(idx);
                    win.set_active_sheet(sheet);
                }
            }
            let pressed: Vec<slint::SharedString> = pressed.into_iter().map(Into::into).collect();
            win.set_pressed_keys(model(pressed));
            render_board(win);
        }
    }
}

/// `--demo`: animate the live view without a running keyd — sweep a pressed key across
/// the board (glow) while cycling the active layer, so both effects are visible.
fn spawn_demo(win: &MainWindow) {
    use slint::Model;
    // Layer cycle (base + each layer) as synthetic stacks.
    let mut layers: Vec<Vec<slint::SharedString>> = vec![Vec::new()];
    for board in win.get_active_sheet().boards.iter() {
        if !board.is_base {
            let stack = vec![board.title.clone()];
            if !layers.contains(&stack) {
                layers.push(stack);
            }
        }
    }
    // Home-row-ish sweep of keyd key names for the glow.
    let keys = ["a", "s", "d", "f", "g", "h", "j", "k", "l", "space"];
    let weak = win.as_weak();
    std::thread::spawn(move || {
        let mut i = 0usize;
        loop {
            let stack = layers[(i / keys.len()) % layers.len()].clone();
            let key: slint::SharedString = keys[i % keys.len()].into();
            i += 1;
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = weak.upgrade() {
                    win.set_live_connected(true);
                    win.set_keys_connected(true);
                    win.set_active_stack(model(stack));
                    win.set_pressed_keys(model(vec![key]));
                    render_board(&win);
                }
            });
            std::thread::sleep(std::time::Duration::from_millis(260));
        }
    });
}
