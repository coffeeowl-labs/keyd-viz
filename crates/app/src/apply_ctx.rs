//! One-click apply lifecycle (Phase 6 E2) — the UI-thread state machine around the
//! `keydviz-apply` pkexec path.
//!
//! [`applying`] owns the transport (spawn the tool, ferry protocol lines, keep/revert);
//! this module owns the *policy and UI state*: the typed [`ApplyState`] mirrored into the
//! Slint `apply-state` property, the singleton [`ApplyCtx`] thread-local that survives any
//! one edit session, what the next apply will write ([`Pending`]), the pre-flight gate, the
//! countdown dialog, and the backup restore / config delete flows that piggy-back on the
//! same dead-man's-switch apply. Every terminal transition funnels through
//! [`handle_apply_event`].

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use slint::ComponentHandle;

use keydviz_core::parse_file;

use crate::{applying, editing};
use crate::{
    build_sheet_data, model, publish_sheet, render_board, reset_edit_ui, set_current_binding,
    window_set_on_top, ApplyDialog, BackupRow, MainWindow, SharedSession, SheetData, SheetSrc,
};

/// The apply lifecycle, mirrored into the Slint string property `MainWindow.apply-state`
/// (app.slint matches these exact tokens to drive the apply UI). Keeping the closed set
/// on the Rust side means a mistyped state is a compile error here, not a silently-dead
/// branch — previously these tokens were ~30 bare string literals set and compared by hand.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyState {
    Idle,
    Confirm,
    Auth,
    Countdown,
    Kept,
    Reverted,
    Failed,
    RevertFailed,
}

impl ApplyState {
    /// The exact token written to the Slint property (and matched in app.slint).
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ApplyState::Idle => "idle",
            ApplyState::Confirm => "confirm",
            ApplyState::Auth => "auth",
            ApplyState::Countdown => "countdown",
            ApplyState::Kept => "kept",
            ApplyState::Reverted => "reverted",
            ApplyState::Failed => "failed",
            ApplyState::RevertFailed => "revert-failed",
        }
    }

    /// Parse the Slint property back to the typed state, so reads funnel through the
    /// same closed set as writes. `None` only for a token nothing here produces.
    pub(crate) fn from_token(s: &str) -> Option<ApplyState> {
        Some(match s {
            "idle" => ApplyState::Idle,
            "confirm" => ApplyState::Confirm,
            "auth" => ApplyState::Auth,
            "countdown" => ApplyState::Countdown,
            "kept" => ApplyState::Kept,
            "reverted" => ApplyState::Reverted,
            "failed" => ApplyState::Failed,
            "revert-failed" => ApplyState::RevertFailed,
            _ => return None,
        })
    }
}

/// What the next apply will write, parked on the singleton [`ApplyCtx`]. A single
/// typed field replaces the two out-of-band `Option` flags this used to be (a
/// `restore_bytes` and a `deleting` path): every transition replaces the whole value,
/// so a half-cleared override is unrepresentable — there's nothing to forget to clear.
pub(crate) enum Pending {
    /// The idle default: apply the session's own current edits.
    Session,
    /// Restore a backup's bytes (armed by `restore_backup`, consumed by `launch_apply`).
    Restore(Vec<u8>),
    /// Delete `<dir>/<name>.conf`; the path is the board to drop on `kept` (armed by
    /// `launch_delete`, read by `handle_apply_event`).
    Delete(PathBuf),
}

/// One-click apply bookkeeping (E2), UI-thread only. The protocol thread can only
/// ferry plain [`applying::ApplyEvent`]s across `invoke_from_event_loop`, so the
/// state the event handler needs — the session to re-base on `kept`, the sheet
/// sources to republish, the live handle for keep/revert, the countdown timer —
/// lives in a thread-local, same shape as [`TRAY`]. Seeded once in `main`.
pub(crate) struct ApplyCtx {
    pub(crate) session: SharedSession,
    pub(crate) srcs: Rc<RefCell<Vec<SheetSrc>>>,
    pub(crate) run: RefCell<Option<applying::ApplyHandle>>,
    pub(crate) timer: RefCell<Option<slint::Timer>>,
    /// The always-on-top countdown dialog (test field + timer + KEEP/revert),
    /// alive only while a run is in `countdown`. Held so `finish`/`teardown_apply`
    /// can close it and the timer can drive its seconds.
    pub(crate) dialog: RefCell<Option<ApplyDialog>>,
    /// What the next apply will write — see [`Pending`]. One typed field that the
    /// arming/launch/terminal transitions replace wholesale, so a stale override can't
    /// ride a later apply (this `ApplyCtx` is a process-wide singleton outliving any one
    /// session). It holds `Pending::Restore` across the sensitive-confirm wait between
    /// `restore_backup` arming it and `launch_apply` consuming it, and `Pending::Delete`
    /// from `launch_delete` until the terminal event (so `handle_apply_event` knows to
    /// drop the board on `kept`); every other path resets it to `Pending::Session`.
    pub(crate) pending: RefCell<Pending>,
    /// The backups last shown in the restore panel, newest first; `restore_backup(i)`
    /// indexes this. Refreshed on edit-enter and after each kept apply.
    pub(crate) backups: RefCell<Vec<applying::Backup>>,
}

