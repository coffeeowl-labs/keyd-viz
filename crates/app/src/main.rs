//! keyd-viz — native GUI cheatsheet for keyd.
//!
//! Parses keyd config(s), builds the semantic board model in `keydviz-core`, and
//! renders it with Slint. By default it detects connected keyboards and shows only
//! the config(s) governing them; with explicit path args it shows exactly those.

mod applying;
mod devices;
mod editing;
mod helper;
mod layer;
mod monitor;
mod picker;
mod prefs;
mod probe;
mod tray;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use devices::InputDevice;
use keydviz_core::board::{KeyCap, KeyState};
use keydviz_core::{
    catalog, import_qmk, parse_file, parse_text, Behavior, Config, Geometry, Ids, MatchKind, Sheet,
    MODIFIERS,
};
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

/// A connected keyboard no *specific* config governs — a create-config candidate
/// (design doc §5.5). Specifically-governed keyboards are excluded: the right action
/// for those is editing the governing config (reachable from the chooser), not
/// spawning a second file with a colliding id. Both unclaimed and wildcard-only
/// keyboards qualify, because a new specific config out-ranks the wildcard.
struct CreateCandidate {
    /// The raw device name (may be empty) — used as the new board's label.
    name: String,
    /// Chip label, e.g. `PFU HHKB (04fe:0021)`.
    label: String,
    /// The `[ids]` entry to seed (the device's `vendor:product`).
    devid: String,
    /// A config name suggested from the device name, sanitised to the apply tool's
    /// allow-list.
    suggested: String,
}

/// The directory create-config reads existing configs from and writes the new one
/// to — the *same* dir the one-click apply tool targets, so candidate detection,
/// collision checks, and the apply path can never disagree. Falls back to the
/// production `/etc/keyd` when one-click isn't available (AppImage / plain source:
/// the new config goes through draft-then-install, but collisions are still checked
/// against the real dir).
fn create_config_dir() -> PathBuf {
    applying::one_click()
        .map(|i| i.config_dir().to_path_buf())
        .unwrap_or_else(|| applying::prod_config_dir().to_path_buf())
}

/// Connected keyboards eligible for a fresh config, deduped by `vendor:product`
/// (one keyboard exposes several event nodes). See [`CreateCandidate`].
fn create_candidates(config_dir: &Path) -> Vec<CreateCandidate> {
    let configs = parse_configs(&conf_files_in(config_dir));
    let matchers: Vec<Ids> = configs.iter().map(|(_, c)| Ids::parse(&c.ids)).collect();
    let mut out: Vec<CreateCandidate> = Vec::new();
    for dev in devices::connected_devices() {
        if !dev.is_keyboard {
            continue;
        }
        let devid = dev.devid();
        if out.iter().any(|c| c.devid == devid) {
            continue;
        }
        let best = matchers
            .iter()
            .map(|ids| ids.match_device(&devid, dev.flags))
            .max_by_key(|m| m.rank())
            .unwrap_or(MatchKind::None);
        if best == MatchKind::Explicit {
            continue; // already governed — edit that config instead
        }
        let label =
            if dev.name.is_empty() { devid.clone() } else { format!("{} ({devid})", dev.name) };
        out.push(CreateCandidate {
            name: dev.name.clone(),
            label,
            suggested: sanitize_config_name(&dev.name),
            devid,
        });
    }
    out
}

/// Turn a free-form device name into a config name the apply tool's allow-list
/// accepts ([`keydviz_apply::valid_name`]): lowercased, every run of
/// non-`[a-z0-9_]` collapsed to a single `-`, leading/trailing `-` trimmed, capped
/// at 64. Falls back to `keyboard` when nothing usable survives.
fn sanitize_config_name(name: &str) -> String {
    let mut s = String::new();
    let mut prev_dash = false;
    for c in name.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_alphanumeric() || lc == '_' {
            s.push(lc);
            prev_dash = false;
        } else if !s.is_empty() && !prev_dash {
            s.push('-');
            prev_dash = true;
        }
    }
    let capped: String = s.trim_matches('-').chars().take(64).collect();
    let capped = capped.trim_end_matches('-');
    if capped.is_empty() {
        "keyboard".to_string()
    } else {
        capped.to_string()
    }
}

