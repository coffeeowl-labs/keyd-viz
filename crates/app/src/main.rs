//! keyd-viz — native GUI cheatsheet for keyd.
//!
//! Parses keyd config(s), builds the semantic board model in `keydviz-core`, and
//! renders it with Slint. By default it detects connected keyboards and shows only
//! the config(s) governing them; with explicit path args it shows exactly those.

mod devices;
mod editing;
mod helper;
mod layer;
mod monitor;
mod prefs;
mod probe;
mod tray;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use devices::InputDevice;
use keydviz_core::board::{KeyCap, KeyState};
use keydviz_core::{catalog, import_qmk, parse_file, parse_text, Config, Geometry, Ids, Sheet};
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
        badge_left: bl_text.into(),
        badge_left_color: brush(if bl_color.is_empty() { "#000000" } else { &bl_color }),
        has_badge_left: k.badge_left.is_some(),
        badge_right: br_text.into(),
        badge_right_color: brush(if br_color.is_empty() { "#000000" } else { &br_color }),
        has_badge_right: k.badge_right.is_some(),
    }
}

fn to_sheet_data(sheet: &Sheet, device: &str, layout_id: &str, matched_ids: &[String]) -> SheetData {
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

/// Everything needed to (re)build one sheet, retained so the layout picker can morph it
/// to a different geometry without re-reading the config. `qmk` is set for boards
/// imported from QMK (whose geometry is fixed and not catalog-pickable); otherwise the
/// geometry comes from the curated catalog by `layout_id`.
struct SheetSrc {
    path: PathBuf,
    cfg: Config,
    device: String,
    /// Concrete `vendor:product` ids of connected keyboards that matched this config, so
    /// the UI can highlight which `[ids]` entry is currently plugged in.
    matched_ids: Vec<String>,
    layout_id: String,
    qmk: Option<(Geometry, String)>,
}

impl SheetSrc {
    /// A catalog-backed source for a parsed config, defaulting the layout to the saved
    /// choice (if any) or the name-based guess.
    fn catalog(path: &Path, cfg: &Config, device: &str, matched_ids: Vec<String>) -> Self {
        let path_str = path.to_string_lossy().into_owned();
        // `--layout <id>` forces a layout (handy for testing); else the saved choice,
        // else the name-based guess.
        let layout_id = flag_value("--layout")
            .filter(|id| catalog::name(id).is_some())
            .or_else(|| prefs::load(&path_str))
            .unwrap_or_else(|| catalog::guess(&path_str).to_string());
        SheetSrc {
            path: path.to_path_buf(),
            cfg: cfg.clone(),
            device: device.into(),
            matched_ids,
            layout_id,
            qmk: None,
        }
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
    to_sheet_data(&sheet, &src.device, &layout_id, &src.matched_ids)
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
    /// Keep following connected keyboards while running (re-highlight `[ids]` and refresh
    /// the device map on hotplug)? True for the auto-detect paths; false for explicit path
    /// args and QMK import, which show exactly what was asked for.
    live_devices: bool,
}

impl Detection {
    /// An auto-detect result that keeps following devices (used by the detection fallbacks).
    fn new(srcs: Vec<SheetSrc>, subtitle: String) -> Self {
        Detection { srcs, device_map: Vec::new(), subtitle, live_devices: true }
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
        matched_ids: Vec::new(),
        layout_id: String::new(),
        qmk: Some((imp.geometry, profile)),
    };
    Ok(Detection { srcs: vec![src], device_map: Vec::new(), subtitle, live_devices: false })
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
            // value-flags: skip the flag *and* its argument so the value isn't picked up
            // as a positional config path.
            "--layout" | "--helper-socket" => i += 2,
            a if a.starts_with('-') => i += 1,
            a => {
                args.push(PathBuf::from(a));
                i += 1;
            }
        }
    }
    if !args.is_empty() {
        let srcs: Vec<SheetSrc> = parse_configs(&args)
            .iter()
            .map(|(p, c)| SheetSrc::catalog(p, c, "", Vec::new()))
            .collect();
        let n = srcs.len();
        return Detection {
            srcs,
            device_map: Vec::new(),
            subtitle: format!("{n} config(s) from arguments"),
            live_devices: false,
        };
    }

    let conf_paths = conf_files_in(Path::new("/etc/keyd"));
    if conf_paths.is_empty() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let srcs: Vec<SheetSrc> = parse_configs(&conf_files_in(&examples))
            .iter()
            .map(|(p, c)| SheetSrc::catalog(p, c, "", Vec::new()))
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
            let rank = ids.match_device(&devid, dev.flags).rank();
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
            configs.iter().map(|(p, c)| SheetSrc::catalog(p, c, "", Vec::new())).collect();
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
        // Concrete ids of the keyboards that matched this config (deduped) — drives both
        // the device→sheet map and the "which [ids] entry is plugged in" highlight.
        let mut matched_ids: Vec<String> = Vec::new();
        for &di in &per_config[ci] {
            let devid = devices[di].devid();
            if !matched_ids.contains(&devid) {
                matched_ids.push(devid);
            }
        }
        srcs.push(SheetSrc::catalog(path, cfg, &label, matched_ids.clone()));
        for devid in matched_ids {
            if !device_map.iter().any(|(d, _)| *d == devid) {
                device_map.push((devid, idx));
            }
        }
    }
    let n = srcs.len();
    Detection {
        srcs,
        device_map,
        subtitle: format!("{n} connected keyboard(s) detected"),
        live_devices: true,
    }
}

