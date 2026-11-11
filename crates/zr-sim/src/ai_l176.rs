//! Lane 176 (wave 20), the R20 review's AI items:
//! - R20-1: `AIGuardIdleState::m_guardeePos` is a `Coord3D` with no constructor (`AIGuard.h:181`); the
//!   `MemoryPool` zeroes the block, so an object guard's first idle scan compares against (0,0,0)
//!   (`AIGuard.cpp:757-772`) and takes `STATE_FAILURE` to `AI_GUARD_RETURN`.
//! - R20-3: a `STATE_SLEEP` return runs the state's conditions (`StateMachine.cpp:203-252`), so the
//!   early "waiting for the next scan" sleep checks `attackAggressors` too (`AIGuard.cpp:191`).
//! - R20-9: the attack move's resume runs `AIMoveToState::onEnter(); forceRepath();` and the move's update
//!   on the same frame (`AIStates.cpp:3669-3674`).
//! - S2: `privateBusy` clears the machine for a parked jet too (`AIUpdate.cpp:4054-4060`,
//!   `JetAIUpdate.cpp:2595-2601`).
//! - S3: the slave's leash reads the *slave's* module data (`SlavedUpdate.cpp:255-278`, `:706-716`).
//! - S4: `orderSlavesDisabledUntil` idles each slave through `aiIdle(CMD_FROM_AI)`, not `ai.stop()`
//!   (`SpawnBehavior.cpp:371-386`).

use super::*;
use crate::ai::AiTunables;
use crate::ai_states::AiStateType;
use crate::body::DamageInfo;
use crate::damage::DamageType;
use crate::orders::{GuardMode, GuardTarget};
use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
use crate::terrain::TerrainLogic;
use crate::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
use crate::Command;
use std::sync::Arc;

/// An armed vehicle whose guard scan radius is `vision` and whose weapon reaches `range`.
fn armed(name: &str, vision: f32, range: f32) -> Arc<ObjectTemplate> {
    let mut t = ObjectTemplate::new(name);
    t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
    t.geometry.major_radius = 8.0;
    t.max_health = 100.0;
    t.initial_health = 100.0;
    t.vision_range = vision;
    t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 120.0, 120.0, 360.0));
    let mut w = WeaponTemplate::new("Cannon");
    w.primary_damage = 20.0;
    w.damage_type = DamageType::ArmorPiercing;
    w.attack_range = range;
    w.min_delay_between_shots = 10;
    w.max_delay_between_shots = 10;
    let mut set = WeaponSetTemplate::default();
    set.weapons[0] = Some(Arc::new(w));
    t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
    Arc::new(t)
}

/// A weaponless, very tough vehicle: a guard's ward or chase target that never fights back.
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
    let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
    g.add_player(0, 0);
    g.add_player(1, 0);
    g.set_enemies(0, 1);
    g
}

fn guard_sub(g: &GameLogic, id: ObjectId) -> AiStateType {
    g.find_object(id).expect("object").ai.machine.guard_sub
}

/// Put `id` into the guard state (`AIGuardState::onEnter`, `AIStates.cpp:6733`) guarding `target`.
fn guard_enter(g: &mut GameLogic, id: ObjectId, target: Coord3D, object: ObjectId) {
    let now = g.frame();
    if let Some(o) = g.objects.get_mut(&id) {
        o.ai.machine.goal_position = target;
        o.ai.machine.goal_object = object;
        o.ai.machine.guard.mode = GuardMode::Normal;
        o.ai.machine.guard.next_scan_time = now;
    }
    g.ai_guard_enter(id);
}

