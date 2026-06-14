//! Detection: decide which config sheets to show and how connected keyboards map to
//! them. The auto-detect path globs `/etc/keyd/*.conf`, matches each connected
//! keyboard to its best `[ids]` config (explicit beats wildcard), and labels each
//! sheet with the device on it; explicit path args and QMK import show exactly what
//! was asked for. [`rescan`] redoes just the matching on hotplug without re-deciding
//! the shown set. The device capability read + `[ids]` matching live in the
//! `devices` module and `keydviz_core`; this module is the policy that wires them to
//! the displayed [`Detection`].

use std::path::{Path, PathBuf};

use keydviz_core::{
    find_conflicts, import_qmk, parse_file, parse_text, Config, IdConflict, Ids, MatchKind,
};

use crate::create::KEYD_VIRTUAL_VENDOR;
use crate::devices::{self, InputDevice};
use crate::SheetSrc;

/// All `*.conf` files directly inside `dir` (sorted; empty if unreadable).
pub(crate) fn conf_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "conf"))
        .collect();
    v.sort();
    v
}

/// Parse the given config paths into `(path, Config)`, warning on failures.
pub(crate) fn parse_configs(paths: &[PathBuf]) -> Vec<(PathBuf, Config)> {
    paths
        .iter()
        .filter_map(|p| match parse_file(p) {
            Ok(cfg) => Some((p.clone(), cfg)),
            Err(e) => {
                eprintln!("warning: skipping {}: {e}", p.display());
                None
            }
        })
        .collect()
}

/// A short label for the device(s) that matched a config. One physical keyboard
/// can expose several event nodes (e.g. a "Consumer Control" node) sharing a
/// `vendor:product`; we group by that so each physical keyboard appears once,
/// preferring the full-keyboard node's name.
fn device_label(devices: &[InputDevice], idxs: &[usize]) -> String {
    // (devid, chosen name, name-is-from-full-keyboard)
    let mut groups: Vec<(String, &str, bool)> = Vec::new();
    for &i in idxs {
        let d = &devices[i];
        // Only a real keyboard headlines a board. Media-only pseudo-devices (Video Bus,
        // WMI hotkeys, lid/power switches) are keyboard-*capable* in keyd's eyes — so a
        // `[ids] *` wildcard genuinely matches them — but labeling the board "Video Bus"
        // is alarming and wrong. Drop anything lacking the full alphanumeric block, plus
        // keyd's own virtual keyboard. (A combo keyboard+touchpad node still passes — its
        // keyboard half is real.) A wildcard config whose only leftovers are pseudo-
        // devices thus gets an empty label; the header falls back to the config name and
        // the banner explains the catch-all.
        if !d.full_keyboard || d.vendor == KEYD_VIRTUAL_VENDOR {
            continue;
        }
        let devid = d.devid();
        if let Some(g) = groups.iter_mut().find(|g| g.0 == devid) {
            if (d.full_keyboard && !g.2) || g.1.is_empty() {
                g.1 = &d.name;
                g.2 = d.full_keyboard;
            }
        } else {
            groups.push((devid, &d.name, d.full_keyboard));
        }
    }
    let names: Vec<&str> = groups.iter().map(|g| g.1).filter(|n| !n.is_empty()).collect();
    match names.len() {
        0 => String::new(),
        1 => names[0].to_string(),
        _ => format!("{} (+{})", names[0], names.len() - 1),
    }
}

/// The board-header label for a config: the real keyboard(s) it *explicitly* governs,
/// or the static "all keyboards" when it only matched via the `*` wildcard. A wildcard
/// config's matched devices are arbitrary leftovers — every real keyboard is claimed by
/// its specific config, so only pseudo-devices fall through to the catch-all (Video Bus,
/// the ydotoold virtual device, WMI hotkeys, power/lid switches) — and naming the board
/// after whichever one the rescan happened to pick is misleading. So a wildcard config
/// gets a stable name, not a device of the day.
fn config_label(ids: &Ids, devices: &[InputDevice], idxs: &[usize]) -> String {
    let explicit: Vec<usize> = idxs
        .iter()
        .copied()
        .filter(|&i| {
            ids.match_device(&devices[i].devid(), devices[i].flags) == MatchKind::Explicit
        })
        .collect();
    if !explicit.is_empty() {
        device_label(devices, &explicit)
    } else if ids.has_wildcard() {
        WILDCARD_LABEL.to_string()
    } else {
        String::new()
    }
}

/// Stable board-header label for a wildcard (`[ids] *`) config.
pub(crate) const WILDCARD_LABEL: &str = "all keyboards";

