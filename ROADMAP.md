# keyd-viz — Roadmap & Design Record

> **Status:** Planning complete, pre-implementation.
> **Purpose of this document:** the single durable source of truth for this project's
> direction, decisions, rationale, and the verified technical facts behind them. It is
> written to survive context loss — if you are picking this up cold, read this top to
> bottom and you will have everything.

> **Project name:** working title `keyd-viz` (current repo dir is `keyd-cheatsheet`).
> Final name TBD at launch — candidates: `keyd-viz`, `keyd-board`, `keyflow`.

---

## 1. The one-paragraph vision (North Star)

The **visual face keyd never had**: a lightweight, native, *beautiful* Linux app that shows
**your active keyboard's real layout**, with **live layer state** and **live keypress
highlighting** — for any user's keyd config and any physical keyboard. It is the first
visual tool of its kind for keyd, and the first Linux-native live overlay for a *software*
key remapper. It replaces the current static HTML cheatsheet entirely.

### The live UX model (refined north star — the end state to build toward)

The endgame is **a single, live, morphing keyboard** (think ZSA Keymapp / OverKeys), **not**
a stacked cheatsheet of every layer. Concretely:

1. **One keymap on screen at a time** — even with multiple keyboards plugged in, exactly one
   board is shown.
2. **The shown board follows the keyboard you last pressed a key on.** Type on a different
   keyboard and the view swaps to *that* keyboard's map. (Requires the active-device signal
   from `keyd monitor`'s device column — Phase 4 / privileged helper.)
3. **Layer changes *replace* the shown board**, they do not light up an additional one. The
   single board morphs to always show the currently-active layer (base when nothing is held).

**Implication for the current build:** the stacked all-layers view shipped in Phases 0–3 is a
*reference/cheatsheet mode* and a stepping stone — useful for printing/learning, but it is
**not** the north-star live view. The live view is a single board driven by *active keyboard ×
active layer*. The two can coexist as modes (live vs. cheatsheet), or the cheatsheet becomes a
secondary view; decide when we build the live single-board mode.

**Phase mapping of this model:**
- "Single board that *replaces* on layer change" → a UI mode buildable **now** on top of
  Phase 3's live layer stream (no new privilege). Candidate next increment ("Phase 3.5").
- "Follows the last-pressed keyboard" + "one map even with several keyboards" → needs the
  active-device signal from `keyd monitor` → **Phase 4** (privileged helper).

### Hard requirements (non-negotiable)
- **Zero manual permission setup.** The shipped product must deliver *full* functionality
  right after install — **no `usermod`, no manual group membership, no re-login, no fiddling.**
  All privileged access (the keyd layer socket *and* `/dev/input`) is brokered by the installed
  helper service; the GUI stays unprivileged and "just works." Any manual group step (e.g. the
  `keyd` group used to test Phase 3 today) is **development-interim only** and must not survive
  into the shipped experience. This is what makes the privileged-helper architecture mandatory,
  not optional.
- **No browser dependency.** Must not require a browser to be installed or running. No
  bundled Chromium. Low idle RAM (it may run resident in the tray all day).
- **Modern, beautiful UI.** Must look modern, never dated. Porting the current cheatsheet's
  look is the *floor*, not the ceiling. A visual downgrade is a project failure.
- **Runs on any Linux distro**, not tied to one flavor.
- keyd is Linux-only (evdev/uinput, kernel input layer) — so cross-OS (Win/Mac) is explicitly
  **not** a goal. Cross-*distro* portability is.

### Guiding principle from the user
> "Scale of work should not be a factor, just end result (perf, security, UI/UX). If we can
> make something completely new that performs better, looks better and is easier to use, it
> shouldn't matter how much work is involved."

Every decision below optimizes **end result**, not effort.

---

## 2. Locked decisions (with rationale)

### 2.1 Stack: **Rust + Slint**
- **Why native, not web:** the constraints (no browser, no background browser process, low
  RAM, runs-all-the-time) rule out Electron (ships Chromium — the exact RAM hog being avoided)
  and Tauri (leans on system `webkit2gtk`, the per-distro fragility we want to avoid, and is
  still web-engine-tied).
