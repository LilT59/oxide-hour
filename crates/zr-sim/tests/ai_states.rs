//! `AIStateMachine` (`zr-sim::ai_states`) behaviour: commands set the machine's state, the states
//! run, and they transition back to `AI_IDLE` on completion.

use std::sync::Arc;
use zr_sim::ai_states::AiStateType;
use zr_sim::combat::{AttackPhase, AttackTarget};
use zr_sim::math::Coord3D;
use zr_sim::orders::{GuardMode, GuardTarget};
use zr_sim::template::{kind_of, LocomotorKind, LocomotorParams};
use zr_sim::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use zr_sim::{Command, DamageInfo, DamageType, GameLogic, ObjectTemplate, TerrainLogic};

fn unit(name: &str) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.geometry.height = 10.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = 300.0;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
    let mut w = WeaponTemplate::new("Gun");
    w.primary_damage = 20.0;
    w.primary_damage_radius = 5.0;
    w.secondary_damage_radius = 5.0;
    w.attack_range = 100.0;
    w.min_delay_between_shots = 10;
    w.max_delay_between_shots = 10;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    Arc::new(t)
}

#[test]
fn move_order_sets_move_state_and_returns_to_idle() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = g.create_object(&unit("Tank"), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(20.0, 10.0, 0.0) });
    g.update();
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::MoveTo);
    g.run_frames(240);
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::Idle, "the move completed");
    // `AIInternalMoveToState::update`'s arrival does not clear `m_goalPosition` (`AIStates.cpp:1929-1939`),
    // so the port keeps its `move_goal`; the unit ends on the goal cell `onEnter`'s `adjustDestination`
    // chose (`AIStates.cpp:1661-1667`), which this map's adjust moves off the click. Compare with the
    // destination the move was given, not the raw click (lane 127, wave 16).
    let o = g.find_object(id).unwrap();
    let goal = o.ai.move_goal.expect("the finished move keeps its goal");
    assert!(o.position.dist_2d(&goal) < 2.0, "the move completed at its goal ({:.2},{:.2})", o.position.x, o.position.y);
}

#[test]
fn attack_order_enters_attack_state() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let a = g.create_object(&unit("A"), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    let b = g.create_object(&unit("B"), Coord3D::new(30.0, 10.0, 0.0), 0.0, 1);
    g.submit(0, Command::Attack { ids: vec![a], target: b });
    g.update();
    let o = g.find_object(a).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::AttackObject);
    assert!(matches!(o.combat.attack.as_ref().map(|x| x.target), Some(AttackTarget::Object(id)) if id == b));
}

#[test]
fn stop_returns_to_idle() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = g.create_object(&unit("Tank"), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(90.0, 10.0, 0.0) });
    g.run_frames(5);
    g.submit(0, Command::Stop { ids: vec![id] });
    g.update();
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::Idle);
}

#[test]
fn hunt_enters_hunt_state() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = g.create_object(&unit("Tank"), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.command_hunt(0, &[id]);
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::Hunt);
}

/// `AIHuntState::update` (`AIStates.cpp:7262`): with no victim anywhere and no `PLAYER_HUNT`, the
/// state succeeds and the unit goes idle instead of hunting forever.
#[test]
fn hunt_with_no_victim_exits_to_idle() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    g.add_player(0, 0);
    let id = g.create_object(&unit("Tank"), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.command_hunt(0, &[id]);
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::Hunt);
    g.run_frames(90);
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::Idle, "no victim: hunt succeeded");
}

