//! Real-data exercise of the `Collide` modules on Alpine Assault (GLA vs China): a GLA tank picks
//! up a `SalvageCrate` (weapon-set upgrade), a money crate pays out, and a GLA tank squishes enemy
//! infantry. All three go through `GameLogic::process_object_collisions` (`PartitionManager`'s
//! contact list + `Object::onCollide`).
//!
//! `cargo run --release -p zr-game --example collide_demo` (needs a Zero Hour install, or
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

    let setups = [PlayerSetup::new("FactionGLA", 0), PlayerSetup::new("FactionChina", 1)];
    let mut g = Game::new_skirmish_with(data.clone(), &vfs, "Alpine Assault", &setups, 1).expect("start skirmish");
    let gla = g.skirmish_player(0).expect("GLA player").index;
    let china = g.skirmish_player(1).expect("China player").index;
    // `SalvageCrate` requires `PickupScience = SCIENCE_GLA`; the sim does not yet grant the
    // `PlayerTemplate` `IntrinsicSciences`, so grant it here.
    g.logic_mut().add_science(gla, "SCIENCE_GLA");

    let base = g.spawned().iter().find(|s| s.player == gla && s.role == "StartingBuilding").expect("GLA base").pos;

    // ---- salvage crate ----------------------------------------------------------------------
    let salvager = g
        .spawn_unit(gla, "GLATankScorpion", Coord3D::new(base.x + 80.0, base.y + 80.0, 0.0), 0.0)
        .expect("GLATankScorpion");
    let salvager_pos = g.logic().find_object(salvager).unwrap().position;
    let crate_id = g
        .spawn_unit(0, "SalvageCrate", Coord3D::new(salvager_pos.x + 4.0, salvager_pos.y, salvager_pos.z), 0.0)
        .expect("SalvageCrate");
    g.run_frames(3);
    let salvaged = g.logic().find_object(crate_id).is_none();
    let flags = g.logic().find_object(salvager).map(|o| o.weapon_set_flags).unwrap_or(0);
    println!("salvage crate: consumed {salvaged}, salvager weapon_set_flags 0x{flags:x}");

    // ---- money crate ------------------------------------------------------------------------
    let before = g.logic().player(gla).map(|p| p.money).unwrap_or(0);
    let rebel = g
        .spawn_unit(gla, "GLAInfantryRebel", Coord3D::new(base.x + 120.0, base.y + 120.0, 0.0), 0.0)
        .expect("GLAInfantryRebel");
    let rebel_pos = g.logic().find_object(rebel).unwrap().position;
    let money_crate = g
        .spawn_unit(0, "1000DollarCrate", Coord3D::new(rebel_pos.x + 4.0, rebel_pos.y, rebel_pos.z), 0.0)
        .expect("1000DollarCrate");
    g.run_frames(3);
    let after = g.logic().player(gla).map(|p| p.money).unwrap_or(0);
    println!("money crate: consumed {}, money {before} -> {after}", g.logic().find_object(money_crate).is_none());

    // ---- crush ------------------------------------------------------------------------------
    let tank = g.spawn_unit(gla, "GLATankScorpion", Coord3D::new(base.x - 200.0, base.y - 200.0, 0.0), 0.0).expect("GLATankScorpion");
    let infantry = g.spawn_unit(china, "ChinaInfantryRedguard", Coord3D::new(base.x - 260.0, base.y - 200.0, 0.0), 0.0).expect("ChinaInfantryRedguard");
    g.submit(gla, Command::MoveTo { ids: vec![tank], pos: Coord3D::new(base.x - 320.0, base.y - 200.0, 0.0) });
    g.run_frames(90);
    let crushed = g.logic().find_object(infantry).is_none_or(|o| o.is_dead());
    println!("crush: infantry crushed {crushed}");

    println!("crc {:08x}", g.crc());
}
