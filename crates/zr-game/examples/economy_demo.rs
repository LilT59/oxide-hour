//! Base economy demo on Alpine Assault: China's dozer builds a Power Plant, then a Supply Center
//! next to the nearest supply dock, then the Supply Center builds a Supply Truck that harvests
//! boxes into money. Reports money over time.
//!
//! `cargo run --release -p zr-game --example economy_demo` (needs a Zero Hour install, or
//! `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::supply::DockKind;
use zr_sim::{Command, Coord3D, ObjectId};

fn build_near(g: &mut Game, player: zr_sim::PlayerIndex, dozer: ObjectId, template: &str, center: Coord3D, max_radius: f32) -> Option<ObjectId> {
    // Try a spiral of sites until `isLocationLegalToBuild` accepts one (a failed try costs nothing).
    let before = g.logic().objects().filter(|o| o.template.name == template).count();
    let mut angle = 0.0f32;
    let mut radius = 30.0f32;
    while radius <= max_radius {
        for _ in 0..12 {
            let pos = Coord3D::new(center.x + radius * angle.cos(), center.y + radius * angle.sin(), 0.0);
            g.submit(player, Command::Construct { dozer, template: template.into(), pos, angle: 0.0 });
            g.step(); // process the command so we can tell whether it succeeded
            let now: Vec<_> = g.logic().objects().filter(|o| o.template.name == template).collect();
            if now.len() > before {
                return Some(now[now.len() - 1].id);
            }
            angle += std::f32::consts::TAU / 12.0;
        }
        radius += 30.0;
    }
    None
}

fn wait_until_done(g: &mut Game, id: ObjectId, limit: u32) -> bool {
    for _ in 0..limit {
        g.step();
        if g.logic().find_object(id).is_some_and(|o| !o.under_construction) {
            return true;
        }
    }
    false
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

    let p0 = g.skirmish_player(0).expect("player0").index;
    let dozer = g.spawned().iter().find(|s| s.player == p0 && s.template.contains("Dozer")).expect("dozer").id;
    let cc = g.spawned().iter().find(|s| s.player == p0 && s.role == "StartingBuilding").expect("command center").id;
    let cc_pos = g.object(cc).expect("cc").position;
    let rules = g.logic().rules();
    println!(
        "rules: ValuePerSupplyBox {}, AllowedHeightVariation {}, LowEnergyPenaltyModifier {}, min {} max {}",
        rules.value_per_supply_box,
        rules.allowed_height_variation_for_building,
        rules.low_energy_penalty_modifier,
        rules.min_low_energy_production_speed,
        rules.max_low_energy_production_speed
    );
    println!("start money {}", g.player_money(p0).unwrap());

    // 1. Power Plant (a prerequisite for the Supply Center).
    let plant = build_near(&mut g, p0, dozer, "ChinaPowerPlant", cc_pos, 180.0).expect("power plant site");
    let plant_done = wait_until_done(&mut g, plant, 1200);
    println!(
        "power plant {} at frame {}, money {}, energy {:?}, done {}",
        plant.0,
        g.frame(),
        g.player_money(p0).unwrap(),
        g.logic().energy_tally(p0),
        plant_done
    );

    // 2. Supply Center next to the nearest supply dock.
    let nearest_dock = g
        .logic()
        .objects()
        .filter(|o| o.template.dock.as_ref().is_some_and(|d| d.kind == DockKind::Warehouse))
        .min_by(|a, b| a.position.dist_2d(&cc_pos).partial_cmp(&b.position.dist_2d(&cc_pos)).unwrap())
        .map(|o| (o.id, o.position, o.template.name.clone()));
    let Some((dock_id, dock_pos, dock_name)) = nearest_dock else {
        println!("no supply dock on this map");
        return;
    };
    println!("nearest supply dock {} ({dock_name}) at ({:.0}, {:.0})", dock_id.0, dock_pos.x, dock_pos.y);
    let center = build_near(&mut g, p0, dozer, "ChinaSupplyCenter", dock_pos, 180.0).expect("supply center site");
    let center_done = wait_until_done(&mut g, center, 1200);
    println!(
        "supply center {} at frame {}, money {}, energy {:?}, done {}",
        center.0,
        g.frame(),
        g.player_money(p0).unwrap(),
        g.logic().energy_tally(p0),
        center_done
    );

    // 3. Build a supply truck and let it harvest.
    g.submit(p0, Command::QueueUnit { factory: center, template: "ChinaVehicleSupplyTruck".into() });
    g.step();
    println!("queued supply truck, money {}", g.player_money(p0).unwrap());
    let before = g.player_money(p0).unwrap();
    let mut reported = 0u32;
    for _ in 0..2400 {
        g.step();
        if g.frame() % 300 == 0 && reported < 8 {
            reported += 1;
            let truck = g.logic().objects().find(|o| o.template.name == "ChinaVehicleSupplyTruck");
            let info = truck.map_or_else(
                || "no truck yet".to_string(),
                |t| format!("truck boxes {} target {:?} timer {} cfg {:?}", t.boxes, t.supply_target.map(|x| x.0), t.supply_timer, t.template.supply_truck),
            );
            println!(
                "  frame {:>5}: money {} (+{}), {info}",
                g.frame(),
                g.player_money(p0).unwrap(),
                g.player_money(p0).unwrap() as i64 - before as i64
            );
        }
    }
    println!("final money {} (+{}) at frame {}", g.player_money(p0).unwrap(), g.player_money(p0).unwrap() as i64 - before as i64, g.frame());
    println!("crc {:08x}", g.crc());
}
