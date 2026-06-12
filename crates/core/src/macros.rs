//! The editor's model of a keyd **macro** binding — `macro(...)` / `macro2(...)` —
//! the compose/decompose half that lets the GUI present a macro as an ordered list
//! of typed tokens instead of raw keyd syntax. Mirrors [`crate::taphold`].
//!
//! keyd's macro grammar (`man keyd`, MACROS): `macro(<exp>)` where `<exp>` is a
//! **space**-separated sequence of tokens, each one of — a key code (`enter`),
//! a literal unicode group typed verbatim (`Hello`), a `+`-joined key unit
//! (`leftcontrol+leftmeta`, `3+5`) or modifier shorthand (`C-a`, `A-M-x`), or a
//! delay `<n>ms` (n < 1024). **Tokenization *is* the escaping**: `macro(space)`
//! presses the space key while `macro(s pace)` types "space"; `macro(3+5)` is a
//! chord while `macro(3 + 5)` types "3+5". keyd has **no** backslash escaping
//! inside a macro (verified: A0 spike) — token boundaries are the only mechanism.
//! `macro2(<timeout>, <repeat>, <macro>)` wraps a macro with repeat timing
//! (`repeat = 0` disables); its three args are **comma**-separated even though the
//! inner macro's tokens are space-separated.
//!
//! ## Faithfulness
//! [`Macro::parse`] returns `None` for any shape we can't model losslessly (nested
//! macros, literal `(`/`)`, non-integer `macro2` args, trailing junk) so those stay
//! raw and the editor refuses to clobber them — the `overloadi` philosophy. Because
//! a literal text run and an equivalent key sequence are intentionally collapsible
//! (typing "enter" emits `e n t e r`, five key taps), the contract is **serialize
//! idempotence**: `serialize(parse(serialize(m))) == serialize(m)`. For any `Macro`
//! produced by `parse`, the stronger `parse(serialize(m)) == m` also holds.

use crate::keycodes::is_keycode;
use crate::parser::{is_chord_key, parse_fn};

/// The C/M/A/S/G modifier letters of keyd's shorthand (`C-a` = Control+a). Order:
/// **C**ontrol **M**eta **A**lt **S**hift altg**R** (written `G`).
const MOD_PREFIXES: [char; 5] = ['C', 'M', 'A', 'S', 'G'];

/// One token of a macro, in the order it's typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroToken {
    /// A single key tap by keyd key name: `enter`, `space`, `a`, `+`.
    Key(String),
    /// A simultaneous key unit — modifier shorthand (`mods` = the `C`/`M`/`A`/`S`/`G`
    /// letters) and/or a `+`-joined group (`keys`). `C-a` ⇒ `{mods:[C], keys:[a]}`;
    /// `leftcontrol+leftmeta` ⇒ `{mods:[], keys:[leftcontrol, leftmeta]}`.
    Chord { mods: Vec<char>, keys: Vec<String> },
    /// Literal text typed verbatim.
    Text(String),
    /// A `<n>ms` pause, n < 1024.
    Delay(u16),
}

impl MacroToken {
    /// This token as macro source — may expand to **several** space-separated
    /// tokens (a `Text` whose content needs splitting to type literally).
    fn serialize(&self) -> String {
        match self {
            MacroToken::Key(k) => k.clone(),
            MacroToken::Delay(n) => format!("{n}ms"),
            MacroToken::Chord { mods, keys } => {
                let mut s = String::new();
                for m in mods {
                    s.push(*m);
                    s.push('-');
                }
                s.push_str(&keys.join("+"));
                s
            }
            MacroToken::Text(t) => serialize_text(t),
        }
    }
}

/// A decomposed macro the editor can read, edit, and re-serialize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Macro {
    pub tokens: Vec<MacroToken>,
    /// `Some((timeout, repeat))` ⇔ a `macro2(...)` (repeat `0` disables repeating);
    /// `None` ⇔ a plain `macro(...)`.
    pub repeat: Option<(u32, u32)>,
}

