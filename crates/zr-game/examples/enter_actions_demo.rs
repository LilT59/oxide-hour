//! Real-data exercise of the enter-type abilities on Alpine Assault (GLA vs China): a GLA
//! Hijacker takes an enemy vehicle, a GLA Terrorist converts a car to a car bomb, a GLA Saboteur
//! sabotages an enemy power plant (brownout) and steals cash from a supply center, and a China
//! Black Lotus captures an enemy building. All of them are ordered with `Command::Enter`
//! (`MSG_ENTER`, exactly what retail's `CommandXlat` sends for hijack / convert / sabotage) except
//! the capture, which is a special power (`Command::DoSpecialPower`).
//!
//! `cargo run --release -p zr-game --example enter_actions_demo` (needs a Zero Hour install, or
//! `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::{Command, Coord3D, DisabledType, ObjectId, SpecialPowerTarget};
use zr_templates::{KindOf, WeaponSetType};

/// Step until `done` holds or `max` frames pass; returns the frame it finished on (`None` on
/// timeout) and the frame count.
fn run_until<F: Fn(&Game) -> bool>(g: &mut Game, max: u32, done: F) -> Option<u32> {
    for _ in 0..max {
        g.step();
        if done(g) {
            return Some(g.frame());
        }
    }
    None
}

