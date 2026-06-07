# Phase 6 — Edit Mode (design doc)

> **Status:** DRAFT v2 (2026-06-05; competitive research + planning added 2026-06-06). Living
> document — refined through adversarial critic reviews before code is written. v2 incorporates
> **critic review #1**, which corrected the security model (the drafted live-preview channel was
> unsafe) and moved lossless round-trip from a deferred phase into the MVP; the 06-06 pass added
> the competitive research (§10), the north-star UX vision (§9), and per-phase acceptance
> criteria (§6). Decisions marked **PROPOSED** are not final; **OPEN** marks unresolved
> questions; **DECIDED** marks settled ones. **Review #2 (2026-06-06) resolved all E0-blocking
> OPEN items** — round-trip representation, `include` policy, multi-keyboard/backup, AppImage
> apply, and the test-oracle sizing are now DECIDED; E0 is unblocked.
>
> **What this feature is:** a GUI front-end for authoring and editing `/etc/keyd/*.conf` —
> point at a key, pick what it does, see it on the board, save it back safely. It is *not*
> "VIA for keyd": VIA writes portable firmware to the device with no root; this edits a
> root-owned text file and reloads a daemon, bound to this one Linux machine. Useful, but a
> different thing — we describe it honestly to avoid setting up the wrong expectations.

---

## 1. Vision & scope

Today keyd-viz *renders* a keyboard from a keyd config. Edit Mode adds the reverse: edit the
config visually and write it back. It must:

- Edit a user's **real, existing** config without destroying it (comments, ordering, and any
  keyd feature we don't model must survive a save — see §5.1). This is the MVP, not a later
  luxury: every config in the wild is hand-authored, so an editor that can only touch files it
  created has almost no audience.
- Work for a keyboard that has **no config yet** (detect it, create a starter config).
- Cover the **common** keyd actions first (remap, layer, overload, oneshot, toggle, lettermod,
  modifiers, chords, disable), then grow toward the full vocabulary.
- Be **safe**: never silently brick the keyboard; the user always has a way back.
- Preserve keyd-viz's identity: the **viewer stays the default**; editing is an explicit mode;
  the GUI itself stays unprivileged.

**Non-goals (at least initially):**
- Not a replacement for a text editor — a power user with a complex hand-tuned config can keep
  `$EDITOR`; we aim to *not break* their file if they do open it in the GUI.
- Not firmware/QMK editing (keyd is software-only; that's a different tool).
- Not live remapping of the keyboard you're typing on *while editing* (see §5.6 — preview is
  visual, not applied-to-your-input-device).

---

## 2. Why this is hard — the two cruxes

The *breadth* of keyd features is steady, low-risk work. The genuine difficulty is two things:

1. **Round-trip fidelity.** keyd configs are hand-authored. If the editor loads a file and
   writes it back, it must not silently drop comments, ordering, or any action it doesn't
   understand. v2 makes this an MVP requirement and solves it by **carrying every unmodeled
   construct verbatim** plus a **round-trip safety gate** (§5.1) — not by restricting the
   editor to files it created.
2. **Privileged write + reload + not bricking input.** Configs live in root-owned `/etc/keyd/`;
   applying needs a reload over a socket that is **root-equivalent**; a bad config can make the
   keyboard unusable. This collides with keyd-viz's "GUI is never privileged" principle and
   demands a real safety design (§5.2–§5.4).

---

## 3. Verified facts (the spike + review #1, all checked against keyd 2.6.0 source @ `f564288`)

### 3.1 Our codebase

