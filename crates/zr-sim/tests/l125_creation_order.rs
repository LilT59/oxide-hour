//! Lane 125: the creation-time RNG draws follow `Object::Object`'s module construction order (R10 finding 11).
//!
//! `Object::Object` builds the behaviour modules with `mi.getNthName(modIdx)` (the template's INI order,
//! `Object.cpp:300-410`), so each module constructor's `GameLogicRandomValue` draw happens at that module's INI
//! position: `LifetimeUpdate::calcSleepDelay` (`LifetimeUpdate.cpp:77`), `DeletionUpdate::calcSleepDelay`
//! (`DeletionUpdate.cpp:74`), the `AIUpdateInterface` constructor's turret idle scan and locomotor draws
//! (`AIUpdate.cpp:288-296`, `TurretAI.cpp:1286-1291`, `Locomotor.cpp:673-675`), `StealthDetectorUpdate`'s phase
//! (`StealthDetectorUpdate.cpp:88-90`) and `AutoHealBehavior`'s starts-active phase (`AutoHealBehavior.cpp:98-105`).
//! `onObjectCreated` (the AI state machine's idle entries) runs only after all of them (`Object.cpp:474-477`).
//!
//! Before this lane the port drew in one fixed order (lifetime, deletion, AI, stealth, then the idle entries, then
//! AutoHeal), so a template whose INI lists the modules differently consumed the random stream in a different order
//! from C++.

use std::sync::Arc;
use zr_sim::behaviors::{AutoHealTemplate, StealthDetectorTemplate};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams, SleepyModuleKind};
use zr_sim::{Coord3D, GameLogic, ObjectTemplate, TerrainLogic};

fn logic() -> GameLogic {
    GameLogic::new(TerrainLogic::flat(80, 80, 0), 1)
}

fn unit(name: &str) -> ObjectTemplate {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t
}

/// One `SET_NORMAL` locomotor: the AI constructor's `chooseLocomotorSet` draws three values per locomotor
/// (`Locomotor.cpp:673-675`), and the state machine's idle entries come later.
fn with_ai(mut t: ObjectTemplate) -> ObjectTemplate {
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    t.ai_update = Some(1);
    t
}

fn with_detector(mut t: ObjectTemplate, rate: u32) -> ObjectTemplate {
    t.stealth_detector = Some(StealthDetectorTemplate {
        detection_rate: rate,
        detection_range: 100.0,
        initially_disabled: false,
        extra_required_kind_of: 0,
        extra_forbidden_kind_of: 0,
        can_detect_while_garrisoned: false,
        can_detect_while_contained: false,
    });
    t
}

fn with_auto_heal(mut t: ObjectTemplate, delay: u32) -> ObjectTemplate {
    t.auto_heal = vec![AutoHealTemplate {
        healing_amount: 10.0,
        healing_delay: delay,
        start_healing_delay: 0,
        radius: 0.0,
        affects_whole_player: false,
        skip_self_for_healing: false,
        single_burst: false,
        kind_of: kind_of::VEHICLE,
        forbidden_kind_of: 0,
        starts_active: true,
        triggered_by: vec![],
        conflicts_with: vec![],
    }];
    t
}

