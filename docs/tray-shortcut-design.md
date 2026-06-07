# v1.2 — Tray icon (design note)

> **Status:** tray **shipped** (in `crates/app/src/tray.rs`, wired into `main.rs`); see the
> CHANGELOG `[Unreleased]` entry. The **global hotkey was dropped** (rationale below). The
> global-shortcut section is kept as a research record, not a plan. Environment-specific facts
> were **verified live** on the dev machine (KDE Plasma 6.6.5, Wayland, Arch). Crate versions
> are current as of 2026-06.

**Goal (shipped):** keyd-viz lives in the system tray; clicking the icon (or its Show/Hide menu
item) summons/dismisses the window — pairing with the compact pinnable overlay shipped in v1.1.
The tray tooltip reflects the active keyboard layer.

**Why hotkey dropped (user decision 2026-06-06):** on Wayland an app can't grab a global hotkey —
KDE registers the *action* but typically leaves it **unassigned** until the user binds it in System
Settings, and even then `Activated` carries no activation token so it often can't raise the window.
That's patchy backend support plus a "user must go bind a key" friction step, for a path strictly
weaker than the tray (which *can* focus, via the env token). Not worth a second code path. The tray
alone delivers the summon/dismiss feature portably.

---

## The platform reality (this drives the whole design)

The dev target is **KDE Plasma 6.6 on Wayland**. Two hard constraints, both the same family as the
v1.1 pin/always-on-top no-op (`crates/app/src/main.rs` `set_window_on_top`, Wayland `set_window_level`
is empty):

1. **An app cannot grab a global hotkey on Wayland** — the compositor owns the keyboard. You
   register an *action* with the compositor; you do **not** pick the key.
2. **An app cannot focus/raise itself on Wayland** without a valid **xdg-activation token**. You can
   always `show()`/`hide()` the toplevel, but raise-to-front-and-focus depends on a token the
   compositor mints. This is the same wall as the pin no-op — do **not** advertise always-on-top.

---

## Tray icon — DECIDED: `ksni`

**Crate:** `ksni = { version = "0.3", default-features = false, features = ["blocking"] }`
(latest 0.3.4; MSRV 1.80).

- Pure-Rust **StatusNotifierItem** over D-Bus — exactly the protocol KDE's plasmashell consumes.
  **No GTK, no C deps, no tokio.** Confirmed installed stack: `dbus`, `kstatusnotifieritem`,
  `knotifications` all present; nothing new to install.
- **Rejected `tray-icon`** (Tauri): its Linux backend is GTK3 + libappindicator and **requires a
  GTK event loop on the tray-creating thread** — collides with Slint's winit loop on Wayland, drags
  in GTK, and is the legacy path (Tauri itself filed tauri#11293 "use ksni"). Avoid.
- **Wiring:** `blocking` feature → `tray.spawn()` runs ksni's D-Bus service on **its own thread**.
  Callbacks (`Tray::activate`, menu `StandardItem::activate`) fire on ksni's thread → must hop to
  the UI thread via `slint::invoke_from_event_loop` (the exact pattern keyd-viz already uses for the
  listen/monitor threads). Keep the returned `Handle`/guard alive for the app's lifetime (bind it in
  `main`, like `_glow_timer`/`_reload_timer`).
- **Reflect the active layer:** store layer in the tray struct; on layer change call
  `Handle::update(|t| t.layer = …)` to re-emit `tool_tip()`. `Handle` is `Send + Clone`. `update`
  does D-Bus I/O — keep it off the UI hot path (forwarder thread / `mpsc` channel), don't call inline
  on the UI thread.
- **Icon:** `icon_name()` (XDG theme name, e.g. `input-keyboard`) or `icon_pixmap()` (raw ARGB32
  from memory — bundle our own).

**Tray window-raise is the *good* case:** a tray-item click is one of the legacy SNI calls KDE
retrofitted with activation support — plasmashell sets **`XDG_ACTIVATION_TOKEN` in our env** just
before invoking `Activate`. ksni 0.3.4's `activate(&mut self, x, y)` does **not** surface the token,
so read `std::env::var("XDG_ACTIVATION_TOKEN")` inside the callback. With that token present, KWin
legitimately allows raise — so **tray-summon can focus the window**, unlike a self-initiated raise.

---

## Global shortcut — DROPPED (research record only; see "Why hotkey dropped" above)

> The design below was the planned approach before the hotkey was cut. Kept so a future
> revisit doesn't re-research it from scratch.

### Approach that *would* have been used: XDG GlobalShortcuts portal via `ashpd`

**Crate:** `ashpd = { version = "0.13", features = ["global_shortcuts"] }` (latest 0.13.11; the
`global_shortcuts` module is feature-gated). `zbus 5.x` only if we ever drop to raw DBus.

