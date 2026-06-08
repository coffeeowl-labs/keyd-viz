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
    /// section's end) in the file's own line-ending style.
    pub fn set_or_add_binding(&mut self, key: &str, new_val: &str) {
        if !self.set_binding(key, new_val) {
            self.push_binding(key, new_val);
        }
    }

    /// Append a fresh `key = value` binding line to this section.
    fn push_binding(&mut self, key: &str, new_val: &str) {
        // The file's line style, inferred from the section header.
        let style = match self.header.eol {
            Eol::None => Eol::Lf,
            e => e,
        };
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
            matches!(s.kind, SectionKind::Main | SectionKind::Layer | SectionKind::Composite)
                && s.base_name().trim() == layer
        })
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
            matches!(s.kind, SectionKind::Main | SectionKind::Layer | SectionKind::Composite)
                && s.base_name().trim() == layer
        }) {
            removed |= s.remove_binding(key);
        }
        removed
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
            header: Entry { raw: format!("[{n}]"), eol: header_eol, kind: EntryKind::Header },
            name: n.to_string(),
            kind: SectionKind::Layer,
            entries: Vec::new(),
            dirty: false,
        });
        self.dirty = true;
        Ok(())
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
        self.sections.retain(|s| {
            !(matches!(s.kind, SectionKind::Main | SectionKind::Layer | SectionKind::Composite)
                && s.base_name().trim() == base)
        });
        let removed = self.sections.len() != before;
        self.dirty |= removed;
        removed
    }

    /// Every binding that activates `layer` via a well-known layer function — the
    /// inverse of [`Self::orphan_layer_refs`]. Used to warn before deleting a layer
    /// ("N bindings still point here"). One `(section-base, key)` per offending
    /// binding, in file order. Same precision rules as [`layer_refs`].
    pub fn references_to(&self, layer: &str) -> Vec<(String, String)> {
        let is_layer = |s: &&Section| {
            matches!(s.kind, SectionKind::Main | SectionKind::Layer | SectionKind::Composite)
        };
        let mut out = Vec::new();
        for s in self.sections.iter().filter(is_layer) {
            for e in &s.entries {
                let EntryKind::Binding { key, val: Some(val), .. } = &e.kind else { continue };
                if layer_refs(val) == Some(layer) {
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
        for s in self.sections.iter_mut().filter(|s| {
            matches!(s.kind, SectionKind::Main | SectionKind::Layer | SectionKind::Composite)
        }) {
            let kind = s.kind;
            for e in &mut s.entries {
                let new_line = match &e.kind {
                    EntryKind::Binding { val: Some(v), key, .. } => layer_ref_span(v)
                        .filter(|span| &v[span.clone()] == old_base)
                        .map(|span| (key.clone(), format!("{}{new}{}", &v[..span.start], &v[span.end..]))),
                    _ => None,
                };
                if let Some((key, nv)) = new_line {
                    e.raw = format!("{key} = {nv}");
                    if let EntryKind::Binding { val, typed, dirty, .. } = &mut e.kind {
                        *typed = classify(kind, Some(&nv));
                        *val = Some(nv);
                        *dirty = true;
                    }
                    rewritten += 1;
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

    /// Bindings that activate a layer this config never defines — keyd rejects such a
    /// file, so the editor can flag it *before* apply (e.g. you bound `layer(symbols)`
    /// but haven't created `[symbols]` yet, or deleted a layer something still points
    /// at). One entry per offending binding, in file order.
    ///
    /// Deliberately **high-precision over high-recall**: `keyd check` is the real
    /// gate at apply time, so a missed orphan is far cheaper than a false alarm on a
    /// valid config. Only well-known layer activators are scanned, modifier targets
    /// (keyd's built-in modifier layers) are never flagged, and composite `a+b`
    /// targets are skipped (their definition rules are subtle) — see [`layer_refs`].
    pub fn orphan_layer_refs(&self) -> Vec<OrphanRef> {
        let is_layer = |s: &&Section| {
            matches!(s.kind, SectionKind::Main | SectionKind::Layer | SectionKind::Composite)
        };
        let defined: std::collections::HashSet<&str> =
            self.sections.iter().filter(is_layer).map(|s| s.base_name().trim()).collect();

        let mut out = Vec::new();
        for s in self.sections.iter().filter(is_layer) {
            for e in &s.entries {
                let EntryKind::Binding { key, val: Some(val), .. } = &e.kind else { continue };
                if let Some(layer) = layer_refs(val) {
                    if !defined.contains(layer) {
                        out.push(OrphanRef {
                            section: s.base_name().trim().to_string(),
                            key: key.clone(),
                            layer: layer.to_string(),
                        });
                    }
                }
            }
        }
        out
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

/// A binding that points at an undefined layer — `key = …layer(`layer`)…` living in
/// section `[`section`]` (base name). See [`EditConfig::orphan_layer_refs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanRef {
    pub section: String,
    pub key: String,
    pub layer: String,
}

/// Layer-activating functions whose sole argument is a layer name. The tap/hold family
/// ([`crate::parser::TAPHOLD`]) also takes a layer as its *first* arg — but only when
/// that arg isn't a modifier, which [`layer_refs`] guards.
const LAYER_FNS: [&str; 4] = ["layer", "oneshot", "toggle", "swap"];

/// The layer name a binding value activates via a well-known layer function, or `None`.
/// Modifier targets (keyd's built-in modifier layers) and composite `a+b` targets are
/// excluded — both are valid without a matching `[…]` section, so flagging them would
/// be a false alarm.
fn layer_refs(val: &str) -> Option<&str> {
    layer_ref_span(val).map(|r| &val[r])
}

/// The byte range within `val` of the layer name it activates — the splice point
/// [`EditConfig::rename_layer`] rewrites, leaving everything else in the value verbatim.
/// `None` under the same precision rules as [`layer_refs`] (which is `&val[span]`).
fn layer_ref_span(val: &str) -> Option<std::ops::Range<usize>> {
    let (name, args) = crate::parser::parse_fn(val)?;
    let arg0 = *args.first()?;
    let trimmed = arg0.trim();
    let referenced = LAYER_FNS.contains(&name) || crate::parser::TAPHOLD.contains(&name);
    if !(referenced && !crate::parser::is_mod(trimmed) && !trimmed.contains('+')) {
        return None;
    }
    // `parse_fn` slices `arg0` out of `val`, so its byte offset is the pointer delta;
    // add the leading whitespace `trim` drops to land on the name itself.
    let off = arg0.as_ptr() as usize - val.as_ptr() as usize;
    let lead = arg0.len() - arg0.trim_start().len();
    Some(off + lead..off + lead + trimmed.len())
}

/// Replace the bracketed name in a section header line, preserving leading/trailing
/// whitespace (`  [nav]  ` → `  [symbols]  `) and any inner `]` keyd keeps in a name
/// (which spans the first `[` to the last `]`). Callers only pass header lines, so both
/// brackets are present; a malformed line without them is left untouched.
fn rewrite_header_name(raw: &mut String, new_name: &str) {
    if let (Some(open), Some(close)) = (raw.find('['), raw.rfind(']')) {
        *raw = format!("{}[{new_name}]{}", &raw[..open], &raw[close + 1..]);
    }
}

/// The §5.1 round-trip gate, run before a file is opened for editing (a `false` sends
/// it to view-only). With the per-line model this is identity-by-construction, so the
/// gate is a self-check against parse/serialize asymmetry we may introduce later —
/// most importantly line-ending fidelity. (Non-UTF-8 files never reach here: the
/// `&str` boundary upstream already fails them to view-only.)
pub fn round_trips(text: &str) -> bool {
    EditConfig::parse(text).serialize() == text
}

/// C `isspace` (default locale) — what keyd's line trim uses.
fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

/// Trim leading + trailing C-`isspace`, byte-wise (all space bytes are ASCII, so
/// slicing stays on char boundaries). Crate-visible: the semantic deriver needs
/// keyd's exact trim, not `str::trim`.
pub(crate) fn c_trim(s: &str) -> &str {
    let b = s.as_bytes();
    let start = b.iter().position(|&c| !is_c_space(c)).unwrap_or(b.len());
    let end = b.iter().rposition(|&c| !is_c_space(c)).map_or(start, |i| i + 1);
    &s[start..end]
}

/// keyd's `parse_kvp` (`ini.c`), exactly: operates on the already-trimmed line; the
/// first char may be `=` (so the `=` key is bindable); the key is everything before
/// the first `=` thereafter with its trailing space/tab run trimmed; the value is
/// everything after, with leading spaces/tabs skipped. No `=` → valueless (`None`).
fn parse_kvp(s: &str) -> (&str, Option<&str>) {
    let b = s.as_bytes();
    let mut last_space: Option<usize> = None;
    let mut i = usize::from(b.first() == Some(&b'='));

    while i < b.len() {
        match b[i] {
            b'=' => {
                let key_end = last_space.unwrap_or(i);
                let mut v = i + 1;
                while v < b.len() && (b[v] == b' ' || b[v] == b'\t') {
                    v += 1;
                }
                return (&s[..key_end], Some(&s[v..]));
            }
            b' ' | b'\t' => {
                if last_space.is_none() {
                    last_space = Some(i);
                }
            }
            _ => last_space = None,
        }
        i += 1;
    }
    (s, None)
}

fn section_kind(name: &str) -> SectionKind {
    // Exact verbatim match for the specials, like keyd's strcmp (`[ids ]` ≠ `[ids]`).
    match name {
        "ids" => SectionKind::Ids,
        "global" => SectionKind::Global,
        "aliases" => SectionKind::Aliases,
        _ => {
            let base = name.split(':').next().unwrap_or(name);
            if base == "main" {
                SectionKind::Main
            } else if base.contains('+') {
                SectionKind::Composite
            } else {
                SectionKind::Layer
            }
        }
    }
}

/// Classify a binding value in a section we may not have (preamble → `None`).
fn classify_in(section: Option<SectionKind>, val: Option<&str>) -> Typed {
    match section {
        Some(k) => classify(k, val),
        None => Typed::Raw,
    }
}

/// Conservative typed-overlay classification. Only binding lines inside layer-bearing
/// sections (`main`, named layers, composites) carry semantics the editor models;
/// `[ids]`/`[global]`/`[aliases]` entries are always `Raw` (a `[global]`
/// `macro_timeout = 600` is not a remap). When unsure, `Raw`.
fn classify(section: SectionKind, val: Option<&str>) -> Typed {
    if !matches!(section, SectionKind::Main | SectionKind::Layer | SectionKind::Composite) {
        return Typed::Raw;
    }
    match val {
        Some("noop") => Typed::Noop,
        // A bare alphanumeric token is a plain key name (`b`, `f1`, `leftcontrol`,
        // `pageup`). Punctuation keys (`-`, `=`, …) and macro shorthand (`C-a`) stay
        // Raw until E1 widens the editable set against keyd's key list.
        Some(v) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') => {
            Typed::Remap(v.to_string())
        }
        _ => Typed::Raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kvp(s: &str) -> (&str, Option<&str>) {
        parse_kvp(s)
    }

    // ------------------------------------------------------- parse_kvp parity (ini.c)
    #[test]
    fn kvp_plain() {
        assert_eq!(kvp("a = b"), ("a", Some("b")));
        assert_eq!(kvp("a=b"), ("a", Some("b")));
        assert_eq!(kvp("a\t =\t b"), ("a", Some("b")));
    }

    #[test]
    fn kvp_value_may_contain_equals() {
        assert_eq!(kvp("a = b=c"), ("a", Some("b=c")));
    }

    #[test]
    fn kvp_equals_key_special_case() {
        // ini.c: "Allow the first character to be = as a special case."
        assert_eq!(kvp("= = a"), ("=", Some("a")));
        assert_eq!(kvp("==x"), ("=", Some("x")));
        assert_eq!(kvp("="), ("=", None));
        assert_eq!(kvp("=foo"), ("=foo", None));
        assert_eq!(kvp("=foo=bar"), ("=foo", Some("bar")));
    }

    #[test]
    fn kvp_valueless() {
        assert_eq!(kvp("0123:4567"), ("0123:4567", None));
        assert_eq!(kvp("*"), ("*", None));
    }

    #[test]
    fn kvp_empty_value() {
        assert_eq!(kvp("key ="), ("key", Some("")));
    }

    #[test]
    fn kvp_key_with_internal_space() {
        // Only the *trailing* space/tab run before '=' is trimmed (last_space logic).
        assert_eq!(kvp("a b = c"), ("a b", Some("c")));
    }

    // ------------------------------------------------------------ line classification
    #[test]
    fn header_grammar_matches_ini_c() {
        let cfg = EditConfig::parse("[a]b]\n[#x]\n#[y]\n[main ]\n");
        let names: Vec<&str> = cfg.sections.iter().map(|s| s.name.as_str()).collect();
        // `[a]b]` names `a]b`; `[#x]` is a header (the '[' case wins); `#[y]` is a
        // comment; `[main ]` keeps its inner space and is NOT [main].
        assert_eq!(names, ["a]b", "#x", "main "]);
        assert_eq!(cfg.sections[2].kind, SectionKind::Layer);
        assert!(matches!(cfg.sections[1].entries[0].kind, EntryKind::Comment));
    }

    #[test]
    fn unterminated_bracket_is_a_binding() {
        // ini.c: '[' without a closing ']' falls through to parse_kvp.
        let cfg = EditConfig::parse("[main]\n[foo = bar\n");
        match &cfg.sections[0].entries[0].kind {
            EntryKind::Binding { key, val, .. } => {
                assert_eq!(key, "[foo");
                assert_eq!(val.as_deref(), Some("bar"));
            }
            other => panic!("expected Binding, got {other:?}"),
        }
    }

    #[test]
    fn section_kinds_and_qualifiers() {
        let cfg = EditConfig::parse("[ids]\n[global]\n[aliases]\n[main]\n[nav:C]\n[a+b]\n");
        let kinds: Vec<SectionKind> = cfg.sections.iter().map(|s| s.kind).collect();
        use SectionKind::*;
        assert_eq!(kinds, [Ids, Global, Aliases, Main, Layer, Composite]);
        assert_eq!(cfg.sections[4].base_name(), "nav");
        assert_eq!(cfg.sections[4].qualifier(), Some("C"));
        assert_eq!(cfg.sections[3].qualifier(), None);
    }

    #[test]
    fn classification_is_section_aware() {
        let cfg = EditConfig::parse(
            "[global]\nmacro_timeout = 600\n[main]\na = b\nb = noop\nc = layer(nav)\n",
        );
        let typed = |s: usize, e: usize| match &cfg.sections[s].entries[e].kind {
            EntryKind::Binding { typed, .. } => typed.clone(),
            other => panic!("expected Binding, got {other:?}"),
        };
        assert_eq!(typed(0, 0), Typed::Raw); // [global] value is not a remap
        assert_eq!(typed(1, 0), Typed::Remap("b".into()));
        assert_eq!(typed(1, 1), Typed::Noop);
        assert_eq!(typed(1, 2), Typed::Raw); // actions not yet modeled
    }

    // ----------------------------------------------------------------------- mutation
    #[test]
    fn set_binding_regenerates_one_line_only() {
        let src = "# my config\n[ids]\n0123:4567\n\n[main]\n# capslock\ncapslock = esc\na = b\n";
        let mut cfg = EditConfig::parse(src);
        assert!(cfg.section_mut("main").unwrap().set_binding("a", "c"));
        let out = cfg.serialize();
        assert_eq!(out, src.replace("a = b", "a = c"));
    }

    #[test]
    fn set_binding_edits_the_last_duplicate() {
        // keyd applies entries in order; the last assignment wins, so that's the one
        // the editor must touch.
        let mut cfg = EditConfig::parse("[main]\na = x\na = y\n");
        assert!(cfg.section_mut("main").unwrap().set_binding("a", "z"));
        assert_eq!(cfg.serialize(), "[main]\na = x\na = z\n");
    }

    #[test]
    fn set_binding_missing_key_is_a_noop() {
        let src = "[main]\na = b\n";
        let mut cfg = EditConfig::parse(src);
        assert!(!cfg.section_mut("main").unwrap().set_binding("q", "x"));
        assert_eq!(cfg.serialize(), src);
    }

    #[test]
    fn add_binding_lands_before_trailing_blank_separator() {
        // The blank line separating sections must stay at the section's end.
        let src = "[main]\na = b\n\n[nav]\nh = left\n";
        let mut cfg = EditConfig::parse(src);
        cfg.target_section_mut("main").unwrap().set_or_add_binding("q", "esc");
        assert_eq!(cfg.serialize(), "[main]\na = b\nq = esc\n\n[nav]\nh = left\n");
    }

    #[test]
    fn add_binding_preserves_missing_final_newline() {
        let mut cfg = EditConfig::parse("[main]\na = b");
        cfg.target_section_mut("main").unwrap().set_or_add_binding("q", "esc");
        // The old last line gains its newline; the new last line takes the None.
        assert_eq!(cfg.serialize(), "[main]\na = b\nq = esc");
    }

    #[test]
    fn add_binding_to_empty_section_and_crlf_style() {
        let mut cfg = EditConfig::parse("[main]\r\n");
        cfg.target_section_mut("main").unwrap().set_or_add_binding("a", "b");
        assert_eq!(cfg.serialize(), "[main]\r\na = b\r\n");
    }

    #[test]
    fn target_section_picks_the_last_duplicate() {
        // keyd merges duplicate sections in order; an appended line must land in
        // the LAST one so it out-ranks every earlier assignment.
        let src = "[nav]\nh = left\n[nav:C]\nj = down\n";
        let mut cfg = EditConfig::parse(src);
        cfg.target_section_mut("nav").unwrap().set_or_add_binding("k", "up");
        assert_eq!(cfg.serialize(), "[nav]\nh = left\n[nav:C]\nj = down\nk = up\n");
        // [ids]/[global] are never edit targets.
        assert!(cfg.target_section_mut("ids").is_none());
        assert!(cfg.target_section_mut("missing").is_none());
    }

    #[test]
    fn get_binding_last_duplicate_wins() {
        let cfg = EditConfig::parse("[main]\na = x\na = y\n");
        assert_eq!(cfg.sections[0].get_binding("a"), Some("y"));
        assert_eq!(cfg.sections[0].get_binding("q"), None);
    }

    #[test]
    fn dirty_tracks_edits_and_adds() {
        let mut cfg = EditConfig::parse("[main]\na = b\n");
        assert!(!cfg.is_dirty());
        cfg.target_section_mut("main").unwrap().set_or_add_binding("a", "c");
        assert!(cfg.is_dirty());
        let mut cfg = EditConfig::parse("[main]\n");
        cfg.target_section_mut("main").unwrap().set_or_add_binding("q", "esc");
        assert!(cfg.is_dirty());
    }

    // ----------------------------------------------------------------- remove/clear
    #[test]
    fn remove_binding_drops_just_that_line() {
        let src = "[main]\n# capslock\ncapslock = esc\na = b\n";
        let mut cfg = EditConfig::parse(src);
        assert!(cfg.section_mut("main").unwrap().remove_binding("capslock"));
        // The line goes; its comment is left as-is (we don't guess intent).
        assert_eq!(cfg.serialize(), "[main]\n# capslock\na = b\n");
    }

    #[test]
    fn remove_binding_drops_every_duplicate() {
        // keyd is last-wins, so transparency needs ALL assignments gone.
        let mut cfg = EditConfig::parse("[main]\na = x\nb = y\na = z\n");
        assert!(cfg.section_mut("main").unwrap().remove_binding("a"));
        assert_eq!(cfg.serialize(), "[main]\nb = y\n");
    }

    #[test]
    fn remove_binding_missing_key_is_a_noop() {
        let src = "[main]\na = b\n";
        let mut cfg = EditConfig::parse(src);
        assert!(!cfg.section_mut("main").unwrap().remove_binding("q"));
        assert_eq!(cfg.serialize(), src);
        assert!(!cfg.is_dirty());
    }

    #[test]
    fn remove_only_change_still_marks_dirty() {
        // A pure removal leaves no entry to carry a dirty flag — the section-level
        // flag is what keeps is_dirty() honest (else save/apply would stay off).
        let mut cfg = EditConfig::parse("[main]\na = b\n");
        assert!(!cfg.is_dirty());
        assert!(cfg.section_mut("main").unwrap().remove_binding("a"));
        assert!(cfg.is_dirty());
    }

    #[test]
    fn clear_binding_spans_every_merged_section() {
        // [nav] and [nav:C] both feed the "nav" board; clearing only one would
        // leave the key bound, so clear_binding must hit both.
        let src = "[nav]\nh = left\n[nav:C]\nh = right\nj = down\n";
        let mut cfg = EditConfig::parse(src);
        assert!(cfg.clear_binding("nav", "h"));
        assert_eq!(cfg.serialize(), "[nav]\n[nav:C]\nj = down\n");
        assert!(cfg.is_dirty());
    }

    #[test]
    fn clear_binding_missing_is_a_noop() {
        let src = "[nav]\nh = left\n";
        let mut cfg = EditConfig::parse(src);
        assert!(!cfg.clear_binding("nav", "q"));
        assert!(!cfg.clear_binding("missing", "h"));
        assert_eq!(cfg.serialize(), src);
        assert!(!cfg.is_dirty());
    }

    fn orphans(src: &str) -> Vec<(String, String, String)> {
        EditConfig::parse(src)
            .orphan_layer_refs()
            .into_iter()
            .map(|o| (o.section, o.key, o.layer))
            .collect()
    }

    #[test]
    fn orphan_layer_reference_is_flagged() {
        let got = orphans("[main]\ncapslock = layer(symbols)\n");
        assert_eq!(got, vec![("main".into(), "capslock".into(), "symbols".into())]);
    }

    #[test]
    fn defined_layer_is_not_an_orphan() {
        // The reference resolves once the section exists — even modifier-qualified
        // (`[nav:C]` defines base `nav`).
        assert!(orphans("[main]\na = layer(nav)\n[nav:C]\nh = left\n").is_empty());
        assert!(orphans("[main]\na = toggle(nav)\n[nav]\n").is_empty());
    }

    #[test]
    fn modifier_target_is_never_an_orphan() {
        // overload(mod, tap) / layer(mod) target keyd's built-in modifier layers —
        // valid with no matching section.
        assert!(orphans("[main]\na = overload(shift, esc)\n").is_empty());
        assert!(orphans("[main]\na = oneshot(control)\n").is_empty());
    }

    #[test]
    fn composite_and_non_layer_values_are_skipped() {
        // a+b composite targets (subtle definition rules) and plain/non-fn values
        // never raise a false alarm.
        assert!(orphans("[main]\na = layer(nav+sym)\n").is_empty());
        assert!(orphans("[main]\na = esc\nb = C-c\nc = macro(h i)\n").is_empty());
    }

    #[test]
    fn taphold_layer_target_is_flagged_but_its_tap_is_not() {
        // overload(LAYER, tap): only arg0 is a layer; the tap key must not be scanned.
        let got = orphans("[main]\ncapslock = overload(nav, esc)\n");
        assert_eq!(got, vec![("main".into(), "capslock".into(), "nav".into())]);
    }

    // --------------------------------------------------------------- add/remove layer
    #[test]
    fn add_layer_appends_a_blank_then_header() {
        let mut cfg = EditConfig::parse("[ids]\n*\n\n[main]\na = b\n");
        cfg.add_layer("nav").unwrap();
        // A blank separator, then the new empty section, in the file's LF style.
        assert_eq!(cfg.serialize(), "[ids]\n*\n\n[main]\na = b\n\n[nav]\n");
        assert!(cfg.is_dirty());
        // It's a real editable target now (set_or_add_binding lands in it).
        cfg.target_section_mut("nav").unwrap().set_or_add_binding("h", "left");
        assert_eq!(cfg.serialize(), "[ids]\n*\n\n[main]\na = b\n\n[nav]\nh = left\n");
    }

    #[test]
    fn add_layer_into_empty_file_has_no_separator() {
        let mut cfg = EditConfig::parse("");
        cfg.add_layer("nav").unwrap();
        assert_eq!(cfg.serialize(), "[nav]\n");
    }

    #[test]
    fn add_layer_does_not_stack_a_second_blank() {
        // File already ends in a blank line: the new header reuses it, no double gap.
        let mut cfg = EditConfig::parse("[main]\na = b\n\n");
        cfg.add_layer("nav").unwrap();
        assert_eq!(cfg.serialize(), "[main]\na = b\n\n[nav]\n");
    }

    #[test]
    fn add_layer_preserves_crlf_and_missing_final_newline() {
        // CRLF style is inferred and the missing final newline is preserved: the old
        // last line gains its terminator, the new header becomes the unterminated tail.
        let mut cfg = EditConfig::parse("[main]\r\na = b");
        cfg.add_layer("nav").unwrap();
        assert_eq!(cfg.serialize(), "[main]\r\na = b\r\n\r\n[nav]");
    }

    #[test]
    fn add_layer_rejects_bad_names_and_duplicates() {
        let mut cfg = EditConfig::parse("[ids]\n*\n[main]\na = b\n[nav]\n");
        assert!(cfg.add_layer("").unwrap_err().contains("empty"));
        assert!(cfg.add_layer("  ").unwrap_err().contains("empty"));
        assert!(cfg.add_layer("a b").unwrap_err().contains("letters"));
        assert!(cfg.add_layer("a:b").unwrap_err().contains("letters"));
        assert!(cfg.add_layer("a+b").unwrap_err().contains("letters"));
        assert!(cfg.add_layer("ids").unwrap_err().contains("reserved"));
        assert!(cfg.add_layer("nav").unwrap_err().contains("exists"));
        // A modifier-qualified section already defines the base, so it's a duplicate.
        cfg.add_layer("sym").unwrap();
        // Whitespace is trimmed before all checks.
        assert!(cfg.add_layer("  sym  ").unwrap_err().contains("exists"));
        // Nothing above mutated the file except the one successful `sym`.
        assert!(cfg.section("sym").is_some());
    }

    #[test]
    fn add_layer_base_dup_check_spans_qualified_sections() {
        let mut cfg = EditConfig::parse("[main]\na = b\n[nav:C]\nh = left\n");
        // [nav:C] defines base `nav`; creating `[nav]` would silently merge.
        assert!(cfg.add_layer("nav").unwrap_err().contains("exists"));
    }

    #[test]
    fn remove_layer_drops_all_sections_for_the_base() {
        let src = "[ids]\n*\n\n[main]\na = layer(nav)\n\n[nav]\nh = left\n[nav:C]\nj = down\n";
        let mut cfg = EditConfig::parse(src);
        assert!(cfg.remove_layer("nav"));
        // Both [nav] and [nav:C] go; the dangling layer(nav) ref is left for the
        // orphan check to surface, not silently rewritten.
        assert_eq!(cfg.serialize(), "[ids]\n*\n\n[main]\na = layer(nav)\n\n");
        assert!(cfg.is_dirty());
        assert_eq!(cfg.orphan_layer_refs().len(), 1);
    }

    #[test]
    fn remove_layer_leaves_composites_and_missing_is_noop() {
        let mut cfg = EditConfig::parse("[main]\na = b\n[nav+sym]\nx = y\n");
        // base "nav" must not take the composite [nav+sym] with it.
        assert!(!cfg.remove_layer("nav"));
        assert!(!cfg.is_dirty());
        assert_eq!(cfg.serialize(), "[main]\na = b\n[nav+sym]\nx = y\n");
    }

    #[test]
    fn references_to_finds_every_activator() {
        let src = "[main]\na = layer(nav)\nb = oneshot(nav)\nc = overload(nav, esc)\n\
                   d = esc\n[fn]\ng = toggle(nav)\n[nav]\nh = left\n";
        let cfg = EditConfig::parse(src);
        let refs = cfg.references_to("nav");
        assert_eq!(
            refs,
            vec![
                ("main".into(), "a".into()),
                ("main".into(), "b".into()),
                ("main".into(), "c".into()),
                ("fn".into(), "g".into()),
            ]
        );
        assert!(cfg.references_to("sym").is_empty());
    }

    #[test]
    fn rename_layer_rewrites_headers_and_every_reference() {
        let src = "[ids]\n*\n\n[main]\na = layer(nav)\nb = oneshot(nav)\n\
                   c = lettermod(nav, 150, 200)\nd = overload(shift, esc)\n\
                   [nav]\nh = left\n[nav:C]\nj = down\n";
        let mut cfg = EditConfig::parse(src);
        assert_eq!(cfg.rename_layer("nav", "symbols").unwrap(), 3);
        assert_eq!(
            cfg.serialize(),
            "[ids]\n*\n\n[main]\na = layer(symbols)\nb = oneshot(symbols)\n\
             c = lettermod(symbols, 150, 200)\nd = overload(shift, esc)\n\
             [symbols]\nh = left\n[symbols:C]\nj = down\n"
        );
        assert!(cfg.is_dirty());
        // No orphans: every reference followed the rename, and `shift` was never a ref.
        assert!(cfg.orphan_layer_refs().is_empty());
    }

    #[test]
    fn rename_layer_rewrites_composite_constituents() {
        let src = "[main]\nx = y\n[nav]\nh = left\n[sym]\nk = up\n[nav+sym]\nq = w\n";
        let mut cfg = EditConfig::parse(src);
        cfg.rename_layer("nav", "navi").unwrap();
        // The composite's `nav` part is rewritten so it doesn't dangle; `sym` is left.
        assert_eq!(
            cfg.serialize(),
            "[main]\nx = y\n[navi]\nh = left\n[sym]\nk = up\n[navi+sym]\nq = w\n"
        );
    }

    #[test]
    fn rename_layer_rejects_bad_names_and_collisions() {
        let mut cfg = EditConfig::parse("[main]\na = b\n[nav]\nh = left\n[sym]\nk = up\n");
        assert!(cfg.rename_layer("nav", "").unwrap_err().contains("empty"));
        assert!(cfg.rename_layer("nav", "a b").unwrap_err().contains("letters"));
        assert!(cfg.rename_layer("nav", "ids").unwrap_err().contains("reserved"));
        assert!(cfg.rename_layer("nav", "nav").unwrap_err().contains("unchanged"));
        assert!(cfg.rename_layer("nav", "sym").unwrap_err().contains("exists"));
        // Nothing changed on any rejection.
        assert!(!cfg.is_dirty());
        assert_eq!(cfg.serialize(), "[main]\na = b\n[nav]\nh = left\n[sym]\nk = up\n");
    }

    #[test]
    fn rename_layer_refuses_main_composite_and_missing() {
        let mut cfg = EditConfig::parse("[main]\na = b\n[nav+sym]\nx = y\n");
        // The base layer and composites aren't simple renames; a missing layer can't be.
        assert!(cfg.rename_layer("main", "base").unwrap_err().contains("renameable"));
        assert!(cfg.rename_layer("nav+sym", "combo").unwrap_err().contains("renameable"));
        assert!(cfg.rename_layer("ghost", "x").unwrap_err().contains("renameable"));
        assert!(!cfg.is_dirty());
    }

    #[test]
    fn rename_layer_qualified_only_and_blocks_existing_base() {
        // A layer that exists only as a qualified section is still renameable.
        let mut cfg = EditConfig::parse("[main]\na = layer(nav)\n[nav:C]\nj = down\n");
        assert_eq!(cfg.rename_layer("nav", "fn").unwrap(), 1);
        assert_eq!(cfg.serialize(), "[main]\na = layer(fn)\n[fn:C]\nj = down\n");
    }

    // -------------------------------------------------------------- starter config (§5.5)
    #[test]
    fn starter_config_is_minimal_valid_and_round_trips() {
        // A specific device id.
        let s = starter_config(&["04fe:0021"]);
        assert_eq!(s, "[ids]\n04fe:0021\n\n[main]\n");
        // Round-trips by construction (the §5.1 gate the create path runs).
        assert!(round_trips(&s));
        let cfg = EditConfig::parse(&s);
        // Has [ids] + [main], an empty main, no diagnostics, no orphans.
        assert!(cfg.diagnostics().is_empty());
        assert!(cfg.orphan_layer_refs().is_empty());
        assert_eq!(cfg.section("ids").unwrap().kind, SectionKind::Ids);
        let main = cfg.section("main").unwrap();
        assert_eq!(main.kind, SectionKind::Main);
        assert!(!main.entries.iter().any(|e| matches!(e.kind, EntryKind::Binding { .. })));
    }

    #[test]
    fn starter_config_wildcard_and_multi_id() {
        assert_eq!(starter_config(&["*"]), "[ids]\n*\n\n[main]\n");
        assert_eq!(
            starter_config(&["04fe:0021", "k:1234:5678"]),
            "[ids]\n04fe:0021\nk:1234:5678\n\n[main]\n"
        );
    }
}