/// Per-source `(matched vendor:product ids, device label)`, plus a `vendor:product → sheet`
/// map — the result of (re)matching connected keyboards against the sheet sources.
type DeviceMatching = (Vec<(Vec<String>, String)>, Vec<(String, i32)>);

/// Re-scan connected keyboards and re-match them against the current sheet sources,
/// returning per-source `(matched ids, device label)` and a fresh `vendor:product → sheet`
/// map. Same matching as [`gather_sheets`], but over the already-chosen sources — so a
/// hotplugged keyboard refreshes the id highlight, the device label, and the
/// follow-keyboard map without re-deciding which configs are shown.
fn rescan(srcs: &[SheetSrc]) -> DeviceMatching {
    let matchers: Vec<Ids> = srcs.iter().map(|s| Ids::parse(&s.cfg.ids)).collect();
    let devices = devices::connected_devices();
    let mut per_src: Vec<Vec<usize>> = vec![Vec::new(); srcs.len()];
    for (di, dev) in devices.iter().enumerate() {
        let devid = dev.devid();
        let mut best: Option<(usize, u8)> = None;
        for (ci, ids) in matchers.iter().enumerate() {
            let rank = ids.match_device(&devid, dev.flags).rank();
            if rank > 0 && best.is_none_or(|(_, br)| rank > br) {
                best = Some((ci, rank));
            }
        }
        if let Some((ci, _)) = best {
            per_src[ci].push(di);
        }
    }
    let mut out = Vec::with_capacity(srcs.len());
    let mut device_map: Vec<(String, i32)> = Vec::new();
    for (ci, idxs) in per_src.iter().enumerate() {
        let label = device_label(&devices, idxs);
        let mut ids: Vec<String> = Vec::new();
        for &di in idxs {
            let devid = devices[di].devid();
            if !ids.contains(&devid) {
                ids.push(devid.clone());
            }
            if !device_map.iter().any(|(d, _)| *d == devid) {
                device_map.push((devid, ci as i32));
            }
        }
        out.push((ids, label));
    }
    (out, device_map)
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

// The tray handle, held on the UI thread. The tray is a process singleton and
// `render_board` (always on the UI thread) pushes the active layer into its tooltip, so a
// thread-local avoids threading the handle through every render call site.
thread_local! {
    static TRAY: RefCell<Option<tray::TrayHandle>> = const { RefCell::new(None) };
}

/// Show the window if hidden, hide it if shown — the tray's summon/dismiss. Uses Slint's
/// own `show`/`hide`, which map/unmap the toplevel surface (so a hidden window also drops
/// its taskbar entry — the app then lives only in the tray) and, unlike poking winit's
/// `set_visible` underneath Slint, actually take effect on the Wayland backend. On show we
/// request user attention (taskbar flash) as a focus hint; a true raise-to-front on
/// Wayland needs an xdg-activation token winit can't yet consume, so that part is
/// best-effort.
fn toggle_window(win: &MainWindow) {
    use i_slint_backend_winit::winit::window::UserAttentionType;
    use i_slint_backend_winit::WinitWindowAccessor;
    if win.window().is_visible() {
        let _ = win.hide();
    } else {
        let _ = win.show();
        win.window()
            .with_winit_window(|w| w.request_user_attention(Some(UserAttentionType::Informational)));
    }
}

fn main() -> Result<(), slint::PlatformError> {
    // `--probe`: print what the installed keyd can do for Edit Mode and exit (no
    // GUI). Diagnostic for the version-dependent capabilities (edit-mode design §6).
    if std::env::args().any(|a| a == "--probe") {
        println!("{}", probe::KeydProbe::run().summary());
        return Ok(());
    }

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
    let live_devices = det.live_devices;
    let srcs = Rc::new(RefCell::new(det.srcs));
    // Edit mode (Phase 6 E1): one optional session; `Some(_)` == editing. Created here
    // so the keyboard switcher and the config-reload timer can respect it.
    let session: SharedSession = Rc::new(RefCell::new(None));
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
            publish_sheet(&win, idx, src);
            prefs::save(&src.path.to_string_lossy(), &src.layout_id);
            render_board(&win);
        });
    }

    // Keyboard switcher: manually show a different detected keyboard's board. keyd
    // aggregates keyboards into one virtual device, so the keypress stream can't
    // auto-follow which one you're on — this is the manual flip.
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_pick_keyboard(move |idx| {
            let Some(win) = weak.upgrade() else { return };
            if session.borrow().is_some() {
                // An edit session is per-file; switching keyboards leaves it. Confirm
                // first if it's dirty, stashing the target to switch to on confirm.
                if win.get_edit_dirty() {
                    win.set_pending_kbd(idx);
                    win.set_discard_prompt(true);
                    return;
                }
                exit_edit(&win, &srcs, &session);
            }
            switch_keyboard(&win, idx);
        });
    }

    // Always-on-top: the UI computes `pinned || compact` and calls this with the effective
    // state. We drive the underlying winit window directly so it works regardless of the WM
    // (KWin's titlebar pin is On-All-Desktops, not keep-above).
    {
        let weak = win.as_weak();
        win.on_apply_on_top(move |on| {
            let Some(win) = weak.upgrade() else { return };
            set_window_on_top(&win, on);
        });
    }

    // ---- edit mode (Phase 6 E1: draft-then-install) ----------------------------

    // Edit toggle: open a session for the active sheet's file (the §5.1 gate may
    // refuse → view-only with a visible reason), or leave edit mode, discarding the
    // unsaved preview.
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_toggle_edit(move || {
            let Some(win) = weak.upgrade() else { return };
            if session.borrow().is_some() {
                // Leaving a dirty session: confirm before discarding the preview.
                if win.get_edit_dirty() {
                    win.set_pending_kbd(-1);
                    win.set_discard_prompt(true);
                } else {
                    exit_edit(&win, &srcs, &session);
                }
                return;
            }
            let idx = win.get_active_index().max(0) as usize;
            let (path, qmk, banner) = {
                let srcs = srcs.borrow();
                let Some(src) = srcs.get(idx) else { return };
                (src.path.clone(), src.qmk.is_some(), affected_line(src))
            };
            if qmk {
                win.set_edit_banner("QMK-imported boards are view-only".into());
                return;
            }
            match editing::EditSession::open(&path) {
                Ok(s) => {
                    let choices = edit_layer_choices(&s);
                    // Default to the first real section (usually `main`, but a config
                    // whose bindings live only in layers has none — pick what exists).
                    let default_layer =
                        choices.first().map(|c| c.name.clone()).unwrap_or("main".into());
                    win.set_edit_layers(model(choices));
                    win.set_edit_layer(default_layer);
                    win.set_selected_phys("".into());
                    win.set_edit_current("".into());
                    win.set_edit_value("".into());
                    win.set_edit_dirty(false);
                    win.set_draft_info("".into());
                    win.set_capture_armed(false);
                    win.set_edit_banner(banner.into());
                    *session.borrow_mut() = Some(s);
                    win.set_edit_mode(true);
                    render_board(&win); // freeze the board to the chosen section
                }
                Err(v) => win.set_edit_banner(v.describe().into()),
            }
        });
    }

    // A cap was clicked: select it and seed the value field with its current binding
    // in the chosen section.
    {
        let weak = win.as_weak();
        let session = session.clone();
        win.on_select_key(move |phys| {
            let Some(win) = weak.upgrade() else { return };
            let sb = session.borrow();
            let Some(s) = sb.as_ref() else { return };
            let layer = win.get_edit_layer().to_string();
            let cur = s.current_binding(&layer, &phys).unwrap_or_default();
            win.set_selected_phys(phys);
            win.set_edit_current(cur.clone().into());
            win.set_edit_value(cur.into());
            win.set_capture_armed(false);
        });
    }

    // Section chooser: edits land in this section, and the board freezes to its layer.
    {
        let weak = win.as_weak();
        let session = session.clone();
        win.on_pick_edit_layer(move |name| {
            let Some(win) = weak.upgrade() else { return };
            win.set_edit_layer(name.clone());
            let phys = win.get_selected_phys().to_string();
            if !phys.is_empty() {
                if let Some(s) = session.borrow().as_ref() {
                    let cur = s.current_binding(&name, &phys).unwrap_or_default();
                    win.set_edit_current(cur.clone().into());
                    win.set_edit_value(cur.into());
                }
            }
            render_board(&win);
        });
    }

    // Apply a binding (typed, palette chip, or captured keypress) to the selection;
    // the board preview re-derives through the same parser the viewer uses (§5.6).
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_apply_binding(move |value| {
            let Some(win) = weak.upgrade() else { return };
            let value = value.trim().to_string();
            let layer = win.get_edit_layer().to_string();
            let phys = win.get_selected_phys().to_string();
            if value.is_empty() || phys.is_empty() {
                return;
            }
            let mut sb = session.borrow_mut();
            let Some(s) = sb.as_mut() else { return };
            match s.set_binding(&layer, &phys, &value) {
                Ok(()) => {
                    let (cfg, dirty, path) = (s.config(), s.dirty(), s.path.clone());
                    drop(sb);
                    win.set_edit_current(value.clone().into());
                    win.set_edit_value(value.into());
                    win.set_edit_dirty(dirty);
                    win.set_capture_armed(false);
                    refresh_preview(&win, &srcs, &path, cfg);
                }
                Err(e) => win.set_edit_banner(format!("\u{26a0} {e}").into()),
            }
        });
    }

    // Arm/disarm press-to-capture; the next live key-down becomes the value (consumed
    // in [`handle_key_event`]).
    {
        let weak = win.as_weak();
        win.on_arm_capture(move || {
            let Some(win) = weak.upgrade() else { return };
            win.set_capture_armed(!win.get_capture_armed());
        });
    }

    // Save the draft and surface the verdict + diff + copy-paste install steps.
    {
        let weak = win.as_weak();
        let session = session.clone();
        win.on_save_draft(move || {
            let Some(win) = weak.upgrade() else { return };
            let sb = session.borrow();
            let Some(s) = sb.as_ref() else { return };
            let info = match s.save_draft() {
                Ok(saved) => draft_summary(s, &saved),
                Err(e) => format!("\u{26a0} draft save failed: {e}"),
            };
            win.set_draft_info(info.into());
        });
    }

    // Discard-guard confirm: drop the unsaved session and carry out the deferred
    // action (leave edit mode, and switch keyboards if that's what triggered it).
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_confirm_discard(move || {
            let Some(win) = weak.upgrade() else { return };
            let pending = win.get_pending_kbd();
            exit_edit(&win, &srcs, &session); // clears discard_prompt + pending_kbd
            if pending >= 0 {
                switch_keyboard(&win, pending);
            }
        });
    }
    {
        let weak = win.as_weak();
        win.on_cancel_discard(move || {
            let Some(win) = weak.upgrade() else { return };
            win.set_discard_prompt(false);
            win.set_pending_kbd(-1);
        });
    }

    let demo = std::env::args().any(|a| a == "--demo");
    if demo {
        spawn_demo(&win);
    } else {
        // Prefer the broker daemon (the zero-permission shipped path); fall back to
        // spawning keyd directly when it isn't *running* (dev). `--helper-socket <path>`
        // or `$KEYDVIZ_HELPER_SOCKET` overrides the path and forces the broker source
        // (then it retries until the daemon comes up). For auto-discovery we probe
        // liveness, not mere file existence, so a stale socket can't strand us.
        let helper_flag = flag_value("--helper-socket");
        let socket = helper_flag.clone().unwrap_or_else(helper::socket_path);
        let forced = helper_flag.is_some() || std::env::var("KEYDVIZ_HELPER_SOCKET").is_ok();
        if forced || helper::is_live(&socket) {
            spawn_helper(&win, socket);
        } else {
            spawn_live(&win); // layer stream  (keyd listen)
            spawn_monitor(&win); // keypress stream (keyd monitor)
        }
    }

    // Keep a quick tap visible: expire the min-glow decay and repaint. Skipped in demo
    // mode, which drives the glow directly.
    let _glow_timer = (!demo).then(|| spawn_glow_decay(&win));

    // Follow keyboard hotplug: re-highlight matched ids + refresh the device map as
    // keyboards come and go. Only in auto-detect mode (not explicit args / QMK / demo).
    let _device_watch = (live_devices && !demo).then(|| spawn_device_watch(&win, srcs.clone()));

    // Live-reload the board when a watched config file changes on disk. Kept alive in a
    // binding so the timer outlives setup and fires for the app's whole life.
    let _reload_timer = spawn_config_reload(&win, srcs.clone(), session.clone());

    // Resident system-tray icon to summon/dismiss the window. Absent (with a warning) on
    // systems without a StatusNotifier host; the app runs normally either way. Held on the
    // UI thread so `render_board` can push the active layer into its tooltip.
    let tray = tray::spawn(&win);
    let has_tray = tray.is_some();
    TRAY.with(|t| *t.borrow_mut() = tray);

    if has_tray {
        // With a tray, the window's close button hides to the tray instead of quitting —
        // the app keeps running windowless and is resurrected by clicking the tray icon.
        // The only way out is the tray's Quit item (-> quit_event_loop), so we run the
        // loop until explicitly quit rather than until the last window closes. Without a
        // tray there'd be no way to bring the window back or quit, so we keep the default
        // close-to-quit behavior (win.run()).
        win.window()
            .on_close_requested(|| slint::CloseRequestResponse::HideWindow);
        win.show()?;
        slint::run_event_loop_until_quit()
    } else {
        win.run()
    }
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
    TRAY.with(|t| {
        if let Some(h) = t.borrow().as_ref() {
            h.set_layer(&title);
        }
    });

    // While editing, the shown board is frozen to the chosen section — the live stack
    // still drives the LIVE pill and tray tooltip above, just not which board renders.
    let shown: slint::SharedString = if win.get_edit_mode() {
        let lay = win.get_edit_layer();
        if lay.is_empty() || lay == "main" {
            slint::SharedString::default()
        } else {
            lay.to_uppercase().into()
        }
    } else {
        title
    };
    let chosen = boards
        .iter()
        .find(|b| if shown.is_empty() { b.is_base } else { b.title == shown })
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

