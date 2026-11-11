//! Chinook combat drop demo on Alpine Assault: load a squad of Rangers into an
//! `AmericaVehicleChinook`, order a combat drop at a ground position, and print the state machine
//! (`MOVE_TO_COMBAT_DROP` -> `DO_COMBAT_DROP`), the rope lengths and each rappeller's descent until
//! the squad is on the ground; then fly the Chinook away.
//!
//! `cargo run --release -p zr-game --example chinook_demo` (needs a Zero Hour install, or
//! `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::{chinook::ChinookPhase, Command, Coord3D, TemplateSource};

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

    if let Some(t) = data.store.find_template("AmericaVehicleChinook").and_then(|t| t.chinook) {
        println!(
            "ChinookAIUpdate: NumRopes {} RopeDropSpeed {:.2}/f RappelSpeed {:.2}/f PerRopeDelay {}..{}f MinDropHeight {:.0} WaitForRopesToDrop {} RopeFinalHeight {:.0}",
            t.num_ropes, t.rope_drop_speed, t.rappel_speed, t.per_rope_delay_min, t.per_rope_delay_max, t.min_drop_height, t.wait_for_ropes_to_drop, t.rope_final_height
        );
    }

    let start = Coord3D::new(base.x + 150.0, base.y + 60.0, 0.0);
    let chinook = g.spawn_unit(p0, "AmericaVehicleChinook", start, 0.0).expect("Chinook template");
    let mut riders = Vec::new();
    for i in 0..4 {
        let pos = Coord3D::new(start.x + 12.0 + i as f32 * 4.0, start.y, 0.0);
        riders.push(g.spawn_unit(p0, "AmericaInfantryRanger", pos, 0.0).expect("Ranger template"));
    }
    g.submit(p0, Command::Enter { ids: riders.clone(), container: chinook });
    g.run_frames(150);
    let boarded = g.logic().find_object(chinook).map_or(0, |o| o.contained.len());
    println!("boarded {boarded}/4 Rangers into Chinook {}", chinook.0);

    // A ground drop a few hundred units from the base.
    let target = Coord3D::new(base.x + 420.0, base.y + 120.0, 0.0);
    if !g.logic_mut().start_combat_drop(chinook, target, None) {
        eprintln!("start_combat_drop refused");
        return;
    }
    println!("combat drop ordered to ({:.0}, {:.0})", target.x, target.y);

    for sec in 0..=30 {
        if sec > 0 {
            g.run_frames(30);
        }
        let Some(o) = g.logic().find_object(chinook) else { break };
        let st = g.logic().chinook_state(chinook);
        let phase = st.map_or("gone", |s| match s.phase {
            ChinookPhase::MoveToDrop => "MOVE",
            ChinookPhase::CombatDrop => "DROP",
        });
        let ropes = st.map_or(0.0, |s| s.ropes.iter().map(|r| r.len).fold(0.0f32, f32::max));
        let mut on_ground = 0;
        for &r in &riders {
            if let Some(u) = g.logic().find_object(r) {
                let ground = g.logic().terrain().ground_height(u.position.x, u.position.y);
                if u.contained_by.is_none() && (u.position.z - ground).abs() < 1.0 {
                    on_ground += 1;
                }
            }
        }
        println!(
            "  t={sec:>2}s {phase:<4} pos ({:7.1}, {:7.1}, {:6.1}) alt {:6.1} longest rope {:5.1} rappelling {} on-ground {on_ground}/4",
            o.position.x,
            o.position.y,
            o.position.z,
            o.position.z - g.logic().terrain().ground_height(o.position.x, o.position.y),
            ropes,
            g.logic().rappelling_count()
        );
        if st.is_none() {
            break;
        }
    }

    // The Chinook leaves after the drop (the state machine goes idle at the end).
    g.submit(p0, Command::MoveTo { ids: vec![chinook], pos: Coord3D::new(base.x, base.y, 0.0) });
    g.run_frames(90);
    if let Some(o) = g.logic().find_object(chinook) {
        println!("Chinook leaves to ({:.0}, {:.0}) at ({:.0}, {:.0})", base.x, base.y, o.position.x, o.position.y);
    }
    println!("crc {:08x}", g.crc());
}