- **Why Slint specifically over other native toolkits:**
  - **Beauty:** Slint is a blank-canvas, GPU-rendered, declarative toolkit purpose-built for
    polished modern UIs (rounded rects, gradients, shadows, custom TTF fonts, animation). It
    imposes **no** native-widget look, so there is zero "dated toolkit" risk.
  - **Footprint:** single small binary, single-digit-to-low-tens-of-MB idle RAM,
    GPU-accelerated. Best-in-class for an always-resident tray app.
  - **Portability:** minimal system deps (not tied to GTK/webkit), so it runs cleanly across
    distros. Can even target wasm later if we ever want a web fallback.
- **Why NOT egui (the other lean Rust option):** egui is *lighter* than Slint but has a real
  aesthetic ceiling — immediate-mode, utilitarian "looks like a tool" feel, limited fine
  typography. It would likely be a **beauty downgrade**, which violates a hard requirement.
  So between the two Rust options, the beauty constraint breaks the tie cleanly toward Slint.
- **Honest caveat about Slint:** it does not hand you a gorgeous theme out of the box — the
  polish is *our* design work (colors, spacing, font, motion). That is an effort cost, not a
  ceiling, and effort is not a constraint here. We are also *porting an existing design*, so
  the visual target already exists.
- **Toolkits explicitly rejected for beauty:** Tkinter, stock Qt Widgets, wxWidgets, and any
  default-native-widget toolkit (dated). Flutter and Qt QML were viable on beauty but lose to
  Slint on footprint / cross-OS-no-longer-needed / Rust single-binary portability.

### 2.2 First milestone: **Foundation (Phase 0 + Phase 1)**
Parity + active-keyboard detection. Chosen over "race to the live view" because it replaces
today's tool immediately, proves the "modern, no downgrade" UI claim with a hard gate, and
builds the core that every later feature sits on. The live view is the headline, but it sits
on this base.

### 2.3 Privilege & packaging: **Privileged helper + unprivileged GUI; native packaging**
User delegated this to "whichever is the better end result on perf/security/UX; more work is
not a consideration; Flatpak not important."

**The better end result is the helper model, decisively — and on security grounds:**
- The simple alternative (adding the user to the `input` group so the GUI can read
  `/dev/input`) is a **permanent, session-wide security downgrade**: *every* process the user
  runs gains the ability to read all keystrokes. Ambient keylogger surface for the whole
  session. Unacceptable given security is a stated priority.
- The helper inverts this: one **small, auditable root daemon** brokers keyd's socket +
  `/dev/input`, and exposes only a **narrow, one-directional** "current layer / current key"
  event stream to the GUI over a user-accessible socket. **The GUI never runs privileged.**
  Attack surface shrinks from "entire session" to "one tiny IPC."
- Better UX too: install the package once (systemd service + socket) — works out of the box,
  no manual group fiddling.
- Keeps the GUI Flatpak-able later if ever wanted, so ignoring Flatpak now costs us nothing.
- **The first milestone (P0+1) needs no privilege at all** (it only parses config files and
  reads world-readable `/sys`), so the helper lands exactly when first required.

**The helper brokers ALL keyd access — including the layer stream — so the GUI never needs
group membership (per the zero-manual-permission hard requirement).** Revises the earlier
Phase 3 note: shelling out to `keyd listen` + the `keyd` group is a **dev-only interim**; the
shipped path routes layer events (and keypress events) through the helper.

**Helper socket security model (no groups, no world-writable socket):** the helper runs as a
**root system service** (installed by the package; install-time root is expected — that's not
"user gymnastics"). It reads the keyd socket and `/dev/input` (it has the privilege), and
exposes a **single, one-directional** event stream (layer + key events out only) over a unix
socket that is **restricted to the active session user** — the helper resolves the seat's
logged-in user via logind and `chown`s the socket to them (or checks the peer UID via
`SO_PEERCRED`). This keeps keystroke/layer data off a world-readable socket *and* off a broad
group grant — the GUI connects with zero setup, and no other local process can read the stream.