| Area | Finding | Source |
| --- | --- | --- |
| Parser | **Semantic-only / lossy for round-trip.** `parse_text` (`crates/core/src/parser.rs`) builds a semantic model and discards comments, blank lines, whitespace, and ordering; no spans, no source text. (Two grammar bugs found in review #1 — `[layer:mods]`/`[a+b]` headers and inline `#` — are now **fixed**, but the model is still not round-trippable.) | spike, review #1 |
| Action coverage | Parser structures `overload*`, `lettermod`, `layer`, `toggle`; **everything else becomes a raw `remaps` string** (`macro`, `command`, `oneshot`, `swap`, …) and already renders as that string. | `parser.rs` |
| Serializer | **None exists.** No `Config → text`. | spike |
| Device enum | `devices::connected_devices()` reads `/proc/bus/input/devices` (world-readable, zero privilege) and lists **all** keyboards using keyd's own rule — config-independent. | `crates/app/src/devices.rs` |
| Config read | `/etc/keyd/*.conf` is world-readable; reading needs no privilege. | `crates/app/src/main.rs` |
| Existing writes | App writes `~/.config/keyd-viz/layouts.tsv` (`prefs.rs`) — precedent for user-writable storage. | spike |
| Helper | Strictly **one-directional, events-out**; hardened systemd sandbox (`SystemCallFilter=~execve`, `ProtectSystem=strict`, `PrivateNetwork`, `DevicePolicy=closed`). Runs as the `keyd-viz` user, which **is in the `keyd` group** — see the security note below. | `crates/helper/`, `packaging/systemd/` |

### 3.2 keyd itself (probe at runtime — facts are version-dependent)

| Capability | Detail | Why it matters |
| --- | --- | --- |
| **Reload** | `keyd reload` over `/var/run/keyd.socket` (`root:keyd`, `0660`). Not SIGUSR1; no file watching. **Reloads *all* configs in `/etc/keyd`, not just the changed file.** | We trigger reload via the privileged path; "we only touched one file" is not how reload behaves. |
| **Validate** | `keyd check [files…]` — parse/validate only, no apply, no daemon. **Syntax-only: a file full of `command(rm -rf …)` passes clean** (verified `check.c` calls only `config_parse`). | Catches typos before apply. **It is NOT a security gate** — see §5.3. |
| **Panic** | Hard-coded **Backspace + Escape + Enter** terminates keyd, restoring the raw keyboard. Not config-overridable. | **This is the primary failsafe** (§5.4). |
| **Key names** | `keyd list-keys` enumerates valid key names; `keyd monitor` discovers names + device ids. | Populate the key picker authoritatively. |
| **Files** | `/etc/keyd/*.conf`, `root:root 0644`, system-only. **No per-user config location.** | Writing the real config requires privilege. |
| **Socket security** | The daemon does **zero peer authentication** — `handle_client` dispatches `bind`/`macro`/`reload` to anyone who can `connect()` (verified `ipc.c`, `daemon.c`). `keyd bind 'x = command(...)'` reaches `execute_command` → `execl("/bin/sh", …)` **as root**, with no privilege drop. | **Socket access = unauthenticated root.** So `keyd`-group membership is itself a root-equivalent capability; the helper's sandbox is the only thing caging it. Dictates the entire apply design (§5.2). |
| **`keyd bind` has no safe subset** | `bind` runs the full config parser and can install **any** action on **any** layer — not just the key being edited. `macro(...)` injects arbitrary keystrokes into the focused window as root (e.g. open a terminal, type `curl…\|sh`). So "block `command()`" does **not** make a bind channel safe. | Kills the idea of a live `keyd bind` preview/command channel (§5.2, §5.6). |

### 3.3 The keyd action vocabulary (breadth estimate)

- **Common (MVP):** plain remap (`a = b`), `noop` (disable), modifiers (`layer(control)`),
  `layer`, `oneshot`, `toggle`, `overload`, `lettermod`, chords (`a+b = …`), `macro`/`C-a`.
- **Uncommon (later):** `overloadt`, `overloadt2`, `overloadi`, `oneshotm`, `oneshotk`,
  `layerm`, `togglem`, `swap`, `swapm`, `clear`, `clearm`, `macro2`, `timeout`, `setlayout`,
  `repeat`.
- **Sections:** `[ids]`, `[global]`, `[main]`, `[<layer>]`, `[<layer>:<mods>]`, `[<a>+<b>]`
  (composite), `[<name>:layout]`, `[aliases]`, `include`.
- **`[global]` options:** the `*_timeout` family, `layer_indicator`, `default_layout`, etc.
- **Sensitive:** `command(<shell>)` runs as root; `macro(...)` injects keystrokes; `include`
  pulls in other files at reload. All three need explicit handling in the safety scan (§5.3).

---

## 4. What makes the MVP tractable

The feature is large, but two decisions keep the first version small **without** sacrificing the
"edit my real config" requirement:

- **Verbatim preservation, not app-ownership.** The editor carries every construct it doesn't
  model — comments, ordering, unknown actions, whole unmodeled sections — as opaque text, and a
  **round-trip gate** refuses to save any file it can't reproduce exactly except for the user's
  intended change (§5.1). This replaces v1's "only edit files we created" scoping, which served
  nobody. The *first shippable* editor can be deliberately narrow in *what it lets you change*
  (e.g. single-key remaps) while still safely round-tripping *any* file.
- **Draft-then-install for persistence (at first).** Before the one-click privileged apply is
  built, "save" writes the edited file to `~/.config/keyd-viz/drafts/` and shows exactly how to
  install it (the diff + `sudo cp … /etc/keyd/ && sudo keyd reload`). This ships value without
  the privileged-writer being on the critical path for v1. The polkit one-click apply (§5.2) is
  a self-contained later phase.

MVP, restated: **open any real config → change a binding visually → see it on the board → save
it back without losing anything.** Create-from-scratch for an unconfigured keyboard is a small
addition on top.

---

## 5. Architecture

### 5.1 Data model, one parser, round-trip — **DECIDED (review #1)**

**One keyd-faithful parser drives both the viewer and the editor.** A second "edit parser"
alongside the existing one would drift; instead we upgrade the single parser to be faithful to
keyd's grammar and derive the render model from it. (Review #1 already fixed two places where the
parser diverged from keyd.)

The editor works on an **edit model** in which each binding is either:
- a **typed known action** (remap / layer / overload / oneshot / toggle / lettermod / chord /
  disable / modifier / macro …), or
- an **opaque `Raw` binding** carrying the original right-hand-side text verbatim. **Raw bindings
  render as their RHS text** (what the viewer already does today) — never as a generic
  "advanced" placeholder, which would make the board less informative than the current viewer.

