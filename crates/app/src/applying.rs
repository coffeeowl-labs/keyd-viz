//! GUI side of the one-click apply path (Phase 6 E2, design doc §5.2–§5.4).
//!
//! Speaks the `keydviz-apply` stdin/stdout protocol (see `crates/apply/src/main.rs`)
//! across a pkexec boundary. This module owns *transport only*: spawning the tool,
//! ferrying protocol lines back as [`ApplyEvent`]s, and the keep/revert half of the
//! dead-man's switch. Pre-flight policy (scan, `keyd check`, staleness, gating) and
//! all UI state live with the caller in `main.rs`.
//!
//! Safety posture, inherited from the design:
//! - The GUI never passes paths; the request names a config and the *tool* derives
//!   `/etc/keyd/<name>.conf`. pkexec is invoked with the tool's **absolute** path so
//!   the polkit action's `exec.path` annotation matches.
//! - The user's KEEP click is the only thing that persists a change. Everything
//!   else — revert click, app crash, window closed, timeout — drops our end of the
//!   tool's stdin, and EOF reverts. Correct by construction: the failure mode and
//!   the cancel path are the same code path.
//! - All protocol I/O happens on one background thread. The request payload can be
//!   up to 64 KiB (`MAX_CONFIG_BYTES` == the pipe buffer), and during the polkit
//!   auth dialog nothing reads the pipe — a UI-thread write could block the event
//!   loop for as long as the auth prompt sits open.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

/// pkexec's own absolute path — like the tool's `KEYD_PATHS`, never a PATH lookup
/// on the privileged boundary.
const PKEXEC: &str = "/usr/bin/pkexec";

/// The packaged apply tool. This MUST be the exact path the polkit policy's
/// `org.freedesktop.policykit.exec.path` annotation binds (see
/// `packaging/polkit/…`): pkexec'ing any other path falls through to polkit's
/// generic exec action, silently losing our deliberately-chosen `auth_admin`
/// default (a hand-install elsewhere therefore stays draft-then-install — it has
/// no matching action anyway). The AUR package and `install.sh` both install here.
const APPLY_TOOL: &str = "/usr/bin/keydviz-apply";

/// Dead-man window we ask for. The tool clamps to 5–120; 30 gives time to
/// physically try the keyboard without leaving a wedged session broken for long.
pub const TIMEOUT_SECS: u64 = 30;

/// The directory the privileged tool writes to (it derives `<dir>/<name>.conf`
/// from the name; the GUI never passes a path). The one source of this constant —
/// the gate, the production `config_dir()`, and the "tool not installed" hint all
/// read it so they can't drift.
pub fn prod_config_dir() -> &'static Path {
    Path::new("/etc/keyd")
}

/// keyd's secondary include search dir (`DATA_DIR`), tried after the config's own
/// directory when resolving an `include`. Both are root-owned, which is exactly why
/// included content is already privilege-gated — the closure scan below is an
/// advisory footgun-catcher, not a security boundary (design §5.3).
pub fn keyd_data_dir() -> &'static Path {
    Path::new("/usr/share/keyd")
}

/// A `command(`/`macro(` found inside a file an `include` directive pulls in. The
/// inline byte scan ([`keydviz_apply::scan`]) can't see across the include boundary,
/// so the one-click apply path reads **one level** of includes and surfaces these for
/// the same explicit confirmation as an inline sensitive action (design §5.3). The
/// content is root-gated, so this is a heads-up — not an escalation gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludedFinding {
    /// The `include <arg>` argument that pulled the file in.
    pub include_arg: String,
    /// The resolved file that was scanned.
    pub file: PathBuf,
    /// The sensitive construct found inside it (always `needs_ack`).
    pub finding: keydviz_apply::scan::Finding,
}

impl IncludedFinding {
    /// A pre-flight line naming the include, the resolved file, and the construct.
    pub fn describe(&self) -> String {
        format!(
            "included `{}` ({}) {}",
            self.include_arg,
            self.file.display(),
            self.finding.describe(),
        )
    }
}

