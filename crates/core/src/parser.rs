//! Derive the semantic [`Config`] (what the boards render) from the line-faithful
//! [`EditConfig`] — **one keyd-faithful parser drives both the viewer and the
//! editor** (edit-mode design §5.1). The grammar layer (lines, sections, kvp
//! splitting per keyd's `ini.c`) lives in [`crate::edit`]; this module interprets
//! binding *values* the way keyd's `parse_descriptor`/`parse_fn` do (config.c):
//!
//!   - `lettermod`/`overload`/`overloadt`/`overloadt2`  → tap/hold ([`Hold`])
//!     (keyd itself rewrites `lettermod(l, t, t1, t2)` into
//!     `overloadi(t, overloadt2(l, t, t2), t1)` — same semantics, handled direct)
//!   - `overloadi(<tap>, <hold descriptor>, <timeout>)` → tap/hold; both leading
//!     args are *descriptors* (tap first!), so the hold target is extracted from a
//!     nested `layer(x)`/`overload*(x, …)` — anything else falls back to a remap
//!   - momentary `layer(x)` → [`Hold`] with no tap action
//!   - `toggle(x)` → chord toggle; chord keys (`a+b`) with any other action → combo
//!   - anything else (macros, commands, plain keys) → remap, rendered verbatim
//!
//! Argument parsing is a port of keyd's `parse_fn`: paren-depth-aware and
//! backslash-skipping, so `overload(nav, macro(a, b))` keeps its nested tap intact
//! (§12). Holds onto a modset-qualified layer (`[caps:C]`) classify as *modifier*
//! holds via a post-pass, since the section may come after `[main]`.

use std::fs;
use std::io;
use std::path::Path;

use crate::edit::{c_trim, EditConfig, EntryKind, SectionKind};
use crate::model::{Config, Hold, HoldKind, Layer};

/// Tap/hold actions whose first arg is the hold target (a layer or modifier) and
/// (optional) second arg is the tap key. `overloadi` is *not* in this family — its
/// leading args are descriptors, handled separately.
pub(crate) const TAPHOLD: [&str; 4] = ["lettermod", "overload", "overloadt", "overloadt2"];

/// Modifier targets — a hold onto one of these is a modifier, not a layer.
pub(crate) const MODS: [&str; 5] = ["control", "shift", "alt", "meta", "altgr"];

pub(crate) fn is_mod(target: &str) -> bool {
    MODS.contains(&target)
}

/// Read and parse a keyd config file.
pub fn parse_file(path: &Path) -> io::Result<Config> {
    Ok(parse_text(&fs::read_to_string(path)?))
}

/// Parse keyd config text. Pure (no I/O); shared by [`parse_file`] and tests.
pub fn parse_text(text: &str) -> Config {
    derive(&EditConfig::parse(text))
}

/// Build the semantic model from an already-parsed [`EditConfig`]. This is what the
/// editor calls to re-render a board after a visual edit — no re-read, no second
/// parser to drift.
pub fn derive(edit: &EditConfig) -> Config {
    let mut cfg = Config::default();

    for section in &edit.sections {
        match section.kind {
            SectionKind::Ids => {
                for e in &section.entries {
                    if matches!(e.kind, EntryKind::Binding { .. }) {
                        cfg.ids.push(c_trim(&e.raw).to_string());
                    }
                }
            }
            // keyd special-cases [global] (daemon options) and [aliases] (key
            // aliases) — not layers, don't render their bodies as boards.
            // (Resolving aliases onto physical keys is still a future enhancement.)
            SectionKind::Global | SectionKind::Aliases => {}
            SectionKind::Main => {
                for (key, val) in bindings(section) {
                    parse_main_binding(&mut cfg, key, val);
                }
                for (key, label) in section_labels(section) {
                    push_label(&mut cfg.labels, key, label);
                }
            }
            SectionKind::Layer | SectionKind::Composite => {
                // A qualifier selects the base layer ([nav:C] extends nav); capture
                // a modset qualifier on the layer, since it changes hold rendering.
                let name = section.base_name().trim().to_string();
                if name.is_empty() {
                    continue;
                }
                let layer = ensure_layer(&mut cfg, &name);
                match section.qualifier() {
                    Some(q) if !q.is_empty() && q != "layout" => {
                        layer.mods = Some(q.to_string());
                    }
                    _ => {}
                }
                for (key, val) in bindings(section) {
                    let layer = ensure_layer(&mut cfg, &name);
                    // A chord line (`j+k = …`) is layer-scoped in keyd, but isn't a
                    // single-key slot binding — keep it off `keys` (which build_layer
                    // looks up per slot) and render it as a member badge instead.
                    if is_chord_key(key) {
                        layer.combos.push((key.to_string(), val.to_string()));
                    } else {
                        layer.keys.push((key.to_string(), val.to_string()));
                    }
                }
                // Custom labels for this layer (merged across same-base sections, e.g.
                // [nav] + [nav:C], since both resolve to the one `name` layer above).
                for (key, label) in section_labels(section) {
                    let layer = ensure_layer(&mut cfg, &name);
                    push_label(&mut layer.labels, key, label);
                }
            }
        }
    }

    // §12: a hold onto a custom modifier layer ([caps:C]) is a modifier hold. The
    // layer section may appear after [main], so classify in a post-pass.
    let mod_layers: Vec<String> = cfg
        .layers
        .iter()
        .filter(|l| l.mods.is_some())
        .map(|l| l.name.clone())
        .collect();
    for h in &mut cfg.holds {
        if h.kind == HoldKind::Layer && mod_layers.contains(&h.target) {
            h.kind = HoldKind::Mod;
        }
    }

    cfg
}

