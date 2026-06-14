//! keyd-viz — native GUI cheatsheet for keyd.
//!
//! Parses keyd config(s), builds the semantic board model in `keydviz-core`, and
//! renders it with Slint. By default it detects connected keyboards and shows only
//! the config(s) governing them; with explicit path args it shows exactly those.

mod apply_ctx;
mod applying;
mod create;
mod detect;
mod devices;
mod editing;
mod glow;
mod helper;
mod layer;
mod monitor;
mod picker;
mod prefs;
mod probe;
mod tray;
mod ui_data;

pub(crate) use apply_ctx::*;
use create::{
    config_name_taken, create_config_dir, create_scan, governed_line,
};
use detect::{gather_sheets, qmk_detection, rescan};
pub(crate) use detect::{conf_files_in, flag_value, parse_configs, WILDCARD_LABEL};
use glow::{spawn_demo, spawn_glow_decay, spawn_helper, spawn_live, spawn_monitor, stamp_glow};
pub(crate) use ui_data::{model, to_sheet_data};

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use keydviz_core::{
    catalog, parse_file, Behavior, Config, Geometry, Macro, MacroToken, Sheet, MODIFIERS,
};

slint::include_modules!();

/// Everything needed to (re)build one sheet, retained so the layout picker can morph it
/// to a different geometry without re-reading the config. `qmk` is set for boards
/// imported from QMK (whose geometry is fixed and not catalog-pickable); otherwise the
/// geometry comes from the curated catalog by `layout_id`.
pub(crate) struct SheetSrc {
    pub(crate) path: PathBuf,
    pub(crate) cfg: Config,
    pub(crate) device: String,
    /// Concrete `vendor:product` ids of connected keyboards that matched this config, so
    /// the UI can highlight which `[ids]` entry is currently plugged in.
    pub(crate) matched_ids: Vec<String>,
    pub(crate) layout_id: String,
    pub(crate) qmk: Option<(Geometry, String)>,
}