- **Packaging:** native-first — **AUR + AppImage**. The package installs the helper as a
  systemd service and wires everything up so the user does nothing. Flatpak deprioritized /
  optional later (sandbox fights `/dev/input`; the helper sidesteps it for native packages).

### 2.4 Smaller decisions (assumed unless overridden)
- **Sunset the Python tool.** The parser is reimplemented in Rust; the Python version retires
  once Phase 0 reaches parity.
- **Open-source from day one**, developed in the open in the `coffeeowl-labs` org (the
  "benefit the world" goal).
- **Rename at launch** (Phase 5), not urgent now.

---

## 3. Prior art / positioning (why this is worth building)

**Bottom line from research: there is no mature graphical tool for keyd.** A Rust+Slint app
with per-user layout viz + active-keyboard detection + live key/layer display is **genuinely
novel for keyd** — first of its kind. The *components* are proven elsewhere (so it is
novel-for-keyd, not novel-in-the-world), which de-risks them.

### The entire existing keyd visual field (all marginal)
| Project | What it is | Status |
|---|---|---|
| `keyd-indicator` (didmar) | GNOME tray dot showing active layers; parses `keyd listen` | **Dead PoC**, 1 commit, AI-generated |
| `keyd-cheatsheet` (coffeeowl-labs) | **This repo.** Static per-layer HTML cheatsheet | The closest existing "visual cheatsheet for keyd" — static only |
| `UrOwnKeyboard` (Oriesu) | XKB-first layout manager; keyd is a *secondary* option | Active, but XKB-centric, no live display, no Wayland |
| `keyd++` (keyd-cpp) | C++ fork of the daemon — **not** a GUI | Active (daemon only) |

There is **no AUR GUI package** for keyd. keyd itself ships only text config + `keyd monitor`
+ the `keyd-application-mapper` script.

### Adjacent tools to learn from (mine, don't fear)
- **keymap-drawer** (caksoylar) — best static visualizer in the hobby; SVG output; renders
  hold-taps and combos; **decouples physical layout from keymap via QMK `info.json` / KLE**.
  This is the model for our Phase 2 physical-layout engine, and the static-quality bar.
- **OverKeys** (conventoangelo) — live on-screen overlay with pressed-key highlight + active-
  layer switching via TCP (QMK/ZMK/kanata). **Windows-only, Flutter.** The live-overlay UX
  template — and proof the pattern works, just not on Linux/keyd.
- **ZSA Keymapp / typ.ing** — gold standard for "live": real layout on screen, re-renders on
  layer switch, keystroke + layer display, usage **heatmap**. Firmware-only. The experience
  to aspire to.
- **VIA** — live Key Tester (highlights pressed keys), all-layers view. **QMK Configurator** —
  canonical visual keymap grid. **KLE** (keyboard-layout-editor.com) — the universal physical-
  layout JSON format everyone reuses.

### The competitive landscape for *software* remappers
- **kanata** — most advanced. Has a **TCP server emitting JSON `{"LayerChange":{"new":...}}`**
  on every layer switch (and on connect), plus `push-msg`. This is *why* kanata has live
  visualizers (OverKeys, nata, kanata-switcher) and keyd does not. keyd's introspection is
  thinner but sufficient (see §4).
- **KMonad** — weakest; no official GUI, no live visualizer; users hack layer display via
  status files + polybar.

**The white space we claim:** (1) per-user layout viz for keyd, (2) active-keyboard detection
(nothing in the keyd ecosystem does this), (3) live keypress + active-layer display, native on
Linux/Wayland. We would be the **first real visual face for keyd**.

---

## 4. Verified keyd runtime facts (the feasibility bedrock)

> All facts below were verified against the keyd source (`rvaiya/keyd` master) and man page.
> File:line refs are to the keyd source tree. **Do not re-research this — it's captured here.**

### 4.1 Live active-layer detection — YES, easy, lowest privilege ✅
- Mechanism: **`keyd listen`**. Connects to the daemon's unix socket, registers as a listener
  (`IPC_LAYER_LISTEN`). Daemon keeps a listener array (`daemon.c`, max **32** listeners) and
  on every layer transition (`on_layer_change()`, `daemon.c`) writes one newline-terminated
  line per change to each listener.
