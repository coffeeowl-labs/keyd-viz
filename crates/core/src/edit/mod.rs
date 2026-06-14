//! Line-faithful keyd config model for Edit Mode (Phase 6 E0, design doc §5.1).
//!
//! keyd's config grammar is strictly line-oriented (`ini.c`: `ini_parse_string` splits
//! only on `\n`; there are no multi-line constructs), so the line is the atom. This
//! model stores **every source line verbatim** plus a typed overlay for the lines we
//! understand; [`EditConfig::serialize`] replays `raw + eol` for every untouched line
//! and regenerates only lines the user edited. Round-trip fidelity
//! (`serialize(parse(f)) == f`) is therefore identity-by-construction — the
//! [`round_trips`] gate is a model-soundness self-check, not the thing preventing
//! data loss.
//!
//! Grammar parity is with keyd 2.6.0 @ `f564288` (`src/ini.c`), verified line-by-line:
//!   - lines are split on `\n`; leading/trailing C-`isspace` is trimmed before
//!     classification (so a stray `\r` is trailing whitespace to keyd — we keep it in
//!     `raw` regardless);
//!   - a trimmed line that starts with `[` **and** ends with `]` is a section header,
//!     and the name is the full inner text verbatim (`[a]b]` names the section `a]b`;
//!     `[main ]` names `main ` — *not* `main`); a `[foo` without the closing bracket
//!     falls through and is parsed as a key-value entry;
//!   - `#` starts a comment only as the first non-whitespace character, and is checked
//!     *after* the header case;
//!   - everything else is a key-value entry per [`parse_kvp`]: the key may itself be
//!     `=` (leading-`=` special case), the trailing space/tab run before the first `=`
//!     is trimmed off the key, leading spaces/tabs after it are skipped, and a line
//!     with no `=` is a *valueless* entry (kept, `val = None`).
//!
//! This deliberately does **not** use `str::lines()`, which silently eats `\r` and the
//! final-newline distinction — the one real way to break round-trip (§5.1).

mod cst;
mod refs;
pub use cst::*;
pub use refs::*;
/// How a source line was terminated. Preserved per line so CRLF files and files
/// without a final newline serialize back byte-identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Eol {
    Lf,
    CrLf,
    /// Last line of a file with no trailing newline.
    None,
}

impl Eol {
    pub fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "\n",
            Eol::CrLf => "\r\n",
            Eol::None => "",
        }
    }
}

/// The typed overlay for a binding line — the actions the *editor* models. Everything
/// else is [`Typed::Raw`] and renders/round-trips as its verbatim text (never a generic
/// "advanced" placeholder). The variant set grows with E1/E2 breadth; classification
/// here is deliberately conservative — when in doubt, `Raw` (view-only, never clobbered).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
    /// A plain key-to-key remap (`a = b`).
    Remap(String),
    /// `noop` — the key is disabled.
    Noop,
    /// Anything we don't (yet) model. The verbatim `raw` line is the source of truth.
    Raw,
}

/// One source line, verbatim, plus what we understood of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The original line, byte-for-byte, without its line terminator.
    pub raw: String,
    pub eol: Eol,
    pub kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryKind {
    /// Empty or all-whitespace line.
    Blank,
    /// `#`-comment (first non-whitespace char is `#`).
    Comment,
    /// A section header line; owned by [`Section::header`].
    Header,
    /// A key-value (or valueless) entry, split per keyd's `parse_kvp`.
    Binding {
        key: String,
        /// `None` when the line has no `=` (keyd keeps such entries, e.g. `[ids]` lines).
        val: Option<String>,
        typed: Typed,
        /// True once the editor regenerated this line (it no longer matches `raw`'s
        /// original bytes' provenance — `raw` *is* kept in sync on edit).
        dirty: bool,
    },
}

/// What a section *is* to keyd. `ids`/`global`/`aliases` are exact-match specials
/// (`config.c` compares the verbatim section name, so `[ids ]` is **not** `[ids]`);
/// every other section is a layer, with `main` and composite (`[a+b]`) called out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    Ids,
    Global,
    Aliases,
    Main,
    /// `[a+b]` — a composite layer over its constituents.
    Composite,
    Layer,
}

impl SectionKind {
    /// True for the layer-bearing kinds (`Main` / `Layer` / `Composite`) that render as
    /// a board, as opposed to the `[ids]`/`[global]`/`[aliases]` exact-match specials.
    pub fn is_board(self) -> bool {
        matches!(self, SectionKind::Main | SectionKind::Layer | SectionKind::Composite)
    }
}

