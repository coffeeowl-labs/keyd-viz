//! The semantic render model: turn a [`Config`] + physical [`Geometry`] into
//! presentation-agnostic boards of key caps. This is the bridge between keyd
//! logic and any renderer (the Slint UI, or the legacy HTML).
//!
//! Ports `render_base` / `render_layer` / `render_config` from the original
//! Python tool, but emits structured data instead of HTML so the visual
//! semantics stay unit-testable and the GUI stays a thin presentation layer.

use crate::geometry::{Geometry, Slot};
use crate::model::{Config, HoldKind, Layer};
use crate::prettify::{base_legend, prettify};
use crate::style::{accent_for, mod_name, REMAP_ACCENT};

/// A small corner badge on a key cap (hold target, `HOLD`, or chord marker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Badge {
    pub text: String,
    /// Background color (hex).
    pub color: String,
}

/// Visual state of a key on a board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyState {
    /// Normal cap.
    Normal,
    /// Dimmed (unchanged on this layer).
    Dim,
    /// The key you hold to reach this layer (highlighted).
    Hold,
}

/// One rendered key cap, positioned in key units (see [`crate::geometry::Slot`]).
#[derive(Debug, Clone, PartialEq)] // not Eq: f32 geometry fields
pub struct KeyCap {
    /// Left edge in key units from the board's top-left.
    pub x: f32,
    /// Top edge in key units from the board's top-left.
    pub y: f32,
    /// Width in standard key units.
    pub width: f32,
    /// Height in key units.
    pub height: f32,
    /// Rotation in degrees clockwise about (`rx`, `ry`).
    pub r: f32,
    pub rx: f32,
    pub ry: f32,
    /// The keyd key name for this physical position (e.g. `a`, `space`, `leftshift`).
    /// Lets a live keypress from `keyd monitor` (same name namespace) light up the cap.
    pub key: String,
    /// The slot's physical key name verbatim (the config LHS vocabulary) — what an edit
    /// to this cap binds. Distinct from `key`, which is the *emitted* chord (post-remap)
    /// and gets rewritten for glow matching. Empty for decorative/unmapped slots.
    pub phys: String,
    /// Primary label shown on the cap.
    pub label: String,
    /// `true` => render as a large emphasized glyph (remap/override/momentary mod).
    pub emphasized: bool,
    /// Faint top-right legend showing the key's normal meaning (empty if none).
    pub ghost: String,
    /// Accent color (hex) for the cap text/border; empty => default cap color.
    pub accent: String,
    pub state: KeyState,
    /// Bottom-left badge (hold target / `HOLD`).
    pub badge_left: Option<Badge>,
    /// Bottom-right badge (chord marker).
    pub badge_right: Option<Badge>,
}

/// One board: the base layer or a single layer's overrides.
#[derive(Debug, Clone, PartialEq)] // not Eq: caps carry f32 geometry
pub struct Board {
    pub is_base: bool,
    /// `Base layer` or the layer name, uppercased.
    pub title: String,
    /// Layer accent color (hex) for the tag/emphasis; empty for the base board.
    pub accent: String,
    /// How to engage this layer (e.g. `hold F`, `toggle: ⇧ + ⇧`); empty if n/a.
    pub how: String,
    /// One-line descriptive hint.
    pub hint: String,
    /// Positioned key caps (absolute geometry; not grouped into rows).
    pub keys: Vec<KeyCap>,
    /// Board extent `(width, height)` in key units, for sizing the panel.
    pub extent: (f32, f32),
}

/// A full cheatsheet for one config: its source, profile, ids, and boards.
#[derive(Debug, Clone, PartialEq)] // not Eq: boards carry f32 widths
pub struct Sheet {
    pub source: String,
    pub profile: String,
    pub ids: Vec<String>,
    pub boards: Vec<Board>,
}

