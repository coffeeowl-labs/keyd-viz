# Custom key labels — design (v1.3 headline)

**Goal.** Let a user attach a free-form display name to a key cap (Oryx-style "see my
own names": *Tab L*, *Nav / Esc*) that lives **in the `.conf`** (portable), is **keyd-safe**,
and survives editing.

## 1. Why not trailing comments

The roadmap assumed `key = value  # Tab L`. **keyd v2.6.0 does not support trailing
comments** in general — it only *incidentally* tolerates trailing text after a layer-arg
action (`layer(nav)`, `oneshot(nav)`), and **warns "invalid key or action" (skipping the
binding) for everything else** (`a = b # x`, `macro(x) # x`, `noop # x`). Verified directly
with `keyd check`. So trailing-comment labels would silently break most bindings. Rejected.

## 2. Storage: key-encoded comment line

A label is a **full-line comment** (keyd ignores any line whose first non-space char is `#` —
universally valid), encoded with a namespaced, key-naming marker:

```
[main]
# keyd-viz: tab = Tab L
tab = layer(nav)
# keyd-viz: capslock = Nav / Esc
capslock = overload(nav, esc)
```

**Grammar.** A comment entry is a label iff, after stripping the leading `#` and ASCII
whitespace, it begins with `keyd-viz:`. The remainder (`<key> = <label>`) is split with the
**existing `parse_kvp`** (`edit.rs`) — the same splitter keyd-viz uses for binding lines, so
the key parses identically, including keyd's leading-`=` special case (a key literally named
`=`) and punctuation keys (`-`, `;`, `'`, `[`…). LHS = key; RHS = label text (free-form: may
contain spaces, `#`, further `=`). Empty RHS ⇒ no label.

**Association is by `(section, key)`** — *not* by line position. A label comment anywhere in
its section labels that section's binding for that key. (The only stable handle in a keyd
config is the key name; there are no IDs. Every storage scheme shares the "rename the key →
orphaned label" failure, so we accept it; it degrades gracefully — see §6.)

Look-alikes that are **not** labels and stay verbatim prose comments: `# keyd: …`,
`# keydviz …` (no colon), `# keyd-viz foo` (no `=`).

## 3. Model & lossless round-trip

**Two layers, one source of truth.** The on-disk source of truth for a label is its `Comment`
line, which stays an ordinary `Comment` entry in `EditConfig` — stored verbatim, replayed
byte-for-byte by `serialize()`. So an unedited config (however the user arranged the comments)
round-trips identically; the `round_trips()` contract is untouched. `EditConfig` is **read
only** for labels at render time; no `EntryKind::Binding` field is added.

**Threading to the renderer (the corrected architecture).** `board.rs` renders from the
*semantic* `Config` only — and `derive()` (`parser.rs`) discards comments. So a label must be
carried on the semantic model to reach a cap (a board-side read view on `EditConfig` cannot —
`board.rs` has no handle to it). Therefore:

- Add `labels: Vec<(String, String)>` to **`Layer`** (`model.rs`) and a `labels:
  Vec<(String, String)>` to **`Config`** for the base/main section (base bindings live in
  `Config.remaps`/`holds`, not in a `Layer`).
- In `derive()`, after collecting each section's bindings, scan that section's `Comment`
  entries for the `keyd-viz:` marker (via `parse_kvp`) and populate these maps. Qualified /
  same-base sections (`[nav]`, `[nav:C]`) that `derive()` merges into one layer have their
  label comments merged too.

The label is thus *copied* into the freshly-derived `Config` on each `derive()` call. That is
**not** a sync hazard: `derive()` rebuilds `Config` from scratch every call, so the comment
line remains the single writable source and the `Config` copy is a throwaway snapshot. (This
corrects the earlier "never duplicated / pure read-view" framing, which was incompatible with
how rendering actually gets its data.)

## 4. Editing labels (the mutation API)

