//! System-tray presence via StatusNotifierItem (SNI) over D-Bus, using `ksni`.
//!
//! The tray gives keyd-viz a resident icon that summons/dismisses the window — pairing
//! with the compact pinnable overlay. It is display-agnostic: SNI is the protocol KDE's
//! plasmashell consumes, and most DEs/WMs with a StatusNotifier host (X11 and Wayland
//! alike) consume it too. Where no host exists (e.g. vanilla GNOME without an
//! AppIndicator extension) `spawn` finds no watcher and the icon is simply absent — the
//! app keeps running normally. No GTK, no C deps, no tokio (the `blocking` + `async-io`
//! features run ksni's D-Bus service on its own thread with a lightweight executor).
//!
//! Threading: ksni's callbacks (`activate`, menu `activate`) fire on its service thread,
//! so every action hops to the Slint UI thread via `slint::Weak::upgrade_in_event_loop`
//! — the same marshalling the listen/monitor workers use. `slint::Weak` is `Send`, so
//! the tray struct can hold one across the thread boundary. Tooltip layer updates run on
//! a dedicated forwarder thread (below) because `Handle::update` blocks on D-Bus I/O and
//! must never stall the UI hot path.

use std::sync::mpsc;

use ksni::menu::StandardItem;
use ksni::{MenuItem, ToolTip, Tray};
use slint::ComponentHandle;

use crate::MainWindow;

/// The tray item. Holds a `Send` weak handle to the window plus the active keyd layer
/// name for the tooltip. Mutated only via `Handle::update` from the forwarder thread.
struct VizTray {
    weak: slint::Weak<MainWindow>,
    /// Active keyd layer, `""` = base. Surfaced in the tooltip description.
    layer: String,
}

impl Tray for VizTray {
    fn id(&self) -> String {
        // Stable id for the SNI registration (one item per running instance).
        "keydviz".into()
    }

    fn title(&self) -> String {
        "keyd-viz".into()
    }

    fn icon_name(&self) -> String {
        // XDG theme icon name, present in every standard icon theme.
        "input-keyboard".into()
    }

    fn tool_tip(&self) -> ToolTip {
        let description = if self.layer.is_empty() {
            "Base layer".into()
        } else {
            format!("Layer: {}", self.layer)
        };
        ToolTip {
            icon_name: "input-keyboard".into(),
            icon_pixmap: Vec::new(),
            title: "keyd-viz".into(),
            description,
        }
    }

    /// Left-click the tray icon → toggle the window. plasmashell sets
    /// `XDG_ACTIVATION_TOKEN` in our env just before this call (legacy SNI `Activate` is
    /// one of the methods KDE retrofitted with activation support), so a tray-summon can
    /// legitimately raise/focus on Wayland — unlike a self-initiated raise.
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self
            .weak
            .upgrade_in_event_loop(|win| crate::toggle_window(&win));
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Show / hide".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t
                        .weak
                        .upgrade_in_event_loop(|win| crate::toggle_window(&win));
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.weak.upgrade_in_event_loop(|_| {
                        let _ = slint::quit_event_loop();
                    });
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Live handle to the running tray. Keep it alive for the app's lifetime — dropping it
/// drops the forwarder thread's sender, ending the thread (the icon stays until the
/// service thread is torn down at process exit). [`set_layer`](Self::set_layer) pushes
/// the active layer into the tooltip without blocking the caller.
pub struct TrayHandle {
    tx: mpsc::Sender<String>,
}

impl TrayHandle {
    /// Push the active keyd layer name into the tray tooltip. Non-blocking: the value is
    /// applied on the forwarder thread, so per-keystroke layer changes never stall the UI.
    pub fn set_layer(&self, layer: &str) {
        let _ = self.tx.send(layer.to_string());
    }
}

/// Spawn the tray on its own D-Bus service thread. Returns `None` (with a warning) if the
/// tray service can't start — no D-Bus session bus, etc. — and the app runs fine without
/// it. Keep the returned [`TrayHandle`] alive for the app's lifetime.
pub fn spawn(win: &MainWindow) -> Option<TrayHandle> {
    use ksni::blocking::TrayMethods;

    let tray = VizTray { weak: win.as_weak(), layer: String::new() };
    let handle = match tray.spawn() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("warning: system tray unavailable: {e}");
            return None;
        }
    };

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        // `Handle::update` blocks on D-Bus I/O, so apply layer changes here off the UI
        // thread. Coalesce bursts: drain everything pending and apply only the latest.
        while let Ok(mut layer) = rx.recv() {
            while let Ok(next) = rx.try_recv() {
                layer = next;
            }
            if handle.update(|t| t.layer = layer.clone()).is_none() {
                break; // tray service shut down
            }
        }
    });

    Some(TrayHandle { tx })
}
