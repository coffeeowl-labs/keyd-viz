# keyd-viz — Roadmap & Design Record

> **Status:** v1.2.0 shipped (2026-06-06) — Phases 0–5 complete. v1.1.0 was a UI/UX
> polish release; **v1.2.0 adds the system-tray resident process** (StatusNotifierItem:
> tray icon, minimize-to-tray, Show/Hide/Quit, active-layer tooltip), bundled with the
> previously-held pin/icon-button/parser fixes. The originally-paired **global hotkey was
> dropped** (Wayland can't grab one and it can't reliably raise; see
> `docs/tray-shortcut-design.md`). **Next up: Phase 6 — edit mode** (`docs/edit-mode-design.md`).
> **Purpose of this document:** the single durable source of truth for this project's
> direction, decisions, rationale, and the verified technical facts behind them. It is
> written to survive context loss — if you are picking this up cold, read this top to
> bottom and you will have everything.

> **Project name:** **`keyd-viz`** (settled 2026-06-04). The GitHub repo is
> `coffeeowl-labs/keyd-viz`; the local checkout directory may still read `keyd-cheatsheet`.

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
- **Confirmed (2026-06-04, runtime):** inspecting `/proc/<pid>/fd` of a live `keyd monitor`
  shows it holding fds on **every `/dev/input/event*` node directly** — no `/run/keyd.socket`,
  no `keyd`-group dependency. So the keypress half works for any user in `input` (verified to
  *spawn* fine as user `ryan`, who is in `input` but not `keyd`).
- **✅ RESOLVED on hardware (2026-06-04):** a press on a keyd-*managed* (grabbed) keyboard
  surfaces in `monitor` under keyd's **virtual** device id (`0fac:0ade`), **not** the physical
  `vendor:product`. (The *key names* are still the physical/pre-remap keysyms — `a`, `leftmeta`
  — which is what the glow needs; only the device id is virtualised.) Confirmed by debug build
  on the user's HHKB: every key event read `devid="0fac:0ade"` while the `device_map` held the
  physical `04fe:0021`, so the old `Ignore`-on-unmapped path dropped every press (no glow).
  Non-grabbed devices (the Logitech mouse `046d:c098`, synthetic ydotool) *do* keep their
  physical id — so the contradiction with the source read is: monitor reports the *originating*
  device, and for grabbed keyboards that originator is keyd's own virtual device.
- **Two consequences, both now handled in `app::monitor::next_press_state`:**
  1. **Glow:** key events whose device id isn't a specific mapped board are attributed to the
     board currently shown (glow by physical key name, no switch) — so the cap lights up. This
     fixes "live keypresses show no change."
  2. **Follow-the-last-pressed-keyboard is *not possible* from the stock keyd IPC** when keyd
     manages the boards: `monitor` aggregates every grabbed keyboard into one virtual device, and
     `listen` only emits `/<layout>` `+<layer>` `-<layer>` with no device id. With a single shown
     board this is moot; with multiple keyboards the view can't auto-switch on typing.
- **But keyd HAS the signal internally** (keyd v2.6.0 `daemon.c`): `active_kbd = ev->dev->data`
  is set on *every* keypress (≈line 514) — literally the last-pressed keyboard — and each device's
  `->data` carries its keyboard + `config.path`. It's just never exposed over IPC. Three paths to
  the §1 north star, in order of cleanliness:
  1. **Upstream keyd patch** — emit the active device id on the `listen` stream (e.g. `@<id>` when
     `active_kbd` changes; ~10 lines in `on_layer_change` + the `EV_DEV_EVENT` handler). The right
     fix since keyd-viz is *the visual face of keyd*; costs a keyd-version dependency.
  2. **Infer from layer names** — distinct per-config layer names in the `listen` stream identify
     the keyboard. Fragile; useless on the base layer.
  3. **Manual keyboard switcher** — tabs/chips to pick the shown board when several keyboards are
     detected; the glow works for whichever is shown. Robust, zero keyd changes, not automatic.
- **Secondary nuance to verify:** `monitor` reads keyd's *output* device, so a *remapped* key may
  glow the output keysym (CapsLock→Esc glows Esc), not the physical key. Unverified; passthrough
  keys are unaffected.

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
│   - tray summon (Phase 5; global-shortcut dropped)          │
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

