# Changelog

All notable changes to keyd-viz are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/coffeeowl-labs/keyd-viz/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/coffeeowl-labs/keyd-viz/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/coffeeowl-labs/keyd-viz/releases/tag/v1.0.0