New ops on `EditConfig` / `Section`, exposed through `EditSession`:

- **`set_label(section, key, text)`** — find the section's existing `keyd-viz: key = …`
  comment; if present, rewrite its `raw` (mark dirty); else **insert** a new comment entry
  *immediately before* `key`'s binding entry. Needs a new "find binding-entry index by key"
  helper that targets the **same last-wins entry `set_binding` uses** (so the label sits with
  the effective binding), and copies that neighbor's `Eol` (mirroring `push_binding`'s
  trailing-newline care). If the key has no binding entry, append at the section's end
  (orphan-tolerant). Empty `text` ⇒ `clear_label`.
- **`clear_label(section, key)`** — remove the section's `keyd-viz: key = …` comment entry.

**Edit survival, against the *actual* op set** (the critic confirmed there is **no** key-rename
op and **no** unbind-vs-delete seam — `clear_binding` == `remove_binding`, which retains out
the line):
- Retarget a binding (change its value / kind): label untouched (it names the key, not the value).
- Clear/unbind a binding: **label is left in place.** A label describes the *cap*, not the
  binding; the physical key still exists on the board, so a now-binding-less key keeps showing
  its label (main = label, ghost = base legend). Labels are removed **only** by explicit
  `clear_label` (or the user emptying the field). This deliberately avoids depending on a
  delete-vs-unbind distinction the code doesn't have.
- "Renaming" a key in this GUI is really bind-new + clear-old (two ops on two keys); a label
  stays attached to the original key name. **No automatic rename-carry in v1** (there's no
  single rename op to hook); the user re-labels the new key. Acceptable — failure is cosmetic.
- Self-healing: editing a label (re)places its comment adjacent to the binding. Unedited
  labels are left wherever they sit (preserving round-trip); we read them by key regardless.

**Canonical emitted form:** `# keyd-viz: {key} = {label}\n` (single spaces). Re-parses to the
same comment entry ⇒ our own output round-trips. `set_binding`'s line regeneration is
unaffected (it only ever rewrote the binding line; the label is a separate line).

## 5. Board rendering

`build_base` reads `Config.labels`; `build_layer` reads `Layer.labels`; `build_composite`
reads the composite layer's own `labels`. A small `label_for(&[(String,String)], key)` helper
does the lookup.

**Uniform "demote" rule (not a per-branch swap).** The caps in `build_base` don't share one
label/ghost shape — a tap/hold-with-tap cap sets `cap.label = prettify(tap)` and **no ghost**
(it shows the hold target via `badge_left = ↓<target>`), while simple remaps use label+ghost.
So instead of editing each branch, apply ONE step per cap, *after* its normal construction,
just before `keys.push(cap)`:

```
if let Some(lbl) = label_for(labels, name) {
    if cap.ghost.is_empty() { cap.ghost = cap.label.clone(); } // demote the action into the ghost…
    cap.label = lbl.into();                                     // …and put the custom name on top
    cap.emphasized = true;
}
```

