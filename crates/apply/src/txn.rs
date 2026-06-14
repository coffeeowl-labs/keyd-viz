//! Transactional, symlink-safe write-set for `/etc/keyd` (design doc §5.2, §5.4).
//!
//! All file operations go through a dir-fd opened once on the target directory, so
//! no step re-walks the path (no TOCTOU on the directory itself), and every file
//! open is `O_NOFOLLOW` (a symlink planted at any name we touch aborts the apply
//! rather than redirecting it). New content lands in a `O_CREAT|O_EXCL` temp file
//! in the same directory — validated there by the caller (`keyd check` on the exact
//! bytes) — then `renameat`s into place atomically. (`rename` replaces the
//! destination *entry* without following it, so a symlink at the destination gets
//! replaced, never traversed.)
//!
//! The write-set is all-or-nothing: every target's prior state
//! ([`Prior::Existed`]/[`Prior::Absent`]) is captured (and existing bytes backed up
//! to a timestamped sibling) before anything is renamed; [`Txn::revert`] restores
//! priors in reverse order — restore bytes, or *delete* a file we created. The MVP
//! only ever passes one write; the interface is sized for E2+ structural ops.

use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};

/// A directory opened by fd; all operations are `*at`-relative to it.
#[derive(Debug)]
pub struct Dir {
    fd: File, // owns the O_DIRECTORY fd
    path: PathBuf,
}

/// What existed at a target name before we touched it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prior {
    Existed(Vec<u8>),
    Absent,
}

/// One requested write: `name` is the full file name (`hhkb.conf`), pre-validated
/// by the caller — never caller-supplied path material.
pub struct WriteOp {
    pub name: String,
    pub content: Vec<u8>,
}

/// An applied (renamed-into-place) write, with what it replaced.
#[derive(Debug)]
pub struct Applied {
    pub name: String,
    pub prior: Prior,
    /// Name of the timestamped backup holding the prior bytes, if any existed.
    pub backup: Option<String>,
}

/// A fully applied write-set, ready to be kept or reverted.
#[derive(Debug)]
pub struct Txn<'d> {
    dir: &'d Dir,
    pub applied: Vec<Applied>,
}

