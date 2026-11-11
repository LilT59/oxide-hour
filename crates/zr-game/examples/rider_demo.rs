//! Combat Cycle rider demo on Alpine Assault. The rider table is read from the install (the GLA
//! Combat Cycle, or the template named as an argument). The demo boards the listed Rebel, Tunnel
//! Defender and Terrorist in turn (each one replaces the last), tries an unlisted GLA infantry, then
//! puts the rider out and watches the riderless bike scuttle and topple. Each step prints the bike's
//! weapon set, object status, rider model conditions and veterancy.
//!
//! `cargo run --release -p zr-game --example rider_demo [BikeTemplate]` (needs a Zero Hour install,
//! or `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::object::status;
use zr_sim::{Command, Coord3D, GameLogic, ObjectId};
use zr_templates::{ContainKind as TplContainKind, ModuleData, ThingTemplate};

/// The statuses this demo reports, by name.
const STATUS_NAMES: [(&str, u64); 10] = [
    ("RIDER1", status::RIDER1),
    ("RIDER2", status::RIDER2),
    ("RIDER3", status::RIDER3),
    ("RIDER4", status::RIDER4),
    ("RIDER5", status::RIDER5),
    ("RIDER6", status::RIDER6),
    ("RIDER7", status::RIDER7),
    ("RIDER8", status::RIDER8),
    ("UNSELECTABLE", status::UNSELECTABLE),
    ("IMMOBILE", status::IMMOBILE),
];

/// The riders boarded in order, when the bike's table lists them.
const SEQUENCE: [&str; 3] = ["GLAInfantryRebel", "GLAInfantryTunnelDefender", "GLAInfantryTerrorist"];

/// The `RiderN` templates of a combat bike, in slot order (unfilled slots skipped).
fn rider_names(t: &ThingTemplate) -> Vec<String> {
    t.modules
        .iter()
        .filter_map(|m| match &m.data {
            ModuleData::Contain(c) if c.kind == TplContainKind::RiderChange => Some(c.riders.iter().map(|r| r.template_name.clone())),
            _ => None,
        })
        .flatten()
        .filter(|n| !n.is_empty())
        .collect()
}

fn is_rider_bike(t: &&ThingTemplate) -> bool {
    t.modules.iter().any(|m| matches!(&m.data, ModuleData::Contain(c) if c.kind == TplContainKind::RiderChange))
}

fn report(g: &GameLogic, bike: ObjectId, label: &str) {
    println!("[{label}]");
    let Some(b) = g.find_object(bike) else {
        println!("  bike is gone");
        return;
    };
    let st = b.status_mask(g.frame());
    let statuses: Vec<&str> = STATUS_NAMES.iter().filter(|(_, bit)| st & bit != 0).map(|(n, _)| *n).collect();
    let conds: Vec<&str> = g
        .model_conditions(bike)
        .map(|m| m.names())
        .unwrap_or_default()
        .into_iter()
        .filter(|n| n.starts_with("RIDER") || *n == "TOPPLED")
        .collect();
    let riders: Vec<String> = g.contained_units(bike).iter().filter_map(|&r| g.find_object(r).map(|o| o.template.name.clone())).collect();
    println!("  weapon set flags 0x{:08x}  object status {:?}", b.weapon_set_flags, statuses);
    println!("  model conditions {conds:?}  riders {riders:?}");
    println!("  command set override {:?}  stealthed {}", b.command_set_override(), g.is_stealthed(bike));
    let rider_level = g.contained_units(bike).first().and_then(|&r| g.find_object(r)).map(|r| r.experience.level);
    println!("  veterancy bike {:?}  rider {:?}  locomotor {:?}", b.experience.level, rider_level, b.locomotor_set);
}

