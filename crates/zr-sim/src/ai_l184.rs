//! Lane 184 (wave 21): area guards (`guard_area.rs`) and the attack-exit check at a guard state's entry.
//! Crate-internal tests (the functions under test are private, like the lane 166 and 182 tests).
//!
//! AN5 s1 f3129: a Troop Crawler of the `china n guard combat zone1` team (a `TEAM_GUARD_AREA` team) acquired GLA
//! rebel 239, 775 units from its guard centre. The port guarded the centre only, so the rebel was out of range.

use crate::ai::AiTunables;
use crate::ai_states::AiStateType;
use crate::combat::AttackTarget;
use crate::math::Coord3D;
use crate::orders::GuardMode;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use crate::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use crate::GameLogic;
use std::sync::Arc;

/// An armed vehicle with a vision range of `vision`. Its inner guard circle is `GuardInnerModifierHuman` (1.8) or
/// `GuardInnerModifierAI` (1.1) times that: 90 or 55 for the vision 50 used here, so an enemy 170 units away is outside.
fn armed(name: &str, vision: f32) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = vision;
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

/// A weaponless, very tough vehicle: an enemy that never fights back.
fn dummy() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Dummy");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.vision_range = 40.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 240.0));
    Arc::new(t)
}

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(800, 800, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.set_ai_tunables(AiTunables { guard_enemy_scan_rate: 30, guard_enemy_return_scan_rate: 30, ..AiTunables::default() });
    g
}

/// A square polygon trigger with the given corners (integer map coordinates, as in the map file).
fn square(lo: i32, hi: i32) -> Arc<zr_map::PolygonTrigger> {
    Arc::new(zr_map::PolygonTrigger {
        name: "zone".into(),
        layer: String::new(),
        id: 1,
        is_water: false,
        is_river: false,
        river_start: 0,
        points: vec![[lo, lo, 0], [hi, lo, 0], [hi, hi, 0], [lo, hi, 0]],
    })
}

/// The polygon of the tests: its centre (300,300), and a radius of `sqrt(250^2 + 300^2)` (PolygonTrigger.cpp:292-295).
fn zone() -> Arc<zr_map::PolygonTrigger> {
    square(50, 550)
}

/// `PolygonTrigger::getRadius` keeps the original's sum in `halfHeight` (PolygonTrigger.cpp:292-295). For the
/// square (0,200)-(100,300) that is `sqrt(250^2 + 50^2)`, not the half diagonal `sqrt(50^2 + 50^2)`.
/// Revert check: the bounding-box half diagonal (`(hi - lo) / 2` on both axes) gives 70.71 and fails this test.
#[test]
fn polygon_radius_sums_the_bounds_in_the_y_axis_as_the_original_does() {
    let t = zr_map::PolygonTrigger {
        name: "a".into(),
        layer: String::new(),
        id: 1,
        is_water: false,
        is_river: false,
        river_start: 0,
        points: vec![[0, 200, 0], [100, 200, 0], [100, 300, 0], [0, 300, 0]],
    };
    let r = crate::guard_area::polygon_radius(&t);
    assert!((r - 254.951).abs() < 0.001, "radius {r}");
}

/// `AIGuardMachine::lookForInnerTarget` with an area (AIGuard.cpp:273-290): the scan is centred on the polygon and
/// its radius is the scan radius, so a rebel 170 units from the centre is found. A point guard's inner circle
/// (`1.1 * 100`) would not see it. The clock is kept fresh, so only the area's reach decides.
/// Revert check: scan with `ai_guard_range` instead of the polygon radius and the rebel is never acquired.
#[test]
fn an_area_guard_acquires_an_enemy_inside_its_polygon_beyond_the_inner_circle() {
    let mut g = world();
    let guard = g.create_object(&armed("Gun", 50.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&dummy(), Coord3D::new(470.0, 300.0, 0.0), 0.0, 1);
    g.command_guard_area(0, &[guard], zone(), GuardMode::Normal);
    let mut acquired = false;
    for _ in 0..200 {
        g.note_trigger_area_change();
        g.update();
        if g.find_object(guard).unwrap().combat.attack.as_ref().map(|a| a.target) == Some(AttackTarget::Object(enemy)) {
            acquired = true;
            break;
        }
    }
    assert!(acquired, "the area guard attacks the enemy inside its polygon");
}

/// `AIGuardMachine::lookForInnerTarget` (AIGuard.cpp:275-280): with no object having changed trigger areas within
/// `m_guardEnemyScanRate` frames, an area guard's scan does not run. The clock is stamped at creation (frame 0), and
/// the frame is then moved on without any change, so the gate is shut for the whole run.
/// Revert check: drop the `guard_area_scan_open` gate and the rebel is acquired inside the first scan.
#[test]
fn an_area_guard_does_not_scan_while_no_object_has_changed_trigger_areas() {
    let mut g = world();
    let guard = g.create_object(&armed("Gun", 50.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&dummy(), Coord3D::new(470.0, 300.0, 0.0), 0.0, 1);
    g.trigger_membership_pass();
    g.frame = 100;
    g.command_guard_area(0, &[guard], zone(), GuardMode::Normal);
    let _ = enemy;
    for _ in 0..90 {
        g.update();
    }
    assert!(g.find_object(guard).unwrap().combat.attack.is_none(), "no area scan: nothing is acquired");
}

/// `AIAttackState::onEnter` (AIStates.cpp:5526-5527): an exit condition already met at entry is SUCCESS, with no
/// attack. The rebel is 170 units from the centre, outside the inner circle (`1.1 * 100`), so the inner state
/// reports SUCCESS at once and the guard enters the outer circle on the same frame (AIGuard.cpp:426, :441, :183).
/// Revert check: attack in the inner state without the `should_exit` test and the guard is in `GuardInner` first.
#[test]
fn an_inner_entry_outside_the_inner_circle_goes_straight_to_the_outer_circle() {
    let mut g = world();
    let guard = g.create_object(&armed("Gun", 50.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let _enemy = g.create_object(&dummy(), Coord3D::new(470.0, 300.0, 0.0), 0.0, 1);
    g.command_guard_area(0, &[guard], zone(), GuardMode::Normal);
    let mut first_after_return = None;
    for _ in 0..200 {
        g.note_trigger_area_change();
        g.update();
        let sub = g.find_object(guard).unwrap().ai.machine.guard_sub;
        if sub != AiStateType::GuardReturn {
            first_after_return = Some(sub);
            break;
        }
    }
    assert_eq!(first_after_return, Some(AiStateType::GuardOuter), "the first state after the return is the outer circle");
}

/// The guard's machine state is per object: a point guard keeps no polygon (AIGuard.cpp, privateGuardPosition).
#[test]
fn a_point_guard_has_no_area() {
    let mut g = world();
    let guard = g.create_object(&armed("Gun", 50.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.command_guard_area(0, &[guard], zone(), GuardMode::Normal);
    assert!(g.find_object(guard).unwrap().ai.machine.guard.area.is_some());
    g.submit(0, crate::Command::Guard { ids: vec![guard], target: crate::orders::GuardTarget::Point(Coord3D::new(10.0, 10.0, 0.0)), mode: GuardMode::Normal });
    g.update();
    assert!(g.find_object(guard).unwrap().ai.machine.guard.area.is_none());
}