- **Wire format** (`daemon.c`):
  - On connect: snapshot — active layout as `/<name>\n`, then each active non-layout layer as
    `+<name>\n`.
  - Per change: activated `+<layer>\n`; deactivated `-<layer>\n`; layout change `/<layout>\n`.
- For a GUI: either shell out to `keyd listen` and parse stdout, or (cleaner) open the socket
  and speak the IPC directly (`src/ipc.c`, `src/keyd.h`).
- **CAVEAT:** the listen stream emits **layer names only — NOT which keyboard** the change
  happened on. The daemon tracks `active_kbd` internally but does not expose it via listen. For
  multi-keyboard setups, disambiguate the source device via `keyd monitor` (which has a device
  column). For single-keyboard, listen alone is fine.

### 4.2 Live keypress capture — YES, via `keyd monitor`, but privileged ✅⚠️
- **`keyd monitor` does NOT grab devices.** Critical finding: `EVIOCGRAB` / `device_grab()`
  (`device.c`) is called **only** from the daemon path (`daemon.c`), never from `monitor.c`.
  `monitor` reads events **passively**, so it **sees physical key presses** with keyd's keysym
  names — exactly what we need to highlight the physical key.
- **Output format** (`monitor.c`): tab-separated
  `<device name>\t<device id>\t<keyname> down|up` to **stdout**. `-t` adds a `+<n> ms` time
  prefix. Also emits `device added:` / `device removed:` lines.
- The key names are keyd's keysym names (`a`, `leftcontrol`, `capslock`) — the *physical* key
  pressed, pre-remap. Correct abstraction layer for "which physical key is down."
- **Direct evdev `/dev/input/eventX` is BLOCKED** for managed keyboards: keyd holds an
  exclusive `EVIOCGRAB`, so any other reader sees nothing. Do not go this route.
- keyd's **virtual output device** ("keyd virtual keyboard", **vendor `0x0FAC`**,
  `vkbd/uinput.c`; `device.c` sets `is_virtual` when vendor==0x0FAC) is visible to all apps but
  carries *remapped* keys — wrong layer of abstraction. Useful only to *exclude* it.
- **Privilege:** `keyd monitor` needs read access to `/dev/input/event*` → **root or `input`
  group**. This is what the privileged helper exists to encapsulate.

### 4.3 Active-keyboard detection — YES (config→device mapping doable) ✅
- keyd's internal device id format is `"%04x:%04x:%08x"` = **vendor:product:uid**, where uid is
  a hash of capabilities + name (`device.c`), because vendor:product alone is ambiguous.
- **`[ids]` matching** (`config.c` `config_check_match`): **prefix match** (so `vendor:product`
  matches the full id). Prefixes: `k:` keyboards only, `m:` mice only, leading `-` exclude, `*`
  wildcard (keyboards only). Config selection (`daemon.c` `lookup_config_ent`): explicit match
  (return 2) beats wildcard (return 1); wildcard never matches mice/trackpads.
- **To map a connected keyboard → its config from outside keyd:** read vendor:product from
  `/sys/class/input/eventX/device/id/{vendor,product}` or `/proc/bus/input/devices`
  (`I: Vendor=xxxx Product=xxxx`), format `%04x:%04x`, run the same prefix-match against each
  `/etc/keyd/*.conf`'s `[ids]`, honoring `k:`/`m:`/`-`/`*` + "explicit beats wildcard". We can
  replicate keyd's logic exactly. vendor:product is sufficient to find the matching config.
- **"Which keyboard is being typed on right now":** keyd knows internally (`active_kbd`,
  reassigned per keypress) but **does not expose it**. Only external signal is `keyd monitor`'s
  device column — the device on the most recent event line is the active one.

### 4.4 Permissions summary
| Feature | Privilege needed |
|---|---|
| Live layer view (`keyd listen`) | socket-group access only (socket is `/run/keyd.socket`, mode **0660**, root-owned) — lowest |
| Keypress highlight (`keyd monitor`) | root or **`input` group** (`/dev/input` access) |
| Map active keyboard → config | read-only `/sys` or `/proc` — none |
| Physical key geometry | n/a (external data, see §4.5) |

