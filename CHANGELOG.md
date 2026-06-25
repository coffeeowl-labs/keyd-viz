# Changelog

All notable changes to keyd-viz are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.3.0] - 2026-06-24

### Added

- **Custom key labels.** Rename what any key shows on the board — name a layer key
  ("Nav", "Sym"), spell out a cryptic keysym, or annotate a macro — without changing the
  binding. Labels are stored as keyd-safe `# keyd-viz:` comment lines, so they survive a
  round-trip through keyd untouched and a plain `keyd` install simply ignores them.
- **Edit mode (visual config editing).** An explicit **edit** toggle turns the viewer
  into an editor for the displayed config: click a key, then set its binding by typing
  it, pressing the key you want (**capture**), or searching keyd's full key list
  (**pick…**); watch the board re-render live, then persist. You can also make a key
  **dual-function** (tap/hold) — a hold layer/modifier + a tap key, chosen by an
  outcome-labelled "feel" rather than raw timeouts — **unbind** a key so keyd stops
  remapping it, and get an inline warning when a binding activates a layer the config
  never defines (keyd would reject it). The editor is line-faithful — untouched lines
  round-trip byte-for-byte — and any file it can't reproduce exactly (or that keyd would
  reject) stays view-only rather than risk clobbering it. Persist via **save draft**
  (writes to `~/.config/keyd-viz/drafts/` with copy-paste install steps, a diff, and a
  `keyd check` verdict) — works on every install, including the AppImage.
- **One-click apply with auto-revert (AUR/source installs).** *Apply to /etc/keyd…*
  hands the edited config to `keydviz-apply`, a new one-shot privileged tool invoked
  via polkit (`pkexec`; a password per apply, by design). It validates with
  `keyd check`, writes atomically with a timestamped backup, reloads keyd, and then
  arms a **dead-man's switch**: only clicking **KEEP** within the countdown makes the
  change permanent — timeout, closing the app, or a crash automatically restores the
  previous config and reloads. `command()`/`macro()` configs require an extra explicit
  confirmation first. keyd's panic sequence (Backspace+Escape+Enter) remains the
  always-available failsafe and is surfaced in the UI during the countdown.
- **Richer editing — chords, macros, daemon options, and layers.** Beyond simple
  remaps, edit mode now builds **chords** (press `key1`+`key2` → an action), a
  **structured macro** editor (key / typed-text / chord / delay steps, with repeat),
  and the **`[global]`** daemon options (overload/timeout/etc., with unit-aware fields).
  You can **create, rename, and delete layers**, and **create a fresh config** for an
  unconfigured keyboard or **delete** an existing one — both through the same vetted
  one-click apply path.
- **Discoverable layer actions.** A dedicated **layer** key-mode lets you point a key — or
  a chord — at a layer and pick *how* it activates: **momentary** (active while held),
  **toggle** (latches on/off), or **one-shot** (applies to the next key only), without
  hand-writing `layer()`/`toggle()`/`oneshot()`. Dual-function (tap/hold) keys now also
  surface the home-row-mod "feel", so a hold-for-modifier / tap-for-letter key is set by
  the outcome you want rather than raw timeouts.
- **Backup and restore for applied configs.** Every one-click apply leaves a timestamped
  backup; a restore panel lists them newest-first and rolls any one back through the same
  validate-write-reload-countdown path (so a restore is as safe as an apply).
- **Composite layer overlays in the viewer.** A layer defined as a composite of others
  (`[a+b]`) now renders as an overlay of its constituents, so the board shows what the
  combined layer actually produces.

## [1.2.0] - 2026-06-06

### Added

- **System tray / minimize to tray.** keyd-viz now lives in the system tray
  (StatusNotifierItem over D-Bus, so it works on KDE and any desktop with a
  StatusNotifier host — X11 or Wayland). When a tray is present, **closing the window
  hides it to the tray** instead of quitting — the app keeps running in the background.
  Left-click the icon (or its *Show / hide* menu item) to summon or dismiss the window;
  *Quit* fully exits. The tooltip shows the active keyd layer. Where no tray host exists
  (e.g. vanilla GNOME without an AppIndicator extension) the icon is absent and closing
  the window quits as before. Window show/hide is reliable everywhere; raise-to-front on
  Wayland is best-effort (it needs a compositor activation token).
- **Pin (always-on-top).** A pin toggle in the view controls keeps the window above other
  windows; compact mode pins automatically. Honored on X11/XWayland. On native Wayland a
  client can't keep itself on top (no such protocol — it's the compositor's job), so the
  in-app pin is a no-op there; use the compositor's keep-above instead (KDE: right-click
  titlebar → *More Actions → Keep Above Window*, or a KWin rule for class `keydviz`).

### Changed

- The **pin** and **compact** controls are now icon buttons, each with two states (pin /
  pin-off; compact / expand), tinting green when active.

### Fixed