/// The result of deciding what to show: the sheet sources (rebuildable so the picker
/// can change geometry), a `vendor:product → sheet-index` map for following the
/// last-pressed keyboard, and a subtitle.
pub(crate) struct Detection {
    pub(crate) srcs: Vec<SheetSrc>,
    /// `(vendor:product, index into srcs)` for each matched connected keyboard.
    pub(crate) device_map: Vec<(String, i32)>,
    pub(crate) subtitle: String,
    /// Keep following connected keyboards while running (re-highlight `[ids]` and refresh
    /// the device map on hotplug)? True for the auto-detect paths; false for explicit path
    /// args and QMK import, which show exactly what was asked for.
    pub(crate) live_devices: bool,
    /// Load-time `[ids]` collision warnings: human lines for any device id (or the
    /// wildcard) two `/etc/keyd` configs claim at the same rank — a misconfiguration
    /// keyd resolves nondeterministically by file order (edit-mode design §5.5). Empty
    /// for the non-`/etc/keyd` paths (args/examples/QMK), which aren't the user's live set.
    pub(crate) id_warnings: Vec<String>,
}

impl Detection {
    /// An auto-detect result that keeps following devices (used by the detection fallbacks).
    fn new(srcs: Vec<SheetSrc>, subtitle: String) -> Self {
        Detection {
            srcs,
            device_map: Vec::new(),
            subtitle,
            live_devices: true,
            id_warnings: Vec::new(),
        }
    }
}

