//! Lane 194 round 4 (AN5 s1 f3763): `AIUpdateInterface::blockedBy` returns FALSE when the unit is moving backwards
//! (`if (getCurLocomotor() && getCurLocomotor()->isMovingBackwards()) return false;`, AIUpdate.cpp:1316). The troop crawler
//! has `CanMoveBackwards = Yes`; the port stopped the crawler against a rider it had passed.

use crate::math::Coord3D;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use crate::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use crate::GameLogic;
use std::sync::Arc;

fn vehicle() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Crawler");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 11.0;
    t.max_health = 240.0;
    t.initial_health = 240.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 40.0, 120.0, 1000.0, 1000.0));
    t.physics = Some(crate::physics::PhysicsTemplate { mass: 10.0, ..crate::physics::PhysicsTemplate::default() });
    Arc::new(t)
}

fn infantry() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Rebel");
    t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
    t.geometry.major_radius = 4.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
    Arc::new(t)
}

/// A crawler with an infantry 6 units ahead of it. Without the backing-up flag the infantry blocks it; with the flag set (the
/// locomotor's `MOVING_BACKWARDS`, here `PhysicsBehavior::moving_backwards`) it does not. Revert check: removing the early
/// return in `unit_blocked_by` makes the second assertion fail.
#[test]
fn a_unit_moving_backwards_is_not_blocked_by_the_unit_in_front() {
    let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
    let c = g.create_object(&vehicle(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let r = g.create_object(&infantry(), Coord3D::new(106.0, 100.0, 0.0), 0.0, 1);
    g.run_frames(1); // the physics module is created on the first frame
    assert!(g.unit_blocked_by(c, r), "a forward unit is blocked by the infantry ahead");
    g.objects.get_mut(&c).expect("crawler").physics.as_mut().expect("it has a locomotor").moving_backwards = true;
    assert!(!g.unit_blocked_by(c, r), "a unit moving backwards is not blocked (AIUpdate.cpp:1316)");
}

/// An armed vehicle with a 40-unit cannon on treads: its attack approaches on an attack path.
fn gunner() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Gunner");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 50.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 120.0, 120.0, 360.0));
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 20.0;
    w.damage_type = crate::body::DamageType::ArmorPiercing;
    w.attack_range = 40.0;
    w.min_delay_between_shots = 10;
    w.max_delay_between_shots = 10;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    Arc::new(t)
}

/// A tough vehicle that never moves or fights.
fn target() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Target");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    Arc::new(t)
}