> **Phases 0–4 shipped in v1.0 (2026-06-04).** Phase 5 is split: distribution + live
> config reload landed in v1.0; the tray-resident process shipped in v1.2.0; the
> originally-paired global-hotkey summon was dropped (Wayland limits — see Phase 5 / the
> tray design note).

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
- **v1.0 (shipped):** live config reload (UI-thread mtime poll, redraws the board on edit);
  helper install scripted (`packaging/install.sh`/`uninstall.sh`); **AUR PKGBUILD**
  (`packaging/aur/`) and an **AppImage** of the GUI (`packaging/appimage/build-appimage.sh`,
  built in CI on tag push via `.github/workflows/release.yml`); README repositioned as "the
  face of keyd"; renamed `keyd-cheatsheet` → `keyd-viz`.
- **v1.1.0 (shipped 2026-06-05):** UI/UX polish — board zoom (scroll + controls), compact
  pinnable mode, auto-fit window, live keyboard hotplug tracking + connected-id highlight,
  chooser-first header redesign, and the fast tap-hold glow fix.
- **v1.2.0 (shipped 2026-06-06):** system-tray resident process (StatusNotifierItem via
  `ksni`): tray icon + Show/Hide + Quit, minimize-to-tray (close hides to the tray),
  tooltip shows the active layer; pairs with the compact mode → pinned overlay. Bundled the
  previously-held pin (X11)/icon-button/parser viewer-bug fixes. The originally-paired
  **global shortcut was dropped** (Wayland can't grab a hotkey and the portal can't raise;
  rationale in `docs/tray-shortcut-design.md`). Flatpak still optional, layer-only.

### Phase 6 — Edit mode  *(visual config authoring — a GUI for `/etc/keyd`)*
Turn keyd-viz from a read-only visualizer into a visual keyd config **editor**: open any real
config, change a binding visually, see it on the board, and save it back without losing
anything. The two cruxes are lossless round-tripping (solved by carrying unmodeled constructs
verbatim + a `serialize(parse(f)) == f` gate, in the MVP) and a privileged-but-safe write path
(a single transient pkexec apply tool — no live socket channel — with a byte-level safety scan
and a dead-man's-switch revert; the panic sequence is the primary failsafe). MVP persists via
draft-then-install before the one-click apply lands. **Full design in
[`docs/edit-mode-design.md`](docs/edit-mode-design.md)** (DRAFT v2, phases E0–E3, security
analysis, testing protocol, decision log) — in review, no code yet.

**Value checkpoints:** after P0–1 the tool already beats today's. P3 is the cheap headline.
P4 is the ambitious frontier. P6 is the category-defining leap (the first keyd GUI editor).

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
  framed JSON or bincode stream). Decide at P3. **Options drafted in
  [`docs/helper-design.md`](docs/helper-design.md)** (helper daemon vs udev/uaccess ACL vs
  auto-group), with a recommendation — review pending.
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
    white label). *(Auto-follow-the-last-pressed-keyboard was attempted here via a
    `vendor:product → sheet` map but **does not work for keyd-managed keyboards** — see the
    2026-06-04 entry below; a manual keyboard switcher replaced it.)* `render_board()` now
    centralizes board selection + glow stamping;
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

