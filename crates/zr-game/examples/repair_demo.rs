//! Dozer/worker repair, resume-construction and idle auto-repair on real data (Alpine Assault).
//!
//! 1. damage an America Power Plant and order a dozer to repair it (health per second);
//! 2. kill a dozer mid-construction and resume the scaffold with another dozer;
//! 3. leave an idle dozer near a damaged structure and watch its `BoredRange` auto-repair;
//! 4. a GLA worker repairs (it is a dozer and a supply truck);
//! 5. an AI player's idle dozer repairs its damaged base.
//!
//! `cargo run --release -p zr-game --example repair_demo` (needs a Zero Hour install, or
//! `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::{Game, GameData, PlayerSetup};
use zr_sim::skirmish_ai::SkirmishAiConfig;
use zr_sim::{Command, Coord3D, ObjectId, PlayerIndex};

/// Build `template` at the first legal site around `center` (a failed try costs nothing).
fn construct_near(g: &mut Game, player: PlayerIndex, dozer: ObjectId, template: &str, center: Coord3D, max_radius: f32) -> Option<ObjectId> {
    let before = g.logic().objects().filter(|o| o.template.name == template).count();
    let mut angle = 0.0f32;
    let mut radius = 40.0f32;
    while radius <= max_radius {
        for _ in 0..12 {
            let pos = Coord3D::new(center.x + radius * angle.cos(), center.y + radius * angle.sin(), 0.0);
            g.submit(player, Command::Construct { dozer, template: template.into(), pos, angle: 0.0 });
            g.step();
            let now: Vec<_> = g.logic().objects().filter(|o| o.template.name == template).collect();
            if now.len() > before {
                return Some(now[now.len() - 1].id);
            }
            angle += std::f32::consts::TAU / 12.0;
        }
        radius += 40.0;
    }
    None
}

/// The AI with an empty build list: its dozer has nothing to build, so only the idle auto-repair
/// (`DozerPrimaryIdleState`) runs.
fn empty_ai_config() -> SkirmishAiConfig {
    SkirmishAiConfig {
        build_list: Vec::new(),
        resource_gatherers: 0,
        base_defense: None,
        gatherer_template: String::new(),
        gatherer_factory: String::new(),
        attack_units: Vec::new(),
        attack_factory: String::new(),
        structure_seconds: 0,
        team_seconds: 60,
        rebuild_delay_seconds: 30,
        resources_wealthy: 2000,
        resources_poor: 500,
        structures_wealthy_mod: 1.0,
        structures_poor_mod: 1.0,
        team_wealthy_mod: 1.0,
        team_poor_mod: 1.0,
        team_resources_to_build: 1.0,
        supply_center_safe_radius: 250.0,
        max_recruit_distance: 500.0,
        skill_sets: Vec::new(),
        base_defense_extra_distance: 0.0,
        difficulty: zr_sim::script::Difficulty::Normal,
    }
}

fn ground(g: &Game, x: f32, y: f32) -> Coord3D {
    Coord3D::new(x, y, g.logic().terrain().ground_height(x, y))
}

fn human_game(data: Arc<GameData>, vfs: &Vfs) -> Game {
    let setups = [PlayerSetup::new("FactionAmerica", 0), PlayerSetup::new("FactionChina", 1)];
    Game::new_skirmish_with(data, vfs, "Alpine Assault", &setups, 1).expect("start skirmish")
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));

    part1_repair_structure(data.clone(), &vfs);
    part2_resume_after_death(data.clone(), &vfs);
    part3_idle_auto_repair(data.clone(), &vfs);
    part4_gla_worker(data.clone(), &vfs);
    part5_ai_repairs_base(data.clone(), &vfs);
}