/// Resolve an `include` argument the way keyd does for the lookup — the config's own
/// directory first, then `DATA_DIR` — returning the first existing regular file. Only
/// plain relative args reach here; absolute / `..` ones are flagged `SuspectInclude`
/// and ignored by keyd, so they're never followed.
fn resolve_include(arg: &str, config_dir: &Path, data_dir: &Path) -> Option<PathBuf> {
    [config_dir, data_dir].into_iter().map(|d| d.join(arg)).find(|p| p.is_file())
}

/// One-level include closure scan (design §5.3): for each `Include` directive in
/// `base_findings`, read the file keyd would include and scan it, keeping only the
/// `command(`/`macro(` findings (the footgun). **Non-recursive** — an included file's
/// own `include` lines are dead to keyd, so they're neither followed nor flagged.
/// Reads against the given search dirs; a missing / unreadable include is silently
/// skipped (keyd only warns on those, and `keyd check` is the real syntax gate).
pub fn scan_includes(
    base_findings: &[keydviz_apply::scan::Finding],
    config_dir: &Path,
    data_dir: &Path,
) -> Vec<IncludedFinding> {
    use keydviz_apply::scan::{scan, Finding};
    let mut out = Vec::new();
    for f in base_findings {
        let Finding::Include { arg, .. } = f else { continue };
        let Some(path) = resolve_include(arg, config_dir, data_dir) else { continue };
        let Ok(bytes) = std::fs::read(&path) else { continue };
        for inner in scan(&bytes).into_iter().filter(Finding::needs_ack) {
            out.push(IncludedFinding { include_arg: arg.clone(), file: path.clone(), finding: inner });
        }
    }
    out
}

/// How an apply run reaches the tool.
pub enum Invocation {
    /// Production: `pkexec /usr/bin/keydviz-apply --timeout N`. The absolute tool
    /// path must match the polkit policy's `exec.path` annotation.
    Pkexec { tool: PathBuf },
    /// Debug builds only: the sibling `target/debug/keydviz-apply` spawned
    /// directly with `--dev-dir` — the full protocol, no privilege, against a
    /// fake config dir (`$KEYDVIZ_APPLY_DEV_DIR`).
    Dev { tool: PathBuf, dir: PathBuf },
}

impl Invocation {
    /// The directory whose `<name>.conf` files one-click apply may target —
    /// the gate `EditSession::apply_target` checks against.
    pub fn config_dir(&self) -> &Path {
        match self {
            Invocation::Pkexec { .. } => prod_config_dir(),
            Invocation::Dev { dir, .. } => dir,
        }
    }
}

/// Detect whether one-click apply can work here, and how. `None` → the UI shows
/// draft-then-install only (AppImage / plain source build — a packaging
/// trade-off, not an error).
pub fn one_click() -> Option<Invocation> {
    // Dev escape hatch first (debug builds only): same precedent as
    // KEYDVIZ_HELPER_SOCKET. The dev tool sits next to our own binary.
    if cfg!(debug_assertions) {
        if let Ok(dir) = std::env::var("KEYDVIZ_APPLY_DEV_DIR") {
            let tool = std::env::current_exe().ok()?.parent()?.join("keydviz-apply");
            return tool
                .exists()
                .then(|| Invocation::Dev { tool, dir: PathBuf::from(dir) });
        }
    }
    let tool = PathBuf::from(APPLY_TOOL);
    (tool.exists() && Path::new(PKEXEC).exists()).then_some(Invocation::Pkexec { tool })
}

/// One protocol (or process-level) event, delivered in order on the caller's
/// callback. Exactly one *terminal* event ends every run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyEvent {
    /// `finding <desc>` — advisory scan echo (pre-flight already showed these).
    Finding(String),
    /// `error <reason>` — refused, nothing written (tool exit 2). Terminal.
    Refused(String),
    /// `applied <secs>` — written + reloaded; the dead-man window is open.
    Applied { secs: u64 },
    /// `kept` — the change is permanent (exit 0). Terminal.
    Kept,
    /// `reverted <reason>` — prior state restored + reloaded (exit 3). Terminal.
    Reverted(String),
    /// `revert-failed <why>` — LOUD: new config still live, prior not restored
    /// (exit 4). The message names the backup and the panic sequence. Terminal.
    RevertFailed(String),
    /// pkexec exit 126: the user dismissed the auth dialog. Terminal.
    AuthDismissed,
    /// pkexec exit 127: not authorized / no polkit agent in this session. Terminal.
    NotAuthorized,
    /// Spawn failure, EOF without a verdict, or an unexplained exit. Terminal.
    Failed(String),
}

