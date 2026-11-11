//! Lane 190 (wave 21, the R26 review). Crate-internal tests; each names its finding and the revert check it was run with.
//! - R26-1: the pursue entry runs `AIInternalMoveToState::onEnter` after its `computePath` (AIStates.cpp:1627-1667), so
//!   `friend_startingMove` sets `m_isMoving` on entry.
//! - R26-2: a blocked-and-stuck pursuer fails the pursue and approaches (AIStates.cpp:2858-2866, :2935-2940).
//! - R26-3: `canPursue` asks for the turret of the current weapon (AIStates.cpp:2366).
//! - R26-4: a computer player does not chase an airborne aircraft unless hunting (AIStates.cpp:2680-2688, :2615-2621).
//! - R26-5: the attack fallback's `ignoreObstacle(victim)` persists (AIUpdate.cpp:455, :4305-4308).
//! - Lane 189's open defect: a same-ID temporary `AI_MOVE_TO` (the transport's HeadOffMap exit) shares the main move's
//!   state object, so its goal survives its end (StateMachine.cpp:512-532; DeliverPayloadAIUpdate.cpp:1131).

use crate::ai::PathRequest;
use crate::combat::{AttackExitConditions, AttackPhase};
use crate::math::Coord3D;
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use crate::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use crate::GameLogic;
use std::sync::Arc;

/// An armed vehicle (a 40-unit cannon). `turret` gives it a turret template; `turret_slots` is the weapon-slot mask the
/// turret controls (0 = every slot), `CombatTemplate::turret_controlled_slots`.
fn armed(name: &str, turret: Option<f32>, turret_slots: u8) -> Arc<ObjectTemplate> {
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
    t.combat = CombatTemplate { weapon_set: Some(set), turret_turn_rate: turret, turret_controlled_slots: turret_slots, ..CombatTemplate::default() };
    Arc::new(t)
}

/// A weaponless, very tough vehicle with a slow locomotor (speed 30): an enemy that never fights back.
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

/// The guard attack's exit conditions as the guard states set them for a victim: the no-unit-found exit only.
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

