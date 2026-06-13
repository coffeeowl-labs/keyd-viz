# Edit-Mode UX Critique — AI Persona Panel (first pass)

> **Date:** 2026-06-12 · **Method:** the gated AI UX-critic pass from
> [ROADMAP §8](../ROADMAP.md). Four persona-primed critic agents (never-used-a-remapper
> novice · QMK/VIA power user new to keyd · impatient skimmer · low-vision/accessibility)
> each read **rendered screenshots** of 13 edit-mode states, applied a Nielsen +
> 5-second-first-impression + hesitation-walkthrough rubric, then a per-screen
> adversarial pass deduped the findings and judged each **real vs. simulated** against the
> actual pixels, and a synthesis pass triaged them like a code review. 66 agents total.
>
> **Reproduce the inputs:** `scripts/ux-screenshots.sh` → `/tmp/uxcrit/*.png` (drives the
> `--render-state` harness in `crates/app/src/main.rs`; software-rendered, faithful).
> Re-run after edit-mode UI changes and re-run the `ux-critic-edit-mode` Workflow.
>
> **Honest limits (read first):** the personas *simulate* confusion from a single static
> frame — this pass cannot measure time-on-task, eye-path, error-recovery, or
> interaction-resolved ambiguity (hover tooltips, focus rings, applied-state feedback). It
> complements, not replaces, one or two real first-run humans. Treat it as a cheap,
> repeatable first weeding.
>
> **Seed caveat:** the `apply-summary` screen used hand-seeded content (a real apply needs
> `/etc/keyd` + pkexec). **Finding B5(b)** — the `command()` banner referencing a change the
> diff doesn't show — is an artifact of that seed, **not** confirmed real-app behavior.
> Capture a real `/etc/keyd` apply before acting on B5(b). B5(a)/B6 (layout, button styling)
> are real.

## 1. Executive summary

The edit-mode flow is functionally complete and visually consistent, but it reads like a
tool built by someone who already knows keyd. The two highest-leverage problems are **a
save/commit model that no persona could decode** (`set` vs `save draft` vs `done` vs `apply
anyway`, recurring on 8 of 13 screens) and **raw keyd syntax surfaced at exactly the wrong
moments** (`lettermod(nav, f, 150, 200)` as the tap/hold headline, and the same expression
as the only representation of a change on the final live-apply confirmation). On top of
that, the destructive-confirmation bars (discard, delete-layer) put the green/"go" accent on
the *safe* button and leave the destructive one a quiet grey — an inverted convention that
risks the wrong click on irreversible, root-level actions. A pervasive dim-grey helper-text
treatment (measured as low as ~1.4:1 on tap-hold) means the load-bearing explanatory
sentences are consistently the least legible text on screen. None of this blocks an expert,
but a first-timer lacks the orientation, vocabulary, and commit-model confidence to trust
the tool.

## 2. Blockers & majors (deduped, by impact)

### B1 — Raw keyd syntax is the headline on tap/hold AND the only diff representation on live-apply
**Severity: blocker** · Screens: `tap-hold`, `apply-summary` · Personas: novice, qmk-power, a11y
The tap/hold editor's most prominent text is `= lettermod(nav, f, 150, 200)` — a
keyd-internal function name with two unlabeled magic numbers. The apply confirmation (the
last gate before a **root-running** live write) shows the change only as `~ [main] k =
lettermod(control, k, 150, 200)`. Even a QMK power user can't confirm intent; a novice fears
editing code they'll break.
**Fix:** Render a plain-language summary everywhere the expression appears: "Tap F → types f;
hold F → switches to the nav layer (150ms tap / 200ms hold)." Keep the raw expression behind
an "advanced/read-only" disclosure. On the apply diff, gloss each line and spell `[main]` as
"on the main layer."

