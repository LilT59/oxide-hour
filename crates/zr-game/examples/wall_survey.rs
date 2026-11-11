//! Survey of every shipped map for `KINDOF_WALK_ON_TOP_OF_WALL` objects (China walls), so the
//! wall-layer code has a real-data target.
//!
//! `cargo run --release -p zr-game --example wall_survey` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_map::MapFile;
use zr_templates::KindOf;

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = GameData::load(&vfs);

    // Which templates are wall pieces?
    let wall_templates: Vec<String> = data.templates().things().iter().filter(|t| t.is_kind_of(KindOf::WalkOnTopOfWall)).map(|t| t.name.clone()).collect();
    println!("{} walk-on-wall templates: {:?}", wall_templates.len(), wall_templates);

    let maps: Vec<String> = vfs.files().map(|(p, _)| p.to_owned()).filter(|p| p.starts_with("maps\\") && p.ends_with(".map")).collect();
    let mut with_walls = 0;
    for m in &maps {
        let Some(raw) = vfs.read(m) else { continue };
        let Ok(bytes) = zr_assets::compression::decompress_if_needed(&raw) else { continue };
        let Ok(map) = MapFile::parse(&bytes) else { continue };
        let mut n = 0usize;
        let mut names: Vec<String> = Vec::new();
        for o in &map.objects {
            if data.templates().thing(&o.template).is_some_and(|t| t.is_kind_of(KindOf::WalkOnTopOfWall)) {
                n += 1;
                if !names.iter().any(|x| x == &o.template) {
                    names.push(o.template.clone());
                }
            }
        }
        if n > 0 {
            with_walls += 1;
            println!("{m}: {n} wall pieces {:?}", names);
        }
    }
    println!("{} maps, {} with wall pieces", maps.len(), with_walls);
}