- **Modifier/composite layer headers now parse.** `[layer:mods]` (e.g. `[nav:C]`) and
  composite `[a+b]` sections were rejected by the config parser, so their bindings were
  silently attributed to the previous section (or dropped). `[layer:mods]` now merges into
  its base layer and `[a+b]` is its own layer, matching keyd's grammar.
- **`#` inside a value is no longer treated as a comment.** keyd only treats `#` as a comment
  at the start of a line; the parser was stripping from the first `#` anywhere, truncating any
  binding whose value contained one.
- **`[global]` and `[aliases]` sections no longer render as bogus layers.** keyd special-cases
  these (daemon options / key aliases); the parser was treating every non-`[ids]` section as a
  layer, so a common `[global]` block (e.g. `overload_tap_timeout`) drew a junk board.

## [1.1.0] - 2026-06-05

### Added

- **Zoom.** Scroll over the board or use the `−`/`+` controls to scale the keyboard
  (0.4×–2.4×); the window auto-fits the scaled board. Click the percentage to reset.
- **Compact mode.** A toggle that hides all chrome, leaving a minimal, pinnable
  keyboard-only window — a corner "what does this layer do" reminder.
- **Auto-fit window.** The window sizes itself to the selected layout and re-fits when
  you change the layout or zoom.
- **Connected-keyboard id highlight.** The config's `[ids]` entries are shown as tags,
  with the one matching a currently-plugged-in keyboard highlighted green.
- **Live hotplug tracking.** Plugging or unplugging a keyboard updates the highlighted
  ids, device label, and follow-keyboard map within ~1.5s, no restart.

### Changed

- **Header redesign.** The keyboard chooser is now the primary top-line navigation; the
  old title/subtitle and a duplicate on-board legend were removed. Config path and `[ids]`
  moved to line 2 alongside the live-status pill.
- Dropped the per-board layer header so activating a layer no longer shifts the board; the
  active layer is already named in the LIVE pill. The pill sits on line 2 so its width
  changing with the layer name doesn't resize the window.

### Fixed

- **Fast tap-hold taps now glow.** An isolated tap-hold *tap* (e.g. the `f` in "fear")
  resolves on release and emits down+up within a single display frame, so it never lit up.
  A min-glow decay (60 ms, anchored to key-down) keeps such sub-frame taps visible without
  adding a tail to normal typing.

## [1.0.0] - 2026-06-04

First public release — the visual face of [keyd](https://github.com/rvaiya/keyd).

### Added

- **Layout rendering.** Parses `/etc/keyd/*.conf` and draws a base board plus one board
  per layer, rendering keyd's tap/hold model distinctly (tap legend on the key, hold
  action as a badge). Handles `lettermod`, `overload*`, momentary/toggle `layer`, chord
  toggles, and plain remaps.
- **Physical-layout catalog.** A curated set of common layouts (ANSI/ISO 60%, 65%, HHKB,
  TKL, Full, Ortho) with an in-app picker, persisted per config. Boards can also be
  imported from QMK with `--qmk-info <info.json>`.
- **Live layer view.** The board follows keyd in real time, switching to the active layer
  as you hold `nav`/`sym`/etc.
- **Live keypress glow.** Keys light up as you press them using keyd's *post-remap*
  output, so layer outputs and chords glow the correct keys.
- **Live config reload.** Editing a `.conf` redraws the board within ~1s, no restart.
- **Multi-keyboard support.** Detects connected devices and shows the matching config,
  with a manual switcher for ambiguous setups.
- **Hardened broker daemon** (`keydviz-helperd`). Streams keyd's live state to the GUI so
  it needs **no special permissions**. Reads keyd's control socket and the virtual evdev
  device directly — fully exec-free. Runs as a dedicated non-root `keyd-viz` user under a
  locked-down systemd sandbox (network-isolated, `DevicePolicy=closed`, dropped caps,
  `SystemCallFilter` minus `execve`, `MemoryDenyWriteExecute`), with logind active-session
  authorization. Layers stream by default; keypress reads are opt-in.
- **Packaging.** `packaging/install.sh` / `uninstall.sh` for the broker service, an AUR
  `PKGBUILD`, and an AppImage of the GUI.

### Known limitations

- The AppImage bundles the **GUI only**. Without the broker installed, the GUI falls back
  to reading keyd directly, which requires membership in the `keyd`/`input` groups. For
  the full zero-permission live experience, install the broker (AUR package, or
  `packaging/install.sh`).

### Deferred to v1.1

- System-tray resident process and a KDE global-hotkey to summon/dismiss the window.
- Flatpak packaging.

[1.3.0]: https://github.com/coffeeowl-labs/keyd-viz/compare/v1.2.0...v1.3.0
[1.2.0]: https://github.com/coffeeowl-labs/keyd-viz/compare/v1.1.0...v1.2.0
[1.1.0]: https://github.com/coffeeowl-labs/keyd-viz/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/coffeeowl-labs/keyd-viz/releases/tag/v1.0.0
