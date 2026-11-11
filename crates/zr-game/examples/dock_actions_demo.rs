//! Real-data exercise of the player dock / get-repaired / get-healed commands on Alpine Assault
//! (China vs America):
//!
//! 1. a China supply truck ordered to a supply warehouse docks, picks up boxes and delivers them to
//!    the player's own supply center (`Command::Dock`);
//! 2. a damaged China tank ordered to a repair pad docks and is healed (`Command::GetRepaired`);
//! 3. a damaged China infantry ordered to a heal building enters it and is healed
//!    (`Command::GetHealed`).
//!
//! The repair pad / heal building are discovered from the loaded data by `KINDOF_REPAIR_PAD` /
//! `KINDOF_HEAL_PAD`, so the demo skips gracefully if the install has none.
//!
//! `cargo run --release -p zr-game --example dock_actions_demo` (needs a Zero Hour install, or
//! `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::{Command, Coord3D};
use zr_templates::KindOf;

/// Step until `done` holds or `max` frames pass; returns the frame it finished on.
fn run_until<F: Fn(&Game) -> bool>(g: &mut Game, max: u32, done: F) -> Option<u32> {
    for _ in 0..max {
        g.step();
        if done(g) {
            return Some(g.frame());
        }
    }
    None
}

/// The first `ThingTemplate` carrying `kind` (`ThingFactory::firstTemplate` order).
fn find_by_kind(data: &GameData, kind: KindOf) -> Option<String> {
    data.templates().things().iter().find(|t| t.kind_of.contains(kind)).map(|t| t.name.clone())
}

fn ground(g: &Game, x: f32, y: f32) -> Coord3D {
    Coord3D::new(x, y, g.logic().terrain().ground_height(x, y))
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));

    let setups = [PlayerSetup::new("FactionChina", 0), PlayerSetup::new("FactionAmerica", 1)];
    let mut g = Game::new_skirmish_with(data.clone(), &vfs, "Alpine Assault", &setups, 1).expect("start skirmish");
    let p0 = g.skirmish_player(0).expect("China player").index;
    let base = g.spawned().iter().find(|s| s.player == p0 && s.role == "StartingBuilding").expect("China base").pos;

    // ---- 1. supply truck docks a warehouse, delivers to the supply center ----------------------
    if let Some(truck) = g.spawn_object("ChinaVehicleSupplyTruck", ground(&g, base.x + 60.0, base.y - 40.0), 0.0, p0) {
        let warehouse = g.spawn_object("SupplyWarehouse", ground(&g, base.x + 60.0, base.y - 110.0), 0.0, p0);
        let center = g.spawn_object("ChinaSupplyCenter", ground(&g, base.x + 130.0, base.y - 40.0), 0.0, p0);
        if let (Some(warehouse), Some(_center)) = (warehouse, center) {
            let can = g.logic().can_dock_at(truck, warehouse);
            let boxes = g.logic().find_object(warehouse).map_or(0, |o| o.boxes);
            let money = g.player_money(p0).unwrap_or(0);
            g.submit(p0, Command::Dock { ids: vec![truck], target: warehouse });
            let frame = run_until(&mut g, 2400, |g| g.player_money(p0).unwrap_or(0) > money);
            let after = g.player_money(p0).unwrap_or(0);
            println!(
                "dock: warehouse boxes {boxes}, can_dock_at {can}; money {money} -> {after} at frame {frame:?}; truck boxes {}",
                g.logic().find_object(truck).map_or(-1, |o| o.boxes)
            );
        } else {
            println!("dock: could not spawn SupplyWarehouse / ChinaSupplyCenter");
        }
    } else {
        println!("dock: missing ChinaVehicleSupplyTruck");
    }

    // ---- 2. damaged tank repairs at a repair pad ------------------------------------------------
    match find_by_kind(&data, KindOf::RepairPad) {
        Some(pad_name) => {
            let pad = g.spawn_object(&pad_name, ground(&g, base.x - 80.0, base.y + 40.0), 0.0, p0);
            let tank = g.spawn_object("ChinaTankBattleMaster", ground(&g, base.x - 80.0, base.y + 110.0), 0.0, p0);
            match (pad, tank) {
                (Some(pad), Some(tank)) => {
                    let max = g.object(tank).map_or(0.0, |o| o.health);
                    g.damage_object(tank, max * 0.6);
                    let damaged = g.object(tank).map_or(0.0, |o| o.health);
                    let can = g.logic().can_get_repaired_at(tank, pad);
                    g.submit(p0, Command::GetRepaired { ids: vec![tank], target: pad });
                    let frame = run_until(&mut g, 2400, |g| g.object(tank).is_some_and(|o| o.health >= max));
                    println!(
                        "get-repaired: {pad_name} {damaged:.0}/{max:.0} hp, can_get_repaired_at {can}; full at frame {frame:?} (hp {:.0})",
                        g.object(tank).map_or(0.0, |o| o.health)
                    );
                }
                _ => println!("get-repaired: could not spawn {pad_name} / ChinaTankBattleMaster"),
            }
        }
        None => println!("get-repaired: no KINDOF_REPAIR_PAD template in this install"),
    }

    // ---- 3. damaged infantry heals at a heal building -------------------------------------------
    match find_by_kind(&data, KindOf::HealPad) {
        Some(heal_name) => {
            let heal = g.spawn_object(&heal_name, ground(&g, base.x + 80.0, base.y + 40.0), 0.0, p0);
            let ranger = g.spawn_object("ChinaInfantryRedguard", ground(&g, base.x + 80.0, base.y + 110.0), 0.0, p0);
            match (heal, ranger) {
                (Some(heal), Some(inf)) => {
                    let max = g.object(inf).map_or(0.0, |o| o.health);
                    g.damage_object(inf, max * 0.6);
                    let damaged = g.object(inf).map_or(0.0, |o| o.health);
                    let can = g.logic().can_get_healed_at(inf, heal);
                    g.submit(p0, Command::GetHealed { ids: vec![inf], target: heal });
                    let frame = run_until(&mut g, 2400, |g| g.object(inf).is_some_and(|o| o.health >= max));
                    let contained = g.logic().find_object(inf).and_then(|o| o.contained_by) == Some(heal);
                    println!(
                        "get-healed: {heal_name} {damaged:.0}/{max:.0} hp, can_get_healed_at {can}; full at frame {frame:?} (hp {:.0}, contained {contained})",
                        g.object(inf).map_or(0.0, |o| o.health)
                    );
                }
                _ => println!("get-healed: could not spawn {heal_name} / ChinaInfantryRedguard"),
            }
        }
        None => println!("get-healed: no KINDOF_HEAL_PAD template in this install"),
    }

    println!("crc {:08x}", g.crc());
}
