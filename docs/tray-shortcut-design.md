# v1.2 — Tray icon + global shortcut (design note)

> **Status:** research done 2026-06-06, code not started. Environment-specific facts were
> **verified live** on the dev machine (KDE Plasma 6.6.5, Wayland, Arch). Crate versions are
> current as of 2026-06 — re-check on implementation.

**Goal:** keyd-viz lives in the system tray; a global hotkey (and the tray icon) summon/dismiss
the window — pairing with the compact pinnable overlay shipped in v1.1. The tray tooltip/icon can
reflect the active keyboard layer.

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

## Global shortcut — DECIDED: XDG GlobalShortcuts portal via `ashpd`

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

## Implementation plan & files

Build **tray first** (more self-contained, and it's the path that can actually focus), then the
global shortcut.

- `crates/app/Cargo.toml` — add `ksni` (blocking) and `ashpd` (global_shortcuts) + a minimal tokio
  for the portal thread.
- `crates/app/src/tray.rs` (new) — the `ksni::Tray` impl (id/icon/title/tool_tip/activate/menu);
  Show-Hide + Quit menu items; callbacks → `invoke_from_event_loop`.
- `crates/app/src/hotkey.rs` (new) — the ashpd portal session on a dedicated runtime thread;
  `Activated` → `invoke_from_event_loop(toggle)`.
- `crates/app/src/main.rs` — spawn both in `main()` after `MainWindow::new()`, keep handles alive;
  a shared `toggle_window(token: Option<String>)` UI-thread fn using
  `win.window().with_winit_window(|w| { is_visible? set_visible(false) : set_visible(true) +
  request_user_attention })`; push layer changes to the tray at the existing `set_active_layer` site.
- Window show/hide is reliable via winit `set_visible`; raise is best-effort per the above.

## Open decisions for implementation
- Given the hotkey can't reliably raise (no token yet), is the global shortcut still worth shipping
  in v1.2, or lead with tray-only (which *can* focus) and add the shortcut as a follow-up? (Leaning:
  ship both — the shortcut still toggles usefully, and it's future-proofed for when the portal gains
  tokens.)
- Tray icon artwork (bundle a pixmap vs. theme `input-keyboard`).
- Does the global-shortcut UX need an in-app "assign a hotkey in System Settings" hint card?

## Verified-correct facts (don't re-research)
- Portal GlobalShortcuts v1 live (xdg-desktop-portal-kde 6.6.5, KWin 6.6.5); ashpd 0.13.11 has the
  module; ksni 0.3.4; `global-hotkey` is X11-only; `tray-icon` needs a GTK loop.
- winit native-Wayland `set_window_level` is a no-op; `set_visible` is not.
- Tray `Activate` carries a token via `XDG_ACTIVATION_TOKEN` env; GlobalShortcuts `Activated` does
  not (yet).