impl Dir {
    /// Open the target directory itself `O_NOFOLLOW` (the path must be a real
    /// directory, not a symlink to one).
    pub fn open(path: &Path) -> io::Result<Dir> {
        let c = cstr(path.as_os_str().as_encoded_bytes())?;
        let fd = unsafe {
            libc::open(
                c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Dir { fd: unsafe { File::from_raw_fd(fd) }, path: path.to_path_buf() })
    }

    /// Absolute path of a name inside this directory (for `keyd check <path>` and
    /// diagnostics only — file I/O always goes through the fd).
    pub fn display_path(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    fn openat(&self, name: &str, flags: libc::c_int, mode: libc::c_int) -> io::Result<File> {
        let c = cstr(name.as_bytes())?;
        let fd = unsafe {
            libc::openat(
                self.fd.as_raw_fd(),
                c.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                mode,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    /// Read a file's bytes, `None` if absent. A symlink at `name` is an error
    /// (`ELOOP`), never followed.
    pub fn read(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        match self.openat(name, libc::O_RDONLY, 0) {
            Ok(mut f) => {
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;
                Ok(Some(buf))
            }
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Create `name` exclusively (fails if anything — including a symlink — already
    /// sits there), write `bytes`, fsync.
    pub fn write_new(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let mut f =
            self.openat(name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o644)?;
        f.write_all(bytes)?;
        f.sync_all()
    }

    pub fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        let (cf, ct) = (cstr(from.as_bytes())?, cstr(to.as_bytes())?);
        let rc = unsafe {
            libc::renameat(self.fd.as_raw_fd(), cf.as_ptr(), self.fd.as_raw_fd(), ct.as_ptr())
        };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        self.sync()
    }

    pub fn unlink(&self, name: &str) -> io::Result<()> {
        let c = cstr(name.as_bytes())?;
        let rc = unsafe { libc::unlinkat(self.fd.as_raw_fd(), c.as_ptr(), 0) };
        if rc < 0 {
            return Err(io::Error::last_os_error());
        }
        self.sync()
    }

    /// fsync the directory so renames/unlinks are durable before we report them.
    fn sync(&self) -> io::Result<()> {
        self.fd.sync_all()
    }

    /// The plain file names directly inside this directory (non-recursive). Used by
    /// [`prune_backups`] to find timestamped backups. Reads via the stored path: the
    /// directory is fixed and root-owned (this only runs in the privileged tool), and
    /// enumeration needs no symlink hardening — every name found is re-opened through
    /// the `*at` fd (with `O_NOFOLLOW`) before it is ever unlinked.
    pub fn entries(&self) -> io::Result<Vec<String>> {
        let mut out = Vec::new();
        for ent in std::fs::read_dir(&self.path)? {
            if let Some(n) = ent?.file_name().to_str() {
                out.push(n.to_string());
            }
        }
        Ok(out)
    }
}

/// Best-effort retention: keep only the `keep` newest timestamped backups of `name`
/// (the full config file name, e.g. `hhkb.conf`), unlinking the rest. Matches the
/// `.{name}.keydviz-bak.{stamp}.{pid}` scheme *strictly* — both `{stamp}` and
/// `{pid}` must be all-digits and be the only two trailing dot-parts — so a
/// hand-named or unrelated file can never be selected, and the live config (no
/// `.keydviz-bak.` marker, no leading dot) never matches. Pure housekeeping: every
/// error is swallowed, because a failed prune must never turn an otherwise-kept
/// apply into a failure.
pub fn prune_backups(dir: &Dir, name: &str, keep: usize) {
    let prefix = format!(".{name}.keydviz-bak.");
    let Ok(entries) = dir.entries() else { return };
    let mut baks: Vec<(String, u64)> = entries
        .into_iter()
        .filter_map(|f| {
            let rest = f.strip_prefix(&prefix)?; // expect exactly "{stamp}.{pid}"
            let mut it = rest.split('.');
            let stamp = it.next()?;
            let pid = it.next()?;
            // Exactly two parts, both non-empty digit runs — nothing else qualifies.
            if it.next().is_some() || stamp.is_empty() || pid.is_empty() {
                return None;
            }
            if !pid.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            Some((f.clone(), stamp.parse::<u64>().ok()?))
        })
        .collect();
    baks.sort_by(|a, b| b.1.cmp(&a.1)); // newest first
    for (fname, _) in baks.into_iter().skip(keep) {
        let _ = dir.unlink(&fname);
    }
}

/// Apply a write-set: for each op, capture the prior (backing up existing bytes to
/// `.{name}.keydviz-bak.{stamp}`), write the new content to a temp file, let
/// `check` validate the temp (the *exact bytes* about to go live — `keyd check` in
/// production), then rename into place. Any failure reverts everything already
/// applied and returns the error.
pub fn apply(
    dir: &Dir,
    ops: Vec<WriteOp>,
    stamp: u64,
    check: impl Fn(&Path) -> io::Result<()>,
) -> io::Result<Txn<'_>> {
    let mut txn = Txn { dir, applied: Vec::new() };
    for op in ops {
        if let Err(e) = apply_one(dir, &op, stamp, &check, &mut txn.applied) {
            let _ = txn.revert();
            return Err(e);
        }
    }
    Ok(txn)
}

fn apply_one(
    dir: &Dir,
    op: &WriteOp,
    stamp: u64,
    check: &impl Fn(&Path) -> io::Result<()>,
    applied: &mut Vec<Applied>,
) -> io::Result<()> {
    let prior = match dir.read(&op.name)? {
        Some(bytes) => Prior::Existed(bytes),
        None => Prior::Absent,
    };

    // Temp + check first, backup second: a config the checker rejects must leave
    // zero debris, not an orphan backup.
    let tmp = format!(".{}.keydviz-tmp.{}", op.name, std::process::id());
    dir.write_new(&tmp, &op.content)?;
    if let Err(e) = check(&dir.display_path(&tmp)) {
        let _ = dir.unlink(&tmp);
        return Err(e);
    }

    let backup = match &prior {
        Prior::Existed(bytes) => {
            // stamp + pid: unique across concurrent invocations within one second.
            let bak = format!(".{}.keydviz-bak.{stamp}.{}", op.name, std::process::id());
            if let Err(e) = dir.write_new(&bak, bytes) {
                let _ = dir.unlink(&tmp);
                return Err(e);
            }
            Some(bak)
        }
        Prior::Absent => None,
    };

    dir.rename(&tmp, &op.name)?;

    applied.push(Applied { name: op.name.clone(), prior, backup });
    Ok(())
}

/// Transactionally delete `name`: capture its prior bytes (backed up to a
/// timestamped sibling, same scheme as [`apply`]), then unlink it. The returned
/// [`Txn`] reverts by recreating the file from the captured prior — so a delete is
/// just an `Existed → Absent` transition in the same write-set model, and the drop
/// backstop / dead-man's switch protect it identically. Errors (touching nothing
/// persisted) if the config is absent — there is nothing to delete — or if `name`
/// is a symlink (`O_NOFOLLOW` read aborts rather than deleting through it).
pub fn delete<'d>(dir: &'d Dir, name: &str, stamp: u64) -> io::Result<Txn<'d>> {
    let Some(bytes) = dir.read(name)? else {
        return Err(io::Error::new(io::ErrorKind::NotFound, "config does not exist"));
    };
    // Back up before unlinking; if the unlink fails, drop the backup so a failed
    // delete leaves zero debris (mirrors apply's temp-then-backup discipline).
    let bak = format!(".{name}.keydviz-bak.{stamp}.{}", std::process::id());
    dir.write_new(&bak, &bytes)?;
    if let Err(e) = dir.unlink(name) {
        let _ = dir.unlink(&bak);
        return Err(e);
    }
    Ok(Txn {
        dir,
        applied: vec![Applied { name: name.to_string(), prior: Prior::Existed(bytes), backup: Some(bak) }],
    })
}

impl Txn<'_> {
    /// Restore every prior, most recent first: rewrite the saved bytes over the
    /// target (via temp + rename, same discipline), or delete a file that did not
    /// exist before. The first error aborts (and is returned) — at that point the
    /// disk needs human eyes, which is exactly what the timestamped backups and
    /// the keyd panic sequence are for.
    pub fn revert(&mut self) -> io::Result<()> {
        while let Some(a) = self.applied.pop() {
            match a.prior {
                Prior::Existed(bytes) => {
                    let tmp = format!(".{}.keydviz-rev.{}", a.name, std::process::id());
                    self.dir.write_new(&tmp, &bytes)?;
                    self.dir.rename(&tmp, &a.name)?;
                }
                Prior::Absent => self.dir.unlink(&a.name)?,
            }
        }
        Ok(())
    }

    /// Commit: the writes stay. Consumes the transaction so the drop backstop
    /// can't revert a kept apply.
    pub fn keep(mut self) {
        self.applied.clear();
    }
}

/// Backstop: an un-kept transaction reverts on **every** exit path — early
/// `?`-returns and panic-unwinds included. Without this, any unexpected unwind
/// between rename+reload and the dead-man verdict (the original sin: `println!`
/// panicking on EPIPE when the GUI died) would leave a possibly-lockout config
/// installed with no revert. Best-effort by necessity (`Drop` can't propagate
/// errors, and can't reload keyd — the on-disk priors are restored and the next
/// reload or the panic sequence recovers the live state); the explicit
/// [`Txn::revert`] in the normal paths reports errors properly and leaves this a
/// no-op.
impl Drop for Txn<'_> {
    fn drop(&mut self) {
        let _ = self.revert();
    }
}

fn cstr(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "embedded NUL in name"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Self-cleaning temp dir (no external deps, like the rest of the workspace).
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let p = std::env::temp_dir()
                .join(format!("keydviz-apply-test-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ok_check(_: &Path) -> io::Result<()> {
        Ok(())
    }

    fn op(name: &str, content: &str) -> WriteOp {
        WriteOp { name: name.into(), content: content.as_bytes().to_vec() }
    }

    #[test]
    fn apply_then_keep_installs_content_and_backup() {
        let td = TempDir::new("keep");
        std::fs::write(td.0.join("a.conf"), "old").unwrap();
        let dir = Dir::open(&td.0).unwrap();

        let txn = apply(&dir, vec![op("a.conf", "new")], 42, ok_check).unwrap();
        assert_eq!(std::fs::read(td.0.join("a.conf")).unwrap(), b"new");
        let bak = format!(".a.conf.keydviz-bak.42.{}", std::process::id());
        assert_eq!(txn.applied[0].backup.as_deref(), Some(bak.as_str()));
        assert_eq!(std::fs::read(td.0.join(bak)).unwrap(), b"old");
        txn.keep();
        // keep() defuses the drop backstop: the new content survives the drop.
        assert_eq!(std::fs::read(td.0.join("a.conf")).unwrap(), b"new");
    }

    #[test]
    fn dropping_an_unkept_txn_reverts() {
        // The backstop: any exit path that drops the txn without keep() — early
        // return, panic-unwind — must restore the prior state.
        let td = TempDir::new("dropguard");
        std::fs::write(td.0.join("a.conf"), "old").unwrap();
        let dir = Dir::open(&td.0).unwrap();

        let txn = apply(&dir, vec![op("a.conf", "new")], 1, ok_check).unwrap();
        assert_eq!(std::fs::read(td.0.join("a.conf")).unwrap(), b"new");
        drop(txn);
        assert_eq!(std::fs::read(td.0.join("a.conf")).unwrap(), b"old");
    }

    #[test]
    fn drop_guard_fires_on_panic_unwind() {
        let td = TempDir::new("unwind");
        std::fs::write(td.0.join("a.conf"), "old").unwrap();
        let dir = Dir::open(&td.0).unwrap();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _txn = apply(&dir, vec![op("a.conf", "new")], 1, ok_check).unwrap();
            panic!("simulated EPIPE-style panic between apply and verdict");
        }));
        assert!(result.is_err());
        assert_eq!(std::fs::read(td.0.join("a.conf")).unwrap(), b"old");
    }

