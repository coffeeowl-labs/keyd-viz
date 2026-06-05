//! keyd-viz — native GUI cheatsheet for keyd.
//!
//! Parses keyd config(s), builds the semantic board model in `keydviz-core`, and
//! renders it with Slint. By default it detects connected keyboards and shows only
//! the config(s) governing them; with explicit path args it shows exactly those.

mod devices;
mod layer;
mod monitor;
mod prefs;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use devices::InputDevice;
use keydviz_core::board::{KeyCap, KeyState};
use keydviz_core::{catalog, import_qmk, parse_file, parse_text, Config, Geometry, Ids, Sheet};
use slint::{Brush, Color, Model, ModelRc, VecModel};

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
        x: k.x,
        y: k.y,
        width: k.width,
        height: k.height,
        rotation: k.r,
        rx: k.rx,
        ry: k.ry,
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

fn to_sheet_data(sheet: &Sheet, device: &str, layout_id: &str) -> SheetData {
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
        layout_id: layout_id.into(),
        boards: model(boards),
    }
}

/// Everything needed to (re)build one sheet, retained so the layout picker can morph it
/// to a different geometry without re-reading the config. `qmk` is set for boards
/// imported from QMK (whose geometry is fixed and not catalog-pickable); otherwise the
/// geometry comes from the curated catalog by `layout_id`.
struct SheetSrc {
    path: PathBuf,
    cfg: Config,
    device: String,
    layout_id: String,
    qmk: Option<(Geometry, String)>,
}

impl SheetSrc {
    /// A catalog-backed source for a parsed config, defaulting the layout to the saved
    /// choice (if any) or the name-based guess.
    fn catalog(path: &Path, cfg: &Config, device: &str) -> Self {
        let path_str = path.to_string_lossy().into_owned();
        // `--layout <id>` forces a layout (handy for testing); else the saved choice,
        // else the name-based guess.
        let layout_id = flag_value("--layout")
            .filter(|id| catalog::name(id).is_some())
            .or_else(|| prefs::load(&path_str))
            .unwrap_or_else(|| catalog::guess(&path_str).to_string());
        SheetSrc { path: path.to_path_buf(), cfg: cfg.clone(), device: device.into(), layout_id, qmk: None }
    }
}