/// The one optional edit session, shared by the UI callbacks that need to know
/// whether (and what) we're editing. `Some(_)` == edit mode is on.
type SharedSession = Rc<RefCell<Option<editing::EditSession>>>;

/// Rebuild sheet `idx` from its (already-mutated) source and push it into the model,
/// refreshing the visible `active_sheet` too when `idx` is the one on screen. The
/// shared tail of every place that changes a sheet (layout pick, edit preview, leave-
/// edit, disk reload, device hotplug) — callers update the `SheetSrc`, then publish.
fn publish_sheet(win: &MainWindow, idx: usize, src: &SheetSrc) {
    use slint::Model;
    let data = build_sheet_data(src);
    win.get_sheets().set_row_data(idx, data.clone());
    if win.get_active_index().max(0) as usize == idx {
        win.set_active_sheet(data);
    }
}

/// Switch the shown board to detected keyboard `idx` (no-op if out of range).
fn switch_keyboard(win: &MainWindow, idx: i32) {
    use slint::Model;
    if let Some(sheet) = win.get_sheets().row_data(idx as usize) {
        win.set_active_index(idx);
        win.set_active_sheet(sheet);
        render_board(win);
    }
}

/// The sections offered by the edit-mode chooser, straight from the file's real
/// sections (so `[main]` only appears when it exists — no chip that errors on click).
/// `name` is the exact base name [`editing::EditSession::set_binding`] targets; the
/// chip shows the same (file vocabulary, not display-case).
fn edit_layer_choices(s: &editing::EditSession) -> Vec<EditLayer> {
    s.editable_sections()
        .into_iter()
        .map(|n| EditLayer { name: n.clone().into(), display: n.into() })
        .collect()
}

