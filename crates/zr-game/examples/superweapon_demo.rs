//! Superweapon / special-power / die module demo with real Zero Hour data.
//!
//! Fires each newly ported power on a flat map and prints the sim effect (damage, objects, money,
//! reveals). `cargo run --release -p zr-game --example superweapon_demo` (needs a Zero Hour install
//! or `ZR_ZEROHOUR_DIR`).

use std::sync::Arc;
use zr_assets::{Installs, Vfs};
use zr_game::GameData;
use zr_sim::special_power::SpecialPowerEffect;
use zr_sim::{Command, Coord3D, GameLogic, ObjectId, SpecialPowerTarget, TemplateSource, TerrainLogic};

fn grant_all(g: &mut GameLogic, data: &GameData, player: u8) {
    for s in data.templates().sciences.iter() {
        g.add_science(player, &s.name);
    }
}

fn powers_of(t: &zr_sim::ObjectTemplate) -> Vec<String> {
    t.special_powers.iter().map(|p| p.power.clone()).collect()
}

fn fire(g: &mut GameLogic, source: ObjectId, power: &str, target: ObjectId) {
    g.submit(0, Command::DoSpecialPower { source, power: power.to_string(), target: SpecialPowerTarget::Object(target) });
}

fn main() {
    let installs = Installs::detect();
    let Some(zh) = &installs.zero_hour else {
        eprintln!("no Zero Hour install found; set ZR_ZEROHOUR_DIR");
        return;
    };
    let vfs = Vfs::zero_hour(zh).expect("mounting Zero Hour");
    let data = Arc::new(GameData::load(&vfs));

    let new_game = |data: &Arc<GameData>| {
        let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
        // Air locomotors are force-driven (like the engine's): they need the real gravity, not the
        // synthetic `GameLogic::new` placeholder, or their lift cannot hold them up.
        let mut rules = g.rules();
        rules.gravity = data.templates().gravity;
        g.set_rules(rules);
        g.set_template_source(zr_game::store_template_source(data.clone()));
        let t = data.templates();
        g.set_special_powers(zr_game::adapter::special_power_store(t));
        let (sci, ranks) = zr_game::adapter::science_tables(t);
        g.set_science_tables(sci, ranks);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g
    };

    // ---- Particle Uplink Cannon (America superweapon) --------------------------------------
    if let Some(cannon_t) = data.store.find_template("AmericaParticleCannonUplink") {
        let mut g = new_game(&data);
        grant_all(&mut g, data.as_ref(), 0);
        let cannon = g.create_object(&cannon_t, Coord3D::new(600.0, 1000.0, 0.0), 0.0, 0);
        let Some(target_t) = data.store.find_template("ChinaCommandCenter") else { return };
        let target = g.create_object(&target_t, Coord3D::new(1600.0, 1000.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        let hp0 = g.find_object(target).map_or(0.0, |o| o.health());
        for p in powers_of(&cannon_t) {
            println!("ParticleCannon power {p} effect {:?}", cannon_t.special_powers.iter().find(|x| x.power == p).map(|x| &x.effect));
            fire(&mut g, cannon, &p, target);
        }
        g.run_frames(400);
        let hp1 = g.find_object(target).map_or(0.0, |o| o.health());
        println!("ParticleCannon: target hp {hp0:.0} -> {hp1:.0} (destroyed {})", g.find_object(target).is_none());
    }

    // ---- Neutron Missile (China superweapon) ------------------------------------------------
    if let Some(launcher_t) = data.store.find_template("ChinaNuclearMissileLauncher") {
        let mut g = new_game(&data);
        grant_all(&mut g, data.as_ref(), 0);
        let launcher = g.create_object(&launcher_t, Coord3D::new(600.0, 1000.0, 0.0), 0.0, 0);
        let Some(target_t) = data.store.find_template("AmericaCommandCenter") else { return };
        let target = g.create_object(&target_t, Coord3D::new(1600.0, 1000.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        let hp0 = g.find_object(target).map_or(0.0, |o| o.health());
        for p in powers_of(&launcher_t) {
            println!("Neutron launcher power {p}");
            fire(&mut g, launcher, &p, target);
        }
        g.run_frames(900);
        let hp1 = g.find_object(target).map_or(0.0, |o| o.health());
        let completed = g.completed_special_power_seen(0);
        println!("Neutron: target hp {hp0:.0} -> {hp1:.0}, objects {}, completed-power seen {completed}", g.object_count());
    }

    // ---- Spectre Gunship (America Command Center power) ------------------------------------
    if let Some(cc_t) = data.store.find_template("AmericaCommandCenter") {
        let mut g = new_game(&data);
        grant_all(&mut g, data.as_ref(), 0);
        let cc = g.create_object(&cc_t, Coord3D::new(600.0, 1000.0, 0.0), 0.0, 0);
        let Some(target_t) = data.store.find_template("ChinaCommandCenter") else { return };
        let target = g.create_object(&target_t, Coord3D::new(1200.0, 1000.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        let hp0 = g.find_object(target).map_or(0.0, |o| o.health());
        let before = g.object_count();
        for p in powers_of(&cc_t) {
            if matches!(cc_t.special_powers.iter().find(|x| x.power == p).map(|x| &x.effect), Some(SpecialPowerEffect::SpectreDeployment)) {
                fire(&mut g, cc, &p, target);
            }
        }
        g.run_frames(1500);
        let hp1 = g.find_object(target).map_or(0.0, |o| o.health());
        let gunships = g.objects().filter(|o| o.template.name == "AmericaJetSpectreGunship").count();
        println!("Spectre: objects {before} -> {} (gunships {gunships}), target hp {hp0:.0} -> {hp1:.0}", g.object_count());
    }

    // ---- Spy Vision (America Strategy Center) ----------------------------------------------
    if let Some(sc_t) = data.store.find_template("AmericaStrategyCenter") {
        let mut g = new_game(&data);
        grant_all(&mut g, data.as_ref(), 0);
        let sc = g.create_object(&sc_t, Coord3D::new(600.0, 1000.0, 0.0), 0.0, 0);
        for p in powers_of(&sc_t) {
            println!("StrategyCenter power {p} effect {:?}", sc_t.special_powers.iter().find(|x| x.power == p).map(|x| &x.effect));
            g.submit(0, Command::DoSpecialPower { source: sc, power: p.clone(), target: SpecialPowerTarget::None });
            g.run_frames(2);
            println!("  spy vision active: {}", g.spy_vision_active(sc));
        }
    }

    // ---- Cleanup Area (America ambulance) ---------------------------------------------------
    if let Some(medic_t) = data.store.find_template("AmericaVehicleMedic") {
        let mut g = new_game(&data);
        grant_all(&mut g, data.as_ref(), 0);
        let medic = g.create_object(&medic_t, Coord3D::new(600.0, 1000.0, 0.0), 0.0, 0);
        // A `KINDOF_CLEANUP_HAZARD` object (the toxin field) near the target area.
        let hazard_t = data.store.find_template("PoisonFieldMedium");
        if let Some(hazard_t) = hazard_t {
            let hazard = g.create_object(&hazard_t, Coord3D::new(1000.0, 1000.0, 0.0), 0.0, 1);
            for p in powers_of(&medic_t) {
                if matches!(medic_t.special_powers.iter().find(|x| x.power == p).map(|x| &x.effect), Some(SpecialPowerEffect::CleanupArea { .. })) {
                    g.submit(0, Command::DoSpecialPower { source: medic, power: p.clone(), target: SpecialPowerTarget::Position(Coord3D::new(1000.0, 1000.0, 0.0)) });
                }
            }
            g.run_frames(600);
            println!("CleanupArea: hazard alive {} (was cleaned)", g.find_object(hazard).is_some());
        } else {
            println!("CleanupArea: no toxin field template found");
        }
    }

    // ---- Cash Bounty (GLA Command Center) ---------------------------------------------------
    if let Some(gla_t) = data.store.find_template("Chem_GLACommandCenter") {
        let mut g = new_game(&data);
        grant_all(&mut g, data.as_ref(), 0);
        g.add_player(1, 0);
        let cc = g.create_object(&gla_t, Coord3D::new(600.0, 1000.0, 0.0), 0.0, 0);
        let _ = cc;
        let Some(victim_t) = data.store.find_template("ChinaTankBattleMaster") else { return };
        let Some(attacker_t) = data.store.find_template("GLAVehicleTechnical") else { return };
        let attacker = g.create_object(&attacker_t, Coord3D::new(700.0, 1000.0, 0.0), 0.0, 0);
        let victim = g.create_object(&victim_t, Coord3D::new(800.0, 1000.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        let money0 = g.player_money(0);
        g.damage_object(victim, zr_sim::DamageInfo { source: attacker, amount: 9999.0, kind: zr_sim::DamageType::Unresistable });
        g.run_frames(2);
        println!("CashBounty: player money {money0} -> {} after killing a Battlemaster", g.player_money(0));
    }

    // ---- Baikonur Launch Power --------------------------------------------------------------
    if let Some(pad_t) = data.store.find_template("BaikonurRocketPad") {
        let mut g = new_game(&data);
        grant_all(&mut g, data.as_ref(), 0);
        let pad = g.create_object(&pad_t, Coord3D::new(600.0, 1000.0, 0.0), 0.0, 0);
        let before = g.object_count();
        for p in powers_of(&pad_t) {
            g.submit(0, Command::DoSpecialPower { source: pad, power: p.clone(), target: SpecialPowerTarget::Position(Coord3D::new(1200.0, 1000.0, 0.0)) });
        }
        g.run_frames(30);
        println!("Baikonur: objects {before} -> {}", g.object_count());
    }

    // ---- UpgradeDie (America drone removes its producer's upgrade) --------------------------
    if let Some(drone_t) = data.store.find_template("AmericaVehicleScoutDrone") {
        let mut g = new_game(&data);
        let Some(prod_t) = data.store.find_template("AmericaCommandCenter") else { return };
        let prod = g.create_object(&prod_t, Coord3D::new(600.0, 1000.0, 0.0), 0.0, 0);
        let drone = g.create_object(&drone_t, Coord3D::new(650.0, 1000.0, 0.0), 0.0, 0);
        g.set_producer(drone, prod);
        // Give the producer the upgrade the drone's `UpgradeDie` names.
        let up = drone_t
            .die
            .iter()
            .find_map(|d| match &d.kind {
                zr_sim::DieTemplate::Upgrade { upgrade } => upgrade.clone(),
                _ => None,
            });
        if let Some(up) = up {
            g.give_upgrade(prod, &up);
            let before = g.object_upgrades(prod).any(|u| *u == up);
            g.damage_object(drone, zr_sim::DamageInfo { source: ObjectId::INVALID, amount: 9999.0, kind: zr_sim::DamageType::Unresistable });
            g.run_frames(2);
            let after = g.object_upgrades(prod).any(|u| *u == up);
            println!("UpgradeDie: producer had upgrade {up}: {before} -> {after}");
        } else {
            println!("UpgradeDie: drone has no Upgrade die module");
        }
    }

    // ---- Helicopter slow death (Comanche) ---------------------------------------------------
    if let Some(comanche_t) = data.store.find_template("AmericaVehicleComanche") {
        let mut g = new_game(&data);
        let c = g.create_object(&comanche_t, Coord3D::new(600.0, 1000.0, 60.0), 0.0, 0);
        g.damage_object(c, zr_sim::DamageInfo { source: ObjectId::INVALID, amount: 9999.0, kind: zr_sim::DamageType::Unresistable });
        let mut destroyed_at = None;
        for f in 0..900 {
            g.run_frames(1);
            if g.find_object(c).is_none() {
                destroyed_at = Some(f);
                break;
            }
        }
        println!("HelicopterSlowDeath: Comanche destroyed at +{:?} frames", destroyed_at);
    }

    println!("done");
}
