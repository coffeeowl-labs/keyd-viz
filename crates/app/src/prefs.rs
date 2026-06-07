//! Tiny persistence for the per-config layout choice.
//!
//! keyd carries no physical-layout info, so the layout the user picks for a board is
//! ours to remember. We store it as a flat `layout-id<TAB>config-path` table under the
//! XDG config dir — no dependency, trivially robust, easy to inspect or hand-edit.
//! Everything here is best-effort: a missing or unreadable file just means "no saved
//! choice", and a failed write is ignored (the choice still applies for the session).

use std::path::PathBuf;

/// `$XDG_CONFIG_HOME`, else `$HOME/.config` — the base for keyd-viz's config dir.
/// Shared so every per-user store (layout choices, edit-mode drafts) agrees on it.
pub(crate) fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

/// `~/.config/keyd-viz/layouts.tsv` (honouring `$XDG_CONFIG_HOME`).
fn store_path() -> Option<PathBuf> {
    Some(config_home()?.join("keyd-viz").join("layouts.tsv"))
}

/// The saved layout id for a config path, if any.
pub fn load(config_path: &str) -> Option<String> {
    let text = std::fs::read_to_string(store_path()?).ok()?;
    text.lines().find_map(|line| {
        let (id, path) = line.split_once('\t')?;
        (path == config_path).then(|| id.to_string())
    })
}

/// Persist (or update) the layout id chosen for a config path. Best-effort.
pub fn save(config_path: &str, layout_id: &str) {
    let Some(path) = store_path() else { return };
    // read existing rows, drop any prior entry for this config, append the new one
    let mut rows: Vec<(String, String)> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_once('\t').map(|(i, p)| (i.to_string(), p.to_string())))
        .filter(|(_, p)| p != config_path)
        .collect();
    rows.push((layout_id.to_string(), config_path.to_string()));

    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body: String = rows.iter().map(|(i, p)| format!("{i}\t{p}\n")).collect();
    let _ = std::fs::write(&path, body);
}
