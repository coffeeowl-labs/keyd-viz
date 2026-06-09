//! keyd `[global]` daemon options — the fixed, documented set the editor surfaces as
//! a typed form (keyd-domain knowledge, so it lives in core; the GUI renders from this
//! table). Ordered most-commonly-edited first. Unknown `[global]` lines a config might
//! carry are preserved verbatim by the line model and are NOT in this list.

/// One known keyd `[global]` option.
pub struct GlobalOption {
    /// The config key, e.g. `layer_indicator`.
    pub name: &'static str,
    /// Human label for the form row.
    pub label: &'static str,
    /// Hint shown beside the field (unit + default).
    pub hint: &'static str,
    /// Render as an on/off toggle (keyd writes `1`/`0`) rather than a text field.
    pub boolean: bool,
}

/// The documented keyd globals (man keyd, GLOBALS). Timeouts are milliseconds unless
/// noted; `macro_sequence_timeout` is microseconds.
pub const GLOBAL_OPTIONS: &[GlobalOption] = &[
    GlobalOption {
        name: "layer_indicator",
        label: "Layer indicator",
        hint: "capslock LED while a layer is active",
        boolean: true,
    },
    GlobalOption {
        name: "default_layout",
        label: "Default layout",
        hint: "layout name (e.g. us)",
        boolean: false,
    },
    GlobalOption {
        name: "overload_tap_timeout",
        label: "Overload tap timeout",
        hint: "ms \u{2014} tap-vs-hold cutoff",
        boolean: false,
    },
    GlobalOption {
        name: "chord_timeout",
        label: "Chord timeout",
        hint: "ms \u{2014} max gap between chord keys (default 50)",
        boolean: false,
    },
    GlobalOption {
        name: "chord_hold_timeout",
        label: "Chord hold timeout",
        hint: "ms \u{2014} hold before a chord registers",
        boolean: false,
    },
    GlobalOption {
        name: "oneshot_timeout",
        label: "One-shot timeout",
        hint: "ms \u{2014} one-shot modifier window",
        boolean: false,
    },
    GlobalOption {
        name: "macro_timeout",
        label: "Macro timeout",
        hint: "ms \u{2014} delay before a macro repeats (default 600)",
        boolean: false,
    },
    GlobalOption {
        name: "macro_repeat_timeout",
        label: "Macro repeat timeout",
        hint: "ms \u{2014} interval between repeats (default 50)",
        boolean: false,
    },
    GlobalOption {
        name: "macro_sequence_timeout",
        label: "Macro sequence timeout",
        hint: "\u{00b5}s \u{2014} delay between emitted keys",
        boolean: false,
    },
    GlobalOption {
        name: "disable_modifier_guard",
        label: "Disable modifier guard",
        hint: "advanced \u{2014} leave off unless you know",
        boolean: true,
    },
];

/// Whether `name` is a documented keyd global option (vs. an unmodeled line).
pub fn is_known_global(name: &str) -> bool {
    GLOBAL_OPTIONS.iter().any(|o| o.name == name)
}
