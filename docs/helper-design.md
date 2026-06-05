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

## Option A — Brokering helper daemon  *(ROADMAP's assumed path — CHOSEN)*

A tiny systemd service owns all keyd access and re-exposes **only** a one-directional
event stream (layers + keypresses, events out, no control in) on a unix socket the active
desktop user can read.

**It must NOT run as root.** It needs exactly two accesses — the keyd socket
(`root:keyd`, 0660 → `keyd` group) and `/dev/input/event*` (`root:input`, 0660 → `input`
group). A **dedicated system user `keyd-viz`** in those two groups has everything it needs,
with no root at any point. A worst-case compromise then yields keystroke-*read* in a confined
process — **not** root, no escalation, no filesystem writes, no module loading. See the
**Security model** section below for the full hardening set; that analysis is why Option A,
done this way, is a security *upgrade* over today's "add your login user to `input`."

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
problem). Confines keystroke access to **one sandboxed, non-root, network-less daemon**
instead of the whole login session (the group approach's keylogger surface).
**Cons:** most code (daemon + IPC + packaging); a system service to review and maintain
(non-root, but still the security-critical component — keep it tiny and audited).

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

## Recommendation — DECIDED: Option A, non-root + sandboxed

**Option A (brokering daemon), run as a dedicated non-root `keyd-viz` user inside a tight
systemd sandbox.** It's the only option that robustly covers keypresses given keyd's grab,
needs no group/re-login for the desktop user, and works multi-user. Protocol stays
**events-out-only** and **JSON v1** for reviewability.

**The gating experiment is now RESOLVED** (during the keypress-glow work, 2026-06-04):
- keyd reports its **virtual** device id for managed keyboards, and keypresses **are**
  brokerable — the glow works (so Option A's keypress protocol is sound; Option B is moot).
- **Follow-by-keypress does NOT work** from stock keyd IPC (all grabbed keyboards aggregate
  into one virtual device). True auto-follow needs the daemon's internal `active_kbd`, which
  `keyd listen` doesn't expose → an **upstream keyd ask** (ROADMAP §8). The manual keyboard
  switcher is the stopgap. This is independent of the helper choice.

**Layers-only is the safe default, keypresses are opt-in.** Live layers need *only* the keyd
socket — **zero `/dev/input` access**, so that mode is literally not a keylogger. Ship
layers-only as the default; the user explicitly opts in to the keypress-glow capability (and
the helper's `input`-device access) when they want it. Security-conscious users get the full
live morphing board with no input-device surface at all.

## Security model (hardening Option A)

The concern: *"a root daemon that can read every keystroke is a keylogger if compromised."*
Correct in spirit — live keypress display inherently requires **something** to read
keystrokes — so the design minimizes both the chance of compromise and the blast radius.

**1. Not root.** Runs as system user `keyd-viz` ∈ {`keyd`, `input`}. A compromise = keystroke
*read* only: no escalation, no writes outside the sandbox, no module loading, no other users.

**2. systemd sandbox (cage a compromise).** The keylogger threat is "read keys *and exfil*";
we cut the exfil and the escalation:
- `PrivateNetwork=yes` + `RestrictAddressFamilies=AF_UNIX` → **cannot open a network socket.**
  A keylogger that can't phone home is largely defanged.
- `DevicePolicy=closed` + `DeviceAllow=/dev/input/event* r` → read-only input, nothing else.
- `ProtectSystem=strict`, `ProtectHome=yes`, `PrivateTmp=yes`, read-only FS → nowhere to stash.
- `NoNewPrivileges=yes`, `CapabilityBoundingSet=` (drop all), `SystemCallFilter=@system-service`
  minus `@exec`/`@privileged` → no spawn, no escalation.
- `MemoryDenyWriteExecute`, `LockPersonality`, `RestrictNamespaces`, `ProtectKernel*`,
  `RestrictRealtime`, `RestrictSUIDSGID`, `UMask=0077`.

**3. Tiny attack surface to *get* compromised.**
- IPC is **events-out-only** — a compromised GUI cannot command the helper.
- The helper parses bytes only from **trusted sources** (keyd / the kernel evdev stream),
  never from the network or an untrusted client. The only thing a client supplies is "I
  connected," authorized via `SO_PEERCRED` + logind active-session check. Near-zero
  untrusted-input surface.
- **Rust** → memory-corruption RCE classes largely eliminated.
- Reads keyd's IPC socket + the virtual evdev device **directly** — it does **not** spawn
  `keyd listen`/`keyd monitor`, so it needs no exec and we can forbid it outright.

**4. Net effect: a security *upgrade*, not a downgrade.** Today's path ("add your login user
to `input`") gives **every process in your session** ambient keylogger access, permanently.
The helper shrinks that to **one small, non-root, network-less, sandboxed daemon** — and
strictly less capability than keyd itself already has (keyd also *injects* via uinput; the
helper has read-only device access).

**Irreducible residual:** you can't show live keystrokes without something able to read them.
The goal is to make that something minimal, unprivileged, contained, and **opt-in** — with
layers-only as a first-class mode for users who want zero input-device surface.

## Cleaning up the dev-interim group grants

While prototyping we relied on the desktop user being in `keyd`/`input`. Once the helper
ships, the desktop user needs **neither** — the `keyd-viz` system user holds those groups.
**Do this cleanup only after the helper is in place and verified**, or it breaks the current
working app. See ROADMAP §10 and memory `dev-interim-group-grants` for the exact revert.

## Smallest next step if we proceed with A — **done (2026-06-04)**

1. ✅ Event enum + JSON in `core` — `core::live::LiveEvent` (events-out-only wire protocol),
   shared by helper and app.
2. ✅ Helper binary (`crates/helper`, `keydviz-helperd`): spawns `keyd listen` (+ `keyd monitor`
   under `--keys`), fans parsed `LiveEvent`s to clients. Authz is `SO_PEERCRED` + a logind
   active-session check (`helper::authz::Policy::ActiveSession` → libsystemd `sd_uid_get_state`),
   with a `Uid(n)` policy for dev/same-user. Socket mode follows policy (0600 / 0666).
3. ✅ systemd unit + packaging (`packaging/`): hardened `keydviz-helperd.service` running as
   `keyd-viz` with the full sandbox, `sysusers.d`, a layers-only base + keypresses opt-in drop-in,
   and install docs in `packaging/README.md`.
4. ✅ App: `app::helper` is the broker event source behind the `run_listen`/`run_monitor` seam,
   auto-discovering `/run/keyd-viz/keyd-viz.sock` (or a per-user dev socket) and falling back to
   direct `keyd` when absent.

**Remaining hardening:** read keyd's control socket + virtual evdev directly so the daemon no
longer spawns `keyd` — that drops the exec and unlocks the `~@exec` / no-new-process sandbox tier
the security model calls for. Then AUR/AppImage packaging.