fn wait_until(g: &mut Game, max: u32, mut done: impl FnMut(&GameLogic) -> bool) -> u32 {
    for f in 0..max {
        if done(g.logic()) {
            return f;
        }
        g.run_frames(1);
    }
    max
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));

    let bikes: Vec<&ThingTemplate> = data.templates().things().iter().filter(is_rider_bike).collect();
    if bikes.is_empty() {
        eprintln!("no RiderChangeContain template in the install");
        return;
    }
    println!("rider bikes: {:?}", bikes.iter().map(|t| t.name.as_str()).collect::<Vec<_>>());
    // The GLA Combat Cycle unless another bike is named.
    let default = bikes.iter().find(|t| t.name == "GLAVehicleCombatBike").unwrap_or(&bikes[0]).name.clone();
    let bike_name = std::env::args().nth(1).unwrap_or(default);
    let Some(bike_t) = data.templates().things().iter().find(|t| t.name.eq_ignore_ascii_case(&bike_name)) else {
        eprintln!("no template named {bike_name}");
        return;
    };
    let table = rider_names(bike_t);
    println!("using {bike_name}; rider table: {table:?}");
    let mut scuttle_frames = 0;
    for m in &bike_t.modules {
        if let ModuleData::Contain(c) = &m.data {
            if c.kind == TplContainKind::RiderChange {
                scuttle_frames = c.scuttle_frames;
                println!("  slots {}  scuttle {} frames  scuttle state {}", c.slot_capacity, c.scuttle_frames, c.scuttle_state);
                for (i, r) in c.riders.iter().enumerate().filter(|(_, r)| !r.template_name.is_empty()) {
                    println!(
                        "  Rider{}: {} model {} weapon {:?} status {:?} locomotor {:?}",
                        i + 1,
                        r.template_name,
                        r.model_condition,
                        r.weapon_set,
                        r.object_status,
                        r.locomotor_set
                    );
                }
            }
        }
    }
    let boarding: Vec<&str> = SEQUENCE.iter().copied().filter(|n| table.iter().any(|r| r == n)).collect();
    if boarding.len() < 2 {
        eprintln!("the table lists fewer than two of {SEQUENCE:?}; nothing to swap");
        return;
    }

    let setups = [PlayerSetup::new("FactionGLA", 0), PlayerSetup::new("FactionAmerica", 1)];
    let mut g = Game::new_skirmish_with(data.clone(), &vfs, "Alpine Assault", &setups, 1).expect("start skirmish");
    let p0 = g.skirmish_player(0).expect("player0").index;
    let base = g.spawned().iter().find(|s| s.player == p0 && s.role == "StartingBuilding").expect("base").pos;

    let bike = g.spawn_unit(p0, &bike_name, Coord3D::new(base.x + 90.0, base.y + 60.0, 0.0), 0.0).expect("bike template");
    let riders: Vec<(String, ObjectId)> = boarding
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let at = Coord3D::new(base.x + 120.0, base.y + 60.0 + 15.0 * i as f32, 0.0);
            (n.to_string(), g.spawn_unit(p0, n, at, 0.0).expect("rider template"))
        })
        .collect();
    // An infantry the table does not list: the first GLA infantry template in the install that is not a rider.
    let unlisted_name = data.templates().things().iter().map(|t| t.name.clone()).find(|n| n.starts_with("GLAInfantry") && !table.contains(n));
    let unlisted = unlisted_name.and_then(|n| g.spawn_unit(p0, &n, Coord3D::new(base.x + 120.0, base.y + 110.0, 0.0), 0.0).map(|id| (n, id)));

    report(g.logic(), bike, "start");
    let mut last = None;
    for (name, id) in &riders {
        g.submit(p0, Command::Enter { ids: vec![*id], container: bike });
        let t = wait_until(&mut g, 900, |l| l.container_of(*id) == Some(bike));
        println!("{name} boards after {t} frames");
        report(g.logic(), bike, &format!("{name} aboard"));
        if let Some(prev) = last.replace(*id) {
            println!("  previous rider out: {}", g.logic().container_of(prev).is_none());
        }
    }

    if let Some((name, id)) = unlisted {
        g.submit(p0, Command::Enter { ids: vec![id], container: bike });
        g.run_frames(300);
        println!("unlisted {name} boards: {}", g.logic().container_of(id) == Some(bike));
    }

    let Some(rider) = last else { return };
    g.submit(p0, Command::Exit { container: bike, ids: vec![rider] });
    let t = wait_until(&mut g, 900, |l| l.container_of(rider).is_none());
    println!("the last rider leaves after {t} frames");
    report(g.logic(), bike, "rider out, bike scuttled");

    let t = wait_until(&mut g, 600, |l| l.find_object(bike).is_none_or(|o| o.is_dying()));
    println!("bike topples after {t} more frames (ScuttleDelay {scuttle_frames} frames)");
    report(g.logic(), bike, "bike toppled");
    println!("crc {:08x}", g.crc());
}