/// `AI::findClosestEnemy` has no mood gate: a hunting unit whose template lacks
/// `AutoAcquireEnemiesWhenIdle` still finds and attacks the closest enemy anywhere.
#[test]
fn hunt_ignores_the_idle_mood_flag() {
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.add_ai_player(0, zr_sim::SkirmishAiConfig::default()); // AI players ignore fog when hunting
    let mut t = ObjectTemplate::clone(&unit("Hunter"));
    t.combat.auto_acquire_enemies = false;
    let a = g.create_object(&Arc::new(t), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    let b = g.create_object(&unit("Victim"), Coord3D::new(300.0, 300.0, 0.0), 0.0, 1);
    g.command_hunt(0, &[a]);
    // The first scan is at most `ENEMY_SCAN_RATE` frames after entering the state.
    g.run_frames(40);
    let o = g.find_object(a).unwrap();
    assert!(
        matches!(o.combat.attack.as_ref().map(|x| x.target), Some(AttackTarget::Object(id)) if id == b),
        "the hunter acquired the distant enemy (state {:?})",
        o.ai.machine.current
    );
}

#[test]
fn guard_enters_guard_state() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let id = g.create_object(&unit("Tank"), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.submit(0, Command::Guard { ids: vec![id], target: GuardTarget::Point(Coord3D::new(40.0, 40.0, 0.0)), mode: GuardMode::Normal });
    g.update();
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::Guard);
    g.run_frames(240);
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::Guard, "guard holds");
}

/// A turreted, physics-driven tank: the shape `AIAttackPursueTargetState` needs (a turret and a
/// catchable victim with physics).
fn turret_tank(name: &str, speed: f32) -> Arc<ObjectTemplate> {
    let mut t = (*unit(name)).clone();
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, speed, 120.0, 120.0, 360.0));
    t.physics = Some(zr_sim::physics::PhysicsTemplate::default());
    t.combat.turret_turn_rate = Some(0.2);
    Arc::new(t)
}

#[test]
fn attack_pursues_a_fleeing_target() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let a = g.create_object(&turret_tank("A", 60.0), Coord3D::new(10.0, 100.0, 0.0), 0.0, 0);
    let v = g.create_object(&turret_tank("V", 30.0), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
    // The victim flees to the right, away from the attacker and out of its 100-unit range.
    g.submit(1, Command::MoveTo { ids: vec![v], pos: Coord3D::new(320.0, 100.0, 0.0) });
    g.submit(0, Command::Attack { ids: vec![a], target: v });
    g.run_frames(6);
    let o = g.find_object(a).unwrap();
    let order = o.combat.attack.expect("still attacking");
    assert_eq!(order.phase, AttackPhase::Pursue, "a turreted attacker chases a fleeing target");
}

#[test]
fn guard_retaliates_against_its_last_attacker() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    // A short-sighted guard (inner circle 1.8 * 40 = 72) that cannot see the enemy 80 from its post,
    // so only the aggressor state can make it fire.
    let mut gt = ObjectTemplate::clone(&unit("Guard"));
    gt.vision_range = 40.0;
    let guard = g.create_object(&Arc::new(gt), Coord3D::new(80.0, 100.0, 0.0), 0.0, 0);
    let enemy = g.create_object(&unit("Enemy"), Coord3D::new(180.0, 100.0, 0.0), 0.0, 1);
    g.submit(0, Command::Guard { ids: vec![guard], target: GuardTarget::Point(Coord3D::new(100.0, 100.0, 0.0)), mode: GuardMode::Normal });
    g.run_frames(60); // move to the guard point and reach AI_GUARD_IDLE
    // The enemy shoots the guard; the body records the clearable last attacker.
    g.damage_object(guard, DamageInfo { source: enemy, amount: 5.0, kind: DamageType::SmallArms });
    assert_eq!(g.find_object(guard).unwrap().clearable_last_attacker(), enemy);
    g.run_frames(40);
    let o = g.find_object(guard).unwrap();
    assert!(
        matches!(o.combat.attack.as_ref().map(|a| a.target), Some(AttackTarget::Object(id)) if id == enemy),
        "the guard retaliated against the aggressor"
    );
}