/// Minimal §5.5 affected-keyboards line for the edit banner: which connected
/// device(s) the file being edited currently governs.
fn affected_line(src: &SheetSrc) -> String {
    let path = src.path.display();
    if !src.device.is_empty() {
        format!("{path} \u{2014} applies to {}", src.device)
    } else if !src.matched_ids.is_empty() {
        format!("{path} \u{2014} applies to {}", src.matched_ids.join(", "))
    } else {
        format!("{path} \u{2014} no connected keyboard matches this config")
    }
}

/// Leave edit mode: drop the session, clear the edit chrome, and discard the unsaved
/// preview by re-deriving the sheet from the file on disk.
fn exit_edit(win: &MainWindow, srcs: &Rc<RefCell<Vec<SheetSrc>>>, session: &SharedSession) {
    let Some(s) = session.borrow_mut().take() else { return };
    win.set_edit_mode(false);
    win.set_capture_armed(false);
    win.set_selected_phys("".into());
    win.set_edit_banner("".into());
    win.set_draft_info("".into());
    win.set_edit_dirty(false);
    win.set_discard_prompt(false);
    win.set_pending_kbd(-1);
    let mut srcs = srcs.borrow_mut();
    if let Some((idx, src)) = srcs.iter_mut().enumerate().find(|(_, x)| x.path == s.path) {
        if let Ok(cfg) = parse_file(&src.path) {
            src.cfg = cfg;
        }
        publish_sheet(win, idx, src);
    }
    drop(srcs);
    render_board(win);
}

