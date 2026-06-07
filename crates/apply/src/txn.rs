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
}