thread_local! {
    pub(crate) static APPLY: RefCell<Option<ApplyCtx>> = const { RefCell::new(None) };
}

/// True while an apply run is in flight (confirm / auth / countdown).
pub(crate) fn apply_busy(win: &MainWindow) -> bool {
    matches!(
        ApplyState::from_token(&win.get_apply_state()),
        Some(ApplyState::Confirm | ApplyState::Auth | ApplyState::Countdown)
    )
}

/// Run `f` against the live apply handle if there is one — the single place that
/// knows how the handle is parked (KEEP / revert / cancel all go through here, so
/// the access shape lives in one spot instead of three copies).
pub(crate) fn with_apply_run(f: impl FnOnce(&applying::ApplyHandle)) {
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
pub(crate) fn reset_apply_ui(win: &MainWindow) {
    win.set_apply_state(ApplyState::Idle.as_str().into());
    win.set_apply_info("".into());
    win.set_backups_open(false);
}

/// Open the always-on-top countdown dialog: a test field (auto-focused, so the
/// user can immediately type to verify the remap — keyd remaps at the device
/// level, so our own field exercises the live config), the live timer, and
/// KEEP/revert. Closing the dialog reverts, like any non-KEEP exit. Returns the
/// dialog + the timer driving its seconds; the caller parks both in `ApplyCtx`.
pub(crate) fn open_apply_dialog(
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
pub(crate) fn teardown_apply() {
    with_apply_run(|h| h.revert());
    APPLY.with(|a| {
        if let Some(ctx) = a.borrow().as_ref() {
            *ctx.timer.borrow_mut() = None;
            *ctx.run.borrow_mut() = None;
            // Leaving edit mode / switching configs disarms any pending intent: the
            // singleton ApplyCtx outlives a session, so an override armed for the old
            // config must not survive into the next one.
            *ctx.pending.borrow_mut() = Pending::Session;
            if let Some(d) = ctx.dialog.borrow_mut().take() {
                let _ = d.hide();
            }
        }
    });
}

/// Refuse session-changing actions while an apply is in flight: a binding edit or
/// session swap mid-countdown has no good semantics (which bytes did the user
/// keep?), so the answer is one visible "not now" instead.
pub(crate) fn refuse_if_applying(win: &MainWindow) -> bool {
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
pub(crate) fn apply_gate(s: &editing::EditSession) -> (bool, String) {
    match applying::one_click() {
        Some(inv) => (s.apply_target(inv.config_dir()).is_some(), String::new()),
        None => {
            let hint = if s.apply_target(applying::prod_config_dir()).is_some() {
                "applying changes directly needs keyd-viz installed (AUR or source) \
                 \u{2014} for now, use 'save draft' and follow its install steps"
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
pub(crate) fn launch_apply(win: &MainWindow, sensitive_ok: bool) {
    APPLY.with(|a| {
        let actx = a.borrow();
        let Some(ctx) = actx.as_ref() else { return };
        // Consume the pending intent up front, before the early returns below: a launch
        // that bails (tool uninstalled mid-session, etc.) must NOT leave a restore armed
        // to be silently picked up by a later apply. Reset to `Session` means an ordinary
        // apply of the session's own edits — including the `Delete` case, which never
        // reaches here (it goes through `launch_delete`), so it harmlessly falls through.
        let pending = ctx.pending.replace(Pending::Session);
        // Re-resolve instead of caching: tool/pkexec could have been (un)installed
        // since the session opened. On the None paths surface a visible failure
        // rather than a dead button — the most likely cause is the tool being
        // removed mid-session, which the user should be told about.
        let Some(how) = applying::one_click() else {
            win.set_apply_state(ApplyState::Failed.as_str().into());
            win.set_apply_info(
                "can't apply changes directly right now \u{2014} keyd-viz isn't fully \
                 installed. use 'save draft' instead"
                    .into(),
            );
            return;
        };
        let sb = ctx.session.borrow();
        let Some(s) = sb.as_ref() else { return };
        let Some(name) = s.apply_target(how.config_dir()) else {
            drop(sb);
            win.set_apply_state(ApplyState::Failed.as_str().into());
            win.set_apply_info("keyd-viz can't apply this config directly anymore".into());
            return;
        };
        // A pending restore applies the backup's bytes; otherwise the session's.
        let bytes = match pending {
            Pending::Restore(b) => b,
            _ => s.serialized().into_bytes(),
        };
        drop(sb);
        // keyd reload bounces the virtual device mid-apply; don't leave a stale
        // capture armed across the hiccup.
        win.set_capture_armed(false);
        win.set_apply_state(ApplyState::Auth.as_str().into());
        let weak = win.as_weak();
        let req = applying::ApplyRequest {
            name,
            bytes,
            sensitive_ok,
            op: applying::ApplyOp::Apply,
            how,
        };
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
                win.set_apply_state(ApplyState::Failed.as_str().into());
                win.set_apply_info(format!("couldn't start applying: {e}").into());
            }
        }
    });
}

/// A byte count as a compact human size for the restore list ("843 B" / "1.2 KB").
pub(crate) fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else {
        format!("{:.1} KB", n as f64 / 1024.0)
    }
}

/// Re-list this config's timestamped backups and publish them to the restore panel.
/// Cheap (one dir read, no auth); called on edit-enter and after each kept apply (a
/// kept apply just created a fresh backup and pruned old ones). Sets `has_backups`
/// so the "restore from backup…" button only appears when there's something to roll
/// back to. A no-op (empty list) when one-click apply isn't available for this config.
pub(crate) fn refresh_backups(win: &MainWindow) {
    APPLY.with(|a| {
        let actx = a.borrow();
        let Some(ctx) = actx.as_ref() else { return };
        let mut rows: Vec<BackupRow> = Vec::new();
        if let Some(how) = applying::one_click() {
            let sb = ctx.session.borrow();
            if let Some(name) = sb.as_ref().and_then(|s| s.apply_target(how.config_dir())) {
                let baks = applying::list_backups(how.config_dir(), &name);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                rows = baks
                    .iter()
                    .map(|b| BackupRow {
                        ago: applying::describe_age(b.stamp, now).into(),
                        detail: format!("{} \u{00b7} {}", applying::fmt_utc(b.stamp), human_size(b.size))
                            .into(),
                    })
                    .collect();
                *ctx.backups.borrow_mut() = baks;
            } else {
                ctx.backups.borrow_mut().clear();
            }
        } else {
            ctx.backups.borrow_mut().clear();
        }
        win.set_has_backups(!rows.is_empty());
        win.set_backups(model(rows));
    });
}

/// Restore the `idx`-th backup (as shown in the panel, newest first): read its bytes,
/// pre-flight them exactly like a normal apply (size, `keyd check`, sensitive-construct
/// scan), arm the restore override, and route into the apply flow. The override makes
/// `launch_apply` write the backup's bytes instead of the session's, so the restore is
/// syntax-checked, backs up the *current* config first (undoable), and rides the
/// countdown/dead-man's switch — a bad backup self-reverts just like a bad edit.
pub(crate) fn restore_backup(win: &MainWindow, idx: i32) {
    let backup = APPLY.with(|a| {
        a.borrow().as_ref().and_then(|c| c.backups.borrow().get(idx as usize).cloned())
    });
    let Some(backup) = backup else { return };
    let bytes = match std::fs::read(&backup.path) {
        Ok(b) => b,
        Err(e) => {
            win.set_apply_state(ApplyState::Failed.as_str().into());
            win.set_apply_info(format!("couldn't read the backup file: {e}").into());
            return;
        }
    };
    if bytes.len() > keydviz_apply::MAX_CONFIG_BYTES {
        win.set_apply_state(ApplyState::Failed.as_str().into());
        win.set_apply_info("this backup is too large to restore".into());
        return;
    }
    // Configs are text; a non-UTF-8 backup is corrupt — refuse rather than mangle it.
    let Ok(text) = std::str::from_utf8(&bytes) else {
        win.set_apply_state(ApplyState::Failed.as_str().into());
        win.set_apply_info("this backup file is corrupt (not valid text)".into());
        return;
    };
    // The backup was valid when written, but the installed keyd may have changed —
    // catch a now-invalid config here, before paying the auth prompt.
    if let Some(Err(e)) = editing::keyd_check_bytes(text) {
        win.set_apply_state(ApplyState::Failed.as_str().into());
        win.set_apply_info(
            format!("this backup is no longer valid for your keyd and can't be restored:\n{e}")
                .into(),
        );
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut info = format!(
        "Restoring the backup from {} ({}). The current config is backed up first, so \
         this is undoable.\n",
        applying::describe_age(backup.stamp, now),
        applying::fmt_utc(backup.stamp),
    );
    let findings = keydviz_apply::scan::scan(&bytes);
    for f in &findings {
        if let Some(s) = applying::finding_summary(f) {
            info.push_str(&format!("\u{26a0} {s}\n"));
        }
    }
    // Arm the override so launch_apply writes these bytes, then run the normal path.
    APPLY.with(|a| {
        if let Some(c) = a.borrow().as_ref() {
            *c.pending.borrow_mut() = Pending::Restore(bytes);
        }
    });
    win.set_apply_info(info.into());
    if findings.iter().any(|f| f.needs_ack()) {
        win.set_apply_state(ApplyState::Confirm.as_str().into());
    } else {
        launch_apply(win, false);
    }
}

/// Spawn the apply tool to *delete* `<dir>/<name>.conf` (the caller has confirmed
/// and verified `name` is an allow-listed target). Mirrors `launch_apply`: the same
/// auth → countdown → KEEP/revert flow, but a `delete` request with no payload. The
/// `Pending::Delete` intent tells `handle_apply_event` to drop the board on `kept`.
pub(crate) fn launch_delete(win: &MainWindow, name: String, path: PathBuf) {
    APPLY.with(|a| {
        let actx = a.borrow();
        let Some(ctx) = actx.as_ref() else { return };
        let Some(how) = applying::one_click() else {
            win.set_apply_state(ApplyState::Failed.as_str().into());
            win.set_apply_info(
                "can't delete this config directly right now \u{2014} keyd-viz isn't fully \
                 installed. remove the file manually"
                    .into(),
            );
            return;
        };
        win.set_capture_armed(false);
        *ctx.pending.borrow_mut() = Pending::Delete(path);
        win.set_apply_state(ApplyState::Auth.as_str().into());
        let weak = win.as_weak();
        let req = applying::ApplyRequest {
            name,
            bytes: Vec::new(),
            sensitive_ok: false,
            op: applying::ApplyOp::Delete,
            how,
        };
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
                *ctx.pending.borrow_mut() = Pending::Session;
                win.set_apply_state(ApplyState::Failed.as_str().into());
                win.set_apply_info(format!("couldn't start applying: {e}").into());
            }
        }
    });
}

/// After a `kept` *delete*: the file is gone from disk. Drop the session, remove the
/// config's board, leave edit mode, and reselect a surviving board. Runs inside the
/// apply event handler, so — unlike `exit_edit` — it must NOT call `teardown_apply`
/// (that would re-borrow the `APPLY` thread-local we're already inside); the run has
/// already ended on `kept`, so there is nothing to tear down anyway.
pub(crate) fn remove_config_after_delete(win: &MainWindow, ctx: &ApplyCtx, path: &Path) {
    *ctx.session.borrow_mut() = None;
    win.set_edit_mode(false);
    reset_edit_ui(win);
    let mut srcs = ctx.srcs.borrow_mut();
    if let Some(idx) = srcs.iter().position(|x| x.path == path) {
        srcs.remove(idx);
    }
    let data: Vec<SheetData> = srcs.iter().map(build_sheet_data).collect();
    win.set_sheets(model(data));
    if !srcs.is_empty() {
        let active = win.get_active_index().max(0).min(srcs.len() as i32 - 1);
        win.set_active_index(active);
        use slint::Model;
        if let Some(row) = win.get_sheets().row_data(active as usize) {
            win.set_active_sheet(row);
        }
    } else {
        win.set_active_index(0);
    }
    drop(srcs);
    render_board(win);
}

/// Apply protocol events, on the UI thread. Terminal events stop the countdown
/// and release the handle; `Kept` additionally re-bases the session on the new
/// on-disk state. The countdown timer is cosmetic — only the tool's verdict
/// lines decide the outcome, so the timer reaching 0 changes nothing by itself.
pub(crate) fn handle_apply_event(win: &MainWindow, ev: applying::ApplyEvent) {
    use applying::ApplyEvent as E;
    APPLY.with(|a| {
        let actx = a.borrow();
        let Some(ctx) = actx.as_ref() else { return };
        let finish = |state: ApplyState, info: Option<String>| {
            *ctx.timer.borrow_mut() = None;
            *ctx.run.borrow_mut() = None;
            *ctx.pending.borrow_mut() = Pending::Session;
            if let Some(d) = ctx.dialog.borrow_mut().take() {
                let _ = d.hide();
            }
            win.set_apply_state(state.as_str().into());
            if let Some(info) = info {
                win.set_apply_info(info.into());
            }
        };
        match ev {
            // Advisory echo — pre-flight already listed the findings.
            E::Finding(_) => {}
            E::Applied { secs } => {
                win.set_apply_state(ApplyState::Countdown.as_str().into());
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
                            format!("couldn't open the test window: {e} \u{2014} reverting")
                                .into(),
                        );
                    }
                }
            }
            E::Kept => {
                // A kept *delete* removes the board and leaves edit mode; a kept
                // *apply* re-bases the session on the new on-disk bytes.
                let deleted = match &*ctx.pending.borrow() {
                    Pending::Delete(p) => Some(p.clone()),
                    _ => None,
                };
                if let Some(path) = deleted {
                    finish(ApplyState::Idle, None);
                    remove_config_after_delete(win, ctx, &path);
                } else {
                    let path =
                        ctx.session.borrow().as_ref().map(|s| s.path.display().to_string());
                    finish(ApplyState::Kept, path.map(|p| format!("{p} updated")));
                    reopen_after_kept(win, ctx);
                }
            }
            E::Reverted(reason) => {
                let why = match reason.as_str() {
                    "TimedOut" => "no confirmation in time",
                    "Eof" => "cancelled",
                    other => other,
                };
                finish(
                    ApplyState::Reverted,
                    Some(format!(
                        "reverted: {why} \u{2014} the previous config is back; \
                         your edits are still staged"
                    )),
                );
            }
            // Verbatim: the tool's message names the backup file and the panic
            // sequence — exactly what the user needs to copy.
            E::RevertFailed(w) => finish(ApplyState::RevertFailed, Some(w)),
            E::Refused(r) => {
                finish(ApplyState::Failed, Some(format!("refused: {r} \u{2014} nothing was written")));
            }
            E::AuthDismissed => {
                finish(
                    ApplyState::Failed,
                    Some("authentication cancelled \u{2014} nothing was written".to_string()),
                );
            }
            E::NotAuthorized => {
                finish(
                    ApplyState::Failed,
                    Some(
                        "authorization unavailable (is a polkit agent running?) \
                         \u{2014} nothing was written"
                            .to_string(),
                    ),
                );
            }
            E::Failed(m) => finish(ApplyState::Failed, Some(m)),
        }
    });
}