/// R20-1: `AIGuardIdleState::m_guardeePos` starts at (0,0,0) — the zeroed `MemoryPool` block — so an
/// object guard's first idle scan finds its ward "moved" (unless it stands within two cells of the
/// origin), records the position and returns `STATE_FAILURE`, which the guard machine takes to
/// `AI_GUARD_RETURN` (`AIGuard.cpp:757-772`). Revert check: initialise `guardee_pos` to the guard's goal
/// position in `ai_guard_enter` (the old port) and the scan leaves the state in `AI_GUARD_IDLE`.
#[test]
fn an_object_guards_first_idle_scan_compares_against_the_zeroed_block() {
    let mut g = world();
    let id = g.create_object(&armed("Gun", 100.0, 50.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let ward = g.create_object(&dummy(), Coord3D::new(250.0, 100.0, 0.0), 0.0, 0);
    guard_enter(&mut g, id, Coord3D::new(250.0, 100.0, 0.0), ward);
    assert_eq!(
        g.find_object(id).unwrap().ai.machine.guard.guardee_pos,
        Coord3D::ZERO,
        "the freshly built AIGuardIdleState's block is zeroed, not the ward's position"
    );
    let now = g.frame();
    if let Some(o) = g.objects.get_mut(&id) {
        o.ai.machine.guard.next_scan_time = now; // the scan is due
        o.ai.machine.guard_sub = AiStateType::GuardIdle; // the guard reached its post
    }
    let ret = g.ai_guard_idle_update(id);
    assert!(matches!(ret, StateReturn::Continue), "a transition, not the scan's sleep");
    assert_eq!(guard_sub(&g, id), AiStateType::GuardReturn, "the ward is 250 units from (0,0,0)");
    assert_eq!(
        g.find_object(id).unwrap().ai.machine.guard.guardee_pos,
        Coord3D::new(250.0, 100.0, 0.0),
        "the scan records the ward's position"
    );
    // A second scan with the recorded position no longer fails.
    let now = g.frame();
    if let Some(o) = g.objects.get_mut(&id) {
        o.ai.machine.guard.next_scan_time = now;
        o.ai.machine.guard_sub = AiStateType::GuardIdle;
    }
    g.ai_guard_idle_update(id);
    assert_eq!(guard_sub(&g, id), AiStateType::GuardIdle, "no second return for the same position");
}

/// R20-3: `AIGuardIdleState::update`'s early `STATE_SLEEP(m_nextEnemyScanTime - now)` (`AIGuard.cpp:731`)
/// goes through `friend_checkForSleepTransitions` (`StateMachine.cpp:203-252`), which runs the state's
/// `attackAggressors` condition (`AIGuard.cpp:191`). A guard woken while it waits for its scan must return
/// fire instead of sleeping on. Revert check: return the sleep without the condition and the guard stays
/// in `AI_GUARD_IDLE`.
#[test]
fn an_idle_guard_waiting_for_its_scan_returns_fire_at_an_attacker() {
    let mut g = world();
    g.set_ai_tunables(AiTunables { guard_enemy_scan_rate: 15, ..AiTunables::default() });
    let id = g.create_object(&armed("Gun", 100.0, 50.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let post = Coord3D::new(100.0, 100.0, 0.0);
    guard_enter(&mut g, id, post, ObjectId::INVALID);
    let attacker = g.create_object(&armed("Enemy", 100.0, 50.0), Coord3D::new(130.0, 100.0, 0.0), 0.0, 1);
    // `ActiveBody::m_clearableLastAttacker`: the attacker the guard should answer.
    if let Some(o) = g.objects.get_mut(&id) {
        o.last_damage_info = DamageInfo { source: attacker, amount: 1.0, kind: DamageType::SmallArms };
        o.last_damage_cleared = false;
    }
    // The guard is asleep until a scan 20 frames away (it was woken by the hit).
    let now = g.frame();
    if let Some(o) = g.objects.get_mut(&id) {
        o.ai.machine.guard.next_scan_time = now + 20;
        o.ai.machine.guard_sub = AiStateType::GuardIdle;
    }
    let ret = g.ai_guard_idle_update(id);
    assert!(matches!(ret, StateReturn::Continue), "the transition clears the sleep (internalSetState, StateMachine.cpp:98)");
    assert_eq!(guard_sub(&g, id), AiStateType::GuardAttackAggressor, "attackAggressors fired on the sleep return");
    assert_eq!(
        g.find_object(id).unwrap().ai.machine.goal_object, attacker,
        "the aggressor became the attack's victim"
    );
}

/// R20-9: the attack move's resume runs `AIMoveToState::onEnter(); forceRepath();` and then the move's
/// update on that same frame (`AIStates.cpp:3669-3674`); the update's wait block stamps the state's
/// `m_pathTimestamp` (`:1789`). Revert check: call only the enter (the old port) and the timestamp stays
/// at the frame the pre-attack path landed, while `forceRepath`'s own value never reaches
/// `attack_move_path_goal`.
#[test]
fn an_attack_move_resumes_with_its_entry_and_update_on_the_same_frame() {
    let mut g = world();
    let id = g.create_object(&armed("Gun", 100.0, 50.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let goal = Coord3D::new(300.0, 100.0, 0.0);
    g.ai_attack_move_to_position(id, goal);
    g.run_frames(5);
    let before = g.find_object(id).unwrap().ai.move_path_timestamp;
    assert!(before < g.frame(), "the pre-attack path landed earlier (stamp {before})");
    // The attack ended: `AIAttackMoveToState` is current again, with its saved goal and resume marker.
    let saved = g.find_object(id).unwrap().ai.machine.attack_move_goal;
    if let Some(o) = g.objects.get_mut(&id) {
        o.ai.machine.attack_move_resume = true;
        o.ai.machine.current = AiStateType::AttackMoveTo;
        o.ai.machine.goal_position = saved;
    }
    let now = g.frame();
    g.ai_attack_move_enter(id);
    let o = g.find_object(id).unwrap();
    assert_eq!(o.ai.machine.attack_move_path_goal, Coord3D::new(-100.0, -100.0, -100.0), "forceRepath wrote the state's path goal");
    assert_eq!(o.ai.move_path_timestamp, now, "the move's update ran in the same frame (the wait block stamps it)");
}

/// S2: `AIUpdateInterface::privateBusy` (`AIUpdate.cpp:4054-4060`) clears the machine and enters `AI_BUSY`.
/// `JetAIUpdate::aiDoCommand` lets `AICMD_BUSY` through without taking off ("don't need (or want) to take
/// off for these", `JetAIUpdate.cpp:2595-2601`), so a parked jet's busy command must not start a takeoff.
/// Revert check: classify `Busy` as an "other" command (the old `classify`) and the state becomes
/// `JetTakingOffAwaitClearance`.
#[test]
fn a_parked_jets_busy_command_clears_its_machine_instead_of_taking_off() {
    let mut g = world();
    let mut t = (*armed("Jet", 300.0, 300.0)).clone();
    t.kind_of |= kind_of::AIRCRAFT;
    t.jet = Some(crate::modules2::JetAiTemplate {
        needs_runway: true,
        keeps_parking_space_when_airborne: true,
        takeoff_pause: 15,
        return_to_base_idle_time: 300,
        out_of_ammo_damage_per_second: 0.1,
        takeoff_dist_for_max_lift: 0.0,
        min_height: 5.0,
        parking_offset: 0.0,
        sneaky_offset_when_attacking: 0.0,
        attacking_loco: crate::template::LocomotorSet::Normal,
        returning_loco: crate::template::LocomotorSet::Normal,
        attack_loco_persist_time: 0,
        attackers_miss_persist_time: 0,
    });
    let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    // A parked jet: `ALLOW_AIR_LOCO` clear, and `HAS_PRODUCER_LOCATION` set so `getProducerLocation`
    // does not re-derive the flag from a (missing) airfield.
    if let Some(o) = g.objects.get_mut(&id) {
        o.jet.flags = crate::jet_ai::jet_flag::HAS_PRODUCER_LOCATION;
    }
    g.ai_busy(id, CommandSource::Ai);
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::Busy, "parked jets go AI_BUSY");
}

/// S3: the leash reads the *slave's* `SlavedUpdateModuleData` (`SlavedUpdate.cpp:255`, `:706-716`), not its
/// master's. The pinned point is `masterPos + m_guardPointOffset` (`:255-259`) and an idle slave more than
/// `CLOSE_ENOUGH` (15) from it is sent there (`:267-269`). Revert check: read the master's template (which
/// has `GuardMaxRange` 0 here) and no order is issued.
#[test]
fn a_slaves_leash_uses_the_slaves_own_guard_range() {
    let mut g = world();
    // The master also carries a `SlavedUpdate`, with GuardMaxRange 0: the old code read this one.
    let mut master_t = (*dummy()).clone();
    master_t.slaved = Some(crate::modules2::SlavedTemplate {
        guard_max_range: 0,
        guard_wander_range: 0,
        attack_range: 0,
        repair_rate_per_second: 0.0,
        stay_on_same_layer_as_master: false,
    });
    let master = g.create_object(&Arc::new(master_t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let mut slave_t = (*dummy()).clone();
    slave_t.slaved = Some(crate::modules2::SlavedTemplate {
        guard_max_range: 30,
        guard_wander_range: 0,
        attack_range: 0,
        repair_rate_per_second: 0.0,
        stay_on_same_layer_as_master: false,
    });
    let slave = g.create_object(&Arc::new(slave_t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.slaved_on_enslave(slave, master);
    let offset = g.find_object(slave).unwrap().behavior.slaved_guard_offset;
    assert!((offset.x * offset.x + offset.y * offset.y - 900.0).abs() < 1e-3, "GuardMaxRange 30 from the direction draw: {offset:?}");
    // Idle, far from the pinned point.
    if let Some(o) = g.objects.get_mut(&slave) {
        o.set_position(Coord3D::new(300.0, 100.0, 0.0));
        o.ai.machine.current = AiStateType::Idle;
    }
    let pinned = Coord3D::new(100.0 + offset.x, 100.0 + offset.y, 0.0);
    g.slaved_leash(slave, master);
    let o = g.find_object(slave).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::MoveTo, "the slave is sent to its pinned point");
    assert!(
        o.ai.machine.goal_position.dist_2d(&pinned) <= crate::pathfind::PATHFIND_CELL_SIZE_F,
        "the goal is the pinned point (adjusted to a cell): {:?} vs {pinned:?}",
        o.ai.machine.goal_position
    );
}

/// S3, the threshold: the C++ sends an *idle* slave more than `CLOSE_ENOUGH` (15 units) from its pinned
/// point there (`SlavedUpdate.cpp:266-271`) even when it is still inside `GuardMaxRange` of the master —
/// the old port's "further than GuardMaxRange from the master" rule would leave it alone. Revert check:
/// restore the old distance rule and no order is issued.
#[test]
fn an_idle_slave_between_its_pinned_point_and_its_guard_range_returns_to_the_point() {
    let mut g = world();
    let master = g.create_object(&dummy(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let mut slave_t = (*dummy()).clone();
    slave_t.slaved = Some(crate::modules2::SlavedTemplate {
        guard_max_range: 30,
        guard_wander_range: 0,
        attack_range: 0,
        repair_rate_per_second: 0.0,
        stay_on_same_layer_as_master: false,
    });
    let slave = g.create_object(&Arc::new(slave_t), Coord3D::new(110.0, 100.0, 0.0), 0.0, 0);
    g.slaved_on_enslave(slave, master);
    // Pin the direction so the geometry is exact: pinned = master + (30,0) = (130,100).
    if let Some(o) = g.objects.get_mut(&slave) {
        o.behavior.slaved_guard_offset = Coord3D::new(30.0, 0.0, 0.0);
        o.set_position(Coord3D::new(110.0, 100.0, 0.0)); // 20 from the pinned point, 10 from the master
        o.ai.machine.current = AiStateType::Idle;
    }
    g.slaved_leash(slave, master);
    let o = g.find_object(slave).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::MoveTo, "CLOSE_ENOUGH is 15, not GuardMaxRange");
    assert!(
        o.ai.machine.goal_position.dist_2d(&Coord3D::new(130.0, 100.0, 0.0)) <= crate::pathfind::PATHFIND_CELL_SIZE_F,
        "the goal is the pinned point: {:?}",
        o.ai.machine.goal_position
    );
}

/// S4: `SpawnBehavior::orderSlavesDisabledUntil` (`SpawnBehavior.cpp:371-386`) idles each slave with
/// `ai->aiIdle(CMD_FROM_AI)` and then disables it. `privateIdle` clears the state machine but keeps the
/// path (the next update's completed-move block drops it); the old `ai.stop()` shortcut cleared it here.
/// Revert check: `o.ai.stop()` empties the path.
#[test]
fn a_disabled_spawn_is_idled_through_private_idle() {
    let mut g = world();
    let mut spawner_t = (*dummy()).clone();
    spawner_t.spawn = Some(crate::modules2::SpawnTemplate {
        spawn_number: 1,
        spawn_replace_delay: 0,
        one_shot: false,
        spawn_template_names: Vec::new(),
        spawned_require_spawner: false,
        initial_burst: 0,
        aggregate_health: false,
    });
    let spawner = g.create_object(&Arc::new(spawner_t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let spawn = g.create_object(&armed("Spawn", 100.0, 50.0), Coord3D::new(120.0, 100.0, 0.0), 0.0, 0);
    if let Some(o) = g.objects.get_mut(&spawner) {
        o.spawned.push(spawn);
    }
    // The spawn is on a move with a path in flight.
    if let Some(o) = g.objects.get_mut(&spawn) {
        o.ai.machine.current = AiStateType::MoveTo;
        o.ai.path = vec![Coord3D::new(200.0, 100.0, 0.0)];
        o.ai.move_goal = Some(Coord3D::new(200.0, 100.0, 0.0));
    }
    let frame = g.frame() + 30;
    g.propagate_disabled_to_slaves(spawner, crate::disabled::DisabledType::Unmanned, frame);
    let o = g.find_object(spawn).unwrap();
    assert_eq!(o.ai.machine.current, AiStateType::Idle, "privateIdle enters AI_IDLE");
    assert_eq!(o.ai.path, vec![Coord3D::new(200.0, 100.0, 0.0)], "privateIdle does not clear the path");
    assert!(o.is_disabled_by_type(crate::disabled::DisabledType::Unmanned), "the slave is disabled until the frame");
}

/// S6: `SpawnBehavior::computeAggregateStates` sets `OBJECT_STATUS_MASKED` (`SpawnBehavior.cpp:1017`) from
/// the module's per-frame `update` (`:196-200`), whose first call is the frame after creation
/// (`UPDATE_SLEEP_NONE` in the constructor). So an aggregate-health spawner is targetable on its creation
/// frame and `WeaponSet::getAttackResult` rejects it (`WeaponSet.cpp:483-496`) from the next frame on: the
/// heap slot's first wake is the whole window.
#[test]
fn an_aggregate_spawner_is_masked_from_its_first_heap_frame() {
    let mut g = world();
    let mut spawner_t = (*dummy()).clone();
    spawner_t.spawn = Some(crate::modules2::SpawnTemplate {
        spawn_number: 1,
        spawn_replace_delay: 0,
        one_shot: false,
        spawn_template_names: Vec::new(),
        spawned_require_spawner: false,
        initial_burst: 0,
        aggregate_health: true,
    });
    let spawner = g.create_object(&Arc::new(spawner_t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    let masked = |g: &GameLogic| g.find_object(spawner).unwrap().status_bits & crate::object::status::MASKED != 0;
    assert!(!masked(&g), "the creation frame: the module has not run yet");
    g.update(); // frame 0
    assert!(!masked(&g), "still the creation frame's update: the spawn module wakes next frame");
    g.update(); // frame 1
    assert!(masked(&g), "the first heap frame of `SpawnBehavior::update` sets MASKED");
}

/// R20-1, the other half: only an *object* guard has a guardee position to compare. A location guard's
/// `findTargetToGuardByID()` is null (`AIGuard.cpp:758-759`), so its idle scan never runs the move test —
/// the zeroed `m_guardeePos` must not be compared with `m_positionToGuard`. Revert check: use
/// `ai_guard_target` (which falls back to the position to guard) and the first scan "moves" to RETURN.
#[test]
fn a_location_guards_first_idle_scan_does_not_compare_its_post() {
    let mut g = world();
    let id = g.create_object(&armed("Gun", 100.0, 50.0), Coord3D::new(250.0, 100.0, 0.0), 0.0, 0);
    guard_enter(&mut g, id, Coord3D::new(250.0, 100.0, 0.0), ObjectId::INVALID);
    let now = g.frame();
    if let Some(o) = g.objects.get_mut(&id) {
        o.ai.machine.guard.next_scan_time = now;
        o.ai.machine.guard_sub = AiStateType::GuardIdle;
    }
    g.ai_guard_idle_update(id);
    assert_eq!(guard_sub(&g, id), AiStateType::GuardIdle, "a location guard has no guardee to move");
}

/// R20-4b: `BuildAssistant.cpp:719-728` blocks a placement over an allied unit whose
/// `AIUpdateInterface::isBusy()` is true — `getStateMachine()->isInBusyState()` (`AIUpdate.cpp:3139-3142`),
/// i.e. the AI state machine's current state is `AIBusyState` (`StateMachine.h:262`, the only `isBusy()`
/// override, `AIStateMachine.h:345`), not the hack-internet module's phase. Revert check: test
/// `hack_internet::is_busy` instead and the busy ally no longer blocks.
#[test]
fn a_busy_allied_unit_blocks_a_building_placement() {
    let mut g = world();
    let pos = Coord3D::new(100.0, 100.0, 0.0);
    let unit = g.create_object(&dummy(), pos, 0.0, 0);
    let mut site = ObjectTemplate::new("Site");
    site.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
    site.geometry.major_radius = 20.0;
    site.geometry.minor_radius = 20.0;
    site.geometry.height = 10.0;
    site.max_health = 500.0;
    site.initial_health = 500.0;
    assert!(
        g.is_location_legal_to_build_for(&site, &pos, 0.0, Some(0), Some(0)),
        "an idle ally under the footprint does not block the placement (mobile, allied)"
    );
    if let Some(o) = g.objects.get_mut(&unit) {
        o.ai.machine.current = AiStateType::Busy;
    }
    assert!(
        !g.is_location_legal_to_build_for(&site, &pos, 0.0, Some(0), Some(0)),
        "`them->getAI()->isBusy()` returns LBC_OBJECTS_IN_THE_WAY"
    );
}

/// A guard order must still go through the public command path (the state tests above drive the guard
/// state directly): `MSG_GUARD_POSITION` enters `AI_GUARD` (`privateGuardPosition`, `AIUpdate.cpp:4066`).
#[test]
fn a_guard_order_still_enters_the_guard_state() {
    let mut g = world();
    let id = g.create_object(&armed("Gun", 100.0, 50.0), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
    g.submit(0, Command::Guard { ids: vec![id], target: GuardTarget::Point(Coord3D::new(100.0, 100.0, 0.0)), mode: GuardMode::Normal });
    g.update();
    assert_eq!(g.find_object(id).unwrap().ai.machine.current, AiStateType::Guard, "the order enters AI_GUARD");
}