/// Render a `SheetSrc` to display data with its current geometry (catalog or QMK).
fn build_sheet_data(src: &SheetSrc) -> SheetData {
    let path_str = src.path.to_string_lossy();
    let (geom, profile, layout_id) = match &src.qmk {
        Some((g, prof)) => (g.clone(), prof.clone(), String::new()),
        None => {
            let id = &src.layout_id;
            let g = catalog::geometry(id).unwrap_or_else(|| {
                catalog::geometry("ansi60").expect("ansi60 always exists")
            });
            let name = catalog::name(id).unwrap_or("ANSI 60%");
            (g, name.to_string(), id.clone())
        }
    };
    let sheet = Sheet::build(&src.cfg, &path_str, &geom, &profile);
    to_sheet_data(&sheet, &src.device, &layout_id)
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

/// The result of deciding what to show: the sheet sources (rebuildable so the picker
/// can change geometry), a `vendor:product → sheet-index` map for following the
/// last-pressed keyboard, and a subtitle.
struct Detection {
    srcs: Vec<SheetSrc>,
    /// `(vendor:product, index into srcs)` for each matched connected keyboard.
    device_map: Vec<(String, i32)>,
    subtitle: String,
}

impl Detection {
    fn new(srcs: Vec<SheetSrc>, subtitle: String) -> Self {
        Detection { srcs, device_map: Vec::new(), subtitle }
    }
}

/// The value following `--flag` on the command line, if present.
fn flag_value(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// `--qmk-info <info.json>` path: render a keyd config on a board imported from QMK.
///
/// The geometry + key identities come from QMK (`info.json` zipped index-wise with the
/// default keymap's keycodes); the keyd bindings come from the first positional
/// `*.conf` (or an empty config — a plain labeled board — if none is given). Optional
/// `--qmk-keymap <keymap.json>` supplies identities; `--qmk-layout <NAME>` picks the
/// variant when a board defines several.
fn qmk_detection(info_path: &str) -> Result<Detection, String> {
    let info = std::fs::read_to_string(info_path).map_err(|e| format!("{info_path}: {e}"))?;
    let keymap = match flag_value("--qmk-keymap") {
        Some(p) => Some(std::fs::read_to_string(&p).map_err(|e| format!("{p}: {e}"))?),
        None => None,
    };
    let prefer = flag_value("--qmk-layout");
    let imp = import_qmk(&info, keymap.as_deref(), prefer.as_deref())?;

    // Overlay config: the first positional *.conf, else an empty config (plain board).
    let conf = std::env::args().skip(1).find(|a| a.ends_with(".conf"));
    let (source, cfg) = match &conf {
        Some(p) => (p.clone(), parse_file(Path::new(p)).map_err(|e| format!("{p}: {e}"))?),
        None => ("(no config)".to_string(), parse_text("")),
    };

    let profile = format!("QMK · {}", imp.layout_name);
    let unmapped = if imp.unmapped > 0 {
        format!(" \u{2014} {} slot(s) unmapped", imp.unmapped)
    } else {
        String::new()
    };
    let subtitle = format!(
        "QMK import: {} ({} keys){unmapped}",
        imp.layout_name,
        imp.geometry.slots.len()
    );
    let src = SheetSrc {
        path: PathBuf::from(source),
        cfg,
        device: String::new(),
        layout_id: String::new(),
        qmk: Some((imp.geometry, profile)),
    };
    Ok(Detection { srcs: vec![src], device_map: Vec::new(), subtitle })
}

/// Decide which sheets to render, the device→sheet map, and a subtitle.
///
/// - Explicit path args  → render exactly those configs (no device map).
/// - Otherwise           → glob `/etc/keyd/*.conf`, detect connected keyboards,
///   and render only the matching configs (labeled with the device). If nothing
///   matches, fall back to showing all configs. If `/etc/keyd` is empty, fall back
///   to the bundled examples.
fn gather_sheets() -> Detection {
    // Positional config paths: skip flags and the value that follows a value-flag
    // (only `--layout` reaches this path; the `--qmk-*` flags route to qmk_detection).
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut args: Vec<PathBuf> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--layout" => i += 2,
            a if a.starts_with('-') => i += 1,
            a => {
                args.push(PathBuf::from(a));
                i += 1;
            }
        }
    }
    if !args.is_empty() {
        let srcs: Vec<SheetSrc> =
            parse_configs(&args).iter().map(|(p, c)| SheetSrc::catalog(p, c, "")).collect();
        let n = srcs.len();
        return Detection::new(srcs, format!("{n} config(s) from arguments"));
    }

    let conf_paths = conf_files_in(Path::new("/etc/keyd"));
    if conf_paths.is_empty() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let srcs: Vec<SheetSrc> = parse_configs(&conf_files_in(&examples))
            .iter()
            .map(|(p, c)| SheetSrc::catalog(p, c, ""))
            .collect();
        let n = srcs.len();
        return Detection::new(
            srcs,
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
        let srcs: Vec<SheetSrc> =
            configs.iter().map(|(p, c)| SheetSrc::catalog(p, c, "")).collect();
        let n = srcs.len();
        return Detection::new(srcs, format!("{n} config(s) \u{2014} no connected keyboard detected"));
    }

    let mut srcs = Vec::new();
    let mut device_map: Vec<(String, i32)> = Vec::new();
    for (ci, (path, cfg)) in configs.iter().enumerate() {
        if per_config[ci].is_empty() {
            continue;
        }
        let idx = srcs.len() as i32;
        let label = device_label(&devices, &per_config[ci]);
        srcs.push(SheetSrc::catalog(path, cfg, &label));
        // Map every device id that matched this config to its sheet index (deduped).
        for &di in &per_config[ci] {
            let devid = devices[di].devid();
            if !device_map.iter().any(|(d, _)| *d == devid) {
                device_map.push((devid, idx));
            }
        }
    }
    let n = srcs.len();
    Detection { srcs, device_map, subtitle: format!("{n} connected keyboard(s) detected") }
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
    let det = match flag_value("--qmk-info") {
        Some(info) => match qmk_detection(&info) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        },
        None => gather_sheets(),
    };
    let sheets_data: Vec<SheetData> = det.srcs.iter().map(build_sheet_data).collect();

    // `--list`: print the detection result to stdout and exit (no GUI). Useful for
    // debugging device detection and for scripting.
    if std::env::args().any(|a| a == "--list") {
        println!("{}", det.subtitle);
        for s in &sheets_data {
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

    // The layout picker. Hidden for QMK-imported boards (their geometry is fixed, not
    // catalog-pickable); otherwise the full curated library.
    let any_qmk = det.srcs.iter().any(|s| s.qmk.is_some());
    let layouts: Vec<LayoutChoice> = if any_qmk {
        Vec::new()
    } else {
        catalog::list()
            .iter()
            .map(|b| LayoutChoice { id: b.id.into(), name: b.name.into() })
            .collect()
    };
    win.set_layouts(model(layouts));

    // All detected sheets are kept as storage so a keypress can switch the view to
    // whichever keyboard was last pressed; only one is shown at a time (`active_sheet`).
    let first = sheets_data.first().cloned();
    let device_map: Vec<DeviceMatch> = det
        .device_map
        .iter()
        .map(|(d, i)| DeviceMatch { devid: d.clone().into(), sheet: *i })
        .collect();
    win.set_sheets(model(sheets_data));
    win.set_device_map(model(device_map));
    win.set_active_index(0);
    if let Some(active) = first {
        win.set_active_sheet(active);
    }
    // Seed the base board so the window is never blank before keyd connects.
    render_board(&win);

    // Layout picker: re-lay-out the active keyboard to the chosen geometry, update both
    // the live `active_sheet` and its stored entry (so following-the-keyboard keeps the
    // choice), persist it, and re-stamp the board. QMK sheets ignore this.
    let srcs = Rc::new(RefCell::new(det.srcs));
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        win.on_pick_layout(move |id| {
            let Some(win) = weak.upgrade() else { return };
            let idx = win.get_active_index().max(0) as usize;
            let mut srcs = srcs.borrow_mut();
            let Some(src) = srcs.get_mut(idx) else { return };
            if src.qmk.is_some() {
                return;
            }
            src.layout_id = id.to_string();
            let data = build_sheet_data(src);
            win.get_sheets().set_row_data(idx, data.clone());
            win.set_active_sheet(data);
            prefs::save(&src.path.to_string_lossy(), &src.layout_id);
            render_board(&win);
        });
    }

    // Keyboard switcher: manually show a different detected keyboard's board. keyd
    // aggregates keyboards into one virtual device, so the keypress stream can't
    // auto-follow which one you're on — this is the manual flip.
    {
        let weak = win.as_weak();
        win.on_pick_keyboard(move |idx| {
            let Some(win) = weak.upgrade() else { return };
            if let Some(sheet) = win.get_sheets().row_data(idx as usize) {
                win.set_active_index(idx);
                win.set_active_sheet(sheet);
                render_board(&win);
            }
        });
    }

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

    // stamp the live keypress glow onto the caps whose keyd output is held down. keyd
    // reports the post-remap keysym set, so a cap carries the full chord it emits
    // (`leftcontrol+left`, `leftshift+9`). A cap fires when every keysym it emits is held;
    // a more-specific cap suppresses its subsets so pressing nav `n` (=C-left) lights only
    // n, not the real Ctrl and the arrow key it also reports.
    let pressed: std::collections::HashSet<String> =
        win.get_pressed_keys().iter().map(|s| s.to_string()).collect();
    stamp_glow(&mut board, &pressed);
    win.set_active_board(board);
}

