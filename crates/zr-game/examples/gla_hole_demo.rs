//! GLA rebuild-hole demo on Alpine Assault: a GLA structure leaves a rebuild hole when destroyed,
//! the hole's worker respawns after `WorkerRespawnDelay`, rebuilds the structure, and the hole and
//! worker go away when it is complete. Attackers are transferred from the structure to the hole.
//!
//! `cargo run --release -p zr-game --example gla_hole_demo` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::combat::AttackTarget;
use zr_sim::{Command, Coord3D, ObjectId};
use zr_templates::{ModuleData, ThingTemplate};

/// The object an attacker currently targets (`Object::combat.attack`).
fn attack_target(g: &Game, id: ObjectId) -> Option<ObjectId> {
    match g.logic().find_object(id)?.combat.attack.as_ref()?.target {
        AttackTarget::Object(o) => Some(o),
        AttackTarget::Ground(_) => None,
    }
}

fn find(data: &GameData, pred: impl Fn(&ThingTemplate) -> bool) -> Option<String> {
    data.templates().things().iter().find(|t| pred(t)).map(|t| t.name.clone())
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));
    let setups = [PlayerSetup::new("FactionGLA", 0), PlayerSetup::new("FactionAmerica", 1)];
    let mut g = Game::new_skirmish_with(data.clone(), &vfs, "Alpine Assault", &setups, 1).expect("start skirmish");
    let p0 = g.skirmish_player(0).expect("player0").index;
    let p1 = g.skirmish_player(1).expect("player1").index;

    // A GLA structure that leaves a hole: pick the one with the shortest build so the demo is quick.
    let mut candidates: Vec<&ThingTemplate> = data
        .templates()
        .things()
        .iter()
        .filter(|t| t.modules.iter().any(|m| matches!(&m.data, ModuleData::RebuildHoleExpose(_))))
        .filter(|t| !t.name.contains("Boss") && !t.name.contains("DemoTrap"))
        .collect();
    candidates.sort_by_key(|t| t.build_time as i64);
    let Some(structure) = candidates.first().map(|t| t.name.clone()) else {
        eprintln!("no RebuildHoleExposeDie structure found");
        return;
    };
    let expose = data
        .templates()
        .things()
        .iter()
        .find(|t| t.name == structure)
        .and_then(|t| t.modules.iter().find_map(|m| match &m.data {
            ModuleData::RebuildHoleExpose(d) => Some(d.clone()),
            _ => None,
        }))
        .expect("expose module");
    let hole_name = expose.hole_name.clone().unwrap_or_default();
    let worker_name = data
        .templates()
        .things()
        .iter()
        .find(|t| t.name == hole_name)
        .and_then(|t| t.modules.iter().find_map(|m| match &m.data {
            ModuleData::RebuildHole(d) => d.worker_object_name.clone(),
            _ => None,
        }))
        .unwrap_or_else(|| "GLAInfantryWorker".to_owned());
    let build_time = data.templates().things().iter().find(|t| t.name == structure).map_or(0.0, |t| t.build_time);
    println!(
        "{structure} -> hole {hole_name} (max health {:.0}), worker {worker_name}, build {build_time:.0}s",
        expose.hole_max_health
    );

    let base = g.spawned().iter().find(|s| s.player == p0).map(|s| s.pos).expect("p0 start");
    let spos = Coord3D::new(base.x + 250.0, base.y + 250.0, base.z);
    let Some(s) = g.spawn_object(&structure, spos, 0.0, p0) else { eprintln!("could not spawn {structure}"); return };

    // An enemy tank attacks the structure; the attack should transfer to the hole.
    let Some(attacker) = find(&data, |t| t.is_kind_of(zr_templates::KindOf::Vehicle) && t.build_cost > 0) else {
        eprintln!("no vehicle found");
        return;
    };
    let apos = Coord3D::new(spos.x + 120.0, spos.y, spos.z);
    let Some(atk) = g.spawn_object(&attacker, apos, 0.0, p1) else { eprintln!("could not spawn the attacker"); return };
    g.set_enemies(p0, p1);
    g.submit(p1, Command::Attack { ids: vec![atk], target: s });
    g.run_frames(2);
    println!("attacker {attacker} targets structure {}: {:?}", s.0, attack_target(&g, atk));

    // Destroy the structure: a hole appears in its place.
    g.damage_object(s, 1.0e9);
    g.run_frames(1);
    let hole = g.logic().objects().find(|o| o.template.name == hole_name).map(|o| o.id);
    let Some(hole) = hole else { eprintln!("no hole appeared"); return };
    let health = g.object(hole).map_or(0.0, |o| o.health);
    println!("hole {} appeared at frame {} with {:.0} health", hole.0, g.frame(), health);
    println!("attacker retargeted to the hole: {:?}", attack_target(&g, atk));
    // Stop the attacker so it does not destroy the hole (which would cancel the rebuild).
    g.submit(p1, Command::Stop { ids: vec![atk] });

    // Watch the respawn -> rebuild -> cleanup cycle, following the hole's own worker handle.
    let mut worker_seen = None;
    let mut rebuilt = None;
    let mut hole_gone = None;
    for _ in 0..5000 {
        g.step();
        if worker_seen.is_none() {
            if let Some(w) = g.logic().find_object(hole).and_then(|o| o.behavior.hole_worker) {
                worker_seen = Some((g.frame(), w));
                // GLA has no power economy: `Energy::getEnergySupplyRatio` returns 0 for 0/0, so the
                // power penalty slows the rebuild to `1 / MinLowEnergyProductionSpeed`. This is the
                // retail "GLA power plant bug", not a sim error (see LOG).
                let tally = g.logic().energy_tally(p0);
                let pen = g.logic().rules().low_power_penalty(g.logic().energy_supply_ratio(p0));
                println!("worker {} spawned at frame {}; energy {tally:?} build-speed factor {pen}", w.0, g.frame());
            }
        }
        if rebuilt.is_none() && g.logic().objects().any(|o| o.template.name == structure && !o.under_construction) {
            rebuilt = Some(g.frame());
            println!("{structure} rebuilt at frame {}", g.frame());
        }
        if rebuilt.is_some() && g.object(hole).is_none() {
            hole_gone = Some(g.frame());
            println!("hole {} removed at frame {}", hole.0, g.frame());
            break;
        }
    }
    println!(
        "result: worker at {:?}, rebuilt at {:?}, hole gone at {:?}; structures now {}",
        worker_seen.map(|(f, _)| f),
        rebuilt,
        hole_gone,
        g.logic().objects().filter(|o| o.template.name == structure).count()
    );
}