/// Turn the core's structured `[ids]` conflicts into one human warning line each,
/// naming the contesting files (by base name) and the contested id. The core finds
/// the clashes (pure, tested); this just phrases them.
fn format_id_conflicts(configs: &[(PathBuf, Config)], conflicts: &[IdConflict]) -> Vec<String> {
    let file_name = |i: &usize| -> String {
        configs
            .get(*i)
            .map(|(p, _)| {
                p.file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "(unknown)".to_string())
    };
    conflicts
        .iter()
        .map(|c| {
            let files: Vec<String> = c.configs.iter().map(file_name).collect();
            let joined = match files.len() {
                0 | 1 => files.first().cloned().unwrap_or_default(),
                2 => format!("{} and {}", files[0], files[1]),
                n => format!("{} and {}", files[..n - 1].join(", "), files[n - 1]),
            };
            let what = match c.kind {
                MatchKind::Wildcard => {
                    "the wildcard `[ids] *` (any keyboard no other config claims)".to_string()
                }
                _ => format!("device id {}", c.id),
            };
            format!(
                "\u{26a0} {joined} both claim {what} — keyd picks one by file order \
                 (nondeterministic); remove the duplicate."
            )
        })
        .collect()
}

/// The value following `--flag` on the command line, if present.
pub(crate) fn flag_value(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// `--qmk-info <info.json>` path: render a keyd config on a board imported from QMK.
///
/// The geometry + key identities come from QMK (`info.json` zipped index-wise with the
/// default keymap's keycodes); the keyd bindings come from the first positional
/// `*.conf` (or an empty config — a plain labeled board — if none is given). Optional
/// `--qmk-keymap <keymap.json>` supplies identities; `--qmk-layout <NAME>` picks the
/// variant when a board defines several.
pub(crate) fn qmk_detection(info_path: &str) -> Result<Detection, String> {
    let info = std::fs::read_to_string(info_path).map_err(|e| format!("{info_path}: {e}"))?;
    let keymap = match flag_value("--qmk-keymap") {
        Some(p) => Some(std::fs::read_to_string(&p).map_err(|e| format!("{p}: {e}"))?),
        None => None,
    };
    let prefer = flag_value("--qmk-layout");
    let imp = import_qmk(&info, keymap.as_deref(), prefer.as_deref())?;

    // Overlay config: the first positional *.conf, else an empty config (plain board).
    let conf = std::env::args().skip(1).find(|a| a.ends_with(".conf"));
    let (source, cfg) = match &conf {
        Some(p) => (p.clone(), parse_file(Path::new(p)).map_err(|e| format!("{p}: {e}"))?),
        None => ("(no config)".to_string(), parse_text("")),
    };

    let profile = format!("QMK · {}", imp.layout_name);
    let unmapped = if imp.unmapped > 0 {
        format!(" \u{2014} {} slot(s) unmapped", imp.unmapped)
    } else {
        String::new()
    };
    let subtitle = format!(
        "QMK import: {} ({} keys){unmapped}",
        imp.layout_name,
        imp.geometry.slots.len()
    );
    let src = SheetSrc {
        path: PathBuf::from(source),
        cfg,
        device: String::new(),
        matched_ids: Vec::new(),
        layout_id: String::new(),
        qmk: Some((imp.geometry, profile)),
    };
    Ok(Detection {
        srcs: vec![src],
        device_map: Vec::new(),
        subtitle,
        live_devices: false,
        id_warnings: Vec::new(),
    })
}

/// Decide which sheets to render, the device→sheet map, and a subtitle.
///
/// - Explicit path args  → render exactly those configs (no device map).
/// - Otherwise           → glob `/etc/keyd/*.conf`, detect connected keyboards,
///   and render only the matching configs (labeled with the device). If nothing
///   matches, fall back to showing all configs. If `/etc/keyd` is empty, fall back
///   to the bundled examples.
pub(crate) fn gather_sheets() -> Detection {
    // Positional config paths: skip flags and the value that follows a value-flag
    // (only `--layout` reaches this path; the `--qmk-*` flags route to qmk_detection).
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut args: Vec<PathBuf> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            // value-flags: skip the flag *and* its argument so the value isn't picked up
            // as a positional config path.
            "--layout" | "--helper-socket" => i += 2,
            a if a.starts_with('-') => i += 1,
            a => {
                args.push(PathBuf::from(a));
                i += 1;
            }
        }
    }
    if !args.is_empty() {
        let srcs: Vec<SheetSrc> = parse_configs(&args)
            .iter()
            .map(|(p, c)| SheetSrc::catalog(p, c, "", Vec::new()))
            .collect();
        let n = srcs.len();
        return Detection {
            srcs,
            device_map: Vec::new(),
            subtitle: format!("{n} config(s) from arguments"),
            live_devices: false,
            id_warnings: Vec::new(),
        };
    }

    let conf_paths = conf_files_in(Path::new("/etc/keyd"));
    if conf_paths.is_empty() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let srcs: Vec<SheetSrc> = parse_configs(&conf_files_in(&examples))
            .iter()
            .map(|(p, c)| SheetSrc::catalog(p, c, "", Vec::new()))
            .collect();
        let n = srcs.len();
        return Detection::new(
            srcs,
            format!("{n} example keyboard(s) \u{2014} no /etc/keyd configs found"),
        );
    }

    let configs = parse_configs(&conf_paths);
    let matchers: Vec<Ids> = configs.iter().map(|(_, c)| Ids::parse(&c.ids)).collect();
    // Same-rank `[ids]` clashes across the live config set — surface on load rather
    // than silently inheriting keyd's file-order pick (§5.5). A property of the files,
    // so computed whether or not a keyboard is currently connected.
    let id_warnings = format_id_conflicts(&configs, &find_conflicts(&matchers));
    let devices = devices::connected_devices();

    // Assign each connected device to its best-matching config (explicit > wildcard).
    let mut per_config: Vec<Vec<usize>> = vec![Vec::new(); configs.len()];
    for (di, dev) in devices.iter().enumerate() {
        let devid = dev.devid();
        let mut best: Option<(usize, u8)> = None;
        for (ci, ids) in matchers.iter().enumerate() {
            let rank = ids.match_device(&devid, dev.flags).rank();
            if rank > 0 && best.is_none_or(|(_, br)| rank > br) {
                best = Some((ci, rank));
            }
        }
        if let Some((ci, _)) = best {
            per_config[ci].push(di);
        }
    }

    let matched_any = per_config.iter().any(|v| !v.is_empty());
    if !matched_any {
        // No connected keyboard matched — show everything rather than nothing.
        let srcs: Vec<SheetSrc> =
            configs.iter().map(|(p, c)| SheetSrc::catalog(p, c, "", Vec::new())).collect();
        let n = srcs.len();
        let mut det =
            Detection::new(srcs, format!("{n} config(s) \u{2014} no connected keyboard detected"));
        det.id_warnings = id_warnings;
        return det;
    }

    let mut srcs = Vec::new();
    let mut device_map: Vec<(String, i32)> = Vec::new();
    for (ci, (path, cfg)) in configs.iter().enumerate() {
        if per_config[ci].is_empty() {
            continue;
        }
        let idx = srcs.len() as i32;
        let label = config_label(&matchers[ci], &devices, &per_config[ci]);
        // Concrete ids of the keyboards that matched this config (deduped) — drives both
        // the device→sheet map and the "which [ids] entry is plugged in" highlight.
        let mut matched_ids: Vec<String> = Vec::new();
        for &di in &per_config[ci] {
            let devid = devices[di].devid();
            if !matched_ids.contains(&devid) {
                matched_ids.push(devid);
            }
        }
        srcs.push(SheetSrc::catalog(path, cfg, &label, matched_ids.clone()));
        for devid in matched_ids {
            if !device_map.iter().any(|(d, _)| *d == devid) {
                device_map.push((devid, idx));
            }
        }
    }
    let n = srcs.len();
    Detection {
        srcs,
        device_map,
        subtitle: format!("{n} connected keyboard(s) detected"),
        live_devices: true,
        id_warnings,
    }
}

