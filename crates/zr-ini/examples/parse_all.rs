//! Parses every shipped `.ini` file of the player's Zero Hour install.
//!
//! Usage: `cargo run --release -p zr-ini --example parse_all [-- Object Name]`
//! prints the summary, and a dump of `Object <Name>` (default
//! `AmericaTankCrusader`).

use std::collections::BTreeMap;
use zr_assets::{Installs, Vfs};

fn main() {
    let sample = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "AmericaTankCrusader".into());
    let install = Installs::detect()
        .zero_hour
        .expect("Zero Hour install not found");
    let vfs = Vfs::zero_hour(&install).expect("mounting Zero Hour");

    let zr_ini::Loaded {
        db,
        files,
        bytes,
        map_override_blocks: map_blocks,
        failures,
        legacy,
        language,
        order,
        unreferenced,
    } = zr_ini::load_zero_hour(&vfs);
    println!(
        "{files} .ini files ({} KiB): {} parsed, {} failed, {} unloaded legacy",
        bytes / 1024,
        files - failures.len() - legacy.len(),
        failures.len(),
        legacy.len()
    );
    println!(
        "engine load order ({language}): {} files merged; {} never reached by the engine",
        order.len(),
        unreferenced.len()
    );
    for p in &unreferenced {
        println!("  (unreferenced) {p}");
    }
    for e in &failures {
        println!("  FAIL {e}");
    }
    for e in &legacy {
        println!("  (legacy, never loaded by the engine) {e}");
    }
    let nested: usize = db.iter().map(|b| b.descendant_count()).sum();
    println!("{} top-level blocks, {nested} nested blocks in base data (+{map_blocks} top-level in map.ini overrides)\n", db.len());

    let unique: BTreeMap<_, _> = db
        .counts()
        .into_iter()
        .map(|(k, n)| {
            let u = db.count_unique(&k);
            (k, (n, u))
        })
        .collect();
    println!("{:<34}{:>8}{:>8}", "block type", "defs", "unique");
    for (k, (n, u)) in unique {
        println!("{k:<34}{n:>8}{u:>8}");
    }

    match db.get("Object", &sample) {
        Some(b) => println!(
            "\n--- Object {sample} ({} definition(s)) ---\n{b}",
            db.definitions("Object", &sample).len()
        ),
        None => println!("\nObject {sample} not found"),
    }
}