    #[test]
    fn revert_restores_prior_bytes() {
        let td = TempDir::new("revert");
        std::fs::write(td.0.join("a.conf"), "old").unwrap();
        let dir = Dir::open(&td.0).unwrap();

        let mut txn = apply(&dir, vec![op("a.conf", "new")], 1, ok_check).unwrap();
        txn.revert().unwrap();
        assert_eq!(std::fs::read(td.0.join("a.conf")).unwrap(), b"old");
    }

    #[test]
    fn revert_deletes_created_file() {
        let td = TempDir::new("absent");
        let dir = Dir::open(&td.0).unwrap();

        let mut txn = apply(&dir, vec![op("fresh.conf", "x")], 1, ok_check).unwrap();
        assert!(td.0.join("fresh.conf").exists());
        assert_eq!(txn.applied[0].prior, Prior::Absent);
        txn.revert().unwrap();
        assert!(!td.0.join("fresh.conf").exists());
    }

    #[test]
    fn failed_check_leaves_target_untouched() {
        let td = TempDir::new("check");
        std::fs::write(td.0.join("a.conf"), "old").unwrap();
        let dir = Dir::open(&td.0).unwrap();

        let err = apply(&dir, vec![op("a.conf", "bad")], 1, |_| {
            Err(io::Error::other("keyd check failed"))
        })
        .unwrap_err();
        assert_eq!(err.to_string(), "keyd check failed");
        assert_eq!(std::fs::read(td.0.join("a.conf")).unwrap(), b"old");
        // Zero debris: a rejected config leaves neither temp nor orphan backup.
        let leftovers: Vec<_> = std::fs::read_dir(&td.0)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("keydviz"))
            .collect();
        assert!(leftovers.is_empty(), "debris: {leftovers:?}");
    }

    #[test]
    fn check_sees_the_exact_bytes() {
        let td = TempDir::new("bytes");
        let dir = Dir::open(&td.0).unwrap();
        apply(&dir, vec![op("a.conf", "payload")], 1, |p| {
            assert_eq!(std::fs::read(p).unwrap(), b"payload");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn symlink_target_is_not_followed() {
        let td = TempDir::new("symlink");
        let outside = td.0.join("outside.txt");
        std::fs::write(&outside, "precious").unwrap();
        std::os::unix::fs::symlink(&outside, td.0.join("a.conf")).unwrap();
        let dir = Dir::open(&td.0).unwrap();

        // Reading the prior hits ELOOP (O_NOFOLLOW) — the apply aborts before
        // writing anything, and the symlink's target is never touched.
        let err = apply(&dir, vec![op("a.conf", "new")], 1, ok_check).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ELOOP));
        assert_eq!(std::fs::read(&outside).unwrap(), b"precious");
        assert!(td.0.join("a.conf").is_symlink());
    }

    #[test]
    fn dir_must_not_be_a_symlink() {
        let td = TempDir::new("dirlink");
        let real = td.0.join("real");
        std::fs::create_dir(&real).unwrap();
        let link = td.0.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        // Linux reports O_NOFOLLOW|O_DIRECTORY on a symlink as ENOTDIR (ELOOP on
        // some kernels) — either way, the open must refuse.
        let errno = Dir::open(&link).unwrap_err().raw_os_error();
        assert!(matches!(errno, Some(libc::ENOTDIR) | Some(libc::ELOOP)), "got {errno:?}");
    }

    #[test]
    fn delete_then_keep_removes_file_and_keeps_backup() {
        let td = TempDir::new("del-keep");
        std::fs::write(td.0.join("a.conf"), "live").unwrap();
        let dir = Dir::open(&td.0).unwrap();

        let txn = delete(&dir, "a.conf", 7).unwrap();
        assert!(!td.0.join("a.conf").exists(), "file should be gone");
        let bak = format!(".a.conf.keydviz-bak.7.{}", std::process::id());
        assert_eq!(std::fs::read(td.0.join(&bak)).unwrap(), b"live");
        txn.keep();
        // keep() defuses the drop backstop: the deletion survives.
        assert!(!td.0.join("a.conf").exists());
    }

    #[test]
    fn delete_then_revert_restores_the_file() {
        let td = TempDir::new("del-revert");
        std::fs::write(td.0.join("a.conf"), "live").unwrap();
        let dir = Dir::open(&td.0).unwrap();

        let mut txn = delete(&dir, "a.conf", 1).unwrap();
        assert!(!td.0.join("a.conf").exists());
        txn.revert().unwrap();
        assert_eq!(std::fs::read(td.0.join("a.conf")).unwrap(), b"live");
    }

    #[test]
    fn dropping_an_unkept_delete_restores_the_file() {
        let td = TempDir::new("del-drop");
        std::fs::write(td.0.join("a.conf"), "live").unwrap();
        let dir = Dir::open(&td.0).unwrap();

        let txn = delete(&dir, "a.conf", 1).unwrap();
        assert!(!td.0.join("a.conf").exists());
        drop(txn);
        assert_eq!(std::fs::read(td.0.join("a.conf")).unwrap(), b"live");
    }

    #[test]
    fn deleting_an_absent_config_errors_cleanly() {
        let td = TempDir::new("del-absent");
        let dir = Dir::open(&td.0).unwrap();
        let err = delete(&dir, "ghost.conf", 1).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        // No backup debris from a no-op delete.
        let leftovers: Vec<_> = std::fs::read_dir(&td.0)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("keydviz"))
            .collect();
        assert!(leftovers.is_empty(), "debris: {leftovers:?}");
    }

    #[test]
    fn delete_does_not_follow_a_symlink() {
        let td = TempDir::new("del-symlink");
        let outside = td.0.join("outside.txt");
        std::fs::write(&outside, "precious").unwrap();
        std::os::unix::fs::symlink(&outside, td.0.join("a.conf")).unwrap();
        let dir = Dir::open(&td.0).unwrap();

        let err = delete(&dir, "a.conf", 1).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc::ELOOP));
        assert_eq!(std::fs::read(&outside).unwrap(), b"precious");
        assert!(td.0.join("a.conf").is_symlink(), "symlink itself untouched");
    }

    #[test]
    fn multi_op_set_reverts_all_or_nothing() {
        let td = TempDir::new("multi");
        std::fs::write(td.0.join("a.conf"), "a-old").unwrap();
        let dir = Dir::open(&td.0).unwrap();

        // Second op's check fails → first op (already renamed) must roll back.
        let calls = std::cell::Cell::new(0);
        let err = apply(
            &dir,
            vec![op("a.conf", "a-new"), op("b.conf", "b-new")],
            1,
            |_| {
                calls.set(calls.get() + 1);
                if calls.get() == 2 {
                    Err(io::Error::other("second op rejected"))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "second op rejected");
        assert_eq!(std::fs::read(td.0.join("a.conf")).unwrap(), b"a-old");
        assert!(!td.0.join("b.conf").exists());
    }

    fn bak(td: &TempDir, name: &str, stamp: u64, pid: u32) {
        std::fs::write(td.0.join(format!(".{name}.keydviz-bak.{stamp}.{pid}")), b"x").unwrap();
    }
    fn exists(td: &TempDir, fname: &str) -> bool {
        td.0.join(fname).exists()
    }

    #[test]
    fn prune_keeps_newest_n_and_deletes_older() {
        let td = TempDir::new("prune");
        std::fs::write(td.0.join("a.conf"), "live").unwrap(); // the live config
        for s in [10u64, 20, 30, 40, 50] {
            bak(&td, "a.conf", s, 100);
        }
        let dir = Dir::open(&td.0).unwrap();
        prune_backups(&dir, "a.conf", 2);
        // Newest two (50, 40) survive; older three are gone.
        assert!(exists(&td, ".a.conf.keydviz-bak.50.100"));
        assert!(exists(&td, ".a.conf.keydviz-bak.40.100"));
        for s in [10, 20, 30] {
            assert!(!exists(&td, &format!(".a.conf.keydviz-bak.{s}.100")), "stamp {s} should be pruned");
        }
        // The live config is never touched.
        assert_eq!(std::fs::read(td.0.join("a.conf")).unwrap(), b"live");
    }

    #[test]
    fn prune_is_a_noop_below_the_limit() {
        let td = TempDir::new("prune-few");
        bak(&td, "a.conf", 10, 1);
        bak(&td, "a.conf", 20, 1);
        let dir = Dir::open(&td.0).unwrap();
        prune_backups(&dir, "a.conf", 5);
        assert!(exists(&td, ".a.conf.keydviz-bak.10.1"));
        assert!(exists(&td, ".a.conf.keydviz-bak.20.1"));
    }

    #[test]
    fn prune_never_touches_other_configs_or_unrelated_files() {
        let td = TempDir::new("prune-strict");
        // Backups of a DIFFERENT config, plus look-alikes that must NOT match.
        bak(&td, "b.conf", 10, 1);
        bak(&td, "b.conf", 20, 1);
        std::fs::write(td.0.join(".a.conf.keydviz-bak.notanumber.1"), b"x").unwrap();
        std::fs::write(td.0.join(".a.conf.keydviz-bak.30.notapid"), b"x").unwrap();
        std::fs::write(td.0.join(".a.conf.keydviz-bak.40.1.extra"), b"x").unwrap();
        std::fs::write(td.0.join("a.conf"), "live").unwrap();
        // Two real backups of a.conf so the keep=0 prune has something to remove.
        bak(&td, "a.conf", 100, 1);
        bak(&td, "a.conf", 200, 1);
        let dir = Dir::open(&td.0).unwrap();
        prune_backups(&dir, "a.conf", 0); // delete every *valid* a.conf backup
        // Real a.conf backups gone…
        assert!(!exists(&td, ".a.conf.keydviz-bak.100.1"));
        assert!(!exists(&td, ".a.conf.keydviz-bak.200.1"));
        // …but the other config's backups, the malformed look-alikes, and the live
        // config are all untouched.
        assert!(exists(&td, ".b.conf.keydviz-bak.10.1"));
        assert!(exists(&td, ".b.conf.keydviz-bak.20.1"));
        assert!(exists(&td, ".a.conf.keydviz-bak.notanumber.1"));
        assert!(exists(&td, ".a.conf.keydviz-bak.30.notapid"));
        assert!(exists(&td, ".a.conf.keydviz-bak.40.1.extra"));
        assert_eq!(std::fs::read(td.0.join("a.conf")).unwrap(), b"live");
    }
}
