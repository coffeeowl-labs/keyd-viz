# Layer control + home-row mods — v1.3 design

Status: **BUILT + REVIEWED — pending Ryan live click-through, NOT yet committed.**
Plan-critiqued (blockers folded into "Resolved after plan-critic"); Feature A + Feature B
implemented; adversarial review clean (no blockers/should-fixes); UX review applied (see
"Build + review outcomes"). 375 tests pass, clippy clean.

## Build + review outcomes (2026-06-17)
- **Feature A built** — `core::layeraction` (`LayerAction`/`LayerKind` + 6 unit tests);
  `EditSession::{current_layer_action, set_layer_action}` + `current_tap_hold` now rejects
  pure momentary; 4th "layer" key-mode tab + panel in `app.slint`; `on_apply_layer_action`
  + `seed_layer_action` wired into all 6 seed sites + 3 resets; classifier order pinned at
  both sites; `th_hold_only` fully excised (0 refs); `"layer"` `--render` state added.
- **Feature B built** — appended "; the home-row-mod feel" to the existing "avoid misfires"
  hover string (`app.slint`). Zero new widgets/state. (Verified the panel is wide enough
  that the longer line doesn't elide.)
- **`overloadi` text-edit VERIFIED safe** (the previously-untested concern): a throwaway
  test confirmed an `overloadi(...)` binding opens, classifies as raw (not decomposed),
  text-edits, and saves losslessly — plus there's an existing guard at `editing.rs` that
  refuses to structurally recompose it. So punting `overloadi` UI carries no risk.
- **Adversarial review: CLEAN** — no blockers/should-fixes across classifier correctness,
  round-trip safety, stale-state, `th_hold_only` removal, orphan warnings, edge inputs,
  clobber-guard. Confirmed `set_layer_action` correctly needs no clobber guard (no in-family
  un-decomposable form exists, unlike `overloadi` for tap/hold).
- **UX review: ship-ready + 3 refinements applied** — (1) renamed title "layer action" →
  "layer key" and button "set layer action" → "set layer" (killed the 4×"layer" jargon
  stack, matches the "set tap/hold"/"set macro" convention); (2) commit button now dims
  (`enabled`) until a target layer is picked; (3) **chip-scaling past ~6 layers logged as a
  v1.4 edge** — the target picker is a chip row (consistent with the tap/hold hold-layer
  chips); a config with 10+ layers would wrap. Not fixed now because changing only this panel
  would break consistency with tap/hold — revisit both together in v1.4 if it bites.
- **Known working-tree note:** when this work began the tree already had uncommitted changes
  (a macro "chord"→"combo" rename in `edit_ui.rs`/`app.slint`, `docs/score-design.md`, and a
  ROADMAP score edit) — unrelated to layer control. Must NOT be bundled into the layer-control
  commit; Ryan to confirm their disposition.
Reopens v1.3 (already feature-complete + release-staged) to close two feature-parity
gaps a long-time keyd user would hit on day one. Modifier/layout layers and an
`[aliases]` editor are explicitly **out of scope → v1.4** (see ROADMAP.md).

## Why reopen v1.3

A keyd veteran's muscle memory includes persistent layer toggles and one-shot layers.
Today the editor can *display* `toggle()`/`oneshot()` (humanizer recognises them) and
round-trips them losslessly, but the only way to *create* one is to type raw keyd
syntax into the simple text field. That reads as "this GUI doesn't really understand
layers" to exactly the audience most likely to evangelise the tool. We close that
before tagging.

---

## Feature A — Layer-action picker (the headline)

### Current state (verified)
- `toggle(L)`, `oneshot(L)`, `setlayout(L)`, `swap(L)`, `clear()` round-trip
  losslessly as raw RHS text (confirmed: 194 core tests + a throwaway round-trip test).
  Nothing is broken — they're only *undiscoverable*.
- Momentary `layer(L)` with no tap is **already** editable, but it lives in the
  tap/hold panel as the "hold only" toggle (`taphold.rs:24`, `MOMENTARY_FUNC`).

### Design
Add a 4th key-mode tab **"layer"** alongside `simple` / `tap/hold` / `macro`
(`app.slint:1649–1675`). The panel:

```
Key-mode tabs:  [Simple] [Tap/Hold] [Macro] [Layer]   ([⌨ chord] is board-mode, separate)

Layer mode:
  Target layer:  ( nav        ▾ )        ← reuse the existing hold_layers model
  Behavior:      (●) Momentary — active only while held   → layer(nav)
                 ( ) Toggle    — latch on/off              → toggle(nav)
                 ( ) One-shot  — applies to the next key   → oneshot(nav)
  [ set layer action ]   [ unbind ]
```

### Decision D1 — momentary `layer()` moves OUT of tap/hold into Layer mode
This is the crux (recon flagged the overlap). Chosen approach: **clean separation.**
- tap/hold panel becomes strictly *dual-function* — it always has a tap. Its
  "hold only" affordance is **removed**; pure momentary layer now lives in Layer mode.
- Layer mode owns the three pure layer-activation forms: `layer` / `toggle` / `oneshot`.
- Select-time classifier precedence (`main.rs:670–677`): a binding that parses as a
  pure `layer(L)` / `toggle(L)` / `oneshot(L)` (single arg, no tap) seeds **layer**
  mode; `overload*`/`lettermod` (have a tap) seed **taphold**; macro → macro; else simple.

Mental model after: *tap/hold = "tap does X, hold does Y"; Layer = "this key drives a
layer."* No construct is claimable by two modes.

Alternatives considered (for the critic to weigh):
- **A2** keep momentary in tap/hold, Layer mode only does toggle/oneshot — rejected:
  splits "momentary layer" across two panels; Layer radio missing its most basic option.
- **A3** both panels can emit `layer()`, classifier prefers Layer — rejected: redundant,
  two ways to make the identical binding is a support-question generator.

### Core changes
- `editing.rs`: new mutator `set_layer_action(&self, layer, key, target, kind)` where
  `kind ∈ {momentary, toggle, oneshot}` → serialises `layer(t)` / `toggle(t)` /
  `oneshot(t)` via the existing `set_layer_binding` primitive (same path `set_tap_hold`
  uses). ~one-line RHS compose; no new serializer.
- `editing.rs`: reader `current_layer_action(layer, key) -> Option<(target, kind)>` for
  seeding the panel + driving the classifier. Must reject forms with a tap so it doesn't
  steal `overload(nav, esc)`.
- No `taphold.rs` model change for the data; but `TapHold::parse`/`seed_tap_hold` stop
  treating pure `layer()` as a tap/hold (D1) — momentary handling migrates to the reader
  above. **Verify**: nothing else depends on `TapHold` momentary (`compose` momentary
  path, the `--render` "tap-hold" harness state).

### Slint changes (`app.slint`)
- 4th mode tab; properties `layer_action_target: string`, `layer_action_kind: string`;
  callback `apply_layer_action()`. Reuse `hold_layers` for the dropdown.
- New panel block guarded by `key_mode == "layer"` (clone tap/hold panel structure).

### main.rs glue
- `on_apply_layer_action`: read target + kind → `s.set_layer_action(...)` → `commit_edit`
  (mirror `on_apply_tap_hold`, `main.rs:1034`).
- Extend the select-time classifier per D1.
- Refresh `layer_action_target` dropdown wherever `hold_layers` is refreshed
  (`main.rs:709/797/857`, `edit_ui.rs:427`).

### Edge cases
- `oneshot()`/`toggle()` pointing at a non-existent layer → already covered: `LAYER_FNS`
  (`refs.rs:89`) includes `layer`/`oneshot`/`toggle`, so `orphan_layer_refs` flags all
  three today. `set_layer_action` must write via `set_layer_binding` and
  `on_apply_layer_action` must call `refresh_warnings` in its commit closure.
- Self-reference / `[main]` toggle — keyd allows it; don't special-case-block, just warn
  if orphan.
- Target dropdown lists named layers only (same set tap/hold offers via `hold_layer_choices`),
  which already excludes composite `a+b` — see D2.

### Resolved after plan-critic (BLOCKERs → decisions)
- **D-CLS classifier precedence (pinned):** `macro → layer_action → taphold → simple`.
  `current_layer_action(layer,key) -> Option<(target,kind)>` matches **only** bare
  `layer(L)`/`toggle(L)`/`oneshot(L)` with **arity 1, no tap**. It MUST return `None` for:
  tap-bearing forms (`overload(L,tap)`), the macro-variant `…m`/`…k` forms
  (`layerm`/`oneshotm`/`togglem`/`oneshotk`, arity>1 → stay simple/raw), and **composite
  targets** `layer(a+b)` (→ stay simple/raw, consistent with `hold_layer_choices`
  excluding `+`). Add explicit classifier tests for each. (keyd `layer()` takes a bare
  layer name — no `:` qualifier ambiguity exists.)
- **D-MOM momentary ownership (decided):** `TapHold::parse`/`compose` momentary handling
  stays for the **viewer/humanizer** (changing it would alter read-only headlines —
  out of scope). In **edit mode**, `current_layer_action` takes precedence over
  `current_tap_hold`, so pure `layer()` seeds Layer mode, not tap/hold. `set_layer_action`
  emits `layer(target)` directly (trivial `format!`, identical output by construction).
- **D-HOLDONLY remove "hold only" (cross-file, NOT a no-op):** delete `th_hold_only` from
  `app.slint` (props ~726; panel refs 2149/2152/2157/2167-2169/2177), drop the
  `get_th_hold_only()` branch in `on_apply_tap_hold` (`main.rs:1047` — tap is now always
  `Some`), and remove `set_th_hold_only` in `seed_tap_hold` (`edit_ui.rs:342/357`). Result:
  tap/hold panel is strictly dual-function. Consequence: demoting a dual-function key to
  hold-only is now a mode switch (acceptable; cleaner model). Keep `compose_demote_to_*`
  unit tests as invariants even though GUI-unreachable.
- **D-SEED reset wiring (mirror the macro_draft hazard):** add `seed_layer_action` at
  **every** `seed_tap_hold` call site (6: `on_select_key`, `on_pick_edit_layer`, the
  apply-commit closures, `enter_edit_session`) and clear `layer_action_target/kind` in the
  layer create/delete/rename "no key selected" resets (`main.rs:~718/805/867`). Missing one
  = stale target committed onto the wrong key.
- **D2 swap/clear/setlayout excluded from v1.3 (decided):** `clear()` takes no layer arg
  (doesn't fit the target+behavior shape); `setlayout(L)` targets a *layout* namespace, not
  a layer (a target dropdown would mis-suggest it); `swap` is subtle/rare. All three
  round-trip as raw text in simple mode already, so exclusion costs nothing. Revisit `swap`
  only on user request. **Do not** put `clear`/`setlayout` in a layer-target dropdown.
- **D-RENDER UX gate:** add a `"layer"` state to the `--render` harness (`main.rs:145-199`)
  seeded from a config containing a pure `layer(L)` binding, so the new panel can be
  screenshotted for the UI/UX review.

---

## Feature B — Home-row mods (reshaped after verification)

### Finding that reshapes this
Home-row mods are **already buildable today** and were mis-scoped in the first pass.
`lettermod` is in `parser::TAPHOLD` and is the tap/hold panel's "avoid misfires"
("safe") feel (`taphold.rs:45–47, 64`). Modifier hold-targets
(control/shift/alt/meta/altgr) are already offered (`app.slint:2125`, `hold_mods`).
So: select home-row key `f`, hold = `shift`, tap = `f`, feel = "avoid misfires"
→ `lettermod(shift, f, 150, 200)` — that *is* a home-row mod. `lettermod` is precisely
keyd's home-row-mod convenience wrapper (`overloadi(key, overloadt2(layer,key,hold), idle)`).

### Remaining genuine gaps
1. **Discoverability** — nothing signposts that the "avoid misfires" feel + a modifier
   hold *is* the home-row-mod recipe. A veteran searching for "home-row mods" won't
   recognise it.
2. **`overloadi` decompose** — the raw idle-only form (no `overloadt2` hold) is not in
   `TAPHOLD`, so a hand-written `overloadi(...)` config shows as raw text (still
   round-trips, still editable as text). `lettermod` is the superset for *new* authoring,
   so this only affects opening pre-existing hand-rolled configs.

### Hard constraint — minimalism (Ryan, explicit)
Every *additional* control we add to home-row mods must be 100%-necessary and obey
"keep it simple, expose only what matters." This matches the existing design language:
feels are named by **outcome** ("fast typing" / "avoid misfires"), and keyd's per-key
millisecond knobs are deliberately *never* surfaced for editing (preserved verbatim if
already in the file — `taphold.rs:4–8, 28–30`). Ryan's own layout uses D/F as
layer-on-hold home-row mods; the bar is "expose the recipe clearly," **not** "add
tuning granularity."

**Rejected as non-essential (do NOT add):** idle-timeout slider, hold-timeout slider,
per-modifier timing, a separate `overloadi` feel, bilateral-combination toggles. If a
config already carries custom timings we keep them verbatim; we do not offer to edit them.

### Resolved v1.3 scope — ONE line of copy, zero new controls (post plan-critic)
The churn risk for home-row mods is a **search-time vocabulary gap**, not a missing
control: a veteran scanning the panel doesn't see the words "home-row mod". The feel +
modifier-target data path is already complete.
- **The change:** append the keyd term to the existing "avoid misfires" feel hover-detail
  string (`app.slint:~2241`) → e.g. "ignores quick taps — won't trigger by accident; the
  home-row-mod feel". This line already renders and is blank at rest, so: no new widget,
  no new state/property/callback, no layout change. Discoverable on hover (where a curious
  veteran probes), invisible at rest (no clutter).
- **CUT (violate minimalism):** the conditional "this is a home-row mod" hint (fires after
  you've already built one — condescending, helps too late) and the one-click preset
  (redundant third path to a 2-click result; "which modifier?" is unanswerable).
- **`overloadi` decompose → v1.4** (confirmed correct: not in `parser::TAPHOLD`; round-trips
  as raw text; `lettermod` is the superset for new authoring, so it only affects opening
  pre-existing hand-rolled configs).
- **"HOW it's implemented" granularity** (Ryan's D/F observation): the essential HOW-choices
  — layer-hold vs modifier-hold, and fast vs safe disambiguation — are already exposed (hold
  chips + feel chips). The momentary *layer*-hold (D→numbers) is delivered by **Feature A's
  Layer mode**. The only axis we keep refusing is per-key millisecond timing.

---

## Test plan
- `taphold.rs` / `editing.rs` unit tests: `set_layer_action` for all three kinds;
  `current_layer_action` rejects tap-bearing forms; round-trip of `toggle`/`oneshot`.
- Classifier: `layer(nav)` → layer mode, `overload(nav,esc)` → taphold, `toggle(x)` →
  layer, `oneshot(x)` → layer.
- Orphan warning fires for `toggle(missing)`.
- Existing 367 tests stay green; `cargo clippy` clean.

## Review gates (per Ryan's process)
Each feature, after implementation:
1. **Adversarial "try to break it" review** — verify findings, don't rubber-stamp
   (see [[adversarial-review-before-release]]).
2. **UI/UX review** — render-harness screenshot + critique
   (see [[render-harness-screenshot-capture]]).
GUI behaviour is hand-verified by Ryan (can't auto-drive Slint on live Wayland).

## Release plumbing impact
v1.3 CHANGELOG + AUR are staged. Once these land: refresh CHANGELOG [1.3.0] entry,
AUR `sha256` re-pends on the new tag (post-tag `updpkgsums` + `printsrcinfo`).