- Socket: `SOCKET_PATH = /run/keyd.socket` (older: `/var/run/keyd.socket`), `chmod 0660`,
  root-owned, plus a `.lock` file. Non-root needs the socket's group.
- **Flatpak implications (why we deprioritized it):** sandbox does not expose `/dev/input`,
  `/dev/uinput`, or the socket by default. Layer-only could work with `--filesystem=
  /run/keyd.socket`; keypress capture needs `--device=input` (broad, flagged). The privileged
  helper sidesteps all of this for native packaging.

### 4.5 keyd config → physical layout — keyd has ZERO geometry (must source externally) ✅
- Confirmed: keyd configs are **purely logical** — `[ids]` + layer sections binding keysym
  **names** to behaviors. There are **no coordinates, rows, units, or geometry** anywhere in
  the config format or source. keyd maps names → Linux keycodes (`src/keys.c`), nothing
  spatial.
- Physical position must come from an **external layout source**, exactly as keymap-drawer
  does. Accepted sources:
  - **QMK `info.json`** — per-key `x`/`y`/`w`/`h` in key units (the de-facto standard).
  - **Ortho/parametrized** specs (rows×cols) for simple grids.
  - Generated from **KLE "Raw Data"**.
- **Implication:** we need a physical-layout layer fully separate from keyd. Ship a library of
  common layouts (60% / TKL / full ANSI+ISO / ortho / split) in QMK/KLE format, let the user
  pick or import, and overlay keyd's parsed bindings + live state onto those positions. The
  glue is **keysym name → physical slot**, defined per layout. keyd cannot help here.

---

## 5. Architecture

Clean separation so each piece is independently testable and the privileged surface is tiny.

```
┌─────────────────────────────────────────────────────────────┐
│  app  (Slint UI, UNPRIVILEGED)                               │
│   - cheatsheet board renderer (ports current look)          │
│   - layer tabs / boards, live highlight overlays            │
│   - tray + global-shortcut summon (Phase 5)                  │
│   - connects to helper's user socket for live events        │
└───────────────▲─────────────────────────────────────────────┘
                │ narrow, one-directional event IPC (events out only)
┌───────────────┴─────────────────────────────────────────────┐
│  helper  (root daemon, MINIMAL — Phase 3+)                   │
│   - brokers keyd `listen` socket  → layer events            │
│   - brokers keyd `monitor`        → keypress events         │
│   - exposes only "current layer / current key" to GUI       │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│  core  (pure Rust lib, no I/O — fully unit-tested)          │
│   - keyd .conf parser (port of keyd_cheatsheet.py)          │
│   - [ids] device matcher (replicates keyd's prefix logic)   │
│   - physical-layout model (QMK info.json / KLE loader)      │
│   - keysym-name → physical-slot mapper                      │
└─────────────────────────────────────────────────────────────┘

  runtime/IO layer (in app for P0-1; migrates behind helper for P3+)
   - /sys + /proc device enumeration (world-readable)
```

**Design rules:**
- `core` is pure logic, no I/O — all OS/privileged concerns live in the runtime layer / helper.
- Helper IPC is **one-directional** (events out, no control in) for v1 — minimizes attack
  surface.
- GUI is **never** privileged.

---

## 6. Phased roadmap

Each phase ships standalone value. Build order is fixed; later phases assume earlier ones.

### Phase 0 — Foundation & visual parity  *(no privilege)*  ← FIRST MILESTONE
1. Rust workspace: `core` (pure logic) + `app` (Slint UI). No helper yet.
2. Port the keyd `.conf` parser from `keyd_cheatsheet.py` into `core`: remaps, tap/hold,
   chords, per-layer overrides. Unit tests mirroring the existing pytest cases.
3. Slint cheatsheet renderer: reproduce the current dark theme, color-coded badges, layer
   boards, TOC. Choose a modern font.
4. Standalone window that opens instantly (the browser-tab problem is solved here).
- **GATE:** side-by-side vs current HTML — confirm **no beauty downgrade** before proceeding.
- **Outcome:** a native standalone app that already replaces today's tool.

