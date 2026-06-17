//! The editor's model of a **pure layer-activation** binding — keyd's `layer()`
//! (momentary, active only while held), `toggle()` (a persistent on/off latch), and
//! `oneshot()` (applies to the next keypress only). Unlike a tap/hold these carry
//! **no tap action**: the key's whole job is to drive a layer. The viewer renders
//! them via [`crate::humanizer`]; this is the editor-side compose/decompose half,
//! mirroring [`crate::taphold::TapHold`].
//!
//! Only the three **bare, single-argument** forms are modeled. The macro-/key-bearing
//! variants (`layerm`/`togglem`/`oneshotm`, `oneshotk` — arity > 1) and **composite
//! targets** (`layer(a+b)`, which the layer dropdown can't represent) are intentionally
//! *not* decomposed: [`LayerAction::parse`] returns `None` and the key stays editable
//! as raw text in simple mode — exactly as the tap/hold panel handles its exotic forms.

use crate::parser::parse_fn;

/// How a layer-activation key behaves. Named by outcome, like [`crate::taphold::Behavior`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKind {
    /// `layer(L)` — the layer is active only while the key is held.
    Momentary,
    /// `toggle(L)` — a persistent latch: tap to turn the layer on, tap again to turn it off.
    Toggle,
    /// `oneshot(L)` — the layer applies to the next keypress only, then clears.
    OneShot,
}

impl LayerKind {
    /// The keyd function name this kind serializes to.
    pub fn func(self) -> &'static str {
        match self {
            LayerKind::Momentary => "layer",
            LayerKind::Toggle => "toggle",
            LayerKind::OneShot => "oneshot",
        }
    }

    /// The UI token used to round-trip the kind through Slint (a stringly-typed prop).
    pub fn token(self) -> &'static str {
        match self {
            LayerKind::Momentary => "momentary",
            LayerKind::Toggle => "toggle",
            LayerKind::OneShot => "oneshot",
        }
    }

    /// Map a UI token back to a kind; unknown tokens fall back to momentary (the
    /// default a fresh Layer-mode key opens with).
    pub fn from_token(s: &str) -> LayerKind {
        match s {
            "toggle" => LayerKind::Toggle,
            "oneshot" => LayerKind::OneShot,
            _ => LayerKind::Momentary,
        }
    }

    fn from_func(func: &str) -> Option<LayerKind> {
        match func {
            "layer" => Some(LayerKind::Momentary),
            "toggle" => Some(LayerKind::Toggle),
            "oneshot" => Some(LayerKind::OneShot),
            _ => None,
        }
    }
}

/// A decomposed pure-layer-activation binding the editor can read and re-serialize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerAction {
    pub kind: LayerKind,
    /// The target layer name.
    pub target: String,
}

impl LayerAction {
    /// Decompose an RHS into a layer action, or `None` if it is not one of the three
    /// bare single-argument forms. Rejects (→ stay raw text): the macro/key variants
    /// (`layerm`/`togglem`/`oneshotm`/`oneshotk`, arity ≠ 1), composite targets
    /// (`a+b`), and everything else.
    pub fn parse(rhs: &str) -> Option<LayerAction> {
        let (name, args) = parse_fn(rhs.trim())?;
        let kind = LayerKind::from_func(name)?;
        if args.len() != 1 {
            return None;
        }
        let target = args[0].trim();
        // A composite target can't be represented in the layer dropdown (which lists
        // single named layers), so leave it to raw-text editing — matching the
        // tap/hold panel's exclusion of `+` layers.
        if target.is_empty() || target.contains('+') {
            return None;
        }
        Some(LayerAction {
            kind,
            target: target.to_string(),
        })
    }

    /// The keyd binding text for this action, ready to write as the RHS.
    pub fn serialize(&self) -> String {
        format!("{}({})", self.kind.func(), self.target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_the_three_bare_forms() {
        assert_eq!(
            LayerAction::parse("layer(nav)"),
            Some(LayerAction { kind: LayerKind::Momentary, target: "nav".into() })
        );
        assert_eq!(
            LayerAction::parse("toggle(game)"),
            Some(LayerAction { kind: LayerKind::Toggle, target: "game".into() })
        );
        assert_eq!(
            LayerAction::parse("oneshot(sym)"),
            Some(LayerAction { kind: LayerKind::OneShot, target: "sym".into() })
        );
    }

    #[test]
    fn serialize_round_trips_each_kind() {
        for rhs in ["layer(nav)", "toggle(game)", "oneshot(sym)"] {
            assert_eq!(LayerAction::parse(rhs).unwrap().serialize(), rhs);
        }
    }

    #[test]
    fn rejects_tap_bearing_and_arity_mismatch() {
        // overload* have a tap → tap/hold's job, not ours.
        assert!(LayerAction::parse("overload(nav, esc)").is_none());
        // Macro/key variants carry a second arg we must not silently drop.
        assert!(LayerAction::parse("layerm(nav, macro(x))").is_none());
        assert!(LayerAction::parse("togglem(game, macro(x))").is_none());
        assert!(LayerAction::parse("oneshotm(nav, macro(x))").is_none());
        assert!(LayerAction::parse("oneshotk(nav, esc)").is_none());
        // No-arg / unrelated forms.
        assert!(LayerAction::parse("clear()").is_none());
        assert!(LayerAction::parse("setlayout(dvorak)").is_none());
        assert!(LayerAction::parse("swap(nav)").is_none());
    }

    #[test]
    fn rejects_composite_target_so_it_stays_raw_text() {
        assert!(LayerAction::parse("layer(a+b)").is_none());
        assert!(LayerAction::parse("toggle(nav+sym)").is_none());
    }

    #[test]
    fn rejects_plain_remaps_and_macros() {
        assert!(LayerAction::parse("b").is_none());
        assert!(LayerAction::parse("noop").is_none());
        assert!(LayerAction::parse("macro(a b)").is_none());
    }

    #[test]
    fn token_round_trips() {
        for k in [LayerKind::Momentary, LayerKind::Toggle, LayerKind::OneShot] {
            assert_eq!(LayerKind::from_token(k.token()), k);
        }
        // Unknown token defaults to momentary.
        assert_eq!(LayerKind::from_token("nonsense"), LayerKind::Momentary);
    }
}