/// Per-source `(matched vendor:product ids, device label)`, plus a `vendor:product → sheet`
/// map — the result of (re)matching connected keyboards against the sheet sources.
pub(crate) type DeviceMatching = (Vec<(Vec<String>, String)>, Vec<(String, i32)>);

/// Re-scan connected keyboards and re-match them against the current sheet sources,
/// returning per-source `(matched ids, device label)` and a fresh `vendor:product → sheet`
/// map. Same matching as [`gather_sheets`], but over the already-chosen sources — so a
/// hotplugged keyboard refreshes the id highlight, the device label, and the
/// follow-keyboard map without re-deciding which configs are shown.
pub(crate) fn rescan(srcs: &[SheetSrc]) -> DeviceMatching {
    let matchers: Vec<Ids> = srcs.iter().map(|s| Ids::parse(&s.cfg.ids)).collect();
    let devices = devices::connected_devices();
    let mut per_src: Vec<Vec<usize>> = vec![Vec::new(); srcs.len()];
    for (di, dev) in devices.iter().enumerate() {
        let devid = dev.devid();
        let mut best: Option<(usize, u8)> = None;
        for (ci, ids) in matchers.iter().enumerate() {
            let rank = ids.match_device(&devid, dev.flags).rank();
            if rank > 0 && best.is_none_or(|(_, br)| rank > br) {
                best = Some((ci, rank));
            }
        }
        if let Some((ci, _)) = best {
            per_src[ci].push(di);
        }
    }
    let mut out = Vec::with_capacity(srcs.len());
    let mut device_map: Vec<(String, i32)> = Vec::new();
    for (ci, idxs) in per_src.iter().enumerate() {
        let label = config_label(&matchers[ci], &devices, idxs);
        let mut ids: Vec<String> = Vec::new();
        for &di in idxs {
            let devid = devices[di].devid();
            if !ids.contains(&devid) {
                ids.push(devid.clone());
            }
            if !device_map.iter().any(|(d, _)| *d == devid) {
                device_map.push((devid, ci as i32));
            }
        }
        out.push((ids, label));
    }
    (out, device_map)
}

#[cfg(test)]
mod id_conflict_tests {
    use super::format_id_conflicts;
    use keydviz_core::{find_conflicts, parse_text, Config, Ids, MatchKind, IdConflict};
    use std::path::PathBuf;

    fn cfg(ids_block: &str) -> Config {
        parse_text(&format!("[ids]\n{ids_block}\n\n[main]\n"))
    }

    /// Build the `(path, Config)` list + run the real detector, mirroring gather_sheets.
    fn run(files: &[(&str, &str)]) -> Vec<String> {
        let configs: Vec<(PathBuf, Config)> =
            files.iter().map(|(name, ids)| (PathBuf::from(name), cfg(ids))).collect();
        let matchers: Vec<Ids> = configs.iter().map(|(_, c)| Ids::parse(&c.ids)).collect();
        format_id_conflicts(&configs, &find_conflicts(&matchers))
    }

    #[test]
    fn names_the_files_and_the_contested_id() {
        let w = run(&[("/etc/keyd/a.conf", "04fe:0021"), ("/etc/keyd/b.conf", "04fe:0021")]);
        assert_eq!(w.len(), 1);
        // Base file names only (not the full path), and the contested id.
        assert!(w[0].contains("a.conf and b.conf"), "{}", w[0]);
        assert!(w[0].contains("device id 04fe:0021"), "{}", w[0]);
        assert!(w[0].contains("file order"), "{}", w[0]);
    }

    #[test]
    fn wildcard_clash_phrasing() {
        let w = run(&[("a.conf", "*"), ("b.conf", "04fe:0021"), ("c.conf", "*")]);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("a.conf and c.conf"), "{}", w[0]);
        assert!(w[0].contains("wildcard"), "{}", w[0]);
    }

    #[test]
    fn clean_config_set_warns_nothing() {
        // The normal layering: one specific config + one wildcard catch-all.
        assert!(run(&[("hhkb.conf", "04fe:0021"), ("default.conf", "*")]).is_empty());
    }

    #[test]
    fn three_way_clash_joins_with_commas() {
        let w = run(&[("a.conf", "aa:bb"), ("b.conf", "aa:bb"), ("c.conf", "aa:bb")]);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("a.conf, b.conf and c.conf"), "{}", w[0]);
    }

    #[test]
    fn out_of_range_index_degrades_gracefully() {
        // Defensive: a stale index can't panic, just labels "(unknown)".
        let configs: Vec<(PathBuf, Config)> = vec![(PathBuf::from("a.conf"), cfg("*"))];
        let bogus = vec![IdConflict { id: "x".into(), kind: MatchKind::Explicit, configs: vec![0, 9] }];
        let w = format_id_conflicts(&configs, &bogus);
        assert!(w[0].contains("a.conf and (unknown)"), "{}", w[0]);
    }
}
