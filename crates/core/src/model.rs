//! The parsed representation of a keyd config.
//!
//! Mirrors the dataclasses in the original Python `keyd_cheatsheet.py`. Maps are
//! kept as insertion-ordered `Vec`s because the renderer depends on layer order
//! (and Python's dicts preserved insertion order).

/// Whether a tap/hold binding's hold engages a *layer* or a *modifier*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldKind {
    Layer,
    Mod,
}

/// A tap/hold binding: tap `tap` (None = pure momentary, no tap action), hold
/// engages `target` (a layer or modifier, per `kind`). `key` is the physical key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hold {
    pub key: String,
    pub target: String,
    pub kind: HoldKind,
    pub tap: Option<String>,
}

/// One keyd layer section (e.g. `[nav]`) and its key overrides, order-preserving.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Layer {
    pub name: String,
    pub keys: Vec<(String, String)>,
    /// The `:` modset qualifier from the section header (`[caps:C]` → `Some("C")`):
    /// the layer behaves as those modifiers while held. `None` for plain layers and
    /// for `:layout` sections. A hold onto a modset-qualified layer classifies as a
    /// *modifier* hold (design doc §12).
    pub mods: Option<String>,
}

impl Layer {
    /// The override value bound to `key` in this layer, if any.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.keys.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// A fully parsed keyd config: device ids, layers, tap/hold bindings, chord
/// toggles, and plain remaps.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    /// `[ids]` lines (e.g. `04fe:0021`), verbatim.
    pub ids: Vec<String>,
    /// Layer sections in the order they first appeared (includes `game`, `shift`).
    pub layers: Vec<Layer>,
    /// Tap/hold bindings from `[main]` (`lettermod`/`overload*`/momentary `layer`).
    pub holds: Vec<Hold>,
    /// Chord toggles: `(chord, target_layer)`, e.g. `("leftshift+rightshift", "game")`.
    pub chords: Vec<(String, String)>,
    /// General chord bindings with a non-toggle action: `(chord, value)`, e.g.
    /// `("j+k", "esc")`. keyd canonicalises chord order (`a+b` == `b+a`); entries
    /// here keep the config's spelling. (Previously these were mis-parsed as remaps
    /// keyed by the literal chord string — design doc §12.)
    pub combos: Vec<(String, String)>,
    /// Plain remaps and unrecognized macros: `key -> value`.
    pub remaps: Vec<(String, String)>,
}

impl Config {
    /// Look up a layer section by name.
    pub fn layer(&self, name: &str) -> Option<&Layer> {
        self.layers.iter().find(|l| l.name == name)
    }

    /// The plain-remap value for `key`, if one exists.
    pub fn remap(&self, key: &str) -> Option<&str> {
        self.remaps.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}