impl Macro {
    /// Decompose a binding RHS into a macro the editor can present, or `None` when
    /// the value isn't a macro we model losslessly (the caller then treats it as
    /// raw text and refuses to rewrite it).
    pub fn parse(rhs: &str) -> Option<Macro> {
        let s = rhs.trim();
        let (name, inner) = outer_call(s)?;
        match name {
            "macro" => Some(Macro {
                tokens: parse_tokens(inner)?,
                repeat: None,
            }),
            "macro2" => {
                // The three args are comma-separated; parse_fn splits on depth-0
                // commas, so the inner macro's own commas (raised paren depth) are
                // safe. (`outer_call` above already rejected trailing junk.)
                let (_, args) = parse_fn(s)?;
                if args.len() != 3 {
                    return None;
                }
                let timeout = args[0].trim().parse::<u32>().ok()?;
                let repeat = args[1].trim().parse::<u32>().ok()?;
                let inner_expr = args[2].trim();
                // The inner macro expression is either `macro(...)` or a bare token
                // run (`macro2(120, 80, left)`). Anything else with parens is exotic.
                let src = if let Some((iname, iinner)) = outer_call(inner_expr) {
                    if iname != "macro" {
                        return None; // nested macro2 / other call → stays raw
                    }
                    iinner
                } else if inner_expr.contains('(') {
                    return None;
                } else {
                    inner_expr
                };
                Some(Macro {
                    tokens: parse_tokens(src)?,
                    repeat: Some((timeout, repeat)),
                })
            }
            _ => None,
        }
    }

    /// The keyd binding text for this macro, ready to write as the RHS. A `macro2`
    /// always emits its inner as an explicit `macro(...)` wrapper (normalizing a
    /// bare-inner form) — harmless because the line-faithful edit model keeps an
    /// *unedited* macro byte-verbatim; serialize only runs on a deliberate edit.
    pub fn serialize(&self) -> String {
        let inner = self
            .tokens
            .iter()
            .map(MacroToken::serialize)
            .collect::<Vec<_>>()
            .join(" ");
        match self.repeat {
            None => format!("macro({inner})"),
            Some((t, r)) => format!("macro2({t}, {r}, macro({inner}))"),
        }
    }
}