impl Sheet {
    /// Build the full cheatsheet for one parsed config on a physical [`Geometry`].
    pub fn build(cfg: &Config, source: &str, geom: &Geometry, profile: &str) -> Sheet {
        let mut boards = vec![build_base(cfg, geom)];
        // Non-game layers in declaration order, then game last (matches the
        // original render order).
        for layer in &cfg.layers {
            if layer.name == "game" {
                continue;
            }
            // A `[a+b]` composite is live only while *both* constituents are held, so it
            // renders as an overlay of them (design doc §12) rather than a standalone
            // board showing just its own keys.
            if layer.name.contains('+') {
                boards.push(build_composite(cfg, layer, geom));
            } else {
                boards.push(build_layer(cfg, layer, geom));
            }
        }
        if let Some(game) = cfg.layer("game") {
            boards.push(build_layer(cfg, game, geom));
        }
        Sheet {
            source: source.to_string(),
            profile: profile.to_string(),
            ids: cfg.ids.clone(),
            boards,
        }
    }
}

/// A blank positioned cap at `slot` (geometry filled in, semantics empty). `key` is
/// the keyd key name for the slot (empty for a decorative/unmapped slot).
fn cap_at(slot: &Slot, key: &str) -> KeyCap {
    KeyCap {
        x: slot.x,
        y: slot.y,
        width: slot.w,
        height: slot.h,
        r: slot.r,
        rx: slot.rx,
        ry: slot.ry,
        // Match the glow against the name `keyd monitor` prints (catalog slots use alt
        // names like `equal`/`minus`; monitor emits the primary `=`/`-`). Firmware-only
        // legends (`lower`/`raise`) aren't keyd keys, so they carry no glow key.
        key: {
            let c = canonical(key);
            if is_primary_keysym(c) { c.to_string() } else { String::new() }
        },
        phys: key.to_string(),
        label: String::new(),
        emphasized: false,
        ghost: String::new(),
        accent: String::new(),
        state: KeyState::Normal,
        badge_left: None,
        badge_right: None,
    }
}

/// keyd's `keycode_table` lists each key as `{ primary, alt, shifted }`, but `keyd
/// monitor` always prints the **primary** name. Configs (and our catalog slots) freely
/// use the alt name (`equal`, `minus`, `dot`) or a shifted symbol (`+`, `_`, `:`), so
/// map any of those to the primary name — otherwise the live-keypress glow can't match
/// what monitor reports. Generated from keyd v2.6.0 `src/keys.c`; unknown names pass
/// through unchanged (already primary, or a multi-key action handled elsewhere).
///
/// Right-hand modifiers are also folded to their left twin: keyd tracks every modifier by
/// its mod *bit* and re-emits the canonical key (`keys.c` `modifiers[]` — `MOD_SHIFT`
/// → leftshift, `MOD_CTRL` → leftcontrol, `MOD_SUPER` → leftmeta), so pressing right
/// shift/ctrl/meta actually emits the left one (verified against keyd's offline `test-io`,
/// even with no bindings). `rightalt` is AltGr (`MOD_ALT_GR`), a distinct mod, so it stays.
fn canonical(name: &str) -> &str {
    const ALIAS: &[(&str, &str)] = &[
        ("rightshift", "leftshift"), ("rightcontrol", "leftcontrol"), ("rightmeta", "leftmeta"),
        ("escape", "esc"), ("!", "1"), ("@", "2"), ("#", "3"),
        ("$", "4"), ("%", "5"), ("^", "6"), ("&", "7"),
        ("*", "8"), ("(", "9"), (")", "0"), ("minus", "-"),
        ("_", "-"), ("equal", "="), ("+", "="), ("Q", "q"),
        ("W", "w"), ("E", "e"), ("R", "r"), ("T", "t"),
        ("Y", "y"), ("U", "u"), ("I", "i"), ("O", "o"),
        ("P", "p"), ("leftbrace", "["), ("{", "["), ("rightbrace", "]"),
        ("}", "]"), ("A", "a"), ("S", "s"), ("D", "d"),
        ("F", "f"), ("G", "g"), ("H", "h"), ("J", "j"),
        ("K", "k"), ("L", "l"), ("semicolon", ";"), (":", ";"),
        ("apostrophe", "'"), ("\"", "'"), ("grave", "`"), ("~", "`"),
        ("backslash", "\\"), ("|", "\\"), ("Z", "z"), ("X", "x"),
        ("C", "c"), ("V", "v"), ("B", "b"), ("N", "n"),
        ("M", "m"), ("comma", ","), ("<", ","), ("dot", "."),
        (">", "."), ("slash", "/"), ("?", "/"), ("bookmarks", "favorites"),
        ("prog1", "f21"), ("prog2", "f22"), ("prog3", "f23"), ("prog4", "f24"),
    ];
    ALIAS.iter().find(|(a, _)| *a == name).map_or(name, |&(_, primary)| primary)
}

