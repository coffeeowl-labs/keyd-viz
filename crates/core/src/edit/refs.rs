//! Layer-reference analysis over the CST: which bindings activate which layers,
//! orphan/dangling detection. The read-only graph layer; types + CST live in `super`.

use super::*;

impl EditConfig {
    /// Bindings that activate a layer this config never defines — keyd rejects such a
    /// file, so the editor can flag it *before* apply (e.g. you bound `layer(symbols)`
    /// but haven't created `[symbols]` yet, or deleted a layer something still points
    /// at). One entry per offending binding, in file order.
    ///
    /// Deliberately **high-precision over high-recall**: `keyd check` is the real
    /// gate at apply time, so a missed orphan is far cheaper than a false alarm on a
    /// valid config. Only well-known layer activators are scanned, modifier targets
    /// (keyd's built-in modifier layers) are never flagged, and composite `a+b`
    /// targets are skipped (their definition rules are subtle) — see [`layer_ref_spans`].
    pub fn orphan_layer_refs(&self) -> Vec<OrphanRef> {
        let is_layer = |s: &&Section| s.kind.is_board();
        let defined: std::collections::HashSet<&str> =
            self.sections.iter().filter(is_layer).map(|s| s.base_name().trim()).collect();

        let mut out = Vec::new();
        for s in self.sections.iter().filter(is_layer) {
            for e in &s.entries {
                let EntryKind::Binding { key, val: Some(val), .. } = &e.kind else { continue };
                for sp in layer_ref_spans(val) {
                    let layer = &val[sp];
                    if !defined.contains(layer) {
                        out.push(OrphanRef {
                            section: s.base_name().trim().to_string(),
                            key: key.clone(),
                            layer: layer.to_string(),
                        });
                    }
                }
            }
        }
        out
    }

    /// Composite layers (`[a+b]`) whose constituents aren't all real, standalone layers —
    /// keyd rejects such a file (`keyd check` exits non-zero: "`<part>` is not a valid
    /// layer"), so the editor flags it *before* apply (e.g. you deleted `[nav]` but
    /// `[nav+sym]` still lists it). One `(composite-base, missing-part)` per offending
    /// constituent, deduped, in file order. `main` counts as always-defined (keyd has an
    /// implicit base board even with no explicit `[main]`). A constituent must be a `Main`
    /// or `Layer` section — a composite can't be built from another composite.
    pub fn dangling_composites(&self) -> Vec<(String, String)> {
        let plain: std::collections::HashSet<&str> = self
            .sections
            .iter()
            .filter(|s| matches!(s.kind, SectionKind::Main | SectionKind::Layer))
            .map(|s| s.base_name().trim())
            .collect();
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for s in self.sections.iter().filter(|s| s.kind == SectionKind::Composite) {
            let comp = s.base_name().trim();
            for part in comp.split('+').map(str::trim) {
                if part != "main"
                    && !plain.contains(part)
                    && seen.insert((comp.to_string(), part.to_string()))
                {
                    out.push((comp.to_string(), part.to_string()));
                }
            }
        }
        out
    }
}

/// A binding that points at an undefined layer — `key = …layer(`layer`)…` living in
/// section `[`section`]` (base name). See [`EditConfig::orphan_layer_refs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanRef {
    pub section: String,
    pub key: String,
    pub layer: String,
}

/// Layer-activating functions whose **first** argument is a layer name — the plain
/// forms (`layer`/`oneshot`/`toggle`/`swap`) and their `…m`/`…k` variants that take an
/// extra macro/key after the layer (`layerm`/`oneshotm`/`oneshotk`/`swapm`/`togglem`,
/// verified against keyd 2.6.0). The tap/hold family ([`crate::parser::TAPHOLD`]) also
/// takes a layer first — but only when that arg isn't a modifier, which [`layer_ref_spans`]
/// guards. (`overloadi`'s first arg is an *action*, and `setlayout`'s is a *layout*, so
/// both are deliberately excluded — rewriting them would corrupt a different namespace.)
const LAYER_FNS: [&str; 9] = [
    "layer", "oneshot", "toggle", "swap", "layerm", "oneshotm", "oneshotk", "swapm", "togglem",
];

/// Whether argument `idx` of keyd function `name` is an **action** slot — a position whose
/// value is itself a binding descriptor that can nest further layer references, so the scan
/// must recurse into it. Only the `overload` family nests an action; every other layer
/// function's non-layer args are macros/keys/timeouts (never descriptors). Keeping this set
/// tight is what makes the scan high-precision: it never descends into `macro(...)` /
/// `command(...)` text, so a literal word there can't be mistaken for a layer name.
fn action_slot(name: &str, idx: usize) -> bool {
    match name {
        // (layer, action, [timeout]) — the tap-hold/tap action is arg 1.
        "overload" | "overloadt" | "overloadt2" => idx == 1,
        // (tap-action, hold-action, timeout) — both actions, no layer of its own.
        "overloadi" => idx == 0 || idx == 1,
        _ => false,
    }
}

/// Every byte range within `val` that names an activated layer — the splice points
/// [`EditConfig::rename_layer`] rewrites and the names [`EditConfig::orphan_layer_refs`]
/// checks. Covers references nested inside an action descriptor
/// (`overloadi(esc, layer(nav), 200)` → the `nav` span), not just the top-level call.
/// Modifier targets (keyd's built-in modifier layers) and composite `a+b` targets are
/// excluded — both are valid without a matching `[…]` section, so flagging them would be a
/// false alarm. Spans are in discovery order; a splicing caller must apply them right-to-left.
pub(crate) fn layer_ref_spans(val: &str) -> Vec<std::ops::Range<usize>> {
    // Every arg `parse_fn` yields is a sub-slice of this same buffer, at any depth, so a
    // span's absolute offset is just its pointer delta from `base`.
    let base = val.as_ptr() as usize;
    let mut out = Vec::new();
    collect_layer_spans(val, base, &mut out);
    out
}

/// Recursive worker for [`layer_ref_spans`]; `base` is the root value's pointer.
fn collect_layer_spans(val: &str, base: usize, out: &mut Vec<std::ops::Range<usize>>) {
    let Some((name, args)) = crate::parser::parse_fn(val) else { return };
    let is_layer_fn = LAYER_FNS.contains(&name) || crate::parser::TAPHOLD.contains(&name);
    for (i, arg) in args.iter().enumerate() {
        if is_layer_fn && i == 0 {
            // arg-0 of a layer/tap-hold function is the activated layer. Record it unless
            // it's a modifier or composite target (valid without a section), or itself a
            // call (not a plain layer name — leave such malformed input alone).
            let trimmed = arg.trim();
            if !crate::parser::is_mod(trimmed) && !trimmed.contains('+') && !trimmed.contains('(') {
                let off = arg.as_ptr() as usize - base;
                let lead = arg.len() - arg.trim_start().len();
                out.push(off + lead..off + lead + trimmed.len());
            }
            continue;
        }
        if action_slot(name, i) {
            collect_layer_spans(arg, base, out);
        }
    }
}