/// A vehicle template the GLA Terrorist can convert: a `VEHICLE` with a `WEAPONSET_CARBOMB` set.
/// Prefers a civilian car, else the first vehicle that qualifies.
fn carbombable_vehicle(data: &GameData) -> Option<String> {
    let is_car = |name: &str| {
        let n = name.to_ascii_lowercase();
        n.contains("civilian") || n.contains("car") || n.contains("truck") || n.contains("van")
    };
    let qualifies = |t: &zr_templates::ThingTemplate| {
        t.kind_of.contains(KindOf::Vehicle)
            && t.weapon_sets.iter().any(|ws| ws.conditions.contains(WeaponSetType::Carbomb))
    };
    data.templates()
        .things()
        .iter()
        .filter(|t| qualifies(t) && is_car(&t.name))
        .map(|t| t.name.clone())
        .next()
        .or_else(|| data.templates().things().iter().find(|t| qualifies(t)).map(|t| t.name.clone()))
}

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

    let base = g.spawned().iter().find(|s| s.player == gla && s.role == "StartingBuilding").expect("GLA base").pos;

    // ---- Hijacker takes an enemy vehicle -----------------------------------------------------
    match (g.spawn_object("GLAInfantryHijacker", Coord3D::new(base.x + 60.0, base.y + 60.0, 0.0), 0.0, gla),
           g.spawn_object("ChinaTankBattleMaster", Coord3D::new(base.x + 64.0, base.y + 60.0, 0.0), 0.0, china)) {
        (Some(h), Some(v)) => {
            g.submit(gla, Command::Enter { ids: vec![h], container: v });
            let frame = run_until(&mut g, 600, |g| g.object(v).map(|o| o.owner) == Some(gla));
            let owner = g.object(v).map(|o| o.owner);
            println!("hijack: vehicle owner {owner:?} (wanted {gla}) at frame {frame:?}; hijacker alive {}", g.object(h).is_some());
        }
        _ => println!("hijack: missing GLAInfantryHijacker / ChinaTankBattlemaster"),
    }

    // ---- Terrorist converts a car to a car bomb ----------------------------------------------
    if let Some(car_name) = carbombable_vehicle(&data) {
        println!("carbomb target template: {car_name}");
        match (g.spawn_object("GLAInfantryTerrorist", Coord3D::new(base.x + 100.0, base.y + 60.0, 0.0), 0.0, gla),
               g.spawn_object(&car_name, Coord3D::new(base.x + 104.0, base.y + 60.0, 0.0), 0.0, china)) {
            (Some(t), Some(car)) => {
                g.submit(gla, Command::Enter { ids: vec![t], container: car });
                let frame = run_until(&mut g, 600, |g| {
                    g.logic().find_object(car).is_some_and(|o| o.weapon_set_flags & zr_sim::weapon::WEAPONSET_CARBOMB != 0)
                });
                let flags = g.logic().find_object(car).map_or(0, |o| o.weapon_set_flags);
                println!("carbomb: weapon_set_flags 0x{flags:x} (CARBOMB 0x{:x}) at frame {frame:?}; terrorist alive {}",
                    zr_sim::weapon::WEAPONSET_CARBOMB, g.object(t).is_some());
            }
            _ => println!("carbomb: could not spawn terrorist / {car_name}"),
        }
    } else {
        println!("carbomb: no carbombable vehicle template found");
    }

    // ---- Saboteur sabotages an enemy power plant (brownout) -----------------------------------
    if let (Some(s), Some(plant)) = (
        g.spawn_object("GLAInfantrySaboteur", Coord3D::new(base.x - 60.0, base.y + 60.0, 0.0), 0.0, gla),
        g.spawn_object("ChinaPowerPlant", Coord3D::new(base.x - 40.0, base.y + 60.0, 0.0), 0.0, china),
    ) {
        g.submit(gla, Command::Enter { ids: vec![s], container: plant });
        let frame = run_until(&mut g, 600, |g| g.logic().find_object(s).is_none());
        // `update_brownout_disabled` runs at the start of the next frame.
        g.run_frames(2);
        // `Energy::hasSufficientPower` is false while the grid is sabotaged, and the brownout
        // disables any `KINDOF_POWERED` building of the victim.
        let no_power = !g.logic().has_sufficient_power(china);
        let browned_out = g
            .logic()
            .objects()
            .any(|o| o.player == china && o.is_disabled_by_type(DisabledType::Underpowered));
        println!("sabotage power: saboteur consumed {} at frame {frame:?}; China no-power {no_power} brownout {browned_out}", g.object(s).is_none());
    } else {
        println!("sabotage power: missing GLAInfantrySaboteur / ChinaPowerPlant");
    }

    // ---- Saboteur steals cash from an enemy supply center --------------------------------------
    if let (Some(s), Some(center)) = (
        g.spawn_object("GLAInfantrySaboteur", Coord3D::new(base.x - 140.0, base.y + 60.0, 0.0), 0.0, gla),
        g.spawn_object("ChinaSupplyCenter", Coord3D::new(base.x - 120.0, base.y + 60.0, 0.0), 0.0, china),
    ) {
        let before = g.player_money(china).unwrap_or(0);
        g.submit(gla, Command::Enter { ids: vec![s], container: center });
        let frame = run_until(&mut g, 600, |g| g.logic().find_object(s).is_none());
        let after = g.player_money(china).unwrap_or(0);
        println!("sabotage supply: China money {before} -> {after} at frame {frame:?}; saboteur consumed {}", g.object(s).is_none());
    } else {
        println!("sabotage supply: missing GLAInfantrySaboteur / ChinaSupplyCenter");
    }

    // ---- Black Lotus captures an enemy building -----------------------------------------------
    let glabuilding = g
        .logic()
        .objects()
        .find(|o| o.player == gla && o.template.name.contains("CommandCenter"))
        .map(|o| o.id)
        .unwrap_or(ObjectId::INVALID);
    if let Some(lotus) = g.spawn_object("ChinaInfantryBlackLotus", Coord3D::new(base.x + 160.0, base.y - 60.0, 0.0), 0.0, china) {
        let power = g.logic().capture_power_for(lotus).unwrap_or_else(|| "SPECIAL_BLACKLOTUS_CAPTURE_BUILDING".to_string());
        println!("capture power: {power}, can_capture {}", g.logic().can_capture_building(lotus, glabuilding));
        g.submit(china, Command::DoSpecialPower {
            source: lotus,
            power,
            target: SpecialPowerTarget::Object(glabuilding),
        });
        let frame = run_until(&mut g, 900, |g| g.object(glabuilding).map(|o| o.owner) == Some(china));
        let owner = g.object(glabuilding).map(|o| o.owner);
        println!("capture: building owner {owner:?} (wanted {china}) at frame {frame:?}");
    } else {
        println!("capture: missing ChinaInfantryBlackLotus");
    }

    println!("crc {:08x}", g.crc());
}