/// True if `name` is a keyd *primary* key name — the vocabulary `keyd monitor` actually
/// prints, and therefore the only kind of token a cap may carry for glow matching.
/// Generated from the primary column of keyd v2.6.0 `src/keys.c`.
///
/// This is a cheap invariant oracle: every keysym a cap claims to emit (each `+`-joined
/// part of [`KeyCap::key`]) must satisfy this, or it can never light up on a live
/// keypress — catching alt names (`equal`), shifted names (`(`), and unexpanded chords
/// (`C-left`) without authoring a layout or pressing a key. (It cannot catch a *valid*
/// token attributed to the wrong cap — that needs keyd itself as the oracle.)
pub fn is_primary_keysym(name: &str) -> bool {
    PRIMARY.contains(&name)
}

/// Every primary keysym keyd v2.6.0 recognises (the picker's fallback vocabulary when
/// `keyd list-keys` is unavailable — no keyd installed, dev, AppImage). Same source as
/// [`is_primary_keysym`]: the primary column of `src/keys.c`.
pub fn primary_keysyms() -> &'static [&'static str] {
    PRIMARY
}

/// The primary column of keyd v2.6.0 `src/keys.c` — see [`is_primary_keysym`].
const PRIMARY: &[&str] = &[
    "esc", "1", "2", "3", "4", "5", "6", "7",
    "8", "9", "0", "-", "=", "backspace", "tab", "q",
    "w", "e", "r", "t", "y", "u", "i", "o",
    "p", "[", "]", "enter", "leftcontrol", "a", "s", "d",
    "f", "g", "h", "j", "k", "l", ";", "'",
    "`", "leftshift", "\\", "z", "x", "c", "v", "b",
    "n", "m", ",", ".", "/", "rightshift", "kpasterisk", "leftalt",
    "space", "capslock", "f1", "f2", "f3", "f4", "f5", "f6",
    "f7", "f8", "f9", "f10", "numlock", "scrolllock", "kp7", "kp8",
    "kp9", "kpminus", "kp4", "kp5", "kp6", "kpplus", "kp1", "kp2",
    "kp3", "kp0", "kpdot", "iso-level3-shift", "zenkakuhankaku", "102nd", "f11", "f12", "ro",
    "katakana", "hiragana", "henkan", "katakanahiragana", "muhenkan", "kpjpcomma", "kpenter", "rightcontrol",
    "kpslash", "sysrq", "rightalt", "linefeed", "home", "up", "pageup", "left",
    "right", "end", "down", "pagedown", "insert", "delete", "macro", "mute",
    "volumedown", "volumeup", "power", "kpequal", "kpplusminus", "pause", "scale", "kpcomma",
    "hangeul", "hanja", "yen", "leftmeta", "rightmeta", "compose", "stop", "again",
    "props", "undo", "front", "copy", "open", "paste", "find", "cut",
    "help", "menu", "calc", "setup", "sleep", "wakeup", "file", "sendfile",
    "deletefile", "xfer", "scrolldown", "scrollup", "www", "msdos", "coffee", "display",
    "cyclewindows", "mail", "favorites", "computer", "back", "forward", "closecd", "ejectcd",
    "ejectclosecd", "nextsong", "playpause", "previoussong", "stopcd", "record", "rewind", "phone",
    "iso", "config", "homepage", "refresh", "exit", "move", "edit", "kpleftparen",
    "kprightparen", "new", "redo", "f13", "f14", "f15", "f16", "f17",
    "f18", "f19", "f20", "f21", "f22", "f23", "f24", "playcd",
    "pausecd", "scrollright", "scrollleft", "dashboard", "suspend", "close", "play", "fastforward",
    "bassboost", "print", "hp", "camera", "sound", "question", "email", "chat",
    "search", "connect", "finance", "sport", "shop", "voicecommand", "cancel", "brightnessdown",
    "brightnessup", "media", "switchvideomode", "kbdillumtoggle", "kbdillumdown", "kbdillumup", "send", "reply",
    "forwardmail", "save", "documents", "battery", "bluetooth", "wlan", "uwb", "unknown",
    "next", "prev", "cycle", "auto", "off", "wwan", "rfkill", "micmute",
    "leftmouse", "rightmouse", "middlemouse", "mouse1", "mouse2", "mouseback", "mouseforward", "fn",
    "zoom", "noop",
];

