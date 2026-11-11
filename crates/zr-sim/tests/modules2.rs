//! Round-2 modules: crush, income, base regen, fire weapon, veterancy gain, horde, topple, spawn,
//! eject pilot and crates. Synthetic data only.

use std::sync::Arc;

use zr_sim::behaviors::BehaviorState;
use zr_sim::body::DamageInfo;
use zr_sim::damage::DamageType;
use zr_sim::modules2::{
    AutoDepositTemplate, CrateCollideTemplate, CrateEffect, CrushInfo, EjectPilotTemplate, FireWeaponTemplate,
    HordeTemplate, SpawnTemplate, ToppleTemplate, VeterancyGainTemplate,
};
use zr_sim::ocl::{OclNugget, OclTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams, SimpleTemplateStore};
use zr_sim::weapon::WeaponTemplate;
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn flat() -> TerrainLogic {
    TerrainLogic::flat(100, 100, 0)
}

fn unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

/// `CrusherLevel` > `CrushableLevel` (or `SquishCollide`) lets a moving unit crush its victim.
#[test]
fn crusher_crushes_squishable_victim() {
    let mut g = GameLogic::new(flat(), 1);
    let mut ct = unit("Crusher");
    ct.crush = CrushInfo { crusher_level: 1, crushable_level: 255, squish_collide: false };
    // `checkForOverlapCollision` lives on `PhysicsBehavior`, so a crusher needs physics (the old
    // sim crush pass did not require it; the engine does).
    ct.physics = Some(zr_sim::PhysicsTemplate::default());
    let mut vt = unit("Infantry");
    vt.crush = CrushInfo { crusher_level: 0, crushable_level: 0, squish_collide: true };
    // The victim is an enemy: an allied unit is pushed aside by `moveAllies`, not crushed.
    let victim = g.create_object(&Arc::new(vt), Coord3D::new(300.0, 200.0, 0.0), 0.0, 1);
    let crusher = g.create_object(&Arc::new(ct), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![crusher], pos: Coord3D::new(400.0, 200.0, 0.0) });
    g.run_frames(120);
    assert!(g.find_object(victim).is_none() || g.find_object(victim).unwrap().is_dead(), "the victim was crushed");
}

/// `AutoDepositUpdate` deposits money on its timer.
#[test]
fn auto_deposit_pays_the_owner() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut t = unit("Derrick");
    t.auto_deposit = Some(AutoDepositTemplate { deposit_timing: 30, deposit_amount: 100, initial_capture_bonus: 0, upgraded_boost: vec![] });
    g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(31);
    assert!(g.player(0).unwrap().money >= 100, "the derrick paid out");
}

/// `AutoDepositUpdate::update` (`AutoDepositUpdate.cpp:161-162`): "getObject()->isNeutralControlled()
/// -> return" — a neutral-owned derrick (a map oil derrick the neutral player holds) pays nobody.
/// AN3 s1: the port paid the neutral player 4 x 200 at frame 361, the C++ 0 (the first snapshot
/// divergence after the worker 229 move fix).
#[test]
fn a_neutral_derrick_pays_nobody() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_neutral_player(Some(0));
    let mut t = unit("NeutralDerrick");
    t.auto_deposit = Some(AutoDepositTemplate { deposit_timing: 30, deposit_amount: 200, initial_capture_bonus: 0, upgraded_boost: vec![] });
    g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(31);
    assert_eq!(g.player(0).unwrap().money, 0, "the neutral player is not paid");
}

/// `AutoDepositUpdate::update` (`AutoDepositUpdate.cpp:145-166`): `m_depositOnFrame = now + m_depositFrame`
/// runs *before* the neutral / zero-amount / under-construction returns, so a neutral derrick re-arms its
/// deposit frame every time it comes due. The port returned before advancing, so the frame stayed in the
/// past and the next owner was paid one interval early.
#[test]
fn a_neutral_derrick_still_re_arms_its_deposit_timer() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    g.set_neutral_player(Some(0));
    let mut t = unit("NeutralDerrick");
    t.auto_deposit = Some(AutoDepositTemplate { deposit_timing: 30, deposit_amount: 200, initial_capture_bonus: 0, upgraded_boost: vec![] });
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    // `AutoDepositUpdate`'s constructor sets `m_depositOnFrame = getFrame() + m_depositFrame`
    // (AutoDepositUpdate.cpp:104); the module does not latch it on its first update.
    assert_eq!(g.find_object(id).unwrap().behavior.deposit_next, 30, "the constructor's m_depositOnFrame");
    g.run_frames(40);
    assert_eq!(g.player(0).unwrap().money, 0, "the neutral player is not paid");
    // Frame 30's payout is suppressed as neutral but still re-arms to 60.
    assert_eq!(
        g.find_object(id).unwrap().behavior.deposit_next,
        60,
        "the timer advances on the suppressed frame"
    );
}

/// `BaseRegenerateUpdate` heals the object.
#[test]
fn base_regenerate_heals() {
    let mut g = GameLogic::new(flat(), 1);
    let rules = zr_sim::GameRules { base_regen_health_percent_per_second: 1.0, ..Default::default() };
    g.set_rules(rules);
    let mut t = unit("Base");
    t.base_regenerate = true;
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.damage_object(id, DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 50.0, kind: DamageType::Explosion });
    let before = g.find_object(id).unwrap().health();
    g.run_frames(31);
    assert!(g.find_object(id).unwrap().health() > before, "base regenerated");
}

