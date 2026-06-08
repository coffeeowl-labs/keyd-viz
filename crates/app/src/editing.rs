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
use keydviz_core::{parser, round_trips, Config, TapHold};

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

    /// Make `key` transparent (pass-through) on the `layer` board: remove its
    /// binding so the key falls through to the base layer — keyd's default for any
    /// unbound key. Clears the key from every section that merges into the board
    /// (last-wins means a single leftover would keep it bound). A no-op when the
    /// key was already unbound. `Err` only when there is no such board at all.
    pub fn clear_binding(&mut self, layer: &str, key: &str) -> Result<(), String> {
        if !self.editable_sections().iter().any(|s| s == layer) {
            return Err(format!("this config has no [{layer}] section"));
        }
        self.edit.clear_binding(layer, key);
        Ok(())
    }

    /// The selected key's current binding as a decomposed tap/hold, if it is one
    /// of the editable tap/hold forms — so the panel can show "tap / hold" slots
    /// instead of the raw `overload(...)` text. `None` when the key is unbound or
    /// bound to something that isn't a tap/hold (plain remap, macro, etc.).
    pub fn current_tap_hold(&self, layer: &str, key: &str) -> Option<TapHold> {
        let rhs = self.current_binding(layer, key)?;
        TapHold::parse(key, &rhs)
    }

    /// Make `key` a dual-function (tap/hold) key on the `layer` board: hold →
    /// `target` (a layer or modifier), tap → `tap` (`None` = momentary hold-only).
    /// Editing a key that is already a tap/hold preserves its function and any
    /// timeouts already in the file (see [`TapHold::compose`]); a fresh one gets a
    /// canonical `overload(target, tap)`. `Err` when there is no such board.
    pub fn set_tap_hold(
        &mut self,
        layer: &str,
        key: &str,
        target: &str,
        tap: Option<String>,
    ) -> Result<(), String> {
        // Read the existing binding (immutable) before taking the mutable borrow.
        let existing = self.current_tap_hold(layer, key);
        let th = TapHold::compose(existing.as_ref(), target.to_string(), tap);
        let section = self
            .edit
            .target_section_mut(layer)
            .ok_or_else(|| format!("this config has no [{layer}] section"))?;
        section.set_or_add_binding(key, &th.serialize());
        Ok(())
    }

    /// The semantic model for re-rendering the boards — same derivation the
    /// viewer uses, so the preview is exactly what the viewer would show.
    pub fn config(&self) -> Config {
        parser::derive(&self.edit)
    }

    /// The editable section base-names, in file order, deduped — the exact set the
    /// layer chooser should offer. `main` appears only when the file actually has a
    /// base section, so the chooser can never present a chip that errors on click.
    pub fn editable_sections(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in &self.edit.sections {
            if matches!(
                s.kind,
                SectionKind::Main | SectionKind::Layer | SectionKind::Composite
            ) {
                let base = s.base_name().trim().to_string();
                if !base.is_empty() && !out.contains(&base) {
                    out.push(base);
                }
            }
        }
        out
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

    /// The exact bytes persistence writes — the same `serialize()` behind
    /// [`Self::save_draft`] and the one-click apply payload (E2). One source of
    /// truth: what the user previewed is byte-for-byte what lands on disk.
    pub fn serialized(&self) -> String {
        self.edit.serialize()
    }

    /// `Some(name)` iff this session edits `<dir>/<name>.conf` with a name the
    /// apply tool's allow-list accepts — the only shape one-click apply will
    /// touch (the tool re-derives the destination from the name; it never takes
    /// a path). Anything else stays draft-then-install.
    pub fn apply_target(&self, dir: &Path) -> Option<String> {
        if self.path.parent() != Some(dir) {
            return None;
        }
        let name = self.path.file_name()?.to_str()?.strip_suffix(".conf")?;
        keydviz_apply::valid_name(name).then(|| name.to_string())
    }

    /// Warn when the real file moved under us since open — persisting would
    /// overwrite those external edits. Shared by draft save and apply pre-flight.
    pub fn stale_warning(&self) -> Option<String> {
        match std::fs::read_to_string(&self.path) {
            Ok(now) if now != self.original => Some(format!(
                "{} changed on disk since this session opened — review the diff \
                 before installing",
                self.path.display()
            )),
            _ => None,
        }
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
        let stale_warning = self.stale_warning();

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

/// `~/.config/keyd-viz/drafts/` (honouring `$XDG_CONFIG_HOME`), sharing `prefs`'
/// XDG base so the draft store and the layout store can never disagree.
fn drafts_dir() -> Option<PathBuf> {
    Some(crate::prefs::config_home()?.join("keyd-viz").join("drafts"))
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

/// `keyd check` a candidate body that exists only in memory (apply pre-flight) —
/// written to a temp file for the check, removed after. Like the draft check
/// this is early UX feedback, never the security gate: the privileged tool
/// re-runs `keyd check` on the exact bytes it writes (§5.3, fail closed there).
pub fn keyd_check_bytes(bytes: &str) -> Option<Result<(), String>> {
    // pid + sequence, like probe::check_works: concurrent callers (parallel
    // tests) must never share a temp file.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir()
        .join(format!("keyd-viz-preflight-{}-{seq}.conf", std::process::id()));
    if std::fs::write(&path, bytes).is_err() {
        return None;
    }
    let verdict = keyd_check_draft(&path);
    let _ = std::fs::remove_file(&path);
    verdict
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
/// A compact `-old` / `+new` line diff showing only the lines that actually
/// changed. Computed via a longest-common-subsequence so removals/additions
/// scattered across the file — e.g. clearing a key that recurs in several merged
/// sections — don't drag untouched lines (section headers especially) into the
/// diff. This is the change summary the user reviews before installing or
/// applying, so it must reflect exactly what changed. Configs are small
/// (`MAX_CONFIG_BYTES`), so the O(n·m) table is fine.
fn line_diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    // lcs[i][j] = LCS length of a[i..] and b[j..].
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    // Walk the table in file order, emitting `-`/`+` only for off-subsequence
    // lines; common lines advance both cursors silently.
    let mut out = String::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push_str(&format!("- {}\n", a[i]));
            i += 1;
        } else {
            out.push_str(&format!("+ {}\n", b[j]));
            j += 1;
        }
    }
    for line in &a[i..] {
        out.push_str(&format!("- {line}\n"));
    }
    for line in &b[j..] {
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
    fn clear_binding_makes_a_key_transparent() {
        let td = TempDir::new("clear");
        let mut s = session(&td);
        s.clear_binding("main", "capslock").unwrap();
        assert!(s.dirty());
        // Unbound now → the preview falls through (no remap), and the line is gone.
        assert_eq!(s.current_binding("main", "capslock"), None);
        assert_eq!(s.config().remap("capslock"), None);
        assert_eq!(s.diff(), "- capslock = esc\n");
        // Clearing an already-unbound key is a no-op; a missing board errors.
        let mut s2 = session(&td);
        s2.clear_binding("main", "nonexistent").unwrap();
        assert!(!s2.dirty());
        assert!(s2.clear_binding("sym", "a").unwrap_err().contains("[sym]"));
    }

    #[test]
    fn tap_hold_new_key_emits_overload() {
        let td = TempDir::new("th-new");
        let mut s = session(&td);
        // capslock currently = esc; make it tap esc / hold nav.
        s.set_tap_hold("main", "capslock", "nav", Some("esc".into())).unwrap();
        assert!(s.dirty());
        assert_eq!(s.diff(), "- capslock = esc\n+ capslock = overload(nav, esc)\n");
        let th = s.current_tap_hold("main", "capslock").unwrap();
        assert_eq!(th.target, "nav");
        assert_eq!(th.tap.as_deref(), Some("esc"));
    }

    #[test]
    fn tap_hold_edit_preserves_lettermod_timeouts() {
        // The hand-tuned hhkb case: editing the hold target must keep 150/200.
        let td = TempDir::new("th-edit");
        let p = td.0.join("test.conf");
        std::fs::write(&p, "[ids]\n*\n\n[main]\nf = lettermod(nav, f, 150, 200)\n\n[nav]\nh = left\n[num]\nj = 1\n").unwrap();
        let mut s = EditSession::open(&p).unwrap();
        // The reader decomposes the existing lettermod into slots.
        let cur = s.current_tap_hold("main", "f").unwrap();
        assert_eq!(cur.func, "lettermod");
        assert_eq!(cur.target, "nav");
        // Repoint the hold from nav to num; timings survive.
        s.set_tap_hold("main", "f", "num", Some("f".into())).unwrap();
        assert_eq!(s.diff(), "- f = lettermod(nav, f, 150, 200)\n+ f = lettermod(num, f, 150, 200)\n");
    }

    #[test]
    fn tap_hold_momentary_has_no_tap() {
        let td = TempDir::new("th-mom");
        let mut s = session(&td);
        s.set_tap_hold("main", "capslock", "nav", None).unwrap();
        assert_eq!(s.diff(), "- capslock = esc\n+ capslock = layer(nav)\n");
        let th = s.current_tap_hold("main", "capslock").unwrap();
        assert_eq!(th.tap, None);
    }

    #[test]
    fn clear_across_merged_sections_diffs_cleanly() {
        // Clearing a key that recurs across merged sections must NOT show the
        // untouched header between them as removed-and-re-added (the diff is the
        // user's pre-install review). The LCS line_diff keeps it to real changes.
        let td = TempDir::new("clear-merged");
        let p = td.0.join("test.conf");
        std::fs::write(&p, "[ids]\n*\n\n[nav]\nh = left\n[nav:C]\nh = right\nj = down\n").unwrap();
        let mut s = EditSession::open(&p).unwrap();
        s.clear_binding("nav", "h").unwrap();
        assert_eq!(s.diff(), "- h = left\n- h = right\n");
    }

    #[test]
    fn editable_sections_are_the_real_file_sections() {
        let td = TempDir::new("sections");
        let s = session(&td);
        // SRC has [ids], [main], [nav] — [ids] is not editable, the other two are.
        assert_eq!(s.editable_sections(), vec!["main".to_string(), "nav".to_string()]);

        // A config with no [main] must not advertise a "main" chip that errors on click.
        let p = td.0.join("nomain.conf");
        std::fs::write(&p, "[ids]\n*\n\n[nav]\nh = left\n").unwrap();
        let s2 = EditSession::open(&p).unwrap();
        assert_eq!(s2.editable_sections(), vec!["nav".to_string()]);
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

    #[test]
    fn serialized_is_the_draft_body() {
        let td = TempDir::new("serialized");
        let mut s = session(&td);
        s.set_binding("main", "capslock", "noop").unwrap();
        let saved = s.save_draft_to(&td.0.join("drafts")).unwrap();
        let body = std::fs::read_to_string(&saved.draft_path).unwrap();
        // What apply would send is byte-for-byte what the draft wrote.
        assert_eq!(s.serialized(), body);
    }

    #[test]
    fn apply_target_only_matches_dir_and_valid_names() {
        let td = TempDir::new("target");
        let s = session(&td); // edits <td>/test.conf
        assert_eq!(s.apply_target(&td.0).as_deref(), Some("test"));
        // Wrong dir → not a one-click candidate.
        assert_eq!(s.apply_target(Path::new("/etc/keyd")), None);

        // A name the apply tool's allow-list rejects (dots) never qualifies,
        // even in the right dir.
        let p = td.0.join("my.board.conf");
        std::fs::write(&p, SRC).unwrap();
        let s2 = EditSession::open(&p).unwrap();
        assert_eq!(s2.apply_target(&td.0), None);

        // No .conf suffix → keyd wouldn't load it; not a target either.
        let p3 = td.0.join("noext");
        std::fs::write(&p3, SRC).unwrap();
        let s3 = EditSession::open(&p3).unwrap();
        assert_eq!(s3.apply_target(&td.0), None);
    }

    #[test]
    fn stale_warning_matches_save_draft() {
        let td = TempDir::new("stale2");
        let mut s = session(&td);
        s.set_binding("main", "capslock", "noop").unwrap();
        assert!(s.stale_warning().is_none());
        std::fs::write(td.0.join("test.conf"), "[ids]\n*\n[main]\na = b\n").unwrap();
        assert!(s.stale_warning().is_some());
    }

    #[test]
    fn keyd_check_bytes_mirrors_environment() {
        // Hermetic like probe.rs: with keyd installed both verdicts are real;
        // without keyd both are None — never a false "valid".
        let good = keyd_check_bytes("[ids]\n*\n[main]\n");
        let bad = keyd_check_bytes("[ids]\n*\n[main]\ncapslock = bogus_action(\n");
        match (good, bad) {
            (Some(g), Some(b)) => {
                assert_eq!(g, Ok(()));
                assert!(b.is_err());
            }
            (None, None) => {} // no keyd in PATH
            other => panic!("inconsistent keyd availability: {other:?}"),
        }
    }
}
