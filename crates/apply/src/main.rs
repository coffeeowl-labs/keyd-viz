//! `keydviz-apply` binary — one authenticated invocation, one apply (design §5.2).
//!
//! Protocol (stdin/stdout, line-oriented; stdin is the GUI's private pipe):
//!
//! ```text
//! GUI → tool   "apply <name> <len>[ sensitive-ok]\n"  then exactly <len> raw bytes
//! tool → GUI   "finding <desc>"        zero or more scan findings (advisory)
//!              "error <reason>"        refused; exits 2, nothing written
//!              "applied <secs>"        written + reloaded; dead-man window open
//! GUI → tool   "keep\n"                within <secs>, after the user confirms
//! tool → GUI   "kept"                  exit 0
//!              "reverted <reason>"     exit 3 — prior state restored + reloaded
//!              "revert-failed <why>"   exit 4 — LOUD: the new config is still
//!                                      live and the prior could not be restored;
//!                                      recover from the timestamped backup, or
//!                                      Backspace+Escape+Enter to kill keyd
//! ```
//!
//! The destination is always `/etc/keyd/<name>.conf` — the caller supplies a *name*
//! (strictly validated), never a path. `keyd` itself is invoked by **absolute path
//! only** (root-owned locations; never a PATH lookup in a root process). All reads
//! sit behind a deadline and all protocol writes are panic-proof ([`fdio`]); the
//! transaction additionally reverts on any unexpected unwind (`Txn`'s drop
//! backstop). A release build accepts no overrides; debug builds take
//! `--dev-dir`/`--dev-no-reload` so the revert flow can be demonstrated end-to-end
//! without root (E0 acceptance test).

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, SystemTime};

use keydviz_apply::fdio::{say, FdReader};
use keydviz_apply::scan::scan;
use keydviz_apply::txn::{self, Dir, Txn, WriteOp};
use keydviz_apply::{deadman, valid_name, MAX_CONFIG_BYTES};

const KEYD_DIR: &str = "/etc/keyd";
const DEFAULT_TIMEOUT_SECS: u64 = 20;
/// Deadline for the client to deliver the complete request line + payload. A
/// stalled client must not wedge a root process (review finding F2).
const REQUEST_TIMEOUT_SECS: u64 = 30;
/// Root-owned locations keyd installs to. Never a PATH lookup: a root process
/// resolving `keyd` through a caller-influenced PATH would hand out root exec
/// AND let a fake `keyd check` defeat the syntax gate (review finding #5).
const KEYD_PATHS: [&str; 3] = ["/usr/bin/keyd", "/usr/local/bin/keyd", "/usr/sbin/keyd"];

/// The config any keyd able to validate at all must accept — proves `keyd check`
/// itself works before it is trusted as the syntax gate (fail closed; review
/// finding #4). Mirrors `app::probe` (which can't be shared: this crate is
/// deliberately libc-only).
const KNOWN_GOOD: &str = "[ids]\n*\n[main]\n";

struct Opts {
    dir: PathBuf,
    reload: bool,
    timeout: Duration,
}

fn main() -> ExitCode {
    let opts = match parse_opts() {
        Ok(o) => o,
        Err(e) => return refuse(&e),
    };
    match run(&opts) {
        Ok(code) => code,
        Err(e) => refuse(&e.to_string()),
    }
}

fn refuse(reason: &str) -> ExitCode {
    say(&format!("error {reason}"));
    ExitCode::from(2)
}

fn parse_opts() -> Result<Opts, String> {
    let mut opts = Opts {
        dir: PathBuf::from(KEYD_DIR),
        reload: true,
        timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--timeout" => {
                let v = args.next().ok_or("--timeout needs seconds")?;
                let secs: u64 = v.parse().map_err(|_| "bad --timeout value".to_string())?;
                // Clamp: the window must be long enough to physically reach the
                // keyboard, short enough that a wedged session self-heals.
                opts.timeout = Duration::from_secs(secs.clamp(5, 120));
            }
            // Dev-only escape hatches for the unprivileged E0 demo. Compiled out
            // of release builds — production has no caller-supplied paths, ever.
            "--dev-dir" if cfg!(debug_assertions) => {
                opts.dir = PathBuf::from(args.next().ok_or("--dev-dir needs a path")?);
            }
            "--dev-no-reload" if cfg!(debug_assertions) => opts.reload = false,
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(opts)
}

