//! Turn keyd key-names and binding values into human-readable glyphs.
//!
//! Faithful port of `base_legend`, `prettify`, and the legend maps from the
//! original Python tool.

/// Mod prefixes that may stack at the front of a binding value (`C-`, `S-`, ...).
const MOD_PREFIXES: [char; 5] = ['C', 'S', 'A', 'M', 'G'];

/// The shifted symbol for a key, when Shift is the only modifier (`S-9` → `(`).
fn shift_sym(key: &str) -> Option<&'static str> {
    Some(match key {
        "1" => "!", "2" => "@", "3" => "#", "4" => "$", "5" => "%",
        "6" => "^", "7" => "&", "8" => "*", "9" => "(", "0" => ")",
        "minus" => "_", "equal" => "+", "leftbrace" => "{", "rightbrace" => "}",
        "backslash" => "|", "semicolon" => ":", "apostrophe" => "\"",
        "comma" => "<", "dot" => ">", "slash" => "?", "grave" => "~",
        _ => return None,
    })
}

/// The display legend for a key-name (punctuation, named keys, arrows, ...).
fn legend(key: &str) -> Option<&'static str> {
    Some(match key {
        "minus" => "-", "equal" => "=", "backslash" => "\\", "grave" => "`",
        "leftbrace" => "[", "rightbrace" => "]", "semicolon" => ";",
        "apostrophe" => "'", "comma" => ",", "dot" => ".", "slash" => "/",
        "esc" => "Esc", "tab" => "Tab", "backspace" => "\u{232b}", "delete" => "Del",
        "enter" => "\u{23ce}", "space" => "Space", "left" => "\u{2190}",
        "right" => "\u{2192}", "up" => "\u{2191}", "down" => "\u{2193}",
        "home" => "Home", "end" => "End", "pageup" => "PgUp", "pagedown" => "PgDn",
        "capslock" => "Caps", "menu" => "Menu",
        "leftshift" => "\u{21e7}", "rightshift" => "\u{21e7}",
        "leftcontrol" => "Ctrl", "leftctrl" => "Ctrl", "rightcontrol" => "Ctrl",
        "leftalt" => "Alt", "rightalt" => "Alt",
        "leftmeta" => "\u{25c7}", "rightmeta" => "\u{25c7}", "fn" => "Fn",
        _ => return None,
    })
}

/// The glyph for a single mod prefix char (`C` → `⌃`, `G` → `AltGr`).
fn mod_glyph(c: char) -> &'static str {
    match c {
        'C' => "\u{2303}", // ⌃
        'S' => "\u{21e7}", // ⇧
        'A' => "\u{2325}", // ⌥
        'M' => "\u{25c7}", // ◇
        'G' => "AltGr",
        _ => "",
    }
}

/// Render a bare key-name as a cap legend: known legend, else uppercase single
/// letters, else the name unchanged.
pub fn base_legend(keyname: &str) -> String {
    if let Some(l) = legend(keyname) {
        return l.to_string();
    }
    let mut chars = keyname.chars();
    if let (Some(c), None) = (chars.next(), keyname.chars().nth(1)) {
        if c.is_ascii_alphabetic() {
            return c.to_ascii_uppercase().to_string();
        }
    }
    if !keyname.is_empty() && keyname.chars().all(|c| c.is_ascii_digit()) {
        return keyname.to_string();
    }
    keyname.to_string()
}

/// Compact legend for one macro step, reusing the board's own glyphs so a step
/// reads the same as that key/combo would on its own (`enter` → `⏎`, `C-t` → `⌃T`).
fn macro_step(tok: &crate::macros::MacroToken) -> String {
    use crate::macros::MacroToken;
    match tok {
        MacroToken::Key(k) => base_legend(k),
        MacroToken::Delay(n) => format!("{n}ms"),
        MacroToken::Text(t) => {
            if t.chars().count() > 12 {
                format!("{}\u{2026}", t.chars().take(11).collect::<String>())
            } else {
                t.clone()
            }
        }
        MacroToken::Chord { mods, keys } => {
            let mut s = String::new();
            for m in mods {
                s.push(*m);
                s.push('-');
            }
            s.push_str(&keys.join("+"));
            prettify(&s)
        }
    }
}

/// Render a `macro(...)`/`macro2(...)` value as a compact cap legend: a keyboard
/// glyph plus the first step, with `…` when there are more steps. Falls back to a
/// bare "⌨ macro" when the macro is one we can't decompose.
fn macro_legend(value: &str) -> String {
    match crate::macros::Macro::parse(value) {
        Some(m) => match m.tokens.first() {
            None => "\u{2328}".to_string(), // ⌨ — an empty macro
            Some(first) => {
                let more = if m.tokens.len() > 1 { "\u{2026}" } else { "" };
                format!("\u{2328} {}{}", macro_step(first), more)
            }
        },
        None => "\u{2328} macro".to_string(),
    }
}

/// Turn a keyd binding value into a human glyph, handling stacked `S-/C-/A-/M-/G-`
/// modifier prefixes (and the special shifted-symbol case for a lone `S-`).
pub fn prettify(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with("macro(") || trimmed.starts_with("macro2(") {
        return macro_legend(trimmed);
    }
    let chars: Vec<char> = value.chars().collect();
    let mut i = 0;
    let mut mods: Vec<char> = Vec::new();
    while i + 1 < chars.len() && MOD_PREFIXES.contains(&chars[i]) && chars[i + 1] == '-' {
        mods.push(chars[i]);
        i += 2;
    }
    let base: String = chars[i..].iter().collect();

    if !mods.is_empty() && !base.is_empty() {
        if mods.len() == 1 && mods[0] == 'S' {
            if let Some(sym) = shift_sym(&base) {
                return sym.to_string();
            }
        }
        let glyphs: String = mods.iter().map(|&c| mod_glyph(c)).collect();
        return format!("{}{}", glyphs, base_legend(&base));
    }
    base_legend(value)
}

#[cfg(test)]
mod tests {
    use super::prettify;

    #[test]
    fn macro_shows_glyph_and_first_step() {
        // First step is a Ctrl+t chord → board glyph form, then ellipsis for the rest.
        assert_eq!(prettify("macro(C-t 100ms google.com enter)"), "\u{2328} \u{2303}T\u{2026}");
        // A single-step macro has no ellipsis.
        assert_eq!(prettify("macro(enter)"), "\u{2328} \u{23ce}");
        // First step is typed text.
        assert_eq!(prettify("macro(Hello space World)"), "\u{2328} Hello\u{2026}");
    }

    #[test]
    fn macro2_renders_like_macro() {
        assert_eq!(prettify("macro2(400, 50, macro(Hello space World))"), "\u{2328} Hello\u{2026}");
    }

    #[test]
    fn unmodelable_macro_falls_back_to_generic_glyph() {
        assert_eq!(prettify("macro(x macro(y))"), "\u{2328} macro");
    }

    #[test]
    fn non_macro_values_are_unaffected() {
        assert_eq!(prettify("esc"), "Esc");
        assert_eq!(prettify("C-t"), "\u{2303}T");
        // A binding to the literal `macro` key (no paren) is not a macro action.
        assert_eq!(prettify("macro"), "macro");
    }
}
