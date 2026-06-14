//! Live-view wiring (UI-thread side) and the pressed-key glow overlay.
//!
//! The pure I/O lives in sibling modules — `layer` (keyd listen), `monitor`
//! (keyd monitor), `helper` (the broker client). This module is the glue that runs
//! each of those on a background thread and marshals its events back onto the Slint
//! event loop, plus the glow model itself: which caps light up for a held chord
//! ([`resolve_glow`]) and the short min-glow decay that keeps a sub-frame tap visible
//! ([`GlowState`]). All glow state is UI-thread-only (the `Rc`-backed models aren't
//! `Send`), so it lives in a thread-local.

use std::cell::RefCell;

use slint::ComponentHandle;

use crate::{helper, layer, monitor};
use crate::{model, render_board, BoardData, KeyCapData, MainWindow};

/// Light up the caps the held keysyms (`pressed`, what `keyd monitor` reports) map to.
/// Each cap's `key` is the `+`-joined chord it emits; a cap fires when that whole set is
/// held, and a cap whose set is a strict subset of another firing cap — or an equal,
/// non-emphasized twin — is suppressed, so only the key you actually pressed glows.
pub(crate) fn stamp_glow(board: &mut BoardData, pressed: &std::collections::HashSet<String>) {
    use slint::Model;
    if pressed.is_empty() {
        return;
    }
    let caps: Vec<KeyCapData> = board.keys.iter().collect();
    let sets: Vec<Vec<String>> = caps
        .iter()
        .map(|k| k.key.split('+').filter(|s| !s.is_empty()).map(str::to_string).collect())
        .collect();
    let emph: Vec<bool> = caps.iter().map(|k| k.emphasized).collect();
    let glow = resolve_glow(&sets, &emph, pressed);
    let keys: Vec<KeyCapData> = caps
        .into_iter()
        .enumerate()
        .map(|(i, mut k)| {
            k.pressed = glow[i];
            k
        })
        .collect();
    board.keys = model(keys);
}

/// Decide which caps glow, given each cap's emitted keysym set (`sets`), whether it is an
/// emphasized remap (`emph`), and the currently-held keysyms (`pressed`). A cap *fires*
/// when every keysym it emits is held; it is then suppressed by any other firing cap that
/// emits a strict superset (more specific — `n`=C-left beats the plain Ctrl and arrow it
/// also reports), or by an equal-set emphasized twin (the remapped key beats its
/// passthrough double, e.g. num-layer `j`=4 over the top-row `4`).
pub(crate) fn resolve_glow(
    sets: &[Vec<String>],
    emph: &[bool],
    pressed: &std::collections::HashSet<String>,
) -> Vec<bool> {
    let fires: Vec<bool> =
        sets.iter().map(|s| !s.is_empty() && s.iter().all(|x| pressed.contains(x))).collect();
    let subset = |a: &[String], b: &[String]| a.iter().all(|x| b.contains(x));
    (0..sets.len())
        .map(|i| {
            fires[i]
                && !(0..sets.len()).any(|j| {
                    j != i
                        && fires[j]
                        && subset(&sets[i], &sets[j])
                        && (sets[i].len() < sets[j].len()
                            || (sets[i].len() == sets[j].len() && emph[j] && !emph[i]))
                })
        })
        .collect()
}

