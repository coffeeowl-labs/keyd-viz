//! Edit-mode session state — Phase 6 E1, draft-then-install (design doc §4, §6).
//!
//! One [`EditSession`] per opened config: the line-faithful [`EditConfig`] is the
//! single mutable model, every visual edit goes through [`EditSession::set_binding`],
//! and the board re-renders from [`EditSession::config`] (the same `derive()` the
//! viewer uses — preview *is* the viewer, §5.6). Persistence in E1 is
//! **draft-then-install**: [`EditSession::save_draft`] writes the serialized file to
//! `~/.config/keyd-viz/drafts/<name>.conf` and returns copy-paste install steps —
//! no privilege, no daemon involvement; the one-click pkexec apply is E2.
//!
//! The §5.1 round-trip gate runs at open: a file the model can't reproduce
//! byte-for-byte (or that keyd would reject outright) stays **view-only** — the
//! editor never risks clobbering what it doesn't fully understand.

use std::io;
use std::path::{Path, PathBuf};

use keydviz_core::edit::{EditConfig, SectionKind};
use keydviz_core::{parser, round_trips, Config};

/// Why a config can't be opened for editing (it remains viewable as before).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewOnly {
    /// Couldn't read the file (or it isn't UTF-8).
    Unreadable(String),
    /// `serialize(parse(f)) != f` — the model-soundness gate tripped (§5.1).
    RoundTripGate,
    /// keyd itself would reject the file (entry before the first section).
    KeydRejects(String),
}

impl ViewOnly {
    pub fn describe(&self) -> String {
        match self {
            ViewOnly::Unreadable(e) => format!("can't read config: {e}"),
            ViewOnly::RoundTripGate => {
                "view-only: this file can't be reproduced byte-for-byte".to_string()
            }
            ViewOnly::KeydRejects(w) => format!("view-only: {w}"),
        }
    }
}

/// An open edit session for one real config file.
pub struct EditSession {
    /// The real config this session edits (e.g. `/etc/keyd/hhkb.conf`).
    pub path: PathBuf,
    /// The file's bytes at open — diff base and staleness sentinel.
    original: String,
    edit: EditConfig,
}

/// Result of a draft save: where it went and what to run to install it.
pub struct DraftSaved {
    pub draft_path: PathBuf,
    /// Copy-paste shell steps installing the draft over the real config.
    pub install_steps: String,
    /// Set when the real config changed on disk since the session opened —
    /// installing the draft would overwrite those external edits.
    pub stale_warning: Option<String>,
    /// `keyd check` verdict on the draft, when keyd is available: `Some(Ok(()))`
    /// valid, `Some(Err(msg))` rejected, `None` keyd not found (drafts still save
    /// — the install steps run through the user's own shell, not a root tool).
    pub check: Option<Result<(), String>>,
}

impl EditSession {
    /// Open a config for editing, running the §5.1 gate. `Err` means view-only.
    pub fn open(path: &Path) -> Result<EditSession, ViewOnly> {
        let original = std::fs::read_to_string(path)
            .map_err(|e| ViewOnly::Unreadable(e.to_string()))?;
        if !round_trips(&original) {
            return Err(ViewOnly::RoundTripGate);
        }
        let edit = EditConfig::parse(&original);
        // keyd refuses a file with an entry before the first section header —
        // editing something keyd won't load is a trap, not a feature.
        if let Some(w) = edit.diagnostics().iter().find(|w| w.contains("rejects")) {
            return Err(ViewOnly::KeydRejects(w.clone()));
        }
        Ok(EditSession { path: path.to_path_buf(), original, edit })
    }

    /// Bind `key = val` on the board for `layer` (`"main"` for the base board).
    /// `Err` names the reason (no such section — section creation is E2).
    pub fn set_binding(&mut self, layer: &str, key: &str, val: &str) -> Result<(), String> {
        let section = self
            .edit
            .target_section_mut(layer)
            .ok_or_else(|| format!("this config has no [{layer}] section"))?;
        section.set_or_add_binding(key, val);
        Ok(())
    }

    /// The semantic model for re-rendering the boards — same derivation the
    /// viewer uses, so the preview is exactly what the viewer would show.
    pub fn config(&self) -> Config {
        parser::derive(&self.edit)
    }

    /// The value currently bound to `key` in `layer`'s section, if any.
    pub fn current_binding(&self, layer: &str, key: &str) -> Option<String> {
        self.edit
            .sections
            .iter()
            .rev()
            .filter(|s| {
                matches!(
                    s.kind,
                    SectionKind::Main | SectionKind::Layer | SectionKind::Composite
                ) && s.base_name().trim() == layer
            })
            .find_map(|s| s.get_binding(key).map(str::to_string))
    }

    pub fn dirty(&self) -> bool {
        self.edit.is_dirty()
    }

    /// A compact `-old` / `+new` line diff of the session's changes (common
    /// prefix/suffix trimmed — exact for the single-binding edits E1 produces).
    pub fn diff(&self) -> String {
        line_diff(&self.original, &self.edit.serialize())
    }

    /// Write the draft and return the install steps (§4 draft-then-install).
    pub fn save_draft(&self) -> io::Result<DraftSaved> {
        let dir = drafts_dir()
            .ok_or_else(|| io::Error::other("no XDG_CONFIG_HOME or HOME"))?;
        self.save_draft_to(&dir)
    }

    /// [`Self::save_draft`] with an explicit directory (testable core).
    fn save_draft_to(&self, dir: &Path) -> io::Result<DraftSaved> {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "draft.conf".to_string());
        std::fs::create_dir_all(dir)?;
        let draft_path = dir.join(&name);
        let bytes = self.edit.serialize();
        std::fs::write(&draft_path, &bytes)?;

