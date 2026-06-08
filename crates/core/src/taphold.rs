//! The editor's model of a dual-function ("tap/hold") binding — VIA's Mod-Tap /
//! Layer-Tap, mapped onto keyd. We deliberately model only the refined common
//! case the GUI exposes: a **tap** action plus a **hold** target that is either a
//! layer or a modifier. keyd's per-key timeout knobs are *not* surfaced for
//! editing — but any timeouts already written in the file are preserved verbatim
//! (see [`TapHold::rest`]), so a GUI edit never silently retunes a config the user
//! hand-tuned. The viewer already renders these via [`crate::model::Hold`]; this
//! type is the editor-side compose/decompose half.
//!
//! Editable forms (all share `func(target, tap, …timeouts)` arg order, per
//! [`crate::parser::TAPHOLD`]) plus the momentary `layer(target)` (no tap):
//! `overload`, `overloadt`, `overloadt2`, `lettermod`, `layer`. Exotic shapes
//! (`overloadi` — tap-first, descriptor hold; opaque holds) are intentionally
//! *not* decomposed: [`TapHold::parse`] returns `None` and the panel leaves them
//! as raw text for hand-editing.

use crate::parser::{is_mod, parse_fn, TAPHOLD};

/// keyd's modifier targets a hold can map to (mirrors [`crate::parser::MODS`]).
/// The UI offers these alongside the config's layers as "when held" choices.
pub const MODIFIERS: [&str; 5] = crate::parser::MODS;

/// The momentary (hold-only, no tap) function.
const MOMENTARY_FUNC: &str = "layer";

/// The user-facing tap/hold *feel* — named by outcome, each mapping to a keyd
/// function with baked-in timeouts (keyd's per-key ms are never surfaced). On edit
/// a key's existing timings are preserved; these defaults apply only to a brand-new
/// key or a deliberate feel *switch*. We hard-cap the editable forms to these two
/// (plus momentary) — power users hand-edit the file for anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behavior {
    /// `overloadt2` — permissive hold, resolves on release order, no idle guard.
    /// Engages on overlap with no pause; best for fast home-row typing.
    Responsive,
    /// `lettermod` — idle guard plus hold timeout: ignores quick taps and engages
    /// only on a deliberate hold. Best for keys an accidental hold would ruin.
    TypingSafe,
}

impl Behavior {
    /// The keyd function this feel emits for a fresh binding.
    pub fn func(self) -> &'static str {
        match self {
            Behavior::Responsive => "overloadt2",
            Behavior::TypingSafe => "lettermod",
        }
    }

    /// Baked-in default timeout args for a fresh binding of this feel. Single
    /// backstop ms for `overloadt2`; idle + hold ms for `lettermod`.
    fn default_rest(self) -> Vec<String> {
        match self {
            Behavior::Responsive => vec!["200".to_string()],
            Behavior::TypingSafe => vec!["150".to_string(), "200".to_string()],
        }
    }

    /// Which feel an existing function corresponds to — `None` for forms outside
    /// the two-behavior model (plain `overload`, momentary `layer`, exotic forms).
    pub fn from_func(func: &str) -> Option<Behavior> {
        match func {
            "overloadt2" => Some(Behavior::Responsive),
            "lettermod" => Some(Behavior::TypingSafe),
            _ => None,
        }
    }
}

/// A decomposed tap/hold binding the editor can read, edit, and re-serialize.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapHold {
    /// The keyd function as written (`overload`, `overloadt`, `overloadt2`,
    /// `lettermod`, or `layer` for the momentary form). Preserved across edits so
    /// a `lettermod(...)` stays a `lettermod(...)`; new keys use `overload`.
    pub func: String,
    /// Hold target — a layer name or a modifier (control/shift/alt/meta/altgr).
    pub target: String,
    /// The tap action. `None` is a momentary hold-only key (`func == "layer"`).
    pub tap: Option<String>,
    /// Args after target+tap (the timeouts), kept verbatim so editing the target
    /// or tap preserves the user's tuned timings. Empty for `overload`/`layer`.
    rest: Vec<String>,
}

