# Phase 6 — Edit Mode (design doc)

> **Status:** DRAFT v1 (2026-06-05). Living document — to be refined over a few iterations
> and adversarial critic reviews before any code is written. Decisions marked **PROPOSED**
> are not final; **OPEN** marks unresolved questions.
>
> **Goal of the feature:** turn keyd-viz from a read-only visualizer into *the* GUI for keyd
> — visually create and edit keyd configs, with safe live preview and apply. The VIA/Vial
> moment for a software remapper that has never had a GUI.

---

## 1. Vision & scope

Today keyd-viz *renders* a keyboard from a keyd config. Edit Mode adds the reverse: the user
points at a key, picks what it should do, sees it live, and saves. It must:

- Work for a keyboard that has **no config yet** (detect it, create a starter config).
- Cover the **common** keyd actions first (remap, layer, overload, oneshot, toggle, lettermod,
  modifiers, chords, disable), then grow toward the full vocabulary.
- Be **safe**: never silently brick the user's keyboard; always offer an escape and a revert.
- Preserve keyd-viz's identity: the **viewer stays the default**; editing is an explicit mode;
  the GUI itself stays unprivileged.

**Non-goals (at least initially):**
- Not a general keyd config text editor — power users keep their `$EDITOR`.
- Not lossless round-tripping of hand-authored files in the MVP (see §4, §5.1).
- Not firmware/QMK editing (keyd is software-only; that's a different tool).

---

## 2. Why this is hard — the two cruxes

The *breadth* of keyd features is steady, low-risk, parallelizable work. The genuine
difficulty is two architectural cruxes that must be settled before building UI:

1. **Round-trip fidelity** — keyd configs are hand-authored (comments, aliases, ordering,
   formatting). Our current parser is lossy and only understands a subset of actions. An
   editor must either regenerate files (losing formatting) or carry a lossless representation.
2. **Privileged write + reload + not bricking input** — configs live in root-owned
   `/etc/keyd/`; applying needs a reload over a root-equivalent socket; a bad config can make
   the keyboard unusable. This collides with keyd-viz's "GUI is never privileged" principle
   and demands a real safety design.

---

## 3. Verified facts (the spike)

### 3.1 Our codebase

| Area | Finding | Source |
| --- | --- | --- |
| Parser | **Lossy.** `parse_text` (`crates/core/src/parser.rs:36`) drops comments, blank lines, whitespace, section/key ordering, and even semantic detail (e.g. `lettermod` timings → discarded). Model (`crates/core/src/model.rs`) is semantic-only, no spans, no source text. | spike |
| Action coverage | Parser only structures `overload*`, `lettermod`, `layer`, `toggle`. **Everything else passes through as a raw `remaps` string** (`macro`, `command`, `oneshot`, `swap`, sequences…). | spike |
| Serializer | **None exists.** No `Config → text`. | spike |
| Device enum | `devices::connected_devices()` reads `/proc/bus/input/devices` (world-readable, zero privilege) and lists **all** keyboards using keyd's own classification rule — config-independent. | `crates/app/src/devices.rs` |
| Config read | `/etc/keyd/*.conf` is world-readable; reading needs no privilege. | `crates/app/src/main.rs:348` |
| Existing writes | App already writes `~/.config/keyd-viz/layouts.tsv` (`crates/app/src/prefs.rs`) — precedent for user-writable storage (XDG-aware, best-effort). | spike |
| Helper | Strictly **one-directional, events-out**; hardened systemd sandbox (`SystemCallFilter=~execve`, `ProtectSystem=strict`, `PrivateNetwork`, `DevicePolicy=closed`). Runs as the `keyd-viz` system user, which **is in the `keyd` group**. | `crates/helper/`, `packaging/systemd/` |

### 3.2 keyd itself (v2.6.0, man dated 2025-09-11; probe at runtime — facts are version-dependent)

| Capability | Detail | Why it matters |
| --- | --- | --- |
| **Reload** | `keyd reload` over control socket `/var/run/keyd.socket` (`root:keyd`, `0660`). **Not** SIGUSR1; no file watching. Needs root **or** `keyd` group. | We must trigger reload ourselves, via a privileged path. |
| **Validate** | `keyd check [files…]` — pure parse/validate, no apply, no daemon, nonzero exit on failure. Version-gated. | Catch syntax/semantic errors *before* touching `/etc/keyd`. |
| **Live bind** | `keyd bind "<binding>"` applies a binding at runtime **without writing files**; `keyd bind reset` reverts to the on-disk keymap. | **Live preview** without persisting — and instant revert. |
| **Panic** | Hard-coded **Backspace + Escape + Enter** terminates keyd, restoring the raw keyboard. Not config-overridable. | Last-resort failsafe; document prominently. |
| **Key names** | `keyd list-keys` enumerates valid key names; `keyd monitor` discovers names + device ids. | Populate the key picker authoritatively. |
| **Files** | `/etc/keyd/*.conf`, `root:root 0644`, system-only. **No per-user config location.** | Writing the real config requires privilege. |
| **Security** | *"Users with access to the keyd socket should be considered privileged (assumed to have access to the entire system)"* — because `command()` runs shell **as root**. | **The single most important constraint** — see §5.3. |

### 3.3 The full keyd action vocabulary (breadth estimate)

- **Common (MVP):** plain remap (`a = b`), `noop` (disable), modifiers (`layer(control)`),
  `layer`, `oneshot`, `toggle`, `overload`, `lettermod`, chords (`a+b = …`), `macro`/`C-a`.
- **Uncommon (E2):** `overloadt`, `overloadt2`, `overloadi`, `oneshotm`, `oneshotk`, `layerm`,
  `togglem`, `swap`, `swapm`, `clear`, `clearm`, `macro2`, `timeout`, `setlayout`, `repeat`.
- **Sections:** `[ids]`, `[global]`, `[main]`, `[<layer>]`, `[<layer>:<mods>]`,
  `[<a>+<b>]` (composite), `[<name>:layout]`, `[aliases]`, `include`.
- **`[global]` options:** `*_timeout` family, `layer_indicator`, `default_layout`, etc.
- **Sensitive:** `command(<shell>)` runs as root — must be visibly flagged / gated in UI.

---

## 4. The insight that shrinks the MVP ~80%

Both cruxes can be **deferred out of the first shippable version**:

- **App-owned configs dodge round-tripping.** If the editor's first job is *creating* configs
  (and editing ones it created), keyd-viz owns the file format and can regenerate it freely —
  no comment/format preservation needed. Hand-authored files are shown **view-only ("edit in
  your text editor")** until the lossless layer exists. A managed-file header
  (`# Managed by keyd-viz`) sets expectations.
- **`keyd bind` live-preview + manual save dodges the privileged writer (at first).** The
  editor can preview edits live via `keyd bind` (instant, no files) and, to persist, **write a
  draft to `~/.config/keyd-viz/drafts/` and let the user install it** (even literally: show the
  generated text + a `sudo cp … /etc/keyd/ && sudo keyd reload` one-liner). The one-click
  polkit apply comes later as a self-contained phase.

So the MVP is **"detect any keyboard → generate a starter config → set bindings visually →
preview live → save a draft you install,"** not "reimplement keyd."

---

## 5. Architecture

### 5.1 Data model & round-trip — **PROPOSED**

Introduce an **edit model** distinct from the current render model:

- A structured representation of the keyd config: ids, global options, layers, and per-key
  **bindings**. Each binding is either a **known action** (typed: remap / layer / overload /
  oneshot / toggle / lettermod / chord / disable / modifier / macro…) **or an opaque `Raw`
  binding** carrying the original right-hand-side string verbatim. Opaque bindings render as
  "advanced (raw)" and are preserved untouched on save — this lets us cover 100% of configs
  without having modeled 100% of actions.
- **Serializer (`EditModel → text`)** with a single consistent formatting policy. ~200 LOC.
- **MVP fidelity policy:** regenerate-from-model for **app-owned** files; hand-authored/complex
  files are view-only. A file is "app-owned" if it carries our managed header (or lives in our
  drafts dir).
- **E3 upgrade path:** a lossless CST (spans + trivia) that enables surgical edits of
  hand-authored files preserving comments/formatting (~500–800 LOC parser+serializer). The edit
  model is designed so the CST can back it later without changing the UI layer.

**OPEN:** Do we build the edit model on top of the existing lossy parser (re-parsing for
structure, keeping raw RHS), or write a second, edit-oriented parser from the start? Leaning
toward a dedicated edit parser that always retains raw RHS strings, so nothing is ever lost.

### 5.2 Privilege & apply path — **PROPOSED (needs critic review)**

The GUI must stay unprivileged. Three boundaries are in play: **read** (free), **live preview**
(`keyd bind`), and **persist** (`/etc/keyd` write + `keyd reload`). Candidate designs:

- **(A) pkexec one-shot for persist; helper untouched.** Persisting spawns
  `pkexec /usr/libexec/keyd-viz-apply <draft>`, a tiny privileged tool that runs `keyd check`,
  backs up the old file, writes `/etc/keyd/<name>.conf`, runs `keyd reload`, and arms the
  auto-revert (§5.4). Simple, keeps the helper's events-out purity. **But** it can't drive
  per-edit live preview (one auth prompt per save is fine; per keystroke is not).
- **(B) helper gains a narrow, authz'd command channel** for `keyd bind`/`reload` (the helper
  is already in the `keyd` group). Enables fluid live preview. **But** this breaks the
  one-directional design and — critically — see §5.3, a command channel that can install
  arbitrary bindings is a **root escalation surface**.
- **(C) hybrid (leaning):** live preview via a *constrained* command channel (B) that
  **refuses `command()` and other shell-capable actions** and is gated to the same-uid active
  session; persist via pkexec one-shot (A) with full `keyd check` + backup + auto-revert.

**OPEN:** Is the constrained command channel worth the added attack surface, or do we ship
preview-via-pkexec-on-explicit-"preview"-button (coarser, but no daemon change)? This is the
top question for the first critic review.

### 5.3 Security: the `command()` / `keyd bind` escalation — **must-resolve**

keyd socket access ⇒ root-equivalent, because a binding can be `command(<shell>)` run as root.
Therefore **any** path that lets the GUI (or anything on the user's session) inject bindings is
a privilege-escalation vector. Hard rules for whichever design wins:

- The persist tool and any command channel **must reject** `command()` (and re-validate the
  whole file with `keyd check`) — keyd-viz never installs a shell-executing binding on the
  user's behalf without explicit, unmistakable, separately-confirmed intent.
- A live command channel **must** authenticate the peer (`SO_PEERCRED` + logind active session,
  exactly as the event socket already does) and **must not** be reachable by other users.
- Writing `/etc/keyd` must go through polkit (auth dialog), **not** by adding the user to the
  `keyd` group (that's a permanent, root-equivalent, session-wide downgrade — explicitly
  rejected by the project's permission philosophy).

### 5.4 Safety: validate, preview, auto-revert — **PROPOSED**

Layered defense so the user can never get stuck:

1. **Validate** every candidate with `keyd check` before it touches `/etc/keyd` (gate on the
   command's presence; fall back to our own parser if absent).
2. **Live preview** via `keyd bind` so edits are felt before they're persisted; `keyd bind
   reset` is instant revert.
3. **Apply-with-auto-revert** on persist: back up the current config, write+reload the new one,
   start a timer, and show a "Keep these changes? (reverting in 15s…)" dialog. No confirmation
   ⇒ restore the backup + reload. (`keyd check` can't catch *logical* lockouts like disabling
   every key, so the timer is essential.)
4. **Always surface the panic sequence** (Backspace+Escape+Enter) in the edit UI and docs.

### 5.5 Detect-all-keyboards & create-config flow

Already free (`connected_devices()`). Edit Mode lists every connected keyboard; for one with no
matching config, "Create config" generates a starter `[ids]` (its `vendor:product`) + `[main]`
into the drafts dir, then opens the editor.

---

## 6. Phased plan

Each phase is independently shippable and ordered to deliver value early while de-risking the
cruxes first.

- **E0 — Spikes & decisions (no user-facing code).** Prototype a surgical round-trip on a real
  config; prototype the persist path (pkexec + `keyd check` + backup + auto-revert) and, if
  pursued, the constrained command channel. Probe keyd at runtime (`keyd check`/`bind`/
  `list-keys` presence, socket path). **Settle §5.1, §5.2, §5.3 via critic review.** Output:
  this doc, finalized. **Also stand up the T0/T1 test harness (§8)** — vendor keyd at a pinned
  SHA, get `test-io` building in CI, and land the serializer's first property + behavioral
  tests — *before* the editor grows, since everything trusts the serializer.
- **E1 — Create + simple edit (app-owned, draft-then-install).** Detect all keyboards; create a
  starter config; click a key → set plain remap / disable / a couple of common actions; live
  preview via `keyd bind`; draft saved to XDG; "how to install" path. No daemon changes.
- **E2 — Breadth + one-click apply.** Layers (add/rename/delete), the common-then-uncommon
  action set, chords, `[global]` options; polkit one-click apply with auto-revert; a key picker
  fed by `keyd list-keys`.
- **E3 — Power & fidelity.** Lossless editing of hand-authored files (CST); macro editor;
  `command()` behind explicit confirmation; aliases, includes, composite/layout layers;
  undo/redo; import/export.

---

## 7. Risks & open questions

- **[OPEN, top priority]** Command channel (§5.2/§5.3): constrained live channel vs.
  pkexec-on-preview. Security vs. UX.
- **[OPEN]** Edit model atop existing parser vs. a new edit-oriented parser (§5.1).
- **[RISK]** Logical lockouts `keyd check` can't catch → mitigated by auto-revert + panic seq.
- **[RISK]** Version drift: `keyd check`/`bind`/socket path vary by keyd version → probe at
  runtime, degrade gracefully.
- **[RISK]** Scope creep: keyd's full vocabulary is large → opaque `Raw` bindings (§5.1) let us
  ship without 100% coverage; grow typed actions incrementally.
- **[RISK]** Regression of the beloved viewer → editing is a separate, opt-in mode.
- **[OPEN]** Multi-keyboard configs share layer state and one `[ids]` lives in one file — how
  does the editor present/guard that?
- **[OPEN]** Distro variance for polkit packaging (AppImage has no installer → apply may be
  AUR/source-install only; AppImage stays draft-then-manual).
- **[OPEN]** Test oracle for complex-timing actions (§8.2): how big a hand-authored ground-truth
  suite, and the golden-review policy on keyd version bumps.
- **[OPEN]** Vendor keyd source as a git submodule vs. fetch-at-pinned-SHA in CI (§8.5).

---

## 8. Testing protocol — behavioral verification at scale

The goal (user's framing): **a test harness that emulates layout changes and proves the right
keys come out — thousands of cases covering every keyd feature, kept green throughout
development.** A virtual keyboard is the dream; the *better* foundation turns out to already
exist upstream. Verified facts (E0 spike, keyd 2.6.0 source @ `f564288`):

- **keyd ships `test-io` (`t/test-io.c`)** — a harness that `#include`s keyd's headers and links
  its **actual processing core** (`keyboard.c`, `config.c`, `macro.c`, `keys.c`, …), calling the
  real `kbd_process_events()`. Output is captured by an in-memory `send_key` callback. **No
  kernel, no uinput, no root.** Timing is *simulated* — `.t` files carry virtual-time lines
  (`NNNms`); the engine is fully deterministic. Cases run in microseconds.
- **`.t` DSL:** input events (`<key> down/up`, interleaved `NNNms`), a blank line, then the
  expected output event sequence; comparison is exact. `make test-io` runs ~95 such cases
  against one shared `t/test.conf`.
- **keyd's *other* harness (`t/run.sh` + `runner.py`)** is the real end-to-end one: root, builds
  keyd with a temp `CONFIG_DIR`, launches the daemon, creates a `/dev/uinput` virtual keyboard,
  grabs keyd's output device, injects + reads back. This is the "virtual test keyboard."
- **Isolation:** `CONFIG_DIR`/`SOCKET_PATH` are **compile-time** macros (no runtime flag/env).
  An isolated keyd = `make CONFIG_DIR=/tmp/t SOCKET_PATH=/tmp/t.sock && ./bin/keyd`. Two
  instances coexist only with distinct socket paths.
- **Tooling on this machine:** `keyd 2.6.0`, source at `/tmp/keyd-src`; `python-evdev 1.9.3`
  and `libevdev 1.13.6` **installed**; `evemu` available in `extra` (record/replay fixtures).

### 8.1 The four tiers — **PROPOSED**

| Tier | What | Engine | Privilege | Speed | Scale | Runs |
| --- | --- | --- | --- | --- | --- | --- |
| **T0 Validate** | every generated config parses & validates | `keyd check` + our serializer property tests | none | µs–ms | all | every commit |
| **T1 Behavioral** | input sequence → expected output, per action × key | **keyd's `test-io` core** fed *our* generated config | none | µs/case | **thousands** | every commit |
| **T2 E2E smoke** | real virtual keyboard → isolated keyd → captured output | uinput (python-evdev) + isolated keyd build | root / `input`+uinput rule | ~100ms/case | curated dozens | nightly / pre-release |
| **T3 Safety/UI** | apply-revert, backup restore, `command()` rejection; editor panel snapshots | our code; Slint demo mode | none | ms | per feature | every commit |

**T1 is the workhorse and the answer to "thousands of cases."** We vendor/build `test-io`
against keyd's pinned source, generate a `(config, input, expected)` triple per scenario, and
run keyd's *real* engine. This validates the whole chain **UI-intent → EditModel → serializer →
keyd semantics** without a kernel — and doubles as a **regression canary for keyd upstream**.

### 8.2 The oracle problem (the honest hard part) — **OPEN**

For a *generated* config + *generated* input, what is the "expected output"? If we compute it
ourselves we're reimplementing keyd's semantics — circular. So expected-output comes from three
sources, used deliberately:

1. **Direct oracle (correctness) — simple actions only.** For plain remap / disable / pure
   modifier / straightforward layer, intent → output is unambiguous (`a = b` ⇒ press `a` yields
   `b`). Assert directly. This is where generated breadth gives *true* correctness coverage.
2. **Golden/snapshot (regression) — complex actions.** For tap/hold/overload/oneshot/chord
   timing, capture `test-io` output once, freeze it as golden, diff thereafter. Catches *any*
   future drift in our serializer **or** in keyd — but the first capture is assumed correct, so
   each golden must be reviewed when added (and re-reviewed on keyd version bumps).
3. **Hand-authored oracle (ground truth) — the keyd-style suite.** A curated set of
   `(config, .t)` cases with human-verified expected output, exactly like keyd's own `t/*.t`,
   covering every action type at least once. Small, high-trust, the backbone the generated
   cases hang off.

> **Don't oversell auto-generation.** Thousands of *generated* cases give breadth + regression
> safety; *correctness* of complex-timing cases still rests on the hand-authored oracle and
> reviewed goldens. The protocol must make that boundary explicit, not paper over it.

### 8.3 Generative strategy

A test-case generator enumerates `action × key(s) × (timing profile)` from the EditModel's
known-action set, emitting `(config fragment, input scenario, expected/golden)` triples — so
adding a new typed action automatically expands coverage. Property-based input (randomized but
seeded — note `Math.random` is unavailable in our workflow scripts, but Rust test code has
`rand`) explores tap-vs-hold boundary timing, overlapping holds, chord windows, oneshot expiry.

### 8.4 The virtual test keyboard (T2) — concrete shape

Pytest (or a Rust integration test) that: builds keyd from the pinned source with a temp
`CONFIG_DIR`/`SOCKET_PATH`; writes the generated config there; launches `./bin/keyd`; creates a
uinput device via `python-evdev` `UInput`; **`.grab()`s keyd's output device so events don't
leak into the real session**; injects a scenario; reads back; asserts. Gated behind a marker
(`@pytest.mark.e2e`) and a CI job that is `--privileged` with `uinput` loaded on the host. Kept
to a representative smoke set — it's the ground truth that the kernel/uinput/real-timing path
agrees with T1, not the bulk coverage.

### 8.5 CI integration & maintenance

- T0/T1/T3 run on every push (no privilege → standard GitHub runners). T2 is a separate
  nightly/pre-release privileged job.
- **Pin keyd's source** (submodule or vendored at a known SHA) so `test-io` and behavior are
  reproducible; bump deliberately and re-review goldens on bump (a keyd behavior change *should*
  turn T1 red — that's the canary working).
- Respect the repo's hand-formatting rule: no `cargo fmt --check` gate (per project policy).
- This protocol's T0/T1 layers should land **before** much editor code (test-first for the
  serializer), since the serializer's correctness is the foundation everything else trusts.

---

## 9. Decision log

_(Append decisions here as critic reviews resolve the OPEN/PROPOSED items.)_

- 2026-06-05 — Doc created from the E0 spike findings. Nothing finalized yet.
