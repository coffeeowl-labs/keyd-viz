//! Parse keyd config text into a [`Config`].
//!
//! A faithful port of `parse_text` from the original Python tool. keyd configs are
//! INI-like: `[section]` headers, `key = value` lines, and full-line `#` comments.
//! Recognized in `[main]`:
//!   - `lettermod`/`overload*`  → tap/hold ([`Hold`] with a tap action)
//!   - momentary `layer(x)`     → [`Hold`] with no tap action
//!   - `toggle(x)`              → chord toggle
//!   - anything else            → plain remap
//!
//! Other `[section]`s are layers; their `key = value` lines are overrides.

use std::fs;
use std::io;
use std::path::Path;

use crate::model::{Config, Hold, HoldKind, Layer};

/// keyd tap/hold macro names whose first arg is the hold target and (optional)
/// second arg is the tap key.
const TAPHOLD: [&str; 5] = ["lettermod", "overload", "overloadi", "overloadt", "overloadt2"];

/// Modifier targets — a hold onto one of these is a modifier, not a layer.
const MODS: [&str; 5] = ["control", "shift", "alt", "meta", "altgr"];

fn is_mod(target: &str) -> bool {
    MODS.contains(&target)
}

/// Read and parse a keyd config file.
pub fn parse_file(path: &Path) -> io::Result<Config> {
    Ok(parse_text(&fs::read_to_string(path)?))
}

/// Parse keyd config text. Pure (no I/O); shared by [`parse_file`] and tests.
pub fn parse_text(text: &str) -> Config {
    let mut cfg = Config::default();
    let mut section: Option<String> = None;

    for raw in text.lines() {
        // keyd has full-line comments only: strip from the first '#'.
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }

        if let Some(name) = section_header(line) {
            if name != "ids" && name != "main" {
                // setdefault: register the layer so empty sections still appear.
                ensure_layer(&mut cfg, name);
            }
            section = Some(name.to_string());
            continue;
        }

        match section.as_deref() {
            Some("ids") => {
                cfg.ids.push(line.to_string());
            }
            Some("main") => {
                if let Some((key, val)) = split_kv(line) {
                    parse_main_binding(&mut cfg, key, val);
                }
            }
            Some(layer_name) => {
                if let Some((key, val)) = split_kv(line) {
                    let layer = ensure_layer(&mut cfg, layer_name);
                    layer.keys.push((key.to_string(), val.to_string()));
                }
            }
            // Assignment before any section: the original Python would raise; we
            // skip it rather than panic.
            None => {}
        }
    }
    cfg
}

/// One `[main]` binding line, already split into `key`/`val`.
fn parse_main_binding(cfg: &mut Config, key: &str, val: &str) {
    match parse_fn_call(val) {
        Some((name, inner)) if TAPHOLD.contains(&name) => {
            let args: Vec<&str> = inner.split(',').map(str::trim).collect();
            let target = args[0];
            let tap = args.get(1).copied().unwrap_or(key);
            cfg.holds.push(Hold {
                key: key.to_string(),
                target: target.to_string(),
                kind: if is_mod(target) { HoldKind::Mod } else { HoldKind::Layer },
                tap: Some(tap.to_string()),
            });
        }
        Some(("toggle", inner)) => {
            cfg.chords.push((key.to_string(), inner.trim().to_string()));
        }
        Some(("layer", inner)) => {
            let arg = inner.trim();
            cfg.holds.push(Hold {
                key: key.to_string(),
                target: arg.to_string(),
                kind: if is_mod(arg) { HoldKind::Mod } else { HoldKind::Layer },
                tap: None,
            });
        }
        // Any other macro, or a plain value: record as a remap.
        _ => {
            cfg.remaps.push((key.to_string(), val.to_string()));
        }
    }
}

/// `[name]` → `Some("name")` (word chars only), else `None`.
fn section_header(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    if !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Some(inner)
    } else {
        None
    }
}

/// Split `key = value` on the first `=`, trimming both sides. `None` if no `=`.
fn split_kv(line: &str) -> Option<(&str, &str)> {
    let (k, v) = line.split_once('=')?;
    Some((k.trim(), v.trim()))
}

/// Match a full `name(args)` call. Equivalent to the Python regex
/// `(\w+)\((.*)\)` under `fullmatch`: `name` is word chars, then a parenthesized
/// body that runs to the final `)`.
fn parse_fn_call(val: &str) -> Option<(&str, &str)> {
    let open = val.find('(')?;
    if !val.ends_with(')') {
        return None;
    }
    let name = &val[..open];
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let inner = &val[open + 1..val.len() - 1];
    Some((name, inner))
}

/// Find-or-create a layer by name, returning a mutable reference to it.
fn ensure_layer<'a>(cfg: &'a mut Config, name: &str) -> &'a mut Layer {
    if let Some(idx) = cfg.layers.iter().position(|l| l.name == name) {
        &mut cfg.layers[idx]
    } else {
        cfg.layers.push(Layer { name: name.to_string(), keys: Vec::new() });
        cfg.layers.last_mut().unwrap()
    }
}