/// `StealthDetectorUpdate`'s first wake is `creation + GameLogicRandomValue(1, DetectionRate)`
/// (StealthDetectorUpdate.cpp:88-90), so it pins the random stream's position. With the detector listed before the
/// `AIUpdate` it is the object's first constructor draw; with the AI listed first, the AI's three locomotor draws
/// are consumed first. Revert check: the old fixed order always put the AI first, so the "detector first" template
/// got the fourth draw and the two values became equal.
#[test]
fn the_module_constructors_draw_in_the_templates_ini_order() {
    let only_detector = Arc::new(with_detector(unit("OnlyDetector"), 500));
    let mut g = logic();
    let plain = g.create_object(&only_detector, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let first_draw = g.find_object(plain).unwrap().behavior.detect_next;
    assert!(first_draw > 0, "the detector drew its phase");

    let mut detector_first = with_detector(with_ai(unit("DetectorFirst")), 500);
    detector_first.update_module_order = vec![SleepyModuleKind::StealthDetector, SleepyModuleKind::Ai];
    let mut g = logic();
    let id = g.create_object(&Arc::new(detector_first), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    assert_eq!(
        g.find_object(id).unwrap().behavior.detect_next,
        first_draw,
        "the detector's constructor is the first module, so it gets the stream's first draw"
    );

    let mut ai_first = with_detector(with_ai(unit("AiFirst")), 500);
    ai_first.update_module_order = vec![SleepyModuleKind::Ai, SleepyModuleKind::StealthDetector];
    let mut g = logic();
    let id = g.create_object(&Arc::new(ai_first), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let after_ai = g.find_object(id).unwrap().behavior.detect_next;
    assert_ne!(
        after_ai, first_draw,
        "with the AI listed first its three locomotor draws come before the detector's phase"
    );
}

/// `AutoHealBehavior`'s starts-active constructor draws `GameLogicRandomValue(1, HealingDelay)`
/// (`AutoHealBehavior.cpp:98-105`) and wakes on it. Its wake pins where the AutoHeal module sits in the
/// construction order: before the AI it is the first draw, after it the fourth. Revert check: the old code drew the
/// AutoHeal phase after the AI idle entries, so the "AutoHeal first" template's wake differed from the only-AutoHeal
/// baseline.
#[test]
fn a_starts_active_auto_heal_draws_at_its_module_position() {
    let only = Arc::new(with_auto_heal(unit("OnlyHeal"), 30));
    let mut g = logic();
    let id = g.create_object(&only, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let handle = g.find_object(id).unwrap().behavior.auto_heal[0].handle.expect("registered");
    let first_draw = g.update_wake_frame(handle);

    let mut heal_first = with_auto_heal(with_ai(unit("HealFirst")), 30);
    heal_first.update_module_order = vec![SleepyModuleKind::AutoHeal, SleepyModuleKind::Ai];
    let mut g = logic();
    let id = g.create_object(&Arc::new(heal_first), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let handle = g.find_object(id).unwrap().behavior.auto_heal[0].handle.expect("registered");
    assert_eq!(g.update_wake_frame(handle), first_draw, "AutoHeal is the first module, so it gets the first draw");

    let mut ai_first = with_auto_heal(with_ai(unit("AiFirstHeal")), 30);
    ai_first.update_module_order = vec![SleepyModuleKind::Ai, SleepyModuleKind::AutoHeal];
    let mut g = logic();
    let id = g.create_object(&Arc::new(ai_first), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let handle = g.find_object(id).unwrap().behavior.auto_heal[0].handle.expect("registered");
    assert_ne!(
        g.update_wake_frame(handle),
        first_draw,
        "the AI's locomotor draws come before an AutoHeal listed after it"
    );
}

/// `LifetimeUpdate` and `DeletionUpdate` both roll `GameLogicRandomValue(min, max)` in their constructors
/// (`LifetimeUpdate.cpp:77`, `DeletionUpdate.cpp:74`). A template listing `DeletionUpdate` first therefore gives it
/// the stream's first draw, and one listing `LifetimeUpdate` first gives the lifetime the first draw. Revert check:
/// the old code always rolled the lifetime first, so the "deletion first" template's deletion got the second draw.
#[test]
fn lifetime_and_deletion_roll_in_their_ini_order() {
    let mut only_lifetime = unit("OnlyLifetime");
    only_lifetime.lifetime = Some((10, 100));
    let mut g = logic();
    let id = g.create_object(&Arc::new(only_lifetime), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let first_draw = g.find_object(id).unwrap().behavior.lifetime_die;
    assert!(first_draw > 1, "the lifetime rolled a delay");

    let mut deletion_before_lifetime = unit("DeletionFirst");
    deletion_before_lifetime.deletion = Some((10, 100));
    deletion_before_lifetime.lifetime = Some((10, 100));
    deletion_before_lifetime.update_module_order = vec![SleepyModuleKind::Deletion, SleepyModuleKind::Lifetime];
    let mut g = logic();
    let id = g.create_object(&Arc::new(deletion_before_lifetime), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.behavior.deletion_die, first_draw, "the deletion module is constructed first");
    assert_ne!(o.behavior.lifetime_die, first_draw, "the lifetime takes the second draw");

    let mut lifetime_before_deletion = unit("LifetimeFirst");
    lifetime_before_deletion.deletion = Some((10, 100));
    lifetime_before_deletion.lifetime = Some((10, 100));
    lifetime_before_deletion.update_module_order = vec![SleepyModuleKind::Lifetime, SleepyModuleKind::Deletion];
    let mut g = logic();
    let id = g.create_object(&Arc::new(lifetime_before_deletion), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.behavior.lifetime_die, first_draw, "the lifetime module is constructed first");
    assert_ne!(o.behavior.deletion_die, first_draw, "the deletion takes the second draw");
}
