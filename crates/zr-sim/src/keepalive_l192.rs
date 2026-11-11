//! Lane 192 (R28 F0): does an attacker that stands still, or an immobile structure, keep updating every frame while it
//! attacks, without any explicit wake? C++ keeps it awake through the attack state's return: `AIAttackState::update`
//! returns `CONVERT_SLEEP_TO_CONTINUE(...)` (AIStates.cpp:5710), a CONTINUE leaves `subMachineSleep` at NONE
//! (AIUpdate.cpp:1009-1025), and the module re-runs next frame. The port's `ai_update_sleep` has no sleep pending in
//! that case, so it reports NONE. These tests check the cadence over several reload cycles.

use std::sync::Arc;

use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::ObjectId;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use crate::weapon::{WeaponSetTemplate, WeaponTemplate};

/// A gun with a 30-frame reload and 10 damage per hit, in range of the target.
fn gunner(name: &str, mobile: bool) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = if mobile {
        kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK
    } else {
        kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE | kind_of::CAN_ATTACK
    };
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.ai_update = Some(0);
    if mobile {
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
    }
    let mut w = WeaponTemplate::new("Gun");
    w.primary_damage = 10.0;
    w.attack_range = 100.0;
    w.min_delay_between_shots = 30;
    w.max_delay_between_shots = 30;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat.weapon_set = Some(set);
    t
}

/// A target that never fights back and survives the whole run.
fn dummy_target() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Dummy");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 5.0;
    t.max_health = 100_000.0;
    t.initial_health = 100_000.0;
    t
}

/// Order `attacker` onto `victim` and run `frames` frames; returns the number of hits (health drops).
fn hits_over(g: &mut GameLogic, attacker: ObjectId, victim: ObjectId, frames: u32) -> u32 {
    g.ai_attack_object(attacker, victim, false, false, None);
    let health = |g: &GameLogic| g.objects[&victim].body.as_ref().expect("a body").health();
    let mut last = health(g);
    let mut hits = 0;
    for _ in 0..frames {
        g.run_frames(1);
        let now = health(g);
        if now < last {
            hits += 1;
        }
        last = now;
    }
    hits
}

fn setup(attacker: ObjectTemplate, attacker_pos: Coord3D) -> (GameLogic, ObjectId, ObjectId) {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    let a = g.create_object(&Arc::new(attacker), attacker_pos, 0.0, 0);
    let v = g.create_object(&Arc::new(dummy_target()), Coord3D::new(attacker_pos.x + 50.0, attacker_pos.y, 0.0), 0.0, 1);
    (g, a, v)
}

/// A stationary mobile gunner fires once per reload over 300 frames (about 10 shots). Revert check: a sleep that
/// stops the module after one attack update (`ai_update_sleep` forced to FOREVER for an attack) leaves it at one hit.
#[test]
fn a_stationary_gunner_keeps_firing_over_many_reload_cycles() {
    let (mut g, a, v) = setup(gunner("Ranger", true), Coord3D::new(200.0, 200.0, 0.0));
    let hits = hits_over(&mut g, a, v, 300);
    assert!((9..=11).contains(&hits), "a stationary attacker keeps its 30-frame cadence: {hits} hits in 300 frames");
}

/// An immobile structure with an AI module fires over the same run (its `doLocomotor` returns FOREVER, but the
/// attack's CONTINUE keeps the module awake: AIUpdate.cpp:1114-1116 min's FOREVER in). Revert check as above.
#[test]
fn an_immobile_gun_keeps_firing_over_many_reload_cycles() {
    let (mut g, a, v) = setup(gunner("Bunker", false), Coord3D::new(200.0, 200.0, 0.0));
    let hits = hits_over(&mut g, a, v, 300);
    assert!((9..=11).contains(&hits), "an immobile attacker keeps its 30-frame cadence: {hits} hits in 300 frames");
}