### Phase 1 — Active-keyboard detection  *(no privilege)*  ← FIRST MILESTONE
5. Enumerate connected keyboards from `/sys/class/input` (vendor:product) — world-readable.
6. Replicate keyd's `[ids]` prefix-match in `core` (`k:`/`m:`/`-`/`*`, explicit beats
   wildcard) to map each keyboard → its governing `.conf`.
7. UI auto-selects and shows **only the active keyboard's** config (not all files). Manual
   override picker for multi-keyboard users.
- **Outcome:** shows the *right* keyboard's cheatsheet — strictly better than today's tool.

### Phase 2 — Physical-layout engine
- Adopt QMK `info.json` / KLE physical layouts (the keymap-drawer model).
- Ship a starter layout library: 60% / TKL / full-size ANSI + ISO / ortho / split.
- Import/picker UI; map keysym names → physical slots; persist per keyboard.
- **Outcome:** the "any user, any keyboard" feature — what makes this worth releasing widely.

### Phase 3 — Live layer view  *(introduces the helper)*  ← THE HEADLINE
- Build the privileged helper; broker `keyd listen` → layer events to the GUI.
- GUI highlights the active layer and swaps the displayed board in real time.
- **Outcome:** the live "wow" — high impact, low effort (listen is the easy keyd feature).

### Phase 4 — Live keypress view (+ optional heatmap)
- Helper also brokers `keyd monitor` → keypress events.
- GUI highlights pressed keys in real time; optional usage heatmap (à la ZSA Keymapp).
- Hardest phase (privilege + timing). Gated behind the helper from Phase 3.

### Phase 5 — Tray, summon & distribution
- System-tray resident; KDE global-shortcut to summon/dismiss; live config reload on `.conf`
  change.
- Packaging: AUR + AppImage (Flatpak optional, layer-only).
- README / positioning as "the face of keyd"; rename; publish.

**Value checkpoints:** after P0–1 the tool already beats today's. P3 is the cheap headline.
P4 is the ambitious frontier.

---

## 7. Risks being tracked
- **Visual parity (P0)** — make-or-break for the hard "no downgrade" requirement. Explicit
  gate, not an afterthought.
- **Physical-layout data (P2)** — the largest *content* effort. Mitigation: reuse QMK/KLE
  formats rather than inventing one.
- **Helper IPC design (P3)** — the security-critical surface. Keep it minimal and
  one-directional (events out, no control in) for v1.
- **Multi-keyboard disambiguation** — `keyd listen` lacks device identity; combine with
  `keyd monitor`'s device column when more than one keyboard is configured.
- **keyd socket group/permissions vary by distro packaging** — verify `stat /run/keyd.socket`
  on target; the helper (running as root) avoids depending on user group membership.

---

## 8. Open questions / deferred decisions
- **Final project name** (Phase 5): `keyd-viz` / `keyd-board` / `keyflow` / other.
- **IPC mechanism for the helper** (P3): unix socket protocol + serialization (likely a small
  framed JSON or bincode stream). Decide at P3.
