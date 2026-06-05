//! keyd-viz brokering daemon (`keydviz-helperd`).
//!
//! Reads keyd's live streams and re-exposes them to the unprivileged GUI as a
//! one-directional [`LiveEvent`] stream over a unix socket — the mechanism that
//! delivers ROADMAP §1's "zero manual permission setup" (`docs/helper-design.md`,
//! Option A). The GUI never runs privileged and can only *read* events; it cannot
//! command the daemon.
//!
//! **Security posture** (see the design doc): this runs as a dedicated non-root system
//! user in groups `keyd` + `input`, caged by the systemd sandbox in `packaging/`
//! (`PrivateNetwork`, read-only FS, dropped caps). By default it serves **layers only**
//! (no `/dev/input` access at all — literally not a keylogger); keypresses are opt-in via
//! `--keys`. Layers are read **directly from keyd's control socket** ([`keyd_ipc`], no
//! child process); keypresses still spawn `keyd monitor` until that's read from evdev too.
//!
//! Authz: every connection is gated by the peer uid (`SO_PEERCRED`, kernel-attested).
//! The default [`Policy::Uid`] serves only the daemon's own uid — the dev / same-user
//! path. The shipped service instead runs with `--active-session`, so it serves whoever
//! logind reports as the foreground desktop user ([`Policy::ActiveSession`]); that's what
//! lets it run as `keyd-viz` without a shared group or hard-coded uid. See `authz.rs`.

mod authz;
mod evdev;
mod keyd_ipc;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use authz::{Decision, Policy};
use keydviz_core::live::{parse_listen_line, LayerAction, LiveEvent};

/// Connected clients plus the current layer snapshot, so a late-joining GUI is brought
/// up to date instead of waiting for the next transition.
#[derive(Default)]
struct Hub {
    clients: Mutex<Vec<UnixStream>>,
    /// Active layer stack (most recent last) + current layout, mirrored from the stream
    /// so we can replay a snapshot to new clients.
    snapshot: Mutex<(Vec<String>, Option<String>)>,
    keyd_version: String,
}

impl Hub {
    /// Fan one event out to every client, dropping any that error. Layer events also
    /// update the replay snapshot.
    fn broadcast(&self, ev: &LiveEvent) {
        if let LiveEvent::Layer { action, name } = ev {
            let (stack, layout) = &mut *self.snapshot.lock().unwrap();
            match action {
                LayerAction::On if !stack.contains(name) => stack.push(name.clone()),
                LayerAction::Off => stack.retain(|n| n != name),
                LayerAction::Layout => *layout = Some(name.clone()),
                LayerAction::On => {}
            }
        }
        let line = ev.to_line();
        let mut clients = self.clients.lock().unwrap();
        clients.retain_mut(|c| c.write_all(line.as_bytes()).and_then(|_| c.flush()).is_ok());
    }

    /// Register a new client: greet it, then replay the current layer snapshot so its
    /// view is immediately correct.
    fn add_client(&self, mut stream: UnixStream) {
        let hello = LiveEvent::Hello { keyd: self.keyd_version.clone() };
        if stream.write_all(hello.to_line().as_bytes()).is_err() {
            return;
        }
        let (stack, layout) = self.snapshot.lock().unwrap().clone();
        if let Some(name) = layout {
            let _ = stream.write_all(
                LiveEvent::Layer { action: LayerAction::Layout, name }.to_line().as_bytes(),
            );
        }
        for name in stack {
            let line = LiveEvent::Layer { action: LayerAction::On, name }.to_line();
            if stream.write_all(line.as_bytes()).is_err() {
                return;
            }
        }
        self.clients.lock().unwrap().push(stream);
    }
}