/// The keyd modifier keysym a `C`/`M`/`A`/`S`/`G` chord prefix expands to (keyd v2.6.0
/// `keys.c` `modifiers[]`): Ctrl / Super / Alt / Shift / AltGr.
fn mod_keysym(c: u8) -> Option<&'static str> {
    Some(match c {
        b'C' => "leftcontrol",
        b'M' => "leftmeta",
        b'A' => "leftalt",
        b'S' => "leftshift",
        b'G' => "rightalt",
        _ => return None,
    })
}

/// True for a keyd *shifted* key name (`+`, `:`, `(`, `A`, …) — typing it emits the base
/// key with Shift held. Generated from the shifted column of keyd v2.6.0 `src/keys.c`.
fn is_shifted_name(t: &str) -> bool {
    const SHIFTED: &[&str] = &[
        "!", "@", "#", "$", "%", "^", "&", "*",
        "(", ")", "_", "+", "Q", "W", "E", "R",
        "T", "Y", "U", "I", "O", "P", "{", "}",
        "A", "S", "D", "F", "G", "H", "J", "K",
        "L", ":", "\"", "~", "|", "Z", "X", "C",
        "V", "B", "N", "M", "<", ">", "?",
    ];
    SHIFTED.contains(&t)
}

/// The set of keysyms a binding *emits*, canonicalised to the names `keyd monitor` prints
/// and joined with `+` for matching the live-keypress glow: `C-left` -> `leftcontrol+left`,
/// `(` -> `leftshift+9`, `equal` -> `=`. Returns `None` for actions/sequences with no
/// fixed single-chord output (`macro(...)`, `layer(...)`, a space-separated sequence).
fn output_chord(val: &str) -> Option<String> {
    let v = val.trim();
    if v.is_empty() || v.contains('(') || v.contains(' ') {
        return None;
    }
    // Strip leading `X-` modifier prefixes (keyd: `while (c[1] == '-')`).
    let mut parts: Vec<&str> = Vec::new();
    let mut c = v;
    while c.len() >= 2 && c.as_bytes()[1] == b'-' {
        let m = mod_keysym(c.as_bytes()[0])?; // unknown prefix => not a plain chord
        if !parts.contains(&m) {
            parts.push(m);
        }
        c = &c[2..];
    }
    if c.is_empty() {
        return None; // dangling modifiers, no key
    }
    let key = canonical(c);
    if !is_primary_keysym(key) {
        return None; // not a real keyd key (layer name, unknown action) -> nothing to glow
    }
    // A shifted key name (`(`, `:`, `A`) carries an implicit Shift.
    if is_shifted_name(c) && !parts.contains(&"leftshift") {
        parts.push("leftshift");
    }
    if !parts.contains(&key) {
        parts.push(key);
    }
    Some(parts.join("+"))
}

