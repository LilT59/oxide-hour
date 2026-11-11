//! Airfield jet cycle on real data (`JetAIUpdate` / `ParkingPlaceBehavior`), headless.
//!
//! `cargo run --release -p zr-game --example jet_demo -- <scenario>` (needs a Zero Hour install, or
//! `ZR_ZEROHOUR_DIR`). Scenarios (Alpine Assault, America vs China):
//!
//! * `raptor`  - build a Raptor at an America Airfield, order it to attack an enemy structure and
//!   trace the takeoff, the attack, the return when out of ammo, the landing, the rearm, the re-park
//!   and a second attack order.
//! * `mig`     - the same with a China Airfield and a MiG (`NapalmMissileWeapon`).
//! * `idle`    - a built jet is ordered to move, then left alone: it returns to its airfield after
//!   `ReturnToBaseIdleTime` and lands.
//! * `killfield` - the airfield is destroyed while its jets are airborne (they circle and burn) and
//!   while one is still parked (killed with the airfield).
//! * `comanche` - a Comanche (`NeedsRunway = No`, helipad) rises from the airfield, attacks, lands
//!   next to it on an `Enter` order and takes off again once it is healed.
//! * `fleet`   - four Raptors ordered to attack at once: runway reservation and takeoff queueing.
//! * `all`     - every scenario.
//!
//! Prints one line per state / flag change and a position line every `PERIOD` frames.

use std::collections::BTreeMap;
use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::{Command, Coord3D, ObjectId};

const PERIOD: u32 = 30;

struct Tracer {
    last: BTreeMap<u32, String>,
}

impl Tracer {
    fn new() -> Self {
        Tracer { last: BTreeMap::new() }
    }

    fn describe(g: &Game, id: ObjectId) -> Option<String> {
        let o = g.logic().find_object(id)?;
        // Unlimited clips (`ClipSize = 0`) carry no ammo information.
        let w = o.combat.weapons.weapons.iter().flatten().filter(|w| w.template.clip_size > 0).map(|w| w.ammo_in_clip()).collect::<Vec<_>>();
        let f = o.jet.flags;
        let mut flags = String::new();
        for (bit, name) in [
            (1u16 << 0, "pend"),
            (1 << 1, "air"),
            (1 << 3, "takeoff"),
            (1 << 4, "landing"),
            (1 << 5, "retloco"),
            (1 << 6, "circle"),
            (1 << 8, "taxi"),
        ] {
            if f & bit != 0 {
                flags.push_str(name);
                flags.push(' ');
            }
        }
        let af = o.producer_id.0;
        Some(format!(
            "{:?} loco={:?} [{}] ammo={:?} hp={:.0} producer={}",
            o.ai.machine.current,
            o.locomotor_set,
            flags.trim_end(),
            w,
            o.health(),
            af
        ))
    }

    /// Print a line when the object's state / flags / ammo change, and every `PERIOD` frames its position.
    fn step(&mut self, g: &Game, name: &str, id: ObjectId) {
        let frame = g.frame();
        let Some(desc) = Self::describe(g, id) else {
            if self.last.remove(&id.0).is_some() {
                println!("  f{frame:>5} {name}: GONE");
            }
            return;
        };
        let changed = self.last.get(&id.0) != Some(&desc);
        if changed || frame % PERIOD == 0 {
            let o = g.logic().find_object(id).expect("alive");
            let alt = o.position.z - g.logic().terrain().ground_height(o.position.x, o.position.y);
            let spd = o.physics.as_ref().map_or(0.0, |p| p.speed_2d());
            let tag = if changed { "*" } else { " " };
            println!(
                "{tag} f{frame:>5} {name:<8} ({:7.1},{:7.1}) alt {:5.1} ang {:6.1} spd {:4.2} | {desc}",
                o.position.x,
                o.position.y,
                alt,
                o.angle.to_degrees(),
                spd
            );
            self.last.insert(id.0, desc);
        }
    }
}

struct World {
    g: Game,
    p0: u8,
    p1: u8,
    base: Coord3D,
}

fn world(data: &Arc<GameData>, vfs: &Vfs, p0_faction: &str, p1_faction: &str) -> World {
    let setups = [PlayerSetup::new(p0_faction, 0).with_cash(100_000), PlayerSetup::new(p1_faction, 1)];
    let mut g = Game::new_skirmish_with(data.clone(), vfs, "Alpine Assault", &setups, 1).expect("start skirmish");
    let p0 = g.skirmish_player(0).expect("player0").index;
    let p1 = g.skirmish_player(1).expect("player1").index;
    g.set_enemies(p0, p1);
    let base = g.spawned().iter().find(|s| s.player == p0 && s.role == "StartingBuilding").expect("base").pos;
    World { g, p0, p1, base }
}

