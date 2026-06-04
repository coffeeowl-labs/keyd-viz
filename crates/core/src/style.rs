//! Display constants shared with the renderer: layer accent colors and the
//! human names for modifier targets. Ported from the original Python tool.

/// Accent color (hex) for a layer or mod, used to tint caps, badges, and tags.
pub fn accent_for(name: &str) -> &'static str {
    match name {
        "nav" => "#4aa3ff",
        "num" => "#3ddc84",
        "sym" => "#c792ea",
        "control" => "#ff6b6b",
        "game" => "#9aa0a6",
        _ => DEFAULT_ACCENT,
    }
}

/// Fallback accent for layers/mods with no assigned color.
pub const DEFAULT_ACCENT: &str = "#ffb454";
/// Accent for plain remaps (the "orange" caps).
pub const REMAP_ACCENT: &str = "#ffb454";

/// Human name for a modifier target (`control` → `Ctrl`); unknown names pass
/// through unchanged.
pub fn mod_name(target: &str) -> &str {
    match target {
        "control" => "Ctrl",
        "shift" => "Shift",
        "alt" => "Alt",
        "meta" => "Super",
        "altgr" => "AltGr",
        other => other,
    }
}