impl ApplyEvent {
    /// Terminal events end the run; after one, no further events arrive.
    pub fn is_terminal(&self) -> bool {
        !matches!(self, ApplyEvent::Finding(_) | ApplyEvent::Applied { .. })
    }
}

/// Parse one protocol line. `None` for anything unrecognized — unknown lines are
/// skipped (forward-compat with a newer tool), and a run that ends without a
/// terminal line is mapped from the exit code instead.
pub fn parse_event(line: &str) -> Option<ApplyEvent> {
    if line == "kept" {
        return Some(ApplyEvent::Kept);
    }
    if let Some(d) = line.strip_prefix("finding ") {
        return Some(ApplyEvent::Finding(d.to_string()));
    }
    if let Some(r) = line.strip_prefix("error ") {
        return Some(ApplyEvent::Refused(r.to_string()));
    }
    if let Some(s) = line.strip_prefix("applied ") {
        return s.trim().parse().ok().map(|secs| ApplyEvent::Applied { secs });
    }
    if let Some(r) = line.strip_prefix("reverted ") {
        return Some(ApplyEvent::Reverted(r.to_string()));
    }
    if let Some(w) = line.strip_prefix("revert-failed ") {
        return Some(ApplyEvent::RevertFailed(w.to_string()));
    }
    None
}

/// Drain protocol lines from the tool's stdout, forwarding parsed events.
/// Returns whether a terminal event was seen (if not, the caller maps the
/// process exit code). Stops at the terminal event rather than EOF so a
/// misbehaving tool can't stall us after the verdict is in.
fn pump(out: impl BufRead, on_event: &(impl Fn(ApplyEvent) + ?Sized)) -> bool {
    for line in out.lines() {
        let Ok(line) = line else { break };
        if let Some(ev) = parse_event(&line) {
            let terminal = ev.is_terminal();
            on_event(ev);
            if terminal {
                return true;
            }
        }
    }
    false
}

/// Map a verdict-less exit to its event. pkexec owns 126 (auth dismissed) and
/// 127 (not authorized / no agent); the tool's own codes (0/2/3/4) are always
/// preceded by a protocol line, so reaching this with one of them means the
/// conversation broke — report it as such, never guess a verdict.
fn map_exit(code: Option<i32>) -> ApplyEvent {
    match code {
        Some(126) => ApplyEvent::AuthDismissed,
        Some(127) => ApplyEvent::NotAuthorized,
        Some(c) => ApplyEvent::Failed(format!("apply tool exited {c} without a verdict")),
        None => ApplyEvent::Failed("apply tool killed by a signal".to_string()),
    }
}

/// Which privileged operation a run performs. Both share the tool's reload +
/// dead-man's-switch tail and the same verdict events — they differ only in the
/// request line (and whether a payload follows).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ApplyOp {
    /// Write `<dir>/<name>.conf` with `bytes`.
    Apply,
    /// Remove `<dir>/<name>.conf` (no payload, no scan).
    Delete,
}

/// The request line, exactly as the tool's `read_line` expects it.
fn request_header(req: &ApplyRequest) -> String {
    match req.op {
        ApplyOp::Delete => format!("delete {}\n", req.name),
        ApplyOp::Apply if req.sensitive_ok => {
            format!("apply {} {} sensitive-ok\n", req.name, req.bytes.len())
        }
        ApplyOp::Apply => format!("apply {} {}\n", req.name, req.bytes.len()),
    }
}

/// Everything `start` needs. The caller has already gated (`apply_target`),
/// scanned (`keydviz_apply::scan`, `sensitive_ok` only after an explicit click),
/// size-checked, and `keyd check`ed the bytes. For [`ApplyOp::Delete`], `bytes` is
/// empty and `sensitive_ok` is ignored.
pub struct ApplyRequest {
    pub name: String,
    pub bytes: Vec<u8>,
    pub sensitive_ok: bool,
    pub op: ApplyOp,
    pub how: Invocation,
}

/// Live handle to a run: the GUI's half of the dead-man's switch.
pub struct ApplyHandle {
    stdin: Arc<Mutex<Option<ChildStdin>>>,
}