/// Subscribe to the `keydviz-helperd` broker on a background thread: one socket carries
/// both the layer stream and (if the helper brokers keypresses) the glow. Each
/// [`LiveEvent`] is split onto the same UI paths the direct-`keyd` sources feed, so the
/// view behaves identically whether the source is the helper or a spawned `keyd`.
pub(crate) fn spawn_helper(win: &MainWindow, socket: String) {
    use keydviz_core::live::LiveEvent;
    let weak = win.as_weak();
    std::thread::spawn(move || {
        // Layer state is reduced here (the helper sends raw transitions, like keyd).
        let mut active = keydviz_core::live::ActiveLayers::default();
        helper::run_helper_client(
            &socket,
            {
                let weak = weak.clone();
                move |connected| {
                    let weak = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(win) = weak.upgrade() {
                            win.set_live_connected(connected);
                            if !connected {
                                win.set_keys_connected(false);
                                win.set_active_stack(model(Vec::new()));
                                clear_glow(&win);
                                render_board(&win);
                            }
                        }
                    });
                }
            },
            move |ev| {
                // Reduce layer events on this thread; ship a plain snapshot to the UI.
                let layer_snapshot = ev.as_layer().map(|le| {
                    active.apply(&le);
                    active.active()
                });
                let monitor_ev = ev.as_monitor();
                let keys_connected = matches!(ev, LiveEvent::Key { .. });
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = weak.upgrade() {
                        if let Some(stack) = layer_snapshot {
                            let stack: Vec<slint::SharedString> =
                                stack.into_iter().map(Into::into).collect();
                            win.set_active_stack(model(stack));
                            win.set_live_connected(true);
                            render_board(&win);
                        }
                        if let Some(mev) = monitor_ev {
                            if keys_connected {
                                win.set_keys_connected(true);
                            }
                            handle_key_event(&win, mev);
                        }
                    }
                });
            },
        );
    });
}

/// Subscribe to live layer state from `keyd listen` on a background thread, pushing
/// updates onto the UI thread. Degrades gracefully to "live view off" when the keyd
/// socket isn't accessible (e.g. not in the `keyd` group).
pub(crate) fn spawn_live(win: &MainWindow) {
    let weak = win.as_weak();
    std::thread::spawn(move || {
        layer::run_listen(move |state| {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = weak.upgrade() {
                    let stack: Vec<slint::SharedString> =
                        state.active.iter().map(|s| s.clone().into()).collect();
                    win.set_active_stack(model(stack));
                    win.set_live_connected(state.connected);
                    render_board(&win);
                }
            });
        });
    });
}

/// Subscribe to live keypresses from `keyd monitor` on a background thread. Drives the
/// pressed-key glow and follows the last-pressed keyboard. Works wherever `/dev/input`
/// is readable (typically the `input` group); the shipped product routes this through
/// the privileged helper so even that isn't required (ROADMAP §1).
pub(crate) fn spawn_monitor(win: &MainWindow) {
    let weak = win.as_weak();
    let weak_conn = weak.clone();
    std::thread::spawn(move || {
        monitor::run_monitor(
            move |connected| {
                let weak = weak_conn.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = weak.upgrade() {
                        win.set_keys_connected(connected);
                        if !connected {
                            clear_glow(&win);
                            render_board(&win);
                        }
                    }
                });
            },
            move |ev| {
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = weak.upgrade() {
                        handle_key_event(&win, ev);
                    }
                });
            },
        );
    });
}

/// Minimum time a key stays lit after it goes down, so a fast tap-hold *tap* — which keyd
/// emits as a near-simultaneous down+up at release — is still visible instead of flashing
/// for less than a display frame. (Why "fear" wouldn't light `f` but "after" would: an
/// isolated tap resolves on release and emits down+up together; a rolled tap emits the
/// down earlier, spread across frames. This makes both visible.)
///
/// Tuned to the minimum that reliably survives a painted frame: ~3–4 frames at 60Hz (more
/// at higher refresh), with margin for a dropped frame. Anchored to key-*down*, so any key
/// held at least this long turns off the instant it's released — most typists, even fast
/// ones, get no visible tail; only genuine sub-frame taps linger, just enough to be seen.
const MIN_GLOW: std::time::Duration = std::time::Duration::from_millis(60);

/// Glow bookkeeping (UI-thread only): the truly-held keys, plus a short per-key "keep lit
/// until" decay so quick taps remain visible. A thread-local because every keypress
/// callback runs on the Slint event loop and the `Rc`-backed models aren't `Send`, so it
/// can't be shared across the source threads anyway.
#[derive(Default)]
struct GlowState {
    /// Physically-held keys (from keyd's output), maintained by [`monitor::next_press_state`].
    held: Vec<String>,
    /// key → instant after which it may stop glowing (refreshed on every event for that key).
    until: std::collections::HashMap<String, std::time::Instant>,
}

