//! Air locomotor demo on Alpine Assault: an America Comanche (HOVER) and Raptor (WINGS) fly a
//! `MoveTo` across the map, ignoring the ground pathfinder and holding their preferred height.
//!
//! `cargo run --release -p zr-game --example air_demo` (needs a Zero Hour install, or
//! `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::{Command, Coord3D};

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));

    let setups = [PlayerSetup::new("FactionAmerica", 0), PlayerSetup::new("FactionChina", 1)];
    let mut g = Game::new_skirmish_with(data.clone(), &vfs, "Alpine Assault", &setups, 1).expect("start skirmish");
    let p0 = g.skirmish_player(0).expect("player0").index;
    let base = g.spawned().iter().find(|s| s.player == p0 && s.role == "StartingBuilding").expect("base").pos;

    // Clear of the starting building: air locomotors are force-driven and now collide like the
    // engine's, so spawning inside the command center's footprint would pin them.
    let start = Coord3D::new(base.x + 160.0, base.y + 160.0, 0.0);
    let goal = Coord3D::new(base.x - 500.0, base.y - 500.0, 0.0);
    let comanche = g.spawn_unit(p0, "AmericaVehicleComanche", start, 0.0).expect("comanche template");
    let raptor = g.spawn_unit(p0, "AmericaJetRaptor", Coord3D::new(start.x + 20.0, start.y, 0.0), 0.0).expect("raptor template");
    println!("spawned Comanche {} (HOVER) and Raptor {} (WINGS) at ({:.0}, {:.0})", comanche.0, raptor.0, start.x, start.y);

    for (name, id) in [("Comanche", comanche), ("Raptor", raptor)] {
        let loco = g.logic().find_object(id).and_then(|o| o.locomotor());
        if let Some(l) = loco {
            println!(
                "{name}: appearance {:?} air {} z {:?} stick {} lift {:.4} preferred height {} speed {:.2}/frame min {:.2} turn {:.3} rad/frame",
                l.kind, l.is_airborne(), l.behavior_z, l.stick_to_ground, l.lift, l.preferred_height, l.max_speed, l.min_speed, l.max_turn_rate
            );
        }
        g.submit(p0, Command::MoveTo { ids: vec![id], pos: goal });
    }

    for sec in 0..=10 {
        if sec > 0 {
            g.run_frames(30);
        }
        for (name, id) in [("Comanche", comanche), ("Raptor", raptor)] {
            if let Some(o) = g.logic().find_object(id) {
                println!(
                    "  t={sec:>2}s {name:<8} pos ({:8.1}, {:8.1}, {:6.1}) alt {:6.1} speed {:.2}/frame",
                    o.position.x,
                    o.position.y,
                    o.position.z,
                    o.position.z - g.logic().terrain().ground_height(o.position.x, o.position.y),
                    o.ai.speed
                );
            }
        }
    }
    println!("crc {:08x}", g.crc());
}
