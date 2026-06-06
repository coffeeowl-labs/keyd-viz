# Phase 6 — Edit Mode (design doc)

> **Status:** DRAFT v2 (2026-06-05). Living document — refined through adversarial critic
> reviews before code is written. v2 incorporates **critic review #1**, which corrected the
> security model (the drafted live-preview channel was unsafe) and moved lossless round-trip
> from a deferred phase into the MVP. Decisions marked **PROPOSED** are not final; **OPEN**
> marks unresolved questions; **DECIDED** marks settled ones.
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

Preservation extends to **whole unmodeled sections** (`[global]`, `[aliases]`, `include` lines)
and to comments/ordering, carried as opaque trivia. Granularity (per-line vs per-section raw) is
an implementation detail to settle in E0, but the contract is fixed:

**The round-trip gate (the core safety invariant):** before any save, the editor checks that
`serialize(parse(file))` reproduces the on-disk file. If it doesn't — i.e. the file contains
something the model can't reproduce — the editor **refuses to regenerate** and falls back to
view-only for that file. This gate, not a `# Managed by keyd-viz` header, is what prevents
silent clobbering. A header is at most a hint; the gate is the guarantee.

> Why this replaces v1's plan: v1 keyed fidelity off file *ownership* (a header), which silently
> clobbers a file the moment a user hand-edits it. The round-trip diff is an honest, content-
> based check that works for any file.

**Cost note:** the serializer is small for the *modeled* actions, but real fidelity lives in the
unmodeled sections and trivia — the earlier "~200 LOC" figure was for the easy half. Budget for
faithfully re-emitting `[ids]` (including `*` wildcards and `-id` exclusions), `[global]`,
`[aliases]`, and `include`.

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
- **`include` policy:** for app-managed files, either refuse `include` outright, or vet the
  transitive closure of included files on every save *and* note that the scan is meaningless if
  an attacker can drop a file the include points at. Simplest MVP stance: refuse to persist a
  config containing `include`.
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
4. **Atomic multi-file backup/restore.** Because reload is global and one `[ids]` can be shared
   across keyboards (§5.5), back up and restore the *set* of affected files together, so a revert
   can't leave a half-old/half-new state that bricks a *different* keyboard.

### 5.5 Detect-all-keyboards & create-config flow — **PROPOSED**

Device enumeration is already free (`connected_devices()`). Edit Mode lists every connected
keyboard. "Create config" for an unconfigured keyboard generates a starter `[ids]` (its
`vendor:product`) + `[main]` as a draft. **But "no config names this id" ≠ "this device is
unclaimed":** an existing config using `[ids] *` (wildcard) or exclusions already matches it, and
keyd forbids the same id in two files. So before offering create, evaluate wildcard/exclusion
matching; if a wildcard config already claims the device, offer to **edit that config** rather
than spawn a colliding file.

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
cruxes before UI breadth.

- **E0 — Foundations & decisions (mostly non-UI).**
  1. Make the single parser round-trippable: typed + `Raw` bindings, opaque trivia/sections,
     and the `serialize(parse(file)) == file` gate (§5.1). **Freeze the EditModel + serializer
     interface first**, then write its property tests (you can't write serializer tests before
     the type exists — v1's "tests before the serializer" was a cycle).
  2. Prototype the privileged apply tool (§5.2): no caller paths, name validation, `O_NOFOLLOW`
     + temp + `renameat`, byte-level safety scan (§5.3), dead-man's-switch revert (§5.4).
  3. Probe keyd at runtime (`keyd check`/`list-keys` presence, socket path, version).
- **E1 — Edit a real config (draft-then-install).** Open any config the round-trip gate accepts;
  click a key → change a plain remap / disable / a couple of common actions; visual preview;
  save a draft + show the install steps. No daemon changes, no privileged writer yet. This is the
  first version with a real audience.
- **E2 — Breadth + one-click apply.** More typed actions (common → uncommon), layers
  (add/rename/delete), chords, `[global]` options; the polkit one-click apply with the
  dead-man's-switch revert; key picker fed by `keyd list-keys`; create-config flow (§5.5).
- **E3 — Power & fidelity.** Lossless CST for surgical formatting-preserving edits; macro editor;
  `command()` behind explicit confirmation; aliases/includes/composite/layout layers; undo/redo.

---

## 7. Risks & open questions

- **[DECIDED]** No live `keyd bind` channel; single transient pkexec apply tool (§5.2). *(was the
  top open question in v1)*
- **[DECIDED]** One keyd-faithful parser; round-trip-diff gate, not a header (§5.1).
- **[DECIDED]** Revert authority lives in the privileged tool as a dead-man's switch; panic
  sequence is the primary failsafe (§5.4).
- **[OPEN]** `Raw`/trivia granularity (per-line vs per-section) and how `[global]`/`[aliases]`/
  `include` are carried (§5.1).
- **[OPEN]** `include` policy: refuse outright in managed files, or vet the transitive closure
  (§5.3).
- **[OPEN]** Multi-keyboard: shared `[ids]`/layer state across one file — UI presentation and the
  atomic backup set (§5.4, §5.5).
- **[OPEN]** Distro variance for polkit packaging: the AppImage has no installer, so one-click
  apply is likely AUR/source-install only; **AppImage users stay on draft-then-install** even
  after E2 — state this up front, not as a footnote.
- **[OPEN]** Test oracle for complex-timing actions and golden-review policy on keyd bumps (§8).
- **[RISK]** Version drift (`keyd check`/socket/grammar vary by version) → probe at runtime;
  security checks must **fail closed**, never fall back to the lossy parser (§5.3).
- **[RISK]** Regression of the beloved viewer → editing is a separate, opt-in mode; the parser
  upgrade is covered by the existing + new parser tests.

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

### 8.2 The oracle problem — why the hand-authored suite is the spine — **OPEN**

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

## 9. Decision log

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
