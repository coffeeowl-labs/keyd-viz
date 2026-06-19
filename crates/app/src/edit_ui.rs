//! Edit-mode UI glue: the bridge between an [`editing::EditSession`] and the Slint
//! window. Board rendering ([`render_board`]), the chord builder, the per-edit form
//! seeders (tap-hold, macro, label), the warning/preview refreshers, and the
//! enter/exit/commit lifecycle all live here. The session model and its mutators are
//! in `editing`; the apply path is in `apply_ctx`; this module is the
//! window-property marshalling in between.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use keydviz_core::{parse_file, Behavior, Config, MacroToken, MODIFIERS};

use crate::editing;
use crate::glow::stamp_glow;
use crate::{
    apply_gate, build_sheet_data, model, refresh_backups, reset_apply_ui, set_current_binding,
    teardown_apply, BoardData, ChordRow, EditLayer, GlobalRow, KeyCapData, MacroRow, MainWindow,
    SheetData, SheetSrc, TRAY,
};

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
pub(crate) fn clear_chord_builder(win: &MainWindow) {
    win.set_chord_keys(model(Vec::<slint::SharedString>::new()));
    win.set_chord_edit_orig("".into());
    // Reset the inline layer-action picker so a stale target can't carry between chords.
    win.set_chord_layer_target("".into());
    win.set_chord_layer_kind("momentary".into());
}