- *(Phase 2 — started: geometry engine + QMK importer)* Decouple geometry from identity.
  - **Positioned-geometry model** (`core::geometry`): `Slot { x, y, w, h, r, rx, ry, key: Option<String> }`
    + `Geometry { slots }`. Caps are now **absolutely positioned** in key units (51px pitch =
      46px cap + 5px gap) inside a plain Rectangle, so any geometry — staggered, ISO-enter,
      ortho, split, rotated clusters — renders from `(x, y, w, h, r)` alone. `Board.rows` is gone;
      `KeyCap` carries its own position; `BoardView` sizes to `Geometry::extent()`. Built-in HHKB/
      ANSI60 layouts kept as a compact `from_rows` authoring table (widths encode stagger). Visual
      parity verified on hardware against the prior row-based renderer.
  - **QMK auto-importer** (`core::qmk`) — the crux of Phase 2. QMK `info.json`/`keyboard.json`
    gives **geometry only** (no key identity); identity lives in the board's default keymap, whose
    layer-0 array is **index-aligned** with the `LAYOUT` macro = with the `info.json` layout array.
    So `import()` zips by index: `layout[i]` (geometry) with `layers[0][i]` (a `KC_*` keycode),
    translating each keycode → keyd key name. Unmappable codes (`MO()/LT()/MT()`, `KC_NO`/
    `XXXXXXX`, `KC_TRNS`/`_______`, custom `QK_*`) → `key: None` → dim blank slot, with an
    `unmapped` count surfaced as a "N slot(s) unmapped" hint. Variant selection: keymap's `layout`
    field, else the sole layout, else an error listing the choices (CLI `--qmk-layout` to pick).
    No keymap → conservative fallback to `info.json` human `label`s. `keycode_to_keyd` does
    letters/digits/F-keys algorithmically + a ~70-entry `NAMED` table (every RHS validated against
    `keyd list-keys`); `QK_GESC`/`KC_GESC` resolve to their tap identity (`esc`). serde/serde_json
    are the crate's only deps (both pure Rust — still builds offline, no system libs).
  - **App wiring**: `keydviz <conf?> --qmk-info <info.json> [--qmk-keymap <keymap.json>]
    [--qmk-layout <NAME>]` imports the geometry and renders it as a single board (overlaying the
    `.conf` semantics, or an empty config to show raw keycaps). 14 core tests green.
  - **Verified end-to-end with real QMK data**: fetched DZ60's upstream `keyboard.json` (67-key
    `LAYOUT`) + converted its default `keymap.c` to a Configurator-style `keymap.json` the way
    `qmk c2json` does (paren-balanced `LAYOUT(...)` extraction, depth-aware comma split so
    `MO(1)` survives). Renders a correct staggered 60% ANSI board with real legends; the four
    `XXXXXXX` matrix gaps + `MO(1)` show as dim blanks. keymap-drawer is the architectural
    blueprint (geometry ⊥ identity, joined by index); KLE carries geometry but no identity, so a
    KLE path will still need this label step.
  - Next (Phase 2): board-picker UX, curated layout library, KLE import + manual-label editor.

- *(Phase 2 — curated layout library + in-app picker)* Pick a common layout, no JSON/network.
  - **Decision (with the user): no runtime QMK API, no bundled board dump.** "Common layouts"
    is a small, finite, stable set — so we bake them in. We deliberately *don't* build UX around
    custom/handwired boards (the `--qmk-info` importer stays as the escape hatch for those, and a
    future KLE+editor path covers the truly bespoke). This keeps us self-reliant: no dependency on
    an external API/app that could break us later, and the layouts work fully offline.
  - **`core::catalog`** — seven curated layouts as baked-in `(x, y, w, h, keyd-name)` tables:
    ANSI 60%, ISO 60%, HHKB, 65%, TKL, Full-size (104), Ortho 4×12. Coordinates are transcribed
    from QMK's *canonical* community-layout definitions (`layouts/default/<name>/info.json`),
    zipped with the standard keyd-name sequence per layout — so geometry is exact by construction,
    yet fully self-contained (no runtime JSON). `list()` / `geometry(id)` / `name(id)` /
    `guess(path)`. **This fixes the HHKB bottom-row gap** (the reference insets the bottom row to
    x=1.5 with blocker corners) and adds the tall ISO enter, TKL/Full nav clusters, and the
    numpad (tall `+`/Enter). 6 tests (key counts match QMK, non-zero extent, no exactly-
    overlapping slots, ISO enter is 2u-tall, filename guess). `layout.rs` is now a thin
    `layout_for` shim over the catalog; the old `HHKB`/`ANSI60`/`Row`/`Layout` exports are gone.
  - **In-app layout picker**: a chip row above the board (`for choice in layouts`) morphs the
    active keyboard onto the chosen geometry live — re-runs `Sheet::build` against the new
    `Geometry` and re-stamps the keyd overlay, no restart. The choice is **persisted per config**
    (`app::prefs`, a dependency-free `id<TAB>path` TSV under `$XDG_CONFIG_HOME/keyd-viz/`), so it
    sticks and survives following-the-last-pressed-keyboard. Hidden for QMK-imported boards (fixed
    geometry). `SheetSrc` now retains each parsed `Config` so re-layout needs no re-read; CLI
    `--layout <id>` forces a layout (and feeds screenshot testing).
  - Verified: HHKB renders with the corrected inset bottom row + the picker + the keyd overlay
    (screenshot); TKL/Full nav-cluster and numpad name↔position alignment cross-checked against
    the QMK reference; clippy clean; 20 core tests + app suite green.
  - **SHELVED for good (2026-06-04):** KLE import + a manual-label editor for bespoke/handwired
    boards. Per the user, this only comes back if users actually request it — the curated library
    covers the common case and `--qmk-info` covers the long tail, so the bespoke path isn't worth
    the click-to-edit UI cost on spec. (Was the last open Phase 2 item.)