/// Repaint the preview after an edit: swap the session-derived config into the sheet
/// source for `path` and rebuild — the same path a disk reload takes, so the preview
/// is exactly what the viewer would show for the saved file (§5.6).
fn refresh_preview(win: &MainWindow, srcs: &Rc<RefCell<Vec<SheetSrc>>>, path: &Path, cfg: Config) {
    let mut srcs = srcs.borrow_mut();
    if let Some((idx, src)) = srcs.iter_mut().enumerate().find(|(_, x)| x.path == path) {
        src.cfg = cfg;
        publish_sheet(win, idx, src);
    }
    drop(srcs);
    render_board(win);
}

/// Human summary of a saved draft for the panel: verdict, staleness, the change
/// diff, and the copy-paste install steps.
fn draft_summary(s: &editing::EditSession, saved: &editing::DraftSaved) -> String {
    let mut out = format!("draft saved: {}\n", saved.draft_path.display());
    match &saved.check {
        Some(Ok(())) => out.push_str("keyd check: OK\n"),
        Some(Err(e)) => out.push_str(&format!("\u{26a0} keyd check failed: {e}\n")),
        None => out.push_str("keyd not found \u{2014} draft not validated\n"),
    }
    if let Some(w) = &saved.stale_warning {
        out.push_str(&format!("\u{26a0} {w}\n"));
    }
    let diff = s.diff();
    if !diff.is_empty() {
        out.push('\n');
        out.push_str(diff.trim_end());
        out.push('\n');
    }
    out.push_str("\ninstall:\n");
    out.push_str(&saved.install_steps);
    out
}