impl ApplyHandle {
    /// The user confirmed the keyboard works: send `keep`. Best-effort — the
    /// window may have just expired and the pipe closed (EPIPE); the authoritative
    /// outcome is whichever terminal event the tool emits. Only ever called in the
    /// `countdown` state, by which point the writer thread has long finished the
    /// request and released the lock, so this never contends.
    pub fn keep(&self) {
        if let Ok(mut g) = self.stdin.lock() {
            if let Some(w) = g.as_mut() {
                let _ = w.write_all(b"keep\n");
                let _ = w.flush();
            }
        }
    }

    /// The user backed out (or the UI is shutting down): drop our end of stdin.
    /// The tool sees EOF and reverts — the same path an outright GUI crash takes.
    ///
    /// `try_lock`, never `lock`: this is called from the UI thread, and during the
    /// `auth` state the writer thread can be *blocked inside the request write*
    /// holding the lock (a near-`MAX_CONFIG_BYTES` payload fills the pipe while
    /// pkexec's dialog stalls the reader). Blocking here would freeze the whole
    /// event loop until the dialog resolves. If the lock is held we skip closing
    /// stdin — the tool hasn't received the config yet (still pre-auth), and once
    /// the write completes the run proceeds to its normal countdown where revert
    /// works again; nothing persists without an explicit KEEP regardless.
    pub fn revert(&self) {
        if let Ok(mut g) = self.stdin.try_lock() {
            *g = None;
        }
    }
}

