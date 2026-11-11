//! Real-data bridges: loads Alpine Assault and Silent River, reports every bridge the sim built
//! (from/to/width/deck height) and drives a dozer across one. Skips when no install is found.
//!
//! `cargo run --release -p zr-game --example bridge_demo` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::{Command, Coord3D, LOGICFRAMES_PER_SECOND};

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));
    for map in ["Alpine Assault", "Silent River"] {
        let setups = [PlayerSetup::new("FactionAmerica", 0), PlayerSetup::new("FactionChina", 1)];
        let mut g = match Game::new_skirmish_with(data.clone(), &vfs, map, &setups, 1) {
            Ok(g) => g,
            Err(e) => {
                println!("{map}: {e:#}");
                continue;
            }
        };
        let p1 = g.skirmish_player(0).expect("player0").index;
        let bridges = g.logic().terrain().bridges().to_vec();
        println!("{map}: {} bridges", bridges.len());
        for b in &bridges {
            let mid = Coord3D::new((b.from.x + b.to.x) * 0.5, (b.from.y + b.to.y) * 0.5, 0.0);
            let ground = g.logic().terrain().ground_height(mid.x, mid.y);
            let deck = g.logic().terrain().layer_height(mid.x, mid.y, b.layer);
            let len = ((b.to.x - b.from.x).powi(2) + (b.to.y - b.from.y).powi(2)).sqrt();
            println!(
                "  layer {}: ({:.0}, {:.0}) -> ({:.0}, {:.0}), length {:.0}, width {:.0}, deck {:.1} (ground {:.1})",
                b.layer, b.from.x, b.from.y, b.to.x, b.to.y, len, b.width, deck, ground
            );

            // Drive a dozer from before the from-side to beyond the to-side.
            let (dx, dy) = (b.to.x - b.from.x, b.to.y - b.from.y);
            let l = (dx * dx + dy * dy).sqrt().max(1.0);
            let (ax, ay) = (dx / l, dy / l);
            let start = Coord3D::new(b.from.x - ax * 90.0, b.from.y - ay * 90.0, 0.0);
            let goal = Coord3D::new(b.to.x + ax * 90.0, b.to.y + ay * 90.0, 0.0);
            let start = Coord3D::new(start.x, start.y, g.logic().terrain().surface_height(start.x, start.y, b.from.z));
            let Some(id) = g.spawn_unit(p1, "AmericaVehicleDozer", start, 0.0) else { continue };
            g.submit(p1, Command::MoveTo { ids: vec![id], pos: goal });
            g.run_frames(1);
            let path = g.logic().find_object(id).unwrap().ai.path.clone();
            let max_z = path.iter().fold(f32::MIN, |m, p| m.max(p.z));
            let mut len_path = 0.0;
            let mut prev = start;
            for p in &path {
                len_path += prev.dist_2d(p);
                prev = *p;
            }
            println!(
                "    dozer: {} waypoints, length {:.0}, max waypoint z {:.1} (deck {:.1}), straight {:.0}",
                path.len(),
                len_path,
                max_z,
                b.from.z,
                start.dist_2d(&goal)
            );
            let mut frames = 0;
            while g.logic().find_object(id).is_some_and(|o| o.ai.is_moving()) && frames < 60 * LOGICFRAMES_PER_SECOND {
                g.run_frames(LOGICFRAMES_PER_SECOND);
                frames += LOGICFRAMES_PER_SECOND;
            }
            let end = g.object(id).unwrap().position;
            println!("    after {} s: ({:.0}, {:.0}), {:.0} from goal, CRC {:08x}", frames / LOGICFRAMES_PER_SECOND, end.x, end.y, end.dist_2d(&goal), g.crc());
        }
        // Destroy every bridge object and report the pathfinder/CRC effect.
        let towers = g.objects().filter(|o| o.template.contains("BridgeTower")).count();
        let before = g.crc();
        for id in g.bridge_object_ids() {
            let hp = g.object(id).map_or(0.0, |o| o.health);
            g.damage_object(id, hp + 1.0);
        }
        let destroyed = (0..bridges.len()).filter(|&i| g.bridge_destroyed(i).unwrap_or(false)).count();
        println!("  {towers} towers; destroyed {destroyed}/{} bridges (CRC {before:08x} -> {:08x})", bridges.len(), g.crc());
    }
}