/// File mtime, or `None` if the path is missing/unreadable (so a config saved via a
/// temporary file mid-write is simply skipped until it settles).
fn file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Watch each config's file mtime on the UI thread and live-reload the board when it
/// changes — editing your keyd `.conf` redraws the layout without a restart (layers and
/// Set the window's always-on-top level via the underlying winit window.
///
/// Works on **X11/XWayland** (winit toggles `_NET_WM_STATE_ABOVE`). On **native Wayland**
/// it is a no-op — winit's `set_window_level` is empty there because Wayland has no
/// client-side keep-above protocol by design; always-on-top is purely the compositor's
/// call. On Wayland, pin the window via the compositor instead (KDE: right-click the
/// titlebar → More Actions → Keep Above Window, or a KWin window rule for class `keydviz`).
/// Also a no-op on backends without a winit window.
fn set_window_on_top(win: &MainWindow, on: bool) {
    use i_slint_backend_winit::winit::window::WindowLevel;
    use i_slint_backend_winit::WinitWindowAccessor;
    let level = if on { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal };
    win.window().with_winit_window(|w| w.set_window_level(level));
}

/// glow are already live; this closes the gap for the base board). Polls once a second
/// (no extra deps, no background thread — Slint timer callbacks run on the event-loop
/// thread, so they can hold the non-`Send` `Rc` state). Reuses [`build_sheet_data`] and
/// [`render_board`], so the current layer/glow overlays are reapplied after the swap.
/// Returns the timer; keep it alive for the app's life or it stops.
fn spawn_config_reload(
    win: &MainWindow,
    srcs: Rc<RefCell<Vec<SheetSrc>>>,
    session: SharedSession,
) -> slint::Timer {
    let weak = win.as_weak();
    // Seed last-seen mtimes so we only reload on a *future* change, not at startup.
    let mut mtimes: Vec<Option<std::time::SystemTime>> =
        srcs.borrow().iter().map(|s| file_mtime(&s.path)).collect();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(1000), move || {
        let Some(win) = weak.upgrade() else { return };
        let mut srcs = srcs.borrow_mut();
        let mut changed = false;
        for (idx, src) in srcs.iter_mut().enumerate() {
            // The file being edited is exempt: its sheet shows the session's preview,
            // and save-time staleness detection covers external edits (§4).
            if session.borrow().as_ref().is_some_and(|s| s.path == src.path) {
                continue;
            }
            let now = file_mtime(&src.path);
            if now.is_none() || now == mtimes[idx] {
                continue; // missing (mid-save) or unchanged
            }
            mtimes[idx] = now;
            match parse_file(&src.path) {
                Ok(cfg) => {
                    src.cfg = cfg;
                    publish_sheet(&win, idx, src);
                    changed = true;
                    eprintln!("keyd-viz: reloaded {}", src.path.display());
                }
                Err(e) => eprintln!("keyd-viz: reload of {} failed: {e}", src.path.display()),
            }
        }
        drop(srcs);
        if changed {
            render_board(&win); // reapply the live layer/glow overlays onto the new board
        }
    });
    timer
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

