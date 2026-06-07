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
}

/// A whole config file as an ordered list of verbatim lines: anything before the
/// first section header (`preamble` — only blanks/comments in a file keyd accepts),
/// then the sections in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditConfig {
    pub preamble: Vec<Entry>,
    pub sections: Vec<Section>,
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
/// slicing stays on char boundaries).
fn c_trim(s: &str) -> &str {
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
}