fn ground(g: &Game, x: f32, y: f32) -> Coord3D {
    Coord3D::new(x, y, g.logic().terrain().ground_height(x, y))
}

/// Run `frames` frames tracing `ids`.
fn run(g: &mut Game, tr: &mut Tracer, ids: &[(&str, ObjectId)], frames: u32) {
    for _ in 0..frames {
        g.step();
        for (n, id) in ids {
            tr.step(g, n, *id);
        }
    }
}

/// Run until `cond` or `max` frames; returns whether `cond` held.
fn run_until(g: &mut Game, tr: &mut Tracer, ids: &[(&str, ObjectId)], max: u32, mut cond: impl FnMut(&Game) -> bool) -> bool {
    for _ in 0..max {
        g.step();
        for (n, id) in ids {
            tr.step(g, n, *id);
        }
        if cond(g) {
            return true;
        }
    }
    false
}

fn find_new(g: &Game, owner: u8, template: &str, known: &[ObjectId]) -> Option<ObjectId> {
    g.objects().find(|o| o.owner == owner && o.template == template && !known.contains(&o.id)).map(|o| o.id)
}

fn state_of(g: &Game, id: ObjectId) -> Option<zr_sim::ai_states::AiStateType> {
    g.logic().find_object(id).map(|o| o.ai.machine.current)
}

fn alt_of(g: &Game, id: ObjectId) -> f32 {
    g.logic().find_object(id).map_or(0.0, |o| o.position.z - g.logic().terrain().ground_height(o.position.x, o.position.y))
}

/// Build the airfield and the jet; returns (world, airfield, jet).
fn setup_jet(data: &Arc<GameData>, vfs: &Vfs, faction: &str, enemy_faction: &str, airfield_t: &str, jet_t: &str) -> (World, ObjectId, ObjectId) {
    let mut w = world(data, vfs, faction, enemy_faction);
    let pos = ground(&w.g, w.base.x + 40.0, w.base.y - 380.0);
    let airfield = w.g.spawn_object(airfield_t, pos, 0.0, w.p0).expect("airfield template");
    println!("airfield {} at ({:.0}, {:.0}, {:.0})", airfield.0, pos.x, pos.y, pos.z);
    // Plenty of money (see `world`); queue the jet.
    w.g.submit(w.p0, Command::QueueUnit { factory: airfield, template: jet_t.into() });
    let before = w.g.player_money(w.p0).unwrap_or(0);
    w.g.run_frames(1);
    println!("queued {jet_t}: money {before} -> {}", w.g.player_money(w.p0).unwrap_or(0));
    let mut tr = Tracer::new();
    let mut jet = None;
    for _ in 0..1500 {
        w.g.step();
        if jet.is_none() {
            jet = find_new(&w.g, w.p0, jet_t, &[]);
            if let Some(j) = jet {
                println!("{jet_t} {} appeared at frame {}", j.0, w.g.frame());
                tr.step(&w.g, "jet", j);
            }
        }
        if let Some(j) = jet {
            tr.step(&w.g, "jet", j);
            // Wait until it has finished taxiing out of the hangar and parked.
            if matches!(state_of(&w.g, j), Some(zr_sim::ai_states::AiStateType::Idle)) && w.g.frame() > 10 {
                break;
            }
        }
    }
    let jet = jet.expect("jet produced");
    (w, airfield, jet)
}