impl GlowState {
    /// The set to light: everything held, plus any key still inside its decay window.
    /// Prunes expired decay entries as a side effect.
    fn glow_set(&mut self) -> Vec<String> {
        let now = std::time::Instant::now();
        self.until.retain(|_, t| *t > now);
        let mut out = self.held.clone();
        for k in self.until.keys() {
            if !out.iter().any(|h| h == k) {
                out.push(k.clone());
            }
        }
        out
    }

    fn clear(&mut self) {
        self.held.clear();
        self.until.clear();
    }
}

thread_local! {
    static GLOW: RefCell<GlowState> = RefCell::new(GlowState::default());
}

/// Drop all glow state and clear the rendered set — used when a source disconnects or the
/// shown keyboard switches (we don't know the new context's held keys).
pub(crate) fn clear_glow(win: &MainWindow) {
    GLOW.with(|g| g.borrow_mut().clear());
    win.set_pressed_keys(model(Vec::new()));
}

/// Apply one `keyd monitor` key event on the UI thread: follow the last-pressed keyboard
/// (switch the shown sheet), update the held set + min-glow decay, and re-render. The
/// held-set + follow-keyboard decision stays in [`monitor::next_press_state`] (pure,
/// tested); the decay overlay ([`GlowState`]) keeps fast taps visible.
pub(crate) fn handle_key_event(win: &MainWindow, ev: monitor::MonitorEvent) {
    use slint::Model;

    let monitor::MonitorEvent::Key(k) = ev else { return }; // ignore device add/remove

    // An armed press-to-capture eats this key-down as the new binding value. monitor
    // reports the *emitted* keysym, so capturing from a key that is itself remapped
    // reports its output — which is the symbol the user is choosing anyway.
    if win.get_edit_mode() && win.get_capture_armed() {
        if matches!(k.action, monitor::KeyAction::Down) {
            win.invoke_apply_binding(k.key.clone().into());
        }
        return; // the captured press shouldn't also glow or switch keyboards
    }

    let map: Vec<(String, i32)> =
        win.get_device_map().iter().map(|m| (m.devid.to_string(), m.sheet)).collect();

    GLOW.with(|g| {
        let mut g = g.borrow_mut();
        let monitor::Press { switch_to, pressed } =
            monitor::next_press_state(&k, &map, win.get_active_index(), &g.held);
        // Editing is per-file: don't let a keypress yank the view to another keyboard.
        if let Some(idx) = switch_to.filter(|_| !win.get_edit_mode()) {
            g.until.clear(); // new board: don't carry the old board's decaying glow
            if let Some(sheet) = win.get_sheets().row_data(idx as usize) {
                win.set_active_index(idx);
                win.set_active_sheet(sheet);
            }
        }
        g.held = pressed;
        // Floor the glow at MIN_GLOW from *key-down* only. A key held longer than that is
        // kept lit by `held` and turns off the instant it's released; only a sub-frame tap
        // (an isolated tap-hold tap, whose down+up land in one frame) lingers — just long
        // enough to be seen — so normal typing doesn't trail behind your fingers.
        if matches!(k.action, monitor::KeyAction::Down) {
            g.until.insert(k.key.clone(), std::time::Instant::now() + MIN_GLOW);
        }
        let glow: Vec<slint::SharedString> = g.glow_set().into_iter().map(Into::into).collect();
        win.set_pressed_keys(model(glow));
    });
    render_board(win);
}

/// Expire the min-glow decay: a few times a second, recompute the lit set and repaint if
/// it shrank. This is what turns a quick tap's glow back off when no further key events
/// arrive. Returns the timer; keep it alive for the app's life. (Not used in `--demo`,
/// which drives the glow directly.)
pub(crate) fn spawn_glow_decay(win: &MainWindow) -> slint::Timer {
    use slint::Model;
    let weak = win.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(16), move || {
        let Some(win) = weak.upgrade() else { return };
        let changed = GLOW.with(|g| {
            let mut glow = g.borrow_mut().glow_set();
            let mut cur: Vec<String> =
                win.get_pressed_keys().iter().map(|s| s.to_string()).collect();
            glow.sort();
            cur.sort();
            if glow != cur {
                let m: Vec<slint::SharedString> = glow.into_iter().map(Into::into).collect();
                win.set_pressed_keys(model(m));
                true
            } else {
                false
            }
        });
        if changed {
            render_board(&win);
        }
    });
    timer
}