/// The last tap/hold (or momentary) binding for a physical key. Last-wins mirrors
/// the original dict comprehension over `cfg.holds`.
fn last_hold_for<'a>(cfg: &'a Config, key: &str) -> Option<&'a crate::model::Hold> {
    cfg.holds.iter().rev().find(|h| h.key == key)
}

/// The chord target whose chord includes `key` (last-wins, mirroring the dict).
fn chord_target_for(cfg: &Config, key: &str) -> Option<String> {
    cfg.chords
        .iter()
        .rev()
        .find(|(chord, _)| chord.split('+').any(|p| p.trim() == key))
        .map(|(_, target)| target.clone())
}

/// Whether `key` participates in any general (non-toggle) combo. Such keys get a
/// small `⊕` badge so a freshly-added `j+k = esc` is visible on the board (toggle
/// chords carry the louder `⇧⇧` badge instead — see `build_base`).
fn in_combo(cfg: &Config, key: &str) -> bool {
    cfg.combos.iter().any(|(chord, _)| chord.split('+').any(|p| p.trim() == key))
}

/// Whether `key` is a member of any chord declared *within this layer* — gets the
/// same `⊕` badge as a base combo, on the layer's own board.
fn in_layer_combo(layer: &Layer, key: &str) -> bool {
    layer.combos.iter().any(|(chord, _)| chord.split('+').any(|p| p.trim() == key))
}

fn build_base(cfg: &Config, geom: &Geometry) -> Board {
    let mut keys = Vec::new();
    for slot in &geom.slots {
        // Decorative / unmapped slot: a dim blank cap holding its place.
        let Some(name) = slot.key.as_deref() else {
            let mut blank = cap_at(slot, "");
            blank.state = KeyState::Dim;
            keys.push(blank);
            continue;
        };
        let mut cap = cap_at(slot, name);

        if let Some(h) = last_hold_for(cfg, name) {
            let col = if h.kind == HoldKind::Mod {
                accent_for("control")
            } else {
                accent_for(&h.target)
            };
            let label_text = if h.kind == HoldKind::Mod {
                mod_name(&h.target).to_string()
            } else {
                h.target.clone()
            };
            cap.accent = col.to_string();
            match &h.tap {
                // Pure momentary modifier/layer: the key simply *is* that function.
                None => {
                    cap.emphasized = true;
                    cap.label = label_text;
                    cap.ghost = base_legend(name);
                }
                Some(tap) => {
                    cap.label = prettify(tap);
                    // A tap emits the tap action; glow on that, not the physical key.
                    if let Some(out) = output_chord(tap) {
                        cap.key = out;
                    }
                    cap.badge_left = Some(Badge {
                        text: format!("\u{2193}{label_text}"), // ↓<target>
                        color: col.to_string(),
                    });
                }
            }
        } else if let Some(val) = cfg.remap(name) {
            cap.accent = REMAP_ACCENT.to_string();
            cap.emphasized = true;
            cap.label = prettify(val);
            cap.ghost = base_legend(name);
            // Remapped keys emit the remap target; glow matches that, not the physical key.
            if let Some(out) = output_chord(val) {
                cap.key = out;
            }
        } else {
            cap.label = base_legend(name);
        }

        if let Some(target) = chord_target_for(cfg, name) {
            cap.badge_right = Some(Badge {
                text: "\u{21e7}\u{21e7}".to_string(), // ⇧⇧
                color: accent_for(&target).to_string(),
            });
        } else if in_combo(cfg, name) {
            // A general combo (`j+k = esc`): a single badge slot, so this only shows
            // when the key isn't already wearing the louder toggle `⇧⇧`.
            cap.badge_right = Some(Badge {
                text: "\u{2295}".to_string(), // ⊕
                color: REMAP_ACCENT.to_string(),
            });
        }

        keys.push(cap);
    }

    Board {
        is_base: true,
        title: "Base layer".to_string(),
        accent: String::new(),
        how: String::new(),
        // No hint on the base board: it duplicated the window's global legend. Layer
        // boards still carry a hint (how the layer is reached / what it does).
        hint: String::new(),
        keys,
        extent: geom.extent(),
    }
}