        // Staleness: warn when the real file moved under us since open.
        let stale_warning = match std::fs::read_to_string(&self.path) {
            Ok(now) if now != self.original => Some(format!(
                "{} changed on disk since this session opened — review the diff \
                 before installing",
                self.path.display()
            )),
            _ => None,
        };

        let install_steps = format!(
            "sudo cp {} {}\nsudo keyd reload",
            shell_quote(&draft_path.display().to_string()),
            shell_quote(&self.path.display().to_string()),
        );
        Ok(DraftSaved {
            check: keyd_check_draft(&draft_path),
            draft_path,
            install_steps,
            stale_warning,
        })
    }
}

/// `~/.config/keyd-viz/drafts/` (honouring `$XDG_CONFIG_HOME`), like `prefs`.
fn drafts_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("keyd-viz").join("drafts"))
}

/// `keyd check` the draft when keyd is around — early feedback, not a gate
/// (the user installs through their own shell; nothing here is privileged).
fn keyd_check_draft(path: &Path) -> Option<Result<(), String>> {
    let out = std::process::Command::new("keyd").arg("check").arg(path).output().ok()?;
    Some(if out.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&out.stdout);
        Err(detail.trim().replace('\n', " | "))
    })
}

/// Single-quote a path for copy-paste shell steps.
fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Minimal line diff: trim the common prefix and suffix, emit the differing
/// middle as `-`/`+` lines. Exact and readable for localized edits.
fn line_diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let mut end_a = a.len();
    let mut end_b = b.len();
    while end_a > start && end_b > start && a[end_a - 1] == b[end_b - 1] {
        end_a -= 1;
        end_b -= 1;
    }
    let mut out = String::new();
    for line in &a[start..end_a] {
        out.push_str(&format!("- {line}\n"));
    }
    for line in &b[start..end_b] {
        out.push_str(&format!("+ {line}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let p = std::env::temp_dir()
                .join(format!("keydviz-edit-test-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const SRC: &str = "[ids]\n*\n\n[main]\ncapslock = esc\n\n[nav]\nh = left\n";

    fn session(td: &TempDir) -> EditSession {
        let p = td.0.join("test.conf");
        std::fs::write(&p, SRC).unwrap();
        EditSession::open(&p).unwrap()
    }

    #[test]
    fn open_edit_rerender_diff() {
        let td = TempDir::new("flow");
        let mut s = session(&td);
        assert!(!s.dirty());
        assert_eq!(s.current_binding("main", "capslock").as_deref(), Some("esc"));

        s.set_binding("main", "capslock", "noop").unwrap();
        assert!(s.dirty());
        // The preview model reflects the edit (remap b=noop shows as remap).
        assert_eq!(s.config().remap("capslock"), Some("noop"));
        assert_eq!(s.diff(), "- capslock = esc\n+ capslock = noop\n");
    }

    #[test]
    fn edit_targets_the_right_layer_section() {
        let td = TempDir::new("layer");
        let mut s = session(&td);
        s.set_binding("nav", "j", "down").unwrap();
        assert_eq!(s.diff(), "+ j = down\n");
        assert_eq!(s.current_binding("nav", "j").as_deref(), Some("down"));
        // No such section → a named error, not a panic or silent drop.
        assert!(s.set_binding("sym", "a", "b").unwrap_err().contains("[sym]"));
    }

    #[test]
    fn gate_sends_unreproducible_files_to_view_only() {
        // A file keyd rejects outright (entry before first section).
        let td = TempDir::new("gate");
        let p = td.0.join("bad.conf");
        std::fs::write(&p, "stray = line\n[main]\na = b\n").unwrap();
        match EditSession::open(&p) {
            Err(ViewOnly::KeydRejects(_)) => {}
            other => panic!("expected KeydRejects, got {:?}", other.err()),
        }
    }

    #[test]
    fn save_draft_writes_serialized_bytes_and_steps() {
        let td = TempDir::new("draft");
        let mut s = session(&td);
        s.set_binding("main", "capslock", "noop").unwrap();
        // Explicit dir: env vars are process-global and tests run in parallel.
        let saved = s.save_draft_to(&td.0.join("drafts")).unwrap();

        let body = std::fs::read_to_string(&saved.draft_path).unwrap();
        assert_eq!(body, SRC.replace("capslock = esc", "capslock = noop"));
        assert!(saved.install_steps.contains("sudo cp"));
        assert!(saved.install_steps.contains("sudo keyd reload"));
        assert!(saved.stale_warning.is_none());
        // keyd is installed on the dev box: the draft must validate.
        if let Some(check) = saved.check {
            assert_eq!(check, Ok(()));
        }
    }

    #[test]
    fn stale_real_file_is_flagged() {
        let td = TempDir::new("stale");
        let mut s = session(&td);
        s.set_binding("main", "capslock", "noop").unwrap();
        // Simulate an external edit landing while the session was open.
        std::fs::write(td.0.join("test.conf"), "[ids]\n*\n[main]\na = b\n").unwrap();
        let saved = s.save_draft_to(&td.0.join("drafts")).unwrap();
        assert!(saved.stale_warning.is_some());
    }

    #[test]
    fn shell_quote_only_when_needed() {
        assert_eq!(shell_quote("/etc/keyd/hhkb.conf"), "/etc/keyd/hhkb.conf");
        assert_eq!(shell_quote("/tmp/my dir/x.conf"), "'/tmp/my dir/x.conf'");
    }
}