/// `--demo`: animate the live view without a running keyd — sweep a pressed key across
/// the board (glow) while cycling the active layer, so both effects are visible.
pub(crate) fn spawn_demo(win: &MainWindow) {
    use slint::Model;
    // Layer cycle (base + each layer) as synthetic stacks.
    let mut layers: Vec<Vec<slint::SharedString>> = vec![Vec::new()];
    for board in win.get_active_sheet().boards.iter() {
        if !board.is_base {
            let stack = vec![board.title.clone()];
            if !layers.contains(&stack) {
                layers.push(stack);
            }
        }
    }
    // Home-row-ish sweep of keyd key names for the glow.
    let keys = ["a", "s", "d", "f", "g", "h", "j", "k", "l", "space"];
    let weak = win.as_weak();
    std::thread::spawn(move || {
        let mut i = 0usize;
        loop {
            let stack = layers[(i / keys.len()) % layers.len()].clone();
            let key: slint::SharedString = keys[i % keys.len()].into();
            i += 1;
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = weak.upgrade() {
                    win.set_live_connected(true);
                    win.set_keys_connected(true);
                    win.set_active_stack(model(stack));
                    win.set_pressed_keys(model(vec![key]));
                    render_board(&win);
                }
            });
            std::thread::sleep(std::time::Duration::from_millis(260));
        }
    });
}

#[cfg(test)]
mod glow_tests {
    use super::resolve_glow;
    use std::collections::HashSet;

    fn set(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }
    fn held(parts: &[&str]) -> HashSet<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn chord_cap_suppresses_its_subsets() {
        // nav `n`=C-left (leftcontrol+left), the real Ctrl cap, and `h`=left arrow.
        let sets = vec![set(&["leftcontrol", "left"]), set(&["leftcontrol"]), set(&["left"])];
        let emph = vec![true, false, true];
        let glow = resolve_glow(&sets, &emph, &held(&["leftcontrol", "left"]));
        assert_eq!(glow, vec![true, false, false], "only the n chord cap glows");
    }

    #[test]
    fn plain_ctrl_still_glows_alone() {
        // Holding only Ctrl: the chord cap can't fire, the Ctrl cap does.
        let sets = vec![set(&["leftcontrol", "left"]), set(&["leftcontrol"]), set(&["left"])];
        let emph = vec![true, false, true];
        let glow = resolve_glow(&sets, &emph, &held(&["leftcontrol"]));
        assert_eq!(glow, vec![false, true, false]);
    }

    #[test]
    fn emphasized_twin_wins_over_passthrough() {
        // num-layer `j`=4 (emphasized) vs the top-row passthrough `4`.
        let sets = vec![set(&["4"]), set(&["4"])];
        let emph = vec![true, false];
        let glow = resolve_glow(&sets, &emph, &held(&["4"]));
        assert_eq!(glow, vec![true, false]);
    }

    #[test]
    fn shifted_symbol_chord_suppresses_digit_and_shift() {
        // sym `j`=S-9 (leftshift+9) vs the real Shift cap and the passthrough `9`.
        let sets = vec![set(&["leftshift", "9"]), set(&["leftshift"]), set(&["9"])];
        let emph = vec![true, false, false];
        let glow = resolve_glow(&sets, &emph, &held(&["leftshift", "9"]));
        assert_eq!(glow, vec![true, false, false]);
    }

    #[test]
    fn empty_set_never_glows() {
        // the held layer-activator carries no output keysym.
        let sets = vec![Vec::new(), set(&["a"])];
        let emph = vec![false, false];
        let glow = resolve_glow(&sets, &emph, &held(&["a"]));
        assert_eq!(glow, vec![false, true]);
    }
}
