//! Connect-time authorization for the broker socket.
//!
//! The daemon serves a one-directional event stream, but it still must decide *who*
//! may receive it — keypress glow in particular is sensitive. Authz is keyed off the
//! peer uid, which the kernel attests via `SO_PEERCRED` (see [`crate::peer_uid`]) and
//! the client cannot forge.
//!
//! Two policies:
//!
//! - [`Policy::Uid`] — serve exactly one uid. This is the dev / same-user path: the
//!   daemon and GUI run as the same person, no logind needed. It's the default.
//! - [`Policy::ActiveSession`] — serve whoever logind reports as owning an *active*
//!   session (the foreground user on the graphical seat). This is what lets the daemon
//!   run as the dedicated `keyd-viz` system user yet still serve the desktop user, with
//!   no shared group and no hard-coded uid. A user who has switched away (state
//!   `online`, not `active`) is denied, so a background user's GUI can't pull the
//!   foreground user's keystrokes.
//!
//! `ActiveSession` asks libsystemd's `sd_uid_get_state` rather than parsing
//! `/run/systemd/users/<uid>` directly — that file is explicitly marked "do not parse",
//! whereas the function is the stable API over the same data (no D-Bus round trip, no
//! exec — just a couple of file reads under the hood, which stays inside the sandbox).

use std::ffi::CStr;
use std::os::raw::{c_char, c_int};

/// Which clients the daemon will serve. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Serve only this exact peer uid (dev / same-user).
    Uid(u32),
    /// Serve any peer uid logind reports as the active (foreground) session user.
    ActiveSession,
}

/// Outcome of an authz decision, carrying a reason for the deny-path log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Denied, with a short human-readable reason.
    Deny(&'static str),
}

impl Policy {
    /// Decide whether a client with this kernel-attested `uid` may be served.
    pub fn decide(&self, uid: u32) -> Decision {
        match self {
            Policy::Uid(want) if uid == *want => Decision::Allow,
            Policy::Uid(_) => Decision::Deny("uid not permitted"),
            Policy::ActiveSession => match uid_state(uid).as_deref() {
                Some("active") => Decision::Allow,
                Some(_) => Decision::Deny("not the active session user"),
                None => Decision::Deny("no logind session"),
            },
        }
    }

    /// Socket mode this policy needs. [`Policy::Uid`] locks the socket to its owner
    /// (`0600`); [`Policy::ActiveSession`] must let an *unrelated* desktop uid `connect`,
    /// so the node is world-connectable (`0666`) and the [`decide`](Self::decide) check —
    /// not the file mode — is what gates the data. A rejected peer gets zero bytes.
    pub fn socket_mode(&self) -> u32 {
        match self {
            Policy::Uid(_) => 0o600,
            Policy::ActiveSession => 0o666,
        }
    }
}

extern "C" {
    // int sd_uid_get_state(uid_t uid, char **state);  -- libsystemd, LIBSYSTEMD_209.
    // Returns >= 0 on success with a malloc'd state string in *state; < 0 (negative
    // errno) on failure.
    fn sd_uid_get_state(uid: u32, state: *mut *mut c_char) -> c_int;
}

/// logind's session state for `uid` ("active", "online", "closing", "lingering",
/// "offline"), or `None` if the lookup fails. Thin safe wrapper over `sd_uid_get_state`.
fn uid_state(uid: u32) -> Option<String> {
    let mut ptr: *mut c_char = std::ptr::null_mut();
    // SAFETY: sd_uid_get_state writes a single malloc'd, NUL-terminated C string into
    // `ptr` on success and leaves it null on failure. We read it through CStr only when
    // ret >= 0 and ptr is non-null, copy it to an owned String, then free the original
    // with the matching allocator (libc free, since libsystemd allocated it with malloc).
    let ret = unsafe { sd_uid_get_state(uid, &mut ptr) };
    if ret < 0 || ptr.is_null() {
        if !ptr.is_null() {
            unsafe { libc::free(ptr as *mut libc::c_void) };
        }
        return None;
    }
    let state = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
    unsafe { libc::free(ptr as *mut libc::c_void) };
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_policy_matches_only_its_uid() {
        assert_eq!(Policy::Uid(1000).decide(1000), Decision::Allow);
        assert_eq!(Policy::Uid(1000).decide(1001), Decision::Deny("uid not permitted"));
        assert_eq!(Policy::Uid(1000).decide(0), Decision::Deny("uid not permitted"));
    }

    #[test]
    fn socket_mode_follows_policy() {
        assert_eq!(Policy::Uid(1000).socket_mode(), 0o600);
        assert_eq!(Policy::ActiveSession.socket_mode(), 0o666);
    }

    #[test]
    fn nonexistent_uid_has_no_active_session() {
        // A uid with no logind session must never be reported active (logind returns
        // "offline" for an unknown uid, or the lookup yields None) — either way, denied.
        assert_ne!(uid_state(4_000_000_000).as_deref(), Some("active"));
        assert!(matches!(Policy::ActiveSession.decide(4_000_000_000), Decision::Deny(_)));
    }
}