fn scenario_jet(data: &Arc<GameData>, vfs: &Vfs, faction: &str, enemy: &str, airfield_t: &str, jet_t: &str, target_t: &str) {
    println!("=== {jet_t} at {airfield_t} ===");
    let (mut w, airfield, jet) = setup_jet(data, vfs, faction, enemy, airfield_t, jet_t);
    let mut tr = Tracer::new();
    // The enemy target: far enough that the jet must fly, close enough to see in a short trace.
    let tpos = ground(&w.g, w.base.x + 40.0, w.base.y + 150.0);
    let target = w.g.spawn_object(target_t, tpos, 0.0, w.p1).expect("target template");
    println!("target {target_t} {} at ({:.0}, {:.0}); jet parked at frame {}", target.0, tpos.x, tpos.y, w.g.frame());
    let ids = [("jet", jet), ("target", target)];
    println!("--- order: attack");
    w.g.submit(w.p0, Command::Attack { ids: vec![jet], target });
    // Takeoff -> attack -> out of ammo -> return -> land -> reload.
    let landed_again = run_until(&mut w.g, &mut tr, &ids, 3000, |g| {
        matches!(state_of(g, jet), Some(zr_sim::ai_states::AiStateType::JetReloadAmmo))
    });
    println!("reached reload: {landed_again} (frame {})", w.g.frame());
    let done = run_until(&mut w.g, &mut tr, &ids, 1200, |g| matches!(state_of(g, jet), Some(zr_sim::ai_states::AiStateType::Idle)));
    let o = w.g.logic().find_object(jet).expect("jet alive");
    println!(
        "re-parked: {done} frame {} ammo {:?} hp {:.0} producer {} pos ({:.1},{:.1}) alt {:.1} (airfield {})",
        w.g.frame(),
        o.combat.weapons.weapons.iter().flatten().map(|w| w.ammo_in_clip()).collect::<Vec<_>>(),
        o.health(),
        o.producer_id.0,
        o.position.x,
        o.position.y,
        alt_of(&w.g, jet),
        airfield.0
    );
    println!("--- order: second attack (if the target survived)");
    if w.g.logic().find_object(target).is_some() {
        w.g.submit(w.p0, Command::Attack { ids: vec![jet], target });
        run(&mut w.g, &mut tr, &ids, 600);
    } else {
        println!("(target destroyed; ordering a move instead)");
        let dest = ground(&w.g, w.base.x + 200.0, w.base.y + 200.0);
        w.g.submit(w.p0, Command::MoveTo { ids: vec![jet], pos: dest });
        run(&mut w.g, &mut tr, &ids, 600);
    }
}

fn scenario_idle(data: &Arc<GameData>, vfs: &Vfs) {
    println!("=== idle jet returns to base ===");
    let (mut w, _airfield, jet) = setup_jet(data, vfs, "FactionAmerica", "FactionChina", "AmericaAirfield", "AmericaJetRaptor");
    let mut tr = Tracer::new();
    let ids = [("jet", jet)];
    let dest = ground(&w.g, w.base.x + 200.0, w.base.y + 300.0);
    println!("--- order: move to ({:.0}, {:.0})", dest.x, dest.y);
    w.g.submit(w.p0, Command::MoveTo { ids: vec![jet], pos: dest });
    run_until(&mut w.g, &mut tr, &ids, 4000, |g| matches!(state_of(g, jet), Some(zr_sim::ai_states::AiStateType::JetReloadAmmo)));
    run(&mut w.g, &mut tr, &ids, 400);
}

fn scenario_killfield(data: &Arc<GameData>, vfs: &Vfs) {
    println!("=== airfield destroyed ===");
    let (mut w, airfield, jet_a) = setup_jet(data, vfs, "FactionAmerica", "FactionChina", "AmericaAirfield", "AmericaJetRaptor");
    // A second jet, built and parked.
    w.g.submit(w.p0, Command::QueueUnit { factory: airfield, template: "AmericaJetRaptor".into() });
    let mut tr = Tracer::new();
    let mut jet_b = None;
    for _ in 0..1600 {
        w.g.step();
        if jet_b.is_none() {
            jet_b = find_new(&w.g, w.p0, "AmericaJetRaptor", &[jet_a]);
        }
        if let Some(b) = jet_b {
            tr.step(&w.g, "jetB", b);
            if matches!(state_of(&w.g, b), Some(zr_sim::ai_states::AiStateType::Idle)) {
                break;
            }
        }
    }
    let jet_b = jet_b.expect("second jet");
    let ids = [("jetA", jet_a), ("jetB", jet_b)];
    println!("--- order: jetA attacks (takes off, fires all its missiles); jetB stays parked");
    let tpos = ground(&w.g, w.base.x + 40.0, w.base.y + 150.0);
    let target = w.g.spawn_object("ChinaPowerPlant", tpos, 0.0, w.p1).expect("target");
    w.g.submit(w.p0, Command::Attack { ids: vec![jet_a], target });
    // Wait until it is out of ammo and heading home, then lose the airfield.
    run_until(&mut w.g, &mut tr, &ids, 3000, |g| matches!(state_of(g, jet_a), Some(zr_sim::ai_states::AiStateType::JetReturningForLanding)));
    run(&mut w.g, &mut tr, &ids, 60);
    println!("--- the airfield is destroyed at frame {}", w.g.frame());
    w.g.logic_mut().damage_object(
        airfield,
        zr_sim::DamageInfo { source: ObjectId::INVALID, amount: 1.0e9, kind: zr_sim::DamageType::Unresistable },
    );
    run(&mut w.g, &mut tr, &ids, 900);
}

