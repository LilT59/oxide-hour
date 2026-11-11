//! `DAMAGE_STATUS` through an attack (ActiveBody.cpp:483-490, StatusDamageHelper.cpp:80-92), on synthetic data only.
//!
//! A status weapon's damage is a duration in milliseconds: the victim gets the status for that long and no health is
//! lost. The duration is `ConvertDurationFromMsecsToFrames` rounded up (GameCommon.h:70, `REAL_TO_INT_CEIL`).

use std::sync::Arc;
use zr_sim::damage::DamageType;
use zr_sim::object::status;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn shooter(weapon: WeaponTemplate) -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Stunner");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 200.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(weapon));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    t
}

fn target() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Target");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

/// 2000 ms (primary damage) of `IMMOBILE` (bit 42, the C++ `OBJECT_STATUS_IMMOBILE` index), one shot.
fn status_gun() -> WeaponTemplate {
    let mut w = WeaponTemplate::new("Stun");
    w.primary_damage = 2000.0;
    w.damage_type = DamageType::Status;
    w.attack_range = 150.0;
    w.min_delay_between_shots = 1000;
    w.max_delay_between_shots = 1000;
    w.damage_status_mask = status::IMMOBILE;
    w
}

/// The status is set by the shot and held for 60 frames (2000 ms at 30 frames a second), then cleared by the
/// helper's update (`StatusDamageHelper::update`, StatusDamageHelper.cpp:58-65). Health is untouched.
/// Without the status branch in `apply_damage`'s caller the shot does 2000 health damage, and the status is never set.
#[test]
fn a_status_shot_holds_its_status_for_its_milliseconds_and_leaves_health() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let a = g.create_object(&Arc::new(shooter(status_gun())), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
    let b = g.create_object(&Arc::new(target()), Coord3D::new(260.0, 200.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    let mut set_at = None;
    for _ in 0..200 {
        g.run_frames(1);
        if g.find_object(b).unwrap().status_mask(g.frame()) & status::IMMOBILE != 0 {
            set_at = Some(g.frame());
            break;
        }
    }
    let set_at = set_at.expect("the status shot sets IMMOBILE");
    assert_eq!(g.find_object(b).unwrap().health(), 1000.0, "a status shot does no health damage");
    let mut clear_at = None;
    for _ in 0..200 {
        g.run_frames(1);
        if g.find_object(b).unwrap().status_mask(g.frame()) & status::IMMOBILE == 0 {
            clear_at = Some(g.frame());
            break;
        }
    }
    let clear_at = clear_at.expect("the status clears");
    assert_eq!(clear_at - set_at, 60, "2000 ms is 60 frames");
    assert_eq!(g.find_object(b).unwrap().health(), 1000.0);
}