/// A turreted attacker at (100,100) and a victim at (200,100) moving away at speed 10, which `canPursue` accepts for a
/// speed-60 attacker (AIStates.cpp:2385-2401). Its guard attack's entry is run.
/// The attacker's player is a computer player: a human player's auto-acquired attack from the AI is refused at entry
/// (AIStates.cpp:2609-2615), which these tests are not about.
fn pursuit_begun(attacker_turret_slots: u8) -> (GameLogic, crate::object::ObjectId) {
    let mut g = world();
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    let a = g.create_object(&armed("Crawler", Some(360.0), attacker_turret_slots), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&dummy(), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    let p = g.objects.get_mut(&v).and_then(|o| o.physics.as_mut()).expect("the victim has a physics module");
    p.velocity = Coord3D::new(1.0, 0.0, 0.0);
    g.frame = 500;
    g.ai_guard_begin_attack(a, v, exit_at(Coord3D::new(100.0, 100.0, 0.0)), false);
    (g, a)
}

/// R26-1 (AIStates.cpp:1627-1667, :1635): the pursue's entry runs the base `onEnter`, whose `friend_startingMove` sets
/// `m_isMoving`, so an entering pursuer reads as moving at once (the ally push skips it, `isMoving`). Revert check: remove
/// the `friend_starting_move` call from `pursue_enter_move`; `is_moving` stays false.
#[test]
fn a_pursue_entry_starts_its_move_as_friend_starting_move_does() {
    let (g, a) = pursuit_begun(0);
    let o = g.find_object(a).unwrap();
    assert_eq!(o.combat.attack.as_ref().map(|x| x.phase), Some(AttackPhase::Pursue), "the entry pursues");
    assert!(o.ai.is_moving, "the entering pursuer is moving (friend_startingMove)");
}

/// R26-2 (AIStates.cpp:2858-2866): a blocked-and-stuck pursuer's `computePath` returns false, so the pursue succeeds
/// (onEnter, :2935-2940) or fails (updateInternal, :3046-3052) and the attack approaches. The pursuit is begun first, then
/// the unit is stuck and its attack updates. Revert check: remove the `blocked_and_stuck` test from the pursue branch; the
/// phase stays `Pursue`.
#[test]
fn a_blocked_and_stuck_pursuer_approaches_instead_of_pursuing() {
    let (mut g, a) = pursuit_begun(0);
    assert_eq!(g.find_object(a).unwrap().combat.attack.as_ref().map(|x| x.phase), Some(AttackPhase::Pursue), "the pursue began");
    g.objects.get_mut(&a).expect("the attacker").ai.is_blocked_and_stuck = true;
    g.ai_attack_update_object(a);
    let o = g.find_object(a).unwrap();
    assert_eq!(o.combat.attack.as_ref().map(|x| x.phase), Some(AttackPhase::Approach), "the stuck pursuer approaches");
}

/// R26-3 (AIStates.cpp:2366, `getWhichTurretForCurWeapon`): the unit has a turret, but the turret controls only slot 1 and
/// the current weapon is slot 0, which is on the body. `canPursue` refuses (no turret for the current weapon), and the
/// attack approaches. Revert check: pass the template's turret test back into `can_pursue`; the phase is `Pursue`.
#[test]
fn a_pursuer_whose_current_weapon_is_on_the_body_does_not_pursue() {
    let (g, a) = pursuit_begun(0b10);
    let o = g.find_object(a).unwrap();
    assert_eq!(o.combat.attack.as_ref().map(|x| x.phase), Some(AttackPhase::Approach), "no turret for the current weapon");
}

/// R26-4 (AIStates.cpp:2680-2688 and :2615-2621): a computer player does not chase an airborne aircraft unless it is
/// hunting, so an out-of-range attack on a flying jet ends at once. Revert check: drop the `approach_refused` call; the
/// attack stays alive and approaches.
#[test]
fn a_computer_player_does_not_chase_an_airborne_aircraft() {
    let mut g = world();
    g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
    // An anti-air cannon: the weapon choice's anti mask (WeaponSet.cpp:408) must accept the airborne jet, so the attack
    // reaches its approach decision rather than ending for lack of a weapon.
    let mut crawler = (*armed("Crawler", None, 0)).clone();
    if let Some(w) = crawler.combat.weapon_set.as_mut().and_then(|s| s.weapons[0].as_mut()) {
        let mut anti = (**w).clone();
        anti.anti_mask = crate::weapon::anti::GROUND | crate::weapon::anti::AIRBORNE_VEHICLE;
        *w = Arc::new(anti);
    }
    let a = g.create_object(&Arc::new(crawler), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let mut jet = (*dummy()).clone();
    jet.name = "Jet".into();
    jet.kind_of |= kind_of::AIRCRAFT;
    let jet = g.create_object(&Arc::new(jet), Coord3D::new(400.0, 100.0, 0.0), 0.0, 1);
    g.objects.get_mut(&jet).expect("the jet").status_bits |= crate::object::status::AIRBORNE_TARGET;
    g.frame = 500;
    g.ai_start_attack(a, crate::combat::AttackTarget::Object(jet), false, false, None, crate::ai_states::AiStateType::Idle);
    g.ai_attack_update_object(a);
    assert!(g.find_object(a).unwrap().combat.attack.is_none(), "the computer player does not chase the airborne jet");
}

/// R26-5 (AIUpdate.cpp:455, `ignoreObstacle(victim)`; :4305-4308): the attack fallback's ignored obstacle is the victim and
/// it persists after the search (it is cleared only when the move completes, AIUpdate.cpp:1061). Setup: the search has no
/// in-range cell (range 0), so the fallback runs. Revert check: drop the `o.ai.ignore_obstacle_id = victim` write; the
/// field stays INVALID.
#[test]
fn a_failed_attack_path_keeps_the_victim_as_the_ignored_obstacle() {
    let mut g = world();
    // A weaponless attacker: `computeAttackPath` reads range 0 at compute time, so the search fails (AIUpdate.cpp:1800-1830).
    let id = g.create_object(&dummy(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let vpos = Coord3D::new(300.0, 300.0, 0.0);
    let victim = g.create_object(&dummy(), vpos, 0.0, 1);
    {
        let o = g.objects.get_mut(&id).unwrap();
        o.ai.request = PathRequest::Attack;
        o.ai.requested_victim = victim;
        o.ai.requested_destination = vpos;
        o.ai.is_final_goal = true;
        o.ai.adjusts_destination = false;
        o.ai.waiting_for_path = true;
    }
    g.do_pathfind(id);
    assert_eq!(g.find_object(id).unwrap().ai.ignore_obstacle_id, victim, "the victim is the ignored obstacle after the fallback");
}

/// R26-12 (AIStates.cpp:1621-1625): the step-aside move's enter is the internal move's enter, which fails for a unit with the
/// IMMOBILE status (the status, not the kind). The unit has a path, so only the status test can fail the enter. Revert check:
/// drop the status test in `ai_move_out_of_way_enter`; the enter continues.
#[test]
fn a_step_aside_enter_fails_for_an_immobile_unit() {
    let mut g = world();
    let u = g.create_object(&armed("Walker", None, 0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.ai_move_to_position_cmd(u, Coord3D::new(7000.0, 100.0, 0.0), crate::ai_states::CommandSource::Player);
    g.run_frames(2);
    assert!(g.find_object(u).unwrap().ai.raw_path.is_some(), "the unit has a path to step along");
    g.objects.get_mut(&u).expect("the unit").status_bits |= crate::object::status::IMMOBILE;
    assert_eq!(g.ai_move_out_of_way_enter_pub(u), crate::ai_states::StateReturn::Failure, "an immobile unit does not step aside");
}

/// Lane 189's open defect (StateMachine.cpp:512-532, AIStates.cpp:919, DeliverPayloadAIUpdate.cpp:1131): a temporary `AI_MOVE_TO`
/// of the main move's own state is the same state object, so the temporary's goal is the move's goal after the temporary
/// ends (the main resumes without an enter). The HeadOffMap exit depends on it. Setup: a slow unit on a long move, then a
/// `CMD_FROM_AI` move to a second point; once the 20-second temporary has ended, the goal is the second point. Revert check:
/// park the main's set for a same-ID temporary (the `shared` branch removed); the main's own goal comes back.
#[test]
fn a_same_state_temporary_move_keeps_its_goal_after_it_ends() {
    let mut g = world();
    let u = g.create_object(&armed("Walker", None, 0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // Far points: a 60-speed unit covers about 1200 units in the 20-second temporary, so it has not arrived when it ends.
    g.ai_move_to_position_cmd(u, Coord3D::new(7000.0, 100.0, 0.0), crate::ai_states::CommandSource::Player);
    for _ in 0..10 {
        g.run_frames(1);
    }
    let second = Coord3D::new(100.0, 7000.0, 0.0);
    g.ai_move_to_position_cmd(u, second, crate::ai_states::CommandSource::Ai);
    assert!(g.find_object(u).unwrap().ai.machine.temporary.is_some(), "the AI move is a temporary state");
    for _ in 0..(20 * crate::math::LOGICFRAMES_PER_SECOND + 5) {
        g.run_frames(1);
        if g.find_object(u).unwrap().ai.machine.temporary.is_none() {
            break;
        }
    }
    let o = g.find_object(u).unwrap();
    assert!(o.ai.machine.temporary.is_none(), "the temporary move has ended");
    assert_eq!(o.ai.machine.current, crate::ai_states::AiStateType::MoveTo, "the main move resumes");
    // The goal is the temporary's point after the destination adjust (a cell centre, AIStates.cpp:1661).
    assert!(
        o.ai.move_goal.is_some_and(|g| g.dist_2d(&second) < 2.0),
        "the goal is the temporary's point, as the state object keeps it: {:?}",
        o.ai.move_goal
    );
}