/// After `kept`, the file on disk IS the session's bytes. Re-open rather than
/// poke flags: `original` re-bases (truthful staleness from here on), the model
/// is clean at the `EditConfig` level (where `dirty()` actually looks), and the
/// §5.1 gate re-verifies that our own output round-trips. The reload watcher
/// needs no help — its session-path exemption holds (we're still editing), and
/// after `exit_edit` it sees one mtime bump and does a single redundant reload.
pub(crate) fn reopen_after_kept(win: &MainWindow, ctx: &ApplyCtx) {
    let Some(path) = ctx.session.borrow().as_ref().map(|s| s.path.clone()) else { return };
    match editing::EditSession::open(&path) {
        Ok(new_s) => {
            // Preserve the user's place: same section, same selected key.
            let layer = win.get_edit_layer().to_string();
            let phys = win.get_selected_phys().to_string();
            if !phys.is_empty() {
                let cur = new_s.current_binding(&layer, &phys).unwrap_or_default();
                set_current_binding(win, cur.clone());
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
            // The kept apply/restore just wrote a fresh backup (and pruned old ones);
            // re-list so the restore panel reflects the new history.
            refresh_backups(win);
            render_board(win);
        }
        Err(v) => {
            // Shouldn't happen — we wrote serialize() output, which round-trips by
            // construction. Keep the old session rather than yank the user out of
            // edit mode, but say so.
            win.set_edit_banner(
                format!("\u{26a0} couldn't reload the config after applying: {}", v.describe())
                    .into(),
            );
        }
    }
}