fn run(opts: &Opts) -> io::Result<ExitCode> {
    // One unbuffered, deadline-bearing reader for the whole conversation: request
    // line, payload, and (via its raw fd) the dead-man's `keep`.
    let mut input =
        FdReader::new(libc::STDIN_FILENO, Duration::from_secs(REQUEST_TIMEOUT_SECS));

    // ---- request line + payload ------------------------------------------------
    let req = read_line(&mut input)?;
    let mut parts = req.split_whitespace();
    let (cmd, name) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let len: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
    let sensitive_ok = parts.next() == Some("sensitive-ok");

    if cmd != "apply" {
        return Ok(refuse("expected: apply <name> <len> [sensitive-ok]"));
    }
    if !valid_name(name) {
        return Ok(refuse("invalid config name"));
    }
    if len > MAX_CONFIG_BYTES {
        return Ok(refuse("config too large"));
    }
    let mut content = vec![0u8; len];
    input.read_exact(&mut content)?;

    // ---- safety scan on the exact bytes (§5.3) ----------------------------------
    let findings = scan(&content);
    for f in &findings {
        say(&format!("finding {}", f.describe()));
    }
    if findings.iter().any(|f| f.needs_ack()) && !sensitive_ok {
        return Ok(refuse("sensitive constructs need explicit confirmation"));
    }

    // ---- validate environment (never fall open, §5.3) ----------------------------
    let Some(keyd) = keyd_bin() else {
        return Ok(refuse("keyd binary not found in any system location"));
    };
    if !keyd_check_works(keyd) {
        return Ok(refuse("keyd check unavailable; refusing to persist"));
    }

    // ---- transactional write + keyd check + reload -------------------------------
    let dir = Dir::open(&opts.dir)?;
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let ops = vec![WriteOp { name: format!("{name}.conf"), content }];
    let txn = txn::apply(&dir, ops, stamp, |p| keyd_check(keyd, p))?;

    if opts.reload {
        if let Err(e) = keyd_reload(keyd) {
            return Ok(revert_and_report(txn, opts, &format!("reload-failed: {e}")));
        }
    }

    // ---- dead-man's switch (§5.4) -------------------------------------------------
    say(&format!("applied {}", opts.timeout.as_secs()));
    let verdict = deadman::await_keep(&input, opts.timeout);
    if verdict == deadman::Verdict::Keep {
        txn.keep();
        say("kept");
        return Ok(ExitCode::SUCCESS);
    }
    Ok(revert_and_report(txn, opts, &format!("{verdict:?}")))
}

/// Revert the transaction and tell the GUI what happened. A revert *failure* is
/// the one outcome that must never masquerade as anything else (review finding
/// F3): the new config is still live and the prior couldn't be restored, so emit
/// the distinct `revert-failed` line (exit 4) with the recovery instructions —
/// and do NOT reload (that would only re-assert the config we failed to remove).
fn revert_and_report(mut txn: Txn<'_>, opts: &Opts, reason: &str) -> ExitCode {
    match txn.revert() {
        Ok(()) => {
            // Best-effort reload of the restored files; if keyd itself is broken
            // the panic sequence remains (documented, §5.4).
            if opts.reload {
                if let Some(keyd) = keyd_bin() {
                    let _ = keyd_reload(keyd);
                }
            }
            say(&format!("reverted {reason}"));
            ExitCode::from(3)
        }
        Err(e) => {
            say(&format!(
                "revert-failed {e} — the new config is still active; restore the \
                 .keydviz-bak backup in the config directory, or press \
                 Backspace+Escape+Enter to terminate keyd"
            ));
            ExitCode::from(4)
        }
    }
}

fn read_line(r: &mut impl Read) -> io::Result<String> {
    // Byte-at-a-time so we never consume payload bytes past the '\n'.
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    loop {
        r.read_exact(&mut b)?;
        if b[0] == b'\n' {
            return Ok(String::from_utf8_lossy(&line).into_owned());
        }
        line.push(b[0]);
        if line.len() > 256 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "request line too long"));
        }
    }
}

/// The installed keyd, by absolute path only.
fn keyd_bin() -> Option<&'static Path> {
    KEYD_PATHS.iter().map(Path::new).find(|p| p.exists())
}

/// Prove `keyd check` actually validates: write a known-good config to a private
/// temp file and require exit 0. `--version` succeeding is NOT enough — an old
/// keyd without the subcommand must fail closed *here*, before anything is
/// written (review finding #4).
fn keyd_check_works(keyd: &Path) -> bool {
    let path = std::env::temp_dir().join(format!("keydviz-apply-probe-{}.conf", std::process::id()));
    let ok = std::fs::write(&path, KNOWN_GOOD).is_ok() && keyd_check(keyd, &path).is_ok();
    let _ = std::fs::remove_file(&path);
    ok
}

/// `keyd check` on the temp file holding the exact bytes about to go live.
fn keyd_check(keyd: &Path, path: &Path) -> io::Result<()> {
    let out = Command::new(keyd).arg("check").arg(path).output()?;
    if out.status.success() {
        Ok(())
    } else {
        // keyd check prints its diagnostics on stdout (verified v2.6.0).
        let err = String::from_utf8_lossy(&out.stderr);
        let out_ = String::from_utf8_lossy(&out.stdout);
        let detail = if err.trim().is_empty() { out_ } else { err };
        Err(io::Error::other(format!(
            "keyd check rejected the config: {}",
            detail.trim().replace('\n', " | ")
        )))
    }
}

fn keyd_reload(keyd: &Path) -> io::Result<()> {
    let out = Command::new(keyd).arg("reload").output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}