/// Spawn the tool and run the whole conversation on a background thread, calling
/// `on_event` for each event in order, ending with exactly one terminal event.
/// The caller hops events onto the UI thread (`slint::invoke_from_event_loop`),
/// same shape as `spawn_live`/`spawn_monitor`.
pub fn start(
    req: ApplyRequest,
    on_event: impl Fn(ApplyEvent) + Send + 'static,
) -> std::io::Result<ApplyHandle> {
    let mut cmd = match &req.how {
        Invocation::Pkexec { tool } => {
            let mut c = Command::new(PKEXEC);
            c.arg(tool);
            c
        }
        Invocation::Dev { tool, dir } => {
            let mut c = Command::new(tool);
            c.arg("--dev-dir").arg(dir);
            c
        }
    };
    let mut child: Child = cmd
        .arg("--timeout")
        .arg(TIMEOUT_SECS.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // stderr inherited: pkexec/polkit diagnostics land on our stderr.
        .spawn()?;

    let stdin = Arc::new(Mutex::new(child.stdin.take()));
    let stdout = child.stdout.take().expect("stdout was piped");
    let handle = ApplyHandle { stdin: Arc::clone(&stdin) };

    std::thread::spawn(move || {
        // Request write happens HERE, not before spawn returns: during the auth
        // dialog nobody reads the pipe, and header+payload can exceed the pipe
        // buffer — this write may block until pkexec authenticates (or dies).
        // The result is intentionally ignored: a failed write isn't terminal by
        // itself (pkexec may have refused before exec, or revert() already nulled
        // stdin — the tool then sees EOF), so we fall through to the pump and let
        // EOF + wait()'s exit code classify the outcome.
        if let Ok(mut g) = stdin.lock() {
            if let Some(w) = g.as_mut() {
                // Delete carries no payload; apply follows the header with its bytes.
                let payload: &[u8] = if req.op == ApplyOp::Apply { &req.bytes } else { &[] };
                let _ = w
                    .write_all(request_header(&req).as_bytes())
                    .and_then(|()| w.write_all(payload))
                    .and_then(|()| w.flush());
            }
        }

        let saw_verdict = pump(BufReader::new(stdout), &on_event);
        match child.wait() {
            Ok(status) if !saw_verdict => on_event(map_exit(status.code())),
            Err(e) if !saw_verdict => on_event(ApplyEvent::Failed(format!("wait: {e}"))),
            _ => {}
        }
    });
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_every_protocol_line() {
        assert_eq!(
            parse_event("finding line 3: command() runs as root"),
            Some(ApplyEvent::Finding("line 3: command() runs as root".into()))
        );
        assert_eq!(
            parse_event("error invalid config name"),
            Some(ApplyEvent::Refused("invalid config name".into()))
        );
        assert_eq!(parse_event("applied 30"), Some(ApplyEvent::Applied { secs: 30 }));
        assert_eq!(parse_event("kept"), Some(ApplyEvent::Kept));
        assert_eq!(
            parse_event("reverted TimedOut"),
            Some(ApplyEvent::Reverted("TimedOut".into()))
        );
        assert_eq!(
            parse_event("revert-failed rename: EACCES"),
            Some(ApplyEvent::RevertFailed("rename: EACCES".into()))
        );
        // Junk and malformed lines are skipped, never misread as a verdict.
        for junk in ["", "applied", "applied soon", "keptx", "ERROR x", "pkexec noise"] {
            assert_eq!(parse_event(junk), None, "{junk:?} must not parse");
        }
    }

    #[test]
    fn terminality_matches_the_protocol() {
        assert!(!ApplyEvent::Finding(String::new()).is_terminal());
        assert!(!ApplyEvent::Applied { secs: 1 }.is_terminal());
        for t in [
            ApplyEvent::Refused(String::new()),
            ApplyEvent::Kept,
            ApplyEvent::Reverted(String::new()),
            ApplyEvent::RevertFailed(String::new()),
            ApplyEvent::AuthDismissed,
            ApplyEvent::NotAuthorized,
            ApplyEvent::Failed(String::new()),
        ] {
            assert!(t.is_terminal(), "{t:?}");
        }
    }

    /// Run `pump` over a canned conversation, collecting events.
    fn play(script: &str) -> (Vec<ApplyEvent>, bool) {
        let got = std::sync::Mutex::new(Vec::new());
        let saw = pump(Cursor::new(script.as_bytes()), &|ev| got.lock().unwrap().push(ev));
        (got.into_inner().unwrap(), saw)
    }

    #[test]
    fn conversation_refused() {
        let (evs, saw) = play("finding x\nerror sensitive constructs need explicit confirmation\n");
        assert!(saw);
        assert_eq!(evs.len(), 2);
        assert!(matches!(&evs[1], ApplyEvent::Refused(r) if r.contains("sensitive")));
    }

    #[test]
    fn conversation_applied_then_kept() {
        let (evs, saw) = play("applied 30\nkept\n");
        assert!(saw);
        assert_eq!(evs, vec![ApplyEvent::Applied { secs: 30 }, ApplyEvent::Kept]);
    }

    #[test]
    fn conversation_applied_then_reverted() {
        let (evs, saw) = play("applied 30\nreverted Eof\n");
        assert!(saw);
        assert_eq!(evs[1], ApplyEvent::Reverted("Eof".into()));
    }

    #[test]
    fn conversation_revert_failed() {
        let (evs, saw) = play("applied 30\nrevert-failed rename: EROFS — the new config is still active\n");
        assert!(saw);
        assert!(matches!(&evs[1], ApplyEvent::RevertFailed(w) if w.contains("EROFS")));
    }

    #[test]
    fn pump_stops_at_the_verdict() {
        // Trailing garbage after a terminal line must not produce more events.
        let (evs, saw) = play("kept\nfinding late\n");
        assert!(saw);
        assert_eq!(evs, vec![ApplyEvent::Kept]);
    }

    #[test]
    fn eof_without_verdict_reports_unseen() {
        let (evs, saw) = play("finding x\napplied 30\n");
        assert!(!saw, "no terminal line ⇒ pump must say so");
        assert_eq!(evs.len(), 2);
        let (evs, saw) = play("");
        assert!(!saw);
        assert!(evs.is_empty());
    }

    #[test]
    fn exit_codes_map_to_pkexec_semantics() {
        assert_eq!(map_exit(Some(126)), ApplyEvent::AuthDismissed);
        assert_eq!(map_exit(Some(127)), ApplyEvent::NotAuthorized);
        assert!(matches!(map_exit(Some(2)), ApplyEvent::Failed(m) if m.contains("exited 2")));
        assert!(matches!(map_exit(None), ApplyEvent::Failed(m) if m.contains("signal")));
    }

    #[test]
    fn request_header_matches_the_tool_grammar() {
        let mk = |op, sensitive_ok, len: usize| ApplyRequest {
            name: "hhkb".into(),
            bytes: vec![0u8; len],
            sensitive_ok,
            op,
            how: Invocation::Pkexec { tool: PathBuf::from("/x") },
        };
        assert_eq!(request_header(&mk(ApplyOp::Apply, false, 120)), "apply hhkb 120\n");
        assert_eq!(
            request_header(&mk(ApplyOp::Apply, true, 120)),
            "apply hhkb 120 sensitive-ok\n"
        );
        // Delete: no length, no payload, sensitive-ok ignored.
        assert_eq!(request_header(&mk(ApplyOp::Delete, true, 999)), "delete hhkb\n");
    }

    // ---- one-level include closure scan (§5.3) ----

    use keydviz_apply::scan::scan;

    /// A throwaway unique dir under the temp dir (pid + seq, like the pre-flight
    /// check) holding two subdirs that play config-dir and DATA_DIR.
    struct Sandbox {
        root: PathBuf,
    }
    impl Sandbox {
        fn new() -> Sandbox {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root =
                std::env::temp_dir().join(format!("keyd-viz-inc-{}-{seq}", std::process::id()));
            std::fs::create_dir_all(root.join("conf")).unwrap();
            std::fs::create_dir_all(root.join("data")).unwrap();
            Sandbox { root }
        }
        fn conf_dir(&self) -> PathBuf {
            self.root.join("conf")
        }
        fn data_dir(&self) -> PathBuf {
            self.root.join("data")
        }
        fn write(&self, rel: &str, body: &str) {
            std::fs::write(self.root.join(rel), body).unwrap();
        }
        fn run(&self, config: &str) -> Vec<IncludedFinding> {
            let base = scan(config.as_bytes());
            scan_includes(&base, &self.conf_dir(), &self.data_dir())
        }
    }
    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn flags_command_inside_an_included_file() {
        let sb = Sandbox::new();
        sb.write("conf/common", "[main]\nx = command(rm -rf /)\n");
        let found = sb.run("include common\n[main]\na = b\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].include_arg, "common");
        assert!(found[0].file.ends_with("conf/common"));
        assert!(found[0].describe().contains("command line 2"), "{}", found[0].describe());
    }

    #[test]
    fn resolves_from_data_dir_when_absent_beside_config() {
        let sb = Sandbox::new();
        sb.write("data/shared", "[main]\nx = macro(C-a hi)\n");
        let found = sb.run("include shared\n");
        assert_eq!(found.len(), 1);
        assert!(found[0].file.ends_with("data/shared"));
        assert!(matches!(found[0].finding, keydviz_apply::scan::Finding::Macro { .. }));
    }

    #[test]
    fn config_dir_wins_over_data_dir() {
        let sb = Sandbox::new();
        sb.write("conf/dup", "[main]\nx = command(local)\n");
        sb.write("data/dup", "[main]\nx = command(shared)\n");
        let found = sb.run("include dup\n");
        assert_eq!(found.len(), 1);
        assert!(found[0].file.ends_with("conf/dup"));
    }

    #[test]
    fn clean_included_file_flags_nothing() {
        let sb = Sandbox::new();
        sb.write("conf/safe", "[main]\nx = y\ncapslock = esc\n");
        assert!(sb.run("include safe\n").is_empty());
    }

    #[test]
    fn missing_include_is_silently_skipped() {
        let sb = Sandbox::new();
        assert!(sb.run("include nope\n[main]\na = b\n").is_empty());
    }

    #[test]
    fn non_recursive_does_not_follow_includes_of_includes() {
        let sb = Sandbox::new();
        // `mid` only *includes* `deep`; it has no direct sensitive construct. keyd
        // wouldn't expand `mid`'s own include (non-recursive), so neither do we.
        sb.write("conf/mid", "include deep\n[main]\nx = y\n");
        sb.write("conf/deep", "[main]\nx = command(hidden)\n");
        assert!(sb.run("include mid\n").is_empty());
    }

    #[test]
    fn absolute_and_dotdot_includes_are_never_followed() {
        let sb = Sandbox::new();
        // These parse as SuspectInclude (not Include), so the closure scan ignores
        // them — keyd ignores them too. Even if a matching file existed, no finding.
        sb.write("conf/evil", "[main]\nx = command(x)\n");
        let base = scan(b"include /etc/evil\ninclude ../evil\n");
        assert!(scan_includes(&base, &sb.conf_dir(), &sb.data_dir()).is_empty());
    }
}