/// Extract `(name, inner)` from a `name(...)` call: `inner` is the raw content
/// between the first `(` and its matching `)`, requiring nothing but whitespace
/// after the close (no trailing junk) and balanced parens. Returns `None` for an
/// unbalanced/unterminated call (e.g. a literal `(` in the content) — those stay
/// raw. Unlike [`parse_fn`] this does **not** comma-split, so a macro's literal
/// commas survive.
fn outer_call(s: &str) -> Option<(&str, &str)> {
    let open = s.find('(')?;
    let name = &s[..open];
    let b = s.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    let mut close = None;
    while i < b.len() {
        match b[i] {
            b'\\' if i + 1 < b.len() => {
                i += 2;
                continue;
            }
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let close = close?;
    if s[close + 1..].trim().is_empty() {
        Some((name, &s[open + 1..close]))
    } else {
        None
    }
}

/// Space-tokenize a macro expression and classify each token. `None` if any token
/// contains `(` or `)` — a nested call or a literal paren we can't model (stays raw).
fn parse_tokens(inner: &str) -> Option<Vec<MacroToken>> {
    let mut out = Vec::new();
    for tok in inner.split(' ') {
        if tok.is_empty() {
            continue; // collapse runs of spaces
        }
        if tok.contains('(') || tok.contains(')') {
            return None;
        }
        out.push(classify(tok));
    }
    Some(out)
}

/// Classify a single space-delimited token. Order matters: a whole keycode is
/// tested **before** the chord rules so `iso-level3-shift`, `+`, `-` (real keys
/// containing `-`/`+`) aren't mis-split. keyd's rule is "a valid key code presses
/// that key, otherwise the token is typed literally."
fn classify(tok: &str) -> MacroToken {
    // 1. Delay: <n>ms, n < 1024.
    if let Some(num) = tok.strip_suffix("ms") {
        if !num.is_empty() {
            if let Ok(v) = num.parse::<u16>() {
                if (v as u32) < 1024 {
                    return MacroToken::Delay(v);
                }
            }
        }
    }
    // 2. The whole token is a key name.
    if is_keycode(tok) {
        return MacroToken::Key(tok.to_string());
    }
    // 3. Modifier shorthand: a leading run of `C-`/`M-`/`A-`/`S-`/`G-` + a key body.
    if let Some((mods, rest)) = strip_mod_prefixes(tok) {
        let keys = split_plus(rest);
        if chord_keys_valid(&keys) {
            return MacroToken::Chord { mods, keys };
        }
    }
    // 4. A `+`-joined key unit.
    if is_chord_key(tok) {
        let keys = split_plus(tok);
        if chord_keys_valid(&keys) {
            return MacroToken::Chord {
                mods: Vec::new(),
                keys,
            };
        }
    }
    // 5. Literal text.
    MacroToken::Text(tok.to_string())
}

/// Peel a leading run of `C-`/`M-`/`A-`/`S-`/`G-` modifier prefixes off a token,
/// returning the modifier letters and the remaining key body. `None` when there is
/// no leading modifier prefix.
fn strip_mod_prefixes(tok: &str) -> Option<(Vec<char>, &str)> {
    let mut mods = Vec::new();
    let mut rest = tok;
    while let Some(c0) = rest.chars().next() {
        if MOD_PREFIXES.contains(&c0) && rest[c0.len_utf8()..].starts_with('-') {
            mods.push(c0);
            rest = &rest[c0.len_utf8() + 1..];
        } else {
            break;
        }
    }
    if mods.is_empty() {
        None
    } else {
        Some((mods, rest))
    }
}

fn split_plus(s: &str) -> Vec<String> {
    s.split('+').map(str::to_string).collect()
}

/// Every part of a `+`-unit is a real key (so the token is a genuine chord and not
/// literal text that merely contains `+`).
fn chord_keys_valid(keys: &[String]) -> bool {
    !keys.is_empty() && keys.iter().all(|k| is_keycode(k))
}

/// Render literal text as space-separated macro tokens that type it verbatim. A
/// space becomes the `space` key token; a word safe to emit as one literal token
/// (i.e. `classify` reads it straight back as the same `Text`) is kept whole for
/// readability, otherwise it's split to single characters — each printable char's
/// own key types that character, so the split is faithful and idempotent.
fn serialize_text(t: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut first = true;
    for word in t.split(' ') {
        if !first {
            out.push("space".to_string());
        }
        first = false;
        if word.is_empty() {
            continue;
        }
        if classify(word) == MacroToken::Text(word.to_string()) {
            out.push(word.to_string());
        } else {
            for ch in word.chars() {
                out.push(ch.to_string());
            }
        }
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> MacroToken {
        MacroToken::Key(s.to_string())
    }
    fn t(s: &str) -> MacroToken {
        MacroToken::Text(s.to_string())
    }
    fn chord(mods: &str, keys: &[&str]) -> MacroToken {
        MacroToken::Chord {
            mods: mods.chars().collect(),
            keys: keys.iter().map(|s| s.to_string()).collect(),
        }
    }

    // ---- man-page oracle examples ----

    #[test]
    fn parses_the_man_compound_example() {
        let m = Macro::parse("macro(C-t 100ms google.com enter)").unwrap();
        assert_eq!(
            m.tokens,
            vec![chord("C", &["t"]), MacroToken::Delay(100), t("google.com"), k("enter")]
        );
        assert_eq!(m.repeat, None);
        assert_eq!(m.serialize(), "macro(C-t 100ms google.com enter)");
    }

    #[test]
    fn space_key_vs_typed_space() {
        // `space` is the space KEY; `s pace` types the word.
        let key = Macro::parse("macro(space)").unwrap();
        assert_eq!(key.tokens, vec![k("space")]);
        let typed = Macro::parse("macro(s pace)").unwrap();
        assert_eq!(typed.tokens, vec![k("s"), t("pace")]);
        assert_eq!(typed.serialize(), "macro(s pace)");
    }

    #[test]
    fn chord_unit_vs_typed_plus() {
        let unit = Macro::parse("macro(3+5)").unwrap();
        assert_eq!(unit.tokens, vec![chord("", &["3", "5"])]);
        assert_eq!(unit.serialize(), "macro(3+5)");
        let typed = Macro::parse("macro(3 + 5)").unwrap();
        assert_eq!(typed.tokens, vec![k("3"), k("+"), k("5")]);
        assert_eq!(typed.serialize(), "macro(3 + 5)");
    }

    #[test]
    fn multi_modifier_shorthand() {
        let m = Macro::parse("macro(A-M-x)").unwrap();
        assert_eq!(m.tokens, vec![chord("AM", &["x"])]);
        assert_eq!(m.serialize(), "macro(A-M-x)");
    }

    #[test]
    fn plus_joined_named_keys() {
        let m = Macro::parse("macro(leftcontrol+leftmeta)").unwrap();
        assert_eq!(m.tokens, vec![chord("", &["leftcontrol", "leftmeta"])]);
        assert_eq!(m.serialize(), "macro(leftcontrol+leftmeta)");
    }

    #[test]
    fn keys_that_contain_dash_or_plus_stay_keys() {
        // Whole-keycode wins before the chord rules.
        assert_eq!(Macro::parse("macro(iso-level3-shift)").unwrap().tokens, vec![k("iso-level3-shift")]);
        assert_eq!(Macro::parse("macro(+ - space)").unwrap().tokens, vec![k("+"), k("-"), k("space")]);
    }

    // ---- macro2 ----

    #[test]
    fn macro2_wrapped_inner() {
        let m = Macro::parse("macro2(400, 50, macro(Hello space World))").unwrap();
        assert_eq!(m.repeat, Some((400, 50)));
        assert_eq!(m.tokens, vec![t("Hello"), k("space"), t("World")]);
        assert_eq!(m.serialize(), "macro2(400, 50, macro(Hello space World))");
    }

    #[test]
    fn macro2_bare_inner_normalizes_to_wrapped() {
        let m = Macro::parse("macro2(120, 80, left)").unwrap();
        assert_eq!(m.repeat, Some((120, 80)));
        assert_eq!(m.tokens, vec![k("left")]);
        // Normalized form re-parses identically (idempotent).
        assert_eq!(m.serialize(), "macro2(120, 80, macro(left))");
        assert_eq!(Macro::parse(&m.serialize()).unwrap(), m);
    }

    #[test]
    fn macro2_comma_inside_inner_is_safe() {
        // The `,` is a real key, protected by the inner macro's parens.
        let m = Macro::parse("macro2(120, 80, macro(a , b))").unwrap();
        assert_eq!(m.tokens, vec![k("a"), k(","), k("b")]);
        assert_eq!(m.serialize(), "macro2(120, 80, macro(a , b))");
    }

    // ---- stays-raw cases (None) ----

    #[test]
    fn unmodelable_forms_stay_raw() {
        assert!(Macro::parse("overload(nav, a)").is_none()); // not a macro
        assert!(Macro::parse("b").is_none()); // plain remap
        assert!(Macro::parse("macro(a ( b)").is_none()); // literal paren → unbalanced
        assert!(Macro::parse("macro(a macro(b))").is_none()); // nested macro
        assert!(Macro::parse("macro2(notanint, 5, left)").is_none()); // bad timeout
        assert!(Macro::parse("macro2(1, 2, macro2(3, 4, a))").is_none()); // nested macro2
        assert!(Macro::parse("macro(x) trailing").is_none()); // trailing junk
        assert!(Macro::parse("macro2(1, 2)").is_none()); // wrong arg count
    }

    // ---- text serialization (the careful, idempotent part) ----

    #[test]
    fn text_that_is_a_keyname_is_char_split() {
        // User wants to type the literal word "enter", not press Enter.
        let m = Macro {
            tokens: vec![t("enter")],
            repeat: None,
        };
        assert_eq!(m.serialize(), "macro(e n t e r)");
        // Re-parse gives five key taps, but re-serializing is stable (idempotent).
        let reparsed = Macro::parse(&m.serialize()).unwrap();
        assert_eq!(reparsed.tokens, vec![k("e"), k("n"), k("t"), k("e"), k("r")]);
        assert_eq!(reparsed.serialize(), m.serialize());
    }

    #[test]
    fn text_with_literal_plus_is_char_split() {
        // Literal "a+b" must type, not chord — emit `a + b`.
        let m = Macro {
            tokens: vec![t("a+b")],
            repeat: None,
        };
        assert_eq!(m.serialize(), "macro(a + b)");
        assert_eq!(Macro::parse(&m.serialize()).unwrap().serialize(), m.serialize());
    }

    #[test]
    fn ordinary_words_stay_whole() {
        let m = Macro {
            tokens: vec![t("Hello"), t("google.com")],
            repeat: None,
        };
        assert_eq!(m.serialize(), "macro(Hello google.com)");
    }

    #[test]
    fn multi_word_text_uses_space_tokens() {
        let m = Macro {
            tokens: vec![t("Hello World")],
            repeat: None,
        };
        assert_eq!(m.serialize(), "macro(Hello space World)");
    }

    // ---- round-trip property: any parsed macro re-serializes stably ----

    #[test]
    fn parse_serialize_is_idempotent() {
        let cases = [
            "macro(a)",
            "macro(C-t 100ms google.com enter)",
            "macro(Hello space World)",
            "macro(s pace)",
            "macro(3+5)",
            "macro(3 + 5)",
            "macro(A-M-x)",
            "macro(leftcontrol+leftmeta)",
            "macro(iso-level3-shift)",
            "macro(+ - space)",
            "macro2(400, 50, macro(Hello space World))",
            "macro2(120, 80, macro(left))",
            "macro(a , b)",
        ];
        for c in cases {
            let m = Macro::parse(c).unwrap_or_else(|| panic!("should parse: {c}"));
            // For any parse() output, the structural round-trip holds.
            assert_eq!(Macro::parse(&m.serialize()), Some(m.clone()), "structural: {c}");
            // And serialization is a fixed point.
            assert_eq!(m.serialize(), Macro::parse(&m.serialize()).unwrap().serialize(), "idempotent: {c}");
        }
    }
}