- *(Phase 4 — keypress correctness + manual keyboard switcher, 2026-06-04)* Made the live glow
  actually correct, and replaced the (impossible) auto-follow with a manual switcher.
  - **Auto-follow-the-last-pressed-keyboard is impossible from stock keyd IPC.** keyd grabs each
    managed keyboard (`EVIOCGRAB`) and re-emits everything through **one virtual device**
    (`0fac:0ade`), so `keyd monitor` reports every grabbed keyboard's presses under that single
    id — the physical source is gone. `listen` carries no device id either. So with multiple
    keyboards the view can't auto-switch on typing. Built a **manual keyboard-switcher** (chip row,
    `on_pick_keyboard`) as the robust stopgap; unmapped-device key events glow on the shown board
    (`monitor::next_press_state`). True auto-follow needs an upstream keyd patch to expose
    `active_kbd` (see §4.2). Recorded in memory `keyd-monitor-virtual-device`.
  - **Glow correctness — caps glow on what keyd *emits*, not the physical key.** `keyd monitor`
    reports the **post-remap output keysym set**, so each cap now carries the keyd key(s) it emits:
    (1) layer/base remaps glow on their target (num `j = 4` glows the j-cap on `4`, not the top-row
    4); (2) names are canonicalised to keyd's **primary** vocabulary (`monitor` prints `=`/`-`/`;`,
    not the config's `equal`/`minus`/`semicolon`); (3) modifier chords/shifted names expand to the
    full set (`C-left` → `leftcontrol+left`, `S-9` → `leftshift+9`) and match by set-containment,
    with a more-specific cap suppressing the plain Ctrl/arrow/digit it subsumes (`resolve_glow`);
    (4) right modifiers fold to their left twin (keyd re-emits `MOD_SHIFT`→leftshift etc.); (5) only
    real keyd keys get a glow key — firmware legends (`lower`/`raise`) and layer names carry none.
    Also fixed HHKB bottom-row Meta/Alt ordering. Recorded in memory `keyd-monitor-primary-keysym`.
  - **Validation without hand-authored layouts:** a committed pure-Rust invariant test
    (`is_primary_keysym`) walks every `examples/*.conf` × every catalog geometry asserting each
    cap's keysym is one `keyd monitor` can print (caught the firmware-legend slots). Plus a
    **one-time differential sweep** drove keyd's offline `test-io` as the oracle across every key on
    every layer — **343 keys confirmed** against the real keyd engine, which is also what surfaced
    the right-modifier fold. Sweep scaffolding was throwaway (not committed); the invariant test is
    the kept regression net.
  - Workspace: 57 tests green, clippy clean.
  - **Still open in Phase 4:** the **privileged helper** (the §1 zero-manual-permission
    requirement) — unchanged from below; everything here is source-agnostic and slots behind it.

- *(Phase 4 — brokering helper daemon, functional core, 2026-06-04)* The §1 zero-permission
  mechanism — built per `docs/helper-design.md` Option A. Decision recorded in memory
  `helper-design-decided` (non-root, sandboxed, layers-default/keypresses-opt-in).
  - **`core::live`** — moved the pure keyd listen/monitor parsers + active-layer reducer out of
    `app` into core so the GUI and daemon share identical parsing, and added **`LiveEvent`**: the
    one-JSON-object-per-line, **events-out-only** wire protocol (`hello`/`layer`/`key`/`device`)
    with `From` conversions + `as_layer`/`as_monitor` accessors. `app::layer`/`monitor` keep only
    the spawn loops + the UI-side `next_press_state`.
  - **`crates/helper` (`keydviz-helperd`)** — reads keyd's listen/monitor streams, converts to
    `LiveEvent`, and fans out to clients over a unix socket. **Events-out-only**; peer-uid authz via
    `SO_PEERCRED` (serves own uid or `--uid N`); socket `chmod 0600`. **Layers-only by default** (no
    `/dev/input` — not a keylogger); `--keys` opts into keypresses. Tracks a layer snapshot and
    replays it to late-joining clients. `--demo` emits synthetic events for testing without keyd.
    Deps: core + libc only (tiny by design). Verified E2E in `--demo`: client reads hello/layer/key;
    a mismatched-uid client is rejected (0 bytes, logged).
  - **`app::helper`** — the GUI prefers the broker socket when present (or forced via
    `--helper-socket`/`$KEYDVIZ_HELPER_SOCKET`), reading one `LiveEvent` stream for both layers and
    glow; falls back to spawning `keyd` directly when the helper isn't running. Socket path mirrors
    the daemon's default so they meet with no config.
  - Workspace: 62 tests green, clippy clean. **Dev test:** `keydviz-helperd --demo` + `keydviz`
    shows the board morphing + glow driven entirely through the socket, no keyd perms needed.