impl TapHold {
    /// Decompose a binding RHS into a tap/hold the editor can present in slots.
    /// `key` supplies the implicit tap for the `overload(layer)` short form (keyd
    /// defaults the tap to the physical key). Returns `None` for anything that is
    /// not one of the editable forms — the caller then treats the value as raw.
    pub fn parse(key: &str, rhs: &str) -> Option<TapHold> {
        let (name, args) = parse_fn(rhs.trim())?;
        if TAPHOLD.contains(&name) && !args.is_empty() {
            return Some(TapHold {
                func: name.to_string(),
                target: args[0].to_string(),
                // keyd defaults an omitted tap to the physical key.
                tap: Some(args.get(1).copied().unwrap_or(key).to_string()),
                rest: args.iter().skip(2).map(|s| s.to_string()).collect(),
            });
        }
        if name == MOMENTARY_FUNC && args.len() == 1 {
            return Some(TapHold {
                func: MOMENTARY_FUNC.to_string(),
                target: args[0].to_string(),
                tap: None,
                rest: Vec::new(),
            });
        }
        None
    }

    /// The keyd binding text for this tap/hold, ready to write as the RHS. keyd's
    /// own formatting uses `, ` between args (matches the parser, which skips the
    /// space after each comma).
    pub fn serialize(&self) -> String {
        match &self.tap {
            None => format!("{}({})", self.func, self.target),
            Some(tap) => {
                let mut args = Vec::with_capacity(2 + self.rest.len());
                args.push(self.target.as_str());
                args.push(tap.as_str());
                args.extend(self.rest.iter().map(String::as_str));
                format!("{}({})", self.func, args.join(", "))
            }
        }
    }

    /// True when the hold target is a modifier (so the UI shows it as a Mod-Tap
    /// rather than a Layer-Tap).
    pub fn is_modifier_target(&self) -> bool {
        is_mod(&self.target)
    }

    /// The [`Behavior`] this binding's function corresponds to, if any (so the
    /// panel can pre-select the matching feel). `None` for the momentary `layer`
    /// form and any function outside the two-behavior model.
    pub fn behavior(&self) -> Option<Behavior> {
        Behavior::from_func(&self.func)
    }