**Representation — DECIDED (review #2): per-line verbatim entries.** keyd's config grammar is
**strictly line-oriented** — verified in `config.c`/`ini.c`: `read_line` reads to `\n`, INI
splits only on `\n`, `include` is expanded line-by-line, and there are **no** multi-line
constructs (no continuations, no values spanning lines, no here-docs). A line is therefore the
natural, safe atom. The model is an ordered list of sections, each an ordered list of *entries*;
every entry stores its **original source line verbatim** plus an optional typed overlay for the
lines we model. Comments and blank lines are entries too. Sketch:

```rust
struct EditConfig { preamble: Vec<Entry>, sections: Vec<Section>, trailing_newline: bool }
struct Section { header_raw: String, kind: SectionKind, entries: Vec<Entry> }
struct Entry { raw: String, eol: Eol, kind: EntryKind }  // raw = the line, verbatim, sans EOL
enum EntryKind { Blank, Comment, Binding { key, val: Option<String>, typed: Typed, dirty: bool } }
enum Typed { Remap(..), Hold(..), Toggle(..), /* modeled actions */ Raw /* render val verbatim */ }
```

This makes `serialize(parse(f)) == f` **true by construction**: serialize walks entries and emits
`raw + eol` for every untouched entry, regenerating *only* the one line the user edited. No
regeneration code runs on a pure round-trip, so fidelity never depends on serializer correctness
for any action. `[global]`/`[aliases]`/`include`/unknown actions are **not special-cased** — they
are ordinary entries whose `Typed` is `Raw`.

**The round-trip gate, reframed (review #2):** because the representation makes round-trip
identity-by-construction, the gate (`serialize(parse(bytes)) == bytes`, run before entering edit
mode → else view-only) is no longer what prevents clobbering of unmodeled actions (that is now
*structural*). It is a **model-soundness self-check** that catches our own parse/serialize
asymmetry — most importantly **line-ending fidelity**. The new parser must **not** use
`str::lines()` (it silently eats `\r` and the final-newline distinction); it must split on `\n`,
keep `\r`/EOL state per entry, and record whether the last line was terminated. This is the one
construct that can silently break round-trip; everything else (`=`-named keys, duplicate keys,
`[a+b]`/`[nav:C]` headers, weird spacing) is preserved free because untouched lines replay
verbatim. Lines we can't confidently *re-parse* with keyd's exact `parse_kvp` semantics become
`Typed::Raw` and **non-editable** (view-only for that line) — never clobbered.

**E-later upgrade:** a full lossless CST (spans + trivia) enables *surgical* edits that preserve
exact formatting even inside modeled sections. The edit model is designed so a CST can back it
later; the UI depends only on a single `can_round_trip(file) -> bool` capability bit, which is
the only formatting concern allowed above the model.

### 5.2 Privilege & apply path — **DECIDED: single transient privileged tool, no live channel**

Review #1 established that there is **no safely-constrainable live `keyd bind` channel** (§3.2:
`bind` can install `macro()`/any action; the socket has no caller auth; a same-uid channel
re-grants root to every process running as the user). So we drop options B/C from v1 entirely.

**The only privileged path is a transient, one-shot tool invoked via polkit/pkexec for *persist
only*:**

- **No caller-supplied paths.** The GUI does not hand the tool a file path or a target name to
  write. The tool reads the candidate config from a fixed, root-checked location (or from
  stdin), and the destination filename is validated against a strict pattern (`^[A-Za-z0-9_-]+$`)
  before becoming `/etc/keyd/<name>.conf`. This blocks `../cron.d/x`-style traversal and
  arbitrary-`/etc`-write.
- **No symlink / TOCTOU games.** Open the target directory with a dir-fd on `/etc/keyd`; write to
  a temp file with `O_NOFOLLOW`; `keyd check` **the exact bytes just written** (not the draft the
  GUI showed); then atomically `renameat` into place. Back up the prior file(s) the same way.
- **The long-lived helper never gains write/bind capability.** Its events-out-only design and
  sandbox stay intact. The privileged capability exists only for the lifetime of one
  authenticated `pkexec` invocation.
- **polkit action is itself a security artifact.** Spec its action id and `allow_active` setting
  explicitly. If apply is `auth_admin` per save, accept the prompt; do **not** set
  `allow_active=yes`, which would turn the tool into a silent root-write primitive any same-uid
  process could drive.

**Preview is not on this path** — preview is visual (§5.6), so there is no per-keystroke
privileged traffic and thus no auth-fatigue pressure to weaken the polkit policy.

### 5.3 Security: what the apply tool must enforce — **DECIDED**

Because socket access is root-equivalent and `keyd check` validates only syntax, the *content*
safety check is ours to build, and it must run **on the final serialized bytes**, not on the
edit model:

- **Reject (or require separate, explicit, unmistakable confirmation for) `command(`, `macro(`,
  and `include`** in the bytes being written. `command()` is root code-exec; `macro()` is
  keystroke injection as root; `include` launders content past the scan by pulling in another
  file at reload time. None are exempt just because they arrived as opaque `Raw` bindings — the
  scan does not trust the model.
- **Tokenize with keyd's own escaping rules.** keyd un-escapes `\(` `\)` etc. inside
  `command(...)`; a naive substring scan can be evaded. The scan must replicate keyd's grammar so
  obfuscated forms can't slip through.
- **`include` policy — DECIDED (review #2): permit it; scan one level deep as an advisory
  footgun-catcher, do NOT refuse.** Review #1's "include laundering" worry turns out to be
  largely **circular**, verified against `config.c`: `resolve_include_path` confines includes to
  the config's own dir (`/etc/keyd`) or the `DATA_DIR` fallback (`/usr/share/keyd`) — **both
  root-owned** — and `exists_and_is_relative` uses `realpath` + a prefix check that neutralizes
  `..`, symlinks, and absolute-path escapes. `include` is **one level deep** (non-recursive). So
  an attacker cannot control included content without already having write access to a root-owned
  dir — i.e. already root. The real residual risk is a *legitimate admin* including a file that
  itself contains `command(`/`macro(` — a footgun, not an escalation. Therefore: MVP
  (draft-then-install) just **round-trips `include` lines verbatim**, no scan needed; the
  one-click apply path (E2) does a **one-level closure scan** and routes any `command(`/`macro(`
  found in an included file to the same explicit-confirmation flow as inline ones. Match keyd's
  non-recursion (don't build a recursive vetter — it would diverge from keyd). Document that the
  scan reflects apply-time state only and cannot re-vet future reloads — which is fine, because
  included content is already root-gated. Reject absolute/`..` include args with a warning
  (keyd ignores them anyway) as belt-and-suspenders.
- **Never fall open.** If `keyd check` is absent or the environment can't be validated, **refuse
  to persist** — do not fall back to our lossy parser as a security check.
- The user's own machine is the trust boundary we *can't* fully close: `SO_PEERCRED` + active
  session tells us *which human*, not *which program*, so it cannot distinguish the GUI from
  other same-uid processes. This is exactly why there is no live channel and why apply is a
  discrete, user-confirmed polkit action rather than a standing capability.

### 5.4 Safety: validate, apply, dead-man's-switch revert — **DECIDED**

So the user can never get stuck, in order of reliability:

1. **The panic sequence is the primary failsafe.** Backspace+Escape+Enter terminates keyd and
   restores the raw keyboard, no matter how broken the config — surface it prominently in the
   edit UI and docs. Everything below is to avoid *needing* it.
2. **`keyd check`** the exact bytes before they go live (syntax gate; not safety — see §5.3).
3. **Apply with a dead-man's-switch held by the *root* tool, not the GUI.** The privileged tool
   writes + reloads, then **blocks for N seconds waiting for a positive "keep" confirmation** on
   a private fd. Confirm = keep; **anything else (timeout, GUI crash, user can't interact)
   reverts** to the backup and reloads. Revert authority lives in the privileged process because
   the unprivileged GUI cannot write `/etc/keyd`; "keep" is the action that requires working
   input, and its *absence* is safe. (`keyd check` can't catch logical lockouts like disabling
   every key, so this matters even for syntactically valid configs.)
4. **Transactional backup/restore — DECIDED (review #2): a write-set the MVP keeps at size 1.**
   The privileged tool takes a **list** of writes `(path, prior: Existed(bytes) | Absent)` and
   applies/reverts them all-or-nothing, with timestamped backups (same dir-fd + `O_NOFOLLOW`
   discipline as §5.2). **In the MVP the list has exactly one element** (open one file → change a
   binding → write it back), because the MVP performs no multi-file write — global reload re-reads
   every file, but only our one file's *bytes* changed, so every other keyboard re-derives
   identically. Review #1's "atomic *set*" is the right long-term shape but is only exercised by
   E2+ structural ops (create-config, split/move an id between files), which introduce the
   `Absent` case (revert = **delete** the newly-created file, not restore). Designing the
   list+`Existed|Absent` interface now is the cheap forward-compat; MVP just never passes >1.

### 5.5 Editing scope, detect-keyboards & create-config — **DECIDED (review #2)**

**Edit per *file*, not per *device*.** Verified in `config.c`/man: all `[ids]` in one file
**share one state** — there is no per-device scoping inside a file. So a per-device editor would
be a lie the moment a file lists two keyboards (changing a binding changes it for *both*).
Therefore the edit unit is the file, with a **persistent affected-keyboards banner**:
- one concrete id → "Editing config for <device> (`vendor:product`)";
- multiple ids → "Changes affect N keyboards: X, Y" (connected ones emphasized — we already track
  which matched, `main.rs`);
- wildcard (`[ids] *`) → "Applies to ALL keyboards not claimed by another config" + list the
  devices other configs carve out (compute via the existing cross-file ranker).

**Create-config routes through a "who governs this device?" check.** Device enumeration is free
(`connected_devices()`). keyd does **not actually forbid** a duplicate id in two files (review #2
correction) — it resolves the clash *nondeterministically* by `readdir` order, which is worse. So
before creating, run the existing ranker: a specific config (rank 2) beats a wildcard (rank 1),
so if a device is already governed, offer to **edit the governing config** rather than spawn a
colliding file. When nothing claims it, create a starter `[ids]`+`[main]` — and write **only that
new file** (no need to add an exclusion to the wildcard config, since the new specific config
already out-ranks it), keeping create single-file in the MVP. Also **warn on load** if two files
contain a true same-rank duplicate id (a pre-existing user misconfiguration we shouldn't silently
pick a side on).

### 5.6 Preview is visual, not applied — **DECIDED (review #1)**

v1 proposed previewing edits by pushing them to the live keyboard via `keyd bind`. On a single
keyboard that's the device you're typing on — remap Enter or Space mid-edit and you may be unable
to confirm the very dialog asking you to keep it. And it needs the unsafe channel (§5.2). So:

- **MVP preview is purely visual** — the board already renders what a binding does; show the
  edited binding's new legend/badge on the existing board. Zero privilege, zero risk, and it's
  the thing keyd-viz is already good at.
- A real *applied* "feel it on your keyboard" preview can come much later, only after the
  dead-man's-switch apply infra is battle-tested, and only as an explicit momentary action
  ("hold to preview") routed through the same audited apply path — never a standing channel.

---

## 6. Phased plan

Ordered to make "edit my real config safely" real as early as possible, and to de-risk the two
cruxes before UI breadth. Each phase carries a **Done when** — the acceptance bar, not a task
list. Task-level breakdown is deliberately deferred until E0 freezes the data model and
serializer; planning tasks against an interface that doesn't exist yet is the sequencing trap
from review #1.

- **E0 — Foundations & decisions (mostly non-UI).**
  1. Make the single parser round-trippable: typed + `Raw` bindings, opaque trivia/sections,
     and the `serialize(parse(file)) == file` gate (§5.1). **Freeze the EditModel + serializer
     interface first**, then write its property tests.
  2. Prototype the privileged apply tool (§5.2): no caller paths, name validation, `O_NOFOLLOW`
     + temp + `renameat`, byte-level safety scan (§5.3), dead-man's-switch revert (§5.4).
  3. Probe keyd at runtime (`keyd check`/`list-keys` presence, socket path, version).
  - **Done when:** the parser round-trips a corpus of real-world configs (`serialize(parse(f))
    == f`) under test; the EditModel + serializer interface is documented and frozen; the apply
    tool prototype passes the safety scan and demonstrates dead-man's-switch revert in a manual
    test; the keyd runtime probe works on this machine.
- **E1 — Edit a real config (draft-then-install).** Open any config the round-trip gate accepts;
  click a key → change a plain remap / disable / a couple of common actions, entered via
  **press-to-capture or a categorized palette** (§9); visual preview on the board (§5.6); save a
  draft + show the install steps. No daemon changes, no privileged writer yet. First version with
  a real audience.
  - **Done when:** a user can open their real config, change a single binding by pressing the key
    they want (or picking from the palette), see it re-render on the board, and save a draft with
    copy-paste install steps; the round-trip gate sends unsupported files to view-only instead of
    clobbering them; the viewer is unchanged when not editing.
- **E2 — Breadth + one-click apply.** More typed actions (common → uncommon); the per-key **"when
  tapped / when held"** editor for `overload`/`lettermod` (§9); layers (add/rename/delete) with
  named layer-switch variants (momentary/toggle/oneshot); chords; `[global]` options; non-blocking
  orphan/conflict warnings (§9); the polkit one-click apply with dead-man's-switch revert; key
  picker fed by `keyd list-keys`; create-config flow with wildcard-id collision handling (§5.5).
  - **Done when:** the common keyd vocabulary is editable visually (tap-hold first-class, not a
    raw escape hatch); a source/AUR install can apply changes in one click with working
    auto-revert; creating a config for an unconfigured keyboard never produces a colliding
    `[ids]`; AppImage users still get draft-then-install (documented, §7).
- **E3 — Power & fidelity.** Lossless CST for surgical formatting-preserving edits; macro editor
  (editable step table / record-then-edit, §9); `command()` behind explicit confirmation;
  aliases/includes/composite/layout layers; community snippet import (§9); undo/redo.
  - **Done when:** edits to hand-authored files preserve exact formatting inside modeled sections;
    macros are buildable without hand-writing the macro string; advanced sections are editable;
    importing a shared snippet adapts it to the target config's layer/device names.

---

## 7. Risks & open questions

- **[DECIDED]** No live `keyd bind` channel; single transient pkexec apply tool (§5.2). *(was the
  top open question in v1)*
- **[DECIDED]** One keyd-faithful parser; round-trip-diff gate, not a header (§5.1).
- **[DECIDED]** Revert authority lives in the privileged tool as a dead-man's switch; panic
  sequence is the primary failsafe (§5.4).
- **[DECIDED]** (review #2) Round-trip representation is **per-line verbatim entries**;
  `[global]`/`[aliases]`/`include` are ordinary `Raw` entries; round-trip is identity-by-
  construction and the gate is a model-soundness self-check (§5.1).
- **[DECIDED]** (review #2) `include` is **permitted**, not refused — keyd confines includes to
  root-owned dirs, so the laundering threat is circular; one-level closure scan at apply (§5.3).
- **[DECIDED]** (review #2) Edit **per-file** with an affected-keyboards banner; backup is a
  transactional write-set the MVP keeps at size 1 (§5.4, §5.5).
- **[DECIDED]** (review #2) The AppImage uses **draft-then-install permanently** — there is no
  safe one-click apply for a pure portable AppImage (pkexec needs a root-owned tool the AppImage
  can't place; the `pkexec sh -c` dodge is a local-root hole). One-click apply is an AUR/source
  feature; word it as a packaging trade-off, not a missing feature. *(See decision log: the
  pkexec-bundled-tool path is explicitly rejected so it isn't re-proposed.)*
- **[DECIDED]** (review #2) Test oracle: ~30 hand-authored T1 cases (cap ~40), human-written
  expected output; explicit golden-review policy on keyd bumps (§8.2).
- **[RISK]** Version drift (`keyd check`/socket/grammar vary by version) → probe at runtime;
  security checks must **fail closed**, never fall back to the lossy parser (§5.3).
- **[RISK]** Regression of the beloved viewer → editing is a separate, opt-in mode; the parser
  upgrade is covered by the existing + new parser tests.
- **[RISK]** (review #2) keyd does **not** reject duplicate ids across files — it resolves them
  nondeterministically by `readdir` order; the editor must detect and warn rather than silently
  pick a file (§5.5).

---

## 8. Testing protocol — honest about what each tier proves

User's framing: **a harness that emulates layout changes and proves the right keys come out,
kept green throughout development.** The good news from the spike: keyd ships a deterministic,
kernel-free engine we can reuse. The correction from review #1: be precise about *what* it
proves — much of it tests keyd, not our editor, so the hand-authored oracle has to be the spine.

Verified facts (keyd 2.6.0 @ `f564288`, source at `/tmp/keyd-src`):

- **keyd's `test-io` (`t/test-io.c`)** links keyd's real processing core and runs `.t` cases
  (input events + virtual-time `NNNms` lines → expected output) with **simulated, deterministic
  time, no kernel, no root** (confirmed: timeouts are driven by injected `ev->timestamp`, not
  wall clock). It takes an arbitrary config path as `argv[1]`, but builds **one** keyboard from
  it and runs all `.t` files against that — so per-config testing means one process spawn per
  config, and there are hard limits (`static char buf[4096]` per file, 1024 events). "µs/case"
  is in-loop time and excludes parse+spawn.
- **keyd's `t/run.sh` + `runner.py`** is the real end-to-end harness: root, builds keyd with a
  temp `CONFIG_DIR`, launches the daemon, creates a `/dev/uinput` virtual keyboard, grabs the
  output device, injects + reads back. (`CONFIG_DIR`/`SOCKET_PATH` are **compile-time** macros;
  isolation = build with custom values.)
- **License: keyd is MIT** (top-level `LICENSE`; per-file headers point to it). So vendoring its
  source into our test build is fine — **no copyleft contamination** (the earlier GPL worry was
  unfounded). Obligation: preserve keyd's copyright notice + MIT text where we vendor it (e.g.
  `tests/vendor/keyd/LICENSE`), and re-check the license on every SHA bump.
- **Coupling caveat:** `test-io` links keyd's *internal* symbols (`kbd_process_events`,
  `struct keyboard`, …), which have no stability contract. A keyd refactor can break our test
  *build*, not just goldens — budget keyd bumps as deliberate ports.
- Tooling present: `keyd 2.6.0`; `python-evdev 1.9.3` + `libevdev 1.13.6` installed; `evemu` in
  `extra`.

### 8.1 The tiers — **PROPOSED**

| Tier | What it actually proves | Engine | Privilege | Runs |
| --- | --- | --- | --- | --- |
| **T0 Round-trip + validate** | `serialize(parse(f)) == f` on a corpus of real configs; serializer output passes `keyd check` | our code + `keyd check` | none | every commit |
| **T1 Serializer-intent** | for a *hand-authored* `(intent → expected output)` case, `serialize(intent)` produces a config whose `test-io` output matches the expected sequence | keyd `test-io` core | none | every commit |
| **T1b keyd-regression canary** | generated configs' `test-io` output matches frozen goldens (catches drift in our serializer *or* in keyd) | keyd `test-io` core | none | every commit |
| **T2 E2E smoke** | a dozen anchor cases agree on a real kernel/uinput path | uinput + isolated keyd | root / uinput | **self-hosted / pre-release, opt-in** |
| **T3 Safety/UI** | dead-man's-switch revert, backup restore, `command(`/`macro(`/`include` rejection, panel snapshots | our code; Slint demo | none | every commit |

### 8.2 The oracle problem — why the hand-authored suite is the spine — **DECIDED (review #2)**

The trap (review #1): in a purely *generated* test, keyd is **both** the oracle and the
system-under-test. If our serializer emits a wrong-but-valid config (say it swaps `overloadt`'s
two timing args), keyd faithfully runs the wrong config, `test-io` captures a self-consistent
"golden," and the test goes **green** — the bug ships. So:

- **T1 (the real correctness tier)** requires the *expected output* to come from human intent,
  independent of the serializer: a curated `(intent, config-or-not, expected key sequence)` suite
  in keyd's `.t` style, covering every action type. This is small, high-trust, and the actual
  proof that *our* mapping is right.
- **T1b (canary)** is honestly just regression safety: "keyd still does what it did, and our
  serializer still emits the same text." Valuable, but it does **not** prove correctness — don't
  let its case count masquerade as coverage.
- **Drop the "thousands of cases" framing.** keyd covers its whole engine in ~94 hand cases;
  most key-iteration is redundant because keyd is key-agnostic for most actions. State coverage
  as (action types × timing boundaries × structural contexts), cap key iteration to a few
  representatives, and let the generator fuzz timing boundaries — not multiply trivially.

**Concrete T1 sizing (review #2), mirroring keyd's own proportions (dense on timing/state, sparse
where keyd is key-agnostic):** target **~30 hand-authored cases, hard cap ~40**:
- one canonical happy-path case per MVP action type (~10: remap, disable/noop, modifier, layer,
  oneshot, toggle, overload, lettermod, chord, macro) — catches wrong-keyword / swapped-operand
  bugs;
- ~12–18 timing-boundary cases for the timing-sensitive actions (`overload`/`overloadt(2)`,
  `lettermod`, `chord`, `oneshot`, `timeout`), straddling the threshold from **both sides** — this
  is where a swapped-timing-arg bug actually surfaces; ~2–3 each, trivial actions get one;
- ~3–5 structural-context cases (action inside a named layer, a `[layer:mods]` header, an `[a+b]`
  chord header) — the risk there is *section* serialization, not the binding.
- **T1 expected output is human-written, never captured from our own serializer** (that's the
  keyd-is-both-oracle-and-SUT trap). If you can't hand-write the expected sequence, it belongs in
  the T1b canary, not T1. T1 never grows by generation; a bug fix adds exactly one T1 case.

**Golden-review policy on keyd SHA bumps (review #2)** — run T1 and T1b together; the
discriminator is T1:
- **T1 green + T1b red → keyd changed behavior.** The canary did its job. OK to re-baseline T1b,
  but only after a human reads the keyd diff/changelog and writes a one-line note (in the commit /
  decision log) of *what* changed. Also re-check keyd's LICENSE on the bump.
- **T1 red → stop, do not re-baseline.** Human-expected output no longer holds — either a
  correctness-relevant keyd change we must adapt to, or our serializer regressed. Investigate;
  re-baselining T1 is a deliberate, reviewed correctness decision, never a mechanical regen.
- **Never re-baseline both tiers in one unreviewed step** (that silently blesses whatever changed
  — the green-but-wrong failure mode). A *build* break from the internal-symbol coupling is a
  separate, planned port handled before goldens are even evaluated.

### 8.3 The virtual keyboard (T2) — real, but kept in its lane

The uinput E2E is the only tier that exercises the real kernel/timing path, so it's the ground
truth that T1/T1b agree with reality — on a **dozen** anchor cases, not the bulk. Caveats that
keep it from rotting into a permanently-skipped job:
- **Needs a self-hosted runner.** Hosted GitHub runners don't reliably provide `/dev/uinput` /
  `modprobe uinput`; `--privileged` is a container notion that doesn't apply to the hosted host
  VM. Make T2 non-gating and explicitly opt-in (release-tag / nightly / local).
- **Real wall-clock timing flakes** on tap-vs-hold boundaries; keep T2 cases away from the knife-
  edge timings (those live in T1 where time is simulated).
- **Hard-fail if `.grab()` fails** rather than proceeding — a failed grab leaks injected
  keystrokes into the runner's (or developer's) real session.

### 8.4 Sequencing & maintenance

- T0/T1/T1b/T3 run every push (no privilege). T2 is separate and opt-in.
- **Freeze the EditModel + serializer interface before writing their tests** (§6 E0) — test-first
  applies to *behavior* once the *interface* exists, not before.
- **Pin keyd's source** (submodule or vendored at a known SHA); a keyd behavior change turning
  T1b red is the canary working; a *build* break is a planned port (§8 coupling caveat).
- Pin T0's `keyd check` to the same vendored keyd, or document the version skew between the
  installed `keyd check` and the pinned `test-io`.
- Respect the repo's hand-formatting rule: no `cargo fmt --check` gate.

---

## 9. North-star end state & UX vision

Where Edit Mode is headed once fully realized (well beyond the MVP) — so the phases aim at one
coherent picture:

> You open keyd-viz and see your real keyboard, drawn per layer. You click any key on any layer
> board; it highlights. You **press the key you want it to become**, or pick from a categorized
> palette (keys · modifiers · named layers · media · macro · command). For a dual-function key
> you fill a **"when tapped"** and a **"when held"** slot independently; the board immediately
> shows both legends on the cap. Layers are named, reorderable, each with its own color hue. You
> can target a specific keyboard (keyd's `[ids]`) — something the big commercial tools can't do
> cleanly. Bad edits are caught at authoring time (orphaned keys, lockouts) and again by a
> `keyd check` dry-run; applying is one click, validated, backed up, and auto-reverts if you
> don't confirm — with Backspace+Escape+Enter as the always-there panic escape. The same live
> layer view, keypress glow, heatmap, and compact overlay that make keyd-viz a great *viewer*
> double as the editor's instant feedback loop: edit → see it immediately, no compile, no flash.

The MVP (§4) is the smallest honest step toward this: open a real config, change one binding by
capture-or-palette, see it on the board, save it back without losing anything.

---

## 10. Prior art & competitive research

No mainstream GUI edits keyd. The closest *software* remappers (config-file + GUI, our exact
situation) are **Karabiner-Elements** (macOS) and **PowerToys Keyboard Manager** (Windows). The
best *visual* editors are firmware tools — **ZSA Oryx / Keymapp**, **VIA**, **Vial**, **QMK
Configurator**. Also surveyed: kanata/kmonad, Keyboard Maestro, AutoHotkey GUIs, and vendor
suites (Logitech G HUB, Razer Synapse, SteelSeries GG). Full agent findings + source URLs are in
the conversation log; the actionable distillation:

### Where keyd-viz already leads — don't rebuild what we have
- **One board per layer, live active-layer view, keypress glow, compact always-on-top overlay** —
  these are exactly what Oryx/Keymapp are *praised* for and what VIA/QMK do worse. We're ahead on
  visualization; Edit Mode is about matching their *authoring* UX, not their rendering.
- We already render **both tap and hold legends on a dual-function keycap** — the precise thing
  VIA is criticized for omitting.
- **Per-device `[ids]` is a genuine moat:** PowerToys flatly can't target devices ("no API"),
  Karabiner needs a separate EventViewer app to even find device IDs. We enumerate devices for
  free — lean in, and surface device IDs in-app.

### Patterns to adopt (priority order)
1. **Press-to-capture key input** + searchable-dropdown fallback (PowerToys; the single most
   common Karabiner complaint is the *lack* of it). "Press the key you want" beats dropdown-
   hunting. Keep capture and browse in one affordance — don't make them fight (PowerToys
   anti-pattern). → E1.
2. **Per-key "when tapped / when held" slots, fully orthogonal** (Oryx) — this *is* keyd's
   `overload`/`lettermod`. First-class in the GUI; never a raw-config escape hatch. Karabiner and
   QMK both punt on tap-hold — their biggest gap — and tap-hold is keyd's headline. → E2.
3. **Click-key-highlights → categorized action palette** (VIA): categories map to keyd as keys ·
   modifiers · **named layers** (momentary/toggle/oneshot as named choices) · media · macro ·
   command. Always show layer **names**, never indices (QMK anti-pattern). → E1/E2.
4. **Non-blocking orphan/conflict warnings at authoring time** (PowerToys's orphaned-key flag):
   if a remap leaves a key unproducible, or a modifier remap has downstream effects, warn inline —
   not only in docs. → E2.
5. **`keyd check` dry-run before apply** (kanata/Karabiner validate-on-save) — already in the
   safety plan (§5.4). → E2.
6. **Opaque pass-through preservation** (Karabiner) — independent validation of our §5.1 `Raw`
   round-trip. Note Karabiner "solves" round-trip only by owning the file and *accepting comment
   loss*; we do better with verbatim preservation + the round-trip gate, plus **timestamped
   backups before every write** (which Karabiner lacks). → E0/E1.
7. **Community snippet gallery with one-click import** (Karabiner's killer feature) — high value
   but **later**: keyd snippets are less portable (layer names, device IDs), so they need
   templating/namespacing or they paste rules that silently don't fit. → E3.
8. **Macro editor as an editable step table / record-then-edit** (Keyboard Maestro, Pulover's
   Macro Creator) for keyd `macro`/`macro2`. → E3.

### Extensions to our live view (cheap, high perceived value)
Per-layer **heatmap** (Keymapp), **tray layer indicator** (kanata-tray — already on the v1.2
roadmap), per-layer **color hue** on boards (Oryx). The kanata+OverKeys live-layer-overlay idea
is essentially what our compact mode already is.

### Anti-patterns to avoid
Dropdown-only key picking with no capture mode (Karabiner's #1 complaint); punting advanced
features to a raw-text escape hatch (Karabiner/QMK); wholesale file rewrite that nukes comments
(Karabiner); raw numeric layer indices (QMK); surfacing footguns only in docs instead of at
authoring time (PowerToys); account/cloud lock-in and daemon bloat (vendor suites) — stay
local-first and file-based, which is our advantage.

---

## 11. Decision log

- **2026-06-05 — v1 drafted** from the E0 spike. Nothing finalized.
- **2026-06-05 — critic review #1** (4 adversarial reviewers, verified against keyd source).
  Resolutions folded into v2:
  - **Security model corrected.** "Refuse `command()`" is insufficient (`macro()` is also
    root-equivalent; keyd's socket has no caller auth; `keyd check` is syntax-only; same-uid
    channels re-grant root). **Dropped the live `keyd bind` channel; single transient pkexec
    apply tool with no caller paths, byte-level grammar-aware scan, dead-man's-switch revert held
    by the root tool; panic sequence is the primary failsafe.** (§5.2–§5.4)
  - **Round-trip pulled into the MVP.** Verbatim preservation of unmodeled constructs + a
    `serialize(parse(file)) == file` gate replace v1's header-based "app-owned, hand-authored is
    view-only" scoping, which served almost no real users. (§4, §5.1)
  - **One keyd-faithful parser** drives viewer + editor; `Raw` bindings render as their RHS.
    (§5.1)
  - **Two live viewer bugs found and fixed** outside this doc (`[layer:mods]`/`[a+b]` headers;
    inline `#`). (§3.1)
  - **Preview is visual, not applied** (§5.6).
  - **Testing reframed honestly:** hand-authored intent oracle is the correctness spine; the
    generated tier is a regression canary, not correctness; dropped "thousands of cases"; keyd is
    **MIT** (no GPL problem) with an attribution obligation; T2 needs a self-hosted runner and is
    opt-in; fixed the test-before-interface sequencing cycle. (§8)
  - **Dropped the "VIA/Vial moment" framing** — honestly positioned as a GUI for `/etc/keyd`.
  - Roadmap ordering unchanged: Phase 6 stays where it is (owner's call — the cost-based argument
    to do tray/hotkey first doesn't outweigh this being the project's defining ambition).
- **2026-06-06 — competitive research** (Karabiner, PowerToys, Oryx/Keymapp, VIA, Vial, QMK,
  kanata/kmonad, Keyboard Maestro, vendor suites). Added §9 (north-star UX vision), §10 (prior
  art with steal/avoid lists), and per-phase **Done when** criteria in §6. Key conclusions:
  keyd-viz already leads on *visualization* (board-per-layer, live layer, glow, overlay,
  both-legends-on-cap) — Edit Mode targets *authoring* UX; **press-to-capture + categorized
  palette** and **orthogonal tap/hold slots** are the highest-priority patterns to adopt;
  **per-device `[ids]`** is a moat competitors can't match; the round-trip + backup design is
  validated against how Karabiner *fails* to preserve comments. Task-level breakdown still
  deferred to post-E0.
- **2026-06-06 — review #2** (4 investigators resolving the OPEN items, verified against keyd
  source). All E0-blocking unknowns are now DECIDED:
  - **Round-trip representation → per-line verbatim entries.** keyd is strictly line-oriented (no
    multi-line constructs), so `serialize(parse(f)) == f` is identity-by-construction; the gate
    becomes a model-soundness self-check whose one real risk is line-ending fidelity (don't use
    `str::lines()`). `[global]`/`[aliases]`/`include` are not special-cased. (§5.1)
  - **`include` permitted, not refused.** Verified keyd confines includes to root-owned dirs
    (`realpath`+prefix, one level deep), so "include laundering" is circular — an advisory
    one-level closure scan at apply replaces the blanket refusal. (§5.3)
  - **Edit per-file + affected-keyboards banner; backup is a transactional write-set, size 1 in
    the MVP.** keyd shares all state across a file's ids, so per-device editing would be a lie;
    multi-file atomic sets are only needed for E2+ create/split/move. Also: keyd does **not**
    forbid duplicate ids across files (resolves nondeterministically) — detect & warn. (§5.4, §5.5)
  - **AppImage = draft-then-install permanently.** No safe one-click for a pure AppImage; the
    `pkexec`-a-bundled-tool path is **rejected** (inert via ownership check, or a local-root hole
    via interpreter+args) — recorded here so it isn't re-proposed. (§7)
  - **Test oracle sized:** ~30 hand-authored T1 cases (cap ~40), human-written expected output;
    concrete golden-review policy keyed on T1-vs-T1b discrimination at keyd bumps. (§8.2)

---

## 12. Appendix: parser-faithfulness checklist (pre-E0 audit, 2026-06-06)

A pre-E0 audit compared the current parser/supporting modules against keyd's source. Most of the
parser proved **faithful**; this is the punch-list of real divergences for E0's parser rework,
plus what was verified correct (so E0 doesn't re-investigate).

**Already fixed and shipped (viewer):**
- Section-header grammar — `[layer:mods]`, `[a+b]`, any `[...]` (was `[A-Za-z0-9_]` only).
- Inline `#` is literal; `#` is a comment only at line start (matches `ini.c`).
- `[global]`/`[aliases]` no longer render as bogus layers (keyd special-cases them in `do_parse`).

**Fix during E0 (real divergences, mostly need the new line-faithful model):**
- **Mod-vs-Layer classification** uses a hardcoded name list (`control/shift/alt/meta/altgr`),
  not the layer's `:modset` qualifier — a custom modifier layer (`[caps:C]`) mis-renders. Capture
  the `:`-qualifier (modset vs `layout` vs composite) on the layer and classify from it.
- **General chords & composite layers.** Only `a+b = toggle(x)` is modeled; `a+b = <other>`
  becomes a bogus remap keyed `"a+b"`, and `[a+b]` composite sections render as orphan layers
  instead of overlays of their constituents. Model arbitrary-RHS chords + composite layers
  (keyd canonicalises chord order: `a+b == b+a`).
- **Nested/escaped action args.** `parse_fn_call` requires the value to end with `)` and splits
  args on naive `,`, so `overload(nav, macro(a, b))` corrupts the tap. Port keyd's paren-depth +
  backslash-aware arg parser (`parse_fn`).
- **`overloadi`** arg semantics (both leading args are descriptors, not a layer) — niche.
- **`parse_kvp` parity** for round-trip: keyd allows a literal `=` key and keeps valueless
  entries; our `split_once('=')` differs. Needed for `serialize(parse(f)) == f` on odd files.
- **Device matching is a `bool`, keyd uses a capability bitset.** `ids::match_device(devid,
  is_keyboard)` can't represent a combo keyboard+mouse device, so `m:`/`k:` filters mismatch such
  devices. Replace `is_keyboard: bool` with a device-flag set and match by bitwise overlap
  (`config_check_match`). Structural — do it with the E0 model rework.
- **`[aliases]` resolution** (now that they're no longer rendered as layers): resolve aliased key
  names onto their physical keys so aliased bindings render in the right place.
- **Validation parity** for edit-mode diagnostics: keyd rejects a file with no `[ids]` / a leading
  bare assignment; surface that as a warning rather than silently parsing the remainder.

**Verified correct — do NOT re-litigate in E0:**
- `[ids]` matching: first-prefix-match-wins in file order, exclude short-circuits, `*` is a
  keyboards-only last-resort fallback — our two-loop `ids.rs` matches `config_check_match` for all
  realistic inputs (the audit's "exclude-ordering" and `-k:` "bugs" were false alarms).
- `overloadt2` arg order (layer, tap, timeout); `lettermod` timings discarded without breaking
  rendering.
- Keycode table values; US shifted-symbol maps; keysym alias coverage (`equal`/`minus`/… ↔ `=`/
  `-`/…); modifier shorthand. No `super`/`hyper` modifiers exist in keyd (`meta` == Super).
- Chord-glow matching is order-independent (set membership in `resolve_glow`), despite
  `output_chord` emitting order-dependent strings.