- *(Phase 4 — brokering helper daemon, productionization, 2026-06-04)* Turned the dev-functional
  core into the shipped zero-permission system service.
  - **logind active-session authz** (`helper::authz`) — a `Policy` of either `Uid(n)` (the dev /
    same-user default) or `ActiveSession`, which serves whatever uid logind reports as the
    **active** (foreground) session user via libsystemd `sd_uid_get_state` (linked through a tiny
    `build.rs`; the stable API over the "do-not-parse" `/run/systemd/users/<uid>` file — no D-Bus,
    no exec). This lets the daemon run as the dedicated `keyd-viz` user yet serve the desktop user
    with no shared group and no hard-coded uid; a user who switched away (`online`, not `active`) is
    denied. Socket mode follows policy: `0600` for `Uid`, `0666` for `ActiveSession` (the per-conn
    check, not the file mode, gates the data). Verified E2E: an `--active-session` daemon serves the
    live uid-1000 session and binds `0666`.
  - **systemd packaging** (`packaging/`) — a hardened `keydviz-helperd.service` (`User=keyd-viz`,
    `RuntimeDirectory=keyd-viz` → socket at `/run/keyd-viz/keyd-viz.sock`, `PrivateNetwork`,
    `RestrictAddressFamilies=AF_UNIX`, `ProtectSystem=strict`, dropped caps, `DevicePolicy=closed`,
    `SystemCallFilter=@system-service`), `sysusers.d/keyd-viz.conf`, and a **layers-only base** that
    grants only the `keyd` group + zero `/dev/input`; keypress glow is an explicit opt-in drop-in
    (`keypresses.conf` adds the `input` group + `DeviceAllow=char-input r` + `--keys`). Unit verified
    with `systemd-analyze verify`; install/uninstall steps in `packaging/README.md`. `app::helper`
    now auto-discovers the system socket (`/run/keyd-viz/keyd-viz.sock`), preferring a running
    per-user dev socket when present.
  - **Still open:** (1) read keyd's socket/virtual-evdev **directly** to drop the `keyd` exec — which
    then unlocks the `~@exec` / no-new-process tier of the sandbox; (2) AUR/AppImage packaging that
    bundles install + enable. The service is now installable and is the shipped zero-permission path.
- *(Phase 4 — drop the `keyd listen` exec, 2026-06-04)* The daemon now follows layers by reading
  **keyd's control socket directly** instead of spawning `keyd listen`. `helper::keyd_ipc` connects to
  `/var/run/keyd.socket`, writes keyd's 4112-byte `struct ipc_message` with `type=IPC_LAYER_LISTEN`
  (verified against keyd v2.6.0 source + live), then reads the one-way `/`,`+`,`-` text stream that
  `parse_listen_line` already handles — so it's a drop-in for `run_keyd_source(&["listen"])` with our
  own reconnect loop and no child process. Needs no new permission (the `keyd` group it already has).
  Verified E2E against a fake keyd socket: correct subscribe bytes, snapshot + layer on/off broadcast
  to the GUI. **Layers no longer exec anything**; only `keyd monitor` (keypresses, evdev) and the
  `keyd --version` hello string remain before the layers-only service can take the `~@exec` sandbox
  tier. `--keyd-socket PATH` overrides the path. Keypress glow (`keyd monitor`) is unchanged.
- *(Phase 4 — drop the `keyd monitor` exec, read evdev directly, 2026-06-04)* Keypresses now come from
  reading keyd's **virtual keyboard via evdev directly** instead of spawning `keyd monitor`.
  `helper::evdev` finds keyd's uinput keyboard (`0fac:0ade`) by `EVIOCGID`, reads 24-byte
  `input_event` records, maps each `EV_KEY` keycode through the new `core::keycodes::keycode_name`
  (keyd v2.6.0 `keycode_table` transcribed; indexed by raw kernel keycode) and broadcasts down/up —
  re-finding the device on keyd restart. Needs the same `/dev/input` access `keyd monitor` did (the
  `input` group), just no child process and no dependency on the `keyd` binary. Verified live against
  the real keyd virtual keyboard: 64 events, post-remap outputs correct (home-row → digits/arrows on
  the num/nav layers, `leftcontrol`+`c` chord intact) — identical to `keyd monitor`. **The daemon now
  spawns no `keyd` children at all**; only the cosmetic `keyd --version` hello exec remains before the
  full service can take the `~@exec`/no-new-process sandbox tier.