/// Subscribe to the `keydviz-helperd` broker on a background thread: one socket carries
/// both the layer stream and (if the helper brokers keypresses) the glow. Each
/// [`LiveEvent`] is split onto the same UI paths the direct-`keyd` sources feed, so the
/// view behaves identically whether the source is the helper or a spawned `keyd`.
fn spawn_helper(win: &MainWindow, socket: String) {
    use keydviz_core::live::LiveEvent;
    let weak = win.as_weak();
    std::thread::spawn(move || {
        // Layer state is reduced here (the helper sends raw transitions, like keyd).
        let mut active = keydviz_core::live::ActiveLayers::default();
        helper::run_helper_client(
            &socket,
            {
                let weak = weak.clone();
                move |connected| {
                    let weak = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = weak.upgrade() {
                            win.set_live_connected(connected);
                            if !connected {
                                win.set_keys_connected(false);
                                win.set_active_stack(model(Vec::new()));
                                clear_glow(&win);
                                render_board(&win);
                            }
                        }
                    });
                }
            },
            move |ev| {
                // Reduce layer events on this thread; ship a plain snapshot to the UI.
                let layer_snapshot = ev.as_layer().map(|le| {
                    active.apply(&le);
                    active.active()
                });
                let monitor_ev = ev.as_monitor();
                let keys_connected = matches!(ev, LiveEvent::Key { .. });
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = weak.upgrade() {
                        if let Some(stack) = layer_snapshot {
                            let stack: Vec<slint::SharedString> =
                                stack.into_iter().map(Into::into).collect();
                            win.set_active_stack(model(stack));
                            win.set_live_connected(true);
                            render_board(&win);
                        }
                        if let Some(mev) = monitor_ev {
                            if keys_connected {
                                win.set_keys_connected(true);
                            }
                            handle_key_event(&win, mev);
                        }
                    }
                });
            },
        );
    });
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
                            clear_glow(&win);
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

/// Minimum time a key stays lit after it goes down, so a fast tap-hold *tap* — which keyd
/// emits as a near-simultaneous down+up at release — is still visible instead of flashing
/// for less than a display frame. (Why "fear" wouldn't light `f` but "after" would: an
/// isolated tap resolves on release and emits down+up together; a rolled tap emits the
/// down earlier, spread across frames. This makes both visible.)
///
/// Tuned to the minimum that reliably survives a painted frame: ~3–4 frames at 60Hz (more
/// at higher refresh), with margin for a dropped frame. Anchored to key-*down*, so any key
/// held at least this long turns off the instant it's released — most typists, even fast
/// ones, get no visible tail; only genuine sub-frame taps linger, just enough to be seen.
const MIN_GLOW: std::time::Duration = std::time::Duration::from_millis(60);

/// Glow bookkeeping (UI-thread only): the truly-held keys, plus a short per-key "keep lit
/// until" decay so quick taps remain visible. A thread-local because every keypress
/// callback runs on the Slint event loop and the `Rc`-backed models aren't `Send`, so it
/// can't be shared across the source threads anyway.
#[derive(Default)]
struct GlowState {
    /// Physically-held keys (from keyd's output), maintained by [`monitor::next_press_state`].
    held: Vec<String>,
    /// key → instant after which it may stop glowing (refreshed on every event for that key).
    until: std::collections::HashMap<String, std::time::Instant>,
}

impl GlowState {
    /// The set to light: everything held, plus any key still inside its decay window.
    /// Prunes expired decay entries as a side effect.
    fn glow_set(&mut self) -> Vec<String> {
        let now = std::time::Instant::now();
        self.until.retain(|_, t| *t > now);
        let mut out = self.held.clone();
        for k in self.until.keys() {
            if !out.iter().any(|h| h == k) {
                out.push(k.clone());
            }
        }
        out
    }

    fn clear(&mut self) {
        self.held.clear();
        self.until.clear();
    }
}

thread_local! {
    static GLOW: RefCell<GlowState> = RefCell::new(GlowState::default());
}

/// Drop all glow state and clear the rendered set — used when a source disconnects or the
/// shown keyboard switches (we don't know the new context's held keys).
fn clear_glow(win: &MainWindow) {
    GLOW.with(|g| g.borrow_mut().clear());
    win.set_pressed_keys(model(Vec::new()));
}

