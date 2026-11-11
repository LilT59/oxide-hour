//! Survey of every shipped map: `map.ini` overlays (blocks and, for skirmish maps, what changes
//! in the typed templates) and water (areas, cells the pathfinder classifies as water).
//!
//! `cargo run --release -p zr-game --example map_survey` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use std::time::Instant;
use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_map::MapFile;
use zr_sim::pathfind::{CellType, Pathfinder};
use zr_sim::TerrainLogic;

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let t = Instant::now();
    let data = std::sync::Arc::new(GameData::load(&vfs));
    println!("GameData::load: {:?}", t.elapsed());
    let maps: Vec<String> = vfs.files().map(|(p, _)| p.to_owned()).filter(|p| p.starts_with("maps\\") && p.ends_with(".map")).collect();
    let (mut n_ini, mut n_water) = (0, 0);
    for m in &maps {
        let Some(raw) = vfs.read(m) else { continue };
        let Ok(bytes) = zr_assets::compression::decompress_if_needed(&raw) else { continue };
        let Ok(map) = MapFile::parse(&bytes) else { continue };
        let skirmish = map.objects.iter().any(|o| o.props.str("waypointName") == Some("Player_1_Start"));

        // map.ini
        let t = Instant::now();
        let d = data.for_map(&vfs, m);
        let dt = t.elapsed();
        let mut line = String::new();
        if !d.map_ini_files.is_empty() {
            n_ini += 1;
            let mut db = zr_ini::load_zero_hour(&vfs).db;
            let ov = zr_ini::load_map_ini(&mut db, &vfs, m);
            let mut kinds: Vec<String> = db.iter().skip(ov.checkpoint).map(|b| format!("{} {}", b.kind, b.name().unwrap_or(""))).collect();
            kinds.truncate(6);
            line = format!(" map.ini [{}] for_map {:?}: {}", d.map_ini_files.len(), dt, kinds.join("; "));
            if skirmish {
                let mut diffs = Vec::new();
                for b in db.iter().skip(ov.checkpoint) {
                    let n = b.name().unwrap_or("");
                    match b.kind.as_str() {
                        "Object" | "ObjectReskin" => {
                            let (a, c) = (data.templates().thing(n), d.templates().thing(n));
                            match (a, c) {
                                (None, Some(_)) => diffs.push(format!("+{n}")),
                                (Some(a), Some(c)) if format!("{a:?}") != format!("{c:?}") => diffs.push(format!("~{n}")),
                                _ => {}
                            }
                        }
                        "Weapon" => {
                            let find = |t: &zr_templates::Templates| t.weapon(n).map(|w| format!("{w:?}"));
                            if find(data.templates()) != find(d.templates()) {
                                diffs.push(format!("~weapon {n}"));
                            }
                        }
                        _ => {}
                    }
                }
                if !diffs.is_empty() {
                    line.push_str(&format!("\n    template changes: {}", diffs.join(", ")));
                }
            }
        }

        // water
        let mut wline = String::new();
        if let (Some(hm), Some(tr)) = (&map.height_map, &map.triggers) {
            let mut terrain = TerrainLogic::from_height_map(hm);
            terrain.set_water_from_triggers(tr);
            if !terrain.water_areas().is_empty() {
                let t = Instant::now();
                let pf = Pathfinder::new(&terrain);
                let water = pf.cell_type_count(CellType::Water);
                wline = format!(" water areas {} -> {} water cells (classify {:?})", terrain.water_areas().len(), water, t.elapsed());
                if water > 0 {
                    n_water += 1;
                }
            }
        }
        if !line.is_empty() || (skirmish && !wline.is_empty()) {
            println!("{}{}{}{}", m, if skirmish { " [skirmish]" } else { "" }, line, wline);
        }
    }
    println!("{} maps, {} with map.ini overlays, {} with water cells", maps.len(), n_ini, n_water);
}