/// A `[...]` section: its header line plus its body lines, all order-preserving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The header line itself (kind == [`EntryKind::Header`]).
    pub header: Entry,
    /// The full text inside the brackets, verbatim (qualifiers included: `nav:C`).
    pub name: String,
    pub kind: SectionKind,
    pub entries: Vec<Entry>,
    /// Set when a binding was *removed* from this section. Edits and adds mark the
    /// surviving entry dirty, but a removal leaves no entry to flag — so this
    /// captures it, and [`EditConfig::is_dirty`] ORs it in.
    pub dirty: bool,
}

impl Section {
    /// The section name before any `:` qualifier (`nav:C` → `nav`).
    pub fn base_name(&self) -> &str {
        self.name.split(':').next().unwrap_or(&self.name)
    }

    /// True if this section feeds the board named `layer` — a board-bearing section
    /// (see [`SectionKind::is_board`]) whose base name is `layer`. keyd merges every
    /// such section into one board last-wins, so `[nav]` and `[nav:C]` both feed "nav".
    pub fn feeds_board(&self, layer: &str) -> bool {
        self.kind.is_board() && self.base_name().trim() == layer
    }

    /// The `:` qualifier, if any (`nav:C` → `C`; `mylay:layout` → `layout`).
    pub fn qualifier(&self) -> Option<&str> {
        self.name.split_once(':').map(|(_, q)| q)
    }

    /// The current value bound to `key` in this section (last duplicate wins,
    /// like keyd's sequential application).
    pub fn get_binding(&self, key: &str) -> Option<&str> {
        self.entries.iter().rev().find_map(|e| match &e.kind {
            EntryKind::Binding { key: k, val, .. } if k == key => val.as_deref(),
            _ => None,
        })
    }

    /// Set `key = value` in this section: rewrite the last existing binding for
    /// `key`, or append a new line when none exists. The append lands after the
    /// last non-blank entry (so a trailing blank-line separator stays at the
    /// section's end) in `style` — the **file's** line-ending, which the caller
    /// passes because a section can't see it (a freshly-appended `[layer]` header
    /// may carry `Eol::None`, so its own EOL is no guide).
    pub fn set_or_add_binding(&mut self, key: &str, new_val: &str, style: Eol) {
        if !self.set_binding(key, new_val) {
            self.push_binding(key, new_val, style);
        }
    }

    /// Append a fresh `key = value` binding line to this section in the file's
    /// `style` line-ending (passed in — see [`Self::set_or_add_binding`]).
    fn push_binding(&mut self, key: &str, new_val: &str, style: Eol) {
        let at = self
            .entries
            .iter()
            .rposition(|e| !matches!(e.kind, EntryKind::Blank))
            .map_or(0, |i| i + 1);
        let mut eol = style;
        if at == self.entries.len() {
            // Appending at the very end: inherit the terminal entry's (or, for an
            // empty section, the header's) EOL state so a file without a final
            // newline stays that way — the prior last line gains a newline, the
            // new last line takes over the `None`.
            let prev_eol = match self.entries.last_mut() {
                Some(prev) => &mut prev.eol,
                None => &mut self.header.eol,
            };
            if *prev_eol == Eol::None {
                *prev_eol = style;
                eol = Eol::None;
            }
        }
        let typed = classify(self.kind, Some(new_val));
        self.entries.insert(
            at,
            Entry {
                raw: format!("{key} = {new_val}"),
                eol,
                kind: EntryKind::Binding {
                    key: key.to_string(),
                    val: Some(new_val.to_string()),
                    typed,
                    dirty: true,
                },
            },
        );
    }

