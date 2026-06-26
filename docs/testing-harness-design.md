# Testing-harness expansion — design

**Status:** DRAFT (pre plan-critic) · 2026-06-26
**Goal:** Reduce manual-QA bandwidth for a solo engineer by building advanced
automated tests that (a) surface existing edge-case bugs now, and (b) form a
standing regression net so future changes need less click-through.

## Why this app is a good candidate

The value-dense logic — `EditConfig` parse/serialize (CST), `parser::derive`,
the semantic `Config`, `Sheet::build` (board model) — is **pure** (no I/O, no
privilege, no GUI). Pure functions over a structured grammar are the ideal
substrate for property testing, differential testing, and snapshotting. The only
genuinely hard-to-test surface (live Slint on Wayland) is explicitly out of scope.

## Non-goals (explicit "don't bother")

- **Pixel-diffing the `--render` PNGs.** Software-renderer output drifts across
  font/freetype/lib versions → flaky, high-maintenance, low marginal value over
  snapshotting the *semantic* board model (Component 4). The PNG harness stays
  manual, for the UX-critic pass only.
- **Driving the live GUI.** Can't auto-drive Slint on Wayland (established). We
  test the logic *behind* the callbacks (`EditSession`), not the widgets.
- **Recall-complete linting.** keyd is the gate; we don't try to reject
  everything keyd rejects (see [[editor-lints-precision-over-recall]]).

## Dependencies (dev-only — never shipped)

Added under `[dev-dependencies]` only; they do not enter the `keydviz`,
`keydviz-apply`, `keydviz-helper`, or `keydviz-core` *library* artifacts, the
AUR runtime deps, or the AppImage. Both are already in the local cargo cache.

| Crate | Version | Where | Why |
|-------|---------|-------|-----|
| `proptest` | 1.11 | `core`, `app` dev-deps | Structured generators + **shrinking** to minimal repros |
| `insta` (feat `yaml`) | 1.47 | `core` dev-dep | One-command snapshot review of the board/Config model |

The existing hand-rolled LCG `fuzz_round_trips` stays (complementary
pure-garbage-bytes coverage); proptest *adds* structured generation + shrinking.

---

## Component 1 — keyd as a differential oracle  *(highest bug-yield, no new dep)*

keyd is the real validity gate. Reuse the **existing public** helper
`editing::keyd_check_bytes(&str) -> Option<Result<(),String>>` (returns `None`
when keyd is absent / unwritable → the whole suite **auto-skips** on CI boxes
without keyd; gate additionally on an env var `KEYDVIZ_KEYD_ORACLE=1` so it never
runs by accident in normal `cargo test`).

**Inputs:** the example corpus, a mutated corpus (small single-edits of
`hhkb.conf`), and — when Component 2 lands — the proptest-generated configs.

**Properties asserted:**

- **P1-total:** for any text `t` where `round_trips(t)` holds,
  `derive(parse(t))` and `Sheet::build(...)` do **not panic**. (Pure; runs
  always, no keyd needed.)
- **P1-precision:** if keyd **accepts** `t`, then our `EditSession::open` on `t`
  must **not** return `ViewOnly::KeydRejects`, and `orphan_warnings()` (the hard
  ones we surface as errors) must be **empty**. A failure here = a real
  false-alarm bug we ship to users.
- **P1-roundtrip-vs-keyd:** if keyd accepts `t` and `round_trips(t)` holds, then
  keyd must also accept `serialize(parse(t))` (we never corrupt a valid config
  through a round-trip). 

We do **not** assert the converse (keyd-rejects ⟹ we-reject): that's the
recall direction we intentionally don't own.

**Location:** `crates/app/tests/keyd_oracle.rs` (app, because `keyd_check_bytes`
+ `EditSession` live there). Env-gated, keyd-presence-gated.

---

## Component 2 — property suite: grammar-aware generator  *(proptest)*

A `proptest` strategy `arb_keyd_config()` that emits **realistic** configs, not
byte soup: a `[ids]` block, optional `[global]`, a `[main]`, and 0–N layer
sections (`[name]`, modifier `[name:C-S]`, composite `[a+b]`). Each binding is
drawn from a weighted vocabulary covering every parser path we care about:

- plain keysym remaps (`a = b`)
- layer actions: `layer(x)`, `toggle(x)`, `oneshot(x)`
- tap/hold: `overload(x,y)`, `lettermod(...)`, `overloadt2(...)`
- macros: `macro(a b c)`, modifier shorthand, `text`, delays
- chord lines, `# keyd-viz:` label comments, blank/comment lines

Targets must reference layers that exist (so configs are mostly keyd-valid),
with a tunable knob to inject occasional orphan refs for the precision test.

**Properties (all pure, run always):**