#[test]
fn idle_units_sleep_between_scans() {
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let mut t = (*unit("Idle")).clone();
    t.combat.auto_acquire_enemies = false;
    let id = g.create_object(&Arc::new(t), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    g.run_frames(2);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::Idle);
    assert!(o.ai.machine.sleep_till > g.frame(), "an idle unit sleeps until its next scan");
}

#[test]
fn follow_path_walks_the_waypoints() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let id = g.create_object(&unit("Tank"), Coord3D::new(20.0, 20.0, 0.0), 0.0, 0);
    g.ai_follow_path(id, vec![Coord3D::new(60.0, 20.0, 0.0), Coord3D::new(60.0, 60.0, 0.0)], false);
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::FollowPath);
    g.run_frames(600);
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::Idle, "the path completed");
    let p = g.find_object(id).unwrap().position;
    assert!(p.dist_2d(&Coord3D::new(60.0, 60.0, 0.0)) < 3.0, "reached the last waypoint");
}

#[test]
fn dock_state_machine_moves_to_the_dock_and_exits() {
    let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
    let docker = g.create_object(&unit("Truck"), Coord3D::new(50.0, 100.0, 0.0), 0.0, 0);
    let mut dock_t = ObjectTemplate::new("Dock");
    dock_t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    dock_t.geometry.major_radius = 12.0;
    dock_t.max_health = 500.0;
    dock_t.initial_health = 500.0;
    dock_t.dock = Some(zr_sim::supply::DockTemplate {
        kind: zr_sim::supply::DockKind::Warehouse,
        starting_boxes: 5,
        delete_when_empty: false,
        number_approach_positions: 5,
        allows_passthrough: true,
    });
    let dock = g.create_object(&Arc::new(dock_t), Coord3D::new(120.0, 100.0, 0.0), 0.0, 0);
    g.ai_dock(docker, dock);
    assert_eq!(g.find_object(docker).unwrap().ai.machine.current, AiStateType::Dock);
    g.run_frames(500);
    assert_eq!(g.find_object(docker).unwrap().ai.machine.current, AiStateType::Idle, "the dock sequence finished");
}