impl SheetSrc {
    /// A catalog-backed source for a parsed config, defaulting the layout to the saved
    /// choice (if any) or the name-based guess.
    pub(crate) fn catalog(path: &Path, cfg: &Config, device: &str, matched_ids: Vec<String>) -> Self {
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
pub(crate) fn build_sheet_data(src: &SheetSrc) -> SheetData {
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

/// Capture the window's current frame to a PNG (RGBA8). Used by `--render` for the
/// UX-critic screenshot harness. Returns the captured dimensions on success.
fn save_snapshot(win: &MainWindow, path: &str) -> Result<(u32, u32), String> {
    let buf = win.window().take_snapshot().map_err(|e| format!("take_snapshot: {e}"))?;
    let (w, h) = (buf.width(), buf.height());
    // The window is opaque; the software renderer's snapshot leaves alpha at 0 (it only
    // copies RGB), so emit a 3-channel RGB PNG rather than a transparent RGBA one.
    let mut bytes = Vec::with_capacity((w * h * 3) as usize);
    for p in buf.as_slice() {
        bytes.extend_from_slice(&[p.r, p.g, p.b]);
    }
    let file = std::fs::File::create(path).map_err(|e| format!("{path}: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(&bytes))
        .map_err(|e| format!("png encode: {e}"))?;
    Ok((w, h))
}

/// Set the selection's current binding *and* its plain-English description together, so
/// the editor headline can lead with "Tap F → F · Hold F → nav layer" and keep the raw
/// `lettermod(...)` as a secondary disclosure (UX-critic B1). The human form is derived
/// from `selected_phys` (the key the binding belongs to) — set that first.
pub(crate) fn set_current_binding(win: &MainWindow, cur: impl AsRef<str>) {
    let cur = cur.as_ref();
    win.set_edit_current(cur.into());
    let human = if cur.is_empty() {
        String::new()
    } else {
        keydviz_core::humanize(win.get_selected_phys().as_str(), cur)
    };
    win.set_edit_current_human(human.into());
}

/// Drive the UI into a named edit-mode state for the screenshot harness, by invoking
/// the same callbacks a real click-path runs (so panels populate authentically). Assumes
/// the active sheet is `examples/hhkb.conf` (tap-hold keys f/d/space/k; layers main/nav/
/// num/sym/game/shift; one chord leftshift+rightshift). A few terminal-confirmation
/// panels (apply summary, discard) are seeded directly — they're property-gated and a
/// faithful real apply needs `/etc/keyd` + pkexec, out of scope for a screenshot.
fn drive_render_state(win: &MainWindow, state: &str) {
    if state == "base" {
        return; // plain viewer, base board — already rendered
    }
    // Every other state is inside edit mode.
    win.invoke_toggle_edit();
    match state {
        "edit" => {} // edit mode entered, no key selected
        "backups" => {
            // Dev-dir apply makes apply_available genuinely true; refresh_backups then
            // lists the real .keydviz-bak files placed there (see the smoke-test setup).
            win.invoke_show_backups();
            win.set_backups_open(true);
        }
        "key-selected" => win.invoke_select_key("a".into()),
        "label" => {
            win.invoke_select_key("a".into());
            win.set_edit_label("My Key".into());
        }
        "tap-hold" => {
            win.invoke_select_key("f".into()); // lettermod(nav, …) → seeds the editor
            win.invoke_set_key_mode("taphold".into());
        }
        "macro" => {
            win.invoke_select_key("a".into());
            win.invoke_set_key_mode("macro".into());
            // Build a representative draft through the real step callbacks.
            win.set_macro_text_input("git push".into());
            win.invoke_macro_add_text();
            win.set_macro_delay_input("200".into());
            win.invoke_macro_add_delay();
            win.set_macro_chord_c(true);
            win.set_macro_chord_key("enter".into());
            win.invoke_macro_add_chord();
        }
        "picker" => {
            win.invoke_select_key("f".into());
            win.invoke_set_key_mode("taphold".into());
            win.invoke_open_picker("tap".into());
            win.invoke_filter_keys("".into()); // rank the full vocabulary
        }
        "chord" => {
            win.invoke_set_board_mode("chord".into());
            win.invoke_toggle_chord_key("j".into());
            win.invoke_toggle_chord_key("k".into());
        }
        "global" => win.invoke_edit_global(),
        "new-layer" => {
            win.set_new_layer_open(true);
            win.set_new_layer_name("fn".into());
        }
        "rename-layer" => {
            win.invoke_pick_edit_layer("nav".into());
            win.set_rename_target("nav".into());
            win.set_rename_name("navigation".into());
        }
        "delete-layer" => {
            win.invoke_pick_edit_layer("nav".into());
            win.invoke_delete_layer("nav".into());
        }
        "discard" => win.set_discard_prompt(true),
        "apply-summary" => {
            // Seeded: the panel is property-gated; a real apply needs /etc/keyd + pkexec.
            win.set_apply_state(ApplyState::Confirm.as_str().into());
            win.set_apply_info(
                "\u{26a0} this config can run a command when you press a key \u{2014} review \
                 before applying\n\n+ [nav]\n+ g = escape   (G \u{2192} Esc)\n- k = k\n+ k = \
                 lettermod(control, k, 150, 200)   (Tap K \u{2192} K \u{00b7} Hold K \u{2192} Ctrl)"
                    .into(),
            );
        }
        other => eprintln!("unknown --render-state '{other}'"),
    }
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

    // `--render`: force the software renderer before any Slint platform init. It renders
    // synchronously (no GL surface / shown window needed) so `take_snapshot` is reliable,
    // and this flat design (no gradients/blur) is pixel-faithful to the GPU path.
    if std::env::args().any(|a| a == "--render") && std::env::var("SLINT_BACKEND").is_err() {
        std::env::set_var("SLINT_BACKEND", "software");
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
        for w in &det.id_warnings {
            println!("{w}");
        }
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
    // Load-time `[ids]` collision warnings (§5.5) — a static property of the config
    // set, so set once and shown in the viewer (outside edit mode) until restart.
    win.set_load_warnings(det.id_warnings.join("\n").into());
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
    // Macro editor draft: the in-progress list of steps for the selected key, held in
    // Rust as the real core tokens and rebuilt into the `macro_rows` UI model on each
    // change. Committed to the config only on "set macro" (`apply_macro`).
    let macro_draft: Rc<RefCell<Vec<MacroToken>>> = Rc::new(RefCell::new(Vec::new()));
    // One-click apply (E2): the event handler runs out of `invoke_from_event_loop`
    // closures that can't capture these Rcs across the thread hop — park them.
    APPLY.with(|a| {
        *a.borrow_mut() = Some(ApplyCtx {
            session: session.clone(),
            srcs: srcs.clone(),
            run: RefCell::new(None),
            timer: RefCell::new(None),
            dialog: RefCell::new(None),
            pending: RefCell::new(Pending::Session),
            backups: RefCell::new(Vec::new()),
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
            let scan = create_scan(&create_config_dir());
            let (sel_id, sel_name) = match scan.candidates.first() {
                Some(c) => (c.devid.clone(), c.suggested.clone()),
                None => ("*".to_string(), "default".to_string()),
            };
            let data: Vec<CreateCandidateData> = scan
                .candidates
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
            win.set_create_governed(governed_line(&scan.already_configured).into());
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
                create_scan(&dir).candidates.into_iter().find(|c| c.devid == id).map(|c| c.name);
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
            let device_label =
                if id == "*" { WILDCARD_LABEL.to_string() } else { device.unwrap_or_default() };
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
        let macro_draft = macro_draft.clone();
        win.on_select_key(move |phys| {
            let Some(win) = weak.upgrade() else { return };
            // In chord mode a board click adds (or, if already picked, removes) the key
            // from the chord builder's member list — any number of keys, not a fixed pair.
            if win.get_board_mode().as_str() == "chord" {
                toggle_chord_member(&win, phys.as_str());
                return;
            }
            // Re-clicking the currently-selected key deselects it — back to the
            // nothing-picked state (the only way out other than picking another key).
            if phys == win.get_selected_phys() {
                win.set_selected_phys("".into());
                win.set_capture_armed(false);
                win.set_edit_label("".into());
                win.set_selected_has_label(false);
                return;
            }
            let sb = session.borrow();
            let Some(s) = sb.as_ref() else { return };
            let layer = win.get_edit_layer().to_string();
            let cur = s.current_binding(&layer, &phys).unwrap_or_default();
            seed_tap_hold(&win, s, &layer, &phys);
            seed_macro(&win, s, &layer, &phys, &macro_draft);
            seed_label(&win, s, &layer, &phys);
            // Default the editor mode to match the key's current binding: an existing
            // macro opens in macro mode, a tap/hold in tap/hold mode, else simple.
            let mode = if win.get_selected_is_macro() {
                "macro"
            } else if win.get_selected_is_tap_hold() {
                "taphold"
            } else {
                "simple"
            };
            win.set_key_mode(mode.into());
            // Clicking a key returns from the global options form.
            win.set_editing_global(false);
            win.set_selected_phys(phys);
            set_current_binding(&win, cur.clone());
            win.set_edit_value(cur.into());
            win.set_capture_armed(false);
        });
    }

    // Section chooser: edits land in this section, and the board freezes to its layer.
    {
        let weak = win.as_weak();
        let session = session.clone();
        let macro_draft = macro_draft.clone();
        win.on_pick_edit_layer(move |name| {
            let Some(win) = weak.upgrade() else { return };
            // Changing the focused section dismisses any pending delete confirm or open
            // rename field — both named the previously-selected layer, which the user
            // just moved off of.
            win.set_delete_prompt("".into());
            win.set_delete_detail("".into());
            win.set_rename_target("".into());
            win.set_rename_name("".into());
            win.set_editing_global(false); // picking a layer leaves the global options form
            win.set_can_rename(renameable(&name));
            win.set_edit_layer(name.clone());
            // Chords are layer-scoped: switching layers in chord mode reloads that layer's
            // chord list and clears any half-built member set (it belonged to the old layer).
            if win.get_board_mode().as_str() == "chord" {
                clear_chord_builder(&win);
                win.set_chord_action("".into());
                if let Some(rows) =
                    session.borrow().as_ref().map(|s| chord_rows_for_layer(s, name.as_str()))
                {
                    win.set_chord_rows(model(rows));
                }
            }
            let phys = win.get_selected_phys().to_string();
            if !phys.is_empty() {
                if let Some(s) = session.borrow().as_ref() {
                    let cur = s.current_binding(&name, &phys).unwrap_or_default();
                    seed_tap_hold(&win, s, &name, &phys);
                    // Reseed the macro panel/draft for the new layer too, or a stale
                    // draft from the old layer could be committed onto this key here.
                    seed_macro(&win, s, &name, &phys, &macro_draft);
                    seed_label(&win, s, &name, &phys);
                    let mode = if win.get_selected_is_macro() {
                        "macro"
                    } else if win.get_selected_is_tap_hold() {
                        "taphold"
                    } else {
                        "simple"
                    };
                    win.set_key_mode(mode.into());
                    set_current_binding(&win, cur.clone());
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
                    let chords = chord_rows_for_layer(s, &created);
                    refresh_warnings(&win, s); // defining a layer can clear an orphan
                    drop(sb);
                    win.set_edit_layers(model(layers));
                    win.set_hold_layers(model(holds));
                    // The focused layer changed: refresh the chord list and drop any
                    // half-built chord pair (it belonged to the previous layer's board).
                    win.set_chord_rows(model(chords));
                    clear_chord_builder(&win);
                    win.set_chord_action("".into());
                    win.set_can_rename(renameable(&created));
                    win.set_edit_layer(created.into());
                    win.set_selected_phys("".into());
                    win.set_key_mode("simple".into()); // no key selected: reset the editor mode
                    set_current_binding(&win, "");
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
                    let chords = chord_rows_for_layer(s, &next);
                    refresh_warnings(&win, s); // a now-dangling ref becomes an orphan
                    drop(sb);
                    win.set_edit_layers(model(layers));
                    win.set_hold_layers(model(holds));
                    // Focused layer changed: refresh chords, drop any half-built pair.
                    win.set_chord_rows(model(chords));
                    clear_chord_builder(&win);
                    win.set_chord_action("".into());
                    win.set_can_rename(renameable(&next));
                    win.set_edit_layer(next.into());
                    win.set_selected_phys("".into());
                    win.set_key_mode("simple".into()); // no key selected: reset the editor mode
                    set_current_binding(&win, "");
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
                    let chords = chord_rows_for_layer(s, &renamed);
                    refresh_warnings(&win, s); // following refs can clear an orphan
                    drop(sb);
                    win.set_edit_layers(model(layers));
                    win.set_hold_layers(model(holds));
                    // The layer's name changed: refresh its chord list under the new name
                    // and drop any half-built pair from before the rename.
                    win.set_chord_rows(model(chords));
                    clear_chord_builder(&win);
                    win.set_chord_action("".into());
                    win.set_edit_layer(renamed.clone().into());
                    win.set_can_rename(renameable(&renamed));
                    // The selection's section changed name; reset the picked key.
                    win.set_selected_phys("".into());
                    win.set_key_mode("simple".into()); // no key selected: reset the editor mode
                    set_current_binding(&win, "");
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
        let macro_draft = macro_draft.clone();
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
            let result = s.set_binding(&layer, &phys, &value);
            commit_edit(
                &win,
                &srcs,
                sb,
                result,
                |win, s| {
                    seed_tap_hold(win, s, &layer, &phys); // keep the tap/hold panel in sync
                    seed_macro(win, s, &layer, &phys, &macro_draft); // and the macro panel
                    refresh_warnings(win, s); // a binding change can add/clear an orphan
                },
                move |win| {
                    set_current_binding(win, value.clone());
                    win.set_edit_value(value.into());
                    win.set_capture_armed(false);
                },
            );
        });
    }

    // Make the selection transparent (pass-through): clear its binding so the key
    // falls through to the base layer — keyd's default for any unbound key. Distinct
    // from "noop" (which disables the key); mirrors VIA's "▽".
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        let macro_draft = macro_draft.clone();
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
            let result = s.clear_binding(&layer, &phys);
            commit_edit(
                &win,
                &srcs,
                sb,
                result,
                |win, s| {
                    seed_tap_hold(win, s, &layer, &phys); // keep the tap/hold panel in sync
                    seed_macro(win, s, &layer, &phys, &macro_draft); // and the macro panel
                    refresh_warnings(win, s); // a binding change can add/clear an orphan
                },
                |win| {
                    set_current_binding(win, "");
                    win.set_edit_value("".into());
                    win.set_capture_armed(false);
                },
            );
        });
    }

    // Set the selection's custom display label (a keyd-safe `# keyd-viz:` comment). It
    // changes how the cap reads, never the binding — so no warning/orphan recompute, just
    // re-render and refresh dirty. An empty value clears it (same as the clear button).
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_set_label(move |text| {
            let Some(win) = weak.upgrade() else { return };
            if refuse_if_applying(&win) {
                return;
            }
            let text = text.trim().to_string();
            let layer = win.get_edit_layer().to_string();
            let phys = win.get_selected_phys().to_string();
            if phys.is_empty() {
                return;
            }
            let mut sb = session.borrow_mut();
            let Some(s) = sb.as_mut() else { return };
            let result = s.set_label(&layer, &phys, &text);
            commit_edit(
                &win,
                &srcs,
                sb,
                result,
                |win, s| seed_label(win, s, &layer, &phys),
                |_win| {},
            );
        });
    }

    // Clear the selection's custom label (revert the cap to its default legend).
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_clear_label(move || {
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
            let result = s.clear_label(&layer, &phys);
            commit_edit(
                &win,
                &srcs,
                sb,
                result,
                |win, s| seed_label(win, s, &layer, &phys),
                |_win| {},
            );
        });
    }

    // Make the selection a dual-function (tap/hold) key — VIA's Mod-Tap / Layer-Tap.
    // Hold target + tap come from the th_* slots; the composer (TapHold) preserves an
    // existing key's function + timeouts and emits canonical overload(...) for new ones.
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        let macro_draft = macro_draft.clone();
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
            let result = s.set_tap_hold(&layer, &phys, &target, tap, feel);
            // The canonical written binding (read while `s` is still borrowed) seeds the
            // value slot after commit.
            let cur = if result.is_ok() {
                s.current_binding(&layer, &phys).unwrap_or_default()
            } else {
                String::new()
            };
            commit_edit(
                &win,
                &srcs,
                sb,
                result,
                |win, s| {
                    seed_tap_hold(win, s, &layer, &phys);
                    seed_macro(win, s, &layer, &phys, &macro_draft); // keep the macro panel in sync
                    refresh_warnings(win, s); // a tap/hold can target a missing layer
                },
                move |win| {
                    set_current_binding(win, cur.clone());
                    win.set_edit_value(cur.into());
                    win.set_capture_armed(false);
                },
            );
        });
    }

    // Macro editor: build an ordered step list for the selected key, then commit it as
    // a macro(...)/macro2(...) binding on "set macro". The draft tokens live in
    // `macro_draft`; every mutation rebuilds the `macro_rows` view model.
    {
        let weak = win.as_weak();
        let macro_draft = macro_draft.clone();
        win.on_macro_add_text(move || {
            let Some(win) = weak.upgrade() else { return };
            let text = win.get_macro_text_input().to_string();
            if text.is_empty() {
                return;
            }
            macro_draft.borrow_mut().push(MacroToken::Text(text));
            win.set_macro_text_input("".into());
            push_macro_rows(&win, &macro_draft);
        });
    }
    {
        let weak = win.as_weak();
        let macro_draft = macro_draft.clone();
        win.on_macro_add_delay(move || {
            let Some(win) = weak.upgrade() else { return };
            let raw = win.get_macro_delay_input().to_string();
            match raw.trim().parse::<u16>() {
                Ok(ms) if ms < 1024 => {
                    macro_draft.borrow_mut().push(MacroToken::Delay(ms));
                    win.set_macro_delay_input("".into());
                    win.set_edit_banner("".into());
                    push_macro_rows(&win, &macro_draft);
                }
                _ => win.set_edit_banner(
                    "\u{26a0} a pause must be a whole number of milliseconds under 1024".into(),
                ),
            }
        });
    }
    {
        let weak = win.as_weak();
        let macro_draft = macro_draft.clone();
        win.on_macro_add_chord(move || {
            let Some(win) = weak.upgrade() else { return };
            let key = win.get_macro_chord_key().trim().to_string();
            if key.is_empty() {
                win.set_edit_banner("\u{26a0} pick a key for the chord first".into());
                return;
            }
            if !keydviz_core::keycodes::is_keycode(&key) {
                win.set_edit_banner(
                    format!(
                        "\u{26a0} \u{201c}{key}\u{201d} isn\u{2019}t a key name \u{2014} use pick\u{2026} to choose one"
                    )
                    .into(),
                );
                return;
            }
            let mut mods = Vec::new();
            if win.get_macro_chord_c() {
                mods.push('C');
            }
            if win.get_macro_chord_m() {
                mods.push('M');
            }
            if win.get_macro_chord_a() {
                mods.push('A');
            }
            if win.get_macro_chord_s() {
                mods.push('S');
            }
            if win.get_macro_chord_g() {
                mods.push('G');
            }
            macro_draft
                .borrow_mut()
                .push(MacroToken::Chord { mods, keys: vec![key] });
            reset_macro_chord_form(&win);
            win.set_edit_banner("".into());
            push_macro_rows(&win, &macro_draft);
        });
    }
    {
        let weak = win.as_weak();
        let macro_draft = macro_draft.clone();
        win.on_macro_remove(move |idx| {
            let Some(win) = weak.upgrade() else { return };
            let i = idx as usize;
            {
                let mut d = macro_draft.borrow_mut();
                if i < d.len() {
                    d.remove(i);
                }
            }
            push_macro_rows(&win, &macro_draft);
        });
    }
    {
        let weak = win.as_weak();
        let macro_draft = macro_draft.clone();
        win.on_macro_move(move |idx, delta| {
            let Some(win) = weak.upgrade() else { return };
            let j = idx + delta;
            {
                let mut d = macro_draft.borrow_mut();
                let i = idx as usize;
                if i < d.len() && j >= 0 && (j as usize) < d.len() {
                    d.swap(i, j as usize);
                }
            }
            push_macro_rows(&win, &macro_draft);
        });
    }
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        let macro_draft = macro_draft.clone();
        win.on_apply_macro(move || {
            let Some(win) = weak.upgrade() else { return };
            if refuse_if_applying(&win) {
                return;
            }
            let layer = win.get_edit_layer().to_string();
            let phys = win.get_selected_phys().to_string();
            if phys.is_empty() {
                return;
            }
            let tokens = macro_draft.borrow().clone();
            if tokens.is_empty() {
                win.set_edit_banner("\u{26a0} add at least one step".into());
                return;
            }
            // Repeat (macro2) needs both ms values; reject a half-filled form rather
            // than guess.
            let repeat = if win.get_macro_repeat_on() {
                let to = win.get_macro_repeat_timeout().to_string();
                let rp = win.get_macro_repeat_count().to_string();
                match (to.trim().parse::<u32>(), rp.trim().parse::<u32>()) {
                    (Ok(t), Ok(r)) => Some((t, r)),
                    _ => {
                        win.set_edit_banner(
                            "\u{26a0} repeat needs two whole-millisecond numbers".into(),
                        );
                        return;
                    }
                }
            } else {
                None
            };
            let mac = Macro { tokens, repeat };
            let mut sb = session.borrow_mut();
            let Some(s) = sb.as_mut() else { return };
            let result = s.set_macro(&layer, &phys, &mac);
            let cur = if result.is_ok() {
                s.current_binding(&layer, &phys).unwrap_or_default()
            } else {
                String::new()
            };
            commit_edit(
                &win,
                &srcs,
                sb,
                result,
                |win, s| {
                    // Reseed from the stored binding so the panel shows the canonical
                    // (possibly normalized) form that was actually written.
                    seed_macro(win, s, &layer, &phys, &macro_draft);
                    refresh_warnings(win, s); // a macro can't orphan a layer, but keep parity
                },
                move |win| {
                    set_current_binding(win, cur.clone());
                    win.set_edit_value(cur.into());
                    win.set_capture_armed(false);
                    win.set_edit_banner("".into());
                },
            );
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
        let macro_draft = macro_draft.clone();
        win.on_pick_key(move |name| {
            let Some(win) = weak.upgrade() else { return };
            // Route to the field the picker was opened for; never auto-apply.
            match win.get_picker_target().as_str() {
                "tap" => win.set_th_tap(name),
                "chord_action" => win.set_chord_action(name),
                // A macro "key" step is appended straight to the draft list.
                "macro_token" => {
                    macro_draft.borrow_mut().push(MacroToken::Key(name.to_string()));
                    push_macro_rows(&win, &macro_draft);
                }
                "macro_chord_key" => win.set_macro_chord_key(name),
                _ => win.set_edit_value(name),
            }
            win.set_picker_open(false);
            win.set_picker_query("".into());
        });
    }

    // Switch a selected key's editor between simple remap and tap/hold (one binding
    // line — the modes are mutually exclusive). Pure setter; fields are already seeded.
    {
        let weak = win.as_weak();
        win.on_set_key_mode(move |mode| {
            let Some(win) = weak.upgrade() else { return };
            win.set_key_mode(mode);
        });
    }

    // ---- chords (E2): chord-building is a mode of the main board ----
    // Switch the board between "single" (edit one key) and "chord" (click two keys → one
    // action). Either switch clears the other mode's transient selection so the highlights
    // never overlap; entering chord mode refreshes the [main] chord list.
    {
        let weak = win.as_weak();
        let session = session.clone();
        win.on_set_board_mode(move |mode| {
            let Some(win) = weak.upgrade() else { return };
            win.set_selected_phys("".into());
            clear_chord_builder(&win);
            win.set_chord_action("".into());
            if mode.as_str() == "chord" {
                let layer = win.get_edit_layer().to_string();
                if let Some(rows) =
                    session.borrow().as_ref().map(|s| chord_rows_for_layer(s, &layer))
                {
                    win.set_chord_rows(model(rows));
                }
            }
            win.set_board_mode(mode);
        });
    }

    // Add or edit a chord: an existing chord for the two keys (any order) is rewritten
    // in place, else a new line is appended. The combo/toggle badge appears on both keys.
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_add_chord(move || {
            let Some(win) = weak.upgrade() else { return };
            if refuse_if_applying(&win) {
                return;
            }
            let layer = win.get_edit_layer().to_string();
            let keys: Vec<String> = {
                use slint::Model;
                win.get_chord_keys().iter().map(|s| s.to_string()).collect()
            };
            let action = win.get_chord_action().to_string();
            // The chord we're editing, if any (its key set may have changed).
            let orig = win.get_chord_edit_orig().to_string();
            let mut sb = session.borrow_mut();
            let Some(s) = sb.as_mut() else { return };
            let result = s.set_chord(&layer, &keys, &action);
            // Editing an existing chord whose key set was changed: the new key set is a
            // different line, so drop the original (set ran first, so on a validation error
            // nothing was removed). Unchanged keys → same canonical form → set_chord already
            // rewrote it in place, so skip the remove. Run while `s` is still borrowed.
            if result.is_ok()
                && !orig.is_empty()
                && keydviz_core::canonical_chord(&orig)
                    != keydviz_core::canonical_chord(&keys.join("+"))
            {
                let _ = s.remove_chord(&layer, &orig);
            }
            let rows = if result.is_ok() { chord_rows_for_layer(s, &layer) } else { Vec::new() };
            commit_edit(
                &win,
                &srcs,
                sb,
                result,
                refresh_warnings, // a chord can target a missing layer
                move |win| {
                    win.set_chord_rows(model(rows));
                    clear_chord_builder(win); // also clears chord_edit_orig
                    win.set_chord_action("".into());
                    render_board(win); // clear the now-committed members' highlight
                },
            );
        });
    }

    // Edit a chord: fill the builder from the existing row (all its members + action). The
    // user then presses "add", whose canonical match rewrites the existing line.
    // (Fill-only, like the key picker — it does not itself apply.) Any number of members
    // is preserved, so editing a 3+-key chord no longer drops keys.
    {
        let weak = win.as_weak();
        let session = session.clone();
        win.on_edit_chord(move |chord| {
            let Some(win) = weak.upgrade() else { return };
            let parts: Vec<slint::SharedString> =
                chord.split('+').map(|p| p.trim().into()).collect();
            win.set_chord_keys(model(parts));
            // Remember which chord we're editing, so committing replaces it even if its key
            // set is changed (a member added/removed) rather than appending a new chord.
            win.set_chord_edit_orig(chord.clone());
            if let Some(s) = session.borrow().as_ref() {
                let layer = win.get_edit_layer().to_string();
                let canon = keydviz_core::canonical_chord(&chord);
                if let Some((_, act)) = s
                    .chords(&layer)
                    .into_iter()
                    .find(|(k, _)| keydviz_core::canonical_chord(k) == canon)
                {
                    win.set_chord_action(act.into());
                }
            }
            render_board(&win); // light up the loaded members on the board
        });
    }

    // Toggle a clicked board key in/out of the chord builder's member list, and reset it.
    {
        let weak = win.as_weak();
        win.on_toggle_chord_key(move |phys| {
            let Some(win) = weak.upgrade() else { return };
            toggle_chord_member(&win, phys.as_str());
        });
    }
    {
        let weak = win.as_weak();
        win.on_clear_chord_keys(move || {
            let Some(win) = weak.upgrade() else { return };
            clear_chord_builder(&win);
            render_board(&win); // drop the members' highlight
        });
    }

    // Delete a chord (canonical match clears either spelling). The badge disappears.
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_remove_chord(move |chord| {
            let Some(win) = weak.upgrade() else { return };
            if refuse_if_applying(&win) {
                return;
            }
            let layer = win.get_edit_layer().to_string();
            let mut sb = session.borrow_mut();
            let Some(s) = sb.as_mut() else { return };
            let result = s.remove_chord(&layer, &chord);
            let rows = if result.is_ok() { chord_rows_for_layer(s, &layer) } else { Vec::new() };
            commit_edit(
                &win,
                &srcs,
                sb,
                result,
                refresh_warnings,
                move |win| win.set_chord_rows(model(rows)),
            );
        });
    }

    // ---- [global] daemon options (E2) ----
    // Toggle the global-options form. The `⚙ global` chip is the only way in AND out
    // (the form hides the board, so there's no key to click to escape): clicking it
    // while already open returns to the layer board. Entering deselects any key and
    // (re)populates the rows.
    {
        let weak = win.as_weak();
        let session = session.clone();
        win.on_edit_global(move || {
            let Some(win) = weak.upgrade() else { return };
            if win.get_editing_global() {
                win.set_editing_global(false); // toggle off → board returns
                return;
            }
            let Some(s) = session.borrow().as_ref().map(global_rows_for) else { return };
            win.set_selected_phys("".into());
            win.set_global_rows(model(s));
            win.set_editing_global(true);
        });
    }
    // Set / clear a global option. Empty value clears (EditSession::set_global), so the
    // toggle's "off" and a blanked field both route here. Rebuild the rows + preview.
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_set_global(move |name, value| {
            let Some(win) = weak.upgrade() else { return };
            if refuse_if_applying(&win) {
                return;
            }
            let mut sb = session.borrow_mut();
            let Some(s) = sb.as_mut() else { return };
            let result = s.set_global(&name, &value);
            let rows = if result.is_ok() { global_rows_for(s) } else { Vec::new() };
            commit_edit(
                &win,
                &srcs,
                sb,
                result,
                |_win, _s| {},
                move |win| win.set_global_rows(model(rows)),
            );
        });
    }
    {
        let weak = win.as_weak();
        let srcs = srcs.clone();
        let session = session.clone();
        win.on_clear_global(move |name| {
            let Some(win) = weak.upgrade() else { return };
            if refuse_if_applying(&win) {
                return;
            }
            let mut sb = session.borrow_mut();
            let Some(s) = sb.as_mut() else { return };
            s.clear_global(&name);
            let rows = global_rows_for(s);
            commit_edit(
                &win,
                &srcs,
                sb,
                Ok(()),
                |_win, _s| {},
                move |win| win.set_global_rows(model(rows)),
            );
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
            if ApplyState::from_token(&win.get_apply_state()) != Some(ApplyState::Idle)
                || !win.get_edit_dirty()
            {
                return;
            }
            // The normal apply button commits the *session's* edits — never a stale
            // restore override left set if a restore was armed then abandoned.
            APPLY.with(|a| {
                if let Some(c) = a.borrow().as_ref() {
                    *c.pending.borrow_mut() = Pending::Session;
                }
            });
            let sb = session.borrow();
            let Some(s) = sb.as_ref() else { return };
            let bytes = s.serialized();
            if bytes.len() > keydviz_apply::MAX_CONFIG_BYTES {
                win.set_apply_state(ApplyState::Failed.as_str().into());
                win.set_apply_info(
                    format!(
                        "this config is too large to apply \u{2014} {} bytes, and the limit \
                         is {}. remove some bindings and try again.",
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
                win.set_apply_state(ApplyState::Failed.as_str().into());
                win.set_apply_info(
                    format!("this config has an error and can't be applied \u{2014} fix it first:\n{e}")
                        .into(),
                );
                return;
            }
            // Same scan code the privileged tool runs (§5.3) — the pre-flight and
            // the enforcement can't disagree on what needs confirmation.
            let findings = keydviz_apply::scan::scan(bytes.as_bytes());
            let mut info = String::new();
            for f in &findings {
                if let Some(s) = applying::finding_summary(f) {
                    info.push_str(&format!("\u{26a0} {s}\n"));
                }
            }
            // One-level include closure scan (§5.3): a command()/macro() hiding in an
            // included file is invisible to the inline byte scan — read one level of
            // includes (relative to the config's own dir, then DATA_DIR) and require
            // the same confirmation. Advisory: the content is already root-gated.
            let config_dir = s.path.parent().unwrap_or_else(|| Path::new("."));
            let included =
                applying::scan_includes(&findings, config_dir, applying::keyd_data_dir());
            for inc in &included {
                info.push_str(&format!("\u{26a0} {}\n", inc.summary()));
            }
            if let Some(w) = s.stale_warning() {
                info.push_str(&format!("\u{26a0} {w}\n"));
            }
            let diff = s.diff_annotated();
            drop(sb);
            if !diff.is_empty() {
                if !info.is_empty() {
                    info.push('\n');
                }
                info.push_str(diff.trim_end());
            }
            win.set_apply_info(info.into());
            if findings.iter().any(|f| f.needs_ack()) || !included.is_empty() {
                win.set_apply_state(ApplyState::Confirm.as_str().into());
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
            if ApplyState::from_token(&win.get_apply_state()) == Some(ApplyState::Confirm) {
                launch_apply(&win, true);
            }
        });
    }

    // Delete this config (the confirm bar's "delete" — already a second click). A
    // never-persisted config is just a phantom board: drop it like a discard. A real
    // one routes through the privileged delete tool (auth → countdown → KEEP/revert),
    // the same recoverable path as apply (§5.5).
    {
        let weak = win.as_weak();
        let session = session.clone();
        let srcs = srcs.clone();
        win.on_delete_config(move || {
            let Some(win) = weak.upgrade() else { return };
            win.set_delete_config_prompt(false);
            if apply_busy(&win) {
                return;
            }
            let sb = session.borrow();
            let Some(s) = sb.as_ref() else { return };
            if s.is_new() {
                drop(sb);
                exit_edit(&win, &srcs, &session);
                return;
            }
            // A persisted config: it must be a `<dir>/<name>.conf` the tool can reach.
            let target = applying::one_click().and_then(|how| s.apply_target(how.config_dir()));
            let Some(name) = target else {
                drop(sb);
                win.set_apply_state(ApplyState::Failed.as_str().into());
                win.set_apply_info(
                    "this config lives outside keyd's config folder, so keyd-viz can't \
                     remove it for you \u{2014} delete the file manually"
                        .into(),
                );
                return;
            };
            let path = s.path.clone();
            drop(sb);
            launch_delete(&win, name, path);
        });
    }

    // Universal back-out/dismiss. In `auth` we only drop our stdin and *wait*: the
    // run isn't over until the tool (or pkexec's exit code) says how it ended —
    // never report an outcome the privileged side hasn't confirmed.
    {
        let weak = win.as_weak();
        win.on_cancel_apply(move || {
            let Some(win) = weak.upgrade() else { return };
            match ApplyState::from_token(&win.get_apply_state()) {
                Some(
                    ApplyState::Confirm
                    | ApplyState::Kept
                    | ApplyState::Reverted
                    | ApplyState::RevertFailed
                    | ApplyState::Failed,
                ) => {
                    win.set_apply_state(ApplyState::Idle.as_str().into());
                    win.set_apply_info("".into());
                    // Backing out of a pending (sensitive) restore disarms the override,
                    // so it can't ride a later apply. Harmless when none was armed.
                    APPLY.with(|a| {
                        if let Some(c) = a.borrow().as_ref() {
                            *c.pending.borrow_mut() = Pending::Session;
                        }
                    });
                }
                Some(ApplyState::Auth) => {
                    with_apply_run(|h| h.revert());
                    win.set_apply_info("cancelling \u{2014} finishing up\u{2026}".into());
                }
                _ => {}
            }
        });
    }
    // ---- backup / restore (§5.5) ----
    {
        let weak = win.as_weak();
        win.on_show_backups(move || {
            let Some(win) = weak.upgrade() else { return };
            refresh_backups(&win);
        });
    }
    {
        let weak = win.as_weak();
        win.on_restore_backup(move |idx| {
            let Some(win) = weak.upgrade() else { return };
            restore_backup(&win, idx);
        });
    }
    // The dead-man's switch GUI half (KEEP / revert during the countdown) lives on
    // the ApplyDialog, wired in open_apply_dialog — see handle_apply_event.

    // `--render-state <name> --render <out.png>`: screenshot harness for the UX-critic
    // pass. Placed here so every `win.on_*` handler is already wired — each state is
    // driven by invoking the SAME callbacks a click runs, so panels populate
    // authentically rather than from hand-faked models. Software-rendered (see top of
    // main), then exits without spawning the live/helper/tray machinery.
    if let Some(out) = flag_value("--render") {
        let state = flag_value("--render-state").unwrap_or_else(|| "base".into());
        drive_render_state(&win, &state);
        win.show()?;
        let weak = win.as_weak();
        let delay = flag_value("--render-delay").and_then(|s| s.parse().ok()).unwrap_or(400);
        slint::Timer::single_shot(std::time::Duration::from_millis(delay), move || {
            if let Some(win) = weak.upgrade() {
                match save_snapshot(&win, &out) {
                    Ok((w, h)) => eprintln!("rendered {state} {w}x{h} -> {out}"),
                    Err(e) => eprintln!("render error ({state}): {e}"),
                }
            }
            let _ = slint::quit_event_loop();
        });
        win.run()?;
        return Ok(());
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
pub(crate) fn render_board(win: &MainWindow) {
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
    // In chord-building mode, light up the picked members (any count). Reads the live
    // `chord_keys` model so the highlight survives glow/layer re-renders.
    if win.get_edit_mode() && win.get_board_mode() == "chord" {
        let picks: std::collections::HashSet<String> =
            win.get_chord_keys().iter().map(|s| s.to_string()).collect();
        stamp_chord_picks(&mut board, &picks);
    }
    win.set_active_board(board);
}

/// Empty the chord-builder member list (board chord-mode) and forget any chord being
/// edited — every reset path (clear, mode/layer switch, exit) abandons an in-progress edit.
fn clear_chord_builder(win: &MainWindow) {
    win.set_chord_keys(model(Vec::<slint::SharedString>::new()));
    win.set_chord_edit_orig("".into());
}

/// Add `phys` to the chord-builder member list, or remove it if already present (a
/// re-click drops it). Empty `phys` is ignored. Re-renders the board so the highlight
/// follows the change.
fn toggle_chord_member(win: &MainWindow, phys: &str) {
    use slint::Model;
    if phys.is_empty() {
        return;
    }
    let cur: Vec<slint::SharedString> = win.get_chord_keys().iter().collect();
    let next: Vec<slint::SharedString> = if cur.iter().any(|k| k.as_str() == phys) {
        cur.into_iter().filter(|k| k.as_str() != phys).collect()
    } else {
        let mut v = cur;
        v.push(phys.into());
        v
    };
    win.set_chord_keys(model(next));
    render_board(win);
}

/// Mark the caps whose `phys` is a current chord member (board chord-mode) so they
/// highlight like a selection. Mirrors [`stamp_glow`] (rebuilds the board's key model, so
/// the shared sheet model is left pristine) and is reapplied on every render, so picks
/// survive a board rebuild.
fn stamp_chord_picks(board: &mut BoardData, picks: &std::collections::HashSet<String>) {
    use slint::Model;
    let keys: Vec<KeyCapData> = board
        .keys
        .iter()
        .map(|mut k| {
            k.chord_pick = !k.phys.is_empty() && picks.contains(k.phys.as_str());
            k
        })
        .collect();
    board.keys = model(keys);
}

/// The one optional edit session, shared by the UI callbacks that need to know
/// whether (and what) we're editing. `Some(_)` == edit mode is on.
pub(crate) type SharedSession = Rc<RefCell<Option<editing::EditSession>>>;

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

/// A modifier letter's human label for a macro chord step.
fn mod_label(c: char) -> &'static str {
    keydviz_core::mods::Mod::from_letter(c).map_or("?", |m| m.word)
}

/// The UI row (kind + human label) for one macro step.
fn macro_row(tok: &MacroToken) -> MacroRow {
    let (kind, display) = match tok {
        MacroToken::Key(k) => ("key", format!("press {k}")),
        MacroToken::Delay(n) => ("delay", format!("pause {n} ms")),
        MacroToken::Text(t) => ("text", format!("type \u{201c}{t}\u{201d}")),
        MacroToken::Chord { mods, keys } => {
            let mut parts: Vec<String> = mods.iter().map(|c| mod_label(*c).to_string()).collect();
            parts.extend(keys.iter().cloned());
            ("chord", parts.join("+"))
        }
    };
    MacroRow { kind: kind.into(), display: display.into() }
}

/// Rebuild the `macro_rows` view model from the current draft step list.
fn push_macro_rows(win: &MainWindow, draft: &RefCell<Vec<MacroToken>>) {
    let rows: Vec<MacroRow> = draft.borrow().iter().map(macro_row).collect();
    win.set_macro_rows(model(rows));
}

/// Clear the macro editor's chord sub-form: the pending key and all five modifier
/// toggles. Part of every macro-form reset, so it lives in one place rather than five
/// copies of the same six setters that must be kept in sync by hand.
fn reset_macro_chord_form(win: &MainWindow) {
    win.set_macro_chord_key("".into());
    win.set_macro_chord_c(false);
    win.set_macro_chord_m(false);
    win.set_macro_chord_a(false);
    win.set_macro_chord_s(false);
    win.set_macro_chord_g(false);
}

/// Seed the macro panel from the selected key's binding: an existing macro we can
/// decompose loads its steps + repeat into the draft and marks `selected_is_macro`;
/// anything else resets to an empty builder. Clears the staged sub-form either way.
fn seed_macro(
    win: &MainWindow,
    s: &editing::EditSession,
    layer: &str,
    phys: &str,
    draft: &RefCell<Vec<MacroToken>>,
) {
    match s.current_macro(layer, phys) {
        Some(m) => {
            win.set_selected_is_macro(true);
            win.set_macro_repeat_on(m.repeat.is_some());
            match m.repeat {
                Some((t, r)) => {
                    win.set_macro_repeat_timeout(t.to_string().into());
                    win.set_macro_repeat_count(r.to_string().into());
                }
                None => {
                    win.set_macro_repeat_timeout("".into());
                    win.set_macro_repeat_count("".into());
                }
            }
            *draft.borrow_mut() = m.tokens;
        }
        None => {
            win.set_selected_is_macro(false);
            win.set_macro_repeat_on(false);
            win.set_macro_repeat_timeout("".into());
            win.set_macro_repeat_count("".into());
            draft.borrow_mut().clear();
        }
    }
    push_macro_rows(win, draft);
    win.set_macro_text_input("".into());
    win.set_macro_delay_input("".into());
    reset_macro_chord_form(win);
}

/// All `[main]` chords as UI rows for the ⌨ chords manager: `chord` is the verbatim
/// LHS (used to edit/delete), `display` the pretty `j + k` form, `action` the RHS.
fn chord_rows_for_layer(s: &editing::EditSession, layer: &str) -> Vec<ChordRow> {
    s.chords(layer)
        .into_iter()
        .map(|(chord, action)| {
            let display = chord.split('+').map(str::trim).collect::<Vec<_>>().join(" + ");
            ChordRow { chord: chord.into(), display: display.into(), action: action.into() }
        })
        .collect()
}

/// Rows for the `[global]` options form: every documented keyd global (value pulled
/// from the config, "" = unset), followed by any unrecognized `[global]` line so the
/// file's own content is never hidden.
fn global_rows_for(s: &editing::EditSession) -> Vec<GlobalRow> {
    let entries = s.global_entries();
    let value_of = |name: &str| {
        entries.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()).unwrap_or_default()
    };
    let mut rows: Vec<GlobalRow> = keydviz_core::GLOBAL_OPTIONS
        .iter()
        .map(|o| GlobalRow {
            name: o.name.into(),
            label: o.label.into(),
            value: value_of(o.name).into(),
            unit: o.unit.into(),
            hint: o.hint.into(),
            is_bool: o.boolean,
            known: true,
        })
        .collect();
    for (k, v) in &entries {
        if !keydviz_core::is_known_global(k) {
            rows.push(GlobalRow {
                name: k.clone().into(),
                label: k.clone().into(),
                value: v.clone().into(),
                unit: "".into(),
                hint: "".into(),
                is_bool: false,
                known: false,
            });
        }
    }
    rows
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

/// Seed the custom-label field and its "has a label" flag for the selected key, so
/// the label row pre-fills with the current label and shows the `clear` button only
/// when one is set. Independent of the binding kind.
fn seed_label(win: &MainWindow, s: &editing::EditSession, layer: &str, phys: &str) {
    let label = s.current_label(layer, phys).unwrap_or_default();
    win.set_selected_has_label(!label.is_empty());
    win.set_edit_label(label.into());
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
        // Reframed from a failure ("no keyboard matches") to the actual situation:
        // editing works fine, the changes just don't drive a live keyboard right now
        // (true for an example config or a real one whose keyboard is unplugged).
        format!("{path} \u{2014} no matching keyboard connected; edits still save to this file")
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
    win.set_chord_rows(model(chord_rows_for_layer(&s, default_layer.as_str())));
    win.set_edit_layer(default_layer);
    win.set_key_mode("simple".into());
    win.set_board_mode("single".into());
    clear_chord_builder(win);
    win.set_chord_action("".into());
    win.set_selected_is_macro(false);
    win.set_macro_rows(model(Vec::<MacroRow>::new()));
    win.set_macro_text_input("".into());
    win.set_macro_delay_input("".into());
    reset_macro_chord_form(win);
    win.set_macro_repeat_on(false);
    win.set_macro_repeat_timeout("".into());
    win.set_macro_repeat_count("".into());
    win.set_editing_global(false);
    win.set_global_rows(model(global_rows_for(&s)));
    win.set_rename_target("".into());
    win.set_rename_name("".into());
    win.set_selected_phys("".into());
    set_current_binding(win, "");
    win.set_edit_value("".into());
    win.set_edit_label("".into());
    win.set_selected_has_label(false);
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
    refresh_backups(win); // populate the restore panel (reads the now-set session)
    win.set_edit_mode(true);
    render_board(win); // freeze the board to the chosen section
}

/// Clear every edit-mode UI property back to its resting state. Shared by
/// `exit_edit` (the normal leave) and `remove_config_after_delete` (the config was
/// just deleted) so the two can't drift. Does NOT touch the session, the sheet
/// sources, or `edit_mode` — the caller owns those.
pub(crate) fn reset_edit_ui(win: &MainWindow) {
    win.set_capture_armed(false);
    win.set_backups_open(false);
    win.set_has_backups(false);
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
    win.set_delete_config_prompt(false);
    win.set_rename_target("".into());
    win.set_rename_name("".into());
    win.set_can_rename(false);
    win.set_edit_label("".into());
    win.set_selected_has_label(false);
    win.set_key_mode("simple".into());
    win.set_board_mode("single".into());
    win.set_chord_rows(model(Vec::<ChordRow>::new()));
    clear_chord_builder(win);
    win.set_chord_action("".into());
    win.set_selected_is_macro(false);
    win.set_macro_rows(model(Vec::<MacroRow>::new()));
    win.set_macro_text_input("".into());
    win.set_macro_delay_input("".into());
    reset_macro_chord_form(win);
    win.set_macro_repeat_on(false);
    win.set_macro_repeat_timeout("".into());
    win.set_macro_repeat_count("".into());
    win.set_editing_global(false);
    win.set_global_rows(model(Vec::<GlobalRow>::new()));
    win.set_apply_available(false);
    win.set_apply_hint("".into());
    reset_apply_ui(win);
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
    reset_edit_ui(win);
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

/// The shared tail of a successful in-place edit, used by every mutating edit-op
/// callback. Given the still-borrowed session guard `sb` and the mutator's `result`,
/// on `Ok` it snapshots the new config + dirty flag, runs `after_model` (panel reseeds
/// / warning recompute that still need the session `s`), drops the borrow, runs
/// `after_ui` (window-only property updates), then pushes the universal `edit_dirty`
/// flag + board-preview refresh. On `Err` it shows the banner and does nothing else.
/// Anything a callback needs to read from `s` for `after_ui` (e.g. row models, the
/// canonical written binding) is computed before the call, while `s` is still borrowed.
/// Centralizing this means no callback can forget a step or get the read-cfg → drop →
/// refresh ordering wrong.
fn commit_edit(
    win: &MainWindow,
    srcs: &Rc<RefCell<Vec<SheetSrc>>>,
    sb: std::cell::RefMut<Option<editing::EditSession>>,
    result: Result<(), String>,
    after_model: impl FnOnce(&MainWindow, &editing::EditSession),
    after_ui: impl FnOnce(&MainWindow),
) {
    match result {
        Ok(()) => {
            let Some(s) = sb.as_ref() else { return };
            let (cfg, dirty, path) = (s.config(), s.dirty(), s.path.clone());
            after_model(win, s);
            drop(sb);
            after_ui(win);
            win.set_edit_dirty(dirty);
            refresh_preview(win, srcs, &path, cfg);
        }
        Err(e) => win.set_edit_banner(format!("\u{26a0} {e}").into()),
    }
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
    let diff = s.diff_annotated();
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
    window_set_on_top(win.window(), on);
}

/// The window-agnostic core of [`set_window_on_top`], so the apply dialog can be
/// kept above too (it shares the same winit-backed always-on-top mechanism).
pub(crate) fn window_set_on_top(window: &slint::Window, on: bool) {
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