/// Whether `<config_dir>/<name>.conf` already exists — a *filename* collision,
/// distinct from an `[ids]` collision: creating over it would overwrite an unrelated
/// config, so the UI blocks it and asks for a different name.
fn config_name_taken(config_dir: &Path, name: &str) -> bool {
    config_dir.join(format!("{name}.conf")).exists()
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

/// One-click apply bookkeeping (E2), UI-thread only. The protocol thread can only
/// ferry plain [`applying::ApplyEvent`]s across `invoke_from_event_loop`, so the
/// state the event handler needs — the session to re-base on `kept`, the sheet
/// sources to republish, the live handle for keep/revert, the countdown timer —
/// lives in a thread-local, same shape as [`TRAY`]. Seeded once in `main`.
struct ApplyCtx {
    session: SharedSession,
    srcs: Rc<RefCell<Vec<SheetSrc>>>,
    run: RefCell<Option<applying::ApplyHandle>>,
    timer: RefCell<Option<slint::Timer>>,
    /// The always-on-top countdown dialog (test field + timer + KEEP/revert),
    /// alive only while a run is in `countdown`. Held so `finish`/`teardown_apply`
    /// can close it and the timer can drive its seconds.
    dialog: RefCell<Option<ApplyDialog>>,
}

thread_local! {
    static APPLY: RefCell<Option<ApplyCtx>> = const { RefCell::new(None) };
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

    // Key-picker vocabulary (E2): probe the installed keyd once for its real key set;
    // fall back to core's built-in list when keyd can't tell us (not installed, dev,
    // AppImage). It never changes per session, so cache it Rust-side and only ever push
    // the ranked slice into the UI (see `picker::rank_keys`).
    let probe = probe::KeydProbe::run();
    let picker_keys: Rc<Vec<slint::SharedString>> = Rc::new(if probe.keys.is_empty() {
        keydviz_core::board::primary_keysyms().iter().map(|s| (*s).into()).collect()
    } else {
        probe.keys.iter().map(slint::SharedString::from).collect()
    });
    win.set_picker_source(
        if probe.keys.is_empty() {
            "built-in list".to_string()
        } else {
            format!("keyd list-keys ({})", probe.keys.len())
        }
        .into(),
    );

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
    // One-click apply (E2): the event handler runs out of `invoke_from_event_loop`
    // closures that can't capture these Rcs across the thread hop — park them.
    APPLY.with(|a| {
        *a.borrow_mut() = Some(ApplyCtx {
            session: session.clone(),
            srcs: srcs.clone(),
            run: RefCell::new(None),
            timer: RefCell::new(None),
            dialog: RefCell::new(None),
        });
    });
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
                if refuse_if_applying(&win) {
                    return;
                }
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
                if refuse_if_applying(&win) {
                    return;
                }
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
                Ok(s) => enter_edit_session(&win, &session, s, banner),
                Err(v) => win.set_edit_banner(v.describe().into()),
            }
        });
    }

    // Open the create-config panel: list connected keyboards no specific config
    // governs, plus the wildcard, and default the selection + suggested name (§5.5).
    {
        let weak = win.as_weak();
        win.on_begin_create(move || {
            let Some(win) = weak.upgrade() else { return };
            let cands = create_candidates(&create_config_dir());
            let (sel_id, sel_name) = match cands.first() {
                Some(c) => (c.devid.clone(), c.suggested.clone()),
                None => ("*".to_string(), "default".to_string()),
            };
            let data: Vec<CreateCandidateData> = cands
                .iter()
                .map(|c| CreateCandidateData {
                    label: c.label.clone().into(),
                    id: c.devid.clone().into(),
                    suggested: c.suggested.clone().into(),
                })
                .collect();
            win.set_create_candidates(model(data));
            win.set_create_selected_id(sel_id.into());
            win.set_create_name(sel_name.into());
            win.set_create_error("".into());
            win.set_create_open(true);
        });
    }

    // Create the starter config for the chosen id+name and drop straight into edit
    // mode on it. The new config becomes a real board (so the preview/apply/exit
    // machinery — all keyed by path — works unchanged); a never-persisted one is
    // removed again on exit (see `exit_edit`).
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_create_config(move |id, name| {
            let Some(win) = weak.upgrade() else { return };
            let id = id.to_string();
            let name = name.trim().to_string();
            if id.is_empty() {
                win.set_create_error("pick a keyboard or \u{201c}All keyboards\u{201d}".into());
                return;
            }
            if !keydviz_apply::valid_name(&name) {
                win.set_create_error(
                    "name: letters, digits, '_' or '-' only (max 64)".into(),
                );
                return;
            }
            let dir = create_config_dir();
            if config_name_taken(&dir, &name) {
                win.set_create_error(
                    format!("{name}.conf already exists \u{2014} choose another name").into(),
                );
                return;
            }
            let path = dir.join(format!("{name}.conf"));
            let s = match editing::EditSession::create(&path, &[&id]) {
                Ok(s) => s,
                // The starter round-trips by construction; surface anything unexpected
                // in the panel rather than silently failing.
                Err(v) => {
                    win.set_create_error(v.describe().into());
                    return;
                }
            };
            // Look up the device name (for the board label) and build the banner.
            let device =
                create_candidates(&dir).into_iter().find(|c| c.devid == id).map(|c| c.name);
            let banner = if id == "*" {
                "new config \u{2014} applies to all keyboards not claimed by another config"
                    .to_string()
            } else {
                match &device {
                    Some(n) if !n.is_empty() => format!("new config \u{2014} applies to {n} ({id})"),
                    _ => format!("new config \u{2014} applies to {id}"),
                }
            };
            let matched_ids =
                if id == "*" { vec!["*".to_string()] } else { vec![id.clone()] };
            let device_label = device.unwrap_or_default();
            // Add the new config as a board and select it.
            let new_idx = {
                let mut srcs = srcs.borrow_mut();
                srcs.push(SheetSrc::catalog(&path, &s.config(), &device_label, matched_ids));
                let data: Vec<SheetData> = srcs.iter().map(build_sheet_data).collect();
                win.set_sheets(model(data));
                srcs.len() - 1
            };
            win.set_active_index(new_idx as i32);
            {
                use slint::Model;
                if let Some(row) = win.get_sheets().row_data(new_idx) {
                    win.set_active_sheet(row);
                }
            }
            win.set_create_open(false);
            win.set_create_error("".into());
            enter_edit_session(&win, &session, s, banner);
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
            seed_tap_hold(&win, s, &layer, &phys);
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
            // Changing the focused section dismisses any pending delete confirm or open
            // rename field — both named the previously-selected layer, which the user
            // just moved off of.
            win.set_delete_prompt("".into());
            win.set_delete_detail("".into());
            win.set_rename_target("".into());
            win.set_rename_name("".into());
            win.set_can_rename(renameable(&name));
            win.set_edit_layer(name.clone());
            let phys = win.get_selected_phys().to_string();
            if !phys.is_empty() {
                if let Some(s) = session.borrow().as_ref() {
                    let cur = s.current_binding(&name, &phys).unwrap_or_default();
                    seed_tap_hold(&win, s, &name, &phys);
                    win.set_edit_current(cur.clone().into());
                    win.set_edit_value(cur.into());
                }
            }
            render_board(&win);
        });
    }

    // Create a new empty layer and select it: the section chooser, the tap/hold
    // "when held" targets, and the orphan-warning panel all refresh, so binding a
    // key into it (or pointing a layer() at it) works immediately.
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_create_layer(move |name| {
            let Some(win) = weak.upgrade() else { return };
            if refuse_if_applying(&win) {
                return;
            }
            let mut sb = session.borrow_mut();
            let Some(s) = sb.as_mut() else { return };
            match s.add_layer(&name) {
                Ok(created) => {
                    let (cfg, dirty, path) = (s.config(), s.dirty(), s.path.clone());
                    let layers = edit_layer_choices(s);
                    let holds = hold_layer_choices(s);
                    refresh_warnings(&win, s); // defining a layer can clear an orphan
                    drop(sb);
                    win.set_edit_layers(model(layers));
                    win.set_hold_layers(model(holds));
                    win.set_can_rename(renameable(&created));
                    win.set_edit_layer(created.into());
                    win.set_selected_phys("".into());
                    win.set_edit_current("".into());
                    win.set_edit_value("".into());
                    win.set_new_layer_open(false);
                    win.set_new_layer_name("".into());
                    win.set_edit_dirty(dirty);
                    win.set_capture_armed(false);
                    refresh_preview(&win, &srcs, &path, cfg); // shows the new empty board
                }
                Err(e) => {
                    drop(sb);
                    win.set_edit_banner(format!("\u{26a0} {e}").into());
                }
            }
        });
    }

    // Request a layer delete → raise the confirm bar, naming any bindings that would
    // be left dangling so the choice is informed (the actual delete is confirm-gated).
    {
        let weak = win.as_weak();
        let session = session.clone();
        win.on_delete_layer(move |name| {
            let Some(win) = weak.upgrade() else { return };
            if refuse_if_applying(&win) {
                return;
            }
            let sb = session.borrow();
            let Some(s) = sb.as_ref() else { return };
            let refs = s.references_to(&name);
            let detail = if refs.is_empty() {
                format!("\u{26a0} Delete [{name}]? The layer and its bindings are removed.")
            } else {
                let shown = refs.len().min(3);
                let more = refs.len() - shown;
                let tail =
                    if more > 0 { format!(" (+{more} more)") } else { String::new() };
                format!(
                    "\u{26a0} Delete [{name}]? {} binding(s) still point here ({}{tail}) \
                     \u{2014} they'll dangle until you fix them.",
                    refs.len(),
                    refs[..shown].join(", "),
                )
            };
            drop(sb);
            win.set_delete_detail(detail.into());
            win.set_delete_prompt(name);
        });
    }

    // Confirm the pending layer delete: drop the section(s), reselect a surviving
    // layer, and refresh the chooser / hold-targets / warnings / preview.
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_confirm_delete_layer(move || {
            let Some(win) = weak.upgrade() else { return };
            if refuse_if_applying(&win) {
                return;
            }
            let name = win.get_delete_prompt().to_string();
            if name.is_empty() {
                return;
            }
            let mut sb = session.borrow_mut();
            let Some(s) = sb.as_mut() else { return };
            match s.remove_layer(&name) {
                Ok(()) => {
                    let (cfg, dirty, path) = (s.config(), s.dirty(), s.path.clone());
                    let layers = edit_layer_choices(s);
                    let holds = hold_layer_choices(s);
                    // Reselect the first surviving section (none → "" base board).
                    let next =
                        layers.first().map(|c| c.name.to_string()).unwrap_or_default();
                    refresh_warnings(&win, s); // a now-dangling ref becomes an orphan
                    drop(sb);
                    win.set_edit_layers(model(layers));
                    win.set_hold_layers(model(holds));
                    win.set_can_rename(renameable(&next));
                    win.set_edit_layer(next.into());
                    win.set_selected_phys("".into());
                    win.set_edit_current("".into());
                    win.set_edit_value("".into());
                    win.set_edit_dirty(dirty);
                    win.set_capture_armed(false);
                    win.set_delete_prompt("".into());
                    win.set_delete_detail("".into());
                    refresh_preview(&win, &srcs, &path, cfg);
                }
                Err(e) => {
                    drop(sb);
                    win.set_delete_prompt("".into());
                    win.set_delete_detail("".into());
                    win.set_edit_banner(format!("\u{26a0} {e}").into());
                }
            }
        });
    }

    // Dismiss the delete confirm without deleting.
    {
        let weak = win.as_weak();
        win.on_cancel_delete_layer(move || {
            let Some(win) = weak.upgrade() else { return };
            win.set_delete_prompt("".into());
            win.set_delete_detail("".into());
        });
    }

    // Rename the selected layer: rewrite its section header(s) and every binding that
    // points at it, so nothing orphans. The chooser / hold-targets / warnings / preview
    // all refresh, and the renamed layer stays selected under its new name.
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_rename_layer(move |old_base, new_name| {
            let Some(win) = weak.upgrade() else { return };
            if refuse_if_applying(&win) {
                return;
            }
            let mut sb = session.borrow_mut();
            let Some(s) = sb.as_mut() else { return };
            match s.rename_layer(&old_base, &new_name) {
                Ok(renamed) => {
                    let (cfg, dirty, path) = (s.config(), s.dirty(), s.path.clone());
                    let layers = edit_layer_choices(s);
                    let holds = hold_layer_choices(s);
                    refresh_warnings(&win, s); // following refs can clear an orphan
                    drop(sb);
                    win.set_edit_layers(model(layers));
                    win.set_hold_layers(model(holds));
                    win.set_edit_layer(renamed.clone().into());
                    win.set_can_rename(renameable(&renamed));
                    // The selection's section changed name; reset the picked key.
                    win.set_selected_phys("".into());
                    win.set_edit_current("".into());
                    win.set_edit_value("".into());
                    win.set_edit_dirty(dirty);
                    win.set_capture_armed(false);
                    win.set_rename_target("".into());
                    win.set_rename_name("".into());
                    refresh_preview(&win, &srcs, &path, cfg); // legend shows the new name
                }
                Err(e) => {
                    // Keep the field open so the user can correct the name.
                    drop(sb);
                    win.set_edit_banner(format!("\u{26a0} {e}").into());
                }
            }
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
            if refuse_if_applying(&win) {
                return;
            }
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
                    seed_tap_hold(&win, s, &layer, &phys); // keep the tap/hold panel in sync
                    refresh_warnings(&win, s); // a binding change can add/clear an orphan
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

    // Make the selection transparent (pass-through): clear its binding so the key
    // falls through to the base layer — keyd's default for any unbound key. Distinct
    // from "noop" (which disables the key); mirrors VIA's "▽".
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_make_transparent(move || {
            let Some(win) = weak.upgrade() else { return };
            if refuse_if_applying(&win) {
                return;
            }
            let layer = win.get_edit_layer().to_string();
            let phys = win.get_selected_phys().to_string();
            if phys.is_empty() {
                return;
            }
            let mut sb = session.borrow_mut();
            let Some(s) = sb.as_mut() else { return };
            match s.clear_binding(&layer, &phys) {
                Ok(()) => {
                    let (cfg, dirty, path) = (s.config(), s.dirty(), s.path.clone());
                    seed_tap_hold(&win, s, &layer, &phys); // keep the tap/hold panel in sync
                    refresh_warnings(&win, s); // a binding change can add/clear an orphan
                    drop(sb);
                    win.set_edit_current("".into());
                    win.set_edit_value("".into());
                    win.set_edit_dirty(dirty);
                    win.set_capture_armed(false);
                    refresh_preview(&win, &srcs, &path, cfg);
                }
                Err(e) => win.set_edit_banner(format!("\u{26a0} {e}").into()),
            }
        });
    }

    // Make the selection a dual-function (tap/hold) key — VIA's Mod-Tap / Layer-Tap.
    // Hold target + tap come from the th_* slots; the composer (TapHold) preserves an
    // existing key's function + timeouts and emits canonical overload(...) for new ones.
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_apply_tap_hold(move || {
            let Some(win) = weak.upgrade() else { return };
            if refuse_if_applying(&win) {
                return;
            }
            let layer = win.get_edit_layer().to_string();
            let phys = win.get_selected_phys().to_string();
            let target = win.get_th_hold().trim().to_string();
            if phys.is_empty() || target.is_empty() {
                return;
            }
            // Momentary → no tap; otherwise the tap field (defaulting to the key
            // itself when left blank, matching keyd's overload(layer) short form).
            let tap = if win.get_th_hold_only() {
                None
            } else {
                let t = win.get_th_tap().trim().to_string();
                Some(if t.is_empty() { phys.clone() } else { t })
            };
            let feel = feel_from_str(&win.get_th_feel());
            let mut sb = session.borrow_mut();
            let Some(s) = sb.as_mut() else { return };
            match s.set_tap_hold(&layer, &phys, &target, tap, feel) {
                Ok(()) => {
                    let cur = s.current_binding(&layer, &phys).unwrap_or_default();
                    let (cfg, dirty, path) = (s.config(), s.dirty(), s.path.clone());
                    seed_tap_hold(&win, s, &layer, &phys);
                    refresh_warnings(&win, s); // a tap/hold can target a missing layer
                    drop(sb);
                    win.set_edit_current(cur.clone().into());
                    win.set_edit_value(cur.into());
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

    // Searchable key picker (E2): browse keyd's full key vocabulary into the binding or
    // tap field. Fill-only — picking sets the field, the user still applies. Ranking and
    // the result cap run in `picker::rank_keys`; the full vocab stays in `picker_keys`,
    // only the ranked slice is ever pushed to the UI.
    {
        let weak = win.as_weak();
        let picker_keys = picker_keys.clone();
        win.on_open_picker(move |target| {
            let Some(win) = weak.upgrade() else { return };
            win.set_picker_target(target);
            win.set_picker_query("".into());
            win.set_capture_armed(false); // don't let an armed capture fire into a pick
            let (results, truncated) = picker::rank_keys(&picker_keys, "", picker::RESULT_CAP);
            win.set_picker_results(model(results));
            win.set_picker_truncated(truncated as i32);
            win.set_picker_open(true);
        });
    }
    {
        let weak = win.as_weak();
        let picker_keys = picker_keys.clone();
        win.on_filter_keys(move |query| {
            let Some(win) = weak.upgrade() else { return };
            let (results, truncated) = picker::rank_keys(&picker_keys, query.as_str(), picker::RESULT_CAP);
            win.set_picker_results(model(results));
            win.set_picker_truncated(truncated as i32);
        });
    }
    {
        let weak = win.as_weak();
        win.on_pick_key(move |name| {
            let Some(win) = weak.upgrade() else { return };
            // Route to the field the picker was opened for; never auto-apply.
            match win.get_picker_target().as_str() {
                "tap" => win.set_th_tap(name),
                _ => win.set_edit_value(name),
            }
            win.set_picker_open(false);
            win.set_picker_query("".into());
        });
    }

    // Save the draft and surface the verdict + diff + copy-paste install steps.
    {
        let weak = win.as_weak();
        let session = session.clone();
        win.on_save_draft(move || {
            let Some(win) = weak.upgrade() else { return };
            if refuse_if_applying(&win) {
                return;
            }
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

    // ---- one-click apply (Phase 6 E2: pkexec + dead-man's switch) ---------------

    // Pre-flight: every free check runs before pkexec is ever spawned — size, the
    // safety scan (command()/macro() route to an explicit confirm state), `keyd
    // check`, staleness, and the diff the user will watch through the countdown.
    {
        let weak = win.as_weak();
        let session = session.clone();
        win.on_start_apply(move || {
            let Some(win) = weak.upgrade() else { return };
            if win.get_apply_state() != "idle" || !win.get_edit_dirty() {
                return;
            }
            let sb = session.borrow();
            let Some(s) = sb.as_ref() else { return };
            let bytes = s.serialized();
            if bytes.len() > keydviz_apply::MAX_CONFIG_BYTES {
                win.set_apply_state("failed".into());
                win.set_apply_info(
                    format!(
                        "config is {} bytes — keyd's own limit is {}",
                        bytes.len(),
                        keydviz_apply::MAX_CONFIG_BYTES
                    )
                    .into(),
                );
                return;
            }
            // The tool would refuse a broken config anyway — but only after the
            // user paid the auth prompt. Catch it here for free. `None` (no keyd
            // in PATH) falls through: the tool is the authoritative gate.
            if let Some(Err(e)) = editing::keyd_check_bytes(&bytes) {
                win.set_apply_state("failed".into());
                win.set_apply_info(
                    format!("keyd check rejects this config — fix it first:\n{e}").into(),
                );
                return;
            }
            // Same scan code the privileged tool runs (§5.3) — the pre-flight and
            // the enforcement can't disagree on what needs confirmation.
            let findings = keydviz_apply::scan::scan(bytes.as_bytes());
            let mut info = String::new();
            for f in &findings {
                info.push_str(&format!("\u{26a0} {}\n", f.describe()));
            }
            if let Some(w) = s.stale_warning() {
                info.push_str(&format!("\u{26a0} {w}\n"));
            }
            let diff = s.diff();
            drop(sb);
            if !diff.is_empty() {
                if !info.is_empty() {
                    info.push('\n');
                }
                info.push_str(diff.trim_end());
            }
            win.set_apply_info(info.into());
            if findings.iter().any(|f| f.needs_ack()) {
                win.set_apply_state("confirm".into());
            } else {
                launch_apply(&win, false);
            }
        });
    }

    // The explicit command()/macro() acknowledgement — only this click sets the
    // protocol's `sensitive-ok` token (§5.3).
    {
        let weak = win.as_weak();
        win.on_confirm_apply(move || {
            let Some(win) = weak.upgrade() else { return };
            if win.get_apply_state() == "confirm" {
                launch_apply(&win, true);
            }
        });
    }

    // Universal back-out/dismiss. In `auth` we only drop our stdin and *wait*: the
    // run isn't over until the tool (or pkexec's exit code) says how it ended —
    // never report an outcome the privileged side hasn't confirmed.
    {
        let weak = win.as_weak();
        win.on_cancel_apply(move || {
            let Some(win) = weak.upgrade() else { return };
            match win.get_apply_state().as_str() {
                "confirm" | "kept" | "reverted" | "revert-failed" | "failed" => {
                    win.set_apply_state("idle".into());
                    win.set_apply_info("".into());
                }
                "auth" => {
                    with_apply_run(|h| h.revert());
                    win.set_apply_info("cancelling \u{2014} waiting for the tool to exit".into());
                }
                _ => {}
            }
        });
    }
    // The dead-man's switch GUI half (KEEP / revert during the countdown) lives on
    // the ApplyDialog, wired in open_apply_dialog — see handle_apply_event.

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
        //
        // No apply teardown here: during the countdown the control surface is the
        // separate always-on-top ApplyDialog, which stays up when the main window
        // hides — so the user keeps KEEP/revert/test, and the dialog (or the tool's
        // timeout) still governs the dead-man's switch. Closing *the dialog* reverts.
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
/// Whether a layer base name may be renamed: a real named layer, not keyd's implicit
/// `main` base and not a composite (`a+b`, defined by its parts). Mirrors core's
/// [`keydviz_core::edit::EditConfig::rename_layer`] guard, so the UI only offers the
/// rename chip where the action can succeed.
fn renameable(base: &str) -> bool {
    !base.is_empty() && base != "main" && !base.contains('+')
}

fn edit_layer_choices(s: &editing::EditSession) -> Vec<EditLayer> {
    s.editable_sections()
        .into_iter()
        .map(|n| EditLayer { name: n.clone().into(), display: n.into() })
        .collect()
}

/// Layers offered as tap/hold "when held" targets. Excludes:
/// - the base (`main`) — you hold *into* a layer, never onto the base board;
/// - any layer whose name is a modifier (`[shift]` etc.) — `overload(shift, …)`
///   always means the *modifier*, so such a layer can't be addressed by name and
///   would otherwise duplicate the fixed modifier chip;
/// - composite layers (`a+b`) — keyd auto-activates those from their parts; they
///   are not valid `overload`/`layer` targets.
///
/// The 5 modifiers are offered separately (fixed chips in the UI).
fn hold_layer_choices(s: &editing::EditSession) -> Vec<slint::SharedString> {
    s.editable_sections()
        .into_iter()
        .filter(|n| n != "main" && !n.contains('+') && !MODIFIERS.contains(&n.as_str()))
        .map(Into::into)
        .collect()
}

/// Pre-fill the tap/hold slots for the selected key: decompose its current binding
/// into hold-target + tap when it is a tap/hold, otherwise default the tap to the
/// physical key (a sensible start for a new dual-function key) and leave the hold
/// target unset until the user picks one.
/// Push the session's orphan-layer warnings to the panel. Called on open and after
/// every mutating edit, so creating a missing layer (or dropping the reference) clears
/// the warning live.
fn refresh_warnings(win: &MainWindow, s: &editing::EditSession) {
    win.set_edit_warnings(s.orphan_warnings().join("\n").into());
}

fn seed_tap_hold(win: &MainWindow, s: &editing::EditSession, layer: &str, phys: &str) {
    match s.current_tap_hold(layer, phys) {
        Some(th) => {
            win.set_selected_is_tap_hold(true);
            // Light the matching feel chip; leave BOTH unlit ("") for a tap/hold
            // whose form we don't name (plain overload/overloadt) so editing it
            // preserves the form rather than silently converting it.
            win.set_th_feel(feel_str(th.behavior()).into());
            win.set_th_hold(th.target.into());
            win.set_th_hold_only(th.tap.is_none());
            win.set_th_tap(th.tap.unwrap_or_default().into());
        }
        None => {
            // Not a tap/hold yet. Default the tap to the key's current simple remap
            // (so making `capslock = esc` dual-function keeps esc as the tap), or to
            // the physical key when unbound / bound to something non-trivial.
            let default_tap = match s.current_binding(layer, phys) {
                Some(v) if !v.is_empty() && !v.contains('(') && v != "noop" => v,
                _ => phys.to_string(),
            };
            win.set_selected_is_tap_hold(false);
            // A fresh dual-function key defaults to the eager feel.
            win.set_th_feel("fast".into());
            win.set_th_hold("".into());
            win.set_th_hold_only(false);
            win.set_th_tap(default_tap.into());
        }
    }
}

/// The UI "feel" token for an existing binding's behavior: `""` (no chip lit) for
/// a form outside the two-behavior model, so editing it preserves rather than
/// converts. Kept in sync with [`feel_from_str`].
fn feel_str(b: Option<Behavior>) -> &'static str {
    match b {
        Some(Behavior::Responsive) => "fast",
        Some(Behavior::TypingSafe) => "safe",
        None => "", // unnamed form (plain overload/overloadt) → no feel chosen
    }
}

/// Map the UI "feel" token to a [`Behavior`]. `""` → `None` ("no feel chosen":
/// preserve an existing unnamed form); otherwise a concrete feel.
fn feel_from_str(s: &str) -> Option<Behavior> {
    match s {
        "fast" => Some(Behavior::Responsive),
        "safe" => Some(Behavior::TypingSafe),
        _ => None,
    }
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

/// Enter edit mode for `s` (freshly opened *or* just created): seed every edit-mode
/// window property, store the session, and freeze the board to its default section.
/// Shared by the edit toggle and the create-config flow so the two can't drift. The
/// active sheet must already point at the config being edited (its board is what the
/// preview renders onto). `edit_dirty` follows `s.dirty()` — `false` for a clean
/// just-opened file, `true` for a brand-new config that has content to persist.
fn enter_edit_session(
    win: &MainWindow,
    session: &SharedSession,
    s: editing::EditSession,
    banner: String,
) {
    let choices = edit_layer_choices(&s);
    // Default to the first real section (usually `main`, but a config whose bindings
    // live only in layers has none — pick what exists).
    let default_layer = choices.first().map(|c| c.name.clone()).unwrap_or("main".into());
    let (apply_ok, apply_hint) = apply_gate(&s);
    win.set_edit_layers(model(choices));
    win.set_hold_layers(model(hold_layer_choices(&s)));
    win.set_can_rename(renameable(&default_layer));
    win.set_edit_layer(default_layer);
    win.set_rename_target("".into());
    win.set_rename_name("".into());
    win.set_selected_phys("".into());
    win.set_edit_current("".into());
    win.set_edit_value("".into());
    win.set_edit_dirty(s.dirty());
    win.set_draft_info("".into());
    win.set_capture_armed(false);
    win.set_new_layer_open(false);
    win.set_new_layer_name("".into());
    win.set_delete_prompt("".into());
    win.set_delete_detail("".into());
    win.set_edit_banner(banner.into());
    win.set_apply_available(apply_ok);
    win.set_apply_hint(apply_hint.into());
    refresh_warnings(win, &s);
    reset_apply_ui(win);
    *session.borrow_mut() = Some(s);
    win.set_edit_mode(true);
    render_board(win); // freeze the board to the chosen section
}

/// Leave edit mode: drop the session, clear the edit chrome, and discard the unsaved
/// preview by re-deriving the sheet from the file on disk.
fn exit_edit(win: &MainWindow, srcs: &Rc<RefCell<Vec<SheetSrc>>>, session: &SharedSession) {
    let Some(s) = session.borrow_mut().take() else { return };
    // Unconditionally tear down any in-flight apply, so leaving edit mode is safe
    // even on a path that didn't pre-check (the discard-guard's confirm reaches
    // here mid-flight): drop our stdin → the tool sees EOF and reverts, and stop
    // the cosmetic countdown timer. Reverting a live change the user is walking
    // away from is the correct outcome (nothing persists without KEEP).
    teardown_apply();
    win.set_edit_mode(false);
    win.set_capture_armed(false);
    win.set_selected_phys("".into());
    win.set_edit_banner("".into());
    win.set_draft_info("".into());
    win.set_edit_dirty(false);
    win.set_discard_prompt(false);
    win.set_pending_kbd(-1);
    win.set_new_layer_open(false);
    win.set_new_layer_name("".into());
    win.set_delete_prompt("".into());
    win.set_delete_detail("".into());
    win.set_rename_target("".into());
    win.set_rename_name("".into());
    win.set_can_rename(false);
    win.set_apply_available(false);
    win.set_apply_hint("".into());
    reset_apply_ui(win);
    let mut srcs = srcs.borrow_mut();
    if let Some(idx) = srcs.iter().position(|x| x.path == s.path) {
        if s.is_new() && !s.path.exists() {
            // A new config that was never persisted (cancelled, or applied-then-reverted):
            // remove its phantom board entirely rather than re-deriving from a file that
            // doesn't exist. Rebuild the chooser and reselect a surviving board.
            srcs.remove(idx);
            let data: Vec<SheetData> = srcs.iter().map(build_sheet_data).collect();
            win.set_sheets(model(data));
            if !srcs.is_empty() {
                let active = win.get_active_index().max(0).min(srcs.len() as i32 - 1);
                win.set_active_index(active);
                use slint::Model;
                if let Some(row) = win.get_sheets().row_data(active as usize) {
                    win.set_active_sheet(row);
                }
            }
        } else {
            if let Ok(cfg) = parse_file(&srcs[idx].path) {
                srcs[idx].cfg = cfg;
            }
            publish_sheet(win, idx, &srcs[idx]);
        }
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

/// True while an apply run is in flight (confirm / auth / countdown).
fn apply_busy(win: &MainWindow) -> bool {
    matches!(win.get_apply_state().as_str(), "confirm" | "auth" | "countdown")
}

/// Run `f` against the live apply handle if there is one — the single place that
/// knows how the handle is parked (KEEP / revert / cancel all go through here, so
/// the access shape lives in one spot instead of three copies).
fn with_apply_run(f: impl FnOnce(&applying::ApplyHandle)) {
    APPLY.with(|a| {
        if let Some(ctx) = a.borrow().as_ref() {
            if let Some(h) = ctx.run.borrow().as_ref() {
                f(h);
            }
        }
    });
}

/// Reset every apply_* window property to its idle baseline. Called both when a
/// session opens and when it closes so a new apply property can't be cleared in
/// one place and forgotten in the other.
fn reset_apply_ui(win: &MainWindow) {
    win.set_apply_state("idle".into());
    win.set_apply_info("".into());
}

/// Open the always-on-top countdown dialog: a test field (auto-focused, so the
/// user can immediately type to verify the remap — keyd remaps at the device
/// level, so our own field exercises the live config), the live timer, and
/// KEEP/revert. Closing the dialog reverts, like any non-KEEP exit. Returns the
/// dialog + the timer driving its seconds; the caller parks both in `ApplyCtx`.
fn open_apply_dialog(
    win: &MainWindow,
    secs: u64,
) -> Result<(ApplyDialog, slint::Timer), slint::PlatformError> {
    let dialog = ApplyDialog::new()?;
    dialog.set_seconds_left(secs as i32);
    // The diff/findings already composed for the main panel double as the
    // "what changed" summary in the dialog.
    dialog.set_change_summary(win.get_apply_info());
    dialog.on_keep(|| with_apply_run(|h| h.keep()));
    dialog.on_revert(|| with_apply_run(|h| h.revert()));
    // Closing the dialog reverts (drop stdin → EOF). We hide rather than destroy
    // so the tool's terminal event still tears it down cleanly via `finish`.
    dialog.window().on_close_requested(move || {
        with_apply_run(|h| h.revert());
        slint::CloseRequestResponse::HideWindow
    });
    dialog.show()?;
    window_set_on_top(dialog.window(), true);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    let weak = dialog.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(200),
        move || {
            if let Some(d) = weak.upgrade() {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                d.set_seconds_left(left.as_secs() as i32);
            }
        },
    );
    Ok((dialog, timer))
}

/// Tear down a live apply run: revert it (drop stdin → the tool sees EOF and
/// restores the prior config), stop the countdown timer, and close the dialog.
/// Idempotent — a no-op when nothing is in flight, so any exit path can call it
/// unconditionally.
fn teardown_apply() {
    with_apply_run(|h| h.revert());
    APPLY.with(|a| {
        if let Some(ctx) = a.borrow().as_ref() {
            *ctx.timer.borrow_mut() = None;
            *ctx.run.borrow_mut() = None;
            if let Some(d) = ctx.dialog.borrow_mut().take() {
                let _ = d.hide();
            }
        }
    });
}

/// Refuse session-changing actions while an apply is in flight: a binding edit or
/// session swap mid-countdown has no good semantics (which bytes did the user
/// keep?), so the answer is one visible "not now" instead.
fn refuse_if_applying(win: &MainWindow) -> bool {
    if apply_busy(win) {
        win.set_edit_banner("\u{26a0} apply in progress \u{2014} KEEP or revert first".into());
        return true;
    }
    false
}

/// One-click availability for a session, computed once at open: pkexec + the
/// packaged tool must exist AND the file must be `<config-dir>/<name>.conf` with
/// an allow-listed name. The hint names the packaging trade-off only when the
/// file *would* qualify but the tool isn't installed (AppImage / plain source
/// build); a file outside the config dir gets no apply UI at all.
fn apply_gate(s: &editing::EditSession) -> (bool, String) {
    match applying::one_click() {
        Some(inv) => (s.apply_target(inv.config_dir()).is_some(), String::new()),
        None => {
            let hint = if s.apply_target(applying::prod_config_dir()).is_some() {
                "one-click apply needs the packaged keydviz-apply tool \
                 (AUR/source install) \u{2014} use save draft's install steps"
                    .to_string()
            } else {
                String::new()
            };
            (false, hint)
        }
    }
}

/// Spawn the apply tool (pkexec in release; the `--dev-dir` sibling binary in dev)
/// and ferry its protocol events onto the UI thread — the same hop shape as
/// `spawn_live`. The request bytes are written by the engine's background thread,
/// never here on the event loop (the write can block for the auth dialog's
/// lifetime).
fn launch_apply(win: &MainWindow, sensitive_ok: bool) {
    APPLY.with(|a| {
        let actx = a.borrow();
        let Some(ctx) = actx.as_ref() else { return };
        // Re-resolve instead of caching: tool/pkexec could have been (un)installed
        // since the session opened. On the None paths surface a visible failure
        // rather than a dead button — the most likely cause is the tool being
        // removed mid-session, which the user should be told about.
        let Some(how) = applying::one_click() else {
            win.set_apply_state("failed".into());
            win.set_apply_info(
                "one-click apply is no longer available (keydviz-apply or pkexec \
                 missing) \u{2014} use save draft instead"
                    .into(),
            );
            return;
        };
        let sb = ctx.session.borrow();
        let Some(s) = sb.as_ref() else { return };
        let Some(name) = s.apply_target(how.config_dir()) else {
            drop(sb);
            win.set_apply_state("failed".into());
            win.set_apply_info("this config is no longer a one-click apply target".into());
            return;
        };
        let bytes = s.serialized().into_bytes();
        drop(sb);
        // keyd reload bounces the virtual device mid-apply; don't leave a stale
        // capture armed across the hiccup.
        win.set_capture_armed(false);
        win.set_apply_state("auth".into());
        let weak = win.as_weak();
        let req = applying::ApplyRequest { name, bytes, sensitive_ok, how };
        match applying::start(req, move |ev| {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = weak.upgrade() {
                    handle_apply_event(&win, ev);
                }
            });
        }) {
            Ok(h) => *ctx.run.borrow_mut() = Some(h),
            Err(e) => {
                win.set_apply_state("failed".into());
                win.set_apply_info(format!("couldn't launch the apply tool: {e}").into());
            }
        }
    });
}

/// Apply protocol events, on the UI thread. Terminal events stop the countdown
/// and release the handle; `Kept` additionally re-bases the session on the new
/// on-disk state. The countdown timer is cosmetic — only the tool's verdict
/// lines decide the outcome, so the timer reaching 0 changes nothing by itself.
fn handle_apply_event(win: &MainWindow, ev: applying::ApplyEvent) {
    use applying::ApplyEvent as E;
    APPLY.with(|a| {
        let actx = a.borrow();
        let Some(ctx) = actx.as_ref() else { return };
        let finish = |state: &str, info: Option<String>| {
            *ctx.timer.borrow_mut() = None;
            *ctx.run.borrow_mut() = None;
            if let Some(d) = ctx.dialog.borrow_mut().take() {
                let _ = d.hide();
            }
            win.set_apply_state(state.into());
            if let Some(info) = info {
                win.set_apply_info(info.into());
            }
        };
        match ev {
            // Advisory echo — pre-flight already listed the findings.
            E::Finding(_) => {}
            E::Applied { secs } => {
                win.set_apply_state("countdown".into());
                // The countdown surface is a separate always-on-top dialog with a
                // test field, so the user can type to verify before deciding.
                match open_apply_dialog(win, secs) {
                    Ok((dialog, timer)) => {
                        *ctx.dialog.borrow_mut() = Some(dialog);
                        *ctx.timer.borrow_mut() = Some(timer);
                    }
                    Err(e) => {
                        // No control surface would mean a live change the user
                        // can't keep — revert immediately rather than strand it.
                        with_apply_run(|h| h.revert());
                        win.set_apply_info(
                            format!("couldn't open the confirm window: {e} \u{2014} reverting")
                                .into(),
                        );
                    }
                }
            }
            E::Kept => {
                let path = ctx.session.borrow().as_ref().map(|s| s.path.display().to_string());
                finish("kept", path.map(|p| format!("{p} updated")));
                reopen_after_kept(win, ctx);
            }
            E::Reverted(reason) => {
                let why = match reason.as_str() {
                    "TimedOut" => "no confirmation in time",
                    "Eof" => "cancelled",
                    other => other,
                };
                finish(
                    "reverted",
                    Some(format!(
                        "reverted: {why} \u{2014} the previous config is back; \
                         your edits are still staged"
                    )),
                );
            }
            // Verbatim: the tool's message names the backup file and the panic
            // sequence — exactly what the user needs to copy.
            E::RevertFailed(w) => finish("revert-failed", Some(w)),
            E::Refused(r) => {
                finish("failed", Some(format!("refused: {r} \u{2014} nothing was written")));
            }
            E::AuthDismissed => {
                finish(
                    "failed",
                    Some("authentication cancelled \u{2014} nothing was written".to_string()),
                );
            }
            E::NotAuthorized => {
                finish(
                    "failed",
                    Some(
                        "authorization unavailable (is a polkit agent running?) \
                         \u{2014} nothing was written"
                            .to_string(),
                    ),
                );
            }
            E::Failed(m) => finish("failed", Some(m)),
        }
    });
}

/// After `kept`, the file on disk IS the session's bytes. Re-open rather than
/// poke flags: `original` re-bases (truthful staleness from here on), the model
/// is clean at the `EditConfig` level (where `dirty()` actually looks), and the
/// §5.1 gate re-verifies that our own output round-trips. The reload watcher
/// needs no help — its session-path exemption holds (we're still editing), and
/// after `exit_edit` it sees one mtime bump and does a single redundant reload.
fn reopen_after_kept(win: &MainWindow, ctx: &ApplyCtx) {
    let Some(path) = ctx.session.borrow().as_ref().map(|s| s.path.clone()) else { return };
    match editing::EditSession::open(&path) {
        Ok(new_s) => {
            // Preserve the user's place: same section, same selected key.
            let layer = win.get_edit_layer().to_string();
            let phys = win.get_selected_phys().to_string();
            if !phys.is_empty() {
                let cur = new_s.current_binding(&layer, &phys).unwrap_or_default();
                win.set_edit_current(cur.clone().into());
                win.set_edit_value(cur.into());
            }
            *ctx.session.borrow_mut() = Some(new_s);
            win.set_edit_dirty(false);
            // The sheet still holds the preview-derived config; re-derive from
            // disk so viewer state and file agree (exit_edit's path, but staying
            // in edit mode).
            let mut srcs = ctx.srcs.borrow_mut();
            if let Some((idx, src)) = srcs.iter_mut().enumerate().find(|(_, x)| x.path == path) {
                if let Ok(cfg) = parse_file(&src.path) {
                    src.cfg = cfg;
                }
                publish_sheet(win, idx, src);
            }
            drop(srcs);
            render_board(win);
        }
        Err(v) => {
            // Shouldn't happen — we wrote serialize() output, which round-trips by
            // construction. Keep the old session rather than yank the user out of
            // edit mode, but say so.
            win.set_edit_banner(
                format!("\u{26a0} session re-open after apply failed: {}", v.describe()).into(),
            );
        }
    }
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
    window_set_on_top(win.window(), on);
}

/// The window-agnostic core of [`set_window_on_top`], so the apply dialog can be
/// kept above too (it shares the same winit-backed always-on-top mechanism).
fn window_set_on_top(window: &slint::Window, on: bool) {
    use i_slint_backend_winit::winit::window::WindowLevel;
    use i_slint_backend_winit::WinitWindowAccessor;
    let level = if on { WindowLevel::AlwaysOnTop } else { WindowLevel::Normal };
    window.with_winit_window(|w| w.set_window_level(level));
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
    // Keyed by path, not index: `srcs` grows (create-config) and shrinks (phantom
    // removal) at runtime, so an index-aligned vec would desync and panic — the map
    // tracks each config by its own path regardless of position.
    let mut mtimes: std::collections::HashMap<PathBuf, Option<std::time::SystemTime>> =
        srcs.borrow().iter().map(|s| (s.path.clone(), file_mtime(&s.path))).collect();
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
            // A freshly-added path isn't in the map yet → treat as "changed" so it gets
            // seeded (and re-derived once from disk, harmless). `None` mtime = mid-save.
            if now.is_none() || mtimes.get(&src.path).is_some_and(|&prev| prev == now) {
                continue; // missing (mid-save) or unchanged
            }
            mtimes.insert(src.path.clone(), now);
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

#[cfg(test)]
mod create_tests {
    use super::sanitize_config_name;

    #[test]
    fn sanitizes_device_names_to_the_allow_list() {
        assert_eq!(sanitize_config_name("PFU HHKB"), "pfu-hhkb");
        assert_eq!(sanitize_config_name("ASUS ROG Zephyrus G14"), "asus-rog-zephyrus-g14");
        // Leading/trailing junk trimmed; runs of symbols collapse to one dash.
        assert_eq!(sanitize_config_name("  ::Keychron K2:: "), "keychron-k2");
        assert_eq!(sanitize_config_name("My_Board-2"), "my_board-2");
        // Every result is a name the apply tool would accept.
        for n in ["PFU HHKB", "  ::Keychron K2:: ", "My_Board-2", ""] {
            let s = sanitize_config_name(n);
            assert!(keydviz_apply::valid_name(&s), "{n:?} → {s:?} should be valid");
        }
    }

    #[test]
    fn falls_back_when_nothing_usable_survives() {
        assert_eq!(sanitize_config_name(""), "keyboard");
        assert_eq!(sanitize_config_name("!!! ###"), "keyboard");
    }

    #[test]
    fn caps_at_64_and_stays_valid() {
        let long = "a".repeat(200);
        let s = sanitize_config_name(&long);
        assert_eq!(s.len(), 64);
        assert!(keydviz_apply::valid_name(&s));
    }
}