/// Light up the caps the held keysyms (`pressed`, what `keyd monitor` reports) map to.
/// Each cap's `key` is the `+`-joined chord it emits; a cap fires when that whole set is
/// held, and a cap whose set is a strict subset of another firing cap — or an equal,
/// non-emphasized twin — is suppressed, so only the key you actually pressed glows.
fn stamp_glow(board: &mut BoardData, pressed: &std::collections::HashSet<String>) {
    use slint::Model;
    if pressed.is_empty() {
        return;
    }
    let caps: Vec<KeyCapData> = board.keys.iter().collect();
    let sets: Vec<Vec<String>> = caps
        .iter()
        .map(|k| k.key.split('+').filter(|s| !s.is_empty()).map(str::to_string).collect())
        .collect();
    let emph: Vec<bool> = caps.iter().map(|k| k.emphasized).collect();
    let glow = resolve_glow(&sets, &emph, pressed);
    let keys: Vec<KeyCapData> = caps
        .into_iter()
        .enumerate()
        .map(|(i, mut k)| {
            k.pressed = glow[i];
            k
        })
        .collect();
    board.keys = model(keys);
}

/// Decide which caps glow, given each cap's emitted keysym set (`sets`), whether it is an
/// emphasized remap (`emph`), and the currently-held keysyms (`pressed`). A cap *fires*
/// when every keysym it emits is held; it is then suppressed by any other firing cap that
/// emits a strict superset (more specific — `n`=C-left beats the plain Ctrl and arrow it
/// also reports), or by an equal-set emphasized twin (the remapped key beats its
/// passthrough double, e.g. num-layer `j`=4 over the top-row `4`).
fn resolve_glow(
    sets: &[Vec<String>],
    emph: &[bool],
    pressed: &std::collections::HashSet<String>,
) -> Vec<bool> {
    let fires: Vec<bool> =
        sets.iter().map(|s| !s.is_empty() && s.iter().all(|x| pressed.contains(x))).collect();
    let subset = |a: &[String], b: &[String]| a.iter().all(|x| b.contains(x));
    (0..sets.len())
        .map(|i| {
            fires[i]
                && !(0..sets.len()).any(|j| {
                    j != i
                        && fires[j]
                        && subset(&sets[i], &sets[j])
                        && (sets[i].len() < sets[j].len()
                            || (sets[i].len() == sets[j].len() && emph[j] && !emph[i]))
                })
        })
        .collect()
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

    let monitor::Press { switch_to, pressed } =
        monitor::next_press_state(&k, &map, win.get_active_index(), &pressed_now);
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

#[cfg(test)]
mod glow_tests {
    use super::resolve_glow;
    use std::collections::HashSet;

    fn set(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }
    fn held(parts: &[&str]) -> HashSet<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn chord_cap_suppresses_its_subsets() {
        // nav `n`=C-left (leftcontrol+left), the real Ctrl cap, and `h`=left arrow.
        let sets = vec![set(&["leftcontrol", "left"]), set(&["leftcontrol"]), set(&["left"])];
        let emph = vec![true, false, true];
        let glow = resolve_glow(&sets, &emph, &held(&["leftcontrol", "left"]));
        assert_eq!(glow, vec![true, false, false], "only the n chord cap glows");
    }

    #[test]
    fn plain_ctrl_still_glows_alone() {
        // Holding only Ctrl: the chord cap can't fire, the Ctrl cap does.
        let sets = vec![set(&["leftcontrol", "left"]), set(&["leftcontrol"]), set(&["left"])];
        let emph = vec![true, false, true];
        let glow = resolve_glow(&sets, &emph, &held(&["leftcontrol"]));
        assert_eq!(glow, vec![false, true, false]);
    }

    #[test]
    fn emphasized_twin_wins_over_passthrough() {
        // num-layer `j`=4 (emphasized) vs the top-row passthrough `4`.
        let sets = vec![set(&["4"]), set(&["4"])];
        let emph = vec![true, false];
        let glow = resolve_glow(&sets, &emph, &held(&["4"]));
        assert_eq!(glow, vec![true, false]);
    }

    #[test]
    fn shifted_symbol_chord_suppresses_digit_and_shift() {
        // sym `j`=S-9 (leftshift+9) vs the real Shift cap and the passthrough `9`.
        let sets = vec![set(&["leftshift", "9"]), set(&["leftshift"]), set(&["9"])];
        let emph = vec![true, false, false];
        let glow = resolve_glow(&sets, &emph, &held(&["leftshift", "9"]));
        assert_eq!(glow, vec![true, false, false]);
    }

    #[test]
    fn empty_set_never_glows() {
        // the held layer-activator carries no output keysym.
        let sets = vec![Vec::new(), set(&["a"])];
        let emph = vec![false, false];
        let glow = resolve_glow(&sets, &emph, &held(&["a"]));
        assert_eq!(glow, vec![false, true]);
    }
}
