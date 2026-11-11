//! Subdual damage on a unit with a `SubdualDamageCap`, through `GameLogic::damage_object`, on synthetic data.
//!
//! The subdual branch of `ActiveBody::attemptDamage` (ActiveBody.cpp:495-513) accumulates the damage and leaves
//! health alone; the game-side half (`onSubdualChange`, `DISABLED_SUBDUED`, ActiveBody.cpp:1285-1321) runs from the
//! damage path. The heal (`SubdualDamageHelper::update`, SubdualDamageHelper.cpp:56-75) takes the subdual damage down
//! `SubdualDamageHealAmount` every `SubdualDamageHealRate` frames; once the unit is no longer subdued its
//! `DISABLED_SUBDUED` must clear, which is the sleepy-loop side of the heal (not yet wired: see the report).

use std::sync::Arc;
use zr_sim::body::DamageInfo;
use zr_sim::damage::DamageType;
use zr_sim::disabled::DisabledType;
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::{Coord3D, GameLogic, ObjectId, ObjectTemplate, TerrainLogic};

/// 100 max health, a 1000 subdual cap, and a heal of 50 every 10 frames.
fn subduable() -> ObjectTemplate {
    let mut t = ObjectTemplate::new("Tank");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.subdual_damage_cap = 1000.0;
    t.subdual_heal_rate = 10;
    t.subdual_heal_amount = 50.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t
}

fn hit(amount: f32) -> DamageInfo {
    DamageInfo { source: ObjectId::INVALID, amount, kind: DamageType::SubdualVehicle }
}

fn unit(g: &mut GameLogic) -> ObjectId {
    g.create_object(&Arc::new(subduable()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0)
}

/// `ActiveBody::onSubdualChange` (ActiveBody.cpp:1285-1321): the subdual damage reaches the max health, so the unit is
/// subdued and `DISABLED_SUBDUED` is set. The subdual hit does not touch health.
/// Revert-check (orchestrator's logic.rs edits): without `with_subdual(...)` on the body the cap is 0, the hit is
/// ignored and the first assert fails; without `after_subdual_hit` the body is subdued but the disable is never set,
/// and the second assert fails.
#[test]
fn a_subdual_hit_that_reaches_max_health_disables_the_unit() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let id = unit(&mut g);
    g.damage_object(id, hit(100.0));
    assert!(g.find_object(id).unwrap().body.as_ref().unwrap().is_subdued(), "100 subdual damage on 100 max health");
    assert!(g.find_object(id).unwrap().is_disabled_by_type(DisabledType::Subdued));
    assert_eq!(g.find_object(id).unwrap().health(), 100.0, "subdual damage never touches health");
}

/// The first heal step (SubdualDamageHelper.cpp:56-75): the countdown is the heal rate (10 frames) when the hit lands
/// at frame 0, so the step runs in the update of frame 10 and takes 100 subdual damage to 50, below the max. This
/// passes without lane 107: it only needs the body's accounting.
#[test]
fn the_heal_takes_the_subdual_damage_below_the_max() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let id = unit(&mut g);
    g.damage_object(id, hit(100.0));
    g.run_frames(9);
    assert!(g.find_object(id).unwrap().body.as_ref().unwrap().is_subdued(), "no heal step before frame 10");
    g.run_frames(2);
    assert!(!g.find_object(id).unwrap().body.as_ref().unwrap().is_subdued(), "the first step heals 50 of 100");
    assert_eq!(g.find_object(id).unwrap().health(), 100.0);
}

/// The heal clears `DISABLED_SUBDUED` once the unit is no longer subdued (`ActiveBody::onSubdualChange` with
/// `isNowSubdued == false`, ActiveBody.cpp:1302). The heal runs in the sleepy loop's SubdualDamage helper update,
/// which has no `GameLogic` access, so this clear needs lane 107's wiring (the orchestrator's hook 5). Until then this
/// test fails on its last assert, and only that assert depends on lane 107.
#[test]
fn the_heal_clears_the_subdued_disable() {
    let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
    let id = unit(&mut g);
    g.damage_object(id, hit(100.0));
    assert!(g.find_object(id).unwrap().is_disabled_by_type(DisabledType::Subdued));
    g.run_frames(11);
    assert!(!g.find_object(id).unwrap().body.as_ref().unwrap().is_subdued(), "healed below the max");
    assert!(!g.find_object(id).unwrap().is_disabled_by_type(DisabledType::Subdued), "the disable clears with the heal (lane 107)");
}