This is uniform across `build_base`/`build_layer`/`build_composite` and preserves each
branch's secondary info: badges (↓hold-target, ⊕combo) are untouched; the tap action / remap
target / base legend that was the main text drops to the ghost (only overwriting an
*already-empty* ghost, so a remap's existing base-legend ghost is replaced by the action,
matching the approved mockup, while a tap/hold's empty ghost gains the tap action). Net for
the design's own `overload(nav, esc)` → "Nav / Esc": main = "Nav / Esc", ghost = the tap
action, badge = ↓nav — all three still legible.

- A label on a key keyd doesn't remap still shows (main = label, ghost = base legend).
- The **game layer** sets ghost empty deliberately; the demote would fill it with the
  passthrough legend — acceptable (a labelled game key shows label + its key), leave the rule
  uniform.
- **Composite boards (`[a+b]`)** render *effective* bindings that may be inherited from a
  constituent layer; v1 shows only labels defined in the `[a+b]` section itself (its own
  overrides). Inheriting a constituent's label onto the composite cap is a later refinement.
- Long labels use the existing cap text fit; labels are expected short ("Tab L"); the new
  ghost-as-action text can be longer than a base legend — sanity-check fit in manual QA.

## 6. Failure modes (all cosmetic, never break a binding)

- Hand-rename the labelled key → label orphaned → cap shows its default legend. No binding harm.
- Hand-delete the comment → label gone → default legend.
- Orphan label (comment naming a key with no binding) → still rendered on that cap (main =
  label, ghost = base legend); harmless.
- Copy `.conf` elsewhere → label travels (it's in the file). ✔

## 7. Editor UX

In the selection panel (when a key is selected, any kind), add one row:

```
label:  [ Tab L____________ ]  [set]   (clear shown when a label exists)
```

- Pre-filled with the current label. `set` writes/updates the comment; the field empty + set,
  or a `clear` button, removes it. Independent of the remap kind (simple/tap-hold/macro) —
  a label is about display, not behavior. Lives above or beside the binding rows.
- A new property `selected_label` + callbacks `set_label(string)` / `clear_label()`.

## 8. Scope (v1.3)

- **In:** labels for single physical keys, per section (base + each layer/composite). Read,
  set, edit, clear, rename-carry, lossless round-trip, board display.
- **Out (later):** labels on chords (a separate construct, not a board cap); rich
  formatting; per-label colors. Keep it to text.

## 9. Test plan

Core (`edit.rs` / `parser.rs` / new label grammar):
- Parse via `derive()`: `# keyd-viz: tab = Tab L` in `[main]` ⇒ `Config.labels` has
  `("tab","Tab L")`; in `[nav]` ⇒ that `Layer.labels`. Look-alikes (`# keyd:`, `# keydviz`,
  no `=`) ignored. A `=`-named key parses correctly (parse_kvp).
- Round-trip: a config with label comments (in odd positions) serializes **identically** when
  unedited (the `round_trips()` gate).
- `set_label` on a key with no prior label inserts the canonical comment before the (last-wins)
  binding; `keyd check` still passes; output round-trips.
- `set_label` on a key with an existing label rewrites in place (no duplicate).
- `clear_label` removes exactly that comment, nothing else.
- Clearing the *binding* leaves the label; retarget keeps it.
- Merged `[nav]`+`[nav:C]` labels both land on the nav layer.
- Label text with spaces / `#` / a second `=` preserved verbatim.

Board (`board.rs`):
- Labelled remapped key ⇒ `cap.label == custom`, `cap.ghost == prettify(action)`.
- Labelled unmapped key ⇒ `cap.label == custom`, `cap.ghost == base_legend`.

GUI: manual click-through (set/edit/clear a label; see it on the cap; save; reopen).

## 10. Build order

1. Core model: add `labels: Vec<(String,String)>` to `Config` + `Layer`; label grammar
   (reusing `parse_kvp` — **bump it `fn` → `pub(crate) fn`**, it's currently private);
   populate in `derive()`; tests. **Fix the exhaustive `Layer { … }` literal in
   `board.rs` tests** (add `labels: vec![]`) so it still compiles.
2. Core edit: `set_label`/`clear_label` on `EditConfig`/`Section` (+ find-index helper
   mirroring `set_binding`'s last-wins target section); tests.
3. Board: render labels via the uniform demote rule (§5) + tests.
4. EditSession glue (`set_label`/`clear_label`/`label_for`) + Slint label row + properties/callbacks.
5. Manual verification; build release.

**Known compile touch-points (from review):** `parse_kvp` visibility (edit.rs); the
`Layer { .. }` literal at ~board.rs:756. `Config`/`Layer` derive `Clone/Default/PartialEq/Eq`
and have no serde, so a new `Vec` field is otherwise free; the `round_trips()` gate is on
`EditConfig`/text, unaffected.