/// The `key = value` entries of a section (valueless lines carry no binding).
fn bindings(section: &crate::edit::Section) -> impl Iterator<Item = (&str, &str)> {
    section.entries.iter().filter_map(|e| match &e.kind {
        EntryKind::Binding { key, val: Some(val), .. } => Some((key.as_str(), val.as_str())),
        _ => None,
    })
}

/// The `(key, label)` pairs from this section's `# keyd-viz: …` comment lines.
fn section_labels(section: &crate::edit::Section) -> impl Iterator<Item = (&str, &str)> {
    section.entries.iter().filter_map(|e| match e.kind {
        EntryKind::Comment => crate::edit::parse_label_comment(&e.raw),
        _ => None,
    })
}

/// Record a label for `key`, last-wins (mirroring keyd's last-wins binding rule so a
/// later `# keyd-viz:` line overrides an earlier one for the same key).
fn push_label(labels: &mut Vec<(String, String)>, key: &str, label: &str) {
    if let Some(slot) = labels.iter_mut().find(|(k, _)| k == key) {
        slot.1 = label.to_string();
    } else {
        labels.push((key.to_string(), label.to_string()));
    }
}

/// One `[main]` binding line, already split into `key`/`val`.
fn parse_main_binding(cfg: &mut Config, key: &str, val: &str) {
    // Chord keys (`a+b = …`): toggle keeps its dedicated slot (rendered as a chord
    // badge); any other action is a general combo (§12 — previously a bogus remap).
    if is_chord_key(key) {
        match parse_fn(val) {
            Some(("toggle", args)) if args.len() == 1 => {
                cfg.chords.push((key.to_string(), args[0].to_string()));
            }
            _ => cfg.combos.push((key.to_string(), val.to_string())),
        }
        return;
    }

    match parse_fn(val) {
        Some((name, args)) if TAPHOLD.contains(&name) && !args.is_empty() => {
            let target = args[0];
            let tap = args.get(1).copied().unwrap_or(key);
            cfg.holds.push(Hold {
                key: key.to_string(),
                target: target.to_string(),
                kind: if is_mod(target) { HoldKind::Mod } else { HoldKind::Layer },
                tap: Some(tap.to_string()),
            });
        }
        // overloadi(<tap>, <hold descriptor>, <timeout>): tap comes FIRST and the
        // hold is a descriptor (keyd's own lettermod rewrite emits exactly this
        // shape). Extract the hold target when the descriptor is layer-like.
        Some(("overloadi", args)) if args.len() >= 2 => {
            let hold_target = match parse_fn(args[1]) {
                Some(("layer", a)) if a.len() == 1 => Some(a[0]),
                Some(("overload" | "overloadt" | "overloadt2", a)) if !a.is_empty() => {
                    Some(a[0])
                }
                _ if is_mod(args[1]) => Some(args[1]),
                _ => None,
            };
            match hold_target {
                Some(target) => cfg.holds.push(Hold {
                    key: key.to_string(),
                    target: target.to_string(),
                    kind: if is_mod(target) { HoldKind::Mod } else { HoldKind::Layer },
                    tap: Some(args[0].to_string()),
                }),
                // A hold descriptor we can't reduce (macro, command, plain key):
                // show the binding verbatim rather than inventing a layer.
                None => cfg.remaps.push((key.to_string(), val.to_string())),
            }
        }
        Some(("toggle", args)) if args.len() == 1 => {
            cfg.chords.push((key.to_string(), args[0].to_string()));
        }
        Some(("layer", args)) if args.len() == 1 => {
            let arg = args[0];
            cfg.holds.push(Hold {
                key: key.to_string(),
                target: arg.to_string(),
                kind: if is_mod(arg) { HoldKind::Mod } else { HoldKind::Layer },
                tap: None,
            });
        }
        // Any other action, or a plain value: record as a remap.
        _ => {
            cfg.remaps.push((key.to_string(), val.to_string()));
        }
    }
}