    /// Replace the value of the **last** binding for `key` (keyd applies entries in
    /// order, so the last assignment wins) and regenerate that one line as
    /// `key = value`. Every other line in the file is untouched. Returns `false` if
    /// no binding for `key` exists.
    pub fn set_binding(&mut self, key: &str, new_val: &str) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .rev()
            .find(|e| matches!(&e.kind, EntryKind::Binding { key: k, .. } if k == key))
        else {
            return false;
        };
        let EntryKind::Binding { key, val, typed, dirty } = &mut entry.kind else {
            unreachable!("find matched a Binding");
        };
        entry.raw = format!("{key} = {new_val}");
        *val = Some(new_val.to_string());
        *typed = classify(self.kind, Some(new_val));
        *dirty = true;
        true
    }

    /// Remove **every** binding line for `key` from this section, making the key
    /// transparent (it falls through to the base layer — keyd's default for an
    /// unbound key). All duplicates must go: keyd applies entries in order, so a
    /// single leftover assignment would keep the key bound. Comments and blank
    /// lines are left untouched (we don't guess which ones "belonged" to the key).
    /// Marks the section dirty and returns whether anything was removed.
    pub fn remove_binding(&mut self, key: &str) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|e| !matches!(&e.kind, EntryKind::Binding { key: k, .. } if k == key));
        let removed = self.entries.len() != before;
        self.dirty |= removed;
        removed
    }

    /// Index of the **last** `# keyd-viz: <key> = …` label comment for `key` in this
    /// section. Last to mirror `derive`'s within-section last-wins (the deriver scans
    /// entries in order and the final `push_label` wins), so a rewrite updates the
    /// effective label, not a shadowed earlier one.
    fn label_index(&self, key: &str) -> Option<usize> {
        self.entries.iter().rposition(|e| {
            matches!(e.kind, EntryKind::Comment)
                && parse_label_comment(&e.raw).is_some_and(|(k, _)| k == key)
        })
    }

    /// Set (or replace) the custom-label comment for `key` in this section.
    ///
    /// If a label comment for `key` already exists, its line is rewritten in place
    /// (position preserved). Otherwise a fresh `# keyd-viz: key = text` line is
    /// inserted *immediately before* the **last-wins** binding for `key` (so the
    /// label sits with the effective binding), copying that binding's `Eol`. If the
    /// key has no binding in this section, the comment is appended at the section's
    /// end (orphan-tolerant), taking the file's `style` line-ending. Empty `text`
    /// clears the label instead. Marks the section dirty.
    pub fn set_label(&mut self, key: &str, text: &str, style: Eol) {
        let text = c_trim(text);
        if text.is_empty() {
            self.clear_label(key);
            return;
        }
        // A label is exactly one comment line. An embedded newline would split it into
        // a second physical line that re-parses as a bogus binding (keyd rejects it), so
        // collapse any interior CR/LF — set_label is a public API, not just GUI-fed.
        let raw = label_comment_line(key, &text.replace(['\n', '\r'], " "));
        if let Some(i) = self.label_index(key) {
            // Skip an identical rewrite: re-clicking "set" with unchanged text must not
            // mark the config dirty (a byte-identical "unsaved change" reads as a bug).
            if self.entries[i].raw != raw {
                self.entries[i].raw = raw;
                self.dirty = true;
            }
            return;
        }
        // Insert before the last-wins binding (same target `set_binding` rewrites).
        let binding_at = self
            .entries
            .iter()
            .rposition(|e| matches!(&e.kind, EntryKind::Binding { key: k, .. } if k == key));
        match binding_at {
            Some(at) => {
                let eol = self.entries[at].eol;
                self.entries.insert(at, Entry { raw, eol, kind: EntryKind::Comment });
            }
            None => self.push_comment(raw, style),
        }
        self.dirty = true;
    }

    /// Remove **every** label comment for `key` from this section. All copies go (a
    /// stale duplicate could otherwise win in `derive`). Marks the section dirty and
    /// returns whether anything was removed.
    pub fn clear_label(&mut self, key: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| {
            !(matches!(e.kind, EntryKind::Comment)
                && parse_label_comment(&e.raw).is_some_and(|(k, _)| k == key))
        });
        let removed = self.entries.len() != before;
        self.dirty |= removed;
        removed
    }

    /// Append a comment line at the section's end, after the last non-blank entry (so
    /// a trailing blank separator stays last), in `style` — preserving a missing final
    /// newline exactly as [`Self::push_binding`] does.
    fn push_comment(&mut self, raw: String, style: Eol) {
        let at = self
            .entries
            .iter()
            .rposition(|e| !matches!(e.kind, EntryKind::Blank))
            .map_or(0, |i| i + 1);
        let mut eol = style;
        if at == self.entries.len() {
            let prev_eol = match self.entries.last_mut() {
                Some(prev) => &mut prev.eol,
                None => &mut self.header.eol,
            };
            if *prev_eol == Eol::None {
                *prev_eol = style;
                eol = Eol::None;
            }
        }
        self.entries.insert(at, Entry { raw, eol, kind: EntryKind::Comment });
    }
}

/// A whole config file as an ordered list of verbatim lines: anything before the
/// first section header (`preamble` — only blanks/comments in a file keyd accepts),
/// then the sections in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditConfig {
    pub preamble: Vec<Entry>,
    pub sections: Vec<Section>,
    /// Set when a whole **section** was added or removed. Binding edits flag the
    /// surviving entry, and a binding removal flags the owning [`Section::dirty`] —
    /// but creating or deleting a section leaves no per-entry/per-section flag to
    /// catch, so structural changes are recorded here and [`Self::is_dirty`] ORs it in.
    pub dirty: bool,
}

