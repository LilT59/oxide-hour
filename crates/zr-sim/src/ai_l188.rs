//! Lane 188 (AN5 s1 crawler 263, f3129-f3289). An attack's approach entry runs `AIInternalMoveToState::onEnter` after its
//! `computePath` (AIStates.cpp:2665-2678, :1649), so `friend_startingMove` clears `m_movementComplete` (AIUpdate.cpp:2036).
//! The guard's return move exits in the same update (`friend_endingMove`, AIStates.cpp:1761 via `onExit`), and
//! `AIUpdateInterface::update`'s completed-move block (AIUpdate.cpp:1035-1060) then runs `destroyPath`, which clears
//! `m_waitingForPath` and would drop the approach request queued in that update.

use crate::combat::AttackExitConditions;
use crate::math::Coord3D;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use crate::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use crate::GameLogic;
use std::sync::Arc;

fn armed(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
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
    t.combat = CombatTemplate { weapon_set: Some(set), turret_turn_rate: None, ..CombatTemplate::default() };
    Arc::new(t)
}

fn dummy() -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new("Dummy");
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
    t.geometry.major_radius = 8.0;
    t.max_health = 1000.0;
    t.initial_health = 1000.0;
    t.vision_range = 40.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 240.0));
    t.physics = Some(crate::physics::PhysicsTemplate::default());
    Arc::new(t)
}

fn world() -> GameLogic {
    let mut g = GameLogic::new(TerrainLogic::flat(800, 800, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g
}

fn exit_at(center: Coord3D) -> AttackExitConditions {
    AttackExitConditions {
        exit_if_no_unit_found: true,
        exit_if_expired_duration: false,
        exit_if_outside_radius: false,
        center,
        radius_sqr: 0.0,
        give_up_frame: 0,
    }
}

/// The approach entry clears `m_movementComplete` after its request, so the completed-move block of the same update
/// (`ai_end_completed_move`, the port's `AIUpdateInterface::update` block) does not drop the request. Revert check: remove
/// the `if entering { friend_starting_move }` in `combat.rs`'s approach branch; the block clears `waiting_for_path`, the
/// request is never serviced, and the path assertion fails.
#[test]
fn an_approach_entry_survives_a_completed_move_in_the_same_update() {
    let mut g = world();
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let a = g.create_object(&armed("Crawler"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&dummy(), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
    g.frame = 500;
    // The guard's return move exited in this update (`friend_endingMove`, `m_movementComplete` TRUE).
    if let Some(o) = g.objects.get_mut(&a) {
        o.ai.movement_complete = true;
    }
    g.ai_guard_begin_attack(a, v, exit_at(Coord3D::new(100.0, 100.0, 0.0)), false);
    g.ai_end_completed_move(a);
    assert!(g.find_object(a).unwrap().ai.waiting_for_path, "the approach request survives the completed-move block");
    g.process_pathfind_queue();
    assert!(g.find_object(a).unwrap().ai.raw_path.is_some(), "the approach path is installed by this frame's pathfinder pass");
}
