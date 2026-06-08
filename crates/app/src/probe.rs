//! Runtime keyd capability probe (Phase 6 E0, design doc §6 item 3).
//!
//! Edit Mode's facts about keyd are version-dependent (`keyd check` exists, what
//! `list-keys` prints, where the socket lives), so we probe the installed keyd at
//! runtime instead of assuming the version the design was verified against. Nothing
//! here needs privilege; run it lazily when edit mode is entered, not at startup.
//!
//! Fail-closed posture (§5.3): a probe that can't *positively confirm* a capability
//! reports it absent. The privileged apply tool re-runs `keyd check` itself on the
//! exact bytes it writes — this probe only gates UX (e.g. explaining *why* apply is
//! unavailable), it is never the security check.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A config that any keyd able to validate at all must accept — used to prove
/// `keyd check` works (exit 0 here ⇒ the subcommand exists and validates).
const KNOWN_GOOD: &str = "[ids]\n*\n[main]\n";

/// What the installed keyd can do for us.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeydProbe {
    /// `keyd --version` output, trimmed (e.g. `keyd v2.6.0 ()`). `None`: no keyd.
    pub version: Option<String>,
    /// `keyd check` exists and validated a known-good config (exit 0).
    pub check: bool,
    /// Valid key names from `keyd list-keys`, for the picker. Empty if unavailable.
    pub keys: Vec<String>,
    /// keyd's control socket, if present (`/run/keyd.socket`, older `/var/run/...`).
    pub socket: Option<PathBuf>,
}

impl KeydProbe {
    /// Probe the installed keyd. Each capability degrades independently — a partial
    /// keyd (in PATH but no daemon running, say) yields a partial probe.
    pub fn run() -> KeydProbe {
        KeydProbe {
            version: version(),
            check: check_works(),
            keys: list_keys(),
            socket: socket_path(),
        }
    }

    /// One-line human summary for `--probe` / diagnostics.
    pub fn summary(&self) -> String {
        format!(
            "version: {}  check: {}  list-keys: {} names  socket: {}",
            self.version.as_deref().unwrap_or("(no keyd in PATH)"),
            if self.check { "ok" } else { "UNAVAILABLE" },
            self.keys.len(),
            self.socket.as_deref().map_or_else(|| "(none)".into(), |p| p.display().to_string()),
        )
    }
}

fn version() -> Option<String> {
    let out = Command::new("keyd").arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!v.is_empty()).then_some(v)
}

/// Prove `keyd check` works by validating a known-good config (verified on keyd
/// v2.6.0: exit 0 on valid, 255 on invalid). Any failure — keyd missing, no `check`
/// subcommand, tempfile trouble — reads as "unavailable", never as "fine".
fn check_works() -> bool {
    // pid + sequence: concurrent probes in one process (parallel tests) must not
    // share a temp file — one's cleanup would race the other's `keyd check` read.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir()
        .join(format!("keyd-viz-probe-{}-{seq}.conf", std::process::id()));
    let ok = std::fs::write(&path, KNOWN_GOOD).is_ok()
        && Command::new("keyd")
            .arg("check")
            .arg(&path)
            .output()
            .is_ok_and(|out| out.status.success());
    let _ = std::fs::remove_file(&path);
    ok
}

fn list_keys() -> Vec<String> {
    let Ok(out) = Command::new("keyd").arg("list-keys").output() else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn socket_path() -> Option<PathBuf> {
    ["/run/keyd.socket", "/var/run/keyd.socket"]
        .iter()
        .map(Path::new)
        .find(|p| p.exists())
        .map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keyd_installed() -> bool {
        Command::new("keyd").arg("--version").output().is_ok_and(|o| o.status.success())
    }

    /// On a box with keyd installed the probe must find real capabilities; without
    /// keyd it must degrade to all-absent (fail closed) — both asserted, so this
    /// test is meaningful locally *and* hermetic in CI.
    #[test]
    fn probe_matches_environment() {
        let probe = KeydProbe::run();
        if keyd_installed() {
            assert!(probe.version.is_some());
            assert!(probe.check, "keyd present but `keyd check` probe failed");
            assert!(!probe.keys.is_empty(), "keyd present but list-keys empty");
            // `esc` exists in every keyd vocabulary.
            assert!(probe.keys.iter().any(|k| k == "esc"));
        } else {
            assert_eq!(probe, KeydProbe::default());
        }
    }

    #[test]
    fn summary_is_one_line() {
        assert!(!KeydProbe::run().summary().contains('\n'));
    }
}