fn build_layer(cfg: &Config, layer: &Layer, geom: &Geometry) -> Board {
    let name = &layer.name;
    let accent = accent_for(name).to_string();
    let is_game = name == "game";

    // First binding whose hold engages this layer (the key you hold).
    let act_key = cfg.holds.iter().find(|h| &h.target == name).map(|h| h.key.clone());
    let chord = cfg.chords.iter().find(|(_, t)| t == name).map(|(c, _)| c.clone());

    let (how, hint) = if is_game {
        let how = match &chord {
            Some(c) => {
                let keys = c
                    .split('+')
                    .map(|p| base_legend(p.trim()))
                    .collect::<Vec<_>>()
                    .join(" + ");
                format!("toggle: {keys}")
            }
            None => "toggle layer".to_string(),
        };
        (how, "passthrough \u{2014} these revert to plain keys (gaming)".to_string())
    } else {
        let how = match &act_key {
            Some(k) => format!("hold {}", base_legend(k)),
            None => String::new(),
        };
        (how, "highlighted keys change while held".to_string())
    };

    let mut keys = Vec::new();
    for slot in &geom.slots {
        let Some(nm) = slot.key.as_deref() else {
            let mut blank = cap_at(slot, "");
            blank.state = KeyState::Dim;
            keys.push(blank);
            continue;
        };
        let mut cap = cap_at(slot, nm);
        if let Some(val) = layer.get(nm) {
            cap.label = if is_game { base_legend(nm) } else { prettify(val) };
            cap.emphasized = true;
            cap.ghost = if is_game { String::new() } else { base_legend(nm) };
            cap.accent = accent.clone();
            // Glow on what the remapped key emits (a num-layer `j = 4` glows the j-cap
            // when keyd reports `4`). Game/passthrough keys still emit their own key.
            if !is_game {
                cap.key = output_chord(val).unwrap_or_default();
            }
        } else if act_key.as_deref() == Some(nm) {
            cap.label = base_legend(nm);
            cap.accent = accent.clone();
            cap.state = KeyState::Hold;
            cap.badge_left = Some(Badge { text: "HOLD".to_string(), color: accent.clone() });
            // Held to reach this layer — keyd emits nothing for it, so never glow it.
            cap.key = String::new();
        } else {
            cap.label = base_legend(nm);
            cap.state = KeyState::Dim;
        }
        // A chord declared in this layer (`j+k = esc` under `[nav]`): badge each member
        // so it's visible on the layer's own board, just like a base combo.
        if in_layer_combo(layer, nm) {
            cap.badge_right = Some(Badge {
                text: "\u{2295}".to_string(), // ⊕
                color: accent.clone(),
            });
        }
        keys.push(cap);
    }

    Board {
        is_base: false,
        title: name.to_uppercase(),
        accent,
        how,
        hint,
        keys,
        extent: geom.extent(),
    }
}