### B2 — "chord" is never glossed as "pressed together," the exact task phrase
**Severity: blocker** · Screen: `chord` · Personas: novice, qmk-power
The whole screen hinges on "chord" (appears 3×: mode toggle, helper line "chords (main) —
click 2+ keys on the board"), but the plain meaning that matches the user's goal — "press
these keys together to trigger one action" — is stated nowhere. Novices think music; QMK
users expect "combo."
**Fix:** Keep keyd's term, add a gloss subtitle: "two or more keys pressed together (a QMK
combo)." The mode toggle can read "one key" / "multiple keys at once."

### B3 — The save/commit model is undecodable (set / save draft / done / apply anyway)
**Severity: major** · Screens: `edit`, `key-selected`, `tap-hold`, `macro`, `chord`, `global`, `apply-summary` · Personas: all four
Every editor surfaces 2–3 commit-ish verbs with no stated relationship. Worst on
`key-selected`: `set`, `save draft`, and `done` all read as "finish," so users type, hit
`set`, and don't know if they must also `save draft`. On `apply-summary` a third path (`apply
anyway`) joins with no copy explaining which one goes live.
**Fix:** Define one vocabulary and apply it everywhere: `set …`/`apply to A` = stage the
binding in-memory; `save draft` = persist the config file without going live; `apply` = write
to live keyd + reload. Add a one-line model statement near the buttons and confirm
applied-state on the key after `set`. This is the single highest-leverage fix in the report.

### B4 — Destructive-confirm bars put the green accent on the *safe* action
**Severity: major** · Screens: `discard`, `delete-layer` · Personas: all four
On `discard`, the only color-emphasized button is green `keep editing` (same green as
`done`); destructive `discard` is quiet grey. On `delete-layer`, `keep` is green-outlined
while `delete` is a plain grey pill. Green = "go" inverts the convention, so a user intending
to leave/delete is pulled toward the wrong button on irreversible actions. (Note: personas
*overstated visual dominance* — on delete-layer the `delete` pill is actually the
lighter/more solid one. The defensible issue is semantic color inversion, not size.)
**Fix:** Give the destructive button a red/amber treatment matching the warning bar; demote
the safe button to neutral/ghost. Don't rely on hue alone — pair with the warning glyph.

### B5 — Apply confirmation: buttons sit ABOVE the diff, and warn about a `command()` the diff never shows
**Severity: major** · Screen: `apply-summary` · Personas: novice, qmk-power, skimmer, a11y
Two distinct failures on the final live-apply gate: (a) `apply anyway`/`cancel` render in the
warning bar *above* the "2 changes" diff box — a skimmer can act before seeing the evidence;
(b) the amber banner and "Contains command() — review before applying" warn of a root-running
danger that **neither visible change is** (both are plain remaps). The user is told to review
something the screen never localizes. *(See seed caveat above: (b) reflects the seeded
screenshot, not confirmed real behavior; (a) is real.)*
**Fix:** (a) Move buttons below the diff so `apply` reads as "apply *these 2 changes*." (b) If
the diff introduces no `command()`, state "This config already contains command() (not
changed by you) — runs as root" and name the file/line; otherwise soften the banner to "Apply
2 changes."

### B6 — Apply's primary action is the least button-like control
**Severity: major (affordance)** · Screen: `apply-summary` · Personas: all four
`apply anyway` is bare text with no fill/border, while `cancel` carries a green outline — the
abort action out-styles the go-live action on the screen whose entire purpose is to apply.
**Fix:** Give `apply` a deliberate, weighty cautionary button treatment; keep `cancel` safe
but quieter. (Folds into B3/B4's color discipline.)

### B7 — `[global]` form: every field shows the word "default" at full brightness, indistinguishable from a real value
**Severity: major** · Screen: `global` · Personas: all four
Each value box literally displays "default" at ~12:1 contrast — same weight as the row labels
— so it reads as a committed value, not an editable placeholder. No caret, unit, or example
signals "type a number here." The only instruction ("blank a field to reset to default") is
buried in the header.
**Fix:** Show the effective default as faint ghost text (e.g. placeholder `200`), give inputs
a real text-field affordance (caret, focus ring, in-box unit suffix). Also: surface the
numeric default on *every* row (only Chord/Macro-timeout/Macro-repeat show one today;
Overload tap timeout and One-shot show none).

### B8 — `[global]`: units silently switch ms → µs for one row
**Severity: major** · Screen: `global` · Personas: all four
Every timeout says "ms" except "Macro sequence timeout" which says "µs", and the unit lives
only in dim right-side hint text, never in the input. Scanning a column of identical
"default" boxes, a user can enter a value off by 1000×.
**Fix:** Put a fixed, non-dim unit chip (`ms`/`µs`) inside each input and visually flag the µs
row.

### B9 — Dim helper text fails contrast across the flow (measured)
**Severity: major** · Screens: `key-selected` (~2.43:1), `macro` (~2.2:1), `tap-hold` (~1.43:1), `picker` (~3.1:1) · Persona: a11y
The load-bearing sentences that explain current state are consistently the dimmest text on
screen — measured well below the WCAG 3:1 large-text floor in multiple places. Examples: `(no
live key stream to capture from)`, `· editing existing — any timings preserved`, the picker's
`+235 more` truncation cue (the *only* place truncation is communicated). Placeholder text by
contrast measures ~13:1, so the tooling can clearly render legible grey — it's just not used
for sentences.
**Fix:** Lift instructional/status text to at least placeholder brightness (~`(207,214,223)`).
Reserve dim grey for decorative chrome only. (Note: not *all* greys fail — `live view off` and
`no remap` measure fine; target the specific sentences above.)

### B10 — `key-selected` / `picker`: the "pick…" path and the result list under-signal that they're pickable
**Severity: major** · Screens: `key-selected`, `picker` · Personas: all four
`pick…`'s trailing ellipsis gives no hint whether it opens a list or captures a physical
press, and the note explaining why capture is disabled is sub-3:1 grey. In the picker, result
rows (`esc`, `escape`, `1`, `!`…) are flat left-aligned text while the "common" chips above
are clearly pills — so users read the list as informational, not selectable.
**Fix:** Relabel `pick…` → `pick from list…`; lift the capture-disabled note out of dim grey;
give picker rows visible row styling/hover affordance so they read as selectable like the
chips.

### B11 — `key-selected`: selected mode/type indicated by green outline only
**Severity: major** · Screen: `key-selected` · Persona: a11y
Active `single key`/`simple` are shown by green outline + green text only — no fill,
checkmark, or label — while the `main` layer pill *does* use a yellow fill. The non-color
affordance already exists elsewhere; it's just inconsistently applied.
**Fix:** Give the active mode/type a filled background (match the `main` pill) or a checkmark.
(Note: the `edit`-screen "single key" pill *does* have a fill — so this is specifically the
simple/tap-hold/macro type row.)

## 3. Cross-screen themes (highest leverage)

1. **Commit-model vocabulary chaos** (B3) — `set`/`save draft`/`done`/`apply
   anyway`/`create`/`rename`/`add` appear with no stated hierarchy across 8 screens. One
   defined vocabulary + a one-line model statement fixes the most-reported issue in the
   entire pass.
2. **keyd jargon with no plain-language bridge** — `lettermod`, `chord`, `noop`, `overload
   tap timeout`, `modifier guard`, `daemon`, `dangle`, bracket notation `[main]`/`[nav]`.
   House style (correctly) keeps keyd-native terms in *labels*; the fix is consistently
   adding a plain gloss in *hints/tooltips*, not renaming.
3. **Dim helper text** (B9) — the same low-contrast grey is applied to load-bearing sentences
   on every screen. A single token/style change for "instructional text" lifts all of them.
4. **Color carrying meaning that should be redundant** — selected-state by green outline only
   (`key-selected`, `tap-hold`, `chord` board keys); destructive vs safe by hue (`discard`,
   `delete-layer`, `new-layer`). The yellow-filled `main` pill proves the team already has a
   non-color selected pattern — apply it uniformly and add a non-hue cue to destructive
   buttons.
5. **Green accent overloaded as "the action"** — `done`, `create`, `rename`, `set`, and
   selected-mode pills all share green, so "the green thing" is never uniquely the primary
   action on a given screen. Reserve filled green for the one primary action per screen.
6. **Adjacency hazards: destructive next to constructive** — `unbind` beside `set`
   (key-selected), `hold only` beside the tap value (tap-hold), `×`/`× clear` beside reorder
   arrows (macro, chord). Add spacing/visual separation around destructive controls.
7. **Placeholder-vs-value ambiguity in text fields** — `fn` (new-layer), `navigation`
   (rename-layer), and `default` (global) all render as solid committed-looking text with no
   caret/selection cue. Use dim ghost text for suggestions, or select-on-open for editable
   prefills.

## 4. Flow friction (whole-flow / onboarding)

- **Cold start gives no orientation** (`base`): the largest text is a filename
  (`hhkb.conf`); the words "remap," "change a key," or "keyd config editor" appear nowhere. A
  first-timer can't confirm they're in the right tool, and the small `examples/` prefix
  doesn't signal "this is a demo, not your live keyboard." Add a one-line purpose string and
  make example-status prominent.
- **Entering edit mode is barely perceptible** (`edit`): the only changes are a thin yellow
  outline, an `EDIT` chip, and a green `done` — the board looks unchanged, and the brightest
  control (`done`) invites an immediate misclick out of the mode. The "click a key"
  instruction appears *twice* (one near-invisible). Show the instruction once at full
  contrast; make the editable surface visibly active; de-emphasize `done` until a change
  exists.
- **"no connected keyboard matches this config"** reads as a failure that edits won't work,
  when editing an example is valid. Soften to "Editing an example layout — changes won't
  affect a live keyboard."
- **The keep-my-work path is missing at the moment of decision** (`discard`): the bar offers
  only `discard`/`keep editing` though `save draft` exists elsewhere on screen. Add an inline
  "Save draft & leave" so the rescue action is co-located with the warning that prompts it.
- **`live view off · add yourself to the keyd group`** (top bar, every screen) is a bare
  imperative with no button and no explanation of keyd/group/live-view — ambiguous as status
  vs. required action.

## 5. Per-screen notes

- **base** — Filename-as-title with no purpose string; layer badges
  (`↓num`/`↓nav`/`↓Ctrl`/`↓sym`) have no legend and red `↓Ctrl` reads as an error. Unlabeled
  VID:PID chips (`04fe:0021`) read as important-but-meaningless.
- **edit** — Duplicate "click a key" instruction (one near-invisible); weak mode-entry
  signal; `done` dominates an empty state.
- **key-selected** — Three commit verbs (`set`/`save draft`/`done`); `pick…` ambiguous;
  "simple" used as both status word and active tab name (decouple them); only example is `esc`
  (add a letter case like `x`).
- **tap-hold** — Raw `lettermod(...)` headline (B1); `hold:` row never says "switch to layer";
  `feel:` forces fast-typing vs avoid-misfires with no consequence text; `· editing existing`
  note at ~1.43:1.
- **macro** — Selected `macro` mode pill and `set macro` button share identical green; crowded
  `add:` row with no input→button grouping (`+text`/`+pause`/`+chord`); `key…` chip has no
  `+key` sibling; `× ` delete flush against reorder arrows.
- **picker** — Flat-text result rows don't read as selectable; `noop` unexplained;
  `esc`/`escape` shown as undifferentiated aliases; footer `keyd list-keys (315) · +235 more`
  is jargon + the only (dim) truncation cue; shifted symbols (`!` under `1`) unannotated.
- **chord** — "chord" never glossed (B2); no "New chord:" heading to separate the in-progress
  `j ×`/`k ×` draft from saved chords; action placeholder `(e.g. esc)` contradicts the visible
  `toggle(game)`; verb overload (`add`/`save draft`/`done` + `remove`/`× clear`/per-chip `×`).
- **global** — "default" reads as a value not placeholder (B7); ms→µs unit switch (B8); only 3
  rows show numeric defaults; `Disable modifier guard` warning truncates at "…unless you know"
  with no consequence; mixed `default`/`off` resting idioms.
- **new-layer** — `fn` ambiguous as placeholder vs typed value; no concept/naming-rule hint
  (keyd layers are name-addressed, e.g. `layer(fn)`); `create`/`done`/`single key` all green.
- **rename-layer** — Field prefilled `navigation` while chip still says `nav` (mismatch
  invites a double-take); `[nav]` target token is the dimmest, most code-like element on the
  row when it should stand out.
- **delete-layer** — `keep` (green) vs `delete` (grey) inverts convention (B4); `keep` is an
  unusual dismiss verb; consequence ("1 binding will dangle") buried in prose, not on the
  button; `dangle`/`binding`/`[main]` jargon doesn't say whether the config still *loads*.
- **discard** — Green on the safe `keep editing`, grey on destructive `discard` (B4); no inline
  save path (B-flow); labels don't name the exit outcome ("Discard & exit").
- **apply-summary** — Buttons above the diff (B5); `command()` warning references nothing in
  the visible diff (B5 — but see seed caveat); `apply anyway` under-styled vs `cancel` (B6);
  raw `lettermod` diff lines distinguished only by `+`/`~` glyphs with no color/word labels.

## 6. Honest caveats

**Dropped as simulated** (premise contradicted by the actual pixels): the `base`
color-blindness claim (badges already carry text labels, so meaning isn't color-*only*); the
`edit` and `key-selected` "single key looks color-only / set looks disabled" claims
(single-key has a fill; `set` renders as a normal enabled pill — `unbind` is the disabled
one); the `tap-hold` and `rename-layer` "done/chord is the biggest brightest green button"
claims (done is a small outlined pill; chord is grey); the picker title "low-contrast" claim
(measured ~9:1); the delete-layer warning-text "fails contrast" claim (measured ~7.2:1,
passes); the discard "discard is in the rightmost click zone" claim (it sits left of the green
button). **No blocker was dropped on simulated grounds**, so no doubtful must-fix is being
suppressed.

**Uncertain / needs live verification** — anything depending on interaction or precise
measurement that a static screenshot can't settle: all **focus-ring / caret / tab-order**
findings (focus states only appear after keyboard interaction); **hover affordances** on
keycaps and picker rows; **hit-target sizes** vs the 44px guideline (this is a desktop pointer
app, so that's a soft guideline anyway); and several **exact contrast ratios** flagged
"plausibly below 4.5:1" without a sample (e.g. some rename/chord helper lines). The `global`
helper text measured ~4.6–5.0:1 — a *borderline pass*, so its "clear AA fail" framing was
downgraded.

**What this pass structurally cannot do:** the personas *simulate* confusion from a single
static frame per screen. This pass cannot measure **time-on-task**, capture the **eye-path /
first-click** on a live render, observe **error-recovery** when a user picks the wrong commit
button, or confirm whether ambiguities resolve through **interaction** (tooltips on hover,
inline validation on submit, applied-state feedback after `set`). The commit-model finding
(B3) in particular is the kind of thing that *looks* fatal statically but a real user might
learn in two clicks — or might not. A 5-person moderated first-run test (one each:
never-remapped novice, QMK/VIA migrant, impatient skimmer, screen-magnifier user,
keyboard-only user), each given the same per-screen tasks used here, would convert these
"likely" verdicts into ranked, evidence-backed priorities and catch the interaction-dependent
issues this static pass had to mark "uncertain."
