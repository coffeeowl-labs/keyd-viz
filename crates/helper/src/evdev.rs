//! Read keyd's virtual keyboard from evdev directly — replaces spawning `keyd monitor`.
//!
//! keyd grabs the physical keyboards and re-emits the post-remap result through a uinput
//! virtual keyboard (`0fac:0ade`). That virtual device carries exactly the keys we want to
//! glow, so we open it and read `struct input_event` records ourselves instead of forking
//! `keyd monitor` and parsing its text. Each `EV_KEY` event's keycode maps through
//! [`keycode_name`] to the same name keyd would print, so the GUI side is unchanged.
//!
//! Needs read access to `/dev/input` (the `input` group / the unit's keypresses drop-in) —
//! same access `keyd monitor` needed, just without the child process or the `keyd` binary.

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::sync::Arc;
use std::time::Duration;

use keydviz_core::keycode_name;
use keydviz_core::live::{KeyAction, LiveEvent};

use crate::Hub;

/// keyd's uinput virtual *keyboard* identity (its post-remap output). The virtual pointer
/// `0fac:1ade` is a separate device we don't read.
const KEYD_VENDOR: u16 = 0x0fac;
const KEYD_KEYBOARD_PRODUCT: u16 = 0x0ade;

/// `struct input_event.type` for key events.
const EV_KEY: u16 = 1;
/// Wire size of `struct input_event` on 64-bit Linux: `timeval{i64,i64}` + type/code/value.
const INPUT_EVENT_SIZE: usize = 24;

/// `struct input_id` (linux/input.h): bustype, vendor, product, version — 8 bytes.
#[repr(C)]
#[derive(Default)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

/// `_IOC(dir, type, nr, size)` request encoding (asm-generic/ioctl.h).
const fn ioc(dir: u32, typ: u8, nr: u8, size: u32) -> libc::c_ulong {
    ((dir << 30) | (size << 16) | ((typ as u32) << 8) | (nr as u32)) as libc::c_ulong
}
const IOC_READ: u32 = 2;
/// `EVIOCGID` — read the device's `input_id`.
const EVIOCGID: libc::c_ulong = ioc(IOC_READ, b'E', 0x02, 8);
/// `EVIOCGNAME(len)` — read the device name into a `len`-byte buffer.
fn eviocgname(len: u32) -> libc::c_ulong {
    ioc(IOC_READ, b'E', 0x06, len)
}

/// The device's `input_id` via `EVIOCGID`, or `None` on failure.
fn device_id(file: &File) -> Option<InputId> {
    let mut id = InputId::default();
    // SAFETY: EVIOCGID writes exactly sizeof(input_id)=8 bytes into `id`; we pass a
    // matching pointer and trust the result only on success (ret >= 0).
    let ret =
        unsafe { libc::ioctl(file.as_raw_fd(), EVIOCGID, &mut id as *mut InputId as *mut libc::c_void) };
    (ret >= 0).then_some(id)
}

/// The device's human name via `EVIOCGNAME`, or empty on failure.
fn device_name(file: &File) -> String {
    let mut buf = [0u8; 256];
    // SAFETY: EVIOCGNAME copies at most buf.len() bytes (NUL-terminated) into buf and
    // returns the byte count; we read only up to that count and stop at the first NUL.
    let ret = unsafe {
        libc::ioctl(file.as_raw_fd(), eviocgname(buf.len() as u32), buf.as_mut_ptr() as *mut libc::c_void)
    };
    if ret <= 0 {
        return String::new();
    }
    let n = (ret as usize).min(buf.len());
    let end = buf[..n].iter().position(|&b| b == 0).unwrap_or(n);
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// Scan `/dev/input/event*` for keyd's virtual keyboard. Returns its open file, `devid`
/// string (`vendor:product`), and name.
fn find_keyd_keyboard() -> Option<(File, String, String)> {
    for entry in std::fs::read_dir("/dev/input").ok()?.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("event") {
            continue;
        }
        let Ok(file) = OpenOptions::new().read(true).open(entry.path()) else {
            continue;
        };
        if let Some(id) = device_id(&file) {
            if id.vendor == KEYD_VENDOR && id.product == KEYD_KEYBOARD_PRODUCT {
                let devid = format!("{:04x}:{:04x}", id.vendor, id.product);
                let name = device_name(&file);
                return Some((file, devid, name));
            }
        }
    }
    None
}

/// Read keyd's virtual keyboard and broadcast each key down/up — the direct replacement
/// for `keyd monitor`, no child process. Re-finds the device if it disappears (e.g. keyd
/// restart). Blocks, so run it on its own thread.
pub fn run_evdev_monitor(hub: &Arc<Hub>) {
    loop {
        if let Some((mut file, devid, device)) = find_keyd_keyboard() {
            eprintln!("keydviz-helperd: reading keypresses from keyd device {devid} ({device})");
            let mut buf = [0u8; INPUT_EVENT_SIZE];
            while file.read_exact(&mut buf).is_ok() {
                // input_event layout: [0..16] timeval, [16..18] type, [18..20] code,
                // [20..24] value — native endian.
                let typ = u16::from_ne_bytes([buf[16], buf[17]]);
                if typ != EV_KEY {
                    continue;
                }
                let code = u16::from_ne_bytes([buf[18], buf[19]]);
                let value = i32::from_ne_bytes([buf[20], buf[21], buf[22], buf[23]]);
                let action = match value {
                    1 => KeyAction::Down,
                    0 => KeyAction::Up,
                    _ => continue, // 2 = autorepeat; ignore (glow already held from the down)
                };
                if let Some(key) = keycode_name(code) {
                    hub.broadcast(&LiveEvent::Key {
                        devid: devid.clone(),
                        device: device.clone(),
                        key: key.to_string(),
                        action,
                    });
                }
            }
            eprintln!("keydviz-helperd: keyd device {devid} closed; re-finding");
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}