    /// Build the tap/hold to write when the user sets the slots, preserving an
    /// existing key's function and timeouts where it makes sense:
    /// - `tap = None` → momentary `layer(target)` (drops timeouts: `layer()` takes
    ///   none).
    /// - `tap = Some`, editing a key that was already a tap-bearing form → keep its
    ///   `func` + timeouts, swap target/tap (so `lettermod(nav, f, 150, 200)` →
    ///   `lettermod(num, g, 150, 200)`).
    /// - otherwise (a brand-new dual-function key, or one promoted from momentary)
    ///   → canonical `overload(target, tap)`.
    pub fn compose(
        existing: Option<&TapHold>,
        target: String,
        tap: Option<String>,
        feel: Behavior,
    ) -> TapHold {
        let Some(t) = tap else {
            // Momentary hold-only: a layer, no tap, no timeouts — feel is moot.
            return TapHold {
                func: MOMENTARY_FUNC.to_string(),
                target,
                tap: None,
                rest: Vec::new(),
            };
        };
        // Preserve the existing function + timeouts only when the key ALREADY has
        // the chosen feel (so retargeting/retapping never retunes a hand-tuned
        // config). A new key, or a deliberate feel switch, takes the feel defaults.
        if let Some(prev) = existing {
            if prev.behavior() == Some(feel) {
                return TapHold {
                    func: prev.func.clone(),
                    target,
                    tap: Some(t),
                    rest: prev.rest.clone(),
                };
            }
        }
        TapHold {
            func: feel.func().to_string(),
            target,
            tap: Some(t),
            rest: feel.default_rest(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_overload_tap_and_hold() {
        let th = TapHold::parse("capslock", "overload(nav, esc)").unwrap();
        assert_eq!(th.func, "overload");
        assert_eq!(th.target, "nav");
        assert_eq!(th.tap.as_deref(), Some("esc"));
        assert!(!th.is_modifier_target());
        assert_eq!(th.serialize(), "overload(nav, esc)");
    }

    #[test]
    fn parse_overload_short_form_defaults_tap_to_key() {
        // `overload(nav)` taps the physical key; we materialise that explicitly.
        let th = TapHold::parse("a", "overload(nav)").unwrap();
        assert_eq!(th.tap.as_deref(), Some("a"));
        assert_eq!(th.serialize(), "overload(nav, a)");
    }

    #[test]
    fn parse_modifier_hold() {
        let th = TapHold::parse("f", "overload(control, f)").unwrap();
        assert!(th.is_modifier_target());
    }

    #[test]
    fn parse_lettermod_keeps_timeouts_verbatim() {
        let th = TapHold::parse("f", "lettermod(nav, f, 150, 200)").unwrap();
        assert_eq!(th.func, "lettermod");
        assert_eq!(th.target, "nav");
        assert_eq!(th.tap.as_deref(), Some("f"));
        assert_eq!(th.serialize(), "lettermod(nav, f, 150, 200)");
    }

    #[test]
    fn parse_overloadt2_single_timeout_round_trips() {
        // The live hhkb config uses this form (permissive hold, one backstop ms).
        let th = TapHold::parse("f", "overloadt2(nav, f, 200)").unwrap();
        assert_eq!(th.func, "overloadt2");
        assert_eq!(th.target, "nav");
        assert_eq!(th.tap.as_deref(), Some("f"));
        assert_eq!(th.serialize(), "overloadt2(nav, f, 200)");
        // Repointing the hold keeps the form and the single 200ms backstop —
        // because the chosen feel still matches the key's existing function.
        assert_eq!(th.behavior(), Some(Behavior::Responsive));
        let edited =
            TapHold::compose(Some(&th), "num".into(), Some("f".into()), Behavior::Responsive);
        assert_eq!(edited.serialize(), "overloadt2(num, f, 200)");
    }

    #[test]
    fn parse_momentary_layer_has_no_tap() {
        let th = TapHold::parse("capslock", "layer(nav)").unwrap();
        assert_eq!(th.func, "layer");
        assert_eq!(th.tap, None);
        assert_eq!(th.serialize(), "layer(nav)");
    }

    #[test]
    fn parse_rejects_non_taphold_and_exotic_forms() {
        assert!(TapHold::parse("a", "b").is_none()); // plain remap
        assert!(TapHold::parse("a", "noop").is_none());
        assert!(TapHold::parse("a", "macro(x, y)").is_none());
        // overloadi is tap-first with a descriptor hold — not decomposed here.
        assert!(TapHold::parse("a", "overloadi(a, layer(nav), 200)").is_none());
        // toggle is a chord/layer-toggle action, not a tap/hold.
        assert!(TapHold::parse("a", "toggle(game)").is_none());
    }

    #[test]
    fn compose_new_key_uses_the_feel_defaults() {
        let responsive = TapHold::compose(None, "nav".into(), Some("esc".into()), Behavior::Responsive);
        assert_eq!(responsive.serialize(), "overloadt2(nav, esc, 200)");
        let safe = TapHold::compose(None, "nav".into(), Some("esc".into()), Behavior::TypingSafe);
        assert_eq!(safe.serialize(), "lettermod(nav, esc, 150, 200)");
    }

    #[test]
    fn compose_new_momentary_uses_layer() {
        // Feel is irrelevant with no tap.
        let th = TapHold::compose(None, "nav".into(), None, Behavior::Responsive);
        assert_eq!(th.serialize(), "layer(nav)");
    }

    #[test]
    fn compose_edit_within_same_feel_preserves_timeouts() {
        let existing = TapHold::parse("f", "lettermod(nav, f, 150, 200)").unwrap();
        // Same feel (Typing-safe) + swap target/tap → hand-tuned timings survive.
        let edited =
            TapHold::compose(Some(&existing), "num".into(), Some("g".into()), Behavior::TypingSafe);
        assert_eq!(edited.serialize(), "lettermod(num, g, 150, 200)");
    }

    #[test]
    fn compose_feel_switch_applies_new_defaults() {
        // Deliberately switching feel resets to that feel's baked-in defaults.
        let existing = TapHold::parse("f", "overloadt2(nav, f, 200)").unwrap();
        let edited =
            TapHold::compose(Some(&existing), "nav".into(), Some("f".into()), Behavior::TypingSafe);
        assert_eq!(edited.serialize(), "lettermod(nav, f, 150, 200)");
    }

    #[test]
    fn compose_promote_momentary_to_dual_uses_feel_default() {
        let existing = TapHold::parse("capslock", "layer(nav)").unwrap();
        let edited =
            TapHold::compose(Some(&existing), "nav".into(), Some("esc".into()), Behavior::Responsive);
        assert_eq!(edited.serialize(), "overloadt2(nav, esc, 200)");
    }

    #[test]
    fn compose_demote_to_momentary_drops_timeouts() {
        let existing = TapHold::parse("f", "lettermod(nav, f, 150, 200)").unwrap();
        let edited = TapHold::compose(Some(&existing), "nav".into(), None, Behavior::TypingSafe);
        assert_eq!(edited.serialize(), "layer(nav)");
    }
}