- **Slint rendering backend** (femtovg/skia/software) — pick during P0 based on look + deps.
- **Heatmap persistence** (P4) — local data store format/location, opt-in.
- **Upstream opportunity:** propose adding key events to keyd's IPC so external GUIs don't need
  `/dev/input` at all (would eliminate the helper's `monitor` privilege). Long-shot; revisit.

---

## 9. Reference URLs
- keyd: https://github.com/rvaiya/keyd · man page: https://man.archlinux.org/man/extra/keyd/keyd.1.en
- "How keyd works" (grab/uinput/IPC internals): https://serabin1.github.io/blogs/how-keyd-works/
- keyd-indicator (dead PoC): https://github.com/didmar/keyd-indicator
- UrOwnKeyboard: https://github.com/Oriesu/UrOwnKeyboard
- keymap-drawer: https://github.com/caksoylar/keymap-drawer ·
  physical layouts: https://github.com/caksoylar/keymap-drawer/blob/main/PHYSICAL_LAYOUTS.md
- OverKeys: https://github.com/conventoangelo/OverKeys
- kanata: https://github.com/jtroo/kanata · layer-state issue: https://github.com/jtroo/kanata/issues/244
- KMonad layer-display discussion: https://github.com/kmonad/kmonad/discussions/420
- ZSA Keymapp/typ.ing: https://www.zsa.io/training · QMK Configurator: https://config.qmk.fm ·
  VIA: https://caniusevia.com · KLE: http://www.keyboard-layout-editor.com
- QMK info.json reference: https://docs.qmk.fm/reference_configurator_support
- Slint: https://slint.dev

---

## 10. Change log
- *(initial)* Document created. Decisions locked: Rust+Slint stack; first milestone = Phase
  0+1 (foundation + active-keyboard detection); privilege model = privileged helper +
  unprivileged GUI; packaging = native-first (AUR/AppImage). keyd runtime feasibility verified.
- *(Phase 0 — in progress)* Rust workspace scaffolded (`crates/core`, `crates/app`).
  - `core`: keyd parser, value prettifier, physical layouts (HHKB/ANSI-60), and the
    semantic board model (`Sheet`/`Board`/`KeyCap`) ported faithfully from the Python tool.
    15 unit/integration tests green; zero deps; offline-buildable.
  - `app`: Slint UI (`crates/app/ui/app.slint`) reproducing the original dark cheatsheet
    look natively — rounded caps, color-coded hold badges, ghost legends, dim/HOLD states,
    per-layer accents. Renders `/etc/keyd/*.conf` (falls back to bundled examples).
  - **Visual-parity gate MET** (verified via screenshot on KDE/Wayland).
  - **Font: bundled JetBrains Mono** (OFL, vendored in `crates/app/assets/fonts/`),
    registered at runtime via Slint's fontique (`unstable-fontique-08` feature) so typography
    is identical on every machine regardless of installed fonts. Verified resolvable at
    startup. (Slint takes a single `font-family`, so the CSS fallback chain doesn't apply —
    bundling is the robust answer.)
  - **Phase 0 complete.**
- *(Phase 1 — complete)* Active-keyboard detection.
  - `core::ids` — replicates keyd's `[ids]` matching (prefix match, `k:`/`m:`/`-`/`*`,
    explicit-beats-wildcard) as pure, tested logic (`Ids`, `MatchKind`). 5 tests.
  - `app::devices` — enumerates `/proc/bus/input/devices` and classifies keyboards with
    keyd's exact capability rule (all of `KEY_1..KEY_0,KEY_Q..KEY_Y`, or any media key),
    read from the `B: KEY=` bitmap — no privilege. 4 tests.
  - App now detects connected keyboards, assigns each to its best-matching config, and
    renders **only the matching config(s)** (labeled with a green "connected: <device>"
    line), grouping a keyboard's multiple event nodes by `vendor:product`. Falls back to
    all configs if nothing matches, or bundled examples if `/etc/keyd` is empty.
  - Added a `--list` CLI mode (prints detection result, no GUI) for debugging/scripting.
  - Verified on real hardware (HHKB `04fe:0021` → hhkb.conf; laptop `0b05:19b6` →
    laptop.conf). 24 tests total green.
- *(Phase 3 — complete, pending real-hardware confirmation)* Live layer view.
  - `app::layer` — parses the `keyd listen` stream (`+name`/`-name`/`/layout`), tracks the
    active-layer stack, and runs it on a background thread that pushes `LiveState` to the UI
    via `invoke_from_event_loop`. Auto-retries; degrades to "live view off" when the socket
    isn't accessible. 3 tests (parser + state machine).
  - UI: a live-status pill ("● LIVE · active: NAV" / "○ live view off") and a reactive
    highlight (accent border + "● ACTIVE" tag) on whichever board is currently active.
  - `--demo` mode cycles the active layer for testing the highlight without keyd access.
    **Verified visually via --demo** (pill + NAV board highlight render correctly).
  - **Architecture note (revised):** Phase 3 currently uses `keyd listen` directly, gated on
    `keyd`-group membership — but per the **zero-manual-permission hard requirement (§1)**, that
    group step is **dev-interim only**. The shipped path routes the layer stream through the
    privileged helper too (built in Phase 4), so the GUI needs no group. The `layer` module is
    already source-agnostic (it consumes a `LiveState` stream), so swapping `keyd listen` for
    the helper socket is a localized change. (On this dev machine the user is in `input` but not
    `keyd`; to confirm the live view *now*, `sudo usermod -aG keyd <user>` + re-login — but the
    end product will not require this.)
  - 27 tests total green.