/// An approach on an attack path (`requestAttackPath`, AIUpdate.cpp:528) does not stop at the first in-range frame:
/// `m_stopIfInRange = !ai->isAttackPath()` (AIStates.cpp:2673), so the unit keeps walking the path and stays in the approach
/// after it is in range (the arrival at the path's end ends it, :1908-1940, :2722). Run until the unit is within its 40-unit
/// range (its range plus the two 8-unit radii), then one more frame: the approach is still running. Revert check:
/// set `approach_moving` to `false` in combat.rs (`in_range` alone decides); the phase is `Aim` on that frame.
#[test]
fn an_approach_on_an_attack_path_walks_on_through_the_in_range_frame() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let a = g.create_object(&gunner(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&target(), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    g.frame = 500;
    g.ai_attack_object_from(a, v, false, false, None, crate::ai_states::CommandSource::Ai);
    let phase = |g: &GameLogic| g.find_object(a).and_then(|o| o.combat.attack.as_ref().map(|x| x.phase));
    // The approach is chosen by the attack's first update, so the entry order itself stays at the default aim phase.
    g.run_frames(1);
    assert_eq!(phase(&g), Some(crate::combat::AttackPhase::Approach), "out of range: the attack approaches");
    // The weapon's range (37.5 for this cannon) plus the two 8-unit bounding radii is the in-range centre distance.
    let weapon_range = g.find_object(a).and_then(|o| o.combat.weapons.weapon(0).map(|w| w.template.attack_range())).expect("the cannon");
    let in_range_centre_distance = weapon_range + 16.0;
    let mut frames = 1;
    loop {
        g.run_frames(1);
        frames += 1;
        assert!(frames < 400, "the unit reaches its range");
        let d = g.find_object(a).expect("attacker").position.dist_2d(&g.find_object(v).expect("victim").position);
        if d <= in_range_centre_distance {
            break;
        }
    }
    g.run_frames(1);
    assert_eq!(phase(&g), Some(crate::combat::AttackPhase::Approach), "the approach walks on along its attack path");
}


/// Runs the attack until its approach has a path installed (the first pathfinder service), and returns the attacker and victim.
fn approaching_with_a_path() -> (GameLogic, crate::object::ObjectId, crate::object::ObjectId) {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let a = g.create_object(&gunner(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&target(), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    g.frame = 500;
    g.ai_attack_object_from(a, v, false, false, None, crate::ai_states::CommandSource::Ai);
    for _ in 0..200 {
        g.run_frames(1);
        if g.find_object(a).is_some_and(|o| o.ai.has_path()) {
            break;
        }
    }
    (g, a, v)
}

/// `AIInternalMoveToState::update` returns CONTINUE while the move waits for its path (AIStates.cpp:1786-1793), before the
/// arrival test. A unit standing on the path's end whose approach has just queued a fresh request (the approach re-entered
/// from aim) does not arrive on the old path. Revert check: drop the `waiting_for_path` gates from the arrival test in
/// combat.rs; the approach ends (Aim) on the first update.
#[test]
fn an_approach_with_a_fresh_path_still_queued_does_not_arrive_on_its_old_path() {
    let (mut g, a, _v) = approaching_with_a_path();
    let end = *g.find_object(a).expect("attacker").ai.path.last().expect("a path");
    {
        let o = g.objects.get_mut(&a).expect("attacker");
        o.position = end;
        o.ai.waiting_for_path = true;
        o.combat.attack.as_mut().expect("the attack").waiting_for_path = true;
    }
    g.ai_attack_update_object(a);
    assert_eq!(g.find_object(a).unwrap().combat.attack.as_ref().map(|x| x.phase), Some(crate::combat::AttackPhase::Approach), "the fresh request keeps the approach running");
}

/// `destroyPath` clears `m_isAttackPath` (AIUpdate.cpp:2026), so an approach that loses its attack path no longer walks on
/// through an in-range frame: `approach_moving` is false and the in-range success applies (AIStates.cpp:2710). Revert check:
/// drop the `attack_path_active` clear from `destroy_ai_path`; the approach stays in `Approach`.
#[test]
fn a_destroyed_attack_path_lets_an_in_range_approach_stop() {
    let (mut g, a, _v) = approaching_with_a_path();
    let weapon_range = g.find_object(a).and_then(|o| o.combat.weapons.weapon(0).map(|w| w.template.attack_range())).expect("the cannon");
    let in_range_centre_distance = weapon_range + 16.0;
    let v = _v;
    for _ in 0..400 {
        g.run_frames(1);
        let d = g.find_object(a).expect("attacker").position.dist_2d(&g.find_object(v).expect("victim").position);
        if d <= in_range_centre_distance {
            break;
        }
    }
    assert_eq!(g.find_object(a).unwrap().combat.attack.as_ref().map(|x| x.phase), Some(crate::combat::AttackPhase::Approach), "still approaching while on the attack path");
    g.destroy_ai_path(a);
    g.ai_attack_update_object(a);
    assert_eq!(g.find_object(a).unwrap().combat.attack.as_ref().map(|x| x.phase), Some(crate::combat::AttackPhase::Aim), "without an attack path the in-range approach stops");
}

/// A flyer with a 40-unit cannon: its locomotor flies (`surfaces_air`), so `landBound` is false (AIUpdate.cpp:1799-1805).
fn flyer(kind: LocomotorKind) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Flyer");
    t.kind_of = kind_of::AIRCRAFT | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 5.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 50.0;
    let mut loco = LocomotorParams::from_ini(kind, 60.0, 100.0, 100.0, 360.0);
    loco.surfaces_air = true;
    t.locomotor = Some(loco);
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 20.0;
    w.damage_type = crate::body::DamageType::ArmorPiercing;
    w.attack_range = 40.0;
    w.min_delay_between_shots = 10;
    w.max_delay_between_shots = 10;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    Arc::new(t)
}

/// Runs an attacking flyer far above the ground (z 60, the target 800 units away) until its attack path is installed, and
/// returns the number of nodes of its path. `computeAttackPath`'s airborne arm builds exactly two (start and approach target);
/// a ground search for the same target builds many. Revert check: remove the airborne arm from `do_pathfind`; the count is not 2.
fn flyer_attack_path_nodes(kind: LocomotorKind) -> (usize, f32) {
    let mut g = GameLogic::new(TerrainLogic::flat(1200, 1200, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let a = g.create_object(&flyer(kind), Coord3D::new(100.0, 100.0, 60.0), 0.0, 0);
    let v = g.create_object(&target(), Coord3D::new(900.0, 100.0, 0.0), 0.0, 1);
    g.frame = 500;
    g.ai_attack_object_from(a, v, false, false, None, crate::ai_states::CommandSource::Ai);
    for _ in 0..200 {
        g.run_frames(1);
        if g.find_object(a).is_some_and(|o| o.ai.has_path()) {
            break;
        }
    }
    let p = g.find_object(a).and_then(|o| o.ai.raw_path.clone()).expect("the flyer's attack path");
    (p.nodes.len(), p.nodes.last().map_or(f32::NAN, |n| n.pos.x))
}

/// A jet (`Wings`) attacking a far target flies a two-node airborne path (AIUpdate.cpp:1951-1956), not a ground search.
#[test]
fn an_attacking_jet_gets_the_two_node_airborne_path() {
    let (nodes, end_x) = flyer_attack_path_nodes(LocomotorKind::Wings);
    assert_eq!(nodes, 2, "the airborne arm's start and approach target");
    assert!(end_x < 890.0, "the path ends at the approach point, short of the victim at x 900 (got {end_x})");
}

/// A helicopter (`Thrust`, flying) attacking a far target gets the same two-node airborne path.
#[test]
fn an_attacking_helicopter_gets_the_two_node_airborne_path() {
    let (nodes, end_x) = flyer_attack_path_nodes(LocomotorKind::Thrust);
    assert_eq!(nodes, 2, "the airborne arm's start and approach target");
    assert!(end_x < 890.0, "the path ends at the approach point, short of the victim at x 900 (got {end_x})");
}

/// A contact weapon (range 10: under one pathfind cell, `WeaponTemplate::is_contact_weapon`) on an attack path: when it is in
/// range of an object victim the approach succeeds before the stop test (AIStates.cpp:2706-2709), so the unit goes to aim in
/// that frame and does not drive on along the path. Revert check: drop `contact_in_range` from `approach_moving` in combat.rs;
/// the approach is still running in that frame.
#[test]
fn a_contact_weapon_in_range_on_an_attack_path_ends_the_approach() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let mut t = (*gunner()).clone();
    if let Some(set) = t.combat.weapon_set.as_mut() {
        if let Some(Some(w)) = set.weapons.get_mut(0).map(|s| s.as_mut().map(Arc::make_mut)) {
            w.attack_range = 10.0;
        }
    }
    let a = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&target(), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    g.frame = 500;
    g.ai_attack_object_from(a, v, false, false, None, crate::ai_states::CommandSource::Ai);
    let phase = |g: &GameLogic| g.find_object(a).and_then(|o| o.combat.attack.as_ref().map(|x| x.phase));
    let range = g.find_object(a).and_then(|o| o.combat.weapons.weapon(0).map(|w| w.template.attack_range())).expect("the contact cannon");
    assert!(range < 12.5, "the cannon is a contact weapon (range {range})");
    let in_range_centre_distance = range + 16.0;
    for _ in 0..400 {
        g.run_frames(1);
        let d = g.find_object(a).expect("attacker").position.dist_2d(&g.find_object(v).expect("victim").position);
        if d <= in_range_centre_distance {
            break;
        }
    }
    g.run_frames(1);
    assert_eq!(phase(&g), Some(crate::combat::AttackPhase::Aim), "a contact weapon in range goes to aim, not on along its path");
}

/// A 400x400 map with a ridge of height 125 across world x 140..170 (the cells 14..=16), as in `los_l180.rs`.
fn ridge_map() -> TerrainLogic {
    let (w, h) = (400, 400);
    let mut data = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 14..17 {
            data[(x + y * w) as usize] = 200;
        }
    }
    TerrainLogic::from_samples(w, h, 0, data)
}

/// An approach in range of a ground victim whose line is blocked by the ridge does not stop: the stop test needs the view
/// clear (AIStates.cpp:2710-2721), so the unit keeps moving toward its aim point. The unit is set up in range with its approach
/// running and no attack path (`attack_path_active` false, so the stop test applies); the entry would otherwise go to aim. A
/// line-of-sight attacker's own path already needs a clear line (`check_los`), so this is the case the stop test's view covers.
/// Revert check: drop the `view_blocked` term from `approach_moving` in combat.rs; the approach ends (Aim) on the frame.
#[test]
fn a_blocked_view_keeps_an_in_range_approach_moving() {
    let mut g = GameLogic::new(ridge_map(), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let mut sighted = (*gunner()).clone();
    sighted.attack_needs_line_of_sight = true;
    // 53 units from the victim's centre: within the 37.5 range plus the two 8-unit radii.
    let a = g.create_object(&Arc::new(sighted), Coord3D::new(97.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&target(), Coord3D::new(150.0, 100.0, 125.0), 0.0, 1);
    g.frame = 500;
    g.ai_attack_object_from(a, v, false, false, None, crate::ai_states::CommandSource::Ai);
    g.objects.get_mut(&a).expect("attacker").combat.attack.as_mut().expect("the attack").phase = crate::combat::AttackPhase::Approach;
    assert!(g.is_view_blocked_by_obstacle(a, v), "the ridge blocks the line from the attacker to the victim");
    g.run_frames(1);
    assert_eq!(g.find_object(a).unwrap().combat.attack.as_ref().map(|x| x.phase), Some(crate::combat::AttackPhase::Approach), "a blocked line keeps the approach moving");
}