fn priority_game() -> (GameLogic, zr_sim::ObjectId) {
    let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g.add_ai_player(0, zr_sim::SkirmishAiConfig::default()); // AI players ignore fog when hunting
    let a = g.create_object(&unit("Hunter"), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    (g, a)
}

fn hunt_target(g: &mut GameLogic, a: zr_sim::ObjectId) -> Option<zr_sim::ObjectId> {
    g.command_hunt(0, &[a]);
    g.run_frames(40);
    match g.find_object(a).unwrap().combat.attack.as_ref().map(|x| x.target) {
        Some(AttackTarget::Object(v)) => Some(v),
        _ => None,
    }
}

/// `AI::findClosestEnemy` with an `AttackPriorityInfo`: a priority-0 template is never a target
/// (`TEAM_APPLY_ATTACK_PRIORITY_SET` -> `setAttackInfo`), even when it is the closest enemy.
#[test]
fn attack_priority_zero_targets_are_skipped() {
    let (mut g, a) = priority_game();
    let weak = g.create_object(&unit("Weak"), Coord3D::new(60.0, 10.0, 0.0), 0.0, 1);
    let other = g.create_object(&unit("Other"), Coord3D::new(200.0, 10.0, 0.0), 0.0, 1);
    g.scripts_mut().set_attack_priority("Set", "Weak", 0);
    g.set_unit_attack_info(a, Some("Set".to_owned()));
    let victim = hunt_target(&mut g, a);
    assert_eq!(victim, Some(other), "the priority-0 unit {weak:?} is ignored");
    // Without the set the closest enemy wins.
    let (mut g, a) = priority_game();
    let weak = g.create_object(&unit("Weak"), Coord3D::new(60.0, 10.0, 0.0), 0.0, 1);
    g.create_object(&unit("Other"), Coord3D::new(200.0, 10.0, 0.0), 0.0, 1);
    assert_eq!(hunt_target(&mut g, a), Some(weak));
}

/// Each priority point is worth `AttackPriorityDistanceModifier` (100) of distance, so a priority-3
/// target 150 away beats a default-priority target 50 away.
#[test]
fn attack_priority_prefers_a_higher_priority_target_within_the_distance_modifier() {
    let (mut g, a) = priority_game();
    g.create_object(&unit("Weak"), Coord3D::new(60.0, 10.0, 0.0), 0.0, 1);
    let strong = g.create_object(&unit("Strong"), Coord3D::new(160.0, 10.0, 0.0), 0.0, 1);
    g.scripts_mut().set_attack_priority("Set", "Strong", 3);
    g.set_unit_attack_info(a, Some("Set".to_owned()));
    assert_eq!(hunt_target(&mut g, a), Some(strong));
}

/// `AIHuntState::update` keeps scanning while its attack sub-state runs and `AIAttackState::update`
/// adopts the new goal object ("Our parent machine has changed the goal", `AIStates.cpp:5675`): a
/// hunter chasing a far target switches to a better-ranked enemy that shows up.
#[test]
fn hunter_retargets_while_attacking() {
    let (mut g, a) = priority_game();
    let far = g.create_object(&unit("Far"), Coord3D::new(280.0, 10.0, 0.0), 0.0, 1);
    g.scripts_mut().set_attack_priority("Set", "Strong", 5);
    g.set_unit_attack_info(a, Some("Set".to_owned()));
    assert_eq!(hunt_target(&mut g, a), Some(far));
    let strong = g.create_object(&unit("Strong"), Coord3D::new(80.0, 40.0, 0.0), 0.0, 1);
    g.run_frames(40);
    let o = g.find_object(a).unwrap();
    assert!(
        matches!(o.combat.attack.as_ref().map(|x| x.target), Some(AttackTarget::Object(v)) if v == strong),
        "the hunter retargeted (state {:?}, attack {:?})",
        o.ai.machine.current,
        o.combat.attack.as_ref().map(|x| x.target)
    );
}

/// `getAdjustedVisionRangeForObject`: an idle computer-controlled unit scans `vision *
/// GuardOuterModifierAI` (1.333 -> 400 for vision 300) and an Aggressive one a further 1.5x (600).
#[test]
fn aggressive_units_scan_farther_when_idle() {
    let run = |attitude: i32| {
        let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.set_enemies(0, 1);
        g.add_ai_player(0, zr_sim::SkirmishAiConfig::default());
        let mut t = ObjectTemplate::clone(&unit("Idle"));
        t.combat.auto_acquire_enemies = true; // `AutoAcquireEnemiesWhenIdle`
        let a = g.create_object(&Arc::new(t), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
        g.create_object(&unit("Far"), Coord3D::new(510.0, 10.0, 0.0), 0.0, 1);
        g.set_unit_attitude(a, attitude);
        g.run_frames(90);
        g.find_object(a).unwrap().combat.attack.is_some()
    };
    assert!(!run(0), "normal: the enemy 500 away is outside 1.333 * 300");
    assert!(run(2), "aggressive: inside 1.5 * 1.333 * 300");
}

/// Angry-mob members (`INFANTRY` + `IGNORED_IN_GUI`) are exempt from the mood matrix
/// (`AIUpdate.cpp:4376`): an Aggressive AI unit's move is not turned into an attack-move.
#[test]
fn angry_mob_members_are_exempt_from_the_mood_matrix() {
    let state_after_move = |mob: bool| {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        g.add_player(0, 0);
        g.add_ai_player(0, zr_sim::SkirmishAiConfig::default());
        let mut t = ObjectTemplate::clone(&unit("Mob"));
        t.kind_of |= kind_of::INFANTRY;
        t.ignored_in_gui = mob;
        let id = g.create_object(&Arc::new(t), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
        g.set_unit_attitude(id, 2);
        g.submit(0, Command::MoveTo { ids: vec![id], pos: Coord3D::new(90.0, 10.0, 0.0) });
        g.run_frames(5);
        g.find_object(id).unwrap().ai.machine.current
    };
    assert_eq!(state_after_move(true), AiStateType::MoveTo);
    assert_eq!(state_after_move(false), AiStateType::AttackMoveTo);
}

/// `PartitionFilterRejectBuildings` (`PartitionManager.cpp:5020`) as `AI::findClosestEnemy` applies it
/// to a hunt (no `ATTACK_BUILDINGS`): a computer-controlled hunter acquires any enemy structure, a
/// human one only `FS_BASE_DEFENSE` structures (or containers that can attack).
#[test]
fn hunt_rejects_enemy_structures_unless_computer_or_base_defense() {
    let structure = |name: &str, extra: u64| {
        let mut t = ObjectTemplate::clone(&unit(name));
        t.kind_of = kind_of::STRUCTURE | kind_of::SELECTABLE | extra;
        t.locomotor = None;
        Arc::new(t)
    };
    let hunt_with = |computer: bool, extra: u64| {
        let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.set_enemies(0, 1);
        if computer {
            g.add_ai_player(0, zr_sim::SkirmishAiConfig::default());
        }
        let a = g.create_object(&unit("Hunter"), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
        let b = g.create_object(&structure("Bld", extra), Coord3D::new(60.0, 10.0, 0.0), 0.0, 1);
        (hunt_target(&mut g, a), b)
    };
    let (t, b) = hunt_with(true, 0);
    assert_eq!(t, Some(b), "computer-controlled hunters acquire structures");
    let (t, _) = hunt_with(false, 0);
    assert_eq!(t, None, "a human hunter rejects a plain structure");
    let (t, b) = hunt_with(false, kind_of::FS_BASE_DEFENSE);
    assert_eq!(t, Some(b), "a human hunter still acquires base defenses");
}

/// `AIGuardMachine::getStdGuardRange` (`AIGuard.cpp:203`): `vision * GuardInnerModifier*` (AI 1.1 ->
/// 330 for vision 300, human 1.8 -> 540). An enemy 390 away is inside the human guard circle only.
#[test]
fn guard_scan_uses_the_inner_modifier_for_the_owner_type() {
    let acquired = |computer: bool| {
        let mut g = GameLogic::new(TerrainLogic::flat(600, 600, 0), 1);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.set_enemies(0, 1);
        if computer {
            g.add_ai_player(0, zr_sim::SkirmishAiConfig::default());
        }
        let a = g.create_object(&unit("Guard"), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
        g.create_object(&unit("Enemy"), Coord3D::new(400.0, 10.0, 0.0), 0.0, 1);
        g.submit(0, Command::Guard { ids: vec![a], target: GuardTarget::Point(Coord3D::new(10.0, 10.0, 0.0)), mode: GuardMode::Normal });
        g.run_frames(90);
        g.find_object(a).unwrap().combat.attack.is_some()
    };
    assert!(acquired(false), "human: inside 1.8 * 300");
    assert!(!acquired(true), "computer: outside 1.1 * 300");
}

/// The guard machine's scan has no `PartitionFilterRejectBuildings` (`AIGuard.cpp:212`): a human
/// guard attacks an enemy structure in range.
#[test]
fn guard_scan_accepts_enemy_structures() {
    let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    let a = g.create_object(&unit("Guard"), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
    let mut t = ObjectTemplate::clone(&unit("Bld"));
    t.kind_of = kind_of::STRUCTURE | kind_of::SELECTABLE;
    t.locomotor = None;
    g.create_object(&Arc::new(t), Coord3D::new(60.0, 10.0, 0.0), 0.0, 1);
    g.submit(0, Command::Guard { ids: vec![a], target: GuardTarget::Point(Coord3D::new(10.0, 10.0, 0.0)), mode: GuardMode::Normal });
    let mut attacked = false;
    for _ in 0..90 {
        g.run_frames(1);
        attacked |= g.find_object(a).unwrap().combat.attack.is_some();
    }
    assert!(attacked, "the guard attacked the structure");
}

#[test]
fn state_machine_is_deterministic() {
    let run = || {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.set_enemies(0, 1);
        let a = g.create_object(&unit("A"), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
        let b = g.create_object(&unit("B"), Coord3D::new(30.0, 10.0, 0.0), 0.0, 1);
        g.submit(0, Command::AttackMove { ids: vec![a], pos: Coord3D::new(90.0, 50.0, 0.0) });
        g.submit(1, Command::Guard { ids: vec![b], target: GuardTarget::Point(Coord3D::new(30.0, 30.0, 0.0)), mode: GuardMode::Normal });
        let mut crcs = Vec::new();
        for _ in 0..300 {
            g.update();
            crcs.push(g.crc());
        }
        crcs
    };
    assert_eq!(run(), run());
}

/// `AIIdleState::doInitIdleState`'s start-up "cheat move" (`AIStates.cpp:1358-1382`): an idle ground unit
/// moves to its goal cell's world position, which for a footprint that is not centred in a cell is
/// `cell * 10 + 0.5` (`Pathfinder::adjustCoordToCell`, `AIPathfind.cpp:9127`). The civilian cars of AN3
/// leave their `.map` position this way on their first update.
#[test]
fn an_idle_unit_cheat_moves_to_its_goal_cell_on_the_first_update() {
    let mut t = (*unit("Car")).clone();
    t.geometry.shape = zr_sim::template::GeometryShape::Box;
    t.geometry.major_radius = 11.0;
    t.geometry.minor_radius = 4.0;
    t.geometry.height = 7.0;
    t.combat = CombatTemplate::default();
    t.ai_update = Some(1);
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    // The map position of the C++ `CarBug3` id 144.
    let start = Coord3D::new(1234.39, 969.02, 0.0);
    let id = g.create_object(&t, start, 0.0, 0);
    assert_eq!(g.find_object(id).unwrap().position, start, "placed at the map position");
    // The AI update module's first update is one frame after creation (`setWakeFrame(now + 1)`,
    // `AIUpdate.cpp:299-301`), so frame 1 still has the map position.
    g.update();
    assert!((g.find_object(id).unwrap().position.x - start.x).abs() < 0.01, "not yet moved");
    g.update();
    let p = g.find_object(id).unwrap().position;
    // `cell = floor(0.5 + pos/10)` for a footprint that is not centred; `(cell + 0.05) * 10`.
    assert_eq!((p.x, p.y), (1230.5, 970.5), "the goal cell's position");
}

/// `Pathfinder::updateGoal` returns for a `KINDOF_IMMOBILE` object before it touches the goal cell
/// (`AIPathfind.cpp:9931-9934`), so `AIIdleState::doInitIdleState`'s cheat move (`AIStates.cpp:1370-1374`)
/// finds no goal position and does not move it. An immobile template that still carries a ground
/// locomotor used to jump to its cell centre on the first update.
#[test]
fn an_immobile_idle_unit_does_not_cheat_move_on_the_first_update() {
    let mut t = (*unit("Car")).clone();
    t.kind_of |= kind_of::IMMOBILE;
    t.geometry.shape = zr_sim::template::GeometryShape::Box;
    t.geometry.major_radius = 11.0;
    t.geometry.minor_radius = 4.0;
    t.geometry.height = 7.0;
    t.combat = CombatTemplate::default();
    t.ai_update = Some(1);
    let t = Arc::new(t);
    let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
    let start = Coord3D::new(1234.39, 969.02, 0.0);
    let id = g.create_object(&t, start, 0.0, 0);
    // Two updates: the second is the AI module's first (`setWakeFrame(now + 1)`), the frame the cheat
    // move runs on.
    g.update();
    g.update();
    assert_eq!(g.find_object(id).unwrap().position, start, "an immobile object keeps its position");
}