- **P2-roundtrip:** `round_trips(gen)` for every generated config.
- **P2-idempotent:** `serialize(parse(serialize(parse(g)))) == serialize(parse(g))`.
- **P2-derive-total:** `derive(parse(g))` never panics; `Sheet::build` never panics.
- **P2-oracle (opt-in):** under the keyd env-gate, generated configs with no
  injected orphans are accepted by keyd (sanity that the generator is realistic;
  also a second feed into Component 1).

Plus a `arb_arbitrary_text()` strategy (random unicode/ascii incl. the keyd
metachars) asserting **P2-roundtrip** — a *shrinking* successor to the LCG fuzz.

**Location:** `crates/core/tests/properties.rs` + a small generator module.

---

## Component 3 — stateful `EditSession` model test  *(proptest)*

`EditSession` is the stringly-typed mutation surface that has historically
harbored bugs (classifier precedence, seed/reset wiring). Model-based test:
generate a `Vec<Op>` (enum over the ~14 mutators, args constrained to the
session's current layers/keys), apply in sequence to a session opened on a
generated-but-valid starting config (in a `TempDir`), and after **every** step
assert the invariants:

- **P3-no-panic:** no mutator panics. `Err(_)` is an allowed outcome.
- **P3-err-is-inert:** if a mutator returns `Err`, `serialized()` and `dirty()`
  are unchanged from before the call (a rejected edit must not half-apply).
- **P3-output-reparses:** `round_trips(session.serialized())` always holds —
  the editor never emits text it couldn't read back.
- **P3-derive-total:** `derive(parse(session.serialized()))` never panics.
- **P3-dirty-iff-changed:** `dirty()` is true iff `serialized() != original`.

Separate, **non-random** inverse-pair property tests (cleaner than asserting
inverses inside the random walk):

- `add_layer(n)` then `remove_layer(n)` ⟹ serialization == original.
- `set_label` then `clear_label` ⟹ == original.
- `set_binding(old)` after capturing `current_binding`, then restoring ⟹ ==.

**Location:** inline `#[cfg(test)]` in `crates/app/src/editing.rs` (reuses the
private `TempDir`/`session` helpers); add a small helper to open a session from
an arbitrary `&str`.

---

## Component 4 — board-model snapshots  *(insta)*

The `Sheet`/`Board`/`KeyCap` types already `derive(Serialize)`. For each corpus
config, build the sheet exactly as production does
(`(geom,profile)=layout_for(path); Sheet::build(&cfg, src, &geom, profile)`) and
`insta::assert_yaml_snapshot!(sheet)`. Also snapshot the semantic `Config` and
`orphan_warnings()` for a couple of representative edited states.

Any change to humanizer / accent / badge / glow logic surfaces as a reviewable
diff (`cargo insta review`) instead of silent visual drift — replacing a big
chunk of manual click-through. Snapshots live in `crates/core/tests/snapshots/`
and are committed.

**Location:** `crates/core/tests/snapshot.rs`.

---

## Component 5 — coverage measurement in CI  *(cargo-llvm-cov)*

Add a **non-gating** CI job: `cargo llvm-cov --workspace --lcov` producing a
summary + artifact, so the dark corners are visible. No enforced floor initially
(avoid a brittle gate); revisit a floor once we see the baseline. Needs
`llvm-tools-preview`; job installs `cargo-llvm-cov`.

---

## The regression discipline (process, not code)

**Rule:** every confirmed bug gets a named test that *fails before the fix and
passes after*. Name it for the symptom (the repo already does this implicitly:
`empty_set_label_on_unlabelled_key_is_ok_not_an_error`). Document the rule in
`CONTRIBUTING.md` (new) and, for proptest finds, **commit the shrunk minimal
case as a concrete `#[test]`** (proptest's regression file `.proptest-regressions`
is also committed so the seed re-runs).

---

## Rollout order

Sequenced so the bug-hunters run *before* we lock in a snapshot net — otherwise
Component 4 would enshrine current buggy behavior as "correct."

1. **cargo-mutants (local one-off)** on `core` — does the existing 375-test suite
   actually *catch* injected bugs? Highest-yield first spend, zero CI cost.
2. Add dev-deps. Component 2 (property suite) — fastest pure bug-finder, no keyd.
3. Component 1 (keyd oracle) — corpus + `/etc/keyd` history + Component 2's gen;
   **install keyd in CI** so it actually gates.
4. Component 3 (stateful + set-read-back + semantic-inverse EditSession).
5. **Triage every finding → regression test per bug. Fix.**
6. Component 4 (projected snapshots) — lock in rendering once 1–4 are green.
7. Coverage: run `cargo-llvm-cov` **once locally** for the dark-corner map; wire
   it as a `workflow_dispatch`/nightly job only, never per-PR.

## Build outcomes (2026-06-26)

All five components built + a regression pass; full suite green, clippy clean.

- **Component 2** — `crates/core/tests/properties.rs`: grammar-aware generator +
  round-trip/derive/build totality + EOL-stress + arbitrary-char round-trip. 3
  properties, pass (no live bug — confirms the critic's note that `derive` is
  already well-guarded; CST fidelity holds on arbitrary char soup).
- **Component 3** — `crates/app/src/editing.rs` (inline): stateful random walk
  (`editsession_walk_preserves_invariants`, 200 cases) + 5 set-then-read-back +
  2 semantic-inverse + a newline-robustness probe (editor correctly rejects/
  sanitizes — output always round-trips).
- **Component 1** — `crates/app/src/editing.rs` `mod keyd_oracle`: P1-total +
  P1-precision + mutation-path validity + an `/etc/keyd` discovery report. Ran
  locally with keyd: **0 disagreements** on the examples and the dev's
  `/etc/keyd` — the precision lints don't false-alarm on anything keyd accepts.
- **Component 4** — `crates/core/tests/snapshot.rs`: insta YAML snapshot of a
  geometry-projected board model (sorted by `phys`), committed under
  `tests/snapshots/`.
- **Component 5** — `coverage` CI job, `workflow_dispatch`-only; CONTRIBUTING
  documents the local `cargo-llvm-cov`/`cargo-mutants` recipes (core-capped).
- **CI** — keyd built from source in the `app` job (oracle now gates);
  `PROPTEST_CASES=64` bounds gating runs.
- **Mutation pass** — a partial `cargo-mutants -p keydviz-core` run surfaced 12
  test gaps; 9 closed with regression tests (board `mod_keysym` M/A/G,
  `in_combo` non-member, `build_composite` accent pairing; catalog `guess`
  `||`; edit `label_index`/`clear_label`/`push_comment`). 2 confirmed
  **equivalent mutants** in `output_chord` (downstream `is_primary_keysym` guard
  collapses both branches) — no test possible. Teeth verified by flipping the
  `push_comment` operator → the regression test fails as predicted.
- **Process note (thermal):** `cargo-mutants` saturates all cores; running it
  unattended in the background spiked the laptop to max temp. Heavy all-core
  jobs (mutants, llvm-cov) are local + core-capped only, never background.

## Plan-critic revisions (folded 2026-06-26)

Two adversarial critics red-teamed the draft; corrections folded in above and here:

- **DROP P1-roundtrip-vs-keyd** — tautological: `round_trips(t)` *is*
  `serialize(parse(t))==t`, so it runs keyd twice on identical bytes. The real
  property is on the **mutation path**: a valid edit of a keyd-valid config still
  passes `keyd_check_bytes`.
- **P3-dirty: assert one direction only** — `Section::set_binding` (cst.rs:216)
  sets `dirty=true` unconditionally, so a no-op edit flips dirty with bytes
  unchanged. Assert `serialized() != original ⟹ dirty()`, never the reverse.
- **Inverse pairs assert SEMANTIC equality, not bytes** — `append_section`
  injects a blank separator `remove_layer` doesn't reclaim (cst.rs:647), and
  `set_binding` normalizes `=` spacing (cst.rs:228). Use `derive(after) ==
  derive(original)` (or keyd-equivalence), not `serialize() == original`.
- **ADD set-then-read-back (the biggest gap)** — exercises the classifier/typed
  loop that historically broke: `set_binding ⟹ current_binding == Some(v)`;
  `set_layer_action→current_layer_action`; `set_tap_hold→current_tap_hold`;
  `set_macro→current_macro`; `set_chord→chords`.
- **Adversarial Op args** — a fraction of Component-3 mutation args must emit
  newlines / non-canonical spacing / current-values, or `P3-output-reparses` and
  the no-op-dirty paths are unreachable and the model finds nothing.
- **DROP P2-idempotent; round-trip generator mixes EOLs** — structured gen emits
  clean `\n` lines and can't break the splitter; its real value is
  `P2-derive-total` + `P2-oracle`. Mixed `\n`/`\r\n`/no-trailing/lone-`\r` is
  where round-trip can actually break.
- **P1-precision input guards** — `orphan_layer_refs` has no `include` resolution
  and `is_mod` knows only 5 long-form names (parser.rs:35). Keep oracle inputs
  free of `include`s; verify keyd's handling of short-form modifier targets
  (`toggle(C-S)`) before the generator emits them, else false-fire on valid configs.
- **Snapshots project OUT geometry** (requirement) — snapshot `phys/label/ghost/
  emphasized/accent/badges/state/key`, sorted by `phys`; geometry floats are
  already covered by `tests/board.rs` and would bury the semantic diff.
- **proptest determinism** — gating CI runs fixed/bounded (`PROPTEST_CASES≈64`);
  entropy-seeded exploration is local/nightly. Commit `.proptest-regressions`.
- **Coverage = local/nightly, cargo-mutants = local** — neither belongs in per-PR
  CI (instrumented Slint rebuild ~8-10min for a non-gating artifact).
- **Resolved:** reuse `keyd_check_bytes` (tests the exact GUI path) ✓; TempDir
  churn is a non-issue at ≤256 cases ✓; proptest in `core` dev-deps doesn't
  touch the lib artifact/AUR ✓.
