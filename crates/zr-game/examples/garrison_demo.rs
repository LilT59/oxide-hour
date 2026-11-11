//! Garrison firing demo on Alpine Assault: find a garrisonable building and a rifle infantry
//! template, report the building's model `FIREPOINT` bones (`GarrisonContain::loadGarrisonPoints`),
//! put a squad inside, drop an enemy in range and watch the passengers kill it from the assigned
//! fire point (`PassengersAllowedToFire`).
//!
//! `cargo run --release -p zr-game --example garrison_demo` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;

use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::{Command, Coord3D, TemplateSource};
use zr_templates::{ContainKind as TplContainKind, KindOf, ModuleData, ThingTemplate};

fn find(data: &GameData, pred: impl Fn(&ThingTemplate) -> bool) -> Option<String> {
    data.templates().things().iter().find(|t| pred(t)).map(|t| t.name.clone())
}

fn max_range(data: &GameData, name: &str) -> f32 {
    data.store
        .find_template(name)
        .and_then(|t| t.combat.weapon_set.clone())
        .map(|s| s.weapons.iter().flatten().fold(0.0f32, |m, w| m.max(w.attack_range)))
        .unwrap_or(0.0)
}

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
    let p1 = g.skirmish_player(1).expect("player1").index;

    // Resolve every garrison's `FIREPOINT` bones (the same map the adapter installed on the sim
    // templates) so the demo can pick a building that actually has some and print them.
    let fire_map = {
        let db = data.ini_db().expect("the loaded data owns an INI database");
        zr_game::firepoints::garrison_fire_points(&db, &vfs, data.templates())
    };
    let is_enclosing_garrison = |t: &ThingTemplate| {
        t.modules.iter().any(|m| matches!(&m.data, ModuleData::Contain(c) if c.kind == TplContainKind::Garrison && c.is_enclosing_container))
    };
    let garrisons: Vec<&str> = data.templates().things().iter().filter(|t| is_enclosing_garrison(t)).map(|t| t.name.as_str()).collect();
    let mut without: Vec<&str> = garrisons
        .iter()
        .copied()
        .filter(|n| fire_map.get(*n).is_none_or(|f| f.conditions[0].is_empty()))
        .collect();
    without.sort_unstable();
    println!("enclosing garrisons: {}/{} have pristine FIREPOINT bones", garrisons.len() - without.len(), garrisons.len());
    if !without.is_empty() {
        println!("  without roster bones ({}) : {}", without.len(), without.iter().take(12).copied().collect::<Vec<_>>().join(", "));
    }
    let non_enclosing: Vec<&str> = data
        .templates()
        .things()
        .iter()
        .filter(|t| t.modules.iter().any(|m| matches!(&m.data, ModuleData::Contain(c) if c.kind == TplContainKind::Garrison && !c.is_enclosing_container)))
        .map(|t| t.name.as_str())
        .collect();
    let with_stations = non_enclosing.iter().filter(|n| fire_map.get(**n).is_some_and(|f| !f.station_points.is_empty())).count();
    println!("non-enclosing garrisons: {with_stations}/{} have STATION bones", non_enclosing.len());
    let turret: Vec<&str> = data
        .templates()
        .things()
        .iter()
        .filter(|t| t.modules.iter().any(|m| matches!(&m.data, ModuleData::Contain(c) if c.passengers_in_turret)))
        .map(|t| t.name.as_str())
        .collect();
    let with_turret = turret.iter().filter(|n| fire_map.get(**n).is_some_and(|f| !f.turret_points.is_empty())).count();
    println!("PassengersInTurret containers: {with_turret}/{} have FIREPOINT bones", turret.len());
    let Some(building) = find(&data, |t| {
        is_enclosing_garrison(t) && fire_map.get(&t.name).is_some_and(|f| f.conditions.iter().any(|c| !c.is_empty()))
    })
    .or_else(|| find(&data, is_enclosing_garrison))
    else {
        eprintln!("no garrisonable building found");
        return;
    };
    let Some(infantry) = find(&data, |t| {
        t.is_kind_of(KindOf::Infantry)
            && !t.is_kind_of(KindOf::Hero)
            && t.transport_slot_count > 0
            && t.weapon_sets.iter().any(|s| s.weapons.iter().any(|w| w.is_some()))
    }) else {
        eprintln!("no rifle infantry found");
        return;
    };
    let range = max_range(&data, &infantry);
    println!("building {building}, infantry {infantry} (weapon range {range:.0})");
    let points = fire_map.get(&building).cloned().unwrap_or_default();
    let counts: Vec<usize> = points.conditions.iter().map(Vec::len).collect();
    println!("  FIREPOINT bones (pristine/damaged/really-damaged): {counts:?}");
    for (i, p) in points.conditions[0].iter().enumerate() {
        println!("    FIREPOINT{:02} local ({:.1}, {:.1}, {:.1})", i + 1, p.x, p.y, p.z);
    }

    let base = g
        .spawned()
        .iter()
        .find(|s| s.player == p0 && s.template.contains("CommandCenter"))
        .or_else(|| g.spawned().iter().find(|s| s.player == p0))
        .map(|s| s.pos)
        .expect("player 0 start position");
    let bpos = Coord3D::new(base.x + 200.0, base.y + 200.0, base.z);
    let Some(bld) = g.spawn_object(&building, bpos, 0.0, p0) else {
        eprintln!("could not spawn the building");
        return;
    };
    let radius = g.object_radius(bld).unwrap_or(30.0);
    let inside = Coord3D::new(bpos.x + radius + 15.0, bpos.y, bpos.z);
    let Some(ranger) = g.spawn_object(&infantry, inside, 0.0, p0) else {
        eprintln!("could not spawn the infantry");
        return;
    };
    g.submit(p0, Command::Enter { ids: vec![ranger], container: bld });
    let mut boarded = None;
    for _ in 0..600 {
        g.step();
        if g.logic().is_contained(ranger) {
            boarded = Some(g.frame());
            break;
        }
    }
    println!("boarded at frame {:?}, contained {}", boarded, g.logic().is_contained(ranger));
    if !g.logic().is_contained(ranger) {
        return;
    }

    // Drop an enemy just inside the weapon's range measured from the building edge.
    let epos = Coord3D::new(bpos.x, bpos.y + radius + range * 0.6, bpos.z);
    let Some(enemy) = g.spawn_object(&infantry, epos, 0.0, p1) else {
        eprintln!("could not spawn the enemy");
        return;
    };
    g.set_enemies(p0, p1);
    println!("enemy {} spawned {:.0} units out, health {:.0}", enemy.0, epos.dist_2d(&bpos), g.object(enemy).unwrap().health);
    let mut killed = None;
    for f in 0..1200 {
        g.step();
        if g.object(enemy).is_none() {
            killed = Some(g.frame());
            break;
        }
        if f % 150 == 0 {
            let rp = g.object(ranger).unwrap().position;
            println!(
                "  frame {}: enemy health {:.0}, fire position ({:.1}, {:.1}, {:.1}), {:.0} from the building centre",
                g.frame(),
                g.object(enemy).unwrap().health,
                rp.x,
                rp.y,
                rp.z,
                rp.dist_2d(&bpos)
            );
        }
    }
    match killed {
        Some(f) => println!("enemy killed at frame {f} by the garrisoned {}", infantry),
        None => println!("enemy survived; health {:.0}", g.object(enemy).map_or(0.0, |o| o.health)),
    }

    // Non-enclosing garrison (firebase): passengers sit on the model's `STATION` bones instead of
    // the centre (`GarrisonContain::positionObjectsAtStationGarrisonPoints`).
    let firebase = data
        .templates()
        .things()
        .iter()
        .find(|t| {
            t.modules.iter().any(|m| matches!(&m.data, ModuleData::Contain(c) if c.kind == TplContainKind::Garrison && !c.is_enclosing_container))
                && fire_map.get(&t.name).is_some_and(|f| !f.station_points.is_empty())
        })
        .map(|t| t.name.clone());
    if let Some(fb) = firebase {
        let fpos = Coord3D::new(base.x - 200.0, base.y + 200.0, base.z);
        if let Some(fbid) = g.spawn_object(&fb, fpos, 0.0, p0) {
            let s1 = g.spawn_object(&infantry, Coord3D::new(fpos.x + 40.0, fpos.y, fpos.z), 0.0, p0);
            let s2 = g.spawn_object(&infantry, Coord3D::new(fpos.x, fpos.y + 40.0, fpos.z), 0.0, p0);
            if let (Some(s1), Some(s2)) = (s1, s2) {
                g.submit(p0, Command::Enter { ids: vec![s1, s2], container: fbid });
                g.run_frames(40);
                for (name, id) in [("s1", s1), ("s2", s2)] {
                    if let Some(o) = g.object(id) {
                        println!("  {name} at ({:.1}, {:.1}), {:.1} from the firebase centre", o.position.x, o.position.y, o.position.dist_2d(&fpos));
                    }
                }
                println!(
                    "{fb}: {} STATION bones, {} contained",
                    fire_map.get(&fb).map_or(0, |f| f.station_points.len()),
                    g.logic().contained_units(fbid).len()
                );
            }
        }
    }
}
