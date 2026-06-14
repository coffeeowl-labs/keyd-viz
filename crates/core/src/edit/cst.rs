//! Grammar leaves: keyd line classification, key-value + label parsing, header
//! rewriting, and the round-trip self-check. Shared by the model in `super`.

use super::*;

/// Replace the bracketed name in a section header line, preserving leading/trailing
/// whitespace (`  [nav]  ` → `  [symbols]  `) and any inner `]` keyd keeps in a name
/// (which spans the first `[` to the last `]`). Callers only pass header lines, so both
/// brackets are present; a malformed line without them is left untouched.
pub(crate) fn rewrite_header_name(raw: &mut String, new_name: &str) {
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
/// Crate-visible: the label grammar reuses it so a `keyd-viz:` comment's `key = text`
/// splits identically to a binding line (the `=` key, punctuation keys, labels with `=`).
pub(crate) fn parse_kvp(s: &str) -> (&str, Option<&str>) {
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

/// The marker that distinguishes a keyd-viz custom-label comment from a prose
/// comment: `# keyd-viz: <key> = <label>`. keyd ignores the whole line (it's a
/// `#`-comment); keyd-viz parses it for the cap label. See `docs/labels-design.md`.
pub(crate) const LABEL_MARKER: &str = "keyd-viz:";

/// Parse a `# keyd-viz: <key> = <label>` comment into `(key, label)`, or `None` if
/// `raw` isn't a well-formed label comment. The `<key> = <label>` split reuses
/// [`parse_kvp`], so the key parses exactly like a binding key (incl. the `=` key);
/// the label is the trimmed remainder (free-form, may contain `#`/`=`). An empty key
/// or empty label yields `None` (nothing to show).
pub(crate) fn parse_label_comment(raw: &str) -> Option<(&str, &str)> {
    let t = c_trim(raw);
    let rest = c_trim(t.strip_prefix('#')?); // drop the leading '#', then whitespace
    let body = c_trim(rest.strip_prefix(LABEL_MARKER)?); // after "keyd-viz:"
    let (key, val) = parse_kvp(body);
    let key = c_trim(key);
    let label = c_trim(val?);
    (!key.is_empty() && !label.is_empty()).then_some((key, label))
}

/// The canonical on-disk form of a label comment (single spaces). Re-parses to the
/// same `Comment` entry, so keyd-viz's own output round-trips.
pub(crate) fn label_comment_line(key: &str, label: &str) -> String {
    format!("# {LABEL_MARKER} {key} = {label}")
}

pub(crate) fn section_kind(name: &str) -> SectionKind {
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
pub(crate) fn classify_in(section: Option<SectionKind>, val: Option<&str>) -> Typed {
    match section {
        Some(k) => classify(k, val),
        None => Typed::Raw,
    }
}

/// Conservative typed-overlay classification. Only binding lines inside layer-bearing
/// sections (`main`, named layers, composites) carry semantics the editor models;
/// `[ids]`/`[global]`/`[aliases]` entries are always `Raw` (a `[global]`
/// `macro_timeout = 600` is not a remap). When unsure, `Raw`.
pub(crate) fn classify(section: SectionKind, val: Option<&str>) -> Typed {
    if !section.is_board() {
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