impl EditConfig {
    /// Parse config text into the line-faithful model. Infallible: every input is
    /// representable (keyd-invalid constructs land as `Raw`/preamble entries — the
    /// *editor* decides what is editable, the model never drops a byte).
    pub fn parse(text: &str) -> EditConfig {
        let mut cfg = EditConfig::default();
        let mut rest = text;

        while !rest.is_empty() {
            let (raw, eol, advance) = match rest.find('\n') {
                Some(i) => match rest[..i].strip_suffix('\r') {
                    Some(s) => (s, Eol::CrLf, i + 1),
                    None => (&rest[..i], Eol::Lf, i + 1),
                },
                None => (rest, Eol::None, rest.len()),
            };
            rest = &rest[advance..];

            let trimmed = c_trim(raw);
            // Header? (ini.c checks '[' before '#', so `[#x]` is a header and `#[x]`
            // is a comment; `[foo` without `]` falls through to parse_kvp.)
            if trimmed.len() >= 2 && trimmed.starts_with('[') && trimmed.ends_with(']') {
                let name = &trimmed[1..trimmed.len() - 1];
                cfg.sections.push(Section {
                    header: Entry { raw: raw.to_string(), eol, kind: EntryKind::Header },
                    name: name.to_string(),
                    kind: section_kind(name),
                    entries: Vec::new(),
                    dirty: false,
                });
                continue;
            }

            let kind = if trimmed.is_empty() {
                EntryKind::Blank
            } else if trimmed.starts_with('#') {
                EntryKind::Comment
            } else {
                let (key, val) = parse_kvp(trimmed);
                let skind = cfg.sections.last().map(|s| s.kind);
                EntryKind::Binding {
                    typed: classify_in(skind, val),
                    key: key.to_string(),
                    val: val.map(str::to_string),
                    dirty: false,
                }
            };
            let entry = Entry { raw: raw.to_string(), eol, kind };
            match cfg.sections.last_mut() {
                Some(s) => s.entries.push(entry),
                None => cfg.preamble.push(entry),
            }
        }
        cfg
    }