- **Verified live on the machine:** `org.freedesktop.portal.GlobalShortcuts` **version 1** is on the
  session bus, served by `xdg-desktop-portal-kde 6.6.5`; `org.kde.kglobalaccel` is owned by
  `kwin_wayland` (KGlobalAccelD is embedded in KWin on Wayland, and the portal just forwards to it).
- **Rejected `global-hotkey`** crate (Tauri): **X11-only** (XGrabKey) — does not work on native
  Wayland. **Rejected raw KGlobalAccel**: KDE-private, unstable DBus, non-portable; its only edge is
  forcing a default key (see below) — not worth it.
- **Call sequence** (on a dedicated tokio `current_thread` runtime thread; marshal back via
  `slint::invoke_from_event_loop`): `GlobalShortcuts::new()` → `create_session()` →
  `bind_shortcuts(&session, &[NewShortcut::new("toggle-viz","Show/hide keyd-viz")
  .preferred_trigger("LOGO+k")], None, …)` → `receive_activated()` stream → on event, toggle the
  window. **Keep the `Session` alive** for the app's lifetime (drop = unbind).

### Critical UX consequence: the user binds the key, not us
On Wayland/KDE, `preferred_trigger` is **only a hint the compositor may ignore** — KDE registers the
action but typically leaves it **unassigned** until the user sets the combo in **System Settings →
Keyboard → Shortcuts → keyd-viz**. So the feature is "register the action + show a one-time hint
telling the user to assign a hotkey," **not** "app silently owns Meta+K." Design the UX around that.

### Critical limitation: shortcut-summon may show-but-not-raise
The `Activated` signal is *spec'd* to optionally carry an `activation_token`, but the GlobalShortcuts
portal **does not currently deliver one** (known gap — flatpak/xdg-desktop-portal#1678; Yakuake is the
reference victim). So when the hotkey fires: `show()` works, but **raise/focus is not guaranteed**
(degrades to "shown, maybe focused" on Balanced focus-stealing-prevention; "shown, not raised" on
Extreme). For a *toggle*, this is mostly fine (hide-on-second-press always works). Read
`ev.activation_token()` opportunistically so we're correct the moment KDE starts supplying it; fall
back to `request_user_attention` (taskbar flash). **Don't promise raise-to-front from the hotkey.**

> Net asymmetry worth noting: **tray-click can focus** (token via env), **hotkey often can't yet**
> (no token from the portal). Both can always show/hide.

---

## As shipped (tray only)

- `crates/app/Cargo.toml` — `ksni = { version = "0.3.4", default-features = false, features =
  ["blocking", "async-io"] }`. The `blocking` feature alone won't compile — it needs an executor;
  `async-io` provides one without pulling in tokio.
- `crates/app/src/tray.rs` — the `ksni::Tray` impl (id/icon/title/tool_tip/activate/menu) with
  Show-Hide + Quit items; callbacks hop to the UI via `slint::Weak::upgrade_in_event_loop`. Layer→
  tooltip updates run on a dedicated forwarder thread fed by an `mpsc` channel (because
  `Handle::update` blocks on D-Bus I/O and must not stall the UI hot path); the forwarder coalesces
  bursts and applies only the latest layer.
- `crates/app/src/main.rs` — `toggle_window(&MainWindow)` (UI thread) does
  `with_winit_window(|w| is_visible? set_visible(false) : set_visible(true) + request_user_attention)`;
  `tray::spawn(&win)` is called after setup and the handle is held in a UI-thread `thread_local!` so
  `render_board` can push the active layer at the existing `set_active_layer` site.
- Window show/hide is reliable via winit `set_visible`; raise is best-effort (winit 0.30 has no API
  to consume the `XDG_ACTIVATION_TOKEN` the tray click supplies, so we fall back to the attention
  flash).

## Possible follow-ups (not in v1.2)
- Tray icon artwork (bundle an ARGB32 pixmap vs. the current theme `input-keyboard`).
- Revisit the global shortcut if/when the GlobalShortcuts portal starts delivering activation
  tokens (so it could actually raise) — until then the tray is the strictly better path.

## Verified-correct facts (don't re-research)
- Portal GlobalShortcuts v1 live (xdg-desktop-portal-kde 6.6.5, KWin 6.6.5); ashpd 0.13.11 has the
  module; ksni 0.3.4; `global-hotkey` is X11-only; `tray-icon` needs a GTK loop.
- winit native-Wayland `set_window_level` is a no-op; `set_visible` is not.
- Tray `Activate` carries a token via `XDG_ACTIVATION_TOKEN` env; GlobalShortcuts `Activated` does
  not (yet).