/// A chord key binds two or more `+`-joined keys (`j+k`). A lone `+` (the shifted
/// `=`) or a leading/trailing `+` is not a chord.
pub fn is_chord_key(key: &str) -> bool {
    let parts: Vec<&str> = key.split('+').collect();
    parts.len() >= 2 && parts.iter().all(|p| !p.trim().is_empty())
}

/// Canonical form of a chord key for order-independent `a+b == b+a` matching: split
/// on `+`, trim each part, sort, rejoin with `+`. Used to find the existing line for
/// a chord regardless of the order it was spelled (the editor rewrites that line in
/// place, keeping the user's original spelling). Purely lexical on the raw key tokens
/// — it does *not* canonicalise keysyms, so genuinely different LHS spellings like
/// `equal+a` and `=+a` stay distinct (they are distinct lines to keyd). Callers should
/// guarantee [`is_chord_key`] first.
pub fn canonical_chord(key: &str) -> String {
    let mut parts: Vec<&str> = key.split('+').map(str::trim).collect();
    parts.sort_unstable();
    parts.join("+")
}

/// Port of keyd's `parse_fn` (config.c): match `name(arg, …)`. The name is
/// everything before the first `(`, **verbatim** (keyd does not trim it — a space
/// before the paren makes it a non-action). Args split on depth-0 commas, with
/// `\`-escaped characters skipped and nested parens tracked, so
/// `overload(nav, macro(a, b))` yields `["nav", "macro(a, b)"]`. Leading spaces of
/// each arg are skipped (spaces only — keyd doesn't skip tabs here); empty args are
/// dropped; anything after the balancing `)` is discarded — all exactly as keyd
/// does. `None` when there is no `(` or the parens never balance.
pub(crate) fn parse_fn(s: &str) -> Option<(&str, Vec<&str>)> {
    let b = s.as_bytes();
    let open = s.find('(')?;
    let name = &s[..open];

    let mut args = Vec::new();
    let mut c = open + 1;
    loop {
        while c < b.len() && b[c] == b' ' {
            c += 1;
        }
        let start = c;
        let mut depth = 0i32;
        loop {
            if c >= b.len() {
                return None; // unterminated call — not a function (keyd: -1)
            }
            match b[c] {
                b'\\' if c + 1 < b.len() => {
                    c += 2;
                    continue;
                }
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == -1 {
                        break;
                    }
                }
                b',' if depth == 0 => break,
                _ => {}
            }
            c += 1;
        }
        if start != c {
            args.push(&s[start..c]);
        }
        if b[c] == b')' {
            return Some((name, args));
        }
        c += 1; // step past the comma
    }
}

/// The leading keyd function name of a binding RHS (`overload`, `overloadi`, `macro`,
/// …), or `None` when the value isn't a `name(...)` call. Lets the editor tell apart
/// the tap/hold forms it can decompose from exotic ones (`overloadi`) it must leave raw.
pub fn leading_fn(rhs: &str) -> Option<&str> {
    parse_fn(rhs.trim()).map(|(name, _)| name)
}