    /// Emit the file back out: `raw + eol` for every line, in order. For a freshly
    /// parsed model this reproduces the input byte-for-byte; after an edit, exactly
    /// the regenerated line(s) differ.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        let mut push = |e: &Entry| {
            out.push_str(&e.raw);
            out.push_str(e.eol.as_str());
        };
        self.preamble.iter().for_each(&mut push);
        for s in &self.sections {
            push(&s.header);
            s.entries.iter().for_each(&mut push);
        }
        out
    }

    /// Look up a section by its verbatim name (qualifier included).
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// Mutable [`Self::section`].
    pub fn section_mut(&mut self, name: &str) -> Option<&mut Section> {
        self.sections.iter_mut().find(|s| s.name == name)
    }

    /// The section a board edit targets: the **last** layer-bearing section whose
    /// base name is `layer` (`"main"` for the base board). Last, because keyd
    /// merges duplicate sections in order and an appended line must out-rank every
    /// earlier assignment. `None` when the config has no such section (the GUI
    /// treats those caps as not-editable in E1; creating sections is E2).
    pub fn target_section_mut(&mut self, layer: &str) -> Option<&mut Section> {
        self.sections.iter_mut().rev().find(|s| {
            s.feeds_board(layer)
        })
    }

    /// Like [`Self::target_section_mut`], but appends an empty `[main]` when the config
    /// declares no base board — so binding a key on a config whose `[main]` lives in an
    /// `include` (or that has none at all) just works instead of erroring. Only `"main"`
    /// is materialized: it's the base board the GUI always shows even when the file
    /// carries no `[main]`. A named layer is only ever shown when its section already
    /// exists, so a missing one is a bad call, not a board to create → `None`. Mirrors
    /// [`Self::global_section_mut`].
    pub fn target_or_create_section_mut(&mut self, layer: &str) -> Option<&mut Section> {
        if let Some(i) = self.sections.iter().rposition(|s| {
            s.feeds_board(layer)
        }) {
            return Some(&mut self.sections[i]);
        }
        if layer != "main" {
            return None;
        }
        Some(self.append_section("main", SectionKind::Main))
    }

    /// Make `key` transparent on the `layer` board: remove its binding from
    /// **every** layer-bearing section whose base name is `layer`. keyd merges
    /// duplicate sections (and `[nav]` / `[nav:C]` both feed the "nav" board) and
    /// applies them last-wins, so clearing only the last one would leave the key
    /// bound — all contributors must drop it for the key to fall through to base.
    /// This mirrors how the viewer reads a board across all matching sections.
    /// Returns whether anything was removed.
    pub fn clear_binding(&mut self, layer: &str, key: &str) -> bool {
        let mut removed = false;
        for s in self.sections.iter_mut().filter(|s| {
            s.feeds_board(layer)
        }) {
            removed |= s.remove_binding(key);
        }
        removed
    }

    /// Set the custom display label for `key` on the `layer` board (`"main"` for the
    /// base). Chooses the section to write to so the label lands beside the effective
    /// binding and `derive` reads it back: the last board section that already carries
    /// a label for `key` (rewrite in place), else the last that *binds* `key` (insert
    /// adjacent), else the target section (`[main]` created if the base is include-only).
    /// Empty `text` clears the label everywhere. Returns `false` only when the board
    /// doesn't exist and can't be created (a non-`main` layer with no local section).
    pub fn set_label(&mut self, layer: &str, key: &str, text: &str) -> bool {
        let style = self.file_eol();
        if c_trim(text).is_empty() {
            return self.clear_label(layer, key);
        }
        if let Some(i) =
            self.sections.iter().rposition(|s| s.feeds_board(layer) && s.label_index(key).is_some())
        {
            self.sections[i].set_label(key, text, style);
            return true;
        }
        if let Some(i) =
            self.sections.iter().rposition(|s| s.feeds_board(layer) && s.get_binding(key).is_some())
        {
            self.sections[i].set_label(key, text, style);
            return true;
        }
        match self.target_or_create_section_mut(layer) {
            Some(sec) => {
                sec.set_label(key, text, style);
                true
            }
            None => false,
        }
    }

    /// Remove the custom label for `key` from **every** section feeding the `layer`
    /// board (mirrors [`Self::clear_binding`]: merged sections all contribute, so a
    /// leftover copy could still win in `derive`). Returns whether anything changed.
    pub fn clear_label(&mut self, layer: &str, key: &str) -> bool {
        let mut removed = false;
        for s in self.sections.iter_mut().filter(|s| {
            s.feeds_board(layer)
        }) {
            removed |= s.clear_label(key);
        }
        removed
    }

    /// Bind `key = val` on the `layer` board, last-wins-aware. If any section feeding the
    /// board already binds `key`, the **last** (winning) occurrence is rewritten in place —
    /// so editing a key whose binding lives in an earlier merged section updates that line
    /// rather than appending a shadowed duplicate to the target section. Otherwise the
    /// binding is appended to the target (last matching) section, creating `[main]` when the
    /// board is include-only. New lines take the file's own line-ending. Returns `false`
    /// only when the board doesn't exist and can't be created (a non-`main` layer with no
    /// local section). The single write path for [`Self::set_or_add_binding`]'s callers.
    pub fn set_layer_binding(&mut self, layer: &str, key: &str, val: &str) -> bool {
        let style = self.file_eol();
        if let Some(i) =
            self.sections.iter().rposition(|s| s.feeds_board(layer) && s.get_binding(key).is_some())
        {
            self.sections[i].set_binding(key, val);
            return true;
        }
        match self.target_or_create_section_mut(layer) {
            Some(sec) => {
                sec.set_or_add_binding(key, val, style);
                true
            }
            None => false,
        }
    }

    /// Set a chord (`key1+key2`) binding on the `layer` board, matching existing chords by
    /// **canonical** key set so `k+j` updates an existing `j+k` (in any merged section, not
    /// just the target) instead of appending a duplicate. `canon` is the canonical form of
    /// `new_key`. Rewrites the last canonically-matching chord in place, else appends
    /// `new_key` to the target section. Same `false` / `[main]`-creation rules as
    /// [`Self::set_layer_binding`].
    pub fn set_layer_chord(&mut self, layer: &str, canon: &str, new_key: &str, val: &str) -> bool {
        let style = self.file_eol();
        // The last (file-order) section feeding the board that already holds this chord,
        // and the literal key string it's stored under (whichever order the user typed).
        let mut hit: Option<(usize, String)> = None;
        for (i, s) in self.sections.iter().enumerate().filter(|(_, s)| s.feeds_board(layer)) {
            for e in &s.entries {
                if let EntryKind::Binding { key: k, .. } = &e.kind {
                    if crate::parser::is_chord_key(k) && crate::parser::canonical_chord(k) == canon {
                        hit = Some((i, k.clone()));
                    }
                }
            }
        }
        if let Some((i, k)) = hit {
            self.sections[i].set_binding(&k, val);
            return true;
        }
        match self.target_or_create_section_mut(layer) {
            Some(sec) => {
                sec.set_or_add_binding(new_key, val, style);
                true
            }
            None => false,
        }
    }

    /// Set a `[global]` option `name = value`, appending in the file's line-ending style
    /// (creating `[global]` if absent — see [`Self::global_section_mut`]).
    pub fn set_global_option(&mut self, name: &str, value: &str) {
        let style = self.file_eol();
        self.global_section_mut().set_or_add_binding(name, value, style);
    }

    /// Create a new empty layer section `[name]` at the end of the file. `name` is a
    /// *base* layer name (no qualifier/composite) — `Err` names why it was rejected:
    /// empty, an illegal character, a reserved special (`ids`/`global`/`aliases`), or
    /// a base name that already exists (a duplicate would silently merge into the
    /// existing layer, not the fresh one the user asked for). The header lands after a
    /// blank-line separator in the file's own line-ending style, preserving a missing
    /// final newline. Marks the config dirty.
    pub fn add_layer(&mut self, name: &str) -> Result<(), String> {
        let n = name.trim();
        if n.is_empty() {
            return Err("layer name can't be empty".to_string());
        }
        if !n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            return Err("layer name: use letters, digits, '_' or '-'".to_string());
        }
        if matches!(n, "ids" | "global" | "aliases") {
            return Err(format!("[{n}] is a reserved keyd section, not a layer"));
        }
        if self.sections.iter().any(|s| s.base_name().trim() == n) {
            return Err(format!("[{n}] already exists"));
        }

        // `section_kind` classifies the name as keyd would: `main` becomes the base
        // board (`SectionKind::Main`), not a named `Layer` — so an explicitly-created
        // `[main]` behaves like the one `target_or_create_section_mut` would make.
        // (Composite `a+b` is unreachable: the char check above already rejects `+`.)
        self.append_section(n, section_kind(n));
        Ok(())
    }

    /// Append a new empty `[name]` section of `kind` at the end of the file, after a
    /// blank-line separator in the file's own line-ending style (preserving a missing
    /// final newline). Marks the config dirty and returns the new section. The caller
    /// owns name validation — this just performs the EOL-faithful append.
    fn append_section(&mut self, name: &str, kind: SectionKind) -> &mut Section {
        let style = self.file_eol();
        // Preserve a file that had no trailing newline: terminate the old last line
        // (so it doesn't glue to what we append) and let the new header be the
        // unterminated final line.
        let missing_final = matches!(self.last_eol_mut(), Some(e) if *e == Eol::None);
        if missing_final {
            if let Some(e) = self.last_eol_mut() {
                *e = style;
            }
        }
        // A blank separator before the header, attached to the preceding container
        // (blanks belong to whatever line precedes them in this model). Skipped for an
        // otherwise-empty file (nothing to separate from) or one that already ends in a
        // blank line (don't stack a second one).
        let non_empty = !self.sections.is_empty() || !self.preamble.is_empty();
        if non_empty && !self.last_is_blank() {
            let blank = Entry { raw: String::new(), eol: style, kind: EntryKind::Blank };
            match self.sections.last_mut() {
                Some(s) => s.entries.push(blank),
                None => self.preamble.push(blank),
            }
        }
        let header_eol = if missing_final { Eol::None } else { style };
        self.sections.push(Section {
            header: Entry { raw: format!("[{name}]"), eol: header_eol, kind: EntryKind::Header },
            name: name.to_string(),
            kind,
            entries: Vec::new(),
            dirty: false,
        });
        self.dirty = true;
        self.sections.last_mut().expect("just pushed")
    }

    /// The `[global]` section for editing daemon options, creating an empty one at the
    /// end of the file if the config has none yet. keyd accepts `[global]` anywhere, so
    /// appending round-trips cleanly; most real configs already carry one.
    pub fn global_section_mut(&mut self) -> &mut Section {
        // Last-wins: keyd applies `[global]` blocks in file order, so an edit must land
        // on the LAST one (mirrors [`Self::target_or_create_section_mut`]); editing an
        // earlier, shadowed block would silently do nothing.
        if let Some(i) = self.sections.iter().rposition(|s| s.name == "global") {
            return &mut self.sections[i];
        }
        self.append_section("global", SectionKind::Global)
    }

    /// Delete a layer: drop **every** layer-bearing section whose base name is `base`
    /// (`[nav]` *and* `[nav:C]` — they both define the "nav" layer). Composite layers
    /// (`[nav+sym]`) are a different layer and are left alone. Surrounding blank lines
    /// and comments are kept (we don't guess which "belonged" to the layer). Bindings
    /// elsewhere that still point at `base` become orphans — surfaced by
    /// [`Self::orphan_layer_refs`], not silently rewritten. Marks the config dirty.
    /// Returns whether anything was removed.
    pub fn remove_layer(&mut self, base: &str) -> bool {
        let before = self.sections.len();
        self.sections.retain(|s| !s.feeds_board(base));
        let removed = self.sections.len() != before;
        self.dirty |= removed;
        removed
    }

    /// Every binding that activates `layer` via a well-known layer function — the
    /// inverse of [`Self::orphan_layer_refs`]. Used to warn before deleting a layer
    /// ("N bindings still point here"). One `(section-base, key)` per offending
    /// binding, in file order. Same precision rules as [`layer_ref_spans`].
    pub fn references_to(&self, layer: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for s in self.sections.iter().filter(|s| s.kind.is_board()) {
            for e in &s.entries {
                let EntryKind::Binding { key, val: Some(val), .. } = &e.kind else { continue };
                if layer_ref_spans(val).iter().any(|sp| &val[sp.clone()] == layer) {
                    out.push((s.base_name().trim().to_string(), key.clone()));
                }
            }
        }
        out
    }

    /// Rename layer `old_base` to `new_name`, rewriting **every** reference so nothing
    /// orphans: each `[old_base]` / `[old_base:qual]` section header, each composite
    /// `[…+old_base+…]` that includes it as a constituent, and every binding that
    /// activates it via a well-known layer function (`layer`/`oneshot`/`toggle`/`swap`
    /// and the tap/hold family — exactly the set [`Self::references_to`] reports). Inside
    /// a rewritten value only the layer name is spliced; timeout args and all other text
    /// are preserved verbatim (e.g. `lettermod(nav, 150, 200)` → `lettermod(sym, 150, 200)`).
    ///
    /// `Err` names why it was rejected, mirroring [`Self::add_layer`]'s name rules, plus:
    /// the new name must differ from the old, `old_base` must exist as a *renameable*
    /// layer — not the `main` base or a composite (`a+b` is defined by its parts, not a
    /// name) — and the new base must not already exist. Marks the config dirty. Returns
    /// the number of *binding* references rewritten (renamed headers excluded), so the
    /// caller can report "updated N references".
    pub fn rename_layer(&mut self, old_base: &str, new_name: &str) -> Result<usize, String> {
        let new = new_name.trim();
        if new.is_empty() {
            return Err("layer name can't be empty".to_string());
        }
        if !new.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            return Err("layer name: use letters, digits, '_' or '-'".to_string());
        }
        if matches!(new, "ids" | "global" | "aliases") {
            return Err(format!("[{new}] is a reserved keyd section, not a layer"));
        }
        if new == old_base {
            return Err("the layer name is unchanged".to_string());
        }
        // Only a plain named layer can be renamed — not keyd's implicit `main` base, and
        // not a composite (which is defined by its `+`-joined parts, not by a name).
        let renameable =
            self.sections.iter().any(|s| s.kind == SectionKind::Layer && s.base_name().trim() == old_base);
        if !renameable {
            return Err(format!("[{old_base}] isn't a renameable layer"));
        }
        if self.sections.iter().any(|s| s.base_name().trim() == new) {
            return Err(format!("[{new}] already exists"));
        }

        // 1. The layer's own section headers (`[nav]`, `[nav:C]`) — preserve any
        //    `:qualifier` and the header line's surrounding whitespace.
        for s in self
            .sections
            .iter_mut()
            .filter(|s| s.kind == SectionKind::Layer && s.base_name().trim() == old_base)
        {
            let renamed = match s.name.split_once(':') {
                Some((_, q)) => format!("{new}:{q}"),
                None => new.to_string(),
            };
            rewrite_header_name(&mut s.header.raw, &renamed);
            s.name = renamed;
            s.dirty = true;
        }

        // 2. Composite headers that list `old_base` as a constituent (`[nav+sym]` →
        //    `[symbols+sym]`) — otherwise that part dangles and keyd rejects the file.
        for s in self.sections.iter_mut().filter(|s| s.kind == SectionKind::Composite) {
            let (base, qual) = match s.name.split_once(':') {
                Some((b, q)) => (b, Some(q)),
                None => (s.name.as_str(), None),
            };
            if !base.split('+').any(|c| c == old_base) {
                continue;
            }
            let new_base =
                base.split('+').map(|c| if c == old_base { new } else { c }).collect::<Vec<_>>().join("+");
            let renamed = match qual {
                Some(q) => format!("{new_base}:{q}"),
                None => new_base,
            };
            rewrite_header_name(&mut s.header.raw, &renamed);
            s.name = renamed;
            s.dirty = true;
        }

        // 3. Every binding that activates the layer, in any layer-bearing section —
        //    splice in the new name, keep the rest of the value byte-for-byte.
        let mut rewritten = 0usize;
        for s in self.sections.iter_mut().filter(|s| s.kind.is_board()) {
            let kind = s.kind;
            for e in &mut s.entries {
                let new_line = match &e.kind {
                    EntryKind::Binding { val: Some(v), key, .. } => {
                        // Every reference to `old_base` in this value — including ones
                        // nested in an action descriptor (`overloadi(esc, layer(nav), …)`).
                        let mut spans: Vec<_> = layer_ref_spans(v)
                            .into_iter()
                            .filter(|sp| &v[sp.clone()] == old_base)
                            .collect();
                        if spans.is_empty() {
                            None
                        } else {
                            // Splice right-to-left so earlier offsets stay valid as the
                            // string length changes.
                            spans.sort_by_key(|sp| std::cmp::Reverse(sp.start));
                            let mut nv = v.clone();
                            for sp in &spans {
                                nv.replace_range(sp.clone(), new);
                            }
                            Some((key.clone(), nv, spans.len()))
                        }
                    }
                    _ => None,
                };
                if let Some((key, nv, n)) = new_line {
                    e.raw = format!("{key} = {nv}");
                    if let EntryKind::Binding { val, typed, dirty, .. } = &mut e.kind {
                        *typed = classify(kind, Some(&nv));
                        *val = Some(nv);
                        *dirty = true;
                    }
                    rewritten += n;
                }
            }
        }

        self.dirty = true;
        Ok(rewritten)
    }

    /// The file's line-ending style (`Lf`/`CrLf`), inferred from the first terminated
    /// line; `Lf` for an empty file or one that is a single unterminated line.
    fn file_eol(&self) -> Eol {
        self.preamble
            .iter()
            .map(|e| e.eol)
            .chain(self.sections.iter().flat_map(|s| {
                std::iter::once(s.header.eol).chain(s.entries.iter().map(|e| e.eol))
            }))
            .find(|e| *e != Eol::None)
            .unwrap_or(Eol::Lf)
    }

    /// Whether the file's last line is a blank line — so [`Self::add_layer`] doesn't
    /// stack a second separator on a file that already ends in one. A section with no
    /// entries ends in its (non-blank) header, so this is `false` there.
    fn last_is_blank(&self) -> bool {
        let last = match self.sections.last() {
            Some(s) => s.entries.last(),
            None => self.preamble.last(),
        };
        matches!(last, Some(e) if matches!(e.kind, EntryKind::Blank))
    }

    /// The EOL of the file's last line (the last entry of the last section, or its
    /// header if empty, else the last preamble line), for trailing-newline fixups.
    fn last_eol_mut(&mut self) -> Option<&mut Eol> {
        if let Some(s) = self.sections.last_mut() {
            return Some(match s.entries.last_mut() {
                Some(e) => &mut e.eol,
                None => &mut s.header.eol,
            });
        }
        self.preamble.last_mut().map(|e| &mut e.eol)
    }

    /// True once any binding line was edited, added, or removed this session, or a
    /// whole section was created/deleted. Edits/adds flag the surviving entry; a
    /// binding removal sets the owning section's `dirty`; a section add/remove sets
    /// the config-level [`Self::dirty`] — all three are ORed in here.
    pub fn is_dirty(&self) -> bool {
        let dirty = |e: &Entry| matches!(e.kind, EntryKind::Binding { dirty: true, .. });
        self.dirty
            || self.preamble.iter().any(dirty)
            || self
                .sections
                .iter()
                .any(|s| s.dirty || s.entries.iter().any(dirty))
    }

    /// keyd-validation-parity diagnostics (design doc §12): conditions the model
    /// happily represents but keyd treats specially, verified against keyd 2.6.0:
    /// an entry before the first section header makes `ini_parse_string(s, NULL)`
    /// return NULL — the whole file is rejected (with a misleading "missing [ids]"
    /// message); a file with *no* `[ids]` parses fine but never matches a keyboard.
    pub fn diagnostics(&self) -> Vec<String> {
        let mut warns = Vec::new();
        if self.preamble.iter().any(|e| matches!(e.kind, EntryKind::Binding { .. })) {
            warns.push(
                "entry before the first section header — keyd rejects this file outright"
                    .to_string(),
            );
        }
        if !self.sections.iter().any(|s| s.kind == SectionKind::Ids) {
            warns.push(
                "no [ids] section — keyd will never match this file to a keyboard".to_string(),
            );
        }
        warns
    }

}

/// Generate the minimal starter config for a brand-new keyboard (design doc §5.5):
/// an `[ids]` section listing `ids_lines` — each a `vendor:product` or the bare `*`
/// wildcard — followed by an empty `[main]`. This is the smallest config keyd both
/// accepts and matches to a device; the user fills in bindings from the editor. The
/// output round-trips by construction (it parses and re-serializes identically) and
/// carries no orphan/diagnostic issues. Authored in LF (a freshly created file).
pub fn starter_config(ids_lines: &[&str]) -> String {
    let mut out = String::from("[ids]\n");
    for line in ids_lines {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("\n[main]\n");
    out
}

#[cfg(test)]
mod tests;