/// Add `phys` to the chord-builder member list, or remove it if already present (a
/// re-click drops it). Empty `phys` is ignored. Re-renders the board so the highlight
/// follows the change.
pub(crate) fn toggle_chord_member(win: &MainWindow, phys: &str) {
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
pub(crate) fn stamp_chord_picks(board: &mut BoardData, picks: &std::collections::HashSet<String>) {
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
pub(crate) fn publish_sheet(win: &MainWindow, idx: usize, src: &SheetSrc) {
    use slint::Model;
    let data = build_sheet_data(src);
    win.get_sheets().set_row_data(idx, data.clone());
    if win.get_active_index().max(0) as usize == idx {
        win.set_active_sheet(data);
    }
}

/// Switch the shown board to detected keyboard `idx` (no-op if out of range).
pub(crate) fn switch_keyboard(win: &MainWindow, idx: i32) {
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
pub(crate) fn renameable(base: &str) -> bool {
    !base.is_empty() && base != "main" && !base.contains('+')
}

pub(crate) fn edit_layer_choices(s: &editing::EditSession) -> Vec<EditLayer> {
    s.editable_sections()
        .into_iter()
        .map(|n| EditLayer { name: n.clone().into(), display: n.into() })
        .collect()
}

/// A modifier letter's human label for a macro chord step.
pub(crate) fn mod_label(c: char) -> &'static str {
    keydviz_core::mods::Mod::from_letter(c).map_or("?", |m| m.word)
}

/// The UI row (kind + human label) for one macro step.
pub(crate) fn macro_row(tok: &MacroToken) -> MacroRow {
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
pub(crate) fn push_macro_rows(win: &MainWindow, draft: &RefCell<Vec<MacroToken>>) {
    let rows: Vec<MacroRow> = draft.borrow().iter().map(macro_row).collect();
    win.set_macro_rows(model(rows));
}

/// Clear the macro editor's chord sub-form: the pending key and all five modifier
/// toggles. Part of every macro-form reset, so it lives in one place rather than five
/// copies of the same six setters that must be kept in sync by hand.
pub(crate) fn reset_macro_chord_form(win: &MainWindow) {
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
pub(crate) fn seed_macro(
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
pub(crate) fn chord_rows_for_layer(s: &editing::EditSession, layer: &str) -> Vec<ChordRow> {
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
pub(crate) fn global_rows_for(s: &editing::EditSession) -> Vec<GlobalRow> {
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
pub(crate) fn hold_layer_choices(s: &editing::EditSession) -> Vec<slint::SharedString> {
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
pub(crate) fn refresh_warnings(win: &MainWindow, s: &editing::EditSession) {
    win.set_edit_warnings(s.orphan_warnings().join("\n").into());
}

pub(crate) fn seed_tap_hold(win: &MainWindow, s: &editing::EditSession, layer: &str, phys: &str) {
    match s.current_tap_hold(layer, phys) {
        Some(th) => {
            win.set_selected_is_tap_hold(true);
            // Light the matching feel chip; leave BOTH unlit ("") for a tap/hold
            // whose form we don't name (plain overload/overloadt) so editing it
            // preserves the form rather than silently converting it.
            win.set_th_feel(feel_str(th.behavior()).into());
            win.set_th_hold(th.target.into());
            // `current_tap_hold` only returns tap-bearing forms now (pure momentary
            // `layer()` is owned by Layer mode), so `tap` is always present here.
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
            win.set_th_tap(default_tap.into());
        }
    }
}

/// Pre-fill the Layer-mode picker for the selected key: if its binding is a pure
/// layer action (`layer()`/`toggle()`/`oneshot()`), light the matching target +
/// behavior; otherwise default to momentary with no target chosen (a sensible start
/// for a fresh layer key). `selected_is_layer_action` drives the select-time mode
/// classifier in `main.rs`.
pub(crate) fn seed_layer_action(
    win: &MainWindow,
    s: &editing::EditSession,
    layer: &str,
    phys: &str,
) {
    match s.current_layer_action(layer, phys) {
        Some(la) => {
            win.set_selected_is_layer_action(true);
            win.set_layer_action_kind(la.kind.token().into());
            win.set_layer_action_target(la.target.into());
        }
        None => {
            win.set_selected_is_layer_action(false);
            win.set_layer_action_kind("momentary".into());
            win.set_layer_action_target("".into());
        }
    }
}

/// Seed the custom-label field and its "has a label" flag for the selected key, so
/// the label row pre-fills with the current label and shows the `clear` button only
/// when one is set. Independent of the binding kind.
pub(crate) fn seed_label(win: &MainWindow, s: &editing::EditSession, layer: &str, phys: &str) {
    let label = s.current_label(layer, phys).unwrap_or_default();
    win.set_selected_has_label(!label.is_empty());
    win.set_edit_label(label.into());
}

/// The UI "feel" token for an existing binding's behavior: `""` (no chip lit) for
/// a form outside the two-behavior model, so editing it preserves rather than
/// converts. Kept in sync with [`feel_from_str`].
pub(crate) fn feel_str(b: Option<Behavior>) -> &'static str {
    match b {
        Some(Behavior::Responsive) => "fast",
        Some(Behavior::TypingSafe) => "safe",
        None => "", // unnamed form (plain overload/overloadt) → no feel chosen
    }
}

/// Map the UI "feel" token to a [`Behavior`]. `""` → `None` ("no feel chosen":
/// preserve an existing unnamed form); otherwise a concrete feel.
pub(crate) fn feel_from_str(s: &str) -> Option<Behavior> {
    match s {
        "fast" => Some(Behavior::Responsive),
        "safe" => Some(Behavior::TypingSafe),
        _ => None,
    }
}

/// Minimal §5.5 affected-keyboards line for the edit banner: which connected
/// device(s) the file being edited currently governs.
pub(crate) fn affected_line(src: &SheetSrc) -> String {
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
pub(crate) fn enter_edit_session(
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
    win.set_selected_is_layer_action(false);
    win.set_layer_action_target("".into());
    win.set_layer_action_kind("momentary".into());
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
    win.set_selected_is_layer_action(false);
    win.set_layer_action_target("".into());
    win.set_layer_action_kind("momentary".into());
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
pub(crate) fn exit_edit(win: &MainWindow, srcs: &Rc<RefCell<Vec<SheetSrc>>>, session: &SharedSession) {
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
pub(crate) fn refresh_preview(win: &MainWindow, srcs: &Rc<RefCell<Vec<SheetSrc>>>, path: &Path, cfg: Config) {
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
pub(crate) fn commit_edit(
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
pub(crate) fn draft_summary(s: &editing::EditSession, saved: &editing::DraftSaved) -> String {
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
