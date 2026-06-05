# Packaging: the `keydviz-helperd` system service

The broker daemon (`keydviz-helperd`) is what gives keyd-viz its "install and forget"
live view with **no per-user permission setup**: it runs once, as a dedicated
unprivileged system user, and streams layer (and optionally keypress) events to whichever
desktop user is logged in at the graphical seat. The GUI connects read-only; it never runs
privileged and cannot command the daemon.

See `docs/helper-design.md` for the full security rationale. The short version:

- Runs as the system user **`keyd-viz`**, never root. A compromise is keystroke-*read*
  only — no escalation, and (sandbox below) no network to exfiltrate over.
- **Layers-only by default** — the base unit grants only the `keyd` group and *zero*
  `/dev/input` access, so as shipped it is physically incapable of reading keystrokes.
  Keypress glow is a separate, explicit opt-in (`keypresses.conf`).
- Hardened by a tight systemd sandbox (`PrivateNetwork`, `RestrictAddressFamilies=AF_UNIX`,
  read-only FS, dropped capabilities, `DevicePolicy=closed`, …).
- Authorized per-connection by `SO_PEERCRED` + logind: only the uid logind reports as the
  **active** (foreground) session user is served (`--active-session`).

## Install

Run the script as a normal user (it builds as you and uses `sudo` only for the install
steps). Re-running it cleanly updates an existing install.

```bash
./packaging/install.sh            # layers only (safe default)
./packaging/install.sh --keys     # also enable keypress glow (reads /dev/input)
```

Then just launch the GUI (`keydviz`): it auto-discovers the socket at
`/run/keyd-viz/keyd-viz.sock` and uses the broker. No groups to join, no logout.

To switch keypress glow on/off later, just re-run with or without `--keys`. To remove
everything: `./packaging/uninstall.sh` (add `--purge` to also drop the `keyd-viz` user).

<details><summary>What the script does (manual equivalent)</summary>

```bash
cargo build --release -p keydviz-helper
sudo install -Dm755 target/release/keydviz-helperd /usr/bin/keydviz-helperd
sudo install -Dm644 packaging/sysusers.d/keyd-viz.conf /usr/lib/sysusers.d/keyd-viz.conf
sudo systemd-sysusers
sudo install -Dm644 packaging/systemd/keydviz-helperd.service \
    /usr/lib/systemd/system/keydviz-helperd.service
# --keys only: also drop in keypresses.conf under /etc/systemd/system/keydviz-helperd.service.d/
sudo systemctl daemon-reload
sudo systemctl enable --now keydviz-helperd
```

</details>

## Verify

```bash
systemctl status keydviz-helperd
systemd-analyze security keydviz-helperd     # sandbox exposure score
journalctl -u keydviz-helperd -f             # watch authz decisions / rejections
```

## Uninstall

```bash
sudo systemctl disable --now keydviz-helperd
sudo rm -f /usr/lib/systemd/system/keydviz-helperd.service \
           /etc/systemd/system/keydviz-helperd.service.d/keypresses.conf \
           /usr/lib/sysusers.d/keyd-viz.conf \
           /usr/bin/keydviz-helperd
sudo systemctl daemon-reload
# Optionally remove the user: sudo userdel keyd-viz
```

## Note on the dev group grant

Before this service existed, live layers required adding your login user to the `keyd`
group by hand. Once the service is installed and working, that grant is redundant and can
be reverted — see the `dev-interim-group-grants` note. The whole point of the helper is
that no human user needs `keyd`/`input` membership anymore; only the `keyd-viz` service
user does, and only for as long as the unit runs it.