- *(Phase 3.5 — complete)* Single-board live mode (north-star UX, §1 "live UX model").
  - In live mode the app shows **exactly one board at a time** — the active keyboard's
    active layer — and the board **morphs in place** as the layer changes (no stacking).
    A bare held mod (e.g. `control`) that keyd reports as a layer but has no dedicated
    board cleanly falls back to the **base** board.
  - `app::layer` `LiveState` now carries the full active-layer **stack** (`active:
    Vec<String>`, most-recent last) instead of a single string; `main::resolve_title`
    walks it (most-recent first) to the topmost layer that actually has a board, and
    `show_layer` points the UI's single `active_board` at it. Both run on the UI thread.
  - UI: the live single board **is the only view** — no view picker. The legacy stacked
    "cheatsheet" (a browser-era artifact) and its Live/Cheatsheet toggle were removed per
    user direction: a live viewer shouldn't ship a mode selector. The window renders the
    **active sheet's** header + one `active_board`; at startup the base board is seeded so
    it's never blank before keyd connects (pill reads "live view off" until then).
  - The active keyboard is exposed as a single `active_sheet` property (the first detected
    sheet for now); Phase 4 will repoint it to whichever keyboard the last keypress came
    from. Board lookups (`main::show_live`) run on the UI thread, since Slint's `Rc`-backed
    models aren't `Send` and can't cross to the listen thread.
  - `--demo` drives the same single board, cycling base → each layer. **Verified visually
    via --demo** last iteration (one board morphing SHIFT→GAME→…).
- *(Phase 4 — in progress: keypress half done; privileged helper shelved)* Live keypresses.
  - **`keyd monitor` format re-verified against keyd v2.6.0** (matches §4.2): key events are
    `"<name>\t<vendor:product:hash>\t<key> <down|up|repeat>"` (binary fmt `%s\t%s\t%s %s`);
    startup/hotplug emit `device added:/removed: <id> <name…> (/dev/input/eventN)`. Captured a
    real event via `ydotool` to confirm. **Confirmed `keyd monitor` runs for a normal user in
    the `input` group** (ryan) — so the keypress half needs *no* extra permission on this box,
    unlike `keyd listen` (which needs the `keyd` group).
  - `app::monitor` — parses both record kinds into `MonitorEvent` (strips the per-device hash to
    `vendor:product`); `run_monitor` mirrors `run_listen` (retry + connect callback). 4 tests.
  - `core::KeyCap` gains `key` (the keyd key name per physical position) so a monitor keypress
    maps straight onto a cap — no evdev keycode table needed (layouts are already keyed by keyd
    names).
  - App: `spawn_monitor` maintains a pressed-key set → **live glow** (brighter fill, cyan ring,
    white label) and **follows the last-pressed keyboard** via a `vendor:product → sheet` map
    built during detection. `render_board()` now centralizes board selection + glow stamping;
    both the listen and monitor streams feed it through window-property state (kept on the UI
    thread, since Slint models aren't `Send`). Status pill gained a **"LIVE keys"** state for
    when monitor works but the `keyd`-group layer socket doesn't. `--demo` sweeps a glowing key
    while cycling layers. 31 tests green.
  - **SHELVED for discussion (needs user direction):** the privileged-helper / socket-security
    design — the mechanism that delivers the §1 *zero-manual-permission* requirement for *all*
    users (not just those already in `input`/`keyd`). Everything above is source-agnostic and
    slots in behind the helper unchanged. See open questions §8 (helper IPC, framing, authz via
    SO_PEERCRED/logind, systemd unit, packaging).
  - Next (for the human): pick the helper design, OR pivot to Phase 2 (physical-layout engine).
