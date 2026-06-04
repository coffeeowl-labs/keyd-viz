# Zero-permission live access — design options (DRAFT for review)

> Status: **draft, no code written.** This exists so the "how do we deliver live
> view with *zero* manual permission steps?" decision (ROADMAP §1 hard requirement,
> §8 open question) is a review rather than a blank page. Pick a direction and we
> implement. Nothing here is locked in.

## The requirement, restated

From ROADMAP §1: *"user/permission gymnastics must be automated for the end result.
We don't want the user to have to worry about adding their user to a group."* So after
a normal package install, a normal desktop user must get **both** live signals with no
`usermod`, no re-login, no manual file edits:

- **Layer stream** — `keyd listen`, via `/run/keyd.socket` (`root:keyd`, `0660`).
- **Keypress stream** — `keyd monitor`, via `/dev/input/event*` (`root:input`, `0660`).

Today the app shells out to both. That works **only** if the user is already in `keyd`
(layers) and `input` (keys). On this dev box `ryan` is in `input` but not `keyd`, so
keypresses work and layers don't — exactly the gap to close.

## What we already have

The app's `layer` and `monitor` modules consume an **event stream** behind a plain
callback (`run_listen` / `run_monitor`). They don't care whether the bytes come from a
spawned `keyd` process or a socket — so whichever option below we pick, the UI side is a
localized swap (point the stream at the helper/socket instead of `Command::new("keyd")`).

---

## Option A — Privileged helper daemon  *(ROADMAP's assumed path)*

A tiny root systemd service owns all keyd access and re-exposes **only** a one-directional
event stream (layers + keypresses, events out, no control in) on a unix socket the active
desktop user can read.

**Granting the right user, no group:** the standard trick is `logind`/`uaccess`-style
session awareness:
- Helper listens on `/run/keyd-viz.sock`.
- On connect, it reads the peer's uid via `SO_PEERCRED` and asks `logind` (sd-bus
  `GetSessionByPID` / `session.Active` / `session.User`) whether that uid owns the
  **active graphical session**. Only then does it stream. No group, no world-readable
  socket, GUI never privileged.
- Ships as: a `sysusers.d` entry (if any), a `systemd` unit, and a socket unit (optional
  socket-activation). Package enables it. Done at install — user does nothing.

**Pros:** robust for multi-user / fast-user-switching; works regardless of distro group
naming; smallest attack surface if the protocol stays events-out-only; the long-term
"correct" answer and the only one that cleanly covers *keypresses* (see Option B's grab
problem).
**Cons:** most code (daemon + IPC + packaging); a root service to review and maintain.

**v1 wire protocol (sketch — newline-delimited JSON, one event per line):**
```
{"t":"layer","action":"on","name":"nav"}
{"t":"layer","action":"off","name":"nav"}
{"t":"key","devid":"04fe:0021","key":"a","action":"down"}
{"t":"device","action":"added","devid":"04fe:0021","name":"PFU HHKB"}
{"t":"hello","keyd":"2.6.0"}      // sent once on connect
```
This is just the union of what `layer::LayerEvent` and `monitor::MonitorEvent` already
model, so `core`/`app` parsing is reusable. JSON now for debuggability; can swap to bincode
later behind the same enum. **Events out only** — the GUI can never command the daemon.

---

## Option B — Packaged `udev` / ACL rule, no daemon

Grant the active session access to the *resources* directly, the way logind already does
for `/dev/dri`, webcams, etc.

- **Keypresses:** a shipped udev rule `TAG+="uaccess"` on input devices makes `logind` put
  an ACL on `/dev/input/event*` for the active-session user — no `input` group needed.
- **Layers:** the socket is created by keyd at runtime, not a device, so `uaccess` doesn't
  apply cleanly; would need a `tmpfiles.d`/drop-in granting the session an ACL, or keyd
  packaging cooperation.

**Pros:** no daemon at all; tiny; uses the same mechanism the desktop already trusts.
**Cons / open risks:**
1. **The grab problem.** keyd holds `EVIOCGRAB` on managed keyboards, so even *with* access
   a second reader may see nothing from the physical node (only keyd's virtual output). If
   true, `keyd monitor` is the only thing that can surface managed-keyboard presses and an
   ACL doesn't help us read them ourselves — this option may not work for keypresses at all.
   **(Same uncertainty as ROADMAP §4.2's physical-vs-virtual flag — resolve that first; it
   gates this option.)**
2. **Breadth/security:** `uaccess` on *all* input devices is effectively keylogger-grade
   access for the session — broad, and reviewers will balk.
3. Socket ACL story is distro/packaging-dependent.

---

## Option C — Auto group-membership at install

Package post-install adds the installing user to `keyd` + `input` (sysusers / `.install`).

**Pros:** trivial; no runtime code.
**Cons:** needs a **re-login** to take effect (violates "no gymnastics" in spirit); only
covers the one user the installer picked; doesn't generalize to multi-user. Fails the
requirement as written. Useful only as a documented fallback.

---

## Recommendation

**Option A (helper daemon)** as the shipped mechanism — it's the only one that robustly
covers keypresses given keyd's grab, needs no group/re-login, and works multi-user. Keep
the protocol **events-out-only** and **JSON v1** for reviewability.

**But gate the decision on one experiment first:** resolve ROADMAP §4.2's physical-vs-virtual
device-id question on real hardware (press a key, watch for the glow). It determines:
- whether keypresses are even brokerable as we assume (affects A's protocol and B entirely);
- whether follow-by-keypress works at all, or needs the daemon's internal `active_kbd`
  (which `keyd listen` doesn't currently expose — possible upstream ask, §8).

If keypresses turn out to be impractical to broker cleanly, a strong fallback is **A for
layers only** (the easy, low-privilege win) + keep keypresses on the existing `input`-group
path, documented — still a big step up from today and fully covers the single-keyboard north
star.

## Smallest next step if we proceed with A

1. Define the event enum + JSON (de)serialization in `core` (shared by helper and app).
2. Helper binary: open `keyd listen` + `keyd monitor`, fan their parsed events to connected
   clients; `SO_PEERCRED` + logind active-session check for authz.
3. systemd unit + socket; package wiring.
4. App: add a "helper socket" event source behind the existing `run_listen`/`run_monitor`
   seam; fall back to direct `keyd` spawning when the helper isn't present (dev mode).
