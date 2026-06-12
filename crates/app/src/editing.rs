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

use keydviz_core::edit::{starter_config, EditConfig, EntryKind, Section, SectionKind};
use keydviz_core::{
    canonical_chord, is_chord_key, parser, round_trips, Behavior, Config, Macro, TapHold,
};

/// Does `section` belong to the layer named `layer`? `"main"` matches the base
/// (`[main]`); any other name matches its layer/composite section(s), including a
/// modset-qualified `[nav:C]` (same base name) so chord ops span every spelling.
fn section_is_layer(section: &Section, layer: &str) -> bool {
    if layer == "main" {
        section.kind == SectionKind::Main
    } else {
        matches!(section.kind, SectionKind::Layer | SectionKind::Composite)
            && section.base_name().trim() == layer
    }
}

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
    /// The real config this session edits (e.g. `/etc/keyd/hhkb.conf`). For a
    /// freshly-created config (see [`EditSession::create`]) the file does not yet
    /// exist — the apply tool's `Absent` write path creates it.
    pub path: PathBuf,
    /// The file's bytes at open — diff base and staleness sentinel. Empty for a
    /// brand-new config: the diff then shows the whole starter as additions, and
    /// the staleness check (a read of a not-yet-existing path) yields nothing.
    original: String,
    edit: EditConfig,
    /// This session is creating a new config that isn't on disk yet (§5.5). It has
    /// content to persist even before the first edit, so [`Self::dirty`] reports
    /// dirty until the create is applied (after which the session is re-opened as a
    /// normal on-disk one and `created` is false).
    created: bool,
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
        Ok(EditSession { path: path.to_path_buf(), original, edit, created: false })
    }

    /// Start a brand-new config for an unconfigured keyboard (design doc §5.5).
    /// `path` is where it *will* live — `<config-dir>/<name>.conf`, which must not
    /// exist yet (the caller checks; this is the path the one-click apply tool
    /// re-derives from the name). `ids_lines` are the `[ids]` entries (a chosen
    /// device's `vendor:product`, or the bare `*` wildcard). The session opens on a
    /// minimal `[ids]`+`[main]` starter ([`starter_config`]) with the same §5.1 gate
    /// as [`Self::open`] — which the generated text passes by construction — so the
    /// whole edit/preview/draft/apply surface works unchanged from here.
    pub fn create(path: &Path, ids_lines: &[&str]) -> Result<EditSession, ViewOnly> {
        let starter = starter_config(ids_lines);
        // Identity-by-construction, but run the gate so create can never seed a model
        // the editor would otherwise refuse — one definition of "editable", not two.
        if !round_trips(&starter) {
            return Err(ViewOnly::RoundTripGate);
        }
        let edit = EditConfig::parse(&starter);
        if let Some(w) = edit.diagnostics().iter().find(|w| w.contains("rejects")) {
            return Err(ViewOnly::KeydRejects(w.clone()));
        }
        Ok(EditSession { path: path.to_path_buf(), original: String::new(), edit, created: true })
    }

    /// Bind `key = val` on the board for `layer` (`"main"` for the base board).
    /// Creates the `[layer]` section if the config doesn't declare one locally
    /// (e.g. its bindings live in an `include`), so the bind always lands.
    pub fn set_binding(&mut self, layer: &str, key: &str, val: &str) -> Result<(), String> {
        let section = self
            .edit
            .target_or_create_section_mut(layer)
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
    /// `target` (a layer or modifier), tap → `tap` (`None` = momentary hold-only),
    /// with the chosen `feel` ([`Behavior`]). `feel == None` means "no feel picked"
    /// — an existing tap/hold whose form we don't name (plain `overload`) is then
    /// preserved as-is. Retargeting/retapping a key that already has the chosen
    /// feel preserves its function and timeouts (see [`TapHold::compose`]); a new
    /// key or a deliberate feel switch takes that feel's defaults. `Err` when there
    /// is no such board.
    pub fn set_tap_hold(
        &mut self,
        layer: &str,
        key: &str,
        target: &str,
        tap: Option<String>,
        feel: Option<Behavior>,
    ) -> Result<(), String> {
        // Refuse to recompose over keyd's `overloadi(...)` — its tap-first/descriptor-hold
        // form. The viewer badges it as a tap/hold (the parser routes it to `holds`), but
        // `current_tap_hold` can't decompose it, so the panel opens empty; re-setting would
        // silently discard the original's nested hold descriptor + tuned timeouts. `keyd
        // check` wouldn't catch the loss (the result is valid), so guard it here — the user
        // edits these as raw text in simple mode instead.
        if let Some(cur) = self.current_binding(layer, key) {
            if parser::leading_fn(&cur) == Some("overloadi") {
                return Err("this key uses keyd's advanced overloadi() form \u{2014} switch \
                            to simple mode to edit it as text"
                    .to_string());
            }
        }
        // Read the existing binding (immutable) before taking the mutable borrow.
        let existing = self.current_tap_hold(layer, key);
        let th = TapHold::compose(existing.as_ref(), target.to_string(), tap, feel);
        let section = self
            .edit
            .target_or_create_section_mut(layer)
            .ok_or_else(|| format!("this config has no [{layer}] section"))?;
        section.set_or_add_binding(key, &th.serialize());
        Ok(())
    }

    /// The selected key's binding decomposed as a structured macro, if it's a
    /// `macro(...)`/`macro2(...)` we can model losslessly — so the panel can show
    /// the token-list builder instead of raw text. `None` when the key is unbound,
    /// isn't a macro, or is a macro shape we don't model (nested, literal parens,
    /// exotic `macro2` args), which keeps it editable as raw text and clobber-safe.
    pub fn current_macro(&self, layer: &str, key: &str) -> Option<Macro> {
        let rhs = self.current_binding(layer, key)?;
        Macro::parse(&rhs)
    }

    /// Write `mac` as the binding for `key` on `layer`, serialized to keyd macro
    /// syntax. Creates the `[layer]` section if the config doesn't declare one
    /// locally (same as [`Self::set_binding`]).
    ///
    /// Two guards keep this faithful: (1) it refuses to overwrite an *existing*
    /// macro that [`Macro::parse`] can't decompose — re-setting would silently
    /// discard the original's exotic form (the `overloadi` philosophy); (2) it
    /// refuses to write a macro whose serialization doesn't survive our own
    /// round-trip, which catches anything keyd can't faithfully represent (e.g. a
    /// literal `(`/`)` that slipped into a text step — keyd has no escape for it).
    pub fn set_macro(&mut self, layer: &str, key: &str, mac: &Macro) -> Result<(), String> {
        if mac.tokens.is_empty() {
            return Err("a macro needs at least one step".to_string());
        }
        // Don't recompose over a macro we couldn't decompose in the first place.
        if let Some(cur) = self.current_binding(layer, key) {
            if matches!(parser::leading_fn(&cur), Some("macro") | Some("macro2"))
                && Macro::parse(&cur).is_none()
            {
                return Err("this key uses an advanced macro form keyd-viz can't edit \u{2014} \
                            switch to simple mode to edit it as text"
                    .to_string());
            }
        }
        // Self-round-trip guard: the serialized macro must re-parse and re-serialize
        // identically, else it holds something keyd can't represent (a literal paren).
        let rhs = mac.serialize();
        let stable = Macro::parse(&rhs).is_some_and(|m| m.serialize() == rhs);
        if !stable {
            return Err("this macro has text keyd can\u{2019}t type \u{2014} remove any \
                        parentheses and try again"
                .to_string());
        }
        let section = self
            .edit
            .target_or_create_section_mut(layer)
            .ok_or_else(|| format!("this config has no [{layer}] section"))?;
        section.set_or_add_binding(key, &rhs);
        Ok(())
    }

    /// Every chord defined in `layer` (`"main"` for the base, or a layer name like
    /// `"nav"`), as `(chord_key, action)` in file order — the verbatim LHS spelling and
    /// RHS value, deduped by canonical form (keep-last, since keyd is last-wins). The
    /// chord editor lists these. keyd scopes a chord to the layer it's declared in.
    /// See [`keydviz_core::canonical_chord`].
    pub fn chords(&self, layer: &str) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for s in &self.edit.sections {
            if !section_is_layer(s, layer) {
                continue;
            }
            for e in &s.entries {
                let EntryKind::Binding { key: k, val: Some(v), .. } = &e.kind else { continue };
                if is_chord_key(k) {
                    let canon = canonical_chord(k);
                    out.retain(|(ek, _)| canonical_chord(ek) != canon);
                    out.push((k.clone(), v.clone()));
                }
            }
        }
        out
    }

    /// Bind `key1+key2 = action` in `layer` (`"main"` or a layer name). If a chord for
    /// these two keys already exists in any order (canonical match), its line is
    /// rewritten in place — its original LHS spelling preserved, only the action changed
    /// (so editing `k+j` updates the existing `j+k`). Otherwise a new `key1+key2` line
    /// is appended. `Err` on: no such section, the two keys equal, or an empty
    /// key/action.
    pub fn set_chord(
        &mut self,
        layer: &str,
        key1: &str,
        key2: &str,
        action: &str,
    ) -> Result<(), String> {
        let (key1, key2, action) = (key1.trim(), key2.trim(), action.trim());
        if key1.is_empty() || key2.is_empty() {
            return Err("pick two keys for the chord".into());
        }
        if key1 == key2 {
            return Err("a chord needs two different keys".into());
        }
        if action.is_empty() {
            return Err("enter an action for the chord".into());
        }
        let new_key = format!("{key1}+{key2}");
        let canon = canonical_chord(&new_key);
        let section = self
            .edit
            .target_or_create_section_mut(layer)
            .ok_or_else(|| format!("this config has no [{layer}] section"))?;
        // Rewrite an existing chord line (any order) in place, else append a new one.
        let existing = section.entries.iter().find_map(|e| match &e.kind {
            EntryKind::Binding { key: k, .. } if is_chord_key(k) && canonical_chord(k) == canon => {
                Some(k.clone())
            }
            _ => None,
        });
        match existing {
            Some(k) => {
                section.set_binding(&k, action);
            }
            None => section.set_or_add_binding(&new_key, action),
        }
        Ok(())
    }

    /// Remove the chord whose key matches `chord_key` (canonical, order-independent)
    /// from `layer` — clearing every spelling across every matching section so a
    /// leftover `k+j` can't keep it bound (mirrors [`Self::clear_binding`]). `Err`
    /// only when `layer` has no section; a missing chord is a no-op `Ok`.
    pub fn remove_chord(&mut self, layer: &str, chord_key: &str) -> Result<(), String> {
        let canon = canonical_chord(chord_key.trim());
        let mut found = false;
        for s in &mut self.edit.sections {
            if !section_is_layer(s, layer) {
                continue;
            }
            found = true;
            let keys: Vec<String> = s
                .entries
                .iter()
                .filter_map(|e| match &e.kind {
                    EntryKind::Binding { key: k, .. }
                        if is_chord_key(k) && canonical_chord(k) == canon =>
                    {
                        Some(k.clone())
                    }
                    _ => None,
                })
                .collect();
            for k in keys {
                s.remove_binding(&k);
            }
        }
        if !found {
            return Err(format!("this config has no [{layer}] section"));
        }
        Ok(())
    }

    /// The `[global]` daemon options currently set, as `(name, value)` (last-wins per
    /// key, in file order). Empty when there is no `[global]` section. The editor pairs
    /// these with [`keydviz_core::GLOBAL_OPTIONS`] to render the options form, and shows
    /// any key not in that table as a generic row so nothing in the file is hidden.
    pub fn global_entries(&self) -> Vec<(String, String)> {
        let Some(section) = self.edit.section("global") else { return Vec::new() };
        let mut out: Vec<(String, String)> = Vec::new();
        for e in &section.entries {
            if let EntryKind::Binding { key, val: Some(v), .. } = &e.kind {
                // last-wins: a later assignment to the same key replaces the earlier.
                out.retain(|(k, _)| k != key);
                out.push((key.clone(), v.clone()));
            }
        }
        out
    }

    /// Set a `[global]` option `name = value`, creating the `[global]` section if the
    /// config has none. An empty `value` clears the option (removes its line → keyd
    /// falls back to the built-in default). `Err` on an empty name.
    pub fn set_global(&mut self, name: &str, value: &str) -> Result<(), String> {
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() {
            return Err("missing option name".into());
        }
        if value.is_empty() {
            self.clear_global(name);
            return Ok(());
        }
        self.edit.global_section_mut().set_or_add_binding(name, value);
        Ok(())
    }

    /// Remove a `[global]` option (every assignment of it), so keyd uses its default.
    /// A no-op when the option (or the `[global]` section) isn't present.
    pub fn clear_global(&mut self, name: &str) -> bool {
        match self.edit.section_mut("global") {
            Some(s) => s.remove_binding(name.trim()),
            None => false,
        }
    }

    /// Create a new empty layer `[name]` and return its canonical (trimmed) name so
    /// the caller can select it. `Err` names why it was rejected (empty, bad chars,
    /// reserved special, or a duplicate base name). See
    /// [`keydviz_core::edit::EditConfig::add_layer`].
    pub fn add_layer(&mut self, name: &str) -> Result<String, String> {
        self.edit.add_layer(name)?;
        Ok(name.trim().to_string())
    }

    /// Delete the layer `base` (every section that defines it). `Err` when no such
    /// layer exists. Bindings elsewhere that still point at it become orphans (the
    /// warning panel surfaces them); see [`Self::references_to`] for the pre-delete
    /// heads-up.
    pub fn remove_layer(&mut self, base: &str) -> Result<(), String> {
        if !self.edit.remove_layer(base) {
            return Err(format!("this config has no [{base}] layer"));
        }
        Ok(())
    }

    /// Where `layer` is still referenced — `"<key> in [<section>]"` per offending
    /// binding — so the UI can warn before a delete drops it. Empty when nothing
    /// points at it (a clean delete).
    pub fn references_to(&self, layer: &str) -> Vec<String> {
        self.edit
            .references_to(layer)
            .into_iter()
            .map(|(section, key)| format!("{key} in [{section}]"))
            .collect()
    }

    /// Rename layer `old_base` to `new_name`, following every reference (so nothing
    /// orphans), and return its canonical (trimmed) new name so the caller can reselect
    /// it. `Err` names why it was rejected (bad name, unchanged, not a renameable layer,
    /// or a name already in use). See
    /// [`keydviz_core::edit::EditConfig::rename_layer`].
    pub fn rename_layer(&mut self, old_base: &str, new_name: &str) -> Result<String, String> {
        self.edit.rename_layer(old_base, new_name)?;
        Ok(new_name.trim().to_string())
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

    /// Warnings for bindings that activate a layer this config never defines —
    /// one line per missing layer, naming where it's referenced (capped). Empty
    /// when the config is clean; recomputed after every edit so creating the layer
    /// (or removing the reference) clears it live. See
    /// [`keydviz_core::edit::EditConfig::orphan_layer_refs`].
    pub fn orphan_warnings(&self) -> Vec<String> {
        let mut groups: Vec<(String, Vec<String>)> = Vec::new();
        for o in self.edit.orphan_layer_refs() {
            let site = format!("{} in [{}]", o.key, o.section);
            match groups.iter_mut().find(|(l, _)| *l == o.layer) {
                Some((_, sites)) => sites.push(site),
                None => groups.push((o.layer, vec![site])),
            }
        }
        groups
            .into_iter()
            .map(|(layer, sites)| {
                let shown = sites.len().min(3);
                let more = sites.len() - shown;
                let tail = if more > 0 { format!(" (+{more} more)") } else { String::new() };
                format!(
                    "\u{26a0} no [{layer}] layer \u{2014} referenced by {}{tail}",
                    sites[..shown].join(", ")
                )
            })
            .collect()
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
        // A not-yet-applied new config has content to persist even before any edit.
        self.created || self.edit.is_dirty()
    }

    /// This session is creating a config that isn't on disk yet (`true` only between
    /// [`Self::create`] and the first successful apply, after which the session is
    /// re-opened on the now-existing file). Lets the caller treat a never-persisted
    /// new config as a removable phantom board on exit, rather than re-deriving it
    /// from a file that doesn't exist.
    pub fn is_new(&self) -> bool {
        self.created
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
    fn tap_hold_new_key_uses_feel_default() {
        let td = TempDir::new("th-new");
        let mut s = session(&td);
        // capslock currently = esc; make it a Responsive tap esc / hold nav.
        s.set_tap_hold("main", "capslock", "nav", Some("esc".into()), Some(Behavior::Responsive))
            .unwrap();
        assert!(s.dirty());
        assert_eq!(s.diff(), "- capslock = esc\n+ capslock = overloadt2(nav, esc, 200)\n");
        let th = s.current_tap_hold("main", "capslock").unwrap();
        assert_eq!(th.target, "nav");
        assert_eq!(th.tap.as_deref(), Some("esc"));
        assert_eq!(th.behavior(), Some(Behavior::Responsive));
    }

    #[test]
    fn tap_hold_edit_preserves_lettermod_timeouts() {
        // The hand-tuned hhkb case: editing within the same feel must keep 150/200.
        let td = TempDir::new("th-edit");
        let p = td.0.join("test.conf");
        std::fs::write(&p, "[ids]\n*\n\n[main]\nf = lettermod(nav, f, 150, 200)\n\n[nav]\nh = left\n[num]\nj = 1\n").unwrap();
        let mut s = EditSession::open(&p).unwrap();
        // The reader decomposes the existing lettermod into slots.
        let cur = s.current_tap_hold("main", "f").unwrap();
        assert_eq!(cur.func, "lettermod");
        assert_eq!(cur.behavior(), Some(Behavior::TypingSafe));
        // Repoint the hold from nav to num at the same feel; timings survive.
        s.set_tap_hold("main", "f", "num", Some("f".into()), Some(Behavior::TypingSafe)).unwrap();
        assert_eq!(s.diff(), "- f = lettermod(nav, f, 150, 200)\n+ f = lettermod(num, f, 150, 200)\n");
    }

    #[test]
    fn tap_hold_momentary_has_no_tap() {
        let td = TempDir::new("th-mom");
        let mut s = session(&td);
        s.set_tap_hold("main", "capslock", "nav", None, Some(Behavior::Responsive)).unwrap();
        assert_eq!(s.diff(), "- capslock = esc\n+ capslock = layer(nav)\n");
        let th = s.current_tap_hold("main", "capslock").unwrap();
        assert_eq!(th.tap, None);
    }

    #[test]
    fn tap_hold_refuses_to_clobber_an_overloadi() {
        // overloadi is badged as a tap/hold by the viewer but can't be decomposed by
        // the panel — re-setting it must NOT silently discard the original. The user
        // edits it as raw text in simple mode (set_binding) instead.
        let td = TempDir::new("th-overloadi");
        let p = td.0.join("test.conf");
        let orig = "overloadi(a, overloadt2(nav, a, 500), 200)";
        std::fs::write(&p, format!("[ids]\n*\n\n[main]\na = {orig}\n\n[nav]\nh = left\n")).unwrap();
        let mut s = EditSession::open(&p).unwrap();
        // The panel can't read it (None) — that's exactly the trap.
        assert!(s.current_tap_hold("main", "a").is_none());
        // Applying the tap/hold panel over it is refused, with a steer to simple mode.
        let err = s.set_tap_hold("main", "a", "num", Some("a".into()), Some(Behavior::Responsive))
            .unwrap_err();
        assert!(err.contains("overloadi") && err.contains("simple"), "{err}");
        // The original is untouched and the session stays clean.
        assert_eq!(s.current_binding("main", "a").as_deref(), Some(orig));
        assert!(!s.dirty());
        // Editing it as raw text in simple mode still works (the supported path).
        s.set_binding("main", "a", "b").unwrap();
        assert_eq!(s.current_binding("main", "a").as_deref(), Some("b"));
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
    fn bind_on_a_config_without_main_creates_it() {
        // A config whose [main] lives entirely in an include (or that has no [main]
        // at all) still shows the base board — binding a key must create the [main]
        // section rather than erroring "no [main]" (the include-scan test scenario).
        let td = TempDir::new("nomain-bind");
        let p = td.0.join("inc.conf");
        std::fs::write(&p, "[ids]\n*\n\ninclude shared\n").unwrap();
        let mut s = EditSession::open(&p).unwrap();
        s.set_binding("main", "a", "b").unwrap();
        assert_eq!(s.current_binding("main", "a").as_deref(), Some("b"));
        let out = s.serialized();
        assert!(out.contains("[main]"), "appended a [main] section");
        assert!(out.contains("include shared"), "preserved the include directive");
        // tap/hold takes the same path.
        s.set_tap_hold("main", "f", "nav", Some("f".into()), None).unwrap();
        assert!(s.current_binding("main", "f").is_some());
    }

    #[test]
    fn add_layer_then_edit_it() {
        let td = TempDir::new("addlayer");
        let mut s = session(&td);
        // SRC has [ids], [main], [nav]; create a fresh [sym].
        assert_eq!(s.add_layer("sym").unwrap(), "sym");
        assert!(s.dirty());
        // It joins the editable chooser and accepts a binding.
        assert!(s.editable_sections().contains(&"sym".to_string()));
        s.set_binding("sym", "a", "b").unwrap();
        assert_eq!(s.current_binding("sym", "a").as_deref(), Some("b"));
        // Bad names and duplicates surface a reason, not a panic.
        assert!(s.add_layer("sym").unwrap_err().contains("exists"));
        assert!(s.add_layer("a b").unwrap_err().contains("letters"));
    }

    #[test]
    fn add_layer_clears_the_orphan_warning_live() {
        let td = TempDir::new("addlayer-orphan");
        let p = td.0.join("test.conf");
        std::fs::write(&p, "[ids]\n*\n[main]\ncapslock = layer(sym)\n").unwrap();
        let mut s = EditSession::open(&p).unwrap();
        assert_eq!(s.orphan_warnings().len(), 1);
        s.add_layer("sym").unwrap();
        // Defining the layer resolves the dangling reference.
        assert!(s.orphan_warnings().is_empty());
    }

    #[test]
    fn remove_layer_and_its_references() {
        let td = TempDir::new("rmlayer");
        let p = td.0.join("test.conf");
        std::fs::write(&p, "[ids]\n*\n[main]\ncapslock = layer(nav)\n[nav]\nh = left\n").unwrap();
        let mut s = EditSession::open(&p).unwrap();
        // Before deleting: the heads-up names where nav is still used.
        assert_eq!(s.references_to("nav"), vec!["capslock in [main]".to_string()]);
        s.remove_layer("nav").unwrap();
        assert!(s.dirty());
        assert!(!s.editable_sections().contains(&"nav".to_string()));
        // The now-dangling layer(nav) becomes an orphan warning (honest, not silent).
        assert_eq!(s.orphan_warnings().len(), 1);
        // Deleting a layer that doesn't exist is a named error.
        assert!(s.remove_layer("nope").unwrap_err().contains("[nope]"));
    }

    #[test]
    fn rename_layer_follows_references_and_clears_orphans() {
        let td = TempDir::new("rnlayer");
        let p = td.0.join("test.conf");
        std::fs::write(&p, "[ids]\n*\n[main]\ncapslock = layer(nav)\n[nav]\nh = left\n").unwrap();
        let mut s = EditSession::open(&p).unwrap();
        assert_eq!(s.rename_layer("nav", "symbols").unwrap(), "symbols");
        assert!(s.dirty());
        assert!(s.editable_sections().contains(&"symbols".to_string()));
        assert!(!s.editable_sections().contains(&"nav".to_string()));
        // The reference followed the rename — no orphan, and the binding now points at it.
        assert!(s.orphan_warnings().is_empty());
        assert_eq!(s.current_binding("main", "capslock").as_deref(), Some("layer(symbols)"));
        // Renaming a non-layer / missing base is a named error.
        assert!(s.rename_layer("main", "base").is_err());
    }

    #[test]
    fn create_new_config_is_editable_and_dirty_from_the_start() {
        let td = TempDir::new("create");
        // The target file does NOT exist — create() never reads it.
        let path = td.0.join("newboard.conf");
        let mut s = EditSession::create(&path, &["04fe:0021"]).unwrap();
        // A fresh config has content to persist even before any edit.
        assert!(s.dirty());
        assert_eq!(s.serialized(), "[ids]\n04fe:0021\n\n[main]\n");
        // The starter derives the chosen id and an empty, editable [main].
        assert_eq!(s.config().ids, vec!["04fe:0021".to_string()]);
        assert_eq!(s.editable_sections(), vec!["main".to_string()]);
        // Editing the new config works like any other session.
        s.set_binding("main", "capslock", "esc").unwrap();
        assert_eq!(s.current_binding("main", "capslock").as_deref(), Some("esc"));
        // The diff shows the whole new file as additions (original is empty).
        assert_eq!(s.diff(), "+ [ids]\n+ 04fe:0021\n+ \n+ [main]\n+ capslock = esc\n");
    }

    #[test]
    fn create_wildcard_and_apply_target() {
        let td = TempDir::new("create-wild");
        let path = td.0.join("default.conf");
        let s = EditSession::create(&path, &["*"]).unwrap();
        assert_eq!(s.serialized(), "[ids]\n*\n\n[main]\n");
        // It's a one-click apply candidate (right dir, allow-listed name) even though
        // the file doesn't exist yet — the tool's Absent path creates it.
        assert_eq!(s.apply_target(&td.0).as_deref(), Some("default"));
        // No stale warning for a not-yet-existing file.
        assert!(s.stale_warning().is_none());
    }

    #[test]
    fn create_save_draft_writes_the_starter() {
        let td = TempDir::new("create-draft");
        let path = td.0.join("mine.conf");
        let mut s = EditSession::create(&path, &["dead:beef"]).unwrap();
        s.set_binding("main", "a", "b").unwrap();
        let saved = s.save_draft_to(&td.0.join("drafts")).unwrap();
        let body = std::fs::read_to_string(&saved.draft_path).unwrap();
        assert_eq!(body, "[ids]\ndead:beef\n\n[main]\na = b\n");
        assert!(saved.install_steps.contains("mine.conf"));
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
    fn chords_lists_all_main_chords() {
        let td = TempDir::new("chord-list");
        let p = td.0.join("test.conf");
        std::fs::write(&p, "[ids]\n*\n\n[main]\nj+k = esc\nx+c = toggle(game)\n").unwrap();
        let s = EditSession::open(&p).unwrap();
        assert_eq!(
            s.chords("main"),
            vec![
                ("j+k".to_string(), "esc".to_string()),
                ("x+c".to_string(), "toggle(game)".to_string()),
            ]
        );
    }

    #[test]
    fn set_chord_appends_then_edits_in_canonical_order() {
        let td = TempDir::new("chord-set");
        let mut s = session(&td);
        // First time: a new line is appended to [main].
        s.set_chord("main", "j", "k", "esc").unwrap();
        assert!(s.dirty());
        assert_eq!(s.chords("main"), vec![("j+k".to_string(), "esc".to_string())]);
        // Editing the reversed spelling rewrites the SAME line (LHS preserved, value changed).
        s.set_chord("main", "k", "j", "tab").unwrap();
        let chords = s.chords("main");
        assert_eq!(chords, vec![("j+k".to_string(), "tab".to_string())], "one line, rewritten");
        assert!(s.serialized().contains("j+k = tab"));
        assert!(!s.serialized().contains("k+j"), "no duplicate reversed line");
    }

    #[test]
    fn set_chord_rejects_bad_input() {
        let td = TempDir::new("chord-bad");
        let mut s = session(&td);
        assert!(s.set_chord("main", "j", "j", "esc").unwrap_err().contains("different"));
        assert!(s.set_chord("main", "j", "k", "  ").unwrap_err().contains("action"));
        // No [main] section at all → setting a chord creates one (mirrors set_binding).
        let p = td.0.join("nomain.conf");
        std::fs::write(&p, "[ids]\n*\n\n[nav]\nh = left\n").unwrap();
        let mut s2 = EditSession::open(&p).unwrap();
        s2.set_chord("main", "j", "k", "esc").unwrap();
        assert_eq!(s2.chords("main"), vec![("j+k".to_string(), "esc".to_string())]);
        assert!(s2.serialized().contains("[main]"));
    }

    #[test]
    fn remove_chord_clears_either_spelling() {
        let td = TempDir::new("chord-rm");
        let p = td.0.join("test.conf");
        std::fs::write(&p, "[ids]\n*\n\n[main]\nj+k = esc\n").unwrap();
        let mut s = EditSession::open(&p).unwrap();
        // Remove via the reversed spelling — canonical match still finds it.
        s.remove_chord("main", "k+j").unwrap();
        assert!(s.dirty());
        assert!(s.chords("main").is_empty());
        assert!(!s.serialized().contains("j+k"));
        // Removing a chord that isn't there is a no-op; no [main] is an error.
        let mut s2 = EditSession::open(&p).unwrap();
        s2.remove_chord("main", "a+b").unwrap();
        let p3 = td.0.join("nomain.conf");
        std::fs::write(&p3, "[ids]\n*\n\n[nav]\nh = left\n").unwrap();
        let mut s3 = EditSession::open(&p3).unwrap();
        assert!(s3.remove_chord("main", "j+k").unwrap_err().contains("[main]"));
    }

    #[test]
    fn toggle_chord_flows_to_config_and_orphan_warning() {
        let td = TempDir::new("chord-toggle");
        let mut s = session(&td); // SRC defines [nav]
        // A toggle chord onto an existing layer lands in config().chords, no orphan.
        s.set_chord("main", "leftshift", "rightshift", "toggle(nav)").unwrap();
        assert!(s
            .config()
            .chords
            .contains(&("leftshift+rightshift".to_string(), "nav".to_string())));
        assert!(s.orphan_warnings().is_empty());
        // Retargeting it at a missing layer raises an orphan warning (the value, not
        // the '+'-joined key, is what the orphan scan reads).
        s.set_chord("main", "leftshift", "rightshift", "toggle(missing)").unwrap();
        assert!(s.orphan_warnings().iter().any(|w| w.contains("missing")));
    }

    #[test]
    fn chords_are_layer_scoped() {
        // A chord set on [nav] lands in [nav], is listed only for "nav" (not "main"),
        // round-trips into that section, and removes from it — proving the chord ops
        // honor the layer parameter rather than always hitting [main].
        let td = TempDir::new("chord-layer");
        let mut s = session(&td); // SRC defines [main] + [nav]
        s.set_chord("nav", "h", "l", "esc").unwrap();
        assert_eq!(s.chords("nav"), vec![("h+l".to_string(), "esc".to_string())]);
        assert!(s.chords("main").is_empty(), "the nav chord must not leak into main");
        // Serialized under the [nav] header, and derive() routes it to that layer's combos.
        assert!(s.serialized().contains("h+l = esc"));
        let nav = s.config().layers.iter().find(|l| l.name == "nav").unwrap().clone();
        assert!(nav.combos.contains(&("h+l".to_string(), "esc".to_string())));
        // Remove scoped to nav clears it; main stays untouched.
        s.remove_chord("nav", "l+h").unwrap();
        assert!(s.chords("nav").is_empty());
        assert!(!s.serialized().contains("h+l"));
    }

    #[test]
    fn set_global_creates_section_then_edits_and_clears() {
        let td = TempDir::new("global");
        // SRC has no [global]; setting an option creates one (appended).
        let mut s = session(&td);
        assert!(s.global_entries().is_empty());
        s.set_global("layer_indicator", "1").unwrap();
        assert!(s.dirty());
        assert_eq!(s.global_entries(), vec![("layer_indicator".to_string(), "1".to_string())]);
        assert!(s.serialized().contains("[global]"));
        assert!(s.serialized().contains("layer_indicator = 1"));
        // Re-setting rewrites in place (no duplicate line).
        s.set_global("layer_indicator", "0").unwrap();
        assert_eq!(s.global_entries(), vec![("layer_indicator".to_string(), "0".to_string())]);
        // Empty value clears the option back to the keyd default.
        s.set_global("layer_indicator", "").unwrap();
        assert!(s.global_entries().is_empty());
        assert!(!s.serialized().contains("layer_indicator"));
        // Missing name is rejected; clearing an absent option is a no-op.
        assert!(s.set_global("", "1").unwrap_err().contains("name"));
        assert!(!s.clear_global("oneshot_timeout"));
    }

    #[test]
    fn global_entries_reads_existing_section_last_wins() {
        let td = TempDir::new("global2");
        let p = td.0.join("test.conf");
        std::fs::write(
            &p,
            "[global]\nmacro_timeout = 600\nlayer_indicator = 1\nmacro_timeout = 400\n\n[main]\na = b\n",
        )
        .unwrap();
        let s = EditSession::open(&p).unwrap();
        // Duplicate key collapses to the last assignment, order otherwise preserved.
        assert_eq!(
            s.global_entries(),
            vec![
                ("layer_indicator".to_string(), "1".to_string()),
                ("macro_timeout".to_string(), "400".to_string()),
            ]
        );
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

    #[test]
    fn macro_round_trips_through_the_session() {
        use keydviz_core::MacroToken;
        let td = TempDir::new("macro-rt");
        let mut s = session(&td);
        let mac = Macro {
            tokens: vec![
                MacroToken::Chord { mods: vec!['C'], keys: vec!["t".into()] },
                MacroToken::Delay(100),
                MacroToken::Text("google.com".into()),
                MacroToken::Key("enter".into()),
            ],
            repeat: None,
        };
        s.set_macro("main", "capslock", &mac).unwrap();
        assert_eq!(
            s.current_binding("main", "capslock").as_deref(),
            Some("macro(C-t 100ms google.com enter)")
        );
        // Read back as a structured macro.
        assert_eq!(s.current_macro("main", "capslock"), Some(mac));
    }

    #[test]
    fn set_macro_creates_layer_section_and_supports_macro2() {
        let td = TempDir::new("macro-mk");
        let mut s = session(&td);
        // `nav` has no local [nav] for a new key here? It does (h = left), so use it.
        let mac = Macro {
            tokens: vec![keydviz_core::MacroToken::Key("space".into())],
            repeat: Some((400, 50)),
        };
        s.set_macro("nav", "j", &mac).unwrap();
        assert_eq!(
            s.current_binding("nav", "j").as_deref(),
            Some("macro2(400, 50, macro(space))")
        );
    }

    #[test]
    fn current_macro_none_for_non_macro() {
        let td = TempDir::new("macro-none");
        let s = session(&td);
        assert!(s.current_macro("main", "capslock").is_none()); // = esc, a plain remap
        assert!(s.current_macro("main", "nonexistent").is_none());
    }

    #[test]
    fn set_macro_refuses_empty_and_literal_paren() {
        use keydviz_core::MacroToken;
        let td = TempDir::new("macro-guard");
        let mut s = session(&td);
        // Empty.
        assert!(s.set_macro("main", "capslock", &Macro { tokens: vec![], repeat: None }).is_err());
        // A literal '(' in text can't be represented — refuse, don't write garbage.
        let bad = Macro {
            tokens: vec![MacroToken::Text("a(b".into())],
            repeat: None,
        };
        assert!(s.set_macro("main", "capslock", &bad).is_err());
        // The original binding is untouched.
        assert_eq!(s.current_binding("main", "capslock").as_deref(), Some("esc"));
    }

    #[test]
    fn set_macro_refuses_to_clobber_an_unmodelable_macro() {
        // A nested macro keyd accepts but we don't decompose: editing must refuse so
        // we never silently rewrite (and lose) the original.
        let td = TempDir::new("macro-clobber");
        let p = td.0.join("test.conf");
        std::fs::write(&p, "[ids]\n*\n[main]\na = macro(b macro(c))\n").unwrap();
        let mut s = EditSession::open(&p).unwrap();
        assert!(s.current_macro("main", "a").is_none()); // not decomposable
        let new = Macro {
            tokens: vec![keydviz_core::MacroToken::Key("x".into())],
            repeat: None,
        };
        assert!(s.set_macro("main", "a", &new).is_err());
        assert_eq!(s.current_binding("main", "a").as_deref(), Some("macro(b macro(c))"));
    }
}