- *(Phase 4 — exec-free daemon + no-exec sandbox tier, 2026-06-04)* Dropped the last exec (`keyd
  --version` for the hello string; the GUI never used it, and keyd's presence is implied by the layer
  stream). The daemon now execs **nothing** in any mode, so the unit denies it outright:
  `SystemCallFilter=~execve execveat` + `MemoryDenyWriteExecute=yes` on the base service — a
  code-exec foothold can't spawn a shell or map writable+executable memory, on top of the existing
  no-network / read-only-FS / dropped-caps cage. The keypresses drop-in tightened to
  `DeviceAllow=char-input r` (our evdev reader opens read-only, unlike `keyd monitor`'s O_RDWR). The
  service no longer depends on the `keyd` binary at runtime at all (it talks to keyd's socket + evdev
  device directly). Unit re-verified with `systemd-analyze verify`; needs a reinstall + restart on the
  target to confirm the daemon runs clean under the tightened seccomp. **This completes the helper's
  security hardening** — remaining work is just AUR/AppImage packaging.
- *(Phase 6 E0 — line-faithful edit model, 2026-06-06)* First Edit Mode code, per
  `docs/edit-mode-design.md` §5.1 (review #2's per-line-verbatim decision). **`core::edit`** —
  `EditConfig`/`Section`/`Entry` store every source line byte-for-byte (raw + per-line EOL:
  LF/CRLF/none — no `str::lines()`) with a typed overlay (`Typed::{Remap, Noop, Raw}`,
  deliberately conservative; grows in E1/E2). `serialize()` replays untouched lines verbatim and
  regenerates only edited ones, so `serialize(parse(f)) == f` is identity-by-construction; the
  `round_trips()` gate is the model-soundness self-check the app runs before entering edit mode.
  Grammar parity re-verified line-by-line against keyd `ini.c` @ `f564288` (`/tmp/keyd-src`):
  `parse_kvp` exactly (leading-`=` key, trailing space/tab run off the key, valueless entries
  kept), header-before-comment precedence (`[#x]` is a header), `[a]b]` → name `a]b`, `[foo`
  without `]` is a *kvp entry*, verbatim special-section names (`[ids ]` ≠ `[ids]`).
  `Section::set_binding` edits the **last** duplicate (keyd's last-wins order) and dirties one
  line. Tests: kvp-parity table, header edge cases, examples + `/etc/keyd` corpus round-trip,
  EOL-fidelity cases, fixed-seed fuzz (500 byte-soups), single-line-diff mutation. 57 core tests
  green, clippy clean. **Still open in E0:** §12 parser-faithfulness fixes (paren-depth `parse_fn`,
  modset classification, viewer-model derivation from `EditConfig`), the privileged apply-tool
  prototype (§5.2–5.4), and the runtime keyd probe.
- *(Phase 6 E0 — runtime probe + privileged apply-tool prototype, 2026-06-06)* The other two E0
  legs. **`app::probe`** (`keydviz --probe`): probes the installed keyd lazily — `--version`, a
  *proven* `keyd check` round (validates a known-good config; fail closed), `list-keys` (315
  names on this box, feeds the E1 picker), socket path. **`crates/apply` (`keydviz-apply`)** —
  the §5.2–§5.4 one-shot privileged tool, prototype complete: stdin protocol
  (`apply <name> <len> [sensitive-ok]` + raw bytes; no caller paths ever), strict name
  allow-list, byte-level safety scan as a verified **superset** of keyd's own detection
  (substring-per-line beats arg-splitting evasion; `include` matched exactly like
  `read_config_file` — raw byte-0, untrimmed; comments can't execute so don't flag),
  dir-fd + `O_NOFOLLOW` + `O_EXCL`-temp + `renameat` write path (symlinks abort, rename doesn't
  follow), `keyd check` on the exact temp bytes, timestamped `stamp.pid` backups, transactional
  write-set (`Existed|Absent`, all-or-nothing revert, MVP passes exactly 1), and the
  **dead-man's switch**: after write+reload the tool polls its private fd for a literal `keep`
  line — timeout/EOF/garbage all revert and reload. Caught in testing: stdin must be **one
  unbuffered fd-0 reader** end-to-end (std's `StdinLock` deadlocks on re-lock *and* could
  buffer the `keep` away from the dead-man's raw poll). Verified E2E unprivileged via
  debug-only `--dev-dir`: EOF→revert, timeout→revert, keep→kept, bad-config→`keyd check`
  refusal with zero debris. 22 crate tests green; deps = libc only. **E0 is complete** except
  the §12 parser-faithfulness fixes; polkit policy + packaging of the apply tool is E2.
- *(Phase 6 E0 — §12 parser-faithfulness fixes, 2026-06-06)* The E0 punch-list, closing E0.
  **One parser:** `parse_text` now *derives* the semantic `Config` from `EditConfig`
  (`parser::derive`, exported) — the viewer and editor share the grammar layer and can't
  drift, and the viewer gains keyd's exact kvp/header handling for free. **Fixed
  divergences** (each verified against keyd 2.6.0 source): ported `parse_fn` (paren-depth +
  backslash-skip + leading-space-only trim + empty-args-dropped + trailing-garbage-discarded)
  so `overload(nav, macro(a, b))` keeps its nested tap; `overloadi(<tap>, <hold-desc>,
  <timeout>)` handled with keyd's real arg order (tap FIRST; keyd rewrites lettermod into
  exactly this shape) — layer-like hold descriptors reduce to a `Hold`, opaque ones fall back
  to verbatim remap; modset-qualified layers (`[caps:C]` → `Layer.mods`) classify holds as
  modifier via post-pass; general chords (`j+k = esc`) land in new `Config.combos` instead of
  remaps keyed by the literal chord string; `EditConfig::diagnostics()` carries the two
  *runtime-verified* validation-parity warnings (entry-before-first-section → keyd rejects the
  whole file, exit 255 with a misleading "missing [ids]" message because `ini_parse_string(s,
  NULL)` returns NULL; no-`[ids]` → parses clean but never matches a keyboard). **Device
  matching is now a capability bitset** (`DeviceFlags` in keyd's `ID_*` bit space) replacing
  `is_keyboard: bool`: single-loop faithful `config_check_match` port (exclude hits reject
  immediately; wrong-type prefix hits keep scanning), the daemon's wildcard rule
  (KEYBOARD && !TRACKPAD), and `app::devices` reads `B: REL=`/`B: ABS=` to populate
  MOUSE/TRACKPAD — combo keyboard+mouse devices now match `m:`/`k:` filters like keyd does.
  Faithful oddities pinned in tests: `k:*` is a *dead entry* (keyd's wildcard check is an
  exact compare), and a `k:` id matches a button-bearing mouse via the KEY bit. Deferred from
  §12 (renderer concerns, not model): composite-layer overlay rendering, `[aliases]`
  placement resolution. Workspace: 146 tests green, clippy clean; viewer re-verified on real
  hardware (`--list`: HHKB + laptop map unchanged). **E0 is complete.**
- *(Phase 6 E0 — code-review fixes on the apply tool, 2026-06-07)* A 7-angle adversarial
  review of the branch surfaced three real defects in the dead-man's-switch path — all in the
  exact GUI-crash scenario the switch exists for — plus three robustness items. Fixed:
  (1) **EPIPE panic bypassed the revert**: `println!("applied …")` panics when the GUI has
  closed the pipe (Rust ignores SIGPIPE), unwinding past `await_keep` with the possibly-lockout
  config still live. Fix is two-layered: `fdio::say()` writes protocol lines best-effort
  (never panics, swallows EPIPE — the reader is gone, the revert matters more), and `Txn` now
  has a **drop backstop** — any un-kept transaction reverts on every exit path including
  panic-unwind (`keep()` consumes the txn to defuse it); pinned by a catch_unwind test.
  (2) **Unbounded reads as root**: the request line + payload now come through
  `fdio::FdReader`, an unbuffered raw-fd reader with a 30 s deadline (poll before every read) —
  a stalled client gets `TimedOut`, not a wedged pkexec process. (3) **Revert failure
  masqueraded as a generic error**: a failed revert now emits a distinct `revert-failed` line
  (exit 4) naming the backup + the panic sequence, and deliberately does NOT reload (that
  would re-assert the config it failed to remove); `reverted`/exit 3 is only ever printed
  when the prior state is actually back. Also: `keyd` is invoked by **absolute path only**
  (`/usr/bin → /usr/local/bin → /usr/sbin`, fail closed if absent — no PATH lookups in a root
  process), `keyd_check_works` now proves the `check` subcommand by validating a known-good
  config instead of trusting `--version`, and CI gained a `rust-apply` job (test + clippy -D
  warnings + a release build so the dev-flag compile-out is exercised). E2E re-verified: the
  old EOF/keep flows unchanged; the F1 reproduction (stdout reader dead at "applied") now
  reverts cleanly; the F2 stall hits the deadline with nothing written. Deferred to E1 (low,
  niche, all latent on real configs): keyd low-byte REL/ABS parity in devices.rs, `[ids]`
  kvp-key vs raw-line, modset-qualifier validation/first-wins parity, collapsing the unused
  `Typed`/`dirty` overlay until the editor consumes it.
- *(Phase 6 E1 — edit a real config, draft-then-install, 2026-06-07)* The first
  user-facing Edit Mode cut, in three layers. **Core editing API** (`core/edit.rs`):
  `Section::get_binding`/`set_or_add_binding` (last-duplicate-wins lookup; in-place value
  rewrite or append after the section's last non-blank entry, preserving the file's EOL
  style and a missing final newline), `target_section_mut` (LAST layer-bearing section of
  a base name, matching keyd's merge), `is_dirty`. **App session** (`app/editing.rs`):
  `EditSession::open` runs the §5.1 gate (unreadable / round-trip-gate / keyd-rejects →
  view-only with a reason); `config()` re-derives through the one shared parser so the
  preview IS the viewer (§5.6); `save_draft` writes `~/.config/keyd-viz/drafts/<name>`,
  returns copy-paste `sudo cp` + `sudo keyd reload` steps, flags a stale source file, and
  runs `keyd check` on the draft when keyd is installed. **Slint UI**: an explicit `edit`
  toggle (viewer untouched by default; gate refusals show a visible banner); while editing,
  caps are click targets with selection highlight, live morphing + follow-the-keyboard
  freeze, a section chooser picks the board, and the panel offers typed entry,
  press-to-capture (consumes the next live key-down — note monitor reports the *emitted*
  keysym), a common-actions chip palette, and save-draft showing verdict + diff + install
  steps. `KeyCap` grew `phys` (the slot's config-LHS name) because `key` is the emitted
  chord — the wrong identity to edit. The config-reload watcher exempts the file being
  edited. Workspace: 150 tests green, clippy clean. **E1 done-when met** pending visual
  review; the searchable palette, tap/hold editor, and one-click pkexec apply are E2.
- *(Phase 6 E2 — one-click apply, 2026-06-07)* First E2 slice: the E0 apply tool is now
  wired end-to-end. **Session accessors** (`serialized()`, `apply_target(dir)` — only
  `<dir>/<name>.conf` with an allow-listed name ever qualifies, `stale_warning()` shared
  with draft save, `keyd_check_bytes`); the app depends on keydviz-apply's dep-free lib
  half so GUI pre-flight and privileged enforcement run the *same* scan code. **Protocol
  engine** (`app::applying`): pkexec by absolute path (matches the policy's `exec.path`),
  typed events, junk-tolerant line parser that stops at the first terminal verdict,
  126/127 pkexec exit mapping (only when no verdict line was seen), and an `ApplyHandle`
  whose `revert()` just drops stdin — cancel and crash are the same EOF-revert path. The
  request write lives on the background thread (64 KiB payload vs 64 KiB pipe while the
  auth dialog blocks the reader). **UI**: pre-flight (size, scan→red confirm bar for
  `command()`/`macro()`, `keyd check`, staleness, diff) all before pkexec is spawned;
  auth → countdown with a mouse-driven KEEP button (the keyboard under test must not be
  required), cosmetic 200 ms timer — only tool verdicts decide outcomes; `kept` re-OPENS
  the session (truthful staleness re-base + the §5.1 gate re-checks our own output);
  `reverted` keeps edits staged; `revert-failed` is loud and verbatim; session-changing
  actions refused mid-flight. **Packaging**: polkit action `io.github.coffeeowl-labs.
  keydviz.apply` with `allow_active=auth_admin` (deliberately not `auth_admin_keep` —
  cached auth would be a time-boxed silent root-write primitive for any same-uid
  process) + `exec.path` annotation; PKGBUILD/install.sh ship tool + policy; AppImage
  stays draft-then-install (decided trade-off). Also fixed in-pass: a latent pid-only
  temp-file race in `probe::check_works` that parallel tests exposed. Debug builds
  honour `KEYDVIZ_APPLY_DEV_DIR` to run the whole flow unprivileged. Workspace: 165
  tests green, clippy clean. **Remaining for E2**: tap/hold editor, searchable palette +
  `list-keys` picker, layers/chords/`[global]`, orphan warnings, create-config flow,
  one-level include closure scan (deferred, design §5.3).