/// Follow keyd's layer state by reading its **control socket directly** — no child
/// process. Connect + subscribe ([`keyd_ipc`]), then parse the `/`,`+`,`-` text lines and
/// fan them out. Reconnects on loss; blocks, so run it on its own thread.
fn run_keyd_listen(socket: &str, hub: &Arc<Hub>) {
    loop {
        match keyd_ipc::connect_layer_listen(socket) {
            Ok(stream) => {
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    if let Some(ev) = parse_listen_line(&line) {
                        let live: LiveEvent = (&ev).into();
                        hub.broadcast(&live);
                    }
                }
            }
            Err(e) => eprintln!("keydviz-helperd: cannot connect to keyd socket {socket}: {e}"),
        }
        // Stream lost: the layer snapshot is stale — clear it so new clients don't get ghosts.
        *hub.snapshot.lock().unwrap() = (Vec::new(), None);
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// Synthetic source for testing the socket/protocol without keyd access (`--demo`):
/// cycles a few layers and taps a key, so a connected GUI shows live activity.
fn run_demo(hub: &Arc<Hub>) {
    let layers = ["nav", "num", "sym"];
    let key = |k: &str, action| LiveEvent::Key {
        devid: "0fac:0ade".into(),
        device: "demo keyboard".into(),
        key: k.into(),
        action,
    };
    use keydviz_core::live::KeyAction::{Down, Up};
    loop {
        for layer in layers {
            hub.broadcast(&LiveEvent::Layer { action: LayerAction::On, name: layer.into() });
            std::thread::sleep(Duration::from_millis(900));
            for k in ["j", "k", "l"] {
                hub.broadcast(&key(k, Down));
                std::thread::sleep(Duration::from_millis(250));
                hub.broadcast(&key(k, Up));
            }
            hub.broadcast(&LiveEvent::Layer { action: LayerAction::Off, name: layer.into() });
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

struct Args {
    socket: String,
    keyd_socket: String,
    keys: bool,
    demo: bool,
    policy: Policy,
}

fn parse_args() -> Args {
    let mut socket = default_socket_path();
    let mut keyd_socket = keyd_ipc::KEYD_SOCKET.to_string();
    let mut keys = false;
    let mut demo = false;
    // Default policy: serve our own uid (dev / same-user). `--uid`/`--active-session`
    // override it. SAFETY: getuid is always safe; it just reads the caller's real uid.
    let mut policy = Policy::Uid(unsafe { libc::getuid() });
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--socket" => socket = it.next().unwrap_or_else(|| die("--socket needs a path")),
            "--keyd-socket" => {
                keyd_socket = it.next().unwrap_or_else(|| die("--keyd-socket needs a path"))
            }
            "--keys" => keys = true,
            "--demo" => demo = true,
            "--active-session" => policy = Policy::ActiveSession,
            "--uid" => {
                let uid = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("--uid needs a number"));
                policy = Policy::Uid(uid);
            }
            "-h" | "--help" => {
                println!(
                    "keydviz-helperd — broker keyd live streams to the GUI over a unix socket\n\n\
                     Usage: keydviz-helperd [--socket PATH] [--keyd-socket PATH] [--keys] \
                     [--demo] [--active-session | --uid N]\n\n\
                     --socket PATH     unix socket to serve (default: {})\n\
                     --keyd-socket P   keyd control socket to read layers from (default: {})\n\
                     --keys            also broker keypresses (keyd monitor; needs /dev/input). \
                     Default: layers only.\n\
                     --demo            emit synthetic events instead of reading keyd (testing)\n\
                     --active-session  serve the logind active (foreground) session user — the \
                     shipped service mode\n\
                     --uid N           serve only this peer uid (default: own uid)",
                    default_socket_path(),
                    keyd_ipc::KEYD_SOCKET
                );
                std::process::exit(0);
            }
            other => die(&format!("unknown argument: {other}")),
        }
    }
    Args { socket, keyd_socket, keys, demo, policy }
}

fn default_socket_path() -> String {
    match std::env::var("XDG_RUNTIME_DIR") {
        Ok(dir) if !dir.is_empty() => format!("{dir}/keyd-viz.sock"),
        _ => "/run/keyd-viz.sock".to_string(),
    }
}

fn die(msg: &str) -> ! {
    eprintln!("keydviz-helperd: {msg}");
    std::process::exit(2);
}

/// The connecting client's uid via `SO_PEERCRED` — the kernel-attested peer identity,
/// unforgeable by the client. `None` if the lookup fails. (`UnixStream::peer_cred` would
/// do this, but it's still unstable on stable Rust, so we read the sockopt directly.)
fn peer_uid(stream: &UnixStream) -> Option<u32> {
    // SAFETY: zeroed ucred is a valid initial value; getsockopt fills it and we pass the
    // matching buffer size. We only read `cred.uid` on success (ret == 0).
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    (ret == 0).then_some(cred.uid)
}

/// Best-effort keyd version string for the `hello` event (empty if keyd isn't runnable).
fn keyd_version() -> String {
    Command::new("keyd")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn main() {
    let args = parse_args();

    // Fresh socket: clear any stale node, bind, and set perms per the authz policy
    // (owner-only for same-uid; world-connectable for active-session, where the
    // per-connection check — not the file mode — gates the data).
    let _ = std::fs::remove_file(&args.socket);
    let listener = match UnixListener::bind(&args.socket) {
        Ok(l) => l,
        Err(e) => die(&format!("cannot bind {}: {e}", args.socket)),
    };
    if let Err(e) = std::fs::set_permissions(
        &args.socket,
        std::os::unix::fs::PermissionsExt::from_mode(args.policy.socket_mode()),
    ) {
        eprintln!("keydviz-helperd: warning: cannot chmod socket: {e}");
    }

    let hub = Arc::new(Hub { keyd_version: keyd_version(), ..Default::default() });

    // Event sources on background threads.
    if args.demo {
        let hub = hub.clone();
        std::thread::spawn(move || run_demo(&hub));
        eprintln!("keydviz-helperd: demo mode (synthetic events)");
    } else {
        let h = hub.clone();
        let keyd_socket = args.keyd_socket.clone();
        std::thread::spawn(move || run_keyd_listen(&keyd_socket, &h));
        if args.keys {
            let h = hub.clone();
            std::thread::spawn(move || evdev::run_evdev_monitor(&h));
            eprintln!("keydviz-helperd: layers + keypresses");
        } else {
            eprintln!("keydviz-helperd: layers only (pass --keys to broker keypresses)");
        }
    }

    eprintln!("keydviz-helperd: serving {} ({:?})", args.socket, args.policy);

    // Accept loop: authorize the kernel-attested peer uid, then register for fan-out.
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("keydviz-helperd: accept error: {e}");
                continue;
            }
        };
        match peer_uid(&stream) {
            Some(uid) => match args.policy.decide(uid) {
                Decision::Allow => hub.add_client(stream),
                Decision::Deny(why) => {
                    eprintln!("keydviz-helperd: rejected client uid {uid}: {why}")
                }
            },
            None => eprintln!("keydviz-helperd: no peer creds, rejecting client"),
        }
    }
}
