//! Containment demo on Alpine Assault: spawn an America Humvee and two Rangers, order them to
//! board, drive the Humvee, then evacuate. Prints the frames and positions.
//!
//! `cargo run --release -p zr-game --example contain_demo` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

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
    let mut g = Game::new_skirmish_with(data, &vfs, "Alpine Assault", &setups, 1).expect("start skirmish");
    let p0 = g.skirmish_player(0).expect("player0").index;

    // Spawn next to the America command center.
    let base = g
        .spawned()
        .iter()
        .find(|s| s.player == p0 && s.template.contains("CommandCenter"))
        .or_else(|| g.spawned().iter().find(|s| s.player == p0))
        .map(|s| s.pos)
        .expect("player 0 start position");
    let Some(humvee) = ["AmericaVehicleHumvee", "AmericaVehicleHumVee"]
        .iter()
        .find_map(|n| g.spawn_object(n, Coord3D::new(base.x + 200.0, base.y + 200.0, base.z), 0.0, p0))
    else {
        eprintln!("no America Humvee template found");
        return;
    };
    let Some(r1) = g.spawn_object("AmericaInfantryRanger", Coord3D::new(base.x + 280.0, base.y + 210.0, base.z), 0.0, p0) else {
        eprintln!("no AmericaInfantryRanger template found");
        return;
    };
    let Some(r2) = g.spawn_object("AmericaInfantryRanger", Coord3D::new(base.x + 280.0, base.y + 190.0, base.z), 0.0, p0) else {
        eprintln!("no AmericaInfantryRanger template found");
        return;
    };
    println!("spawned humvee {} rangers {} {}", humvee.0, r1.0, r2.0);
    if let Some(t) = g.logic().find_object(humvee).and_then(|o| o.template.contain.clone()) {
        println!("humvee contain kind {:?} capacity {}", t.kind, t.capacity);
    }
    if let Some(o) = g.logic().find_object(r1) {
        println!("ranger transport slots {}", o.template.transport_slot_count);
    }

    g.submit(p0, Command::Enter { ids: vec![r1, r2], container: humvee });
    let mut boarded_at = None;
    for _ in 0..300 {
        g.step();
        if g.logic().contained_units(humvee).len() == 2 {
            boarded_at = Some(g.frame());
            break;
        }
    }
    println!("boarded {} of 2 rangers at frame {:?}", g.logic().contained_units(humvee).len(), boarded_at);

    // Drive the transport; the passengers follow.
    let goal = Coord3D::new(base.x + 300.0, base.y + 200.0, 0.0);
    g.submit(p0, Command::MoveTo { ids: vec![humvee], pos: goal });
    g.run_frames(120);
    let hp = g.object(humvee).unwrap().position;
    let rp = g.object(r1).unwrap().position;
    println!("after driving: humvee ({:.1}, {:.1}), passenger ({:.1}, {:.1})", hp.x, hp.y, rp.x, rp.y);

    g.submit(p0, Command::Evacuate { container: humvee });
    g.run_frames(10);
    println!("after evacuate: contained {}", g.logic().contained_units(humvee).len());
    for (name, id) in [("ranger1", r1), ("ranger2", r2)] {
        if let Some(o) = g.object(id) {
            println!("  {name} at ({:.1}, {:.1}), {:.1} from the humvee", o.position.x, o.position.y, o.position.dist_2d(&hp));
        }
    }
}