/// Build a composite layer's board (`[nav+sym]`): its bindings are live only while
/// *all* its constituent layers are held at once, so rendering only the section's own
/// overrides (as [`build_layer`] would) hides that nav's and sym's bindings are *also*
/// active — the "orphan layer" look (design doc §12). Instead overlay the constituents:
/// every key the held stack affects shows its effective binding, tinted by the layer
/// that sets it (nav-blue, sym-purple, the combo's own overrides in the remap accent),
/// so the picture matches what the keyboard does while you hold the whole stack.
fn build_composite(cfg: &Config, layer: &Layer, geom: &Geometry) -> Board {
    let name = &layer.name;
    let own_accent = accent_for(name).to_string();
    // Constituents in name order (`nav+sym` → [nav, sym]). For a key two of them bind,
    // the later one wins (keyd's true precedence is runtime activation order — config
    // order is the faithful static proxy); the composite's own override beats them all.
    let parts: Vec<&str> = name.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
    // Each constituent's hold key (what you press to engage it) + its accent — drives
    // the "hold X + Y" header and the per-cap HOLD badge.
    let holders: Vec<(String, String)> = parts
        .iter()
        .filter_map(|p| {
            cfg.holds.iter().find(|h| &h.target == p).map(|h| (h.key.clone(), accent_for(p).to_string()))
        })
        .collect();
    let how = if holders.is_empty() {
        String::new()
    } else {
        let keys = holders.iter().map(|(k, _)| base_legend(k)).collect::<Vec<_>>().join(" + ");
        format!("hold {keys}")
    };

    let mut keys = Vec::new();
    for slot in &geom.slots {
        let Some(nm) = slot.key.as_deref() else {
            let mut blank = cap_at(slot, "");
            blank.state = KeyState::Dim;
            keys.push(blank);
            continue;
        };
        let mut cap = cap_at(slot, nm);
        // The effective binding for this key when the whole stack is held, with the
        // accent of the layer it came from: the composite's own override first, else
        // the last constituent (name order) that binds it.
        let bound: Option<(&str, String)> = match layer.get(nm) {
            Some(v) => Some((v, own_accent.clone())),
            None => parts
                .iter()
                .rev()
                .find_map(|p| cfg.layer(p).and_then(|l| l.get(nm)).map(|v| (v, accent_for(p).to_string()))),
        };
        if let Some((val, accent)) = bound {
            cap.label = prettify(val);
            cap.emphasized = true;
            cap.ghost = base_legend(nm);
            cap.accent = accent;
            cap.key = output_chord(val).unwrap_or_default();
        } else if let Some((_, accent)) = holders.iter().find(|(k, _)| k == nm) {
            // A key held to engage one of the constituents — emits nothing, so no glow.
            cap.label = base_legend(nm);
            cap.accent = accent.clone();
            cap.state = KeyState::Hold;
            cap.badge_left = Some(Badge { text: "HOLD".to_string(), color: accent.clone() });
            cap.key = String::new();
        } else {
            cap.label = base_legend(nm);
            cap.state = KeyState::Dim;
        }
        // The composite's own chords (`j+k = …` under `[nav+sym]`) badge their members.
        if in_layer_combo(layer, nm) {
            cap.badge_right = Some(Badge { text: "\u{2295}".to_string(), color: own_accent.clone() });
        }
        keys.push(cap);
    }

    Board {
        is_base: false,
        title: name.to_uppercase(),
        accent: own_accent,
        how,
        hint: "live only while all parts are held \u{2014} each key tinted by the layer that sets it"
            .to_string(),
        keys,
        extent: geom.extent(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_keysyms_is_the_oracle_list() {
        // The exposed fallback list and the predicate share one source.
        let all = primary_keysyms();
        assert!(all.contains(&"esc"));
        assert!(all.contains(&"noop"));
        assert!(all.iter().all(|k| is_primary_keysym(k)));
        assert!(!is_primary_keysym("equal")); // alt name, not primary
    }

    fn cap_named<'a>(board: &'a Board, name: &str) -> &'a KeyCap {
        board.keys.iter().find(|c| c.phys == name).expect("slot present")
    }

    #[test]
    fn composite_board_overlays_its_constituents() {
        // [nav+sym] must render as an overlay of nav + sym (not a standalone "orphan"
        // board of only its own keys): a key nav binds shows in nav's accent, a key sym
        // binds in sym's, the combo's own key in the remap accent, base keys stay dim.
        let geom = Geometry::from_rows(&[&[
            ("a", 1.0),
            ("b", 1.0),
            ("c", 1.0),
            ("d", 1.0),
            ("capslock", 1.0),
            ("tab", 1.0),
        ]]);
        let cfg = crate::parser::parse_text(
            "[ids]\n*\n\n[main]\ncapslock = overload(nav, esc)\ntab = overload(sym, tab)\n\n\
             [nav]\na = left\n\n[sym]\nb = 1\n\n[nav+sym]\nc = f1\n",
        );
        let layer = cfg.layer("nav+sym").expect("composite layer parsed");
        let board = build_composite(&cfg, layer, &geom);

        // The composite's own key: emphasized, in the remap accent.
        let c = cap_named(&board, "c");
        assert!(c.emphasized, "composite key emphasized");
        assert_eq!(c.accent, REMAP_ACCENT);
        assert_eq!(c.label, prettify("f1"));
        // Inherited from nav → nav's accent, present (not dim).
        let a = cap_named(&board, "a");
        assert!(a.emphasized && a.state != KeyState::Dim);
        assert_eq!(a.accent, accent_for("nav"));
        // Inherited from sym → sym's accent.
        let b = cap_named(&board, "b");
        assert_eq!(b.accent, accent_for("sym"));
        // A key neither layer touches stays dim base.
        let d = cap_named(&board, "d");
        assert_eq!(d.state, KeyState::Dim);
        assert!(!d.emphasized);
        // Both constituents' hold keys are flagged, and the header names them.
        assert_eq!(cap_named(&board, "capslock").state, KeyState::Hold);
        assert_eq!(cap_named(&board, "tab").state, KeyState::Hold);
        assert!(board.how.contains(&base_legend("capslock")));
        assert!(board.how.contains(&base_legend("tab")));
    }

    #[test]
    fn general_combo_badges_both_keys_toggle_takes_precedence() {
        let geom = Geometry::from_rows(&[&[("j", 1.0), ("k", 1.0), ("x", 1.0), ("c", 1.0)]]);
        let cfg = Config {
            combos: vec![("j+k".into(), "esc".into())],
            chords: vec![("x+c".into(), "game".into())],
            ..Config::default()
        };
        let board = build_base(&cfg, &geom);
        // A general combo gets the ⊕ badge on each constituent key.
        for k in ["j", "k"] {
            let badge = cap_named(&board, k).badge_right.as_ref().expect("combo badge");
            assert_eq!(badge.text, "\u{2295}", "{k} should wear the combo badge");
        }
        // A toggle chord keeps its louder ⇧⇧ badge (single slot, toggle wins).
        for k in ["x", "c"] {
            let badge = cap_named(&board, k).badge_right.as_ref().expect("toggle badge");
            assert_eq!(badge.text, "\u{21e7}\u{21e7}", "{k} should wear the toggle badge");
        }
    }

    #[test]
    fn layer_combo_badges_members_on_layer_board() {
        // A chord declared in a layer (`j+k = esc` under [nav]) badges its members on
        // that layer's own board — the same ⊕ as a base combo, so per-layer chords are
        // visible exactly where they apply.
        let geom = Geometry::from_rows(&[&[("h", 1.0), ("j", 1.0), ("k", 1.0)]]);
        let layer = Layer {
            name: "nav".into(),
            keys: vec![("h".into(), "left".into())],
            combos: vec![("j+k".into(), "esc".into())],
            mods: None,
        };
        let cfg = Config { layers: vec![layer.clone()], ..Config::default() };
        let board = build_layer(&cfg, &layer, &geom);
        for k in ["j", "k"] {
            let badge = cap_named(&board, k).badge_right.as_ref().expect("layer combo badge");
            assert_eq!(badge.text, "\u{2295}", "{k} should wear the layer combo badge");
        }
        // A non-member key on the same layer carries no combo badge.
        assert!(cap_named(&board, "h").badge_right.is_none());
    }
}
