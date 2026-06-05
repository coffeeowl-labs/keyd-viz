//! Direct client for keyd's control socket — replaces spawning `keyd listen`.
//!
//! `keyd listen` is a dumb pipe: it connects to keyd's unix control socket, writes one
//! `struct ipc_message` to subscribe, and copies the daemon's text back to stdout. We do
//! the same in-process, so the daemon no longer forks a `keyd` child just to follow
//! layers — the bytes feed straight into the same [`parse_listen_line`] we already use.
//!
//! Protocol is keyd's, verified against v2.6.0 (`src/ipc.c`, `src/keyd.h`, `src/daemon.c`):
//! after the subscribe write the connection becomes a one-way stream of newline-terminated
//! lines — `/<layout>`, `+<layer>` (activated), `-<layer>` (deactivated) — beginning with a
//! snapshot of current state. It's an explicitly-unstable, no-framing, native-endian raw
//! struct protocol; if keyd's version differs, re-verify the layout.
//!
//! [`parse_listen_line`]: keydviz_core::live::parse_listen_line

use std::io::{self, Write};
use std::os::unix::net::UnixStream;

/// keyd's compiled-in control socket (`SOCKET_PATH` in keyd's Makefile). `/var/run` is a
/// symlink to `/run` on modern systems, so this is the same inode as `/run/keyd.socket`;
/// we use the literal keyd constant.
pub const KEYD_SOCKET: &str = "/var/run/keyd.socket";

/// `enum ipc_message.type` value that subscribes to the layer-event stream
/// (`IPC_LAYER_LISTEN` in keyd's enum).
const IPC_LAYER_LISTEN: u32 = 6;

/// keyd's `struct ipc_message` (`src/keyd.h`), x86-64 / LP64 layout: 4112 bytes total,
/// written/read raw over the socket with no length prefix and native byte order. We only
/// ever send it (to subscribe); keyd replies on this connection with text, not structs.
#[repr(C)]
struct IpcMessage {
    /// `enum` → C `int`, 4 bytes.
    msg_type: u32,
    /// `uint32_t timeout` — unused for listen (0).
    timeout: u32,
    /// `char data[MAX_IPC_MESSAGE_SIZE]` — empty for listen.
    data: [u8; 4096],
    /// `size_t sz` — payload length, 0 for listen.
    sz: u64,
}

// Match keyd's wire size exactly; a mismatch here means our struct layout drifted from the
// daemon's and the subscribe would be silently malformed.
const _: () = assert!(std::mem::size_of::<IpcMessage>() == 4112);

/// Connect to keyd's control socket and subscribe to the layer-event stream. The returned
/// stream is ready to read newline-delimited `/`,`+`,`-` lines (starting with a snapshot).
///
/// Read promptly: keyd sets `SO_SNDTIMEO=50ms` per listener and drops any whose write
/// blocks. A dedicated read→parse→broadcast loop satisfies that.
pub fn connect_layer_listen(socket: &str) -> io::Result<UnixStream> {
    let mut stream = UnixStream::connect(socket)?;
    let msg = IpcMessage { msg_type: IPC_LAYER_LISTEN, timeout: 0, data: [0u8; 4096], sz: 0 };
    // SAFETY: IpcMessage is #[repr(C)] with no padding (verified 4112 bytes above) and
    // every field is initialized, so viewing it as a byte slice exposes no uninitialized
    // memory. We only read the bytes to write the raw struct exactly as keyd's client does.
    let bytes = unsafe {
        std::slice::from_raw_parts(
            &msg as *const IpcMessage as *const u8,
            std::mem::size_of::<IpcMessage>(),
        )
    };
    stream.write_all(bytes)?;
    Ok(stream)
}
