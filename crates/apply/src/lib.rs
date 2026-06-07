//! `keydviz-apply` — the transient privileged apply tool (Phase 6 E0 prototype,
//! design doc §5.2–§5.4).
//!
//! The only privileged path in keyd-viz: a one-shot tool, invoked via polkit/pkexec
//! for *persist only*, alive for exactly one apply. The long-lived helper never gains
//! write capability; the GUI never runs privileged. Hard rules, from the design doc:
//!
//! - **No caller-supplied paths.** The candidate config arrives on stdin; the
//!   destination is `/etc/keyd/<name>.conf` where `<name>` must match
//!   `^[A-Za-z0-9_-]+$` ([`valid_name`]) — no traversal, no arbitrary-`/etc` writes.
//! - **No symlink / TOCTOU games** ([`txn`]): dir-fd on `/etc/keyd`, temp files
//!   written `O_CREAT|O_EXCL|O_NOFOLLOW`, `keyd check` runs on the **exact bytes
//!   just written**, then `renameat` into place. Priors are backed up first.
//! - **Byte-level safety scan** ([`scan`]): `command(` is root code-exec and
//!   `macro(` is keystroke injection — both require the caller's explicit,
//!   unmistakable acknowledgement; `include` is advisory (keyd confines includes to
//!   root-owned dirs — review #2). The scan runs on the final serialized bytes and
//!   does not trust any model.
//! - **Dead-man's-switch revert** ([`deadman`]): after write + reload the tool
//!   blocks for a positive "keep"; timeout, EOF, or anything else reverts the
//!   write-set and reloads again. Revert authority lives *here* (the privileged
//!   process), because the GUI can't write `/etc/keyd`; the absence of "keep" is
//!   safe. The keyd panic sequence (Backspace+Escape+Enter) remains the primary
//!   failsafe above all of this.
//! - **Never fall open.** A missing `keyd check`, an unvalidatable environment, an
//!   error anywhere → refuse / revert, never proceed.
//!
//! The write-set interface is transactional (`(name, prior: Existed|Absent)`,
//! all-or-nothing revert) per §5.4 — the MVP only ever passes a single write, but
//! E2+ structural ops (create/split/move) get the right shape for free.

pub mod deadman;
pub mod fdio;
pub mod scan;
pub mod txn;

/// Maximum accepted config size. keyd itself caps a config at 65536 bytes
/// (`MAX_FILE_SIZE`, config.c) — anything bigger can't be a working config.
pub const MAX_CONFIG_BYTES: usize = 65536;

/// Validate a destination config name (the `<name>` of `/etc/keyd/<name>.conf`).
/// Strict allow-list per design doc §5.2: ASCII alphanumerics, `_`, `-` only —
/// which structurally rules out `../cron.d/x`-style traversal, hidden files, and
/// our own temp/backup prefixes.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_allow_list() {
        for good in ["default", "hhkb", "my-board_2"] {
            assert!(valid_name(good), "{good:?} should be valid");
        }
        for bad in [
            "",
            ".",
            "..",
            "../cron.d/x",
            "a/b",
            "a.conf",
            ".hidden",
            "a b",
            "ümlaut",
            "x\0",
            &"a".repeat(65),
        ] {
            assert!(!valid_name(bad), "{bad:?} should be rejected");
        }
    }
}
