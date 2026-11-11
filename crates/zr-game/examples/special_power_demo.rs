//! Special powers with real Zero Hour data: an America command center fires an A-10 strike (OCL)
//! and a paradrop at a China structure, and the object count changes as the OCL spawns.
//!
//! `cargo run --release -p zr-game --example special_power_demo` (needs a Zero Hour install, or `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::{Command, Coord3D, GameLogic, SpecialPowerTarget, TemplateSource, TerrainLogic};

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));

    let Some(source_t) = data.store.find_template("AmericaCommandCenter") else {
        eprintln!("no AmericaCommandCenter template");
        return;
    };
    let Some(target_t) = data.store.find_template("ChinaCommandCenter") else {
        eprintln!("no ChinaCommandCenter template");
        return;
    };
    println!("AmericaCommandCenter has {} special powers:", source_t.special_powers.len());
    for p in &source_t.special_powers {
        let what = match &p.effect {
            zr_sim::special_power::SpecialPowerEffect::Ocl { ocl, create_at_target, .. } => match ocl {
                Some(o) => {
                    let kinds: Vec<String> = o
                        .nuggets
                        .iter()
                        .map(|n| match n {
                            zr_sim::ocl::OclNugget::CreateObject(_) => "CreateObject".to_string(),
                            zr_sim::ocl::OclNugget::CreateDebris(_) => "CreateDebris".to_string(),
                            zr_sim::ocl::OclNugget::FireWeapon(_) => "FireWeapon".to_string(),
                            zr_sim::ocl::OclNugget::DeliverPayload(d) => format!(
                                "DeliverPayload transport={} payload={:?} parachute={} fire_weapon={}",
                                d.transport.as_ref().map_or("<none>", |t| t.name.as_str()),
                                d.payload.iter().map(|(t, c)| (t.name.as_str(), *c)).collect::<Vec<_>>(),
                                d.is_parachute_directly,
                                d.fire_weapon
                            ),
                            zr_sim::ocl::OclNugget::ApplyRandomForce(_) => "ApplyRandomForce".to_string(),
                            zr_sim::ocl::OclNugget::Attack(_) => "Attack".to_string(),
                            zr_sim::ocl::OclNugget::Unsupported => "Unsupported".to_string(),
                        })
                        .collect();
                    format!("OCL {} ({:?}, at_target {})", o.name, kinds, create_at_target)
                }
                None => "OCL <unresolved>".to_string(),
            },
            zr_sim::special_power::SpecialPowerEffect::FireWeapon => "FireWeapon".to_string(),
            zr_sim::special_power::SpecialPowerEffect::CashHack { amount } => format!("CashHack {amount}"),
            zr_sim::special_power::SpecialPowerEffect::CleanupArea { range } => format!("CleanupArea {range}"),
            zr_sim::special_power::SpecialPowerEffect::Baikonur { .. } => "Baikonur".to_string(),
            zr_sim::special_power::SpecialPowerEffect::SpyVision { base, max, .. } => format!("SpyVision base {base} max {max}"),
            zr_sim::special_power::SpecialPowerEffect::ParticleUplink => "ParticleUplink".to_string(),
            zr_sim::special_power::SpecialPowerEffect::SpectreDeployment => "SpectreDeployment".to_string(),
            zr_sim::special_power::SpecialPowerEffect::CashBounty => "CashBounty".to_string(),
        };
        println!("  {} -> {}", p.power, what);
    }

    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    {
        let t = data.templates();
        // Air locomotors are force-driven like the engine's; use the real gravity so they can fly.
        let mut rules = g.rules();
        rules.gravity = t.gravity;
        g.set_rules(rules);
        g.set_special_powers(zr_game::adapter::special_power_store(t));
        let (sci, ranks) = zr_game::adapter::science_tables(t);
        g.set_science_tables(sci, ranks);
    }
    let src = g.create_object(&source_t, Coord3D::new(800.0, 1000.0, 0.0), 0.0, 0);
    let dst = g.create_object(&target_t, Coord3D::new(1400.0, 1000.0, 0.0), 0.0, 1);
    g.set_enemies(0, 1);
    // Grant every science so the `RequiredScience` gates open.
    for s in data.templates().sciences.iter() {
        g.add_science(0, &s.name);
    }

    // Fire every OCL power at the China structure and watch the object count.
    for power in source_t.special_powers.iter().map(|p| p.power.clone()).collect::<Vec<_>>() {
        let before = g.object_count();
        let hp_before = g.find_object(dst).map_or(0.0, |o| o.health());
        g.submit(0, Command::DoSpecialPower { source: src, power: power.clone(), target: SpecialPowerTarget::Object(dst) });
        g.run_frames(120);
        let after = g.object_count();
        let hp_after = g.find_object(dst).map_or(0.0, |o| o.health());
        let ready = g.special_power_ready_at(src, &power);
        println!("power {power}: objects {before} -> {after}, target hp {hp_before:.0} -> {hp_after:.0}, next ready at frame {ready}");
    }
    println!("target alive: {}, health {:.0}", g.find_object(dst).is_some(), g.find_object(dst).map_or(0.0, |o| o.health()));

    // ---- abilities: Black Lotus captures a structure, Jarmen Kell snipes a driver ----
    if let Some(lotus_t) = data.store.find_template("ChinaInfantryBlackLotus") {
        let mut g2 = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        {
            let t = data.templates();
            g2.set_special_powers(zr_game::adapter::special_power_store(t));
            let (sci, ranks) = zr_game::adapter::science_tables(t);
            g2.set_science_tables(sci, ranks);
            for s in t.sciences.iter() {
                g2.add_science(0, &s.name);
            }
        }
        let lotus = g2.create_object(&lotus_t, Coord3D::new(800.0, 1000.0, 0.0), 0.0, 0);
        let bldg = g2.create_object(&target_t, Coord3D::new(900.0, 1000.0, 0.0), 0.0, 1);
        g2.set_enemies(0, 1);
        for a in &lotus_t.special_abilities {
            println!("Black Lotus ability {}", a.power);
        }
        if let Some(a) = lotus_t.special_abilities.first() {
            let kind = data.templates().special_power(&a.power).map(|p| p.kind.clone());
            println!("  range {} prep {} kind {:?}", a.start_ability_range, a.preparation_frames, kind);
            let owner_before = g2.find_object(bldg).map(|o| o.player);
            g2.submit(0, Command::DoSpecialPower { source: lotus, power: a.power.clone(), target: SpecialPowerTarget::Object(bldg) });
            g2.run_frames(60);
            println!("Black Lotus {}: building owner {owner_before:?} -> {:?}", a.power, g2.find_object(bldg).map(|o| o.player));
        }
    }
    if let Some(kell_t) = data.store.find_template("GLAInfantryJarmenKell") {
        if let Some(tank_t) = data.store.find_template("ChinaTankBattleMaster") {
            let mut g3 = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
            let kell = g3.create_object(&kell_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
            let v = g3.create_object(&tank_t, Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
            g3.set_enemies(0, 1);
            g3.submit(0, Command::Attack { ids: vec![kell], target: v });
            g3.run_frames(400);
            println!("Jarmen Kell snipe: vehicle unmanned {:?}, hp {:?}", g3.find_object(v).map(|o| o.unmanned()), g3.find_object(v).map(|o| o.health()));
        }
    }
}