/// 1. Damage an America Power Plant and order a dozer to repair it.
fn part1_repair_structure(data: Arc<GameData>, vfs: &Vfs) {
    println!("== 1. dozer repairs a damaged Power Plant ==");
    let mut g = human_game(data, vfs);
    let p0 = g.skirmish_player(0).unwrap().index;
    let cc = g.spawned().iter().find(|s| s.player == p0 && s.role == "StartingBuilding").map(|s| s.id).expect("command center");
    let cc_pos = g.object(cc).unwrap().position;
    let plant = g.spawn_unit(p0, "AmericaPowerPlant", ground(&g, cc_pos.x + 70.0, cc_pos.y), 0.0).expect("power plant");
    let dozer = g.spawn_unit(p0, "AmericaVehicleDozer", ground(&g, cc_pos.x + 70.0, cc_pos.y + 40.0), 0.0).expect("dozer");
    let max = g.object(plant).unwrap().health;
    g.damage_object(plant, max * 0.6);
    let damaged = g.object(plant).unwrap().health;
    println!("  plant {} hp {damaged:.0}/{max:.0}; repair rate {:.3}%/s", plant.0, g.logic().repair_health_per_second(dozer) * 100.0);
    g.submit(p0, Command::Repair { dozer, target: plant });
    let mut prev = damaged;
    let mut full = false;
    for sec in 1..=40u32 {
        g.run_frames(30);
        let hp = g.object(plant).map_or(0.0, |o| o.health);
        if sec <= 8 || hp >= max {
            println!("  t={sec}s hp {hp:.0} (+{:.0}/s)", hp - prev);
        }
        prev = hp;
        if hp >= max {
            full = true;
            break;
        }
    }
    println!("  repaired to full: {full} after {} frames; crc {:08x}\n", g.frame(), g.crc());
}

/// 2. Kill the dozer mid-construction, resume the scaffold with another dozer.
fn part2_resume_after_death(data: Arc<GameData>, vfs: &Vfs) {
    println!("== 2. kill the builder, resume with another dozer ==");
    let mut g = human_game(data, vfs);
    let p0 = g.skirmish_player(0).unwrap().index;
    let cc = g.spawned().iter().find(|s| s.player == p0 && s.role == "StartingBuilding").map(|s| s.id).expect("command center");
    let cc_pos = g.object(cc).unwrap().position;
    let a = g.spawn_unit(p0, "AmericaVehicleDozer", ground(&g, cc_pos.x - 60.0, cc_pos.y), 0.0).expect("dozer A");
    let b = g.spawn_unit(p0, "AmericaVehicleDozer", ground(&g, cc_pos.x - 90.0, cc_pos.y), 0.0).expect("dozer B");
    let Some(bldg) = construct_near(&mut g, p0, a, "AmericaPowerPlant", cc_pos, 200.0) else {
        println!("  no legal site found\n");
        return;
    };
    println!("  scaffold {} at percent {:.0}", bldg.0, g.logic().find_object(bldg).unwrap().construction_percent);
    println!("  can B resume while A lives? {}", g.logic().can_resume_construction(b, bldg));
    g.logic_mut().destroy_object(a);
    g.step();
    println!("  killed builder A; can B resume now? {}", g.logic().can_resume_construction(b, bldg));
    g.submit(p0, Command::ResumeConstruction { dozer: b, target: bldg });
    let mut done = false;
    for _ in 0..900 {
        g.step();
        let o = g.logic().find_object(bldg).unwrap();
        if !o.under_construction {
            done = true;
            break;
        }
    }
    let o = g.logic().find_object(bldg).unwrap();
    println!("  resumed and completed: {done} (percent {:.0}) at frame {}\n", o.construction_percent, g.frame());
}

/// 3. An idle dozer auto-repairs a damaged structure within `BoredRange`.
fn part3_idle_auto_repair(data: Arc<GameData>, vfs: &Vfs) {
    println!("== 3. idle dozer auto-repairs within BoredRange ==");
    let mut g = human_game(data, vfs);
    let p0 = g.skirmish_player(0).unwrap().index;
    let cc = g.spawned().iter().find(|s| s.player == p0 && s.role == "StartingBuilding").map(|s| s.id).expect("command center");
    let cc_pos = g.object(cc).unwrap().position;
    let plant = g.spawn_unit(p0, "AmericaPowerPlant", ground(&g, cc_pos.x + 200.0, cc_pos.y), 0.0).expect("power plant");
    // No order; just an idle dozer next to the plant.
    let _dozer = g.spawn_unit(p0, "AmericaVehicleDozer", ground(&g, cc_pos.x + 160.0, cc_pos.y), 0.0).expect("dozer");
    let max = g.object(plant).unwrap().health;
    g.damage_object(plant, max * 0.8);
    println!("  plant damaged to {:.0}/{max:.0}; dozer idle", g.object(plant).unwrap().health);
    let mut full = false;
    for sec in 1..=40u32 {
        g.run_frames(30);
        let hp = g.object(plant).map_or(0.0, |o| o.health);
        if hp >= max {
            println!("  auto-repaired to full at t={sec}s");
            full = true;
            break;
        }
    }
    println!("  auto-repaired to full: {full}, final hp {:.0} at frame {}\n", g.object(plant).map_or(0.0, |o| o.health), g.frame());
}