fn scenario_comanche(data: &Arc<GameData>, vfs: &Vfs) {
    println!("=== Comanche (helipad) ===");
    let (mut w, airfield, heli) = setup_jet(data, vfs, "FactionAmerica", "FactionChina", "AmericaAirfield", "AmericaVehicleComanche");
    let mut tr = Tracer::new();
    let tpos = ground(&w.g, w.base.x + 40.0, w.base.y + 150.0);
    let target = w.g.spawn_object("ChinaPowerPlant", tpos, 0.0, w.p1).expect("target");
    let ids = [("heli", heli), ("target", target)];
    println!("--- order: attack {}", target.0);
    w.g.submit(w.p0, Command::Attack { ids: vec![heli], target });
    run_until(&mut w.g, &mut tr, &ids, 2500, |g| g.logic().find_object(target).is_none());
    println!("target gone: {} frame {}", w.g.logic().find_object(target).is_none(), w.g.frame());
    println!("--- order: land at the airfield (enter)");
    w.g.submit(w.p0, Command::Enter { ids: vec![heli], container: airfield });
    run(&mut w.g, &mut tr, &ids, 900);
}

/// Four Raptors in the four parking spaces, all ordered to attack at once: runway reservation,
/// "next in line for takeoff", the wait for the jet on the other runway.
fn scenario_fleet(data: &Arc<GameData>, vfs: &Vfs) {
    println!("=== four Raptors, one order ===");
    let mut w = world(data, vfs, "FactionAmerica", "FactionChina");
    let pos = ground(&w.g, w.base.x + 40.0, w.base.y - 380.0);
    let airfield = w.g.spawn_object("AmericaAirfield", pos, 0.0, w.p0).expect("airfield");
    let mut jets: Vec<ObjectId> = Vec::new();
    for _ in 0..4 {
        w.g.submit(w.p0, Command::QueueUnit { factory: airfield, template: "AmericaJetRaptor".into() });
    }
    w.g.run_frames(1);
    println!("queued 4 Raptors; money {}", w.g.player_money(w.p0).unwrap_or(0));
    for _ in 0..8000 {
        w.g.step();
        if let Some(j) = find_new(&w.g, w.p0, "AmericaJetRaptor", &jets) {
            println!("jet {} appeared at frame {}", j.0, w.g.frame());
            jets.push(j);
        }
        if jets.len() == 4 && jets.iter().all(|j| matches!(state_of(&w.g, *j), Some(zr_sim::ai_states::AiStateType::Idle))) {
            break;
        }
    }
    println!("all parked at frame {}: {} jets", w.g.frame(), jets.len());
    let tpos = ground(&w.g, w.base.x + 40.0, w.base.y + 150.0);
    let target = w.g.spawn_object("ChinaPowerPlant", tpos, 0.0, w.p1).expect("target");
    let names = ["j1", "j2", "j3", "j4"];
    let ids: Vec<(&str, ObjectId)> = names.iter().copied().zip(jets.iter().copied()).collect();
    let mut tr = Tracer::new();
    println!("--- order: all attack");
    w.g.submit(w.p0, Command::Attack { ids: jets.clone(), target });
    let t0 = w.g.frame();
    let mut airborne_at: BTreeMap<u32, u32> = BTreeMap::new();
    for _ in 0..2500 {
        w.g.step();
        for (n, id) in &ids {
            tr.step(&w.g, n, *id);
            if alt_of(&w.g, *id) > 20.0 {
                airborne_at.entry(id.0).or_insert(w.g.frame() - t0);
            }
        }
    }
    println!("frames after the order until each jet was above 20 units: {airborne_at:?}");
}

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "raptor".into());
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));
    let all = which == "all";
    if all || which == "raptor" {
        scenario_jet(&data, &vfs, "FactionAmerica", "FactionChina", "AmericaAirfield", "AmericaJetRaptor", "ChinaPowerPlant");
    }
    if all || which == "mig" {
        scenario_jet(&data, &vfs, "FactionChina", "FactionAmerica", "ChinaAirfield", "ChinaJetMIG", "AmericaPowerPlant");
    }
    if all || which == "idle" {
        scenario_idle(&data, &vfs);
    }
    if all || which == "killfield" {
        scenario_killfield(&data, &vfs);
    }
    if all || which == "comanche" {
        scenario_comanche(&data, &vfs);
    }
    if all || which == "fleet" {
        scenario_fleet(&data, &vfs);
    }
}
