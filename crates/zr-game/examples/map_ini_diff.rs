//! Shows how a map's `map.ini` changes a template: `map_ini_diff <map> <template>`
//! (e.g. `md_usa05 AmericaStrategyCenter`). Prints the differing lines of the pretty-printed template.
//!
//! `cargo run --release -p zr-game --example map_ini_diff -- md_usa05 AmericaStrategyCenter`

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::{game::map_path, GameData};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [map, template] = args.as_slice() else {
        eprintln!("usage: map_ini_diff <map> <template>");
        return;
    };
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let base = Arc::new(GameData::load(&vfs));
    let d = base.for_map(&vfs, &map_path(map));
    println!("map.ini files: {:?}", d.map_ini_files);
    let show = |g: &GameData| g.templates().thing(template).map(|t| format!("{t:#?}")).unwrap_or_default();
    let (a, b) = (show(&base), show(&d));
    let (a, b): (Vec<_>, Vec<_>) = (a.lines().collect(), b.lines().collect());
    for l in &a {
        if !b.contains(l) {
            println!("- {l}");
        }
    }
    for l in &b {
        if !a.contains(l) {
            println!("+ {l}");
        }
    }
}