/// Find-or-create a layer by name, returning a mutable reference to it.
fn ensure_layer<'a>(cfg: &'a mut Config, name: &str) -> &'a mut Layer {
    if let Some(idx) = cfg.layers.iter().position(|l| l.name == name) {
        &mut cfg.layers[idx]
    } else {
        cfg.layers.push(Layer { name: name.to_string(), ..Layer::default() });
        cfg.layers.last_mut().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------ custom labels
    #[test]
    fn derive_extracts_labels_for_main_and_layers() {
        let cfg = parse_text(
            "[ids]\n*\n\n[main]\n# keyd-viz: tab = Tab L\ntab = layer(nav)\n\
             # not a label, prose comment\ncapslock = esc\n\n\
             [nav]\n# keyd-viz: h = Left\nh = left\n",
        );
        assert_eq!(cfg.label("tab"), Some("Tab L"));
        assert_eq!(cfg.label("capslock"), None); // prose comment isn't a label
        assert_eq!(cfg.layer("nav").unwrap().labels, vec![("h".into(), "Left".into())]);
    }

    #[test]
    fn derive_merges_labels_across_same_base_sections() {
        // [nav] and [nav:C] resolve to one layer; labels from both land on it.
        let cfg = parse_text(
            "[ids]\n*\n[nav]\n# keyd-viz: h = Left\nh = left\n\
             [nav:C]\n# keyd-viz: j = Down\nj = down\n",
        );
        let labels = &cfg.layer("nav").unwrap().labels;
        assert!(labels.contains(&("h".into(), "Left".into())));
        assert!(labels.contains(&("j".into(), "Down".into())));
    }

    #[test]
    fn derive_label_lastwins_and_equal_key() {
        let cfg = parse_text(
            "[ids]\n*\n[main]\n# keyd-viz: a = First\n# keyd-viz: a = Second\na = b\n\
             # keyd-viz: = = Equals\n= = backspace\n",
        );
        assert_eq!(cfg.label("a"), Some("Second")); // last-wins
        assert_eq!(cfg.label("="), Some("Equals")); // the `=` key parses correctly
    }

    // ------------------------------------------------------------ parse_fn parity
    #[test]
    fn parse_fn_nested_args_survive() {
        let (name, args) = parse_fn("overload(nav, macro(a, b))").unwrap();
        assert_eq!(name, "overload");
        assert_eq!(args, ["nav", "macro(a, b)"]);
    }

    #[test]
    fn parse_fn_escaped_chars_skipped() {
        // An escaped comma/paren must not split or close: macro(\,) is one arg.
        let (_, args) = parse_fn(r"macro(a\,b, c)").unwrap();
        assert_eq!(args, [r"a\,b", "c"]);
        let (_, args) = parse_fn(r"macro(\))").unwrap();
        assert_eq!(args, [r"\)"]);
    }

    #[test]
    fn parse_fn_name_is_verbatim() {
        // keyd does not trim the name: a space before '(' makes it a non-action.
        let (name, _) = parse_fn("toggle (x)").unwrap();
        assert_eq!(name, "toggle ");
    }

    #[test]
    fn parse_fn_trailing_garbage_discarded() {
        // keyd stops at the balancing ')' and ignores the rest.
        let (name, args) = parse_fn("layer(nav) trailing").unwrap();
        assert_eq!((name, args), ("layer", vec!["nav"]));
    }

    #[test]
    fn parse_fn_unterminated_is_none() {
        assert_eq!(parse_fn("overload(nav"), None);
        assert_eq!(parse_fn("plainkey"), None);
        assert_eq!(parse_fn(r"macro(a\"), None); // trailing escape never closes
    }

    #[test]
    fn parse_fn_empty_args_dropped() {
        let (_, args) = parse_fn("overload(nav,)").unwrap();
        assert_eq!(args, ["nav"]);
    }

    // -------------------------------------------------------------- §12 semantics
    #[test]
    fn overload_with_nested_macro_tap() {
        // The naive comma split corrupted this tap to "macro(a" (§12).
        let cfg = parse_text("[ids]\n*\n[main]\nspace = overload(nav, macro(a, b))\n");
        assert_eq!(cfg.holds[0].tap.as_deref(), Some("macro(a, b)"));
        assert_eq!(cfg.holds[0].target, "nav");
    }

    #[test]
    fn overloadi_tap_first_hold_descriptor() {
        // overloadi(<tap>, <hold>, <timeout>) — tap is the FIRST arg; the hold is
        // a descriptor. This is keyd's own lettermod rewrite shape.
        let cfg = parse_text("[main]\na = overloadi(a, overloadt2(nav, a, 500), 200)\n");
        assert_eq!(cfg.holds[0].tap.as_deref(), Some("a"));
        assert_eq!(cfg.holds[0].target, "nav");
        assert_eq!(cfg.holds[0].kind, HoldKind::Layer);

        let cfg = parse_text("[main]\nb = overloadi(b, layer(control), 200)\n");
        assert_eq!(cfg.holds[0].target, "control");
        assert_eq!(cfg.holds[0].kind, HoldKind::Mod);
    }

    #[test]
    fn overloadi_opaque_hold_falls_back_to_remap() {
        let cfg = parse_text("[main]\na = overloadi(a, macro(hi), 200)\n");
        assert!(cfg.holds.is_empty());
        assert_eq!(cfg.remaps[0].1, "overloadi(a, macro(hi), 200)");
    }

    #[test]
    fn modset_qualified_layer_holds_classify_as_mod() {
        // [caps:C] is a custom modifier layer; holding it is a modifier hold —
        // even though the section appears after [main] (§12).
        let cfg = parse_text("[main]\ncapslock = layer(caps)\n[caps:C]\nj = down\n");
        assert_eq!(cfg.holds[0].kind, HoldKind::Mod);
        assert_eq!(cfg.layer("caps").unwrap().mods.as_deref(), Some("C"));
        // A plain layer stays a layer hold.
        let cfg = parse_text("[main]\ncapslock = layer(nav)\n[nav]\nj = down\n");
        assert_eq!(cfg.holds[0].kind, HoldKind::Layer);
    }

    #[test]
    fn general_chords_are_combos_not_remaps() {
        let cfg = parse_text("[main]\nj+k = esc\na+s+d = layer(nav)\n");
        assert_eq!(
            cfg.combos,
            vec![
                ("j+k".to_string(), "esc".to_string()),
                ("a+s+d".to_string(), "layer(nav)".to_string())
            ]
        );
        assert!(cfg.remaps.is_empty());
        assert!(cfg.holds.is_empty());
        // Toggle chords keep their dedicated slot (rendered as chord badges).
        let cfg = parse_text("[main]\nx+c = toggle(game)\n");
        assert_eq!(cfg.chords, vec![("x+c".to_string(), "game".to_string())]);
    }

    #[test]
    fn layer_chords_go_to_layer_combos_not_keys() {
        // keyd scopes a chord to its layer: a `j+k` line under [nav] belongs to that
        // layer's combos (rendered as a member badge), never `keys` (which is
        // single-key, slot-addressable) — else it'd be an invisible phantom binding.
        let cfg = parse_text("[main]\ncapslock = layer(nav)\n\n[nav]\nh = left\nj+k = esc\n");
        let nav = cfg.layers.iter().find(|l| l.name == "nav").unwrap();
        assert_eq!(nav.combos, vec![("j+k".to_string(), "esc".to_string())]);
        assert_eq!(nav.keys, vec![("h".to_string(), "left".to_string())]);
        // Base combos are unaffected by a layer chord.
        assert!(cfg.combos.is_empty());
    }

    #[test]
    fn plus_key_itself_is_not_a_chord() {
        // `+` is a real key name (shifted `=`); `a+` / `+a` aren't chords either.
        assert!(!is_chord_key("+"));
        assert!(!is_chord_key("a+"));
        assert!(!is_chord_key("+a"));
        assert!(is_chord_key("j+k"));
    }

    #[test]
    fn canonical_chord_collapses_order() {
        // a+b == b+a regardless of spelling order; 3-key too.
        assert_eq!(canonical_chord("k+j"), canonical_chord("j+k"));
        assert_eq!(canonical_chord("c+a+b"), "a+b+c");
        // Whitespace around parts is trimmed before comparison.
        assert_eq!(canonical_chord("k + j"), canonical_chord("j+k"));
    }

    #[test]
    fn canonical_chord_does_not_over_merge_alt_spellings() {
        // `=` and `equal` are different LHS tokens to keyd — keep them distinct.
        assert_ne!(canonical_chord("equal+a"), canonical_chord("=+a"));
    }
}