/// `FireWeaponUpdate` fires its weapon periodically.
#[test]
fn fire_weapon_update_fires() {
    let mut g = GameLogic::new(flat(), 1);
    let mut w = WeaponTemplate::new("Burst");
    w.primary_damage = 5.0;
    w.damage_type = DamageType::Explosion;
    w.attack_range = 300.0;
    w.has_projectile = true;
    w.weapon_speed = 60.0;
    let mut t = unit("Turret");
    t.fire_weapon = Some(FireWeaponTemplate { weapon: Arc::new(w), initial_delay: 0, exclusive_weapon_delay: 30 });
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(2);
    // The module fired on the first frame (it re-arms `fire_next` after firing).
    assert!(g.find_object(id).unwrap().behavior.fire_next != 0, "the module fired");
}

/// `VeterancyGainCreate` starts the object at a level.
#[test]
fn veterancy_gain_create_sets_level() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = unit("Vet");
    t.experience_required = [0, 10, 20, 30];
    t.veterancy_gain = Some(VeterancyGainTemplate { starting_level: 2, science_required: None });
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    assert_eq!(g.find_object(id).unwrap().experience.level.index(), 2, "starts at Elite");
}

/// `HordeUpdate` sets the horde flag when enough same-kind allies are near.
#[test]
fn horde_update_sets_flag() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut t = unit("Horde");
    t.horde = Some(HordeTemplate {
        update_rate: 1,
        kind_of: kind_of::VEHICLE,
        count: 2,
        radius: 100.0,
        rub_off_radius: 20.0,
        allies_only: true,
        exact_match: false,
        allowed_nationalism: true,
    });
    let a = g.create_object(&Arc::new(t.clone()), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.create_object(&Arc::new(t), Coord3D::new(220.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(3);
    assert!(g.find_object(a).unwrap().behavior.horde, "in horde");
}

/// `ToppleUpdate` kills the object when the fall finishes.
#[test]
fn topple_kills_when_finished() {
    let mut g = GameLogic::new(flat(), 1);
    let mut t = unit("Tree");
    t.topple = Some(ToppleTemplate { kill_when_start_toppling: false, kill_when_finished_toppling: true, topple_left_or_right_only: false });
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.topple(id, false);
    g.run_frames(31);
    assert!(g.find_object(id).is_none(), "toppled and died");
}

/// `SpawnBehavior` creates its slaves on construction.
#[test]
fn spawn_behavior_creates_slaves() {
    let mut g = GameLogic::new(flat(), 1);
    let mut store = SimpleTemplateStore::default();
    let slave = store.add(unit("Slave"));
    let _ = slave;
    g.set_template_source(Arc::new(store));
    let mut t = unit("Stinger");
    t.spawn = Some(SpawnTemplate {
        spawn_number: 2,
        spawn_replace_delay: 0,
        one_shot: false,
        spawn_template_names: vec!["Slave".into()],
        spawned_require_spawner: false,
        initial_burst: 0,
        aggregate_health: false,
    });
    let nexus = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    // A spawner placed at the start: its second entry (listIndex 1) is past one SPAWN_UPDATE_RATE later
    // (SpawnBehavior.cpp:220-223, :230-235, :249).
    g.run_frames(17);
    let slaves = g.objects().filter(|o| o.producer_id == nexus).count();
    assert_eq!(slaves, 2, "two slaves spawned (their producer is the spawner, `SpawnBehavior.cpp:655`)");
}

/// `EjectPilotDie` runs its OCL on death.
#[test]
fn eject_pilot_die_spawns_pilot() {
    let mut g = GameLogic::new(flat(), 1);
    let mut store = SimpleTemplateStore::default();
    let pilot = store.add(unit("Pilot"));
    g.set_template_source(Arc::new(store));
    let pilot_ocl = Arc::new(OclTemplate {
        name: "PilotOCL".into(),
        nuggets: vec![OclNugget::CreateObject(zr_sim::ocl::CreateObjectTemplate {
            is_object: true,
            names_len: 1,
            objects: vec![Some(pilot)],
            count: 1,
            offset: (0.0, 0.0, 0.0),
            disposition: 0,
            ..Default::default()
        })],
    });
    let mut t = unit("Tank");
    t.eject_pilot = Some(EjectPilotTemplate { air_ocl: None, ground_ocl: Some(pilot_ocl) });
    let id = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    g.damage_object(id, DamageInfo { source: zr_sim::ObjectId::INVALID, amount: 200.0, kind: DamageType::Explosion });
    assert!(g.objects().any(|o| o.template.name == "Pilot"), "the pilot ejected");
}

/// A money crate is picked up by a nearby unit.
#[test]
fn money_crate_is_picked_up() {
    let mut g = GameLogic::new(flat(), 1);
    g.add_player(0, 0);
    let mut crate_t = unit("Crate");
    crate_t.locomotor = None;
    crate_t.crate_collides = vec![CrateCollideTemplate { effect: CrateEffect::Money { money_provided: 50, upgrade_boost: vec![] }, ..Default::default() }];
    let crate_id = g.create_object(&Arc::new(crate_t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let picker = unit("Picker");
    g.create_object(&Arc::new(picker), Coord3D::new(205.0, 200.0, 0.0), 0.0, 0);
    g.run_frames(2);
    assert!(g.find_object(crate_id).is_none(), "crate consumed");
    assert!(g.player(0).unwrap().money >= 50, "money paid");
    let _ = BehaviorState::default();
}