/// Apply one `keyd monitor` key event on the UI thread: follow the last-pressed keyboard
/// (switch the shown sheet), update the held set + min-glow decay, and re-render. The
/// held-set + follow-keyboard decision stays in [`monitor::next_press_state`] (pure,
/// tested); the decay overlay ([`GlowState`]) keeps fast taps visible.
fn handle_key_event(win: &MainWindow, ev: monitor::MonitorEvent) {
    use slint::Model;

    let monitor::MonitorEvent::Key(k) = ev else { return }; // ignore device add/remove

    // An armed press-to-capture eats this key-down as the new binding value. monitor
    // reports the *emitted* keysym, so capturing from a key that is itself remapped
    // reports its output — which is the symbol the user is choosing anyway.
    if win.get_edit_mode() && win.get_capture_armed() {
        if matches!(k.action, monitor::KeyAction::Down) {
            win.invoke_apply_binding(k.key.clone().into());
        }
        return; // the captured press shouldn't also glow or switch keyboards
    }

    let map: Vec<(String, i32)> =
        win.get_device_map().iter().map(|m| (m.devid.to_string(), m.sheet)).collect();

    GLOW.with(|g| {
        let mut g = g.borrow_mut();
        let monitor::Press { switch_to, pressed } =
            monitor::next_press_state(&k, &map, win.get_active_index(), &g.held);
        // Editing is per-file: don't let a keypress yank the view to another keyboard.
        if let Some(idx) = switch_to.filter(|_| !win.get_edit_mode()) {
            g.until.clear(); // new board: don't carry the old board's decaying glow
            if let Some(sheet) = win.get_sheets().row_data(idx as usize) {
                win.set_active_index(idx);
                win.set_active_sheet(sheet);
            }
        }
        g.held = pressed;
        // Floor the glow at MIN_GLOW from *key-down* only. A key held longer than that is
        // kept lit by `held` and turns off the instant it's released; only a sub-frame tap
        // (an isolated tap-hold tap, whose down+up land in one frame) lingers — just long
        // enough to be seen — so normal typing doesn't trail behind your fingers.
        if matches!(k.action, monitor::KeyAction::Down) {
            g.until.insert(k.key.clone(), std::time::Instant::now() + MIN_GLOW);
        }
        let glow: Vec<slint::SharedString> = g.glow_set().into_iter().map(Into::into).collect();
        win.set_pressed_keys(model(glow));
    });
    render_board(win);
}

/// Watch for keyboard hotplug: every ~1.5s, re-scan connected devices and — when the set of
/// matched keyboards changes — refresh each sheet's highlighted `[ids]` + device label and
/// the follow-the-keyboard map. Polling (not udev) keeps it dependency-free; a sysfs scan is
/// cheap and hotplug is rare. Returns the timer; keep it alive for the app's life.
fn spawn_device_watch(win: &MainWindow, srcs: Rc<RefCell<Vec<SheetSrc>>>) -> slint::Timer {
    let weak = win.as_weak();
    // Seed with the current match so the first tick doesn't redundantly repaint.
    let mut last: Vec<(String, i32)> = {
        let mut m = rescan(&srcs.borrow()).1;
        m.sort();
        m
    };
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(1500), move || {
        let Some(win) = weak.upgrade() else { return };
        let mut srcs = srcs.borrow_mut();
        let (per_src, device_map) = rescan(&srcs);
        let mut sig = device_map.clone();
        sig.sort();
        if sig == last {
            return; // no hotplug change since last scan
        }
        last = sig;
        for (i, src) in srcs.iter_mut().enumerate() {
            let (ids, label) = &per_src[i];
            src.matched_ids = ids.clone();
            src.device = label.clone();
            publish_sheet(&win, i, src);
        }
        drop(srcs);
        let device_map: Vec<DeviceMatch> =
            device_map.into_iter().map(|(d, i)| DeviceMatch { devid: d.into(), sheet: i }).collect();
        win.set_device_map(model(device_map));
        render_board(&win); // reapply the live layer/glow overlays onto the refreshed sheet
    });
    timer
}

/// Expire the min-glow decay: a few times a second, recompute the lit set and repaint if
/// it shrank. This is what turns a quick tap's glow back off when no further key events
/// arrive. Returns the timer; keep it alive for the app's life. (Not used in `--demo`,
/// which drives the glow directly.)
fn spawn_glow_decay(win: &MainWindow) -> slint::Timer {
    use slint::Model;
    let weak = win.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(16), move || {
        let Some(win) = weak.upgrade() else { return };
        let changed = GLOW.with(|g| {
            let mut glow = g.borrow_mut().glow_set();
            let mut cur: Vec<String> =
                win.get_pressed_keys().iter().map(|s| s.to_string()).collect();
            glow.sort();
            cur.sort();
            if glow != cur {
                let m: Vec<slint::SharedString> = glow.into_iter().map(Into::into).collect();
                win.set_pressed_keys(model(m));
                true
            } else {
                false
            }
        });
        if changed {
            render_board(&win);
        }
    });
    timer
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