/// 4. A GLA worker repairs (it is a dozer and a supply truck).
fn part4_gla_worker(data: Arc<GameData>, vfs: &Vfs) {
    println!("== 4. GLA worker repairs ==");
    let mut g = human_game(data, vfs);
    let p0 = g.skirmish_player(0).unwrap().index;
    let cc = g.spawned().iter().find(|s| s.player == p0 && s.role == "StartingBuilding").map(|s| s.id).expect("command center");
    let cc_pos = g.object(cc).unwrap().position;
    let palace = g.spawn_unit(p0, "GLAPalace", ground(&g, cc_pos.x + 80.0, cc_pos.y), 0.0).expect("GLA palace");
    let worker = g.spawn_unit(p0, "GLAInfantryWorker", ground(&g, cc_pos.x + 80.0, cc_pos.y + 40.0), 0.0).expect("GLA worker");
    let max = g.object(palace).unwrap().health;
    g.damage_object(palace, max * 0.5);
    println!(
        "  palace {:.0}/{max:.0}; worker can repair? {}",
        g.object(palace).unwrap().health,
        g.logic().can_repair_object(worker, palace)
    );
    g.submit(p0, Command::Repair { dozer: worker, target: palace });
    for _ in 0..1200 {
        g.step();
        if g.object(palace).is_some_and(|o| o.health >= max) {
            break;
        }
    }
    println!("  palace repaired to {:.0}/{max:.0} at frame {}\n", g.object(palace).map_or(0.0, |o| o.health), g.frame());
}

/// 5. An AI player's idle dozer repairs its damaged base.
fn part5_ai_repairs_base(data: Arc<GameData>, vfs: &Vfs) {
    println!("== 5. AI player repairs its damaged base ==");
    let mut g = human_game(data, vfs);
    let p0 = g.skirmish_player(0).unwrap().index;
    let cc = g.spawned().iter().find(|s| s.player == p0 && s.role == "StartingBuilding").map(|s| s.id).expect("command center");
    let cc_pos = g.object(cc).unwrap().position;
    let ai: PlayerIndex = 1;
    // Register the slot as a skirmish AI with nothing to build, so its dozer is free to auto-repair.
    g.logic_mut().add_ai_player(ai, empty_ai_config());
    let plant = g.spawn_unit(ai, "ChinaPowerPlant", ground(&g, cc_pos.x + 200.0, cc_pos.y + 120.0), 0.0).expect("AI power plant");
    let _dozer = g.spawn_unit(ai, "ChinaVehicleDozer", ground(&g, cc_pos.x + 160.0, cc_pos.y + 120.0), 0.0).expect("AI dozer");
    let max = g.object(plant).unwrap().health;
    g.damage_object(plant, max * 0.7);
    println!("  AI plant damaged to {:.0}/{max:.0}; AI dozer idle", g.object(plant).unwrap().health);
    let mut started = None;
    let mut full = false;
    for sec in 1..=60u32 {
        g.run_frames(30);
        let hp = g.object(plant).map_or(0.0, |o| o.health);
        if started.is_none() && hp > max * 0.3 + 1.0 {
            started = Some(sec);
            println!("  t={sec}s AI auto-repair started (hp {hp:.0})");
        }
        if hp >= max {
            println!("  AI repaired its base to full at t={sec}s");
            full = true;
            break;
        }
    }
    println!("  AI repaired to full: {full}, final AI plant hp {:.0} at frame {}; crc {:08x}", g.object(plant).map_or(0.0, |o| o.health), g.frame(), g.crc());
}
