//! Smoke tool: parse a keyd config and print a summary of what was understood.
//! Run with: `cargo run -p keydviz-core --example dump -- examples/hhkb.conf`

use std::path::Path;

use keydviz_core::{layout_for, parse_file};

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: dump <keyd.conf>");
        std::process::exit(2);
    });
    let cfg = parse_file(Path::new(&path)).unwrap_or_else(|e| {
        eprintln!("error reading {path}: {e}");
        std::process::exit(1);
    });
    let (_, profile) = layout_for(&path);

    println!("{path}  [{profile}]");
    println!("  ids:    {:?}", cfg.ids);
    println!("  holds:  {}", cfg.holds.len());
    for h in &cfg.holds {
        println!("    {} -> {} ({:?}, tap={:?})", h.key, h.target, h.kind, h.tap);
    }
    println!("  chords: {:?}", cfg.chords);
    println!("  remaps: {:?}", cfg.remaps);
    println!("  layers:");
    for layer in &cfg.layers {
        println!("    [{}] {} keys", layer.name, layer.keys.len());
    }
}
