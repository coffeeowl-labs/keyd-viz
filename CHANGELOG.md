# Changelog

All notable changes to keyd-viz are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[1.0.0]: https://github.com/coffeeowl-labs/keyd-viz/releases/tag/v1.0.0
