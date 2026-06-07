//! `keydviz-apply` binary — one authenticated invocation, one apply (design §5.2).
//!
//! Protocol (stdin/stdout, line-oriented; stdin is the GUI's private pipe):
//!
//! ```text
//! GUI → tool   "apply <name> <len>[ sensitive-ok]\n"  then exactly <len> raw bytes
//! tool → GUI   "finding <desc>"        zero or more scan findings (advisory)
//!              "error <reason>"        refused; exits non-zero, nothing written
//!              "applied <secs>"        written + reloaded; dead-man window open
//! GUI → tool   "keep\n"                within <secs>, after the user confirms
//! tool → GUI   "kept" | "reverted <reason>"
//! ```
//!
//! The destination is always `/etc/keyd/<name>.conf` — the caller supplies a *name*
//! (strictly validated), never a path. A release build accepts no overrides; debug
//! builds take `--dev-dir`/`--dev-no-reload` so the revert flow can be demonstrated
//! end-to-end without root (E0 acceptance test).

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, SystemTime};

use keydviz_apply::scan::scan;
use keydviz_apply::txn::{self, Dir, WriteOp};
use keydviz_apply::{deadman, valid_name, MAX_CONFIG_BYTES};

/// Unbuffered reader on an inherited fd (stdin). Everything — request line,
/// payload, and the dead-man's `keep` — must go through ONE unbuffered reader:
/// std's buffered `StdinLock` could slurp the later `keep\n` into a userspace
/// buffer where the dead-man's raw-fd `poll` would never see it (and a second
/// `stdin.lock()` would deadlock on std's non-reentrant mutex).
struct Fd(RawFd);

impl AsRawFd for Fd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

impl Read for Fd {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let n = unsafe { libc::read(self.0, buf.as_mut_ptr().cast(), buf.len()) };
            if n >= 0 {
                return Ok(n as usize);
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}

const KEYD_DIR: &str = "/etc/keyd";
const DEFAULT_TIMEOUT_SECS: u64 = 20;

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
    println!("error {reason}");
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
    let mut input = Fd(libc::STDIN_FILENO);

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
        println!("finding {}", f.describe());
    }
    if findings.iter().any(|f| f.needs_ack()) && !sensitive_ok {
        return Ok(refuse("sensitive constructs need explicit confirmation"));
    }

    // ---- validate environment (never fall open, §5.3) ----------------------------
    if !keyd_check_works() {
        return Ok(refuse("keyd check unavailable; refusing to persist"));
    }

    // ---- transactional write + keyd check + reload -------------------------------
    let dir = Dir::open(&opts.dir)?;
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let ops = vec![WriteOp { name: format!("{name}.conf"), content }];
    let mut txn = txn::apply(&dir, ops, stamp, keyd_check)?;

    if opts.reload {
        if let Err(e) = keyd_reload() {
            txn.revert()?;
            let _ = reload_best_effort(opts);
            println!("reverted reload-failed: {e}");
            return Ok(ExitCode::from(3));
        }
    }

    // ---- dead-man's switch (§5.4) -------------------------------------------------
    println!("applied {}", opts.timeout.as_secs());
    io::stdout().flush()?;
    let verdict = deadman::await_keep(&input, opts.timeout);
    if verdict == deadman::Verdict::Keep {
        println!("kept");
        return Ok(ExitCode::SUCCESS);
    }
    txn.revert()?;
    let _ = reload_best_effort(opts);
    println!("reverted {verdict:?}");
    Ok(ExitCode::from(3))
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

/// Prove `keyd check` itself works before trusting it as the syntax gate — a keyd
/// without the subcommand fails *closed* here, not open at apply time.
fn keyd_check_works() -> bool {
    Command::new("keyd")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// `keyd check` on the temp file holding the exact bytes about to go live.
fn keyd_check(path: &Path) -> io::Result<()> {
    let out = Command::new("keyd").arg("check").arg(path).output()?;
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

fn keyd_reload() -> io::Result<()> {
    let out = Command::new("keyd").arg("reload").output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(String::from_utf8_lossy(&out.stderr).trim().to_string()))
    }
}

/// Reload after a revert: best effort — the prior config was live moments ago, and
/// if even reload fails the panic sequence is the documented way out.
fn reload_best_effort(opts: &Opts) -> io::Result<()> {
    if opts.reload {
        keyd_reload()
    } else {
        Ok(())
    }
}
