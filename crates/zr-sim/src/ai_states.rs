//! `AIStateMachine` (`AI/AIStates.cpp` / `AIStateMachine.h`) and the `StateMachine` framework
//! (`Common/StateMachine.cpp` / `StateMachine.h`).
//!
//! The engine drives one `AIStateMachine` per `AIUpdateInterface` on the sleepy schedule; the sim
//! runs it once per object per frame from [`GameLogic::update_ai_state_machines`]. States are enum
//! variants plus a persistent [`AiMachine`] blob carried on [`crate::ai::AiState`]; each state's
//! `onEnter` / `update` / `onExit` is ported in the same order as the C++, with the C++ line cited.
//!
//! Ported states: `AI_IDLE`, `AI_MOVE_TO` (`AIInternalMoveToState` / `AIMoveToState`),
//! `AI_MOVE_OUT_OF_THE_WAY`, `AI_MOVE_AND_TIGHTEN`, `AI_ATTACK_MOVE_TO`, `AI_ATTACK_OBJECT`,
//! `AI_ATTACK_POSITION`, `AI_FORCE_ATTACK_OBJECT`, `AI_ATTACK_AND_FOLLOW_OBJECT`, `AI_HUNT`,
//! `AI_ATTACK_AREA`, `AI_GUARD` (`AIGuardMachine`), `AI_ENTER`, `AI_EXIT`, `AI_EXIT_INSTANTLY`,
//! `AI_FACE_OBJECT`, `AI_FACE_POSITION`, `AI_BUSY`, `AI_WAIT`, `AI_DEAD`.
//!
//! `AI_DOCK` is the `AIDockMachine` (`AIDock.cpp`) driving [`crate::dock`] (see `crate::supply`).
//!
//! Reduced: `AI_WANDER`/`AI_PANIC`/`AI_WANDER_IN_PLACE` (the civ repulsor/wander paths
//! have no caller in the sim), `AI_RAPPEL_INTO` (the Chinook keeps its own `zr-sim::chinook` port),
//! `AI_ATTACK_SQUAD`, `AI_FOLLOW_WAYPOINT_PATH*` (the script engine feeds a goal path through the
//! move states instead) and the `AI_MOVE_AND_EVACUATE`/`AI_MOVE_AND_DELETE` production-exit paths.

use crate::ai::{LocoGoalType, MIN_REPATH_TIME};
use crate::combat::{AttackExitConditions, AttackOrder, AttackTarget};
use crate::logic::GameLogic;
use crate::math::{Coord3D, LOGICFRAMES_PER_SECOND};
use crate::module::{UPDATE_SLEEP_FOREVER, UPDATE_SLEEP_NONE};
use crate::object::ObjectId;
use crate::pathfind::{PathObject, PATHFIND_CELL_SIZE_F};
use crate::template::kind_of;

/// `AIStateType` (`AIStateMachine.h:58`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiStateType {
    Idle,
    MoveTo,
    MoveOutOfTheWay,
    MoveAndTighten,
    AttackMoveTo,
    AttackPosition,
    AttackObject,
    ForceAttackObject,
    AttackAndFollowObject,
    Hunt,
    AttackArea,
    Guard,
    Enter,
    Exit,
    ExitInstantly,
    Wander,
    Wait,
    Dead,
    Busy,
    FaceObject,
    FacePosition,
    Dock,
    // Guard sub-machine states (`AIGuard.h`).
    GuardInner,
    GuardOuter,
    GuardReturn,
    GuardIdle,
    GuardGetCrate,
    GuardAttackAggressor,
    // `AIStateMachine` states ported for the script / production callers (appended so the earlier
    // discriminants stay stable).
    /// `AI_FOLLOW_PATH` / `AI_FOLLOW_EXITPRODUCTION_PATH` (`AIFollowPathState`).
    FollowPath,
    /// `AI_FOLLOW_WAYPOINT_PATH_AS_TEAM` / `..._AS_INDIVIDUALS` (`AIFollowWaypointPathState`).
    FollowWaypointPath,
    /// `AI_MOVE_AND_EVACUATE` / `..._AND_EXIT` (`AIMoveAndEvacuateState`).
    MoveAndEvacuate,
    /// `AI_MOVE_AND_DELETE` (`AIMoveAndDeleteState`).
    MoveAndDelete,
    /// `AI_WANDER_IN_PLACE` (`AIWanderInPlaceState`).
    WanderInPlace,
    /// `AI_PANIC` (`AIPanicState`).
    Panic,
    /// `AI_ATTACK_SQUAD` (`AIAttackSquadState`).
    AttackSquad,
    /// `AI_ATTACKFOLLOW_WAYPOINT_PATH_AS_TEAM` / `..._AS_INDIVIDUALS` (`AIAttackFollowWaypointPathState`):
    /// follow a waypoint path, attacking what the mood scan finds on the way.
    AttackFollowWaypointPath,
    // The `JetAIStateType` states (`JetAIUpdate.cpp:62`), see `crate::jet_states`.
    JetTakingOffAwaitClearance,
    JetTaxiToTakeoff,
    JetPauseBeforeTakeoff,
    JetTakingOff,
    JetLandingAwaitClearance,
    JetLanding,
    JetTaxiFromLanding,
    JetTaxiFromHangar,
    JetOrientForParking,
    JetReloadAmmo,
    JetReturningForLanding,
    JetReturnToDeadAirfield,
    JetCirclingDeadAirfield,
    /// `HeliTakeoffOrLandingState` (`TAKING_OFF` / `LANDING` of the helicopter machine).
    HeliTakingOff,
    HeliLanding,
    /// `AI_FOLLOW_EXITPRODUCTION_PATH` (`AIStates.cpp:700`): the `AIFollowPathState` a unit leaving
    /// production walks. Appended last so the earlier discriminants (`crc`) stay stable.
    FollowExitProductionPath,
    /// `AI_MOVE_AND_EVACUATE_AND_EXIT` (`AIMoveAndEvacuateState`, `AIStates.cpp:702`): move, empty the
    /// transport, then move to the goal again and delete itself (both of its exit states are
    /// `AI_MOVE_AND_DELETE`). Appended last so the earlier discriminants (`crc`) stay stable.
    MoveAndEvacuateAndExit,
    /// `AI_GUARD_RETALIATE` (`AIGuardRetaliateState`, `AIStates.cpp:720`): `ActiveBody`'s retaliation
    /// assist (`ActiveBody.cpp:722`) orders a human player's idle allies to avenge a hit. The C++
    /// state's current id stays `AI_GUARD_RETALIATE` while its sub-machine attacks, so the snapshot
    /// prints `GuardRetaliate` for the whole order. See `crate::ai_guard_retaliate`.
    GuardRetaliate,
}

impl AiStateType {
    pub(crate) fn crc(self) -> u32 {
        self as u32
    }
}

/// `StateReturnType` (`StateMachine.h:61`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateReturn {
    Continue,
    Success,
    Failure,
    Sleep(u32),
}

/// `CommandSourceType` (`GameType.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CommandSource {
    Player,
    #[default]
    Ai,
    Script,
}

/// `AK_GUARDTARGET` (`AIUpdate.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuardTargetType {
    None,
    Location,
    Object,
    Area,
}

/// `AIDockMachine`'s states (`AIDock.cpp:52`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockSub {
    Approach,
    WaitForClearance,
    Advance,
    MoveToEntry,
    MoveToDock,
    ProcessDock,
    MoveToExit,
    MoveToRally,
}

/// `AIGuardMachine` state (`AIGuard.cpp:161`).
#[derive(Clone, Debug, PartialEq)]
pub struct GuardMachine {
    /// `m_targetToGuard`.
    pub target_to_guard: ObjectId,
    /// `m_positionToGuard`.
    pub position_to_guard: Coord3D,
    /// `m_nemesisToAttack`.
    pub nemesis: ObjectId,
    /// `m_guardMode`.
    pub mode: crate::orders::GuardMode,
    /// `m_guardeePos` in `AIGuardIdleState`.
    pub guardee_pos: Coord3D,
    /// `m_nextEnemyScanTime` / `m_nextReturnScanTime`.
    pub next_scan_time: u32,
    /// `ExitConditions::m_center` / `m_radiusSqr` / `m_attackGiveUpFrame` for the outer state.
    pub attack_center: Coord3D,
    pub attack_radius_sqr: f32,
    pub attack_give_up_frame: u32,
    /// `m_areaToGuard` (AIGuard.cpp, AIUpdate.h): the polygon of an area guard, `None` for a point or object guard
    /// (lane 184, `guard_area.rs`).
    pub area: Option<std::sync::Arc<zr_map::PolygonTrigger>>,
}

impl Default for GuardMachine {
    fn default() -> Self {
        Self {
            target_to_guard: ObjectId::INVALID,
            position_to_guard: Coord3D::ZERO,
            nemesis: ObjectId::INVALID,
            mode: crate::orders::GuardMode::Normal,
            guardee_pos: Coord3D::ZERO,
            next_scan_time: 0,
            attack_center: Coord3D::ZERO,
            attack_radius_sqr: 0.0,
            attack_give_up_frame: 0,
            area: None,
        }
    }
}

/// A waypoint-graph walk's position (`AIFollowWaypointPathState::m_currentWaypoint` and `hasNextWaypoint`):
/// the waypoint id, and whether it has a link to follow (`open`). The script engine supplies it, since
/// the waypoint graph lives there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalkAt {
    pub waypoint: i32,
    pub open: bool,
}

/// The persistent `AIStateMachine` payload (`AIStateMachine.h:128`). The move-state scratch fields
/// (`move_goal`, `path_goal_position`, `waiting_for_path`, ...) live on [`AiState`].
#[derive(Clone, Debug, PartialEq)]
pub struct AiMachine {
    /// `StateMachine::m_currentState` / `m_defaultStateID`.
    pub current: AiStateType,
    /// `m_sleepTill` (`0` = awake).
    pub sleep_till: u32,
    /// `m_goalObjectID` / `m_goalPosition`.
    pub goal_object: ObjectId,
    pub goal_position: Coord3D,
    /// `m_temporaryState` + `m_temporaryStateFramEnd`.
    pub temporary: Option<AiStateType>,
    pub temporary_end: u32,
    /// `AIUpdateInterface::m_lastCommandSource` (`getLastCommandSource`).
    pub last_command_source: CommandSource,
    /// `AIUpdateInterface::m_guardTargetType` / `m_guardMode` / `m_locationToGuard` /
    /// `m_objectToGuard`, read by `AIGuardState::onEnter`.
    pub guard_target_type: GuardTargetType,
    pub guard_position: Coord3D,
    pub guard_object: ObjectId,
    /// `AIGuardMachine` state.
    pub guard: GuardMachine,
    /// `AIGuardRetaliateMachine` state (`AIGuardRetaliate.h:89-121` + its states' fields), live while
    /// `current` is `GuardRetaliate` (`crate::ai_guard_retaliate`).
    pub retaliate: crate::ai_guard_retaliate::RetaliateMachine,
    /// `AIInternalMoveToState::m_waitingForPath`'s state-level twin; the sim keeps the actual flag on
    /// `AiState`, this records whether the state entered waiting.
    pub move_try_one_more_repath: bool,
    /// `ChinookMoveToBldgState` (ChinookAIUpdate.cpp:692-782), the combat drop's move: `Some(m_destZ)` while the move is
    /// the drop's. Its success waits until the Chinook is within 3 of `m_destZ` (`update`, :770-775). Set by
    /// `start_combat_drop`; cleared when the move exits.
    pub drop_move_z: Option<f32>,
    /// The combat drop's move result, taken by the Chinook's update: `MOVE_TO_COMBAT_DROP` goes to `DO_COMBAT_DROP` on
    /// success and to `AI_IDLE` on failure (ChinookAIUpdate.cpp:853).
    pub drop_move_result: Option<bool>,
    /// `AIAttackApproachTargetState` / `AIAttackPursueTargetState` scratch.
    pub attack_prev_victim_pos: Coord3D,
    pub attack_approach_timestamp: u32,
    pub attack_initial_approach: bool,
    /// `AIUpdateInterface::m_currentVictimID` (`AIUpdate.cpp:215`, `setCurrentVictim`): set by the attack's update passes
    /// (`AIAttackState::update`, AIStates.cpp:5630; approach :2704; pursue :3054), cleared when the attack ends
    /// (`AIAttackState::onExit`, :5739). The snapshot's `atk` and the trace's `victim` read it (`getCurrentVictim`).
    pub current_victim: ObjectId,
    pub attack_stop_if_in_range: bool,
    pub attack_original_victim_pos: Coord3D,
    pub attack_force: bool,
    pub attack_follow: bool,
    /// `AIAttackState::m_isAttackingObject` (false = attacking a position).
    pub attack_is_object: bool,
    /// `AIHuntState::m_nextEnemyScanTime` / `AIAttackAreaState`.
    pub next_enemy_scan: u32,
    /// `AIEnterState::m_entryToClear` / `AIExitState::m_entryToClear`.
    pub entry_to_clear: ObjectId,
    /// `AIAttackMoveToState::m_frameToSleepUntil` / `m_retryCount`.
    pub attack_move_sleep_until: u32,
    pub attack_move_retry: i32,
    /// The attack-move state object's own `m_pathGoalPosition` (`AIStateMachine.h:295`): zero from the constructor
    /// (`:254`), written only when a path arrives during its own update (`AIStates.cpp:1804`), not by a re-entry and
    /// not by a temporary move, which is another state object. Its retry test reads it (`:3680`).
    pub attack_move_path_goal: Coord3D,
    /// `AIGuardIdleState` / `AIGuardReturnState` random scan offsets.
    pub idle_sleep_offset: u16,
    /// `AIAttackState::m_lockedWeaponOnEnter` (`isCurWeaponLocked`).
    pub locked_weapon_on_enter: bool,
    /// `AIFaceState::m_canTurnInPlace` (`AIStates.cpp:7463`): the current locomotor's `MinSpeed` is zero,
    /// so the state turns on the spot instead of driving at the target.
    pub face_can_turn_in_place: bool,
    /// The group destination for an `AIGroup` move (`AIGroup::computeIndividualDestination`); the
    /// state-level twin of `AiState::group_dest`, re-applied on repath.
    pub group_dest: Option<Coord3D>,
    /// The state to return to when an `AI_ATTACK_*` state finishes (models the C++ attack
    /// sub-machine's enclosing state: hunt / attack-area / attack-move / guard).
    pub attack_return: AiStateType,
    /// `AIGuardState::m_guardMachine` presence-flag for `isAttack()`/`isGuardIdle()`.
    pub in_guard_attack: bool,
    pub in_guard_idle: bool,
    /// The `AIGuardMachine` sub-state (`AI_GUARD_INNER` / `_OUTER` / `_RETURN` / `_IDLE`).
    pub guard_sub: AiStateType,
    /// `AIGuardInnerState::m_attackState` presence.
    pub guard_attack_active: bool,
    /// `AIIdleState::m_inited`.
    pub idle_inited: bool,
    /// `AIStateMachine::m_goalPath` + `AIFollowPathState::m_index` / `m_retryCount`.
    pub goal_path: Vec<Coord3D>,
    pub goal_path_index: usize,
    pub goal_path_retry: i32,
    /// `AIFollowWaypointPathState::m_currentWaypoint`: the waypoint the path ends at, when the path walks
    /// the waypoint graph. On arrival there the next waypoint is a random link of it
    /// (`getNextWaypoint`, AIStates.cpp:3791); `None` for an ordinary path.
    pub goal_waypoint: Option<WalkAt>,
    /// The attack-move state's own goal. In the engine the attack sub-machine (`m_attackMoveMachine`)
    /// keeps its own goal object/position, so an interception never changes where the attack-move is
    /// headed; the sim shares one goal, so it saves the destination here when an attack starts
    /// and restores it when the state resumes (`attack_move_resume`).
    pub attack_move_goal: Coord3D,
    pub attack_move_resume: bool,
    /// `AIAttackMoveToState::m_commandSrc` (`AIStates.cpp:3590`): the command source the attack-move
    /// started with. An interception sets `CMD_FROM_AI` (`:3658`), and when the sub-machine becomes idle
    /// again the state puts the saved source back (`:3631`) — that is what the stealth rule
    /// (`StealthUpdate.cpp:720-729`) must see after a player's attack-move finishes a fight.
    pub attack_move_command_source: CommandSource,
    /// Set only while [`ai_start_attack`](Self::ai_start_attack) hands the attack-move over to its own
    /// attack: the attack-move's sub-machine goes from `AI_IDLE` to an attack (`AIAttackMoveToState::update`,
    /// AIStates.cpp:3655), which is not an exit of the attack-move, so its `AI_IDLE` exit draw is skipped.
    /// Transient (cleared before the call returns), so it is not hashed.
    pub attack_move_handoff: bool,
    /// The port's marker for the attack-follow state re-entered after the attack it ran: the engine's
    /// sub-machine (`m_attackFollowMachine`) entered `AI_IDLE` with that attack's exit
    /// (`AIAttackFollowWaypointPathState::update`, `AIStates.cpp:4462-4477`), so the re-entry must not
    /// enter it a second time, and its scan is `getNextMoodTarget(false, false)` (`:4493`).
    /// Cleared as soon as the re-entry runs; only an attack the follow state owns outlives it.
    pub attack_follow_resume: bool,
    /// `AIWanderInPlaceState::m_origin`.
    pub wander_origin: Coord3D,
    /// `AIDockMachine::m_approachPosition` + the current sub-state (`AIDock.cpp`).
    pub dock_approach: i32,
    pub dock_sub: DockSub,
    /// `AIDockWaitForClearanceState::m_enterFrame`.
    pub dock_enter_frame: u32,
    /// `AIDockProcessDockState::m_nextDockActionFrame`.
    pub dock_next_action: u32,
    /// `StateMachine::m_locked` (`lock`/`unlock`): while set, `setState` from outside the machine
    /// is ignored (`AIDockMoveToDockState` locks while moving inside the dock).
    pub locked: bool,
    /// `AIInternalMoveToState::getAdjustsDestination()`: the move adjusts its goal in `onEnter`
    /// (`AIStates.cpp:1661`) and finishes by clearing the locomotor goal (`AIStates.cpp:1930`). The adjust itself
    /// runs at command time ([`ai_adjust_move_goal`](GameLogic::ai_adjust_move_goal)); the path requests do not.
    pub move_adjusts: bool,
    /// The move fields that are not live (see [`MoveStateFields`] and `GameLogic::enter_temp_fields`). With a
    /// temporary state, it is the temporary's set while the main move's fields are live, and the main move's set
    /// while the temporary's own code runs ([`temp_live`](Self::temp_live)). `None` without a temporary state.
    pub parked_move: Option<MoveStateFields>,
    /// `true` while the temporary state's own code runs (its enter, update or exit), so the live `ai.move_goal` etc.
    /// are the temporary's. Between those runs the main move's fields are live, so a command, a stop or a death
    /// writes the main move (C++ keeps the temporary and the main as separate state objects, AIStateMachine.h:247-300).
    pub temp_live: bool,
}

/// The state-level fields of `AIInternalMoveToState` (`AIStateMachine.h:247-300`): `m_goalPosition`,
/// `m_pathGoalPosition`, `m_pathTimestamp`, `m_blockedRepathTimestamp`, `m_adjustDestinations`,
/// `m_tryOneMoreRepath` and the state's `m_waitingForPath`. C++ keeps them on each state object, so a
/// temporary move (`AIMoveOutOfTheWayState`, a dock move `setTemporaryState(AI_MOVE_TO)`, `AIStates.cpp:919`)
/// is a separate object and cannot change the goal or the repath clocks of the main move. The port keeps
/// one set on the unit, so `ai_set_temporary_state` saves the set and `ai_end_temporary_state` restores it
/// when the temporary state ends; the main move then resumes with its own goal (`AIStates.cpp:850-879`,
/// the main `update` runs after the temporary one exits).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveStateFields {
    pub move_goal: Option<Coord3D>,
    pub path_goal_position: Coord3D,
    pub move_waiting_for_path: bool,
    pub move_path_timestamp: u32,
    pub blocked_repath_timestamp: u32,
    pub try_one_more_repath: bool,
    pub adjusts_destination: bool,
    pub machine_try_one_more_repath: bool,
    pub machine_move_adjusts: bool,
}

impl Default for AiMachine {
    fn default() -> Self {
        Self {
            current: AiStateType::Idle,
            sleep_till: 0,
            goal_object: ObjectId::INVALID,
            goal_position: Coord3D::ZERO,
            temporary: None,
            temporary_end: 0,
            last_command_source: CommandSource::Ai,
            guard_target_type: GuardTargetType::None,
            guard_position: Coord3D::ZERO,
            guard_object: ObjectId::INVALID,
            guard: GuardMachine::default(),
            retaliate: crate::ai_guard_retaliate::RetaliateMachine::default(),
            move_try_one_more_repath: true,
            drop_move_z: None,
            drop_move_result: None,
            attack_prev_victim_pos: Coord3D::ZERO,
            attack_approach_timestamp: 0,
            attack_initial_approach: true,
            current_victim: ObjectId::INVALID,
            attack_stop_if_in_range: false,
            attack_original_victim_pos: Coord3D::ZERO,
            attack_force: false,
            attack_follow: false,
            attack_is_object: true,
            next_enemy_scan: 0,
            entry_to_clear: ObjectId::INVALID,
            attack_move_sleep_until: 0,
            attack_move_retry: 5,
            attack_move_path_goal: Coord3D::ZERO,
            idle_sleep_offset: 0,
            locked_weapon_on_enter: false,
            face_can_turn_in_place: false,
            group_dest: None,
            attack_return: AiStateType::Idle,
            in_guard_attack: false,
            in_guard_idle: false,
            guard_sub: AiStateType::GuardIdle,
            guard_attack_active: false,
            idle_inited: false,
            goal_path: Vec::new(),
            goal_path_index: 0,
            goal_path_retry: 0,
            goal_waypoint: None,
            attack_move_goal: Coord3D::ZERO,
            attack_move_resume: false,
            attack_move_command_source: CommandSource::Ai,
            attack_move_handoff: false,
            attack_follow_resume: false,
            wander_origin: Coord3D::ZERO,
            dock_approach: -1,
            dock_sub: DockSub::Approach,
            dock_enter_frame: 0,
            dock_next_action: 0,
            locked: false,
            move_adjusts: true,
            parked_move: None,
            temp_live: false,
        }
    }
}

impl AiMachine {
    pub(crate) fn crc(&self, c: &mut crate::crc::Crc) {
        c.add_u32(self.current.crc());
        c.add_u32(self.sleep_till);
        c.add_u32(self.goal_object.0);
        c.add_coord(&self.goal_position);
        c.add_bool(self.temporary.is_some());
        if let Some(t) = self.temporary {
            c.add_u32(t.crc());
        }
        c.add_u32(self.temporary_end);
        c.add_u32(self.last_command_source as u32);
        c.add_u32(self.guard_target_type as u32);
        c.add_coord(&self.guard_position);
        c.add_u32(self.guard_object.0);
        c.add_u32(self.guard.target_to_guard.0);
        c.add_coord(&self.guard.position_to_guard);
        c.add_u32(self.guard.nemesis.0);
        c.add_u32(self.guard.mode as u32);
        c.add_coord(&self.guard.guardee_pos);
        c.add_u32(self.guard.next_scan_time);
        c.add_coord(&self.guard.attack_center);
        c.add_f32(self.guard.attack_radius_sqr);
        c.add_u32(self.guard.attack_give_up_frame);
        self.retaliate.crc(c);
        c.add_bool(self.move_try_one_more_repath);
        c.add_coord(&self.attack_prev_victim_pos);
        c.add_u32(self.attack_approach_timestamp);
        c.add_bool(self.attack_initial_approach);
        c.add_bool(self.attack_stop_if_in_range);
        c.add_coord(&self.attack_original_victim_pos);
        c.add_bool(self.attack_force);
        c.add_bool(self.attack_follow);
        c.add_bool(self.attack_is_object);
        c.add_u32(self.next_enemy_scan);
        c.add_u32(self.entry_to_clear.0);
        c.add_u32(self.attack_move_sleep_until);
        c.add_u32(self.attack_move_retry as u32);
        c.add_coord(&self.attack_move_path_goal);
        c.add_u32(u32::from(self.idle_sleep_offset));
        c.add_bool(self.locked_weapon_on_enter);
        c.add_bool(self.move_adjusts);
        c.add_bool(self.temp_live);
        match self.parked_move {
            Some(s) => {
                c.add_bool(true);
                c.add_bool(s.move_goal.is_some());
                c.add_coord(&s.move_goal.unwrap_or(Coord3D::ZERO));
                c.add_coord(&s.path_goal_position);
                c.add_bool(s.move_waiting_for_path);
                c.add_u32(s.move_path_timestamp);
                c.add_u32(s.blocked_repath_timestamp);
                c.add_bool(s.try_one_more_repath);
                c.add_bool(s.adjusts_destination);
                c.add_bool(s.machine_try_one_more_repath);
                c.add_bool(s.machine_move_adjusts);
            }
            None => c.add_bool(false),
        }
        c.add_u32(self.attack_return.crc());
        match self.group_dest {
            Some(g) => {
                c.add_bool(true);
                c.add_coord(&g);
            }
            None => c.add_bool(false),
        }
        c.add_bool(self.in_guard_attack);
        c.add_bool(self.in_guard_idle);
        c.add_u32(self.guard_sub.crc());
        c.add_bool(self.guard_attack_active);
        c.add_bool(self.idle_inited);
        c.add_u32(self.goal_path.len() as u32);
        for p in &self.goal_path {
            c.add_coord(p);
        }
        c.add_u32(self.goal_path_index as u32);
        c.add_u32(self.goal_path_retry as u32);
        match self.goal_waypoint {
            Some(w) => {
                c.add_u32(w.waypoint as u32);
                c.add_bool(w.open);
            }
            None => c.add_u32(u32::MAX),
        }
        c.add_coord(&self.attack_move_goal);
        c.add_bool(self.attack_move_resume);
        c.add_u32(self.attack_move_command_source as u32);
        c.add_bool(self.attack_follow_resume);
        c.add_coord(&self.wander_origin);
        c.add_u32(self.dock_approach as u32);
        c.add_u32(self.dock_sub as u32);
        c.add_u32(self.dock_enter_frame);
        c.add_u32(self.dock_next_action);
        c.add_bool(self.locked);
    }
}

const ATTACK_RETRY_COUNT: i32 = 5;
const ATTACK_CLOSE_ENOUGH_CELLS: f32 = 8.0;
/// `AIIdleState::IDLE_COUNTDOWN_DELAY` (`AIStates.cpp:1269`): `LOGICFRAMES_PER_SECOND * 2`.
const IDLE_COUNTDOWN_DELAY: u32 = 2 * LOGICFRAMES_PER_SECOND;
/// `AI_IDLE` entries of the `AIStateMachine` state table's sub-machines, made while the machine is built
/// (`AIUpdateInterfaceMachine` construction, AIUpdate.cpp:629 -> `makeStateMachine`): the `AIAttackMoveToState`
/// constructor inits its `AIAttackMoveMachine` (AIStates.cpp:3527-3528), and the two `AIAttackFollowWaypointPathState`s
/// (AI_ATTACKFOLLOW... defined at AIStates.cpp:692-693) each init an `AIAttackFollowMachine` (:4392-4393). Each
/// default state is `AI_IDLE` and `AIIdleState::onEnter` draws its sleep offset (AIStates.cpp:1327).
/// TheSuperHackers' `ZR_SMNEW`/`ZR_IDLE` trace (`zh-cpp-build`, AN3 seed 1) shows exactly these three per
/// `AIUpdateInterfaceMachine`; the worker, dozer and supply-truck machines enter no `AI_IDLE`.
const AI_STATE_TABLE_SUBMACHINE_IDLES: u32 = 3;
/// `AIUpdateInterface::onObjectCreated`'s own `initDefaultState` (AIUpdate.cpp:630): the main machine's `AI_IDLE`.
const AI_MAIN_MACHINE_IDLES: u32 = 1;
const ENEMY_SCAN_RATE: u32 = LOGICFRAMES_PER_SECOND;

/// The main machine's per-state transition table: `(success, failure)` (`AIStates.cpp:685-728`).
/// Every main-machine state succeeds/fails back to `AI_IDLE`; the guard sub-machine has its own
/// targets handled inside the guard states. Conditions that jump elsewhere are handled inside the
/// state updates (they call `ai_set_state`).
fn state_transition(_s: AiStateType) -> (AiStateType, AiStateType) {
    (AiStateType::Idle, AiStateType::Idle)
}

impl GameLogic {
    /// Whether the per-frame AI pass runs this object (the filter of the old id walk, without the
    /// "has an AI module or a weapon" clause).
    fn ai_pass_runs(o: &crate::object::Object) -> bool {
        !o.is_destroyed()
            // A dead AI stays in the list: `AIUpdateInterface::update` still runs for it and
            // switches it to `AI_DEAD` (`AIUpdate.cpp:1102`). Dead objects without an AI are out.
            && (!o.is_dying() || (o.effectively_dead && o.ai_handle.is_some()))
            && !o.ai.suspended
            // A unit unpacking, hacking or packing runs `HackInternetStateMachine`, which is its
            // whole AI (`HackInternetAIUpdate::isHackingPackingOrUnpacking`).
            && !crate::hack_internet::is_busy(o)
        // Contained passengers keep running their `AIUpdateInterface`
        // (`AIUpdateModuleData::getDisabledTypesToProcess` is `DISABLED_HELD`, AIUpdate.h:305),
        // so an order given to a passenger still completes: its locomotor cannot move it
        // (`PhysicsBehavior::update` skips `DISABLED_HELD`, PhysicsUpdate.cpp:641) but the
        // container carries it to the goal. Firing is gated by `Object::isAbleToAttack`.
    }

    /// The sleepy heap's loop for this frame (`GameLogic::update`, GameLogic.cpp:3851-3905), which runs every due
    /// module in heap order, the AI modules (`PHASE_INITIAL`, key `(wake << 2) | 0`) among them. Each AI module
    /// runs its object's AI update (`ai_update_object`) and is re-keyed to `now + sleep`. The loop is
    /// [`GameLogic::run_sleepy_updates_with`]: a non-AI module that is due before an AI module runs first, as
    /// in the original (its key is smaller, lane 99).
    pub(crate) fn run_ai_heap_pass(&mut self, now: u32) {
        self.run_sleepy_updates_with(now, Self::ai_update_object);
    }

    /// `AIUpdate` module call for the sleepy heap: runs the object's AI update when it is due and
    /// reports the sleep. The sleep is `UPDATE_SLEEP_NONE` (1) while the object is awake, so it runs
    /// again next frame, as before the heap; a destroyed or gone object sleeps forever.
    fn ai_update_object(&mut self, id: ObjectId) -> u32 {
        let Some(o) = self.objects.get(&id) else {
            return UPDATE_SLEEP_FOREVER;
        };
        if !Self::ai_pass_runs(o) {
            return if o.is_destroyed() { UPDATE_SLEEP_FOREVER } else { UPDATE_SLEEP_NONE };
        }
        #[cfg(test)]
        self.ai_pass_log.push(id);
        self.ai_update_one(id);
        self.ai_update_sleep(id)
    }

    /// The sleep `AIUpdateInterface::update` reports after its work (AIUpdate.cpp:1013-1028, :1035-1111):
    /// the least of the state machine's sleep, the deferred path request, and `doLocomotor`'s sleep. A
    /// machine that did not sleep, a dead unit and a dozer or worker (`DozerAIUpdate::update` and
    /// `WorkerAIUpdate::update` return `UPDATE_SLEEP_NONE`, DozerAIUpdate.cpp:1683, WorkerAIUpdate.cpp:316)
    /// report `UPDATE_SLEEP_NONE`. Each turret's sleep is min'd in (`updateTurretAI`, lane 157). Not yet ported: the
    /// jet's own `update` return, which reports `UPDATE_SLEEP_NONE`.
    pub(crate) fn ai_update_sleep(&self, id: ObjectId) -> u32 {
        let now = self.frame;
        let Some(o) = self.objects.get(&id) else { return UPDATE_SLEEP_FOREVER };
        // `AssaultTransportAIUpdate::update` returns `UPDATE_SLEEP_NONE` while alive (it "must not sleep", AssaultTransportAIUpdate.cpp:
        // 376-377) and `UPDATE_SLEEP_FOREVER` once effectively dead (:150-154).
        if o.template.assault_transport.is_some() && o.effectively_dead {
            return UPDATE_SLEEP_FOREVER;
        }
        if o.is_dying() || o.template.dozer.is_some() || self.is_worker(id) || o.template.supply_truck.is_some() || o.template.assault_transport.is_some() {
            return UPDATE_SLEEP_NONE;
        }
        // `DeployStyleAIUpdate::update` calls `AIUpdateInterface::update()` and then returns `UPDATE_SLEEP_NONE` ("We can't
        // sleep the deploy AI because any new commands need to be caught", DeployStyleAIUpdate.cpp:274-277): the parent's
        // sub-sleep is discarded, so a sentry drone is awake every frame (lane 195, AN3 s3 f2486 wake 2489 vs C++ 2487).
        // Only this return is ported; the module's deploy/undeploy state machine (DeployStyleAIUpdate.cpp:95-277) is not.
        if o.template.deploy_style_ai {
            return UPDATE_SLEEP_NONE;
        }
        // `AIUpdateInterface::update`: a state that sleeps (`STATE_SLEEP`) sleeps until its frame.
        let mut sleep = UPDATE_SLEEP_FOREVER;
        let machine = o.ai.machine.sleep_till;
        sleep = sleep.min(if machine > now { machine - now } else { UPDATE_SLEEP_NONE });
        // `TurretAI::updateTurretAI` (AIUpdate.cpp:1090-1098): each turret's return is min'd in. `update_turrets` has run
        // this frame and stored `sleep_until = now + its sub-sleep`, which is what `updateTurretAI` returns (`TurretAI.cpp:702`
        // and the sub-sleep at `:706-746`). A turret that does not run (dead or disabled, AIUpdate.cpp:1082-1090) adds
        // nothing. Before the turret port this returned `UPDATE_SLEEP_NONE`, which kept a tunnel network's AI awake every frame.
        if let Some(m) = o.combat.turret.as_ref() {
            if self.turret_runnable(id) {
                sleep = sleep.min(if m.sleep_until > now { m.sleep_until - now } else { UPDATE_SLEEP_NONE });
            }
        }
        // `m_queueForPathFrame` (AIUpdate.cpp:1064-1078).
        if o.ai.queue_for_path_frame > now {
            sleep = sleep.min(o.ai.queue_for_path_frame - now);
        }
        // `doLocomotor` (AIUpdate.cpp:2131-2132, :2305-2314): an immobile unit returns FOREVER, and a unit with no
        // locomotor returns NONE. Otherwise it sleeps forever only at rest: no goal, no final position, not
        // blocked, and an appearance whose maintain-position needs no calls (`locoUpdate_maintainCurrentPosition`,
        // Locomotor.cpp:2454-2490: legs, climbers, wheels, motorcycles and treads).
        if o.is_kind_of(crate::template::kind_of::IMMOBILE) {
            // `if (tmp < subMachineSleep) subMachineSleep = tmp` (AIUpdate.cpp:1114-1116): FOREVER only min's in.
            // `if (tmp < subMachineSleep) subMachineSleep = tmp` (AIUpdate.cpp:1114-1116): FOREVER only min's in.
            // `if (tmp < subMachineSleep) subMachineSleep = tmp` (AIUpdate.cpp:1114-1116): FOREVER only min's in.
            sleep = sleep.min(UPDATE_SLEEP_FOREVER);
        } else if let Some(loco) = self.locomotor_now(id) {
            let constant_calling = !matches!(
                loco.kind,
                crate::template::LocomotorKind::Legs
                    | crate::template::LocomotorKind::Wheels
                    | crate::template::LocomotorKind::Treads
                    | crate::template::LocomotorKind::Climber
            );
            let at_rest = o.ai.locomotor_goal_type == crate::ai::LocoGoalType::None
                && !o.ai.do_final_position
                && !o.ai.is_blocked
                && !constant_calling;
            sleep = sleep.min(if at_rest { UPDATE_SLEEP_FOREVER } else { UPDATE_SLEEP_NONE });
        } else {
            sleep = sleep.min(UPDATE_SLEEP_NONE);
        }
        sleep.max(UPDATE_SLEEP_NONE)
    }

    /// Run the AI of the objects that have no AI module (structures and the like that carry a weapon),
    /// in id order, after the heap pass.
    pub(crate) fn update_ai_state_machines(&mut self) {
        let ids: Vec<ObjectId> = self
            .objects()
            .filter(|o| Self::ai_pass_runs(o) && o.ai_handle.is_none() && !o.combat.weapons.is_empty())
            .map(|o| o.id)
            .collect();
        for id in ids {
            self.ai_update_one(id);
        }
    }

    /// One object's `AIUpdateInterface::update` (and the dozer/worker halves of the same module).
    fn ai_update_one(&mut self, id: ObjectId) {
        let now = self.frame;
        // The update module's wake frame: a freshly created object's first AI update is one frame
        // after creation (`AIUpdate.cpp:299-301`). After that the wake frame is 0. A dozer or worker's
        // module returns `UPDATE_SLEEP_NONE` (`DozerAIUpdate.cpp:1683`, `WorkerAIUpdate.cpp:316`), so it
        // runs every frame here: the AI's own machine keeps its sleep (`m_sleepTill`), not this gate.
        if let Some(o) = self.objects.get(&id) {
            if o.ai.wake_frame > now {
                return;
            }
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.wake_frame = 0;
        }
        // A dead AI runs only its death check and `AIDeadState` (`AIUpdate.cpp:1102`). It never
        // runs its old state or moves: the sim does not run states for dying units, and
        // `doLocomotor` skips a dead unit whose locomotor does not work when dead (`:2154`).
        // `AssaultTransportAIUpdate::update`'s dead branch (AssaultTransportAIUpdate.cpp:150-154) runs before anything else.
        if self.assault_dead_update(id) {
            return;
        }
        if self.objects.get(&id).is_some_and(|o| o.is_dying()) {
            self.ai_dead_update(id);
            return;
        }
        // `JetAIUpdate::update` runs before `AIUpdateInterface::update` (it calls it last).
        if self.objects.get(&id).is_some_and(|o| o.template.jet.is_some()) {
            self.jet_ai_update(id);
        }
        // The sub-update order of a supply truck's module, for the lane 137 F9 test.
        #[cfg(test)]
        let supply_module = self.objects.get(&id).is_some_and(|o| o.template.supply_truck.is_some());
        // `ChinookAIUpdate::update` (`ChinookAIUpdate.cpp:1078-1230`) does the helicopter's own work
        // (healing/pending command/landing/takeoff, the combat-drop phase) and then returns
        // `SupplyTruckAIUpdate::update()`, which runs the supply machine and then the base AI. So the
        // order is chinook work -> supply machine -> `AIUpdateInterface::update`. A plain supply truck
        // has no own body and runs `supply_ai_update_one` straight away.
        if self.objects.get(&id).is_some_and(|o| o.template.chinook.is_some()) {
            #[cfg(test)]
            if supply_module {
                self.ai_step_log.push(("chinook", id));
            }
            self.update_one_chinook(id);
        }
        if self.objects.get(&id).is_some_and(|o| o.template.supply_truck.is_some()) && !self.is_worker(id) {
            #[cfg(test)]
            self.ai_step_log.push(("supply", id));
            self.supply_ai_update_one(id);
        }
        #[cfg(test)]
        if supply_module {
            self.ai_step_log.push(("ai", id));
        }
        // `AssaultTransportAIUpdate::update` does its member work before `AIUpdateInterface::update` and returns without the base
        // update when the attack is pointless (AssaultTransportAIUpdate.cpp:251-254).
        if self.assault_update(id) {
            return;
        }
        // `AIUpdateInterface::update` (AIUpdate.cpp:1009-1114): run the machine, then the `m_movementComplete` block
        // (`:1035-1061`: a move that exited this update drops its path before anything moves), then the turret
        // state machines (`:1090-1098`), then the locomotor (`doLocomotor`, `:1114`).
        self.ai_machine_update(id);
        self.ai_end_completed_move(id);
        self.update_turrets(id);
        self.ai_do_locomotor(id);
        // The rest of the same module: `DozerAIUpdate::update` runs its own state machine right
        // after `AIUpdateInterface::update` (`DozerAIUpdate.cpp:1619`, `:1666`), and
        // `WorkerAIUpdate::update` its worker/supply and dozer machines (`WorkerAIUpdate.cpp:257`,
        // `:271-309`). One module, one wake frame, one point in the frame.
        self.dozer_module_update(id);
    }

    /// The dead-AI half of `AIUpdateInterface::update` (`AIUpdate.cpp:1102-1107`): a dead AI that is not
    /// in `AI_DEAD` clears its machine, enters `AI_DEAD` and locks it. Both `clear()` and `setState`
    /// are refused while the machine is locked (`StateMachine.cpp:346`, `:544`), so a locked machine (a
    /// dock move) keeps its state and is only locked again. From the next frame `AIDeadState::update`
    /// runs (`AIStates.cpp:1514`): the locomotor goal is cleared and an infantry's 2D velocity is
    /// scrubbed to 80% per frame.
    pub(crate) fn ai_dead_update(&mut self, id: ObjectId) {
        let Some(current) = self.objects.get(&id).map(|o| o.ai.machine.current) else { return };
        if current != AiStateType::Dead {
            self.ai_clear(id);
            self.ai_set_state(id, AiStateType::Dead);
            self.ai_set_machine_locked(id, true);
            // The move the unit was in ends with the transition: `AIInternalMoveToState::onExit` calls
            // `friend_endingMove` (AIStates.cpp:1747), and the same `AIUpdateInterface::update` drops the path
            // on `m_movementComplete` (AIUpdate.cpp:1035). A dead unit with a path would read as moving
            // (`isMoving`, AIUpdate.cpp:3194) and never be pushed by a squisher (`processCollision`, :1520).
            // The dead branch returns before the locomotor, so the completed-move block runs here, after the
            // transition. The flag is set for every dead transition: the path is dropped whatever move (if any)
            // the unit was in, as lane 98b's inline drop did.
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.movement_complete = true;
            }
            self.ai_end_completed_move(id);
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.move_goal = None;
            }
            return;
        }
        // `AIDeadState::update`.
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.locomotor_goal_type = LocoGoalType::None;
        }
        if self.objects.get(&id).is_some_and(|o| o.is_kind_of(kind_of::INFANTRY)) {
            // `phys->scrubVelocity2D(vel * 0.8)`: 0.8 ^ 30 == 0.012, so they stop within a second.
            let vel = self.objects.get(&id).and_then(|o| o.physics.as_ref()).map_or(0.0, |p| p.velocity_magnitude());
            self.scrub_velocity_2d(id, vel * 0.8);
        }
    }

    /// `StateMachine::updateStateMachine` (`StateMachine.cpp:412`) plus `AIStateMachine`'s temporary
    /// state handling (`AIStates.cpp:850`).
    pub(crate) fn ai_machine_update(&mut self, id: ObjectId) {
        let Some(m) = self.objects.get(&id).map(|o| o.ai.machine.clone()) else { return };
        // An attack order set directly on `ObjectCombat::attack` (e.g. `CleanupHazardUpdate`,
        // `CommandButtonHuntUpdate`) is adopted by the attack state, as `update_combat` used to run
        // it without a state transition. This must happen even while the machine is idle-sleeping
        // (the engine's `CleanupHazardUpdate` calls `aiAttackObject`, which wakes the AI).
        {
            let (has_order, is_object, force, target) = match self.objects.get(&id) {
                Some(o) => (
                    o.combat.attack.is_some(),
                    o.combat.attack.as_ref().is_none_or(|a| matches!(a.target, AttackTarget::Object(_))),
                    o.combat.force_attack,
                    o.combat.attack.as_ref().map(|a| a.target),
                ),
                None => return,
            };
            let current0 = self.objects.get(&id).map(|o| o.ai.machine.current).unwrap_or(AiStateType::Idle);
            let in_attack = matches!(
                current0,
                AiStateType::AttackObject | AiStateType::AttackPosition | AiStateType::ForceAttackObject | AiStateType::AttackAndFollowObject
            )
                // The guard machines own their attack order as a sub-state (`AIGuard.h`, `AIGuardRetaliate.h`);
                // C++ `getCurrentStateID()` stays `AI_GUARD` / `AI_GUARD_RETALIATE` while it runs, so
                // the order must not be promoted to a main `AI_ATTACK_*` state here.
                || matches!(current0, AiStateType::Guard | AiStateType::GuardRetaliate);
            if has_order && !in_attack && !current0.is_jet_state() {
                let vpos = match target {
                    Some(AttackTarget::Object(v)) => self.objects.get(&v).map_or(Coord3D::ZERO, |o| o.position),
                    _ => Coord3D::ZERO,
                };
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.attack_is_object = is_object;
                    o.ai.machine.attack_force = force;
                    o.ai.machine.attack_return = AiStateType::Idle;
                    match target {
                        Some(AttackTarget::Object(v)) => {
                            o.ai.machine.goal_object = v;
                            o.ai.machine.goal_position = vpos;
                        }
                        Some(AttackTarget::Ground(p)) => {
                            o.ai.machine.goal_object = ObjectId::INVALID;
                            o.ai.machine.goal_position = p;
                        }
                        None => {}
                    }
                }
                let state = if force {
                    AiStateType::ForceAttackObject
                } else if is_object {
                    AiStateType::AttackObject
                } else {
                    AiStateType::AttackPosition
                };
                self.ai_set_state(id, state);
                return;
            }
        }
        // `AIUpdateInterface`'s state-machine sleep: an idle unit is skipped entirely until its
        // wake frame, unless a temporary state is pending.
        if m.sleep_till != 0 && self.frame < m.sleep_till && m.temporary.is_none() {
            return;
        }
        // `AIStateMachine::updateStateMachine` (AIStates.cpp:850-884): a temporary state overrides the main machine and
        // runs on its own move fields (`enter_temp_fields`). A sleeping main is not woken by it.
        let temp = self.objects.get(&id).and_then(|o| o.ai.machine.temporary);
        if let Some(t) = temp {
            self.enter_temp_fields(id);
            let status = self.ai_state_update(id, t);
            let expired = self.frame > self.objects.get(&id).map_or(0, |o| o.ai.machine.temporary_end);
            let status = if expired && status == StateReturn::Continue { StateReturn::Success } else { status };
            if status == StateReturn::Continue {
                self.leave_temp_fields(id);
                return;
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.temporary = None;
            }
            self.ai_end_temporary_state(id, t);
        }
        // `StateMachine::updateStateMachine` (StateMachine.cpp:414-425): a main still sleeping when its temporary ends
        // stays asleep; `m_sleepTill` is cleared only when the main runs.
        let sleep_till = self.objects.get(&id).map_or(0, |o| o.ai.machine.sleep_till);
        if sleep_till != 0 && self.frame < sleep_till {
            return;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.sleep_till = 0;
        }
        let current = self.objects.get(&id).map(|o| o.ai.machine.current).unwrap_or(AiStateType::Idle);
        let status = self.ai_state_update(id, current);
        // A state's update may have changed the current state (set_state already ran onExit/onEnter);
        // if so, treat the status as Continue so the new state updates next frame (`StateMachine.cpp:452`).
        if self.objects.get(&id).map(|o| o.ai.machine.current) != Some(current) {
            return;
        }
        self.ai_check_transitions(id, current, status);
    }

    /// `State::friend_checkForTransitions` (`StateMachine.cpp:110`): follow the success/failure
    /// transition. Conditional transitions are done inside state updates in the sim.
    fn ai_check_transitions(&mut self, id: ObjectId, state: AiStateType, status: StateReturn) {
        match status {
            StateReturn::Continue => {}
            // The machine's own transitions are `internalSetState`, which ignores the lock.
            StateReturn::Success => {
                if state == AiStateType::MoveTo {
                    self.ai_note_drop_move_result(id, true);
                }
                let target = self.ai_success_state(id, state);
                self.ai_internal_set_state(id, target, 0);
            }
            StateReturn::Failure => {
                if state == AiStateType::MoveTo {
                    self.ai_note_drop_move_result(id, false);
                }
                let target = self.ai_failure_state(id, state);
                self.ai_internal_set_state(id, target, 0);
            }
            // `StateMachine::updateStateMachine` (`StateMachine.cpp:457`): a sleep state sets the
            // machine's wake frame. The sim keeps `AIUpdateInterface`'s state machine asleep until
            // then, so an idle unit costs nothing per frame; a new order clears it through `ai_set_state` / `ai_clear`
            // (the state change resets `m_sleepTill`, StateMachine.cpp:567), and `wake_up_now` only pulls the module.
            StateReturn::Sleep(frames) => {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.sleep_till = self.frame.saturating_add(frames);
                }
            }
        }
    }

    /// `ChinookMoveToBldgState` ends in the machine's transition (ChinookAIUpdate.cpp:853): the combat drop's move
    /// result is kept for the Chinook's update, and the drop's z gate ends with the move.
    fn ai_note_drop_move_result(&mut self, id: ObjectId, success: bool) {
        if let Some(o) = self.objects.get_mut(&id) {
            if o.ai.machine.drop_move_z.take().is_some() {
                o.ai.machine.drop_move_result = Some(success);
            }
        }
    }

    /// The `(success, failure)` targets of a state (`AIStates.cpp:685-728` plus the sub-machines'
    /// enclosing states).
    fn ai_success_state(&self, id: ObjectId, state: AiStateType) -> AiStateType {
        match state {
            AiStateType::AttackObject | AiStateType::AttackPosition | AiStateType::ForceAttackObject | AiStateType::AttackAndFollowObject => {
                self.objects.get(&id).map_or(AiStateType::Idle, |o| o.ai.machine.attack_return)
            }
            AiStateType::GuardInner | AiStateType::GuardAttackAggressor => AiStateType::GuardOuter,
            AiStateType::GuardOuter => AiStateType::GuardGetCrate,
            AiStateType::GuardReturn => AiStateType::GuardIdle,
            s if s.is_jet_state() => self.jet_transition(id, s, true),
            _ => AiStateType::Idle,
        }
    }

    fn ai_failure_state(&self, id: ObjectId, state: AiStateType) -> AiStateType {
        match state {
            AiStateType::AttackObject | AiStateType::AttackPosition | AiStateType::ForceAttackObject | AiStateType::AttackAndFollowObject => {
                self.objects.get(&id).map_or(AiStateType::Idle, |o| o.ai.machine.attack_return)
            }
            AiStateType::GuardInner | AiStateType::GuardAttackAggressor => AiStateType::GuardOuter,
            AiStateType::GuardOuter => AiStateType::GuardGetCrate,
            AiStateType::GuardReturn => AiStateType::GuardInner,
            s if s.is_jet_state() => self.jet_transition(id, s, false),
            _ => AiStateType::Idle,
        }
    }

    /// `StateMachine::setState` (`StateMachine.cpp:541`).
    pub(crate) fn ai_set_state(&mut self, id: ObjectId, new: AiStateType) {
        let before = self.objects.get(&id).map(|o| o.ai.machine.current);
        // `StateMachine::setState`: a locked machine ignores external state changes (`StateMachine.cpp:544`), but
        // `AIStateMachine::setState` still notifies when the requested ID differs from the current one (AIStates.cpp:1050-1058).
        if self.objects.get(&id).is_some_and(|o| o.ai.machine.locked) {
            if before != Some(new) {
                self.wake_up_now(id);
            }
            return;
        }
        self.ai_internal_set_state(id, new, 0);
        // A real change notifies the owner: `AIUpdateInterface::friend_notifyStateMachineChanged` ->
        // `wakeUpNow` (AIStates.cpp:1050-1058, AIUpdate.cpp:992-994, :973-988), which wakes the module for the next frame.
        if before != Some(new) {
            self.wake_up_now(id);
        }
    }

    /// `StateMachine::clear()` (`StateMachine.cpp:344`): run the current state's `onExit(EXIT_RESET)`
    /// *before* a command overwrites the machine goal, then `internalClear()` (goal object/position
    /// reset). The retail `private*` order functions call this first, so an exit that reads the goal
    /// (e.g. `AIDockState::onExit` -> `AIDockMachine::halt`) sees the old goal. A locked machine is
    /// not cleared (`StateMachine.cpp:346`). The sim has no null current state, so the cleared
    /// machine settles on `Idle`: a following [`ai_set_state`](Self::ai_set_state) then runs only the
    /// new state's `onEnter` (the `Idle` exit is a no-op), not a second exit with the new goal.
    pub(crate) fn ai_clear(&mut self, id: ObjectId) {
        if self.objects.get(&id).is_some_and(|o| o.ai.machine.locked) {
            // `StateMachine::clear` refuses a locked machine (StateMachine.cpp:346), but `AIStateMachine::clear` still drops
            // its goal path and waypoint and notifies the owner (AIStates.cpp:1017-1033), whatever the lock.
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.goal_path.clear();
                o.ai.machine.goal_waypoint = None;
            }
            self.wake_up_now(id);
            return;
        }
        // Called from a temporary state's own code: the main state's exit and reset use the main move's fields.
        if self.objects.get(&id).is_some_and(|o| o.ai.machine.temp_live) {
            return self.with_main_move_fields(id, |g| g.ai_clear(id));
        }
        let old = self.objects.get(&id).map_or(AiStateType::Idle, |o| o.ai.machine.current);
        // `onExit(EXIT_RESET)`; the state-specific exits ignore the exit type here.
        self.ai_state_exit(id, old, false);
        if let Some(o) = self.objects.get_mut(&id) {
            // `AIStateMachine::clear` (`AIStates.cpp:1017-1027`): the goal path, goal waypoint and goal squad
            // are dropped with the goal (the squad is not modelled).
            o.ai.machine.goal_path.clear();
            o.ai.machine.goal_waypoint = None;
            // `internalClear()`: the goal is dropped.
            o.ai.machine.goal_object = ObjectId::INVALID;
            o.ai.machine.goal_position = Coord3D::ZERO;
            o.ai.machine.current = AiStateType::Idle;
            o.ai.machine.sleep_till = 0;
        }
        // `AIStateMachine::clear` (AIStates.cpp:1017-1027) always notifies the owner: `friend_notifyStateMachineChanged`
        // -> `wakeUpNow`.
        self.wake_up_now(id);
    }

    fn ai_internal_set_state(&mut self, id: ObjectId, new: AiStateType, depth: u32) {
        if depth > 8 {
            return;
        }
        // A main state changed from a temporary state's own code (the mood conversion inside a dock move) gets the
        // main move's fields: its exit and entry write the main, and the temporary's set is parked until it returns.
        // A main state changed from a temporary state's own code (the mood conversion inside a dock move) gets the
        // main move's fields: its exit and entry write the main, and the temporary's set is parked until it returns.
        // A main state changed from a temporary state's own code (the mood conversion inside a dock move) gets the
        // main move's fields: its exit and entry write the main, and the temporary's set is parked until it returns.
        if self.objects.get(&id).is_some_and(|o| o.ai.machine.temp_live) {
            return self.with_main_move_fields(id, |g| g.ai_internal_set_state(id, new, depth));
        }
        let old = self.objects.get(&id).map(|o| o.ai.machine.current);
        if let Some(old) = old {
            self.ai_state_exit(id, old, true);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            // `AIStateMachine::setState` notifies the AI (`friend_notifyStateMachineChanged`, which wakes it) only
            // when the state changes (`oldID != newStateID`, AIStates.cpp:1051-1058). A same-state re-entry keeps
            // the sleep.
            if old != Some(new) {
                o.ai.machine.sleep_till = 0;
            }
            o.ai.machine.current = new;
            // The saved attack-move goal only matters until the machine is back in the attack-move
            // after an attack; any other state drops it.
            if !matches!(
                new,
                AiStateType::AttackMoveTo
                    | AiStateType::AttackObject
                    | AiStateType::AttackPosition
                    | AiStateType::ForceAttackObject
                    | AiStateType::AttackAndFollowObject
            ) {
                o.ai.machine.attack_move_resume = false;
            }
            // The attack-follow resume marker only survives the attack the follow state itself started.
            if !matches!(
                new,
                AiStateType::AttackFollowWaypointPath
                    | AiStateType::AttackObject
                    | AiStateType::AttackPosition
                    | AiStateType::ForceAttackObject
                    | AiStateType::AttackAndFollowObject
            ) {
                o.ai.machine.attack_follow_resume = false;
            }
        }
        let ret = self.ai_state_enter(id, new);
        // `internalSetState` then runs the enter result through the transition check; if the enter
        // changed the state, skip (`StateMachine.cpp:635`).
        if self.objects.get(&id).map(|o| o.ai.machine.current) == Some(new) && ret != StateReturn::Continue {
            if new == AiStateType::MoveTo {
                // An enter that ends the move (the combat drop's move, ChinookAIUpdate.cpp:853) is its result too.
                self.ai_note_drop_move_result(id, ret == StateReturn::Success);
            }
            let target = if new.is_jet_state() {
                self.jet_transition(id, new, ret == StateReturn::Success)
            } else {
                let (succ, fail) = state_transition(new);
                if ret == StateReturn::Success { succ } else { fail }
            };
            self.ai_internal_set_state(id, target, depth + 1);
        }
    }

    /// `StateMachine::setTemporaryState` (`AIStates.cpp:919`).
    pub(crate) fn ai_set_temporary_state(&mut self, id: ObjectId, state: AiStateType, frames: u32) {
        let old = self.objects.get(&id).and_then(|o| o.ai.machine.temporary);
        if let Some(old) = old {
            self.ai_end_temporary_state(id, old);
        }
        // `setTemporaryState` (AIStates.cpp:919) takes `internalGetState(newStateID)` (StateMachine.cpp:512-532), one state
        // object per ID. A temporary of the main state's own ID is that same object: its enter writes the main move's fields
        // in place, and they stay so after the temporary ends, because the main resumes without an enter (StateMachine.cpp,
        // `updateStateMachine`; AIStates.cpp:854-870). So the main's set is not parked for it. A temporary of another ID is
        // its own state object: it starts from the main move's current fields and the main's set is parked while its code
        // runs (see `MoveStateFields`). Lane 190: the transport's HeadOffMap exit is a same-ID temporary `AI_MOVE_TO`
        // (DeliverPayloadAIUpdate.cpp:1131), so its goal survives the approach's resumption.
        let shared = self.objects.get(&id).is_some_and(|o| o.ai.machine.current == state);
        if shared {
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.parked_move = None;
                o.ai.machine.temporary = Some(state);
                o.ai.machine.temporary_end = self.frame + frames.min(60 * LOGICFRAMES_PER_SECOND);
            }
        } else {
            self.leave_temp_fields(id);
            let main = self.capture_move_fields(id);
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.parked_move = main;
                o.ai.machine.temporary = Some(state);
                o.ai.machine.temporary_end = self.frame + frames.min(60 * LOGICFRAMES_PER_SECOND);
            }
            self.enter_temp_fields(id);
        }
        let ret = self.ai_state_enter(id, state);
        // `setTemporaryState` (AIStates.cpp:951-955): a non-CONTINUE enter exits the state and leaves no temporary.
        if ret != StateReturn::Continue {
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.temporary = None;
            }
            self.ai_end_temporary_state(id, state);
            return;
        }
        if !shared {
            self.leave_temp_fields(id);
        }
    }

    /// A temporary state ends (`AIStateMachine::updateStateMachine`, `AIStates.cpp:854-857`, or
    /// `setTemporaryState` replacing it, `:919-925`): its exit runs on its own fields, then the main move's fields are
    /// live again and the temporary's set is dropped.
    pub(crate) fn ai_end_temporary_state(&mut self, id: ObjectId, state: AiStateType) {
        self.enter_temp_fields(id);
        self.ai_state_exit(id, state, false);
        self.drop_temp_fields(id);
    }

    /// The live move-state fields of `id` (see [`MoveStateFields`]).
    fn capture_move_fields(&self, id: ObjectId) -> Option<MoveStateFields> {
        self.objects.get(&id).map(|o| MoveStateFields {
            move_goal: o.ai.move_goal,
            path_goal_position: o.ai.path_goal_position,
            move_waiting_for_path: o.ai.move_waiting_for_path,
            move_path_timestamp: o.ai.move_path_timestamp,
            blocked_repath_timestamp: o.ai.blocked_repath_timestamp,
            try_one_more_repath: o.ai.try_one_more_repath,
            adjusts_destination: o.ai.adjusts_destination,
            machine_try_one_more_repath: o.ai.machine.move_try_one_more_repath,
            machine_move_adjusts: o.ai.machine.move_adjusts,
        })
    }

    /// Make `s` the live move-state fields of `id`.
    fn apply_move_fields(&mut self, id: ObjectId, s: MoveStateFields) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.move_goal = s.move_goal;
            o.ai.path_goal_position = s.path_goal_position;
            o.ai.move_waiting_for_path = s.move_waiting_for_path;
            o.ai.move_path_timestamp = s.move_path_timestamp;
            o.ai.blocked_repath_timestamp = s.blocked_repath_timestamp;
            o.ai.try_one_more_repath = s.try_one_more_repath;
            o.ai.adjusts_destination = s.adjusts_destination;
            o.ai.machine.move_try_one_more_repath = s.machine_try_one_more_repath;
            o.ai.machine.move_adjusts = s.machine_move_adjusts;
        }
    }

    /// Make the temporary state's set the live one: the main move's live set is parked (`AiMachine::parked_move`).
    /// A unit with no temporary state, or whose temporary code already runs, is unchanged.
    fn enter_temp_fields(&mut self, id: ObjectId) {
        let Some(main) = self.capture_move_fields(id) else { return };
        let Some(o) = self.objects.get(&id) else { return };
        if o.ai.machine.temp_live {
            return;
        }
        let Some(temp) = o.ai.machine.parked_move else { return };
        self.apply_move_fields(id, temp);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.parked_move = Some(main);
            o.ai.machine.temp_live = true;
        }
    }

    /// The temporary state's code is done: the main move's set is live again and the temporary's is parked
    /// (the inverse of [`enter_temp_fields`](Self::enter_temp_fields)).
    fn leave_temp_fields(&mut self, id: ObjectId) {
        let Some(temp) = self.capture_move_fields(id) else { return };
        let Some(o) = self.objects.get(&id) else { return };
        if !o.ai.machine.temp_live {
            return;
        }
        let main = o.ai.machine.parked_move.unwrap_or(temp);
        self.apply_move_fields(id, main);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.parked_move = Some(temp);
            o.ai.machine.temp_live = false;
        }
    }

    /// The temporary state has ended: the main move's set is live and the temporary's set is discarded.
    fn drop_temp_fields(&mut self, id: ObjectId) {
        self.leave_temp_fields(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.parked_move = None;
        }
    }

    /// Run `f`, a write of the main state's move fields, while a temporary state's code runs: the main's set is live
    /// for `f` and the temporary's is put back afterwards. C++ keeps the temporary and the main as separate state
    /// objects, so a main state entered during a temporary (the mood conversion's `aiAttackMoveToPosition` inside a
    /// dock `AI_MOVE_TO`, `AIStates.cpp:2087`, through `privateAttackMoveToPosition`, `AIUpdate.cpp:3595-3615`) runs its
    /// own `onEnter` on its own fields, and those writes survive the temporary's end. Between the temporary's runs the
    /// main's set is already live, so this is a plain call there.
    pub(crate) fn with_main_move_fields<R>(&mut self, id: ObjectId, f: impl FnOnce(&mut Self) -> R) -> R {
        if !self.objects.get(&id).is_some_and(|o| o.ai.machine.temp_live) {
            return f(self);
        }
        self.leave_temp_fields(id);
        let r = f(self);
        if self.objects.get(&id).is_some_and(|o| o.ai.machine.temporary.is_some()) {
            self.enter_temp_fields(id);
        } else if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.parked_move = None;
        }
        r
    }

    // ---- per-state dispatch -----------------------------------------------------------------

    fn ai_state_enter(&mut self, id: ObjectId, state: AiStateType) -> StateReturn {
        match state {
            AiStateType::Idle => self.ai_idle_enter(id),
            AiStateType::MoveTo => self.ai_move_to_enter(id),
            AiStateType::MoveOutOfTheWay => self.ai_move_out_of_way_enter(id),
            AiStateType::MoveAndTighten => self.ai_move_and_tighten_enter(id),
            AiStateType::AttackMoveTo => self.ai_attack_move_enter(id),
            AiStateType::AttackObject | AiStateType::AttackPosition | AiStateType::ForceAttackObject | AiStateType::AttackAndFollowObject => {
                self.ai_attack_enter(id, state)
            }
            AiStateType::Hunt | AiStateType::AttackArea => self.ai_hunt_enter(id, state),
            AiStateType::Guard => self.ai_guard_enter(id),
            AiStateType::GuardRetaliate => self.ai_guard_retaliate_enter(id),
            AiStateType::Enter => self.ai_enter_enter(id),
            AiStateType::Exit | AiStateType::ExitInstantly => self.ai_exit_enter(id),
            AiStateType::FaceObject | AiStateType::FacePosition => self.ai_face_enter(id),
            AiStateType::Busy | AiStateType::Wait | AiStateType::Dead | AiStateType::Wander => StateReturn::Continue,
            AiStateType::Dock => self.ai_dock_enter(id),
            AiStateType::GuardInner => self.ai_guard_inner_enter(id),
            AiStateType::GuardOuter => self.ai_guard_outer_enter(id),
            AiStateType::GuardReturn => self.ai_guard_return_enter(id),
            AiStateType::GuardIdle => self.ai_guard_idle_enter(id),
            AiStateType::GuardGetCrate => StateReturn::Success,
            AiStateType::GuardAttackAggressor => self.ai_guard_aggressor_enter(id),
            AiStateType::FollowPath => self.ai_follow_path_enter(id),
            AiStateType::FollowWaypointPath => self.ai_follow_waypoint_path_enter(id),
            AiStateType::FollowExitProductionPath => self.ai_follow_exit_production_path_enter(id),
            AiStateType::AttackFollowWaypointPath => self.ai_attack_follow_enter(id),
            AiStateType::MoveAndEvacuate | AiStateType::MoveAndEvacuateAndExit => self.ai_move_and_evacuate_enter(id),
            AiStateType::MoveAndDelete => self.ai_move_and_delete_enter(id),
            AiStateType::WanderInPlace => self.ai_wander_in_place_enter(id),
            AiStateType::Panic => self.ai_panic_enter(id),
            AiStateType::AttackSquad => StateReturn::Success,
            s if s.is_jet_state() => self.jet_state_enter(id, s),
            _ => StateReturn::Success,
        }
    }

    fn ai_state_exit(&mut self, id: ObjectId, state: AiStateType, _normal: bool) {
        match state {
            AiStateType::MoveTo | AiStateType::MoveAndTighten => {
                self.ai_internal_move_exit(id);
            }
            AiStateType::AttackMoveTo => self.ai_attack_move_exit(id),
            AiStateType::MoveOutOfTheWay => {
                // `AIMoveOutOfTheWayState::onExit` (`AIStates.cpp:2188-2196`): the base exit, then
                // `destroyPath(); setCanPathThroughUnits(false); clearMoveOutOfWay();`. The port kept
                // both avoided ids recorded and the flag set across the state's exit.
                self.ai_internal_move_exit(id);
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.path.clear();
                    o.ai.raw_path = None;
                    // `destroyPath` also clears `m_isAttackPath` (AIUpdate.cpp:2026).
                    o.ai.attack_path_active = false;
                    o.ai.can_path_through_units = false;
                    o.ai.move_out_of_way = [ObjectId::INVALID, ObjectId::INVALID];
                }
            }
            AiStateType::AttackObject | AiStateType::AttackPosition | AiStateType::ForceAttackObject | AiStateType::AttackAndFollowObject => {
                self.ai_attack_exit(id);
            }
            AiStateType::Enter => self.ai_enter_exit(id),
            AiStateType::Exit | AiStateType::ExitInstantly => self.ai_exit_exit(id),
            AiStateType::Guard | AiStateType::GuardInner | AiStateType::GuardOuter | AiStateType::GuardReturn | AiStateType::GuardAttackAggressor => {
                self.ai_guard_state_exit(id, state);
            }
            AiStateType::GuardRetaliate => self.ai_guard_retaliate_exit(id),
            AiStateType::AttackFollowWaypointPath => self.ai_attack_follow_exit(id),
            AiStateType::FollowPath
            | AiStateType::FollowExitProductionPath
            | AiStateType::FollowWaypointPath
            | AiStateType::MoveAndEvacuate
            | AiStateType::MoveAndEvacuateAndExit
            | AiStateType::MoveAndDelete
            | AiStateType::WanderInPlace
            | AiStateType::Panic => {
                self.ai_internal_move_exit(id);
            }
            AiStateType::Dock => self.ai_dock_exit(id),
            // `AIHuntState::onExit` (`AIStates.cpp:7169`): the hunt ends the temporary weapon lock.
            AiStateType::Hunt => self.release_weapon_lock(id, crate::weapon::WeaponLockType::Temporarily),
            s if s.is_jet_state() => {
                // `JetOrHeliTaxiState` extends `AIMoveOutOfTheWayState` extends `AIInternalMoveToState`
                // (`JetAIUpdate.cpp:475`), so the taxi's exit runs the move state's `onExit`
                // (`friend_endingMove`, `AIStates.cpp:1747`, via `AIMoveOutOfTheWayState::onExit`
                // `:2188-2196`) before the taxi cleanup. The same update's completed-move block then
                // drops the path and clears the locomotor goal (`AIUpdate.cpp:1035-1041`), which is what
                // stops a landed/taxiing jet dead (`locoUpdate_maintainCurrentPosition`) instead of
                // letting it coast to a stop.
                if matches!(s, AiStateType::JetTaxiFromHangar | AiStateType::JetTaxiToTakeoff | AiStateType::JetTaxiFromLanding) {
                    self.ai_internal_move_exit(id);
                }
                self.jet_state_exit(id, s)
            }
            _ => {}
        }
    }

    pub(crate) fn ai_state_update(&mut self, id: ObjectId, state: AiStateType) -> StateReturn {
        match state {
            AiStateType::Idle => self.ai_idle_update(id),
            AiStateType::MoveTo => {
                // `ChinookMoveToBldgState::update` (ChinookAIUpdate.cpp:766-776): the status is CONTINUE while the Chinook is
                // more than 3 from its drop height, whatever the move returned (success or failure). The arrival test has
                // its own copy of this gate (it must not finish the move early); this covers the other returns, such as a
                // failed path search.
                let ret = self.ai_move_to_update(id);
                let z_wait = self.objects.get(&id).is_some_and(|o| o.ai.machine.drop_move_z.is_some_and(|dz| (o.position.z - dz).abs() > 3.0));
                if ret != StateReturn::Continue && z_wait {
                    StateReturn::Continue
                } else {
                    ret
                }
            }
            AiStateType::MoveOutOfTheWay | AiStateType::MoveAndTighten => self.ai_internal_move_update(id),
            AiStateType::AttackMoveTo => self.ai_attack_move_update(id),
            AiStateType::AttackObject | AiStateType::AttackPosition | AiStateType::ForceAttackObject | AiStateType::AttackAndFollowObject => {
                self.ai_attack_update(id)
            }
            AiStateType::Hunt => self.ai_hunt_update(id),
            AiStateType::AttackArea => self.ai_attack_area_update(id),
            AiStateType::Guard => self.ai_guard_update(id),
            AiStateType::GuardRetaliate => self.ai_guard_retaliate_update(id),
            AiStateType::Enter => self.ai_enter_update(id),
            AiStateType::Exit | AiStateType::ExitInstantly => self.ai_exit_update(id),
            AiStateType::FaceObject | AiStateType::FacePosition => self.ai_face_update(id),
            AiStateType::Busy | AiStateType::Wait => StateReturn::Continue,
            AiStateType::Dead => StateReturn::Continue,
            AiStateType::Wander => StateReturn::Success,
            AiStateType::Dock => self.ai_dock_update(id),
            AiStateType::GuardInner => self.ai_guard_inner_update(id),
            AiStateType::GuardOuter => self.ai_guard_outer_update(id),
            AiStateType::GuardReturn => self.ai_guard_return_update(id),
            AiStateType::GuardIdle => self.ai_guard_idle_update(id),
            AiStateType::GuardGetCrate => StateReturn::Success,
            AiStateType::GuardAttackAggressor => self.ai_guard_aggressor_update(id),
            AiStateType::FollowPath | AiStateType::FollowExitProductionPath => self.ai_follow_path_update(id),
            AiStateType::FollowWaypointPath => self.ai_follow_waypoint_path_update(id),
            AiStateType::AttackFollowWaypointPath => self.ai_attack_follow_update(id),
            AiStateType::MoveAndEvacuate => self.ai_move_and_evacuate_update(id),
            AiStateType::MoveAndEvacuateAndExit => self.ai_move_and_evacuate_and_exit_update(id),
            AiStateType::MoveAndDelete => self.ai_move_and_delete_update(id),
            AiStateType::WanderInPlace => self.ai_wander_in_place_update(id),
            AiStateType::Panic => self.ai_follow_path_update(id),
            AiStateType::AttackSquad => StateReturn::Success,
            s if s.is_jet_state() => self.jet_state_update(id, s),
            _ => StateReturn::Success,
        }
    }

    // Seams for `crate::jet_states` (the jet states reuse the move / follow-path / move-out-of-the-way states).
    pub(crate) fn ai_internal_move_update_pub(&mut self, id: ObjectId) -> StateReturn {
        self.ai_internal_move_update(id)
    }
    pub(crate) fn ai_internal_move_enter_pub(&mut self, id: ObjectId, goal: Coord3D, adjusts: bool) -> StateReturn {
        self.ai_internal_move_enter(id, goal, adjusts)
    }
    pub(crate) fn ai_move_out_of_way_enter_pub(&mut self, id: ObjectId) -> StateReturn {
        self.ai_move_out_of_way_enter(id)
    }
    pub(crate) fn ai_follow_path_enter_pub(&mut self, id: ObjectId) -> StateReturn {
        self.ai_follow_path_enter(id)
    }
    pub(crate) fn ai_follow_path_update_pub(&mut self, id: ObjectId) -> StateReturn {
        self.ai_follow_path_update(id)
    }
    /// `AIFollowPathState::onExit` (`AIStates.cpp:3324`).
    pub(crate) fn ai_follow_path_exit_pub(&mut self, id: ObjectId) {
        self.ai_internal_move_exit(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.can_path_through_units = false;
        }
    }
}

// ---- shared helpers ---------------------------------------------------------------------------

impl GameLogic {
    /// `Object::isMobile` (`Object.cpp:2947`): not `KINDOF_IMMOBILE` and not disabled (any type). The
    /// port's locomotor requirement stands in for the engine's locomotor-set surface check.
    pub(crate) fn is_mobile(&self, id: ObjectId) -> bool {
        self.objects
            .get(&id)
            .is_some_and(|o| o.template.locomotor.is_some() && !o.is_kind_of(kind_of::IMMOBILE) && !o.is_disabled())
    }

    /// `AIUpdateInterface::requestPath` (`AIUpdate.cpp:479-517`): record the destination and either build
    /// the quick path on the spot (air) or queue on the pathfinder. `requestPath`'s order is the whole point:
    /// the quick path is checked **first** (`:493-497`, before `m_waitingForPath = TRUE` at `:498`), and only
    /// the queued case reaches the anti-spin `setQueueForPathTime` guard (`:499-514`).
    pub(crate) fn ai_request_path(&mut self, id: ObjectId, goal: Coord3D, adjusts: bool) {
        // `requestPath` reads the unit's blocked-and-stuck state before its own `start_move`, which clears it (the
        // port's fold of `friend_startingMove`). A move's `onEnter` already cleared it at `AIStates.cpp:1635`, and an
        // update's repath (`:1884`) still has it set when `requestPath` runs (`computePath`'s clear is at `:1607`,
        // after it), so capture the flags first (`:506`).
        let (group_dest, stuck_before, had_path_before) = match self.objects.get(&id) {
            Some(o) => (o.ai.machine.group_dest, o.ai.is_blocked_and_stuck, o.ai.has_path()),
            None => (None, false, false),
        };
        // `requestPath` (AIUpdate.cpp:499-516): a path computed within the last three frames makes this request wait
        // one second (`setQueueForPathTime`) and skip the pathfinder queue. `frame-3` wraps before frame 3 in C++.
        let frame = self.frame;
        let recent = self.objects.get(&id).is_some_and(|o| frame >= 3 && o.ai.path_timestamp > frame - 3);
        if let Some(o) = self.objects.get_mut(&id) {
            // `requestPath` sets `m_isAttackPath = FALSE` (AIUpdate.cpp:490).
            o.ai.attack_path_active = false;
            o.ai.start_move(goal, crate::ai::PathRequest::Move, adjusts);
            // The destination was adjusted when the move began (`ai_adjust_move_goal`, AIStates.cpp:1661), and a
            // request never adjusts again: `requestPath` hands `findPath` the goal alone (AIUpdate.cpp:479-516,
            // :1726), and `checkDestination` ignores the unit's own goal. `start_move` keeps `is_final_goal`.
            o.ai.adjusts_destination = false;
            o.ai.machine.move_adjusts = adjusts;
            o.ai.group_dest = group_dest;
            // `AIInternalMoveToState::computePath` (`AIStates.cpp:1604`): the state owns the flag and sets it
            // before `requestPath`.
            o.ai.move_waiting_for_path = true;
        }
        // `AIUpdateInterface::requestPath` (`AIUpdate.cpp:493-497`): "if it is possible to do it
        // without having to use the pathfinder (air units just move point to point) it generates the
        // path immediately" — `computeQuickPath` runs inside the call, *before* the anti-spin guard,
        // and `m_waitingForPath` is never set for it (`:498` is not reached; `destroyPath` clears it), so
        // the move state's update in this same frame arms the locomotor (`AIStates.cpp:1875-1876`) and
        // `doLocomotor` drives toward the goal. The port queued the request like a ground path, so an air
        // move spent one frame with the goal type `NONE` (a hovering Chinook braked instead of turning:
        // AN3 s1 f1620).
        if self.can_compute_quick_path(id) {
            self.compute_quick_path(id, &goal);
            if let Some(o) = self.objects.get_mut(&id) {
                // `computeQuickPath`'s `destroyPath` leaves `m_waitingForPath` FALSE (`AIUpdate.cpp:2024`), and
                // the early "already going there" return leaves the request's flag alone (it was FALSE: this
                // branch precedes `:498`).
                o.ai.waiting_for_path = false;
                // The group destination is spent by the request that consumed it (`AIGroup::computeIndividualDestination`
                // applies the offset once, at order time, `AIGroup.cpp:542-549`; the engine has no field to keep).
                // `doPathfind` clears the port's copy for a queued path; a quick path never reaches `doPathfind`,
                // so clear it here or a later queued path of the same unit would tighten toward it.
                o.ai.group_dest = None;
                o.ai.machine.group_dest = None;
            }
            return;
        }
        if recent {
            // `setQueueForPathTime(LOGICFRAMES_PER_SECOND)` (`:503`) wakes a module asleep past that (AIUpdate.cpp:955-968).
            self.wake_up_for_queue_time(id, LOGICFRAMES_PER_SECOND);
            if let Some(o) = self.objects.get_mut(&id) {
                // `setQueueForPathTime(LOGICFRAMES_PER_SECOND)` (`:503`), then "See if it has been too soon"
                // (`:505-510`): `if (m_path && m_isBlockedAndStuck) { setIgnoreCollisionTime(2 seconds);
                // m_blockedFrames = 0; m_isBlocked = FALSE; m_isBlockedAndStuck = FALSE; }`. Every caller of
                // `requestPath` runs this, the move's `onEnter` included.
                o.ai.queue_for_path_frame = frame + LOGICFRAMES_PER_SECOND;
                if had_path_before && stuck_before {
                    o.ai.ignore_collisions_until = frame + 2 * LOGICFRAMES_PER_SECOND;
                    o.ai.blocked_frames = 0;
                    o.ai.is_blocked = false;
                    o.ai.is_blocked_and_stuck = false;
                }
            }
            return;
        }
        self.pathfinder.queue_for_path(id);
    }

    /// `AIUpdateInterface::aiMoveToObject` (`AI.h:485` -> `aiDoCommand` `AIUpdate.cpp:2660` ->
    /// `privateMoveToObject`, `AIUpdate.cpp:2927`): clear the machine, make `target` its goal object and
    /// enter `AI_MOVE_TO`. `StateMachine::internalSetGoalObject` (`StateMachine.cpp:766`) copies the
    /// object's position into the machine goal; a null object (`ObjectId::INVALID` here) leaves the
    /// goal at the zeroed position `internalClear` gives it (`StateMachine.cpp:326`). The dead do not
    /// listen (`aiDoCommand` -> `isAllowedToRespondToAiCommands`, `AIUpdate.cpp:2597`).
    pub(crate) fn ai_move_to_object_cmd(&mut self, id: ObjectId, target: ObjectId, source: CommandSource) {
        // The default branch of `AssaultTransportAIUpdate::aiDoCommand` (AssaultTransportAIUpdate.cpp:112-115).
        self.assault_command(id, crate::assault_transport::AssaultCommand::Other, source);
        if !self.is_mobile(id) || self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return;
        }
        // `privateMoveToObject` (AIUpdate.cpp:2942) clears; a grounded jet's default branch does (see `jet_base_command_clears`).
        if self.jet_base_command_clears(id) {
            self.ai_clear(id);
        }
        let target_pos = self.objects.get(&target).map(|t| t.position);
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack = None;
            o.combat.attack_move = false;
            o.combat.guard = None;
            o.combat.hunt = false;
            o.ai.machine.goal_object = if target_pos.is_some() { target } else { ObjectId::INVALID };
            o.ai.machine.goal_position = target_pos.unwrap_or(Coord3D::ZERO);
            o.ai.machine.group_dest = None;
            o.ai.machine.last_command_source = source;
        }
        self.ai_command_set_state(id, AiStateType::MoveTo);
    }

    /// `AIUpdateInterface::privateMoveToPosition` (`AIUpdate.cpp:2889`), the move behind
    /// `aiMoveToPosition`. A `CMD_FROM_AI` move to a non-idle AI leaves the main machine alone: it
    /// sets the goal and runs as the 20-second temporary `AI_MOVE_TO` (`AIUpdate.cpp:2909`), and the
    /// main state resumes when the temporary state ends. Every other move clears the machine and
    /// enters `AI_MOVE_TO`. A non-idle `AI_MOVE_TO` main state keeps its own path until then.
    pub(crate) fn ai_private_move_to_position(&mut self, id: ObjectId, pos: Coord3D, source: CommandSource) {
        if !self.is_mobile(id) {
            return;
        }
        let idle = self.objects.get(&id).is_some_and(|o| o.ai.machine.current == AiStateType::Idle);
        if !idle && source == CommandSource::Ai {
            // `setGoalPositionClipped` (`AIUpdate.cpp:2905`), then the blocked flags (`:2906-2908`).
            self.ai_set_goal_position(id, pos);
            self.ai_reset_blocked(id);
            // `setTemporaryState(AI_MOVE_TO, LOGICFRAMES_PER_SECOND * 20)` (`AIUpdate.cpp:2909`).
            self.ai_set_temporary_state(id, AiStateType::MoveTo, 20 * LOGICFRAMES_PER_SECOND);
            return;
        }
        // `getStateMachine()->clear()` (`AIUpdate.cpp:2912`), the goal (`:2913`), the blocked flags
        // (`:2914-2916`) and the command source (`:2917`), then `setState(AI_MOVE_TO)` (`:2918`).
        self.ai_clear(id);
        self.ai_set_goal_position(id, pos);
        self.ai_reset_blocked(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.last_command_source = source;
        }
        self.ai_command_set_state(id, AiStateType::MoveTo);
    }

    /// `StateMachine::setGoalPosition` for the AI machine: the goal position only, the goal object is
    /// untouched (`AIUpdate.cpp:2905`).
    fn ai_set_goal_position(&mut self, id: ObjectId, pos: Coord3D) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.goal_position = pos;
        }
    }

    /// `m_blockedFrames = 0; m_isBlocked = FALSE; m_isBlockedAndStuck = FALSE;` (`AIUpdate.cpp:2906`).
    fn ai_reset_blocked(&mut self, id: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.blocked_frames = 0;
            o.ai.is_blocked = false;
            o.ai.is_blocked_and_stuck = false;
        }
    }

    /// `AIUpdateInterface::aiIdle` (`AIUpdate.cpp:3092`): clear the order and go to `AI_IDLE`.
    pub fn ai_idle(&mut self, id: ObjectId, source: CommandSource) {
        if self.objects.get(&id).is_some_and(|o| o.is_kind_of(kind_of::PROJECTILE)) {
            return;
        }
        // `AssaultTransportAIUpdate::aiDoCommand`'s AICMD_IDLE branch for a command not from the AI (AssaultTransportAIUpdate.cpp:101-106).
        self.assault_command(id, crate::assault_transport::AssaultCommand::Idle, source);
        // `SupplyTruckAIUpdate::privateIdle` (`SupplyTruckAIUpdate.cpp:193-200`): "If the user gives a
        // stop command, I have to turn off autopilot", i.e. it latches the supply machine's Busy state
        // before the base `privateIdle` runs. `WorkerAIUpdate` overrides `privateIdle` with the latch
        // commented out (`WorkerAIUpdate.cpp:531-540`: "the worker's dozer brain will get completely
        // screwed"), so a worker must not latch it. The port's worker templates carry the supply data
        // too (`adapter.rs` maps `WorkerAIUpdate` into `supply_truck`), hence the `!is_worker` test;
        // a chinook does latch it (`ChinookAIUpdate` has no `privateIdle` override).
        if source == CommandSource::Player {
            let worker = self.is_worker(id);
            if let Some(o) = self.objects.get_mut(&id) {
                if o.template.supply_truck.is_some() && !worker {
                    o.supply_ai.force_busy = true;
                }
            }
        }
        // `privateIdle` (`AIUpdate.cpp:3097`) clears the machine first; a jet overrides `aiDoCommand`
        // and owns its own transitions, so only the plain `AIUpdateInterface` clears. The clear runs the
        // current attack's exit (`AIAttackState::onExit`, `AIStates.cpp:5714`: `end_attack` cancels the
        // pre-attack, stops the path and wakes the AI), so the combat fields are dropped only after it.
        if !self.is_jet(id) {
            self.ai_clear(id);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack = None;
            o.combat.attack_move = false;
            o.combat.guard = None;
            o.combat.hunt = false;
            o.combat.hunt_area = None;
            o.ai.machine.last_command_source = source;
        }
        self.ai_command_set_state(id, AiStateType::Idle);
        // `privateIdle`'s passenger loop (AIUpdate.cpp:3108-3116): every contained object's AI is idled too.
        let passengers = self.objects.get(&id).map(|o| o.contained.clone()).unwrap_or_default();
        for p in passengers {
            if self.objects.get(&p).is_some_and(|o| o.ai_handle.is_some()) {
                self.ai_idle(p, source);
            }
        }
    }

    /// `AIUpdateInterface::aiBusy` (`AI.h` -> `AICMD_BUSY` `AIUpdate.cpp:2869` -> `privateBusy`,
    /// `:4054`): clear the machine and enter `AI_BUSY`, which never leaves on its own
    /// (`AIBusyState::update`, `AIStateMachine.h:344`).
    pub(crate) fn ai_busy(&mut self, id: ObjectId, source: CommandSource) {
        if self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return;
        }
        // `privateBusy` (`AIUpdate.cpp:4054-4060`) clears the machine for every object, jets included:
        // `JetAIUpdate::aiDoCommand` lets `AICMD_BUSY` through without taking off (`JetAIUpdate.cpp:
        // 2595-2601`) and reaches `AIUpdateInterface::aiDoCommand` -> `privateBusy` like any unit.
        self.ai_clear(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.last_command_source = source;
        }
        self.ai_command_set_state(id, AiStateType::Busy);
    }

    /// `AIUpdateInterface::joinTeam` (`AIUpdate.cpp:2536-2587`), run by `AIPlayer::checkReadyTeams`
    /// when a *reinforcement* entry starts (`AIPlayer.cpp:2900-2904`): a mobile unit joins the rest of
    /// its team — the first other member with an AI that is not `DISABLED_HELD` (in `Team::m_TeamMemberList`
    /// order, newest first). If that member is idle the unit is sent to its position
    /// (`aiMoveToPosition`, `CMD_FROM_AI`); otherwise it copies the member's goal and re-enters the
    /// current state (which is `AI_IDLE` after the clear: `getCurrentStateID()` is `INVALID`, so
    /// `setState` falls back to the machine's default state).
    pub fn ai_join_team(&mut self, id: ObjectId) {
        let Some(obj) = self.objects.get(&id) else { return };
        if obj.is_dying() || obj.ai.machine.current == AiStateType::Dead {
            return; // `isAiInDeadState()`
        }
        if !self.is_mobile(id) {
            return;
        }
        self.ai_choose_locomotor_set(id, crate::template::LocomotorSet::Normal);
        self.ai_clear(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.goal_waypoint = None;
        }
        let team = self.objects.get(&id).and_then(|o| o.team.clone());
        let mut other = None;
        if let Some(team) = team.and_then(|k| self.scripts.teams.get(&k)) {
            for member in &team.members {
                if *member == id {
                    continue;
                }
                let Some(m) = self.objects.get(member) else { continue };
                if m.ai_handle.is_some() && !m.is_disabled_by_type(crate::disabled::DisabledType::Held) {
                    other = Some(*member);
                    break;
                }
            }
        }
        let Some(other) = other else { return };
        let (other_idle, goal_object, goal_position, other_pos) = match self.objects.get(&other) {
            Some(o) => (
                self.ai_is_idle(o),
                o.ai.machine.goal_object,
                o.ai.machine.goal_position,
                o.position,
            ),
            None => return,
        };
        if other_idle {
            // `aiMoveToPosition(other->getPosition(), CMD_FROM_AI)`.
            self.ai_private_move_to_position(id, other_pos, CommandSource::Ai);
            return;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            if goal_object != ObjectId::INVALID {
                o.ai.machine.goal_object = goal_object;
            } else {
                o.ai.machine.goal_position = goal_position;
            }
            o.ai.machine.last_command_source = CommandSource::Ai;
        }
        // `StateID state = getCurrentStateID();` right after the clear is `INVALID_STATE_ID`, so
        // `setState` runs the machine's default state (`AI_IDLE`).
        self.ai_set_state(id, AiStateType::Idle);
    }

    /// `AIGuardState` support: `getGuardTargetType` / `getGuardLocation` / `getGuardObject` are read
    /// by the guard state's `onEnter`.
    pub(crate) fn ai_set_guard(&mut self, id: ObjectId, target: crate::orders::GuardTarget, mode: crate::orders::GuardMode, source: CommandSource) {
        if !self.is_mobile(id) || self.objects.get(&id).is_some_and(|o| o.is_kind_of(kind_of::PROJECTILE)) {
            return;
        }
        // `privateGuardPosition/Object/Area` (`AIUpdate.cpp:4090,4139,4169`) clear the machine before
        // setting the guard fields the state's `onEnter` reads.
        if self.jet_base_command_clears(id) {
            self.ai_clear(id);
        }
        let object_pos = match target {
            crate::orders::GuardTarget::Object(oid) => Some(self.objects.get(&oid).map_or(Coord3D::ZERO, |t| t.position)),
            _ => None,
        };
        if let Some(o) = self.objects.get_mut(&id) {
            // A point or object guard has no area (`privateGuardPosition` / `privateGuardObject` leave `m_areaToGuard` unused).
            o.ai.machine.guard.area = None;
            match target {
                crate::orders::GuardTarget::Point(p) => {
                    o.ai.machine.guard_target_type = GuardTargetType::Location;
                    o.ai.machine.goal_position = p;
                    o.ai.machine.goal_object = ObjectId::INVALID;
                    o.combat.guard = Some(target);
                }
                crate::orders::GuardTarget::Object(oid) => {
                    o.ai.machine.guard_target_type = GuardTargetType::Object;
                    o.ai.machine.guard_object = oid;
                    o.ai.machine.goal_position = object_pos.unwrap_or(Coord3D::ZERO);
                    o.ai.machine.goal_object = oid;
                    o.combat.guard = Some(target);
                }
            }
            o.ai.machine.guard.mode = mode;
            o.combat.guard_mode = mode;
            o.ai.machine.last_command_source = source;
            o.combat.hunt = false;
            o.combat.attack_move = false;
            o.combat.attack = None;
        }
        self.ai_command_set_state(id, AiStateType::Guard);
    }
}

// ---- AI_IDLE ----------------------------------------------------------------------------------

impl GameLogic {
    /// The `SET_NORMAL` locomotor draws of an `AIUpdate` object's construction (`AIUpdateInterface` ctor ->
    /// `chooseLocomotorSet`, AIUpdate.cpp:296, 830; each `Locomotor::Locomotor` draws its wander angle, its
    /// increment and its direction, Locomotor.cpp:673-675). The wander itself is not ported.
    pub(crate) fn ai_creation_locomotor_draws(&mut self, normal_locomotors: u8) {
        for _ in 0..normal_locomotors {
            let _angle_offset = self.rng.value_real(-crate::math::PI / 6.0, crate::math::PI / 6.0);
            let _offset_increment = self.rng.value_real(0.8, 1.2);
            let _offset_increasing = self.rng.value(0, 1);
        }
    }

    /// The `AI_IDLE` entries of an `AIUpdate` object's construction, in C++ order: `makeStateMachine` builds the
    /// `AIStateMachine` (its state table's sub-machines enter `AI_IDLE` as they are built, AIStates.cpp:3528, 4393),
    /// then `onObjectCreated` enters the main `AI_IDLE` (AIUpdate.cpp:630). Each `AIIdleState::onEnter` draws its
    /// sleep offset (AIStates.cpp:1327). Returns the main machine's offset (the last draw).
    /// Only the base `AIUpdateInterfaceMachine` is traced (AN3 seed 1); the subclasses that override `makeStateMachine`
    /// (jet, chinook, supply truck, transport, wander, deliver payload, hack internet) call the base constructor in the
    /// source, but their counts are not traced.
    pub(crate) fn ai_creation_idle_draws(&mut self) -> u16 {
        let mut main_offset = 0u16;
        for _ in 0..AI_STATE_TABLE_SUBMACHINE_IDLES + AI_MAIN_MACHINE_IDLES {
            main_offset = self.rng.value(0, IDLE_COUNTDOWN_DELAY as i32) as u16;
        }
        main_offset
    }

    /// `AIIdleState::onEnter` (`AIStates.cpp:1313`).
    fn ai_idle_enter(&mut self, id: ObjectId) -> StateReturn {
        // `if (ai) ai->resetNextMoodCheckTime();` (AIStates.cpp:1321), before the sleep offset draw (:1327).
        self.reset_next_mood_check_time(id);
        let off = self.rng.value(0, IDLE_COUNTDOWN_DELAY as i32) as u16;
        // `AIIdleState::onEnter` (AIStates.cpp:1313-1340) does not touch the path or the locomotor goal: the exiting move
        // ends (`ai_internal_move_exit`), and the completed-move block of the update drops the path before the locomotor
        // (AIUpdate.cpp:1035, `ai_update_one`). The idle update's `doInitIdleState` clears the locomotor goal (:1383).
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.idle_inited = true;
            o.ai.machine.idle_sleep_offset = off;
        }
        StateReturn::Continue
    }

    /// `AIIdleState::doInitIdleState`'s "cheat move" (`AIStates.cpp:1358-1382`): an idle unit that is
    /// still doing ground movement and whose locomotor is not `ULTRA_ACCURATE` moves to its goal
    /// cell's world position at frame <= 1. The engine calls `updateGoal` on the unit's own position
    /// first (`:1370`), so the goal cell is the one its position falls in, then `goalPosition`
    /// (`AIPathfind.cpp:5303`) turns that cell back into a position, and `setPosition` (`:1372-1374`)
    /// puts the unit there. `updateGoal` is called once more with the adjusted position (`:1380`).
    /// This is why the civilian cars of AN3 leave their `.map` position on the first update: their
    /// footprint is not centred in a cell, so `adjustCoordToCell` returns `cell * 10 + 0.5`
    /// (`AIPathfind.cpp:9127`). An immobile object never reaches `goalPosition`: `updateGoal` bails
    /// out at its `KINDOF_IMMOBILE` guard (`:9809-9812`), so neither the goal cell nor the position
    /// changes.
    fn ai_idle_cheat_move(&mut self, id: ObjectId) {
        // `if (ai->isIdle() && ai->isDoingGroundMovement())` (`AIStates.cpp:1359`). `isIdle()` holds:
        // this runs from the idle state's own update.
        if !self.is_doing_ground_movement(id) {
            return;
        }
        let Some(o) = self.objects.get(&id) else { return };
        // `Pathfinder::updateGoal` returns before touching the goal cell for an immobile object
        // (`AIPathfind.cpp:9809-9812`), so `goalPosition` (`:1371`) has nothing to return and the
        // cheat move does not run. An immobile template can still hold a ground locomotor.
        if o.is_kind_of(kind_of::IMMOBILE) {
            return;
        }
        let pos = o.position;
        let ultra_accurate = o.loco_rt.ultra_accurate;
        // `if (goalPos.x || goalPos.y || goalPos.z)` (`AIStates.cpp:1368`).
        if pos == Coord3D::ZERO {
            return;
        }
        // `TheAI->pathfinder()->updateGoal(obj, &goalPos, layer)` on the unit's own position (`AIStates.cpp:1370`).
        self.pathfind_update_goal(id, &pos);
        // `if (!ultraAccurate && TheAI->pathfinder()->goalPosition(obj, &goalPos))` (`:1371`).
        if ultra_accurate {
            return;
        }
        let Some(goal) = self.pathfinder.goal_position(&self.terrain, id) else { return };
        // `if (TheGameLogic->getFrame()<=1) obj->setPosition(&goalPos); else` the `STOP_AND_SLIDE` `setFinalPosition`
        // (`:1372-1376`). `STOP_AND_SLIDE` is never defined in the build, so only the frame-1 move happens.
        if self.frame <= 1 {
            if let Some(o) = self.objects.get_mut(&id) {
                o.set_position(goal);
            }
            // `Object::setPosition` -> `setTriggerAreaFlagsForChangeInPosition` -> `Pathfinder::updatePos`
            // (`Object.cpp:2659-2666`).
            if let Some(po) = self.path_object(id) {
                self.pathfinder.update_unit_pos(&po, &goal, false);
            }
        }
        // `TheAI->pathfinder()->updateGoal(obj, &goalPos, layer)` again (`AIStates.cpp:1379`).
        self.pathfind_update_goal(id, &goal);
    }

    /// `Pathfinder::goalPosition` (AIPathfind.cpp:5303): the world position of the unit's `m_pathfindGoalCell`,
    /// when it has one. Read-only; the client's waypoint display uses the same query.
    pub fn ai_pathfind_goal(&self, id: ObjectId) -> Option<Coord3D> {
        self.pathfinder.goal_position(&self.terrain, id)
    }

    /// The pathfinder's view of unit `id` (`PathObject::for_object`), when the unit has a locomotor: its
    /// **current** locomotor set supplies the surfaces and `isAircraftThatAdjustsDestination`
    /// (`AIUpdate.cpp:2408-2429`), so a taxiing jet is a wheels aircraft and not an aircraft goal.
    pub(crate) fn path_object(&self, id: ObjectId) -> Option<PathObject> {
        PathObject::for_object(self.objects.get(&id)?)
    }

    /// `Pathfinder::updateGoal` (AIPathfind.cpp:9805) for unit `id` at `pos`. An immobile unit takes no goal
    /// (`:9809`). A unit not doing ground movement takes an aircraft goal (`updateAircraftGoal`, `:9824`); one that
    /// is doing ground movement marks the ground goal cells.
    pub(crate) fn pathfind_update_goal(&mut self, id: ObjectId, pos: &Coord3D) {
        if self.objects.get(&id).is_none_or(|o| o.is_kind_of(kind_of::IMMOBILE)) {
            return;
        }
        let Some(po) = self.path_object(id) else { return };
        // `if (!ai->isDoingGroundMovement()) { ... updateAircraftGoal(obj, newGoalPos); return; }`
        // (`AIPathfind.cpp:9816-9827`): a unit that is not doing ground movement keys an aircraft
        // goal mark (its own cell record), not the ground `setGoalUnit` cells. `updateAircraftGoal`
        // only marks for `isAircraftThatAdjustsDestination` (`:9905-9951`), so a taxiing jet (wheels)
        // removes any old mark and adds none.
        let ground = self.is_doing_ground_movement(id);
        self.pathfinder.update_goal_dispatch(&po, pos, ground);
    }

    /// `Pathfinder::removeGoal` (AIPathfind.cpp:9963) for unit `id`. An immobile unit has no goal to remove (`:9966`).
    pub(crate) fn pathfind_remove_goal(&mut self, id: ObjectId) {
        if self.objects.get(&id).is_none_or(|o| o.is_kind_of(kind_of::IMMOBILE)) {
            return;
        }
        self.pathfinder.remove_goal(id.0);
    }

    /// `AIIdleState::update` (`AIStates.cpp:1392`) with `doInitIdleState` folded in.
    fn ai_idle_update(&mut self, id: ObjectId) -> StateReturn {
        // `doInitIdleState` (AIStates.cpp:1334-1385). The engine "cheat moves" an idle unit that still
        // counts as doing ground movement to the nearest pathfind cell at start-up: `updateGoal` on its
        // own position (`:1370`), then `goalPosition` (the goal cell's world position, `AIPathfind.cpp:5303`),
        // and at frame <= 1 `setPosition` to it (`:1372-1374`). Later it stores that as the "final
        // position" (`setFinalPosition`, `AIUpdate.h:525`), which leaves `m_doFinalPosition` false, so a
        // running unit does not move; the port keeps only the start-up branch.
        let inited = self.objects.get(&id).is_some_and(|o| o.ai.machine.idle_inited);
        if inited {
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.idle_inited = false;
            }
            self.ai_idle_cheat_move(id);
            // `ai->setLocomotorGoalNone(); ai->setCurrentVictim(nullptr);` (`AIStates.cpp:1383-1384`).
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.locomotor_goal_type = LocoGoalType::None;
                o.ai.requested_victim = ObjectId::INVALID;
            }
        }
        // `timeToSleep = IDLE_COUNTDOWN_DELAY + m_initialSleepOffset; oldSleepOffset = m_initialSleepOffset;
        // m_initialSleepOffset = 0` (AIStates.cpp:1400-1403): the offset is used once, and kept only if the
        // mood check below is the earlier wake.
        let old_offset = self.objects.get_mut(&id).map_or(0, |o| std::mem::take(&mut o.ai.machine.idle_sleep_offset));
        let mut sleep = IDLE_COUNTDOWN_DELAY + u32::from(old_offset);
        // "Check to see if we have created a crate we need to pick up" (`AIStates.cpp:1423`): never a crate.
        let _ = self.check_for_crate_to_pickup(id);
        // `m_shouldLookForTargets` is true for the main `AI_IDLE`. The mood targeting is skipped while the unit is
        // paralyzed, unmanned, EMP-disabled, subdued or hacked (AIStates.cpp:1432-1437); the sleep below still runs.
        let disabled_for_mood = self.objects.get(&id).is_some_and(|o| {
            [
                crate::disabled::DisabledType::Paralyzed,
                crate::disabled::DisabledType::Unmanned,
                crate::disabled::DisabledType::Emp,
                crate::disabled::DisabledType::Subdued,
                crate::disabled::DisabledType::Hacked,
            ]
            .iter()
            .any(|t| o.is_disabled_by_type(*t))
        });
        // `getNextMoodTarget(true, true)` (AIStates.cpp:1444).
        let found = if disabled_for_mood { None } else { self.ai_idle_scan(id, false, None, crate::orders::GuardMode::Normal, false) };
        if let Some(target) = found {
            self.ai_attack_object(id, target, false, false, None);
            // `return STATE_CONTINUE` after the attack (AIStates.cpp:1446-1450).
            return StateReturn::Continue;
        }
        // `AIIdleState::update` (AIStates.cpp:1454-1466): sleep no longer than until the next mood check
        // (`getNextMoodCheckTime`); the saved offset comes back only with that wake.
        let now = self.frame;
        let next = self.next_mood_check_time(id);
        if next > now && next - now < sleep {
            sleep = next - now;
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.idle_sleep_offset = old_offset;
            }
        }
        StateReturn::Sleep(sleep)
    }

}

// ---- AI_MOVE_TO / AIInternalMoveToState / AIMoveToState ---------------------------------------

impl GameLogic {
    /// `AIMoveToState::onEnter` (`AIStates.cpp:2022`).
    fn ai_move_to_enter(&mut self, id: ObjectId) -> StateReturn {
        let m = match self.objects.get(&id) {
            Some(o) => o.ai.machine.clone(),
            None => return StateReturn::Failure,
        };
        let goal = self.ai_goal_position(id, &m);
        if goal.is_none() {
            return StateReturn::Failure;
        }
        let goal = goal.unwrap();
        // `AIMoveToState::onEnter`: a goal object that is the ignored obstacle turns off destination
        // adjustment.
        let mut adjusts = true;
        if m.goal_object != ObjectId::INVALID {
            if let Some(o) = self.objects.get(&id) {
                if o.ai.ignore_obstacle_id == m.goal_object {
                    adjusts = false;
                }
            }
        }
        self.ai_internal_move_enter(id, goal, adjusts)
    }

    /// `AIInternalMoveToState::onEnter` (`AIStates.cpp:1615`) + `computePath` (`AIStates.cpp:1600`).
    fn ai_internal_move_enter(&mut self, id: ObjectId, goal: Coord3D, adjusts: bool) -> StateReturn {
        self.ai_internal_move_enter_with(id, goal, adjusts, false)
    }

    /// [`ai_internal_move_enter`](Self::ai_internal_move_enter) for a goal the caller already adjusted (`goal_adjusted`):
    /// `AIFollowPathState::update` adjusts its last point itself and fails the state when that fails (AIStates.cpp:3403-3408).
    fn ai_internal_move_enter_with(&mut self, id: ObjectId, goal: Coord3D, mut adjusts: bool, goal_adjusted: bool) -> StateReturn {
        if !self.is_mobile(id) {
            return StateReturn::Failure;
        }
        // `m_waitingForPath = ai->isWaitingForPath()` (`AIStates.cpp:1620`), *before* the adjust and
        // `computePath`: a subclass that requested its own path in `onEnter` before delegating here (e.g.
        // `AIMoveAndTightenState`) picks up that request's state. `computePath` then sets it true again
        // (`:1604`), so every path through this function leaves it true.
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.move_waiting_for_path = o.ai.waiting_for_path;
        }
        // `goal->testStatus(OBJECT_STATUS_IMMOBILE)` / a rider cannot move itself.
        if let Some(o) = self.objects.get_mut(&id) {
            if o.is_kind_of(kind_of::IMMOBILE) {
                return StateReturn::Failure;
            }
            o.ai.machine.move_try_one_more_repath = true;
        }
        // `ai->getCurLocomotor()->startMove()` (`AIStates.cpp:1627-1628`): reset the donut timer when a
        // move begins (`Locomotor.cpp:782-786`).
        self.loco_start_move(id);
        // `if (getCurLocomotor()->isUltraAccurate()) setAdjustsDestination(false)` (`AIStates.cpp:1627-1632`):
        // "if we're being ultra accurate, we can't adjust the destination" (the jet taxi's precise nodes).
        if self.objects.get(&id).is_some_and(|o| o.loco_rt.ultra_accurate) {
            adjusts = false;
        }
        // `onEnter`'s adjust (AIStates.cpp:1661-1664), then `computePath` -> `requestPath(&m_goalPosition, getAdjustsDestination())`.
        // `if (getAdjustsDestination() && !obj->testStatus(OBJECT_STATUS_RIDER8)) { adjustDestination (or
        // snapClosestGoalPosition); updateGoal(obj, &m_goalPosition, layer); }` (AIStates.cpp:1661-1667): the adjusted
        // (or snapped) goal becomes the unit's goal cell.
        // `ai->friend_startingMove()` (AIStates.cpp:1635) before the adjust and `computePath`: the move has begun, so
        // `m_isMoving` is set and the blocked state is cleared before the request sees it (`requestPath` reads
        // `m_isBlockedAndStuck` only after this).
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.friend_starting_move();
        }
        let rider8 = self.objects.get(&id).is_some_and(|o| o.status_bits & crate::object::status::RIDER8 != 0);
        let goal = if goal_adjusted {
            goal
        } else if adjusts && !rider8 {
            self.ai_adjust_move_goal(id, goal)
        } else {
            goal
        };
        if goal_adjusted || (adjusts && !rider8) {
            self.pathfind_update_goal(id, &goal);
        }
        self.ai_request_path(id, goal, adjusts);
        if let Some(o) = self.objects.get_mut(&id) {
            // `computePath`'s `friend_startingMove()` (AIStates.cpp:1607) runs after `requestPath`.
            o.ai.friend_starting_move();
            // `computePath` sets the state's `m_waitingForPath = true` (`AIStates.cpp:1604`) before it calls
            // `requestPath`, so the move's first update runs the wait block even when `requestPath` built the
            // path on the spot (a quick-path unit: it clears the flag, takes `updateGoal`/`removeGoal` and goes
            // on to steer). Copying `ai->isWaitingForPath()` here instead would skip that block for every air
            // move, because a quick path clears the AI-level flag.
            o.ai.move_waiting_for_path = true;
            o.ai.desired_speed = crate::ai::FAST_AS_POSSIBLE;
            o.ai.path_extra_distance = 0.0;
        }
        StateReturn::Continue
    }

    /// `AIInternalMoveToState::onEnter`'s `adjustDestination`,
    /// or `snapClosestGoalPosition` when that fails (`AIStates.cpp:1661-1664`). The caller skips it for a `RIDER8`
    /// unit (`AIStates.cpp:1661`).
    ///
    /// Aircraft are adjusted too: `Pathfinder::adjustDestination` (`AIPathfind.cpp:5462`) runs for every non-projectile
    /// object, and `checkForAdjust` (`:5314-5355`) special-cases `KINDOF_AIRCRAFT` only by skipping the path test
    /// (`adjustedPathExists = true`). The port skipped the whole adjust for an air-only `KINDOF_AIRCRAFT`, so a
    /// Chinook flew to the raw `findPositionAround` point instead of that point's cell — `adjustCoordToCell`'s centred
    /// formula is `(cell + 0.5) * 10` (`:9014-9015`) and its non-centred one `(cell + 0.05) * 10` (`:9017-9018`); a
    /// Chinook's footprint is centred. AN3 s1's dock approach aimed at (656.7874,163.8239) where C++ aims at the
    /// adjusted (660.5,160.5) — hidden until f1639 because the 6 deg/frame turn clamp made both trajectories identical
    /// up to the frame the clamp released.
    ///
    /// The compiled `adjustDestination` arm is the retail one (`RETAIL_COMPATIBLE_PATHFINDING` = 1, `GameDefines.h:115-117`):
    /// `checkForAdjust` then `dest = adjustDest` (`:5491-5493`). The `useExactDestination` / `isHuman && single-unit group`
    /// arm at `:5495-5507` is in the `#else` and is dead code in this build; it is deliberately not ported.
    pub(crate) fn ai_adjust_move_goal(&mut self, id: ObjectId, goal: Coord3D) -> Coord3D {
        let po = {
            let Some(o) = self.objects.get(&id) else { return goal };
            let Some(mut po) = PathObject::for_object(o) else { return goal };
            po.ignore_obstacle = o.ai.ignore_obstacle_id;
            po
        };
        // `if (!adjustDestination(obj, locoset, &m_goalPosition)) snapClosestGoalPosition(obj, &m_goalPosition);`
        // (AIStates.cpp:1662-1663). The group's offset adjust is not here: `computeIndividualDestination` did it at
        // order time (AIGroup.cpp:546-548, `player_orders.rs`).
        match self.pathfinder.adjust_destination(&self.terrain, &po, &goal, None) {
            Some(adjusted) => adjusted,
            None => self.pathfinder.snap_closest_goal_position(&self.terrain, &po, &goal),
        }
    }

    /// `Pathfinder::adjustDestination` for a move's goal with no snap fallback (`ai_adjust_move_goal`'s first
    /// step): `None` when the adjust fails (`AIFollowPathState::update` returns STATE_FAILURE, AIStates.cpp:3406-3408).
    fn ai_adjust_move_goal_strict(&mut self, id: ObjectId, goal: Coord3D) -> Option<Coord3D> {
        let po = {
            let o = self.objects.get(&id)?;
            let mut po = PathObject::for_object(o)?;
            po.ignore_obstacle = o.ai.ignore_obstacle_id;
            po
        };
        self.pathfinder.adjust_destination(&self.terrain, &po, &goal, None)
    }

    /// `AIInternalMoveToState::onExit` (`AIStates.cpp:1736`): `ai->friend_endingMove()` (`AIStates.cpp:1747`), which only
    /// sets `m_movementComplete` (`AIUpdate.cpp:2046-2050`). The path is dropped by the update's completed-move block
    /// ([`ai_end_completed_move`](Self::ai_end_completed_move)), which runs after the state machine and before the locomotor
    /// in the same update (`AIUpdate.cpp:1035` before `:1114`).
    pub(crate) fn ai_internal_move_exit(&mut self, id: ObjectId) {
        let mut interrupted_drop = false;
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.friend_ending_move();
            // The combat drop's z gate belongs to its move. A move that exits with the gate still set was interrupted (a
            // success or failure takes the gate first, in `ai_note_drop_move_result`).
            interrupted_drop = o.ai.machine.drop_move_z.take().is_some();
        }
        if interrupted_drop {
            // `ChinookMoveToBldgState::onExit` (ChinookAIUpdate.cpp:770-782) runs on every exit, including a new command that
            // replaces the move (`setMyState` clears the machine, :1212-1222). The preferred height and the ultra-accurate flag
            // are restored at once, and the combat drop is over: the interrupted move has no result (lane 177b), and the
            // Chinook's state is ended here rather than on its next update (lane 187, R24-5).
            self.end_drop_move_settings(id);
            if let Some(st) = self.chinooks.get_mut(&id) {
                st.done = true;
            }
        }
    }

    /// The `m_movementComplete` block of `AIUpdateInterface::update` (`AIUpdate.cpp:1032-1062`), the part the
    /// port keeps here: `setQueueForPathTime(0)`, `destroyPath()` (path, raw path, waiting flag and locomotor goal,
    /// `AIUpdate.cpp:2018-2028`), then `ignoreObstacle(nullptr)`. The pop-to-goal it runs
    /// (`AIUpdate.cpp:1044-1058`) is [`ai_pop_to_goal`](Self::ai_pop_to_goal).
    pub(crate) fn ai_end_completed_move(&mut self, id: ObjectId) {
        let Some(o) = self.objects.get_mut(&id) else { return };
        if !o.ai.movement_complete {
            return;
        }
        o.ai.queue_for_path_frame = 0;
        o.ai.path.clear();
        o.ai.raw_path = None;
        o.ai.waiting_for_path = false;
        // `destroyPath` clears the attack request (`m_isAttackPath = FALSE`, AIUpdate.cpp:2026), so `doPathfind` returns
        // at once for a request that was queued by the exited attack (AIUpdate.cpp:398-400).
        o.ai.attack_path_active = false;
        if o.ai.request == crate::ai::PathRequest::Attack {
            o.ai.request = crate::ai::PathRequest::None;
        }
        o.ai.locomotor_goal_type = LocoGoalType::None;
        // The pop to goal (AIUpdate.cpp:1044-1058), after `destroyPath` and `setLocomotorGoalNone` as in C++.
        self.ai_pop_to_goal(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.movement_complete = false;
            o.ai.ignore_obstacle_id = ObjectId::INVALID;
        }
    }

    /// The pop-to-goal part of the `m_movementComplete` block (`AIUpdate.cpp:1044-1058`): `goalPosition` of the
    /// unit's goal cell; when that cell's centre is a cell or more away, the unit's own cell (`snapPosition`) instead;
    /// then `setFinalPosition` (`AIUpdate.h:525`), which records the position and leaves `m_doFinalPosition` FALSE.
    /// Nothing reads that position as a move in this C++ build: `m_doFinalPosition` is never set TRUE (only the xfer
    /// reads it), so the `NONE` final-position branch of `doLocomotor` (`AIUpdate.cpp:2256-2290`) cannot run.
    ///
    /// The goal cell is the unit's stored `m_pathfindGoalCell` (`Pathfinder::goal_position`), and the pop's `updateGoal`
    /// (`:1057`, `Pathfinder::updateGoal`) re-keys it to the popped cell.
    fn ai_pop_to_goal(&mut self, id: ObjectId) {
        let Some(o) = self.objects.get(&id) else { return };
        let pos = o.position;
        let Some(po) = PathObject::for_object(o) else { return };
        // `if (TheAI->pathfinder()->goalPosition(getObject(), &goalPos))` (`:1046`).
        let Some(mut goal) = self.pathfinder.goal_position(&self.terrain, id) else { return };
        // `if (dx*dx+dy*dy>=PATHFIND_CELL_SIZE_F*PATHFIND_CELL_SIZE_F)` (`:1049-1052`): too far, grid the current position.
        let dx = goal.x - pos.x;
        let dy = goal.y - pos.y;
        if dx * dx + dy * dy >= PATHFIND_CELL_SIZE_F * PATHFIND_CELL_SIZE_F {
            goal = self.pathfinder.snap_position(&self.terrain, &po, &pos);
        }
        // `setFinalPosition(&goalPos)` (`:1057`, `AIUpdate.h:525`).
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.final_position = goal;
            o.ai.do_final_position = false;
        }
        // `TheAI->pathfinder()->updateGoal(getObject(), &goalPos, getObject()->getLayer())` (`:1058`).
        self.pathfind_update_goal(id, &goal);
    }

    /// `if (thePath != nullptr) ai->setLocomotorGoalPositionOnPath();` (`AIStates.cpp:1875-1876`): while
    /// a path exists the locomotor goal is `POSITION_ON_PATH`. This matters for a move entered without
    /// `requestPath` (the jet taxi through `AIMoveOutOfTheWayState`, `AIStates.cpp:2148`), whose goal
    /// would otherwise stay `NONE` and read as "already there" (`getLocomotorDistanceToGoal`).
    fn ai_sync_goal_on_path(&mut self, id: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            if o.ai.has_path() {
                o.ai.set_locomotor_goal_position_on_path();
            }
        }
    }

    /// `AIInternalMoveToState::update`'s arrival test (`AIStates.cpp:1908-1926`): the locomotor's
    /// `getCloseEnoughDist` against `getLocomotorDistanceToGoal` (0 with no path, `AIUpdate.cpp:2449-2467`),
    /// then the ground sanity check measured to the last node or, with no path, the goal
    /// (`AIStates.cpp:1912-1919`). A unit with no current locomotor never arrives (`ai->getCurLocomotor() &&`).
    pub(crate) fn ai_at_goal(&self, id: ObjectId, goal: &Coord3D) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        let Some(loco) = o.locomotor() else { return false };
        let close = loco.close_enough_dist;
        if self.locomotor_distance_to_goal(id) >= close {
            return false;
        }
        // "AIInternalMoveToState Trying to finish early. Continuing..." (`AIStates.cpp:1912-1926`): a
        // ground unit more than four cells from its goal keeps moving. `goalPos = *ai->getPath()->getLastNode()`
        // (`:1916-1918`): the path's last *raw* node (`getLastNode`), which for a start-only path is the
        // start node (`getPath()` is not the waypoint list).
        let target = o
            .ai
            .raw_path
            .as_ref()
            .and_then(|raw| raw.nodes.last().map(|n| n.pos))
            .or_else(|| o.ai.path.last().copied())
            .unwrap_or(*goal);
        if self.is_doing_ground_movement(id) && o.position.dist_2d(&target) > 4.0 * PATHFIND_CELL_SIZE_F {
            return false;
        }
        true
    }

    /// `AIInternalMoveToState::update`'s "reached the end of the path" (`AIStates.cpp:1929-1932`): the
    /// state returns `STATE_SUCCESS`; its `m_goalPosition` and `m_path` stay (only
    /// `if (getAdjustsDestination()) ai->setLocomotorGoalNone();` runs). The path is dropped by the
    /// end-of-update `m_movementComplete` block (`AIUpdate.cpp:1035-1061`) only when the state machine did
    /// *not* start another move in the same update (`friend_startingMove` clears the flag,
    /// `AIUpdate.cpp:2034-2040`); an enclosing state that keeps running after the move reports success
    /// (the attack-move's retry sleep, `AIStates.cpp:3676-3690`) keeps its path.
    fn ai_finish_move(&mut self, id: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            if o.ai.machine.move_adjusts {
                // `if (getAdjustsDestination()) ai->setLocomotorGoalNone();` (`AIStates.cpp:1930-1931`).
                o.ai.locomotor_goal_type = LocoGoalType::None;
            }
        }
    }

    /// `AIInternalMoveToState::forceRepath` (`AIStateMachine.h:275-279`): the state's own `m_pathGoalPosition`
    /// becomes `(-100, -100, -100)` and its `m_pathTimestamp` `-MIN_REPATH_TIME` (an `UnsignedInt`, so the
    /// subtraction wraps: `wrapping_sub`, not a saturating clamp — `now - m_pathTimestamp` then reads
    /// `now + MIN_REPATH_TIME` and the `> MIN_REPATH_TIME` test is true, `AIStates.cpp:1879`). Its only engine caller is
    /// the attack move's sleep-retry wake (`AIStates.cpp:3673`).
    fn ai_force_repath(&mut self, id: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.attack_move_path_goal = Coord3D::new(-100.0, -100.0, -100.0);
            o.ai.move_path_timestamp = 0u32.wrapping_sub(MIN_REPATH_TIME);
        }
    }

    /// `AIInternalMoveToState::update` (`AIStates.cpp:1766-1943`), in the engine's order: the
    /// waiting-for-path gate (`:1786-1813`), the locomotor goal re-arm for a live path (`:1875-1876`),
    /// the blocked / moved-goal repath (`:1879-1903`) and the arrival test (`:1908-1940`).
    fn ai_internal_move_update(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        let (goal, waiting, path_ts, blocked_stuck, blocked_frames, adjusts, retry) = match self.objects.get(&id) {
            Some(o) => (
                o.ai.move_goal,
                // The state's own `m_waitingForPath`; `waiting_for_path` is the `AIUpdateInterface`
                // flag the pathfinder clears in `doPathfind` (`AIStates.cpp:1786`).
                o.ai.move_waiting_for_path,
                o.ai.move_path_timestamp,
                o.ai.is_blocked_and_stuck,
                o.ai.blocked_frames,
                o.ai.machine.move_adjusts,
                o.ai.retry_path,
            ),
            None => return StateReturn::Failure,
        };
        let Some(goal) = goal else {
            // No move goal: nothing to steer toward. (`m_goalPosition` is always set in C++; the
            // port's `None` is the "never moved" state.)
            return StateReturn::Success;
        };
        // `if (m_waitingForPath) { m_pathTimestamp = frame; if (ai->isWaitingForPath()) return CONTINUE;
        // if (thePath==nullptr) return FAILURE; ... }` (`AIStates.cpp:1786-1813`). The gate returns
        // *before* the locomotor goal re-arm at `:1875`, so a move waiting for its next path keeps the
        // goal type (and thus the locomotor state) it had.
        if waiting {
            if let Some(o) = self.objects.get_mut(&id) {
                // `m_pathTimestamp = TheGameLogic->getFrame()` (AIStates.cpp:1789), the state's own clock.
                o.ai.move_path_timestamp = now;
            }
            // `ai->isWaitingForPath()`: the pathfinder has not finished the request yet.
            let still = self.objects.get(&id).is_some_and(|o| o.ai.waiting_for_path);
            if still {
                return StateReturn::Continue;
            }
            if self.objects.get(&id).is_none_or(|o| !o.ai.has_path()) {
                // `if (thePath==nullptr) return STATE_FAILURE;` (`AIStates.cpp:1794`).
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.move_waiting_for_path = false;
                }
                return StateReturn::Failure;
            }
            if let Some(o) = self.objects.get_mut(&id) {
                // `m_waitingForPath = false; m_pathGoalPosition = m_goalPosition;` (`AIStates.cpp:1803`).
                o.ai.move_waiting_for_path = false;
                o.ai.path_goal_position = goal;
                if o.ai.machine.current == AiStateType::AttackMoveTo && !o.ai.machine.temp_live {
                    o.ai.machine.attack_move_path_goal = goal;
                }
            }
            // `if (getAdjustsDestination()) updateGoal(lastNode) else removeGoal(obj)` (AIStates.cpp:1806-1808): the
            // path's last node becomes the goal cell, or the goal is dropped when the move does not adjust.
            if adjusts {
                if let Some(last) = self.objects.get(&id).and_then(|o| o.ai.path.last().copied()) {
                    self.pathfind_update_goal(id, &last);
                }
            } else {
                self.pathfind_remove_goal(id);
            }
            if !retry {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.move_try_one_more_repath = false;
                }
            }
        }
        // `if (thePath != nullptr) ai->setLocomotorGoalPositionOnPath();` (`AIStates.cpp:1875-1876`).
        self.ai_sync_goal_on_path(id);
        // `Bool forceRecompute = false; if (thePath==nullptr) forceRecompute = true;` (`AIStates.cpp:1815-1818`).
        let no_path = self.objects.get(&id).is_none_or(|o| !o.ai.has_path());
        let mut force = no_path;
        // The engine sets a `blocked` local here too (`AIStates.cpp:1819-1827`) but never reads it in this
        // function; only `m_blockedRepathTimestamp` has an effect, so the port keeps just that side.
        if blocked_stuck || blocked_frames > 2 * LOGICFRAMES_PER_SECOND as u16 {
            force = true;
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.blocked_repath_timestamp = now;
            }
        }
        // `isSamePosition(obj->getPosition(), &m_pathGoalPosition, &m_goalPosition)` (`AIStates.cpp:1881`): the
        // *running state object's* own value. The attack move is the one state whose value the port also keeps in a
        // named copy (`AiMachine::attack_move_path_goal`), because its update is pre-empted by the attack sub-machine
        // and its retry test reads it (`AIStates.cpp:3680`); while its own update runs that copy is the live value
        // (the wait block stamps both, `:1804`), and `forceRepath` writes it (`AIStateMachine.h:275-279`).
        let path_goal = self.objects.get(&id).map_or(Coord3D::ZERO, |o| {
            if o.ai.machine.current == AiStateType::AttackMoveTo && !o.ai.machine.temp_live {
                o.ai.machine.attack_move_path_goal
            } else {
                o.ai.path_goal_position
            }
        });
        let goal_moved = self.objects.get(&id).is_some_and(|o| !crate::ai::is_same_position(o.position, path_goal, goal));
        if (force || now.wrapping_sub(path_ts) > MIN_REPATH_TIME) && (force || goal_moved) {
            // `computePath()` (`AIStates.cpp:1600-1607`) sets the state flag and calls `requestPath`, which applies
            // its own ordering: a quick-path unit rebuilds the path immediately (`AIUpdate.cpp:493-497`), and only a
            // queued request sees the three-frame anti-spin wait and the blocked-and-stuck reset (`:499-514`). The
            // port used to duplicate that guard here, before the call, which made the anti-spin branch unreachable
            // for air repaths and skipped the reset at this call site; there is nothing left to check here.
            // `AIMoveOutOfTheWayState::computePath` (AIStates.cpp:2163-2173) overrides the base `computePath`: it sets
            // `m_waitingForPath`, and when the unit is blocked and stuck it lets it path through units; it never
            // requests a path, and it does not call `friend_startingMove` (AIStates.cpp:1607). It returns TRUE, so the
            // update keeps the path it has (`:1896-1901`).
            let move_away = self.objects.get(&id).is_some_and(|o| {
                o.ai.machine.temporary == Some(AiStateType::MoveOutOfTheWay) || o.ai.machine.current == AiStateType::MoveOutOfTheWay
            });
            if move_away {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.move_waiting_for_path = true;
                    if blocked_stuck {
                        o.ai.can_path_through_units = true;
                    }
                }
            } else {
                self.ai_request_path(id, goal, adjusts);
                // `computePath`'s `friend_startingMove()` (AIStates.cpp:1607) after the repath's request (`:1884`).
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.friend_starting_move();
                }
            }
            // `if (ai->getPath() != nullptr) ai->setLocomotorGoalPositionOnPath(); else return STATE_CONTINUE;`
            // (`AIStates.cpp:1896-1901`): a fresh request does not drop the old path, so the unit keeps
            // following it while the new one is computed. A quick path was installed by the request itself.
            if self.objects.get(&id).is_none_or(|o| !o.ai.has_path()) {
                return StateReturn::Continue;
            }
            self.ai_sync_goal_on_path(id);
        }
        // `Real onPathDistToGoal = ai->getLocomotorDistanceToGoal(); if (ai->getCurLocomotor() &&
        // (onPathDistToGoal < ai->getCurLocomotor()->getCloseEnoughDist())) { ... }` (`AIStates.cpp:1908-1940`).
        // `getLocomotorDistanceToGoal` is 0 for a `NONE` goal (`AIUpdate.cpp:2523-2526`), i.e. "already
        // there", which is how the engine re-reports a finished move while its enclosing state keeps
        // running (the attack-move's retry sleep, `AIStates.cpp:3676`). The ground branch dereferences
        // `ai->getPath()->getLastNode()` (`:1916`), which the engine only reaches with a live path: a move
        // whose path search came back empty returns CONTINUE at `:1898-1901`, so the test stays gated.
        // `getLocomotorDistanceToGoal`'s flight branch advances the path's latch (`computeFlightDistToGoal`) before the test.
        self.flight_distance_latch(id);
        let has_path = self.objects.get(&id).is_some_and(|o| o.ai.has_path());
        if has_path && self.ai_at_goal(id, &goal) {
            // `ChinookMoveToBldgState::update` (ChinookAIUpdate.cpp:766-776): a finished move is not a success while the
            // Chinook is more than 3 from the drop height (`status = STATE_CONTINUE`), and it keeps its goal until then.
            let z_wait = self.objects.get(&id).is_some_and(|o| o.ai.machine.drop_move_z.is_some_and(|dz| (o.position.z - dz).abs() > 3.0));
            if z_wait {
                return StateReturn::Continue;
            }
            self.ai_finish_move(id);
            return StateReturn::Success;
        }
        StateReturn::Continue
    }
}

// ---- AIMoveOutOfTheWayState / AIMoveAndTightenState -------------------------------------------

impl GameLogic {
    /// `AIMoveOutOfTheWayState::onEnter` (`AIStates.cpp:2148-2160`) over `AIInternalMoveToState::onEnter`
    /// (`:1615`): keep the path `getMoveAwayFromPath` installed (`AIMoveOutOfTheWayState::computePath`
    /// overrides `:2163-2173` — no repath, it only sets the state's waiting flag), run the base move's
    /// destination adjust and goal bookkeeping (`:1661-1668`) and start the move.
    fn ai_move_out_of_way_enter(&mut self, id: ObjectId) -> StateReturn {
        let Some(o) = self.objects.get(&id) else { return StateReturn::Failure };
        // `AIInternalMoveToState::onEnter` (AIStates.cpp:1621-1625): `if (obj->testStatus(OBJECT_STATUS_IMMOBILE)) return STATE_FAILURE`
        // (the status, not the kind; lane 190, R26-12).
        if o.status_bits & crate::object::status::IMMOBILE != 0 {
            return StateReturn::Failure;
        }
        // `if (ai->getPath()==nullptr) return STATE_FAILURE; m_goalPosition =
        // *getPath()->getLastNode()->getPosition()` (`:2154-2159`).
        // `getPath()->getLastNode()` is the raw path's last node (lane 187, R24-7); the waypoint list is the fallback
        // for the port's waypoint-only paths.
        let Some(mut goal) = o.ai.raw_path.as_ref().and_then(|r| r.nodes.last().map(|n| n.pos)).or_else(|| o.ai.path.last().copied()) else {
            return StateReturn::Failure;
        };
        let Some(po) = PathObject::for_object(o) else { return StateReturn::Failure };
        let rider8 = o.status_bits & crate::object::status::RIDER8 != 0;
        // `setAdjustsDestination(true)` (`:2150`), then `AIInternalMoveToState::onEnter`'s adjust
        // (`:1661-1666`): `adjustDestination`, else `snapClosestGoalPosition`. The base then keys the goal
        // cell with `updateGoal` (`:1667`). An ultra-accurate locomotor (the jet taxi, `:1629-1632`)
        // skips both.
        let ultra = o.loco_rt.ultra_accurate;
        // `ai->getCurLocomotor()->startMove()` (`:1627-1628`), then `ai->friend_startingMove()` (AIStates.cpp:1635, inside
        // `AIInternalMoveToState::onEnter`, which this state returns): `m_isMoving` is set and the blocked flags are cleared
        // before the adjust (AIUpdate.cpp:2034-2040). Lane 187 (R24-7): the start order is `startMove` first, as in C++.
        self.loco_start_move(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.friend_starting_move();
        }
        // `if (getAdjustsDestination() && !RIDER8)` (AIStates.cpp:1661): the adjust is the state's (`setAdjustsDestination(true)`,
        // `:2150`) and is skipped for a unit with `OBJECT_STATUS_RIDER8`.
        if !ultra && !rider8 {
            goal = match self.pathfinder.adjust_destination(&self.terrain, &po, &goal, None) {
                Some(adjusted) => adjusted,
                None => self.pathfinder.snap_closest_goal_position(&self.terrain, &po, &goal),
            };
            self.pathfind_update_goal(id, &goal);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            // The goal was adjusted just above; a later path request does not adjust again (see `ai_request_path`).
            o.ai.adjusts_destination = false;
            o.ai.machine.move_adjusts = !ultra;
            o.ai.move_goal = Some(goal);
            o.ai.path_goal_position = goal;
            // `ai->setPathExtraDistance(0); ai->setDesiredSpeed(FAST_AS_POSSIBLE)` (`:1684-1686`).
            o.ai.path_extra_distance = 0.0;
            o.ai.desired_speed = crate::ai::FAST_AS_POSSIBLE;
            // The `computePath` override sets the *state's* `m_waitingForPath` (`:2167`) without queueing
            // a request; `AIInternalMoveToState::onEnter` copied `ai->isWaitingForPath()` (`:1620`), which
            // stays clear, so the first update walks the path.
            o.ai.move_waiting_for_path = true;
            o.ai.waiting_for_path = false;
            o.ai.movement_complete = false;
        }
        StateReturn::Continue
    }

    /// `AIMoveAndTightenState::onEnter` (`AIStates.cpp:2234-2245`): `setAdjustsDestination(false)`,
    /// `removeGoal` (`:2241`), then `requestApproachPath` and `AIInternalMoveToState::onEnter`, whose own
    /// adjust is skipped (`:1661`) and whose `computePath` override (`:2263-2278`) only uses the request.
    fn ai_move_and_tighten_enter(&mut self, id: ObjectId) -> StateReturn {
        if !self.is_mobile(id) {
            return StateReturn::Failure;
        }
        let goal = self.objects.get(&id).map(|o| o.ai.machine.goal_position).unwrap_or(Coord3D::ZERO);
        if let Some(o) = self.objects.get_mut(&id) {
            // `setAdjustsDestination(false)` (`:2236`): the move always uses the approach path's goal.
            o.ai.adjusts_destination = false;
            o.ai.machine.move_adjusts = false;
        }
        // `TheAI->pathfinder()->removeGoal(obj);` (`:2241`), before the approach request.
        self.pathfind_remove_goal(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.start_move(goal, crate::ai::PathRequest::Approach, false);
            // `requestApproachPath` sets `m_isFinalGoal = TRUE` (AIUpdate.cpp:549), whatever the state's adjust flag is.
            // Lane 187 (R24-10); the approach branch never reads it, so the effect is state fidelity only.
            o.ai.is_final_goal = true;
            // `AIMoveAndTightenState::onEnter` requests the path itself, then delegates to
            // `AIInternalMoveToState::onEnter`, which copies `ai->isWaitingForPath()`.
            o.ai.move_waiting_for_path = true;
        }
        self.pathfinder.queue_for_path(id);
        // The rest of `AIInternalMoveToState::onEnter` after its (skipped) adjust: `computePath` returns
        // without another request (`:2263-2278`), then `setPathExtraDistance(0)`/`setDesiredSpeed`
        // (`:1679-1680`).
        self.loco_start_move(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.move_try_one_more_repath = true;
            o.ai.path_extra_distance = 0.0;
            o.ai.desired_speed = crate::ai::FAST_AS_POSSIBLE;
            o.ai.movement_complete = false;
        }
        StateReturn::Continue
    }

    fn ai_goal_position(&self, id: ObjectId, m: &AiMachine) -> Option<Coord3D> {
        if m.goal_object != ObjectId::INVALID {
            if let Some(o) = self.objects.get(&m.goal_object) {
                return Some(o.position);
            }
        }
        let _ = id;
        Some(m.goal_position)
    }
}

// ---- AI_ATTACK_* / AIAttackState --------------------------------------------------------------

impl GameLogic {
    /// `AIUpdateInterface::aiAttackObject` / `privateAttackObject` (`AIUpdate.cpp:3426`): start an
    /// attack on `victim`. `follow` selects `AI_ATTACK_AND_FOLLOW_OBJECT`, `force` the force variant.
    /// The sub-machine's own attacks (a mood pickup, a hunt, a guard) do not change the command source;
    /// a command does (`setLastCommandSource`, `AIUpdate.cpp:3442`). This wrapper is the sub-machine's:
    /// the sim's own engine-side callers pass `CMD_FROM_AI` (`AIStates.cpp:1446`).
    pub(crate) fn ai_attack_object(&mut self, id: ObjectId, victim: ObjectId, force: bool, follow: bool, max_shots: Option<(i32, usize)>) {
        self.ai_attack_object_from(id, victim, force, follow, max_shots, CommandSource::Ai);
    }

    /// [`Self::ai_attack_object`] with the engine's `CommandSourceType` (`aiAttackObject(victim,
    /// maxShots, cmdSource)`): a player, script or AI order records it as the AI's last command source
    /// (`privateAttackObject` -> `setLastCommandSource`, AIUpdate.cpp:3442).
    pub(crate) fn ai_attack_object_from(
        &mut self,
        id: ObjectId,
        victim: ObjectId,
        force: bool,
        follow: bool,
        max_shots: Option<(i32, usize)>,
        source: CommandSource,
    ) {
        // `AssaultTransportAIUpdate::aiDoCommand`'s AICMD_ATTACK_OBJECT branch for a command not from the AI: reset, then the
        // transport attacks an object (AssaultTransportAIUpdate.cpp:93-96).
        self.assault_command(id, crate::assault_transport::AssaultCommand::AttackObject, source);
        self.ai_start_attack_sourced(id, AttackTarget::Object(victim), force, follow, max_shots, AiStateType::Idle, Some(source));
    }

    /// `AIUpdateInterface::aiAttackPosition` / `privateAttackPosition` (`AIUpdate.cpp:3515`) with the
    /// engine's `CommandSourceType` (`privateAttackPosition` -> `setLastCommandSource`, AIUpdate.cpp:3577).
    pub(crate) fn ai_attack_position_order_from(&mut self, id: ObjectId, pos: Coord3D, follow: bool, source: CommandSource) {
        // AICMD_ATTACK_POSITION has no case in `AssaultTransportAIUpdate::aiDoCommand`, so it takes the default branch: reset
        // for a command not from the AI (AssaultTransportAIUpdate.cpp:112-115).
        self.assault_command(id, crate::assault_transport::AssaultCommand::Other, source);
        self.ai_start_attack_sourced(id, AttackTarget::Ground(pos), false, follow, None, AiStateType::Idle, Some(source));
    }

    /// Shared `privateAttack*`: set the goal, build the `AttackOrder`, then `setState`. `ret != Idle`
    /// is one of the enclosing states' own attacks (the attack sub-machine's `setState`, not a command).
    pub(crate) fn ai_start_attack(&mut self, id: ObjectId, target: AttackTarget, force: bool, follow: bool, max_shots: Option<(i32, usize)>, ret: AiStateType) {
        self.ai_start_attack_sourced(id, target, force, follow, max_shots, ret, None);
    }

    /// [`Self::ai_start_attack`] with an explicit source for the command entry points. The engine's
    /// `privateAttack*` always write `m_lastCommandSource` (`setLastCommandSource`, AIUpdate.cpp:3442,
    /// :3460, :3577); the attack-move's mood pickup also sets `CMD_FROM_AI` (`AIStates.cpp:3658`).
    #[allow(clippy::too_many_arguments)] // the privateAttack* parameters plus the port's return/source seams
    fn ai_start_attack_sourced(
        &mut self,
        id: ObjectId,
        target: AttackTarget,
        force: bool,
        follow: bool,
        max_shots: Option<(i32, usize)>,
        ret: AiStateType,
        source: Option<CommandSource>,
    ) {
        // `privateAttackObject`/`privateForceAttackObject`/`privateAttackPosition`
        // (`AIUpdate.cpp:3440,3458,3574`) clear the machine before setting the goal. The
        // sub-machine's own attacks (`ret != Idle`) are transitions, not commands, so they keep the
        // current machine. A jet's `aiDoCommand` override handles its own state.
        if ret == AiStateType::Idle && !self.is_jet(id) {
            self.ai_clear(id);
        }
        let is_object = matches!(target, AttackTarget::Object(_));
        let object_pos = match target {
            AttackTarget::Object(v) => Some(self.objects.get(&v).map_or(Coord3D::ZERO, |o| o.position)),
            AttackTarget::Ground(_) => None,
        };
        let order = match max_shots {
            Some((n, slot)) => AttackOrder::with_limit(target, n, slot),
            None if force => AttackOrder::new_force(target),
            None => AttackOrder::new(target),
        };
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.attack_is_object = is_object;
            // The previous attack's exit cleared the current victim (`AIAttackState::onExit`, AIStates.cpp:5739).
            o.ai.machine.current_victim = ObjectId::INVALID;
            o.ai.machine.attack_force = force;
            o.ai.machine.attack_follow = follow;
            o.ai.machine.attack_return = ret;
            o.ai.machine.attack_initial_approach = true;
            o.ai.machine.attack_stop_if_in_range = false;
            // A command sets its source before `setState` (`privateAttackObject` and friends, `AIUpdate.cpp:3426-3446`).
            // A sub-machine pickup sets it after (`AIStates.cpp:3655-3658`, below).
            if let (Some(source), AiStateType::Idle) = (source, ret) {
                o.ai.machine.last_command_source = source;
            }
            match target {
                AttackTarget::Object(v) => {
                    o.ai.machine.goal_object = v;
                    o.ai.machine.goal_position = object_pos.unwrap_or(Coord3D::ZERO);
                }
                AttackTarget::Ground(p) => {
                    o.ai.machine.goal_object = ObjectId::INVALID;
                    o.ai.machine.goal_position = p;
                }
            }
            o.combat.attack = Some(order);
            o.combat.force_attack = force;
            // No stop, for a command or a pickup. `privateAttackObject` (AIUpdate.cpp:3426-3446) is `clear()` (whose
            // `onExit(EXIT_RESET)` runs the move's `friend_endingMove`, AIStates.cpp:1736-1750) then `setState`; nothing
            // there stops the unit. The path and the locomotor goal are dropped by the completed-move block of the next
            // update (AIUpdate.cpp:1035-1061, `ai_end_completed_move`), not at the command. The sub-machine pickups
            // (AIStates.cpp:3653-3655, :7260, :7405, :4499) make no stop either.
        }
        let state = if force {
            AiStateType::ForceAttackObject
        } else if follow {
            AiStateType::AttackAndFollowObject
        } else if is_object {
            AiStateType::AttackObject
        } else {
            AiStateType::AttackPosition
        };
        // `aiAttackObject` / `aiAttackPosition` are commands (`aiDoCommand`); the attacks the hunt /
        // guard / attack-move sub-machines start for themselves (`ret != Idle`) are not.
        if ret == AiStateType::Idle {
            self.ai_command_set_state(id, state);
        } else {
            // The attack-move's own attack (`ret == AttackMoveTo`) is the sub-machine going from `AI_IDLE` to an
            // attack: the attack-move state is not exited, so its exit draw is skipped (AIStates.cpp:3655).
            let handoff = ret == AiStateType::AttackMoveTo;
            if handoff {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.attack_move_handoff = true;
                }
            }
            // The enter (`ai_attack_enter`) runs the attack's entry chain (AIStates.cpp:5586) for every caller.
            self.ai_set_state(id, state);
            if handoff {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.attack_move_handoff = false;
                }
            }
            // `friend_setLastCommandSource(CMD_FROM_AI)` after the `setState` (AIStates.cpp:3655-3658).
            if let (Some(source), false) = (source, ret == AiStateType::Idle) {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.last_command_source = source;
                }
            }
        }
    }

    /// `AIAttackState::onEnter` (`AIStates.cpp:5516`): record the original victim position and the
    /// locked-weapon state; the sub-machine itself is the existing per-frame combat pass.
    fn ai_attack_enter(&mut self, id: ObjectId, _state: AiStateType) -> StateReturn {
        let m = match self.objects.get(&id) {
            Some(o) => o.ai.machine.clone(),
            None => return StateReturn::Failure,
        };
        if !self.is_able_to_attack(id) || self.objects.get(&id).is_none_or(|o| o.combat.weapons.is_empty()) {
            return StateReturn::Failure;
        }
        // `AIAttackState::onEnter` (`AIStates.cpp:5541`): "if all of our weapons are out of ammo,
        // can't attack. (this can happen for units which never auto-reload, like the Raptor)"
        if self.is_out_of_ammo(id) && !self.objects.get(&id).is_some_and(|o| o.is_kind_of(kind_of::PROJECTILE)) {
            return StateReturn::Failure;
        }
        let orig = if m.attack_is_object {
            let victim_gone = self.objects.get(&m.goal_object).is_none_or(|v| v.is_dying());
            if victim_gone {
                // `ai->notifyVictimIsDead()` (`AIStates.cpp:5556`).
                self.jet_notify_victim_is_dead(id);
            }
            let Some(v) = self.objects.get(&m.goal_object) else { return StateReturn::Failure };
            if v.is_dying() {
                return StateReturn::Failure;
            }
            v.position
        } else {
            m.goal_position
        };
        if let Some(o) = self.objects.get_mut(&id) {
            if o.combat.attack.is_none() {
                o.combat.attack = Some(AttackOrder::new(if m.attack_is_object { AttackTarget::Object(m.goal_object) } else { AttackTarget::Ground(m.goal_position) }));
            }
            o.combat.force_attack = m.attack_force;
            o.ai.machine.attack_original_victim_pos = orig;
            o.ai.machine.attack_initial_approach = true;
            o.ai.machine.attack_stop_if_in_range = false;
            // `m_lockedWeaponOnEnter = source->isCurWeaponLocked() ? ...` (AIStates.cpp:5462, :5584): the weapon
            // set's lock, which a temporary or permanent lock sets (`WeaponSet::setWeaponLock`).
            o.ai.machine.locked_weapon_on_enter = o.combat.weapons.is_cur_weapon_locked();
        }
        // `AIAttackState::onEnter`'s CONTINUE path runs the sub-machine's entry: `m_attackMachine->initDefaultState()`
        // (AIStates.cpp:5586). That is the one place the attack's entry chain runs for every caller (command, the
        // attack-move, hunt, attack-area and attack-follow pickups): the enter has already set the state, so an entry that
        // ends the attack here leaves the outer state changed and the caller skips its own transition
        // (`internalSetState`, StateMachine.cpp:635). The entry is reached only when the set took effect and the enter
        // did not fail (the failures above return before this point).
        self.ai_attack_enter_pass(id);
        StateReturn::Continue
    }

    /// `AIAttackState::update` (`AIStates.cpp:5600`): run the attack sub-machine (the combat pass).
    fn ai_attack_update(&mut self, id: ObjectId) -> StateReturn {
        if self.objects.get(&id).is_none_or(|o| o.combat.attack.is_none()) {
            return StateReturn::Success;
        }
        // `AIAttackState::update` (`AIStates.cpp:5613`): "if all of our weapons are out of ammo, can't attack."
        if self.is_out_of_ammo(id) && !self.objects.get(&id).is_some_and(|o| o.is_kind_of(kind_of::PROJECTILE)) {
            return StateReturn::Failure;
        }
        if let Some(r) = self.hunt_rescan_during_attack(id) {
            return r;
        }
        let victim = self.objects.get(&id).and_then(|o| match o.combat.attack.as_ref().map(|a| a.target) {
            Some(AttackTarget::Object(v)) => Some(v),
            _ => None,
        });
        self.ai_attack_update_object(id);
        if self.objects.get(&id).is_some_and(|o| o.combat.attack.is_some()) {
            StateReturn::Continue
        } else {
            // `ai->notifyVictimIsDead()` (`AIStates.cpp:5624`): the victim is gone (a jet "lands when the target is gone").
            if victim.is_some_and(|v| self.objects.get(&v).is_none_or(|o| o.is_dying())) {
                self.jet_notify_victim_is_dead(id);
            }
            StateReturn::Success
        }
    }

}

// ---- AI_ATTACK_MOVE_TO ------------------------------------------------------------------------

impl GameLogic {
    /// `AIAttackMoveStateMachine`'s `AI_IDLE` entry (`AIStates.cpp:1313`), as the attack-move's and the
    /// attack-follow's sub-machine make it: `AIIdleState::onEnter` resets the owner's mood check
    /// (`AIUpdate.cpp:4498`, `:1321`) and draws its sleep offset (`:1327`). Only that is ported: the
    /// sub-state's `update` never runs (both enclosing states update their sub-machine only while it is not
    /// idle, `AIStates.cpp:3621`, `:4461`), and `m_inited` and the initial sleep offset belong to the
    /// sub-state object, not to the owner's main idle.
    fn ai_attack_sub_idle_enter(&mut self, id: ObjectId) {
        self.reset_next_mood_check_time(id);
        let _ = self.rng.value(0, IDLE_COUNTDOWN_DELAY as i32);
    }

    /// `AIAttackMoveToState::onExit` (`AIStates.cpp:3598`): `m_attackMoveMachine->setState(AI_IDLE)`, which exits
    /// the sub-machine's current state and enters its `AI_IDLE` (a draw), then the move's exit. A hand-off to the
    /// sub-machine's own attack is not an exit (see [`AiMachine::attack_move_handoff`]).
    fn ai_attack_move_exit(&mut self, id: ObjectId) {
        let handoff = self.objects.get(&id).is_some_and(|o| o.ai.machine.attack_move_handoff);
        if !handoff {
            self.ai_attack_sub_idle_enter(id);
        }
        self.ai_internal_move_exit(id);
    }

    /// `AIAttackMoveToState::onEnter` (`AIStates.cpp:3584`). A fresh entry does `m_attackMoveMachine->clear()` and
    /// `setState(AI_IDLE)` (one `AI_IDLE` draw) and resets the retry count and the retry sleep. Returning from an
    /// attack (`resuming`) does neither: the sub-machine's attack ending already entered its `AI_IDLE`
    /// (see [`ai_attack_exit`](Self::ai_attack_exit)), and `m_retryCount` / `m_frameToSleepUntil` keep their values
    /// (the C++ `update` re-enters only the sub-machine). It restores `m_commandSrc` (`:3631`) on that frame.
    fn ai_attack_move_enter(&mut self, id: ObjectId) -> StateReturn {
        let resuming = self.objects.get(&id).is_some_and(|o| o.ai.machine.attack_move_resume);
        if !resuming {
            self.ai_attack_sub_idle_enter(id);
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.attack_move_retry = ATTACK_RETRY_COUNT;
                o.ai.machine.attack_move_sleep_until = 0;
                // `m_commandSrc = ai->getLastCommandSource()` (`AIStates.cpp:3590`), saved after the order
                // wrote it (`privateAttackMoveToPosition`, AIUpdate.cpp:3608).
                o.ai.machine.attack_move_command_source = o.ai.machine.last_command_source;
            }
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.attack_return = AiStateType::AttackMoveTo;
            // Resuming after an attack: the sub-machine's goal was the victim, the attack-move's
            // own goal is unchanged (see `AiMachine::attack_move_goal`).
            if resuming {
                o.ai.machine.attack_move_resume = false;
                o.ai.machine.goal_position = o.ai.machine.attack_move_goal;
                o.ai.machine.goal_object = ObjectId::INVALID;
                // `ai->friend_setLastCommandSource(m_commandSrc)` (`AIStates.cpp:3631`) — the attack the
                // sub-machine ran set `CMD_FROM_AI` (`:3658`); the player's attack-move takes it back.
                o.ai.machine.last_command_source = o.ai.machine.attack_move_command_source;
            }
        }
        // `AIAttackMoveToState::update` (`AIStates.cpp:3621-3661`): on the frame its attack sub-machine became idle,
        // `forceRetargetThisFrame` makes the scan `getNextMoodTarget(false, false)` — no mood timer, no team
        // victim — and `shouldRepathThisFrame` is cleared, so a target found here is attacked instead of repathing.
        // The sim's attack is a main state, so the enclosing state is re-entered when it ends.
        if resuming {
            if let Some(t) = self.ai_force_retarget_scan(id) {
                // The next attack is again one of this state's: keep the resume marker for the frame it ends.
                if let Some(o) = self.objects.get_mut(&id) {
                    // `ai->friend_endingMove()` (AIStates.cpp:3653): the same hand-off as the update's scan.
                    o.ai.friend_ending_move();
                    o.ai.machine.attack_move_goal = o.ai.machine.goal_position;
                    o.ai.machine.attack_move_resume = true;
                }
                self.ai_start_attack_sourced(id, AttackTarget::Object(t), false, false, None, AiStateType::AttackMoveTo, Some(CommandSource::Ai));
                return StateReturn::Continue;
            }
            // `if (m_frameToSleepUntil>TheGameLogic->getFrame()) return STATE_CONTINUE;` (`AIStates.cpp:3663-3664`):
            // the retry sleep is still running, so this frame only scans (above) — no move entry, no repath.
            let sleeping = self.objects.get(&id).map_or(0, |o| o.ai.machine.attack_move_sleep_until);
            if sleeping > self.frame {
                return StateReturn::Continue;
            }
            // `shouldRepathThisFrame` was set by the resume itself (`:3623-3626`), so the same frame runs
            // `AIMoveToState::onEnter(); forceRepath();` and then the move's update (`:3669-3674`). The enter
            // requests a path (and `computePath` sets the state's `m_waitingForPath`, `:1604`), so the update's
            // wait block stamps `m_pathTimestamp = now` (`:1789`) and returns while the request is outstanding;
            // `forceRepath`'s own two writes are overwritten by that wait block (`:1803-1804`) before the repath
            // gate (`:1879`) reads them, so they cannot cause a second request. What the pair changes is the
            // state's own timestamp and the update's same-frame work. Lane 173 did this for the sleep wake only.
            return self.ai_attack_move_repath_and_update(id);
        }
        self.ai_move_to_enter(id)
    }

    /// `AIMoveToState::onEnter(); forceRepath();` followed by the move's update in the same frame
    /// (`AIStates.cpp:3669-3674`). The attack move runs this on the frame it resumes after its attack and on
    /// its retry-sleep wake frame; `onEnter`'s result is discarded (`:3672`), so the update's is the state's.
    fn ai_attack_move_repath_and_update(&mut self, id: ObjectId) -> StateReturn {
        let _ = self.ai_move_to_enter(id);
        self.ai_force_repath(id);
        self.ai_attack_move_move_update(id)
    }

    /// `AIAttackState::onExit` (`AIStates.cpp:5714`). For an attack the attack-move's or the attack-follow's
    /// sub-machine runs (`attack_return` is the enclosing state) the attack ends, then the sub-machine enters
    /// `AI_IDLE`: by its success / failure transition (`AI_ATTACK_OBJECT` defined with `AI_IDLE` targets,
    /// `AIStates.cpp:1999`), or by the enclosing state's `setState(AI_IDLE)` on exit (`:3600`, `:4520`). Either
    /// way that entry draws once. An attack the sub-machine does not own draws nothing.
    pub(crate) fn ai_attack_exit(&mut self, id: ObjectId) {
        let owner = self.objects.get(&id).map(|o| o.ai.machine.attack_return);
        // The attack's sub-machine leaves through `AIAttackState::onExit` as an internal transition: it does not notify
        // (AIStates.cpp:5714-5745, StateMachine.cpp:126-140), so no `wakeUpNow` follows here (lane 191 R28 F4). The caller
        // that clears or changes the machine notifies for its own transition.
        self.end_attack_internal(id);
        if matches!(owner, Some(AiStateType::AttackMoveTo) | Some(AiStateType::AttackFollowWaypointPath)) {
            self.ai_attack_sub_idle_enter(id);
        }
    }

    /// `AIAttackMoveToState::update` (`AIStates.cpp:3605`): scan for a target while moving; if one
    /// is found, attack it (returning to this state when the attack finishes).
    fn ai_attack_move_update(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        // `AIAttackMoveToState::update` (`AIStates.cpp:3614`): "We need to return to base to reload!"
        if self.is_jet(id) && self.jet_is_out_of_special_reload_ammo(id) {
            return StateReturn::Success;
        }
        // "Check to see if we have created a crate we need to pick up" (`AIStates.cpp:3641`): never a crate.
        let _ = self.check_for_crate_to_pickup(id);
        // The scan runs before the retry sleep (`AIStates.cpp:3637-3662`): "We can attack during these frames, just
        // not move."
        if let Some(t) = self.ai_idle_scan(id, false, None, crate::orders::GuardMode::Normal, true) {
            if let Some(o) = self.objects.get_mut(&id) {
                // `ai->friend_endingMove()` (AIStates.cpp:3653) before `setState(AI_ATTACK_OBJECT)`.
                o.ai.friend_ending_move();
                o.ai.machine.attack_move_goal = o.ai.machine.goal_position;
                o.ai.machine.attack_move_resume = true;
            }
            // "Note that we picked up this command from the ai." (`AIStates.cpp:3658`).
            self.ai_start_attack_sourced(id, AttackTarget::Object(t), false, false, None, AiStateType::AttackMoveTo, Some(CommandSource::Ai));
            return StateReturn::Continue;
        }
        let sleeping = self.objects.get(&id).map_or(0, |o| o.ai.machine.attack_move_sleep_until);
        if sleeping > now {
            return StateReturn::Continue;
        }
        // The wake frame (`AIStates.cpp:3664-3674`): `m_frameToSleepUntil == now` sets `shouldRepathThisFrame`, and
        // then `AIMoveToState::onEnter(); forceRepath();` run *before* the update. The entry computes a fresh path
        // (`AIStates.cpp:1671`), which is what gets the state moving again; without it the update only re-runs the
        // finished path, fails the retry distance test and sleeps again. The enter's result is discarded (`:3672`).
        if sleeping == now {
            return self.ai_attack_move_repath_and_update(id);
        }
        self.ai_attack_move_move_update(id)
    }

    /// `StateReturnType ret = AIMoveToState::update();` and the retry tail of `AIAttackMoveToState::update`
    /// (`AIStates.cpp:3674-3690`).
    fn ai_attack_move_move_update(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        let ret = self.ai_internal_move_update(id);
        if ret != StateReturn::Continue {
            // `m_retryCount`: if we stopped short of the goal, sleep 3 s and try again.
            let (pos_goal, pos, retry) = match self.objects.get(&id) {
                Some(o) => (o.ai.machine.attack_move_path_goal, o.position, o.ai.machine.attack_move_retry),
                None => return ret,
            };
            let dx = pos.x - pos_goal.x;
            let dy = pos.y - pos_goal.y;
            let close = dx * dx + dy * dy < (ATTACK_CLOSE_ENOUGH_CELLS * crate::pathfind::PATHFIND_CELL_SIZE_F).powi(2);
            if retry < 1 || close {
                return ret;
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.attack_move_retry -= 1;
                o.ai.machine.attack_move_sleep_until = now + 3 * LOGICFRAMES_PER_SECOND;
            }
            return StateReturn::Continue;
        }
        ret
    }
}

// ---- AI_HUNT / AI_ATTACK_AREA -----------------------------------------------------------------

impl GameLogic {
    /// `AIHuntState::onEnter` / `AIAttackAreaState::onEnter` (`AIStates.cpp:7144`, `7348`).
    fn ai_hunt_enter(&mut self, id: ObjectId, state: AiStateType) -> StateReturn {
        if !self.is_mobile(id) {
            return StateReturn::Failure;
        }
        let off = self.rng.value(0, ENEMY_SCAN_RATE as i32) as u32;
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.next_enemy_scan = self.frame + off;
            o.ai.machine.attack_return = state;
        }
        StateReturn::Continue
    }

    /// `AIHuntState::update` (`AIStates.cpp:7186`): scan the whole map every `ENEMY_SCAN_RATE`
    /// frames.
    fn ai_hunt_update(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        if self.objects.get(&id).is_some_and(|o| now >= o.ai.machine.next_enemy_scan) {
            match self.hunt_scan(id) {
                Err(()) => return StateReturn::Failure,
                Ok(Some(t)) => self.ai_start_attack(id, AttackTarget::Object(t), false, false, None, AiStateType::Hunt),
                Ok(None) => {
                    // "If we are not doing an all hunt, then exit hunt state - no more victims"
                    // (`AIStates.cpp:7262`): `Player::getUnitsShouldHunt` is only set by the
                    // `PLAYER_HUNT` script action (never used by the skirmish scripts), and the hunt
                    // sub-machine is idle here (an attack is its own state in the sim), so the
                    // state succeeds and the unit goes idle.
                    if !self.player_units_should_hunt(id) {
                        return StateReturn::Success;
                    }
                }
            }
        }
        StateReturn::Continue
    }

    /// The scan block of `AIHuntState::update` (`AIStates.cpp:7192-7258`): `Err` is `STATE_FAILURE`
    /// (out of ammo), `Ok(victim)` the goal object it sets on the hunt sub-machine.
    fn hunt_scan(&mut self, id: ObjectId) -> Result<Option<ObjectId>, ()> {
        let now = self.frame;
        {
            // "if all of our weapons are out of ammo, can't hunt" (`AIStates.cpp:7199`).
            // `WeaponSet::isOutOfAmmo` is true for an empty set too.
            let out_of_ammo = self.objects.get(&id).is_some_and(|o| {
                !o.is_kind_of(kind_of::PROJECTILE)
                    && o.combat.weapons.weapons.iter().flatten().all(|w| w.status(now) == crate::weapon::WeaponStatus::OutOfAmmo)
            });
            if out_of_ammo {
                return Err(());
            }
            // "Check to see if we have created a crate we need to pick up" (`AIStates.cpp:7202`):
            // never yields a crate (see `check_for_crate_to_pickup`).
            let _ = self.check_for_crate_to_pickup(id);
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.next_enemy_scan = now + ENEMY_SCAN_RATE;
            }
            // "Check if team auto targets same victim" (`AIStates.cpp:7217`). With a null attack info a
            // team victim wins outright; otherwise the closest/priority enemy is found and the team
            // victim replaces it unless the new one has a strictly higher priority.
            let info = self.objects.get(&id).and_then(|o| o.ai.attack_info.clone());
            let common = self.team_attacks_common_target(id);
            let team_victim = if common { self.get_team_target_object(id) } else { None };
            let victim = if team_victim.is_some() && info.is_none() {
                team_victim
            } else {
                let mut v = self.ai_idle_scan(id, true, None, crate::orders::GuardMode::Normal, true);
                if common {
                    if let (Some(tv), Some(set)) = (team_victim, info.as_deref()) {
                        if v.is_none() {
                            v = Some(tv);
                        }
                        let tv_priority = self.attack_priority_of(set, tv);
                        let v_priority = v.map_or(0, |x| self.attack_priority_of(set, x));
                        if tv_priority >= v_priority {
                            v = Some(tv);
                        }
                    }
                    self.set_team_target_object(id, v);
                }
                v
            };
            Ok(victim)
        }
    }

    /// The part of `AIHuntState::update` / `AIAttackAreaState::update` that runs while their attack
    /// sub-state is active (`AIStates.cpp:7192`, `7372`): every `ENEMY_SCAN_RATE` frames the scan is
    /// repeated and its victim becomes the sub-machine's goal object, which `AIAttackState::update`
    /// adopts ("Our parent machine has changed the goal", `AIStates.cpp:5675`). In the sim an attack
    /// is its own state whose `attack_return` is the hunt state, so the hunt's re-scan lives here.
    /// `Some(ret)` ends the attack update with `ret`.
    fn hunt_rescan_during_attack(&mut self, id: ObjectId) -> Option<StateReturn> {
        let now = self.frame;
        let (ret, is_object, force, follow, next, goal) = {
            let m = &self.objects.get(&id)?.ai.machine;
            (m.attack_return, m.attack_is_object, m.attack_force, m.attack_follow, m.next_enemy_scan, m.goal_object)
        };
        if !is_object || force || follow || now < next || !matches!(ret, AiStateType::Hunt | AiStateType::AttackArea) {
            return None;
        }
        let victim = if ret == AiStateType::Hunt {
            match self.hunt_scan(id) {
                Err(()) => return Some(StateReturn::Failure),
                Ok(v) => v,
            }
        } else {
            let area = self.objects.get(&id).and_then(|o| o.combat.hunt_area);
            self.objects.get_mut(&id)?.ai.machine.next_enemy_scan = now + ENEMY_SCAN_RATE;
            let Some(area) = area else { return Some(StateReturn::Failure) };
            self.ai_idle_scan(id, true, Some(area), crate::orders::GuardMode::Normal, true)
        };
        // A null victim ends the attack (`victim == nullptr` -> `STATE_SUCCESS`); the hunt state then
        // rescans and exits.
        let Some(t) = victim else { return Some(StateReturn::Success) };
        if t != goal {
            let pos = self.objects.get(&t).map_or(Coord3D::ZERO, |v| v.position);
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.goal_object = t;
                o.ai.machine.goal_position = pos;
                o.ai.machine.attack_original_victim_pos = pos;
                o.combat.attack = Some(AttackOrder::new(AttackTarget::Object(t)));
            }
        }
        None
    }

    /// `Player::getUnitsShouldHunt` of the unit's controlling player (`PLAYER_HUNT`, not ported).
    fn player_units_should_hunt(&self, _id: ObjectId) -> bool {
        false
    }

    /// `AIAttackAreaState::update` (`AIStates.cpp:7372`): hunt within the guarded area.
    fn ai_attack_area_update(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        if self.objects.get(&id).is_some_and(|o| now >= o.ai.machine.next_enemy_scan) {
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.next_enemy_scan = now + ENEMY_SCAN_RATE;
            }
            let area = self.objects.get(&id).and_then(|o| o.combat.hunt_area);
            if area.is_none() {
                return StateReturn::Failure;
            }
            if let Some(t) = self.ai_idle_scan(id, true, area, crate::orders::GuardMode::Normal, true) {
                self.ai_start_attack(id, AttackTarget::Object(t), false, false, None, AiStateType::AttackArea);
            } else {
                return StateReturn::Success;
            }
        }
        StateReturn::Continue
    }
}

// ---- AI_GUARD / AIGuardMachine -----------------------------------------------------------------

impl GameLogic {
    /// `AIGuardState::onEnter` (`AIStates.cpp:6733`): the machine starts in `AI_GUARD_RETURN`.
    fn ai_guard_enter(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        // The first draw is the RETURN state's onEnter reached through `initDefaultState`'s chain
        // (INNER -> OUTER -> GET_CRATE -> RETURN, `AIGuard.cpp:642`), so it uses the return rate.
        let off = self.rng.value(0, self.ai_tunables.guard_enemy_return_scan_rate as i32) as u32;
        if let Some(o) = self.objects.get_mut(&id) {
            // `setTargetPositionToGuard` / `setTargetToGuard` (AIStates.cpp:6744-6747). An area guard only gets
            // `setAreaToGuard`, so `m_positionToGuard` keeps its constructor value (0,0,0; AIGuard.cpp:168).
            // The polygon's centre is read where C++ reads `getCenterPoint` (see `ai_guard_centre`).
            o.ai.machine.guard.position_to_guard = if o.ai.machine.guard.area.is_some() { Coord3D::ZERO } else { o.ai.machine.goal_position };
            o.ai.machine.guard.target_to_guard = o.ai.machine.goal_object;
            o.ai.machine.guard.attack_center = o.ai.machine.goal_position;
            // `AIGuardIdleState::m_guardeePos` has no constructor initialiser (`AIGuard.h:181`): the
            // `MemoryPool` *zeroes* every block it hands out (`operator new` -> `allocateBlockImplementation`
            // -> `memset(p, 0, getAllocationSize())`, GameMemory.h:619-631, GameMemory.cpp:1705-1711), so a
            // freshly built `AIGuardMachine`'s idle state starts at (0,0,0), not at the guarded object's
            // position. Only `update` ever writes it (`AIGuard.cpp:764`, `:769`). The value survives idle
            // re-entries for the machine's lifetime.
            o.ai.machine.guard.guardee_pos = Coord3D::ZERO;
            o.ai.machine.guard.next_scan_time = now + off;
            o.ai.machine.guard_attack_active = false;
            o.ai.machine.guard_sub = AiStateType::GuardReturn;
        }
        // `AIGuardState::onEnter` returns `setState(AI_GUARD_RETURN)`.
        self.ai_guard_return_enter(id)
    }

    /// `AIGuardState::update` (`AIStates.cpp:6769`): run the guard sub-machine.
    fn ai_guard_update(&mut self, id: ObjectId) -> StateReturn {
        // `AIGuardState::update` (`AIStates.cpp:6781`): "if all of our weapons are out of ammo, can't
        // attack. (this can happen for units which never auto-reload, like the Raptor)"
        if self.is_jet(id)
            && self.is_out_of_ammo(id)
            && !self.objects.get(&id).is_some_and(|o| o.is_kind_of(kind_of::PROJECTILE))
        {
            return StateReturn::Failure;
        }
        let sub = self.objects.get(&id).map(|o| o.ai.machine.guard_sub).unwrap_or(AiStateType::GuardIdle);
        // The guard sub-machine manages its own transitions; the enclosing `AI_GUARD` state never
        // returns success/failure from them (it only exits via an order). Its *sleep* is the
        // machine's own, though: `AIGuardState::update` returns
        // `m_guardMachine->updateStateMachine()` (`AIStates.cpp:6793-6798`), so a sub-state that
        // returns `STATE_SLEEP` (`AIGuardIdleState::update`, `AIGuard.cpp:731`, `:777`) sleeps the
        // whole `AIUpdateInterface` module for that many frames (`AIUpdate.cpp:1011-1037`), which is
        // the module's sleepy-heap key. Dropping the sub-state's return kept every idle guard awake
        // every frame (AN1 s1 f2688, lane 170).
        let ret = match sub {
            AiStateType::GuardReturn => self.ai_guard_return_update(id),
            AiStateType::GuardIdle => self.ai_guard_idle_update(id),
            AiStateType::GuardInner => self.ai_guard_inner_update(id),
            AiStateType::GuardOuter => self.ai_guard_outer_update(id),
            AiStateType::GuardAttackAggressor => self.ai_guard_aggressor_update(id),
            _ => StateReturn::Continue,
        };
        match ret {
            StateReturn::Sleep(frames) => StateReturn::Sleep(frames),
            // A transition inside the guard machine is consumed there (`StateMachine.cpp:452-457`,
            // `:112-176`): the enclosing state sees the new state's `onEnter` result, a CONTINUE.
            _ => StateReturn::Continue,
        }
    }

    fn ai_guard_state_exit(&mut self, id: ObjectId, _state: AiStateType) {
        // `AIGuardState::onExit` deletes the guard machine (`AIStates.cpp:6759-6766`), and
        // `StateMachine::~StateMachine` runs the current state's `onExit(EXIT_RESET)`
        // (`StateMachine.cpp:281-288`). Leaving the guard state while its `AI_GUARD_RETURN` sub-state
        // is active therefore runs `AIGuardReturnState::onExit` -> `AIInternalMoveToState::onExit`
        // (`AIGuard.cpp:684-687`) -> `friend_endingMove`, so the return move's path ends and the
        // completed-move block drops it.
        if self.objects.get(&id).is_some_and(|o| o.ai.machine.guard_sub == AiStateType::GuardReturn) {
            self.ai_internal_move_exit(id);
        }
        // Leaving the guard machine exits its current sub-state: inner and aggressor clear the
        // team target in `onExit` (`AIGuard.cpp:486,900`).
        if self.objects.get(&id).is_some_and(|o| matches!(o.ai.machine.guard_sub, AiStateType::GuardInner | AiStateType::GuardAttackAggressor)) {
            self.set_team_target_object(id, None);
        }
        if self.objects.get(&id).is_some_and(|o| o.ai.machine.guard_attack_active) {
            // `AIGuardState::onExit` deletes the guard machine (AIStates.cpp:6759-6766), and the attack sub-state exits
            // through `AIAttackState::onExit` (`end_attack_with`: only an approach or pursue sub-state ends a move).
            self.end_attack_internal(id);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.guard_attack_active = false;
            o.ai.machine.in_guard_attack = false;
            o.ai.machine.in_guard_idle = false;
        }
    }

    /// `AIGuardMachine::findTargetToGuardByID` (`AIGuard.cpp`): the guarded object, or none.
    pub(crate) fn ai_guard_target(&self, id: ObjectId) -> Option<Coord3D> {
        let g = &self.objects.get(&id)?.ai.machine.guard;
        if g.target_to_guard != ObjectId::INVALID {
            if let Some(o) = self.objects.get(&g.target_to_guard) {
                return Some(o.position);
            }
        }
        Some(g.position_to_guard)
    }

    /// `AIGuardMachine::getStdGuardRange` (`AIGuard.cpp:203`): the adjusted vision range.
    /// `AI_VISIONFACTOR_OWNERTYPE | MOOD | GUARDINNER`.
    pub(crate) fn ai_guard_range(&self, id: ObjectId) -> f32 {
        self.adjusted_vision_range(id, true)
    }

    /// `getAdjustedVisionRangeForObject(OWNERTYPE | MOOD)` (`AIGuardOuterState::onEnter`,
    /// `AIGuard.cpp:544`): the outer-circle factors.
    pub(crate) fn ai_guard_outer_range(&self, id: ObjectId) -> f32 {
        self.adjusted_vision_range(id, false)
    }

    /// `AI::getAdjustedVisionRangeForObject(OWNERTYPE | MOOD [| GUARDINNER])` (`AI.cpp:817-880`):
    /// the object's vision range times the owner-type inner/outer factor; a contained object then
    /// uses its largest weapon range instead, and any other computer-controlled object applies the
    /// mood factor (Sleep returns 0).
    fn adjusted_vision_range(&self, id: ObjectId, inner: bool) -> f32 {
        let Some(o) = self.objects.get(&id) else { return 0.0 };
        let t = &self.ai_tunables;
        let ai_player = self.ai_players.contains_key(&o.player);
        let mut r = o.template.vision_range.max(0.0);
        r *= match (inner, ai_player) {
            (true, true) => t.guard_inner_modifier_ai,
            (true, false) => t.guard_inner_modifier_human,
            (false, true) => t.guard_outer_modifier_ai,
            (false, false) => t.guard_outer_modifier_human,
        };
        if o.contained_by.is_some() {
            return o.combat.weapons.weapons.iter().flatten().fold(0.0f32, |m, w| m.max(w.template.attack_range()));
        }
        if ai_player {
            match o.ai.attitude {
                a if a <= -2 => return 0.0,
                1 => r *= t.alert_range_modifier,
                2 => r *= t.aggressive_range_modifier,
                _ => {}
            }
        }
        r
    }

    /// `AIGuardMachine::lookForInnerTarget` (`AIGuard.cpp:212`).
    fn ai_guard_look_for_target(&mut self, id: ObjectId) -> Option<ObjectId> {
        if !self.is_able_to_attack(id) {
            return None;
        }
        // "Check if team auto targets same victim" (`AIGuard.cpp:222`): the team victim is the
        // nemesis without any further attackability test.
        if self.team_attacks_common_target(id) {
            if let Some(v) = self.get_team_target_object(id) {
                return Some(v);
            }
        }
        // `AIGuard.cpp:275-280`: an area guard's scan waits for a recent trigger-area change (lane 184).
        if self.objects.get(&id).is_some_and(|o| o.ai.machine.guard.area.is_some()) && !self.guard_area_scan_open() {
            return None;
        }
        let mode = self.objects.get(&id).map(|o| o.ai.machine.guard.mode).unwrap_or(crate::orders::GuardMode::Normal);
        // `findClosestEnemy` from the guard machine, not `getNextMoodTarget`: `calledByAI` does not apply.
        self.ai_scan(id, false, None, mode, true, true, true)
    }

    /// `AIGuardReturnState::onEnter` (`AIGuard.cpp:642`): the first scan is `GameLogicRandomValue(0,
    /// m_guardEnemyReturnScanRate)` (`:645`).
    fn ai_guard_return_enter(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        let off = self.rng.value(0, self.ai_tunables.guard_enemy_return_scan_rate as i32) as u32;
        // `AIGuardReturnState::onEnter` (AIGuard.cpp:652-657): the area's centre overrides the guarded position.
        let goal = self.ai_guard_centre(id).unwrap_or(Coord3D::ZERO);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.guard.next_scan_time = now + off;
            o.ai.machine.goal_position = goal;
            o.ai.machine.goal_object = ObjectId::INVALID;
        }
        let ret = self.ai_internal_move_enter(id, goal, true);
        if ret == StateReturn::Continue {
            // `AIGuardReturnState` carries `attackAggressors` in the retail branch (`AIGuard.cpp:184`): the move's
            // `onEnter` returns CONTINUE, so the condition runs right away (`StateMachine.cpp:647-651`).
            if self.ai_guard_aggressor_condition(id) {
                return StateReturn::Continue;
            }
        }
        ret
    }

    /// `AIGuardReturnState::update` (`AIGuard.cpp:669`).
    fn ai_guard_return_update(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        // `m_nextReturnScanTime` -> `lookForInnerTarget`.
        if self.objects.get(&id).is_some_and(|o| now >= o.ai.machine.guard.next_scan_time) {
            // `m_nextReturnScanTime = now + m_guardEnemyReturnScanRate` (`AIGuard.cpp:674`).
            let rate = self.ai_tunables.guard_enemy_return_scan_rate;
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.guard.next_scan_time = now + rate;
            }
            if let Some(t) = self.ai_guard_look_for_target(id) {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.guard.nemesis = t;
                    o.ai.machine.guard_sub = AiStateType::GuardInner;
                }
                // The C++ update returns `STATE_FAILURE` here (`AIGuard.cpp:676`), which is a real
                // `internalSetState`: `AIGuardReturnState::onExit` (`:684-687`) runs
                // `AIInternalMoveToState::onExit` -> `friend_endingMove` before `AI_GUARD_INNER`'s
                // `onEnter`. The port's direct sub-state call must run that exit itself.
                self.ai_internal_move_exit(id);
                self.ai_guard_inner_enter(id);
                return StateReturn::Continue;
            }
        }
        let ret = self.ai_internal_move_update(id);
        // `AIGuardReturnState`'s transitions (`AIGuard.cpp:184/189`): success -> `AI_GUARD_IDLE`,
        // failure -> `AI_GUARD_INNER`. Both are `internalSetState`, so the state's `onExit`
        // (`AIGuard.cpp:684-687`: the base move exit) runs first.
        match ret {
            StateReturn::Success => {
                self.ai_internal_move_exit(id);
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.guard_sub = AiStateType::GuardIdle;
                }
                self.ai_guard_idle_enter(id);
            }
            StateReturn::Failure => {
                self.ai_internal_move_exit(id);
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.guard_sub = AiStateType::GuardInner;
                }
                self.ai_guard_inner_enter(id);
            }
            _ => {}
        }
        // A CONTINUE return runs the state's conditions (`StateMachine.cpp:158-192`); `AIGuardReturnState` has
        // `attackAggressors` in the retail branch (`AIGuard.cpp:184`). A guard shot from outside its vision wakes up
        // and turns on the attacker here instead of walking on to its post (R19-3).
        if ret == StateReturn::Continue && self.ai_guard_aggressor_condition(id) {
            return StateReturn::Continue;
        }
        ret
    }

    /// `AIGuardIdleState::onEnter` (`AIGuard.cpp:718`): the first scan is `GameLogicRandomValue(0,
    /// m_guardEnemyScanRate)` (`:723`, `m_guardEnemyScanRate` = `LOGICFRAMES_PER_SECOND/2` by default).
    fn ai_guard_idle_enter(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        let off = self.rng.value(0, self.ai_tunables.guard_enemy_scan_rate as i32) as u32;
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.guard.next_scan_time = now + off;
        }
        // `onEnter` returns CONTINUE, so `internalSetState` runs the state's conditions (`StateMachine.cpp:647-651`):
        // a guard that entered `AI_GUARD_IDLE` returns fire at once (`AIGuard.cpp:191`).
        if self.ai_guard_aggressor_condition(id) {
            return StateReturn::Continue;
        }
        StateReturn::Continue
    }

    /// `AIGuardIdleState::update` (`AIGuard.cpp:729`).
    fn ai_guard_idle_update(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        let waiting = self
            .objects
            .get(&id)
            .filter(|o| now < o.ai.machine.guard.next_scan_time)
            .map(|o| o.ai.machine.guard.next_scan_time - now);
        if let Some(frames) = waiting {
            // `return STATE_SLEEP(m_nextEnemyScanTime - now)` (`AIGuard.cpp:731`): the guard sleeps
            // until its next scan. The sleep return runs the state's conditions
            // (`StateMachine.cpp:217` -> `friend_checkForSleepTransitions` `:203-252`), so a guard
            // attacked while it waits checks `attackAggressors` (`AIGuard.cpp:191`) here too — and a
            // transition clears the sleep (`internalSetState`, `:98`), so the machine runs on.
            if self.ai_guard_aggressor_condition(id) {
                return StateReturn::Continue;
            }
            return StateReturn::Sleep(frames);
        }
        // `m_nextEnemyScanTime = now + m_guardEnemyScanRate` (`AIGuard.cpp:737`).
        let rate = self.ai_tunables.guard_enemy_scan_rate;
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.guard.next_scan_time = now + rate;
            o.ai.machine.in_guard_idle = true;
        }
        // "Check to see if we have created a crate we need to pick up" (`AIGuard.cpp:745`): a pending
        // crate id sends the machine to `AI_GUARD_GET_CRATE`, whose `onEnter` finds no crate
        // (`check_for_crate_to_pickup`) and succeeds into `AI_GUARD_RETURN`.
        if self.objects.get(&id).is_some_and(|o| o.ai.crate_created != ObjectId::INVALID) {
            let _ = self.check_for_crate_to_pickup(id);
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.in_guard_idle = false;
                o.ai.machine.guard_sub = AiStateType::GuardReturn;
            }
            self.ai_guard_return_enter(id);
            return StateReturn::Continue;
        }
        // `AIGuardIdleState::update`'s order (`AIGuard.cpp:745-777`): the crate, then
        // `lookForInnerTarget` (SUCCESS), then the guardee's move (FAILURE) and only then the sleep. The
        // `attackAggressors` condition is not tested between them: the engine evaluates a state's conditions on its
        // *return* (`StateMachine.cpp:457-461` -> `friend_checkForSleepTransitions`), so only the sleep return reaches
        // it (R19-4).
        if let Some(t) = self.ai_guard_look_for_target(id) {
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.guard.nemesis = t;
                o.ai.machine.guard_sub = AiStateType::GuardInner;
            }
            self.ai_guard_inner_enter(id);
            return StateReturn::Continue;
        }
        // See if the object we are guarding moved (`AIGuard.cpp:758-772`): only an *object* guard has a
        // guardee position. `targetToGuard = getGuardMachine()->findTargetToGuardByID()` is null for a
        // location or area guard, and the whole comparison is skipped (the position to guard is not the
        // guardee's last position).
        let target_to_guard = self
            .objects
            .get(&id)
            .map(|o| o.ai.machine.guard.target_to_guard)
            .filter(|t| *t != ObjectId::INVALID)
            .and_then(|t| self.objects.get(&t).map(|o| o.position));
        if let Some(pos) = target_to_guard {
            let moved = self.objects.get(&id).is_some_and(|o| {
                let g = o.ai.machine.guard.guardee_pos;
                let dx = g.x - pos.x;
                let dy = g.y - pos.y;
                dx * dx > 4.0 * crate::pathfind::PATHFIND_CELL_SIZE_F.powi(2) || dy * dy > 4.0 * crate::pathfind::PATHFIND_CELL_SIZE_F.powi(2)
            });
            if moved {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.guard.guardee_pos = pos;
                    o.ai.machine.guard_sub = AiStateType::GuardReturn;
                }
                self.ai_guard_return_enter(id);
                return StateReturn::Continue;
            }
        }
        // `AI_GUARD_IDLE`'s `attackAggressors` condition (`AIGuard.cpp:191`), on the state's sleep return
        // (`StateMachine.cpp:217`), so once per guard scan.
        if self.ai_guard_aggressor_condition(id) {
            return StateReturn::Continue;
        }
        // `return STATE_SLEEP(m_nextEnemyScanTime - now)` (`AIGuard.cpp:777`): after a scan the guard
        // sleeps `m_guardEnemyScanRate` frames, the value the scan above just wrote.
        StateReturn::Sleep(self.objects.get(&id).map_or(rate, |o| o.ai.machine.guard.next_scan_time.saturating_sub(now)))
    }

    /// `AIGuardInnerState::onEnter` (`AIGuard.cpp:395`): attack the nemesis inside the guard radius.
    fn ai_guard_inner_enter(&mut self, id: ObjectId) -> StateReturn {
        let nemesis = self.objects.get(&id).map(|o| o.ai.machine.guard.nemesis).unwrap_or(ObjectId::INVALID);
        if nemesis == ObjectId::INVALID {
            // `AIGuardInnerState::onEnter` (`AIGuard.cpp:395`): with no nemesis it is SUCCESS, and
            // SUCCESS goes to `AI_GUARD_OUTER` (`AIGuard.cpp:183`), not to idle.
            return self.ai_guard_inner_done(id);
        }
        let center = self.ai_guard_target(id).unwrap_or(Coord3D::ZERO);
        let range = self.ai_guard_range(id);
        // `AIGuardInnerState::onEnter` (`AIGuard.cpp:426`): `ATTACK_ExitIfOutsideRadius |
        // ATTACK_ExitIfNoUnitFound`, no duration.
        let exit = AttackExitConditions {
            exit_if_no_unit_found: true,
            exit_if_expired_duration: false,
            exit_if_outside_radius: true,
            center,
            radius_sqr: range * range,
            give_up_frame: 0,
        };
        // `AIAttackState::onEnter` (AIStates.cpp:5526-5527): a condition already met at entry is SUCCESS, with no
        // attack. The inner state then reports SUCCESS (AIGuard.cpp:441), which goes to the outer circle (AIGuard.cpp:183).
        if exit.should_exit(self.find_object(nemesis).map(|v| v.position), self.frame) {
            return self.ai_guard_inner_done(id);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.guard.attack_center = center;
            o.ai.machine.guard.attack_radius_sqr = range * range;
            o.ai.machine.guard_attack_active = true;
            o.ai.machine.in_guard_attack = true;
        }
        self.ai_guard_begin_attack(id, nemesis, exit, false);
        if self.objects.get(&id).is_some_and(|o| o.combat.attack.is_none()) {
            // The attack's entry failed, so `AIGuardInnerState::onEnter` returns SUCCESS (AIGuard.cpp:436-442).
            self.ai_guard_finish_attack(id);
            return self.ai_guard_inner_done(id);
        }
        // The attack's `onEnter` returned CONTINUE, so `internalSetState` runs `AI_GUARD_INNER`'s conditions
        // (`StateMachine.cpp:647-651`): `attackAggressors` in the retail branch (`AIGuard.cpp:183`).
        if self.ai_guard_aggressor_condition(id) {
            return StateReturn::Continue;
        }
        StateReturn::Continue
    }

    /// `AIGuardInnerState::update` (`AIGuard.cpp:446`).
    fn ai_guard_inner_update(&mut self, id: ObjectId) -> StateReturn {
        // `AIGuardInnerState::update` (`AIGuard.cpp:446`): with no attack state it returns SUCCESS.
        if !self.objects.get(&id).is_some_and(|o| o.ai.machine.guard_attack_active) {
            return self.ai_guard_inner_done(id);
        }
        // `ExitConditions::ATTACK_ExitIfNoUnitFound | ExitIfOutsideRadius`.
        let nemesis_ok = self.objects.get(&id).map(|o| o.ai.machine.guard.nemesis).filter(|n| *n != ObjectId::INVALID).and_then(|n| self.objects.get(&n)).is_some_and(|v| !v.is_dying());
        if !nemesis_ok {
            // `AIAttackState::update` (`AIStates.cpp:5622`) ends with SUCCESS for a dead victim, and
            // the inner state's SUCCESS goes to `AI_GUARD_OUTER` (`AIGuard.cpp:183`).
            self.ai_guard_finish_attack(id);
            return self.ai_guard_inner_done(id);
        }
        // `AIGuardInnerState::update` (`AIGuard.cpp:451`): follow the guarded object.
        self.ai_guard_refresh_exit_center(id);
        self.ai_attack_update_object(id);
        if self.objects.get(&id).is_some_and(|o| o.combat.attack.is_none()) {
            self.ai_guard_finish_attack(id);
            return self.ai_guard_inner_done(id);
        }
        // A CONTINUE return runs the state's conditions (`StateMachine.cpp:158-192`); `AIGuardInnerState` has
        // `attackAggressors` in the retail branch (`AIGuard.cpp:183`).
        if self.ai_guard_aggressor_condition(id) {
            return StateReturn::Continue;
        }
        StateReturn::Continue
    }

    /// `AIGuardInnerState` leaving with SUCCESS or FAILURE (`AIGuard.cpp:183`): its `onExit` clears the team
    /// target (`:486`), then `AI_GUARD_OUTER` runs (`AIGuard.cpp:525`).
    fn ai_guard_inner_done(&mut self, id: ObjectId) -> StateReturn {
        self.set_team_target_object(id, None);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.guard_sub = AiStateType::GuardOuter;
        }
        self.ai_guard_outer_enter(id)
    }

    /// `AI_GUARD_GET_CRATE` with no crate to fetch (`AIGuard.cpp:791`, `AIGuardPickUpCrateState::onEnter`):
    /// SUCCESS into `AI_GUARD_RETURN` (`AIGuard.cpp:193`), whose `onEnter` draws its first scan.
    fn ai_guard_get_crate_then_return(&mut self, id: ObjectId) -> StateReturn {
        let _ = self.check_for_crate_to_pickup(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.guard_sub = AiStateType::GuardReturn;
        }
        self.ai_guard_return_enter(id)
    }

    /// `AIGuardOuterState::onEnter`/`update` (`AIGuard.cpp:525`): chase the nemesis, giving up after
    /// `m_guardChaseUnitFrames`.
    fn ai_guard_outer_enter(&mut self, id: ObjectId) -> StateReturn {
        let nemesis = self.objects.get(&id).map(|o| o.ai.machine.guard.nemesis).unwrap_or(ObjectId::INVALID);
        // `AIGuardOuterState::onEnter` (`AIGuard.cpp:529-541`): the patrol mode, and a nemesis that no longer
        // exists, are SUCCESS into `AI_GUARD_GET_CRATE` (`AIGuard.cpp:192`), not into idle.
        let patrol = self.objects.get(&id).is_some_and(|o| o.ai.machine.guard.mode == crate::orders::GuardMode::WithoutPursuit);
        if nemesis == ObjectId::INVALID || patrol || self.find_object(nemesis).is_none() {
            return self.ai_guard_get_crate_then_return(id);
        }
        // `AIGuardOuterState::onEnter` (AIGuard.cpp:549-551): an area guard's centre is the polygon's.
        let center = self.ai_guard_centre(id).unwrap_or(Coord3D::ZERO);
        let range = self.guard_area_outer_range(id, self.ai_guard_outer_range(id));
        let now = self.frame;
        // `AIGuardOuterState::onEnter` (`AIGuard.cpp:553`): all three exit conditions, with the
        // chase give-up frame `m_guardChaseUnitFrames` (`AIData GuardChaseUnitsDuration`).
        let give_up = now + self.ai_tunables.guard_chase_unit_frames;
        let exit = AttackExitConditions {
            exit_if_no_unit_found: true,
            exit_if_expired_duration: true,
            exit_if_outside_radius: true,
            center,
            radius_sqr: range * range,
            give_up_frame: give_up,
        };
        // `AIAttackState::onEnter` (AIStates.cpp:5526-5527): a condition already met at entry is SUCCESS, with no
        // attack, and the outer state's SUCCESS goes to `AI_GUARD_GET_CRATE` (AIGuard.cpp:568-569, :192).
        if exit.should_exit(self.find_object(nemesis).map(|v| v.position), now) {
            return self.ai_guard_get_crate_then_return(id);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.guard.attack_center = center;
            o.ai.machine.guard.attack_radius_sqr = range * range;
            o.ai.machine.guard.attack_give_up_frame = give_up;
            o.ai.machine.guard_attack_active = true;
            o.ai.machine.in_guard_attack = true;
        }
        self.ai_guard_begin_attack(id, nemesis, exit, false);
        if self.objects.get(&id).is_some_and(|o| o.combat.attack.is_none()) {
            // The attack's entry failed: `AIGuardOuterState::onEnter` returns SUCCESS (AIGuard.cpp:568-569).
            self.ai_guard_finish_attack(id);
            return self.ai_guard_get_crate_then_return(id);
        }
        StateReturn::Continue
    }

    /// `AIGuardInnerState::onEnter`'s `m_attackState->onEnter`: build the attack order on the guard
    /// machine's behalf *without* changing the main state (the enclosing `AI_GUARD` state keeps
    /// running its sub-machine). The same seam serves the `AIGuardRetaliateMachine`'s three attacking
    /// sub-states (`AIGuardRetaliate.cpp:411,525,822`).
    pub(crate) fn ai_guard_begin_attack(&mut self, id: ObjectId, nemesis: ObjectId, exit: AttackExitConditions, follow: bool) {
        let vpos = self.objects.get(&nemesis).map_or(Coord3D::ZERO, |v| v.position);
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack = Some(AttackOrder::new(AttackTarget::Object(nemesis)).with_exit(exit, follow));
            o.combat.force_attack = false;
            o.ai.machine.attack_is_object = true;
            o.ai.machine.attack_force = false;
            o.ai.machine.goal_object = nemesis;
            o.ai.machine.goal_position = vpos;
            o.ai.machine.attack_original_victim_pos = vpos;
            o.ai.machine.attack_initial_approach = true;
            o.ai.machine.locked_weapon_on_enter = false;
            // No `aiIdle` here. `AIGuardOuterState::onEnter` (AIGuard.cpp:548-562) and the inner, aggressor and retaliate
            // entries call `AIAttackState::onEnter` on their own sub-machine (AIGuard.cpp:436, :568; AIGuardRetaliate.cpp:411),
            // and that entry never touches the locomotor (AIStates.cpp:5516-5592). The unit's current move (the guard's
            // return path, `m_path`, and its `PositionOnPath` goal) keeps driving it: the attack path is only queued here and
            // `computeAttackPath` (AIUpdate.cpp:1782) runs from `doPathfind` in the end-of-frame queue (AI.cpp:359), where it
            // destroys the old path. A `stop()` here cleared the path and the goal, so a crawler still moving lost its locomotor
            // goal one frame early (AN5 s1, the guard transition at f3129; the goal, not the physics, is what `stop()` changes).
        }
        // `AIAttackState::onEnter` (AIStates.cpp:5516-5592) runs its sub-machine's entry in the same frame: an
        // out-of-range victim's chase or approach requests its path now (`ai_attack_enter_pass`), not on the
        // next update. The caller reads a failed entry as the attack's failure (no `combat.attack`).
        self.ai_attack_enter_pass(id);
    }

    fn ai_guard_outer_update(&mut self, id: ObjectId) -> StateReturn {
        // `AIGuardOuterState::update` (`AIGuard.cpp:573`): with no attack state it returns SUCCESS.
        if !self.objects.get(&id).is_some_and(|o| o.ai.machine.guard_attack_active) {
            return self.ai_guard_get_crate_then_return(id);
        }
        let now = self.frame;
        // `AIGuardOuterState::update` (`AIGuard.cpp:578`): follow the guarded object, and reset the
        // chase timer while the nemesis is still inside the guard range. The reset runs before the
        // attack's own exit test, which reads the same `m_attackGiveUpFrame` (`AIGuard.cpp:131`).
        self.ai_guard_refresh_exit_center(id);
        let vision_sqr = {
            let r = self.ai_guard_range(id);
            r * r
        };
        let chase_frames = self.ai_tunables.guard_chase_unit_frames;
        if let (Some(n), Some(o)) = (self.objects.get(&id).map(|o| o.ai.machine.guard.nemesis), self.objects.get(&id)) {
            let _ = o;
            if let Some(nem) = self.objects.get(&n) {
                let center = self.objects.get(&id).map(|o| o.ai.machine.guard.attack_center).unwrap_or(Coord3D::ZERO);
                let dx = center.x - nem.position.x;
                let dy = center.y - nem.position.y;
                let dz = center.z - nem.position.z;
                if dx * dx + dy * dy + dz * dz <= vision_sqr {
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.ai.machine.guard.attack_give_up_frame = now + chase_frames;
                    }
                }
            }
        }
        // The attack's exit conditions are the outer state's `m_exitConditions` (`AIGuard.cpp:525`), so the
        // attack reads the give-up frame this update has just set.
        let give_up = self.objects.get(&id).map_or(0, |o| o.ai.machine.guard.attack_give_up_frame);
        if let Some(a) = self.objects.get_mut(&id).and_then(|o| o.combat.attack.as_mut()) {
            a.exit.give_up_frame = give_up;
        }
        self.ai_attack_update_object(id);
        if self.objects.get(&id).is_some_and(|o| o.combat.attack.is_none()) {
            self.ai_guard_finish_attack(id);
            // The outer state's SUCCESS or FAILURE both go to `AI_GUARD_GET_CRATE` (`AIGuard.cpp:192`),
            // then `AI_GUARD_RETURN` (`AIGuard.cpp:193`); idle is reached only from RETURN's success.
            return self.ai_guard_get_crate_then_return(id);
        }
        StateReturn::Continue
    }

    /// `hasAttackedMeAndICanReturnFire` (`AIGuard.cpp:59`): the `attackAggressors` transition
    /// condition. Clears the clearable last attacker as a side effect (so it fires once per hit).
    fn ai_guard_check_aggressor(&mut self, id: ObjectId) -> bool {
        let (attacker, player) = match self.objects.get(&id) {
            Some(o) => (o.clearable_last_attacker(), o.player),
            None => return false,
        };
        if attacker == ObjectId::INVALID {
            return false;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.clear_last_attacker();
        }
        let Some(target) = self.objects.get(&attacker) else { return false };
        if target.is_dying() || self.relationship(player, target.player) != crate::damage::Relationship::Enemies {
            return false;
        }
        if !self.is_able_to_attack(id) {
            return false;
        }
        let Some(info) = self.target_info(&AttackTarget::Object(attacker), player, false) else { return false };
        let owner = &self.objects[&id];
        Self::choose_weapon(owner, Some(&info), self.frame, None).is_some()
    }

    /// The `attackAggressors` transition (`AIGuard.cpp:170-174`, defined for `AI_GUARD_IDLE`, and — retail branch,
    /// `#if RETAIL_COMPATIBLE_CRC` — `AI_GUARD_INNER` and `AI_GUARD_RETURN`, `:182-184`) at the point the engine
    /// evaluates a state's conditions: on a CONTINUE return (`StateMachine.cpp:158-192`), on a sleep return
    /// (`:203-252`) and on an `onEnter` that returns CONTINUE (`internalSetState`, `:647-651`). Returns true when the
    /// machine moved to `AI_GUARD_ATTACK_AGGRESSOR`.
    fn ai_guard_aggressor_condition(&mut self, id: ObjectId) -> bool {
        if !self.ai_guard_check_aggressor(id) {
            return false;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.guard_sub = AiStateType::GuardAttackAggressor;
        }
        self.ai_guard_aggressor_enter(id);
        true
    }

    /// `AIGuardAttackAggressorState::onEnter` (`AIGuard.cpp:834`): attack the last attacker with the
    /// `AIAttackState`'s follow variant and all three guard exit conditions.
    fn ai_guard_aggressor_enter(&mut self, id: ObjectId) -> StateReturn {
        // `AIGuard.cpp:839-841`: a damage source with an id becomes the nemesis; otherwise the stored one stays.
        let source = self.objects.get(&id).map_or(ObjectId::INVALID, |o| o.last_damage_info.source);
        if let Some(o) = self.objects.get_mut(&id) {
            if source != ObjectId::INVALID {
                o.ai.machine.guard.nemesis = source;
            }
        }
        let nemesis = self.objects.get(&id).map_or(ObjectId::INVALID, |o| o.ai.machine.guard.nemesis);
        let center = self.ai_guard_target(id);
        if nemesis == ObjectId::INVALID || !self.objects.contains_key(&nemesis) || center.is_none() {
            // `AIGuard.cpp:843-849`: a null nemesis is SUCCESS, and the aggressor state's SUCCESS goes to
            // `AI_GUARD_INNER` (`AIGuard.cpp:194`).
            self.set_team_target_object(id, None);
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.guard_sub = AiStateType::GuardInner;
            }
            return self.ai_guard_inner_enter(id);
        }
        let center = center.unwrap_or(Coord3D::ZERO);
        let range = self.ai_guard_range(id);
        let now = self.frame;
        // `AIGuardAttackAggressorState::onEnter` (`AIGuard.cpp:856`): `m_guardChaseUnitFrames`.
        let give_up = now + self.ai_tunables.guard_chase_unit_frames;
        let exit = AttackExitConditions {
            exit_if_no_unit_found: true,
            exit_if_expired_duration: true,
            exit_if_outside_radius: true,
            center,
            radius_sqr: range * range,
            give_up_frame: give_up,
        };
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.guard.nemesis = nemesis;
            o.ai.machine.guard.attack_center = center;
            o.ai.machine.guard.attack_radius_sqr = range * range;
            o.ai.machine.guard.attack_give_up_frame = give_up;
            o.ai.machine.guard_sub = AiStateType::GuardAttackAggressor;
            o.ai.machine.guard_attack_active = true;
            o.ai.machine.in_guard_attack = true;
        }
        self.ai_guard_begin_attack(id, nemesis, exit, true);
        if self.objects.get(&id).is_some_and(|o| o.combat.attack.is_none()) {
            // The attack's entry failed: the aggressor state's SUCCESS goes to `AI_GUARD_INNER` (AIGuard.cpp:194, :843-849).
            self.ai_guard_finish_attack(id);
            self.set_team_target_object(id, None);
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.guard_sub = AiStateType::GuardInner;
            }
            return self.ai_guard_inner_enter(id);
        }
        StateReturn::Continue
    }

    /// `AIGuardAttackAggressorState::update` (`AIGuard.cpp:874`): run the attack; on finish the guard
    /// machine transitions to `AI_GUARD_INNER`.
    fn ai_guard_aggressor_update(&mut self, id: ObjectId) -> StateReturn {
        // `AIGuardAttackAggressorState::update` (`AIGuard.cpp:874`): with no attack state it returns SUCCESS,
        // and the aggressor state's SUCCESS goes to `AI_GUARD_INNER` (`AIGuard.cpp:194`).
        if !self.objects.get(&id).is_some_and(|o| o.ai.machine.guard_attack_active) {
            self.set_team_target_object(id, None);
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.guard_sub = AiStateType::GuardInner;
            }
            return self.ai_guard_inner_enter(id);
        }
        self.ai_guard_refresh_exit_center(id);
        self.ai_attack_update_object(id);
        if self.objects.get(&id).is_some_and(|o| o.combat.attack.is_none()) {
            self.ai_guard_finish_attack(id);
            self.set_team_target_object(id, None); // onExit of the inner/aggressor state clears the team target (AIGuard.cpp:486,900)
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.guard_sub = AiStateType::GuardInner;
            }
            self.ai_guard_inner_enter(id);
        }
        StateReturn::Continue
    }

    /// `AIGuardInnerState::update`/`AIGuardOuterState::update`/`AIGuardAttackAggressorState::update`
    /// (`AIGuard.cpp:451/578/878`): keep the attack's exit radius centred on the guarded object as
    /// it moves.
    fn ai_guard_refresh_exit_center(&mut self, id: ObjectId) {
        // `if (targetToGuard) m_exitConditions.m_center = *targetToGuard->getPosition();` (AIGuard.cpp:451-455,
        // :578-582, :878-883): only a found guarded object moves the centre. A location or area guard keeps the
        // centre its state entered with (for an area, the polygon's centre), so this reads the object alone.
        let target = self.objects.get(&id).map(|o| o.ai.machine.guard.target_to_guard).filter(|t| *t != ObjectId::INVALID);
        let Some(center) = target.and_then(|t| self.objects.get(&t)).map(|t| t.position) else { return };
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.guard.attack_center = center;
            if let Some(a) = o.combat.attack.as_mut() {
                a.exit.center = center;
            }
        }
    }

    fn ai_guard_finish_attack(&mut self, id: ObjectId) {
        // The guard's attack sub-state ends and the guard moves on (outer -> return, or the get-crate move). Its exit is
        // `AIAttackState::onExit`: the move is ended only by an approach or pursue sub-state (`end_attack_with`).
        self.end_attack_internal(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.guard_attack_active = false;
            o.ai.machine.in_guard_attack = false;
        }
    }
}

// ---- AI_FOLLOW_PATH / AI_FOLLOW_WAYPOINT_PATH / production exits / wander ----------------------

impl GameLogic {
    /// `AIStateMachine::setGoalPath` + `AIFollowPathState` / `AIFollowWaypointPathState`
    /// (`AIStates.cpp:3262`, `:4023`): walk a list of waypoints. `as_team` selects the waypoint-path
    /// variant (group offsets are not modelled; the script engine passes individual destinations).
    pub fn ai_follow_path(&mut self, id: ObjectId, path: Vec<Coord3D>, as_team: bool) {
        let state = if as_team { AiStateType::FollowWaypointPath } else { AiStateType::FollowPath };
        self.ai_set_follow_path(id, path, state, None, None);
    }

    /// `AIUpdateInterface::aiFollowWaypointPath` (`privateFollowWaypointPath`, AIUpdate.cpp:3277) for a
    /// script order: walk the waypoint graph from waypoint `wp` in `AI_FOLLOW_WAYPOINT_PATH_AS_INDIVIDUALS`.
    /// The as-team group speed and offsets (`groupFollowWaypointPathAsTeam`) are not modelled, so both
    /// variants walk the links one unit at a time.
    /// `at` and `pos` are the waypoint and its position, looked up by the script engine (`pos` is the
    /// waypoint's location).
    pub(crate) fn ai_follow_waypoint(&mut self, id: ObjectId, at: WalkAt, pos: Coord3D, source: CommandSource) {
        self.ai_set_follow_path(id, vec![pos], AiStateType::FollowWaypointPath, None, Some(at));
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.last_command_source = source;
        }
    }

    /// `AIUpdateInterface::aiPanic(way)` -> `privatePanic` (AIUpdate.cpp:4031): clear, set the goal
    /// waypoint and enter `AI_PANIC`, which walks the links like a waypoint path. The wander offsets of
    /// `AIPanicState::onEnter` are not modelled. `at` / `pos` as in [`Self::ai_follow_waypoint`].
    pub(crate) fn ai_panic_waypoint(&mut self, id: ObjectId, at: WalkAt, pos: Coord3D, source: CommandSource) {
        self.ai_set_follow_path(id, vec![pos], AiStateType::FollowPath, None, Some(at));
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.last_command_source = source;
        }
        self.ai_set_state(id, AiStateType::Panic);
    }

    /// `AIFollowWaypointPathState::getNextWaypoint` (AIStates.cpp:3791, `ALLOW_BACKTRACK` build): at the
    /// end of a waypoint-graph path the next goal is a random link of the current waypoint, drawn
    /// with `GameLogicRandomValue(0, linkCount-1)` even for a single link. `None` when it has no links.
    fn ai_walk_next_waypoint(&mut self, id: ObjectId) -> Option<Coord3D> {
        let cur = self.objects.get(&id)?.ai.machine.goal_waypoint?;
        let links = self.scripts.waypoint_links.get(&cur.waypoint).cloned().unwrap_or_default();
        if links.is_empty() {
            return None;
        }
        let which = self.rng_mut().value(0, links.len() as i32 - 1) as usize;
        let next = links[which];
        let pos = self.scripts.waypoint_by_id.get(&next)?.pos;
        let open = self.scripts.waypoint_links.get(&next).is_some_and(|l| !l.is_empty());
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.goal_waypoint = Some(WalkAt { waypoint: next, open });
        }
        Some(pos)
    }

    /// `AIFollowWaypointPathState::hasNextWaypoint` (AIStates.cpp:3830): the walk's current waypoint has
    /// a link, so the path does not end at it.
    fn ai_walk_continues(&self, id: ObjectId) -> bool {
        self.objects.get(&id).and_then(|o| o.ai.machine.goal_waypoint).is_some_and(|w| w.open)
    }

    /// `AIUpdateInterface::aiFollowExitProductionPath` -> `privateFollowPath(path, creationObject, CMD_FROM_AI,
    /// exitProduction = true)` (`AIUpdate.cpp:3402`): the path runs in `AI_FOLLOW_EXITPRODUCTION_PATH`, and
    /// the factory that produced the unit is the obstacle it ignores (`ignoreObstacle`, `:3418`).
    pub(crate) fn ai_follow_exit_production_path(&mut self, id: ObjectId, path: Vec<Coord3D>, factory: ObjectId) {
        self.ai_set_follow_path(id, path, AiStateType::FollowExitProductionPath, Some(factory), None);
    }

    /// `privateFollowPath` (`AIUpdate.cpp:3402`): clear the machine, set the goal path and start `state`.
    /// `ignore` is `ignoreObject` (`ignoreObstacle`); `None` leaves the obstacle as it was. `walk` is the
    /// waypoint the path ends at when it walks the waypoint graph ([`AiMachine::goal_waypoint`]).
    pub(crate) fn ai_set_follow_path(
        &mut self,
        id: ObjectId,
        path: Vec<Coord3D>,
        state: AiStateType,
        ignore: Option<ObjectId>,
        walk: Option<WalkAt>,
    ) {
        if !self.is_mobile(id) || path.is_empty() {
            return;
        }
        // `privateFollowPath` (`AIUpdate.cpp:3402`) clears the machine before setting the goal path.
        // `privatePanic` (`:4042`) does the same, so its `ai_follow_path` call clears for it too.
        // Jets own their `aiDoCommand`, so only the plain `AIUpdateInterface` clears.
        if !self.is_jet(id) {
            self.ai_clear(id);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack = None;
            o.combat.attack_move = false;
            o.combat.guard = None;
            o.combat.hunt = false;
            o.ai.machine.goal_path = path;
            o.ai.machine.goal_path_index = 0;
            o.ai.machine.goal_path_retry = ATTACK_RETRY_COUNT;
            o.ai.machine.goal_waypoint = walk;
            o.ai.machine.last_command_source = CommandSource::Ai;
            // `ignoreObstacle(ignoreObject)` (`AIUpdate.cpp:3418`).
            if let Some(f) = ignore {
                o.ai.ignore_obstacle_id = f;
            }
        }
        self.ai_command_set_state(id, state);
    }

    /// `AIUpdateInterface::setAttitude` (`AIUpdate.cpp:4283`).
    pub fn set_unit_attitude(&mut self, id: ObjectId, attitude: i32) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.attitude = attitude as i8;
        }
    }

    /// `AIUpdateInterface::setAttackInfo` (`AIUpdate.h:513`). `None` keeps the current info: the
    /// callers that only set one when the team has a named set (`Object::setTeam`, Object.cpp:912)
    /// pass nothing otherwise.
    pub fn set_unit_attack_info(&mut self, id: ObjectId, info: Option<String>) {
        if let (Some(o), Some(n)) = (self.objects.get_mut(&id), info) {
            o.ai.attack_info = Some(n);
        }
    }

    /// `AttackPriorityInfo::getPriority(victim->getTemplate())` for the attack set `set` (a set that was
    /// never defined is the default info: priority 1 for everything).
    pub(crate) fn attack_priority_of(&self, set: &str, victim: ObjectId) -> i32 {
        self.objects.get(&victim).map_or(1, |v| self.scripts.attack_priority(set, &v.template.name))
    }

    /// `AIUpdateInterface::getMoodMatrixActionAdjustment(MM_Action_Move) & MAA_Action_To_AttackMove`
    /// (`AIUpdate.cpp:4402-4452`): the unit is computer-controlled and its attitude is Alert or
    /// Aggressive, so a plain move becomes an attack-move. Angry-mob members (`INFANTRY` +
    /// `IGNORED_IN_GUI`) are never subject to moods (`AIUpdate.cpp:4376`).
    pub(crate) fn mood_converts_move_to_attack_move(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| {
            o.ai.attitude >= 1
                && self.ai_players.contains_key(&o.player)
                && !(o.template.ignored_in_gui && o.is_kind_of(kind_of::INFANTRY))
        })
    }

    /// `AIUpdateInterface::aiAttackMoveToPosition` for one unit (no group offset): the mood
    /// conversion's replacement for the running move.
    pub(crate) fn ai_attack_move_to_position(&mut self, id: ObjectId, pos: Coord3D) {
        self.ai_attack_move_to_position_from(id, pos, CommandSource::Ai);
    }

    /// [`Self::ai_attack_move_to_position`] with the engine's `CommandSourceType` (`aiAttackMoveToPosition(pos, maxShots,
    /// cmdSource)`, AIUpdate.cpp:3497): the order is recorded as the last command source.
    pub(crate) fn ai_attack_move_to_position_from(&mut self, id: ObjectId, pos: Coord3D, source: CommandSource) {
        // `AssaultTransportAIUpdate::aiDoCommand`'s AICMD_ATTACKMOVE_TO_POSITION branch for a command not from the AI
        // (AssaultTransportAIUpdate.cpp:88-92), before the base command's own checks.
        self.assault_command(id, crate::assault_transport::AssaultCommand::AttackMoveToPosition(pos), source);
        if !self.is_mobile(id) {
            return;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack = None;
            o.combat.attack_move = true;
            o.combat.guard = None;
            o.combat.hunt = false;
            o.ai.machine.goal_position = pos;
            o.ai.machine.goal_object = ObjectId::INVALID;
            o.ai.machine.attack_move_resume = false;
            o.ai.machine.attack_return = AiStateType::AttackMoveTo;
            o.ai.machine.last_command_source = source;
        }
        // The attack move is the main state even while a dock move runs (`privateAttackMoveToPosition`, a
        // retail `clear()` keeps the temporary, `StateMachine.cpp:344`); `ai_internal_set_state` gives its entry the
        // main move's fields.
        self.ai_command_set_state(id, AiStateType::AttackMoveTo);
    }

    /// `AIMoveToState::update` (`AIStates.cpp:2084`): a mood that turns moves into attack-moves
    /// replaces the move (`aiAttackMoveToPosition(&m_goalPosition)`), else the plain move update.
    fn ai_move_to_update(&mut self, id: ObjectId) -> StateReturn {
        // `if (m_isMoveTo && (adjustment & MAA_Action_To_AttackMove)) ai->aiAttackMoveToPosition(...)`
        // (`AIStates.cpp:2087`). The conversion does not end the update: C++ falls through to the goal
        // object handling and then `AIInternalMoveToState::update` (`AIStates.cpp:2087` on), so this
        // state's own arrival test still runs in the same frame. A temporary `AI_MOVE_TO` (a dock move)
        // relies on that: without it the dock move never reports arrival.
        // `aiAttackMoveToPosition(&m_goalPosition, ...)` (AIStates.cpp:2088): the state's own goal, which
        // `AIInternalMoveToState::onEnter` adjusted in place (AIStates.cpp:1661-1664) and the goal-object
        // refresh below updates after this call. `ai.move_goal` is that value (`start_move`).
        // The attack move is a new state object in C++, so the converting move's update keeps its own fields
        // (`AIMoveToState::update` goes on with `m_goalPosition` and `m_waitingForPath` of the old object).
        // While a temporary's code runs, `with_main_move_fields` already keeps the two apart.
        let own = self.capture_move_fields(id);
        let mut attack: Option<MoveStateFields> = None;
        if self.mood_converts_move_to_attack_move(id) {
            let m = match self.objects.get(&id) {
                Some(o) => o.ai.machine.clone(),
                None => return StateReturn::Failure,
            };
            let goal = self.objects.get(&id).and_then(|o| o.ai.move_goal).or_else(|| self.ai_goal_position(id, &m));
            if let Some(goal) = goal {
                let temp_live = self.objects.get(&id).is_some_and(|o| o.ai.machine.temp_live);
                self.ai_attack_move_to_position(id, goal);
                if !temp_live {
                    attack = self.capture_move_fields(id);
                    if let Some(own) = own {
                        self.apply_move_fields(id, own);
                    }
                }
            }
        }
        let r = self.ai_move_to_update_rest(id);
        if let Some(attack) = attack {
            self.apply_move_fields(id, attack);
        }
        r
    }

    /// The part of `AIMoveToState::update` after the mood conversion: the goal-object refresh and
    /// `AIInternalMoveToState::update`.
    fn ai_move_to_update_rest(&mut self, id: ObjectId) -> StateReturn {
        // `AIMoveToState::update` (`AIStates.cpp:2075`): "if we have a goal object, move to it, as it
        // may have moved". While the goal object exists the state's goal is its current position, so
        // a repath follows it. A gone object leaves the last goal in place. Projectile leads are not
        // modelled here (missiles do not use this state).
        if let Some(target) = self.objects.get(&id).map(|o| o.ai.machine.goal_object).filter(|g| *g != ObjectId::INVALID) {
            if let Some(pos) = self.objects.get(&target).map(|t| t.position) {
                if let Some(o) = self.objects.get_mut(&id) {
                    if !o.is_kind_of(kind_of::PROJECTILE) {
                        o.ai.move_goal = Some(pos);
                    }
                }
            }
        }
        self.ai_internal_move_update(id)
    }

    /// `AIFollowWaypointPathState::update` (`AIStates.cpp:4135`): the same mood conversion turns the
    /// follow into `aiAttackFollowWaypointPath` from the current waypoint.
    /// NOT PORTED (lane 185, R23-16): the team-lockstep branch, `if (m_moveAsGroup && currentWaypoint != team's)` in
    /// `AIFollowWaypointPathState::update` (AIStates.cpp:4151-4171: `friend_startingMove()` at :4170, then `computePath()`).
    /// `m_moveAsGroup` (the `groupFollowWaypointPathAsTeam` walk, AIUpdate.cpp group speed) is not modelled (see
    /// `script/mod.rs`), so this walk always takes the individual path (`ai_follow_path_update_as` with `walk`).
    fn ai_follow_waypoint_path_update(&mut self, id: ObjectId) -> StateReturn {
        if self.mood_converts_move_to_attack_move(id) {
            self.ai_set_state(id, AiStateType::AttackFollowWaypointPath);
            return StateReturn::Continue;
        }
        self.ai_follow_path_update_as(id, true)
    }

    /// `AIAttackFollowWaypointPathState::onEnter` (`AIStates.cpp:4445-4450`): `m_attackFollowMachine->clear()`
    /// and `setState(AI_IDLE)` (one sub-machine `AI_IDLE` entry: mood reset + draw), then continue the path
    /// from the current waypoint. The port re-enters this state when an attack it ran ends, which the engine
    /// does not do: that attack's exit already entered the sub-machine's `AI_IDLE`
    /// (`ai_attack_exit`), so the re-entry must skip the draw and run the update's force-retarget scan
    /// (`AIStates.cpp:4462-4500`).
    fn ai_attack_follow_enter(&mut self, id: ObjectId) -> StateReturn {
        if !self.is_mobile(id) {
            return StateReturn::Failure;
        }
        let resuming = self.objects.get(&id).is_some_and(|o| o.ai.machine.attack_follow_resume);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.attack_follow_resume = false;
        }
        if !resuming {
            self.ai_attack_sub_idle_enter(id);
        }
        let (path, idx) = match self.objects.get(&id) {
            Some(o) => (o.ai.machine.goal_path.clone(), o.ai.machine.goal_path_index),
            None => return StateReturn::Failure,
        };
        if idx >= path.len() {
            return StateReturn::Failure;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack_move = true;
            o.ai.machine.goal_path_retry = ATTACK_RETRY_COUNT;
        }
        if resuming {
            // `if (m_attackFollowMachine->isInIdleState())` (`AIStates.cpp:4480-4502`): the scan is
            // `getNextMoodTarget(false, false)` because `forceRetargetThisFrame` is set.
            if let Some(t) = self.ai_force_retarget_scan(id) {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.attack_follow_resume = true;
                }
                self.ai_start_attack(id, AttackTarget::Object(t), false, false, None, AiStateType::AttackFollowWaypointPath);
                return StateReturn::Continue;
            }
            // No target: `shouldRepathThisFrame` recomputes the goal and the path (`:4505-4510`).
        }
        self.ai_internal_move_enter(id, path[idx], idx + 1 == path.len())
    }

    /// `AIAttackFollowWaypointPathState::onExit` (`AIStates.cpp:4518-4521`): `m_attackFollowMachine->setState(AI_IDLE)`
    /// (an `AI_IDLE` entry: mood reset + draw), then the follow path's exit.
    fn ai_attack_follow_exit(&mut self, id: ObjectId) {
        self.ai_attack_sub_idle_enter(id);
        self.ai_internal_move_exit(id);
    }

    /// `AIAttackFollowWaypointPathState::update` (`AIStates.cpp:4454`): look for a mood target and
    /// attack it (returning to this state), else keep following the path.
    fn ai_attack_follow_update(&mut self, id: ObjectId) -> StateReturn {
        // "Check to see if we have created a crate we need to pick up": never a crate.
        let _ = self.check_for_crate_to_pickup(id);
        if let Some(t) = self.ai_idle_scan(id, false, None, crate::orders::GuardMode::Normal, true) {
            // The follow state's sub-machine goes from `AI_IDLE` to an attack: the state itself is not
            // exited, and the jump does not change the AI's last command source (only the attack-move's
            // own pickup does, `AIStates.cpp:3658`).
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.attack_follow_resume = true;
            }
            self.ai_start_attack(id, AttackTarget::Object(t), false, false, None, AiStateType::AttackFollowWaypointPath);
            return StateReturn::Continue;
        }
        self.ai_follow_path_update(id)
    }

    /// `AIMoveAndEvacuateState::onEnter` (`AIStates.cpp:3462`): move to the goal, then empty the
    /// transport.
    pub fn ai_move_and_evacuate(&mut self, id: ObjectId, goal: Coord3D) {
        self.ai_move_and_evacuate_state(id, goal, AiStateType::MoveAndEvacuate);
    }

    /// `aiMoveToAndEvacuateAndExit` -> `privateMoveToAndEvacuateAndExit` (`AIUpdate.cpp:3068`): as
    /// [`Self::ai_move_and_evacuate`], then the unit moves to the goal again and deletes itself.
    pub(crate) fn ai_move_and_evacuate_and_exit(&mut self, id: ObjectId, goal: Coord3D) {
        self.ai_move_and_evacuate_state(id, goal, AiStateType::MoveAndEvacuateAndExit);
    }

    fn ai_move_and_evacuate_state(&mut self, id: ObjectId, goal: Coord3D, state: AiStateType) {
        if !self.is_mobile(id) {
            return;
        }
        // `privateMoveToAndEvacuate` (`AIUpdate.cpp:3042`) clears before setting the goal.
        if !self.is_jet(id) {
            self.ai_clear(id);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack = None;
            o.ai.machine.goal_position = goal;
            o.ai.machine.goal_object = ObjectId::INVALID;
            // `setLastCommandSource(cmdSource)` (`AIUpdate.cpp:3047`, `:3073`): every port caller is a
            // script (`aiMoveToAndEvacuate` from `ScriptActions.cpp`, `CMD_FROM_SCRIPT`).
            o.ai.machine.last_command_source = CommandSource::Script;
        }
        self.ai_set_state(id, state);
    }

    /// `AIMoveAndDeleteState::onEnter` (`AIStates.cpp:3731`): move to the goal, then delete self.
    pub fn ai_move_and_delete(&mut self, id: ObjectId, goal: Coord3D) {
        if !self.is_mobile(id) {
            return;
        }
        // `privateMoveToAndEvacuateAndExit` (`AIUpdate.cpp:3068`) clears before setting the goal.
        if !self.is_jet(id) {
            self.ai_clear(id);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack = None;
            o.ai.machine.goal_position = goal;
            o.ai.machine.goal_object = ObjectId::INVALID;
        }
        self.ai_set_state(id, AiStateType::MoveAndDelete);
    }

    /// `AIWanderInPlaceState::onEnter` (`AIStates.cpp:4691`): wander around the current position.
    pub fn ai_wander_in_place(&mut self, id: ObjectId) {
        if !self.is_mobile(id) {
            return;
        }
        // `privateWanderInPlace` (`AIUpdate.cpp:4020`) clears.
        if !self.is_jet(id) {
            self.ai_clear(id);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack = None;
        }
        self.ai_set_state(id, AiStateType::WanderInPlace);
    }

    /// `AIWanderState` / `AIPanicState` (`AIStates.cpp:4569`, `:4802`): wander/panic along a
    /// waypoint path.
    pub fn ai_panic(&mut self, id: ObjectId, path: Vec<Coord3D>) {
        if path.is_empty() {
            return;
        }
        self.ai_follow_path(id, path, false);
        self.ai_set_state(id, AiStateType::Panic);
    }

    /// `AIFollowPathState::onEnter` (`AIStates.cpp:3262`).
    fn ai_follow_path_enter(&mut self, id: ObjectId) -> StateReturn {
        self.ai_follow_path_enter_with(id, false, false)
    }

    /// `AIFollowWaypointPathState::onEnter`: a waypoint walk's point adjusts only when no link continues it.
    fn ai_follow_waypoint_path_enter(&mut self, id: ObjectId) -> StateReturn {
        self.ai_follow_path_enter_with(id, false, true)
    }

    /// `AIFollowPathState::onEnter` for `AI_FOLLOW_EXITPRODUCTION_PATH` (`AIStates.cpp:3283-3288`): the
    /// unit may path through units, and its base move does not adjust. A path with a single point adjusts
    /// after the base call (`AIStates.cpp:3308-3311`), see [`ai_follow_path_enter_with`](Self::ai_follow_path_enter_with).
    fn ai_follow_exit_production_path_enter(&mut self, id: ObjectId) -> StateReturn {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.can_path_through_units = true;
        }
        self.ai_follow_path_enter_with(id, true, false)
    }

    fn ai_follow_path_enter_with(&mut self, id: ObjectId, exit_production: bool, waypoint_walk: bool) -> StateReturn {
        if !self.is_mobile(id) {
            return StateReturn::Failure;
        }
        let path = self.objects.get(&id).map(|o| o.ai.machine.goal_path.clone()).unwrap_or_default();
        if path.is_empty() {
            return StateReturn::Failure;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.goal_path_index = 0;
            o.ai.machine.goal_path_retry = ATTACK_RETRY_COUNT;
        }
        let goal = path[0];
        // `setAdjustsDestination(false)` for the exit state (`AIStates.cpp:3293`).
        // Both follow-path onEnters run the base `AIInternalMoveToState::onEnter` first, with the state's default
        // `m_adjustDestinations = true` (AIStateMachine.h:257), so the first point adjusts even when a link follows (the
        // request carries the adjust). The exit-production path never adjusts. A waypoint walk's `setAdjustsDestination`
        // calls come after that base call (R17-7; AIStates.cpp:4070-4095), so they are applied below.
        let adjusts = !exit_production;
        let ret = self.ai_internal_move_enter(id, goal, adjusts);
        // `AIFollowWaypointPathState::onEnter` (AIStates.cpp:4084-4090), after the base call: `if (hasNextWaypoint())
        // setAdjustsDestination(false)`, else `setAdjustsDestination(ai->isDoingGroundMovement())` (the last link's
        // final adjust follows, not ported: `adjustDestination` on the already adjusted goal returns that cell).
        // `AIFollowPathState::onEnter` (AIStates.cpp:3296-3306): `if (nextPos) setAdjustsDestination(false)`, after the
        // base call. The base call's request still carries the adjust (`requestPath(&m_goalPosition,
        // getAdjustsDestination())`, AIStates.cpp:1604), but the flag is off when the request comes back: the wait
        // block of `AIInternalMoveToState::update` (AIStates.cpp:1790-1807) reads `getAdjustsDestination()` on the
        // first update after the path arrives and calls `removeGoal(obj)` for a leg with a next point, not
        // `updateGoal(lastNode)`. Lane 159: the port kept the flag, so a tunnel defender's exit leg marked its
        // closest-path end as its goal cell (250's `(147,194)`), which the C++ does not hold (AH7 f1953).
        if waypoint_walk {
            let continues = self.ai_walk_continues(id);
            let ground = !continues && self.is_doing_ground_movement(id);
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.move_adjusts = ground;
            }
        } else if path.len() > 1 {
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.move_adjusts = false;
            }
        } else if let Some(o) = self.objects.get_mut(&id) {
            // `else { setAdjustsDestination(m_adjustFinal); setPathExtraDistance(0); }` (AIStates.cpp:3308-3311), and
            // `m_adjustFinal` is TRUE here (AIStates.cpp:3276, :3286, AIStateMachine.h:809). So an exit-production
            // path with one point adjusts its destination after all (R18-10).
            o.ai.machine.move_adjusts = true;
        }
        // `AIFollowPathState::onEnter` sets the extra distance after `AIInternalMoveToState::onEnter`.
        self.ai_follow_path_extra(id, &path, 0, 0.0);
        ret
    }

    /// `AIFollowPathState::update` (`AIStates.cpp:3341`).
    fn ai_follow_path_update(&mut self, id: ObjectId) -> StateReturn {
        self.ai_follow_path_update_as(id, false)
    }

    /// `walk` is the waypoint walk (`AIFollowWaypointPathState::update`): its `friend_startingMove` comes after the
    /// destination adjust (`computeGoal`, `adjustDestination` fails before it, AIStates.cpp:4244-4247), where the path
    /// follow's comes before the adjust (:3384, :3406-3408). Lane 185 (R23-17).
    fn ai_follow_path_update_as(&mut self, id: ObjectId, walk: bool) -> StateReturn {
        let status = self.ai_internal_move_update(id);
        if status == StateReturn::Continue {
            return status;
        }
        let (mut idx, retry, mut path) = match self.objects.get(&id) {
            Some(o) => (o.ai.machine.goal_path_index, o.ai.machine.goal_path_retry, o.ai.machine.goal_path.clone()),
            None => return StateReturn::Failure,
        };
        // `m_retryCount`: a failure retries the same point until the limit, then advances.
        if status == StateReturn::Failure && retry > 0 {
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.goal_path_retry = retry - 1;
            }
        } else {
            idx += 1;
        }
        let pos = self.objects.get(&id).map(|o| o.position).unwrap_or(Coord3D::ZERO);
        while idx < path.len() && pos.dist_2d(&path[idx]) < crate::pathfind::PATHFIND_CELL_SIZE_F {
            idx += 1;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.goal_path_index = idx;
        }
        // `ai->ignoreObstacleID(INVALID_ID); // we have exited whatever object we are leaving, if any.` (AIStates.cpp:3377,
        // on every segment advance, before the end-of-path test). A tunnel exit's container is the ignored obstacle
        // only for its first leg (`aiFollowPath` in `OpenContain::exitObjectViaDoor`, `TunnelContain.cpp`, AIUpdate.cpp:3412).
        // Lane 159: without the clear, the defender's start cell stayed an ignored obstacle, so the closest-path search
        // took the unit as free to move where C++ starts it stuck (`startedStuck`, AIPathfind.cpp:8778) (AH7 f1953).
        self.ai_set_ignore_obstacle(id, ObjectId::INVALID);
        if idx >= path.len() {
            // The end of a waypoint walk takes a random link (`AIFollowWaypointPathState::update`);
            // an ordinary path, or a waypoint without links, is done.
            let Some(next) = self.ai_walk_next_waypoint(id) else {
                return StateReturn::Success;
            };
            // A walk only needs the point it heads for, so the reached points are dropped (the path is
            // cloned every update; a patrol must not grow it without bound).
            path.clear();
            path.push(next);
            idx = 0;
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.goal_path = path.clone();
                o.ai.machine.goal_path_index = idx;
            }
        }
        // `ai->friend_startingMove()` (AIStates.cpp:3384, after the end-of-path return; the waypoint walk's own call is
        // `:4247`, after its `computeGoal`): the next segment is a move in progress.
        if !walk {
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.friend_starting_move();
            }
        }
        let mut goal = path[idx];
        // `AIFollowPathState::update` (AIStates.cpp:3403): the last point adjusts only when
        // `m_adjustFinal && (m_adjustFinalOverride || ai->isDoingGroundMovement())`; `m_adjustFinal` is TRUE and the
        // override FALSE (AIStates.cpp:3276, :3286; AIStateMachine.h:809-810). An airborne unit keeps its last point.
        let adjusts = idx + 1 == path.len() && !self.ai_walk_continues(id) && self.is_doing_ground_movement(id);
        if adjusts {
            // `if (!adjustDestination(...)) return STATE_FAILURE;` (AIStates.cpp:3406-3408), then `updateGoal` (:3409).
            match self.ai_adjust_move_goal_strict(id, goal) {
                Some(adjusted) => goal = adjusted,
                None => return StateReturn::Failure,
            }
        }
        if walk {
            // The waypoint walk's `friend_startingMove` (AIStates.cpp:4247), after its adjust (:4244).
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.friend_starting_move();
            }
        }
        // `computePath` does not reset `m_pathExtraDistance` (`AIUpdate.cpp:1715-1760`): the value stays
        // unless the follow-path state sets it for the segment (`AIStates.cpp:3387-3395`).
        let keep = self.objects.get(&id).map_or(0.0, |o| o.ai.path_extra_distance);
        self.ai_internal_move_enter_with(id, goal, adjusts, adjusts);
        self.ai_follow_path_extra(id, &path, idx, keep);
        StateReturn::Continue
    }

    /// `AIFollowPathState`'s `setPathExtraDistance` (`AIStates.cpp:3299-3305` in `onEnter`, `:3387-3395` in
    /// `update`): on a segment with a next point the extra distance is that segment's length, plus four
    /// cells when a point follows it. Other segments keep `keep`. Only `AI_FOLLOW_PATH` and
    /// `AI_FOLLOW_EXITPRODUCTION_PATH` set it here; the waypoint states use `calcExtraPathDistance`,
    /// which needs the waypoint graph this port does not have.
    fn ai_follow_path_extra(&mut self, id: ObjectId, path: &[Coord3D], idx: usize, keep: f32) {
        let follows = matches!(
            self.objects.get(&id).map(|o| o.ai.machine.current),
            Some(AiStateType::FollowPath | AiStateType::FollowExitProductionPath)
        );
        let extra = match (follows, path.get(idx + 1), path.get(idx)) {
            (true, Some(next), Some(pos)) => {
                let mut offset = pos.dist_2d(next);
                if path.get(idx + 2).is_some() {
                    offset += 4.0 * PATHFIND_CELL_SIZE_F;
                }
                offset
            }
            _ => keep,
        };
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.path_extra_distance = extra;
        }
    }

    /// `AIMoveAndEvacuateState::onEnter` (`AIStates.cpp:3462`).
    fn ai_move_and_evacuate_enter(&mut self, id: ObjectId) -> StateReturn {
        let goal = self.objects.get(&id).map(|o| o.ai.machine.goal_position).unwrap_or(Coord3D::ZERO);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.wander_origin = o.position; // `m_origin`, restored on exit
        }
        self.ai_internal_move_enter(id, goal, true)
    }

    /// `AIMoveAndEvacuateState::update` (`AIStates.cpp:3480`).
    /// Once the move is over (either way) a live unit empties its transport (`aiEvacuate(FALSE)`) and its
    /// team is activated (`Team::setActive`): this is how a script-created transport team starts
    /// (`doCreateReinforcements` only activates a team without a transport). A dead unit fails.
    fn ai_move_and_evacuate_update(&mut self, id: ObjectId) -> StateReturn {
        let status = self.ai_internal_move_update(id);
        if status == StateReturn::Continue {
            return status;
        }
        if !self.ai_evacuate_and_activate(id) {
            return StateReturn::Failure;
        }
        status
    }

    /// `AIMoveAndEvacuateState::update` for `AI_MOVE_AND_EVACUATE_AND_EXIT` (`AIStates.cpp:3480`): the same
    /// evacuation, then the state's success and failure state, `AI_MOVE_AND_DELETE`, takes over (it moves
    /// to the goal again and deletes the unit, AIStates.cpp:702).
    fn ai_move_and_evacuate_and_exit_update(&mut self, id: ObjectId) -> StateReturn {
        let status = self.ai_internal_move_update(id);
        if status == StateReturn::Continue {
            return status;
        }
        if !self.ai_evacuate_and_activate(id) {
            return StateReturn::Failure;
        }
        self.ai_set_state(id, AiStateType::MoveAndDelete);
        StateReturn::Continue
    }

    /// The end of `AIMoveAndEvacuateState::update` (AIStates.cpp:3500): a live unit empties its transport
    /// (`aiEvacuate(FALSE)`) and its team is activated (`Team::setActive`). Returns false when the unit is
    /// dead. This is how a script-created transport team starts (`doCreateReinforcements` only activates a
    /// team without a transport).
    fn ai_evacuate_and_activate(&mut self, id: ObjectId) -> bool {
        // `AIStates.cpp:3500`: the unit is checked with `isEffectivelyDead()` (the port's `is_dying()`).
        if self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return false;
        }
        let player = self.objects.get(&id).map(|o| o.player).unwrap_or(0);
        // `AIStates.cpp:3499`: the evacuation is CMD_FROM_AI, so an assault transport's reset is skipped (AssaultTransportAIUpdate.cpp:86).
        self.command_evacuate(player, id, CommandSource::Ai);
        self.activate_team_of(id);
        true
    }

    /// `AIMoveAndDeleteState::onEnter` (`AIStates.cpp:3731`).
    fn ai_move_and_delete_enter(&mut self, id: ObjectId) -> StateReturn {
        let goal = self.objects.get(&id).map(|o| o.ai.machine.goal_position).unwrap_or(Coord3D::ZERO);
        self.ai_internal_move_enter(id, goal, true)
    }

    /// `AIMoveAndDeleteState::update` (`AIStates.cpp:3745`): delete self once the move finishes.
    fn ai_move_and_delete_update(&mut self, id: ObjectId) -> StateReturn {
        let status = self.ai_internal_move_update(id);
        if status == StateReturn::Success {
            self.destroy_object(id);
        }
        status
    }

    /// `AIWanderInPlaceState::onEnter` (`AIStates.cpp:4691`).
    fn ai_wander_in_place_enter(&mut self, id: ObjectId) -> StateReturn {
        let origin = self.objects.get(&id).map(|o| o.position).unwrap_or(Coord3D::ZERO);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.wander_origin = origin;
        }
        self.ai_wander_pick_goal(id, &origin);
        let goal = self.objects.get(&id).map(|o| o.ai.machine.goal_position).unwrap_or(origin);
        self.ai_internal_move_enter(id, goal, true)
    }

    /// `AIWanderInPlaceState::update` (`AIStates.cpp:4718`).
    fn ai_wander_in_place_update(&mut self, id: ObjectId) -> StateReturn {
        let status = self.ai_internal_move_update(id);
        if status != StateReturn::Continue {
            let origin = self.objects.get(&id).map(|o| o.ai.machine.wander_origin).unwrap_or(Coord3D::ZERO);
            self.ai_wander_pick_goal(id, &origin);
            let goal = self.objects.get(&id).map(|o| o.ai.machine.goal_position).unwrap_or(origin);
            self.ai_internal_move_enter(id, goal, true);
        }
        StateReturn::Continue
    }

    /// `AIWanderInPlaceState`'s random offset within the locomotor's wander radius
    /// (`AIStates.cpp:4700`); the sim uses a fixed 3-cell radius (`getWanderAboutPointRadius` is
    /// not parsed).
    fn ai_wander_pick_goal(&mut self, id: ObjectId, origin: &Coord3D) {
        let delta = 3;
        let ox = self.rng.value(-delta, delta) as f32 * crate::pathfind::PATHFIND_CELL_SIZE_F;
        let oy = self.rng.value(-delta, delta) as f32 * crate::pathfind::PATHFIND_CELL_SIZE_F;
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.goal_position = Coord3D::new(origin.x + ox, origin.y + oy, origin.z);
        }
    }

    /// `AIPanicState::onEnter` (`AIStates.cpp:4802`); the `MODELCONDITION_PANICKING` bit is derived
    /// from the state in [`crate::condition`].
    fn ai_panic_enter(&mut self, id: ObjectId) -> StateReturn {
        self.ai_follow_path_enter(id)
    }
}

// ---- AI_DOCK ----------------------------------------------------------------------------------

/// Where a dock sub-state goes on success / failure (`AIDockMachine::AIDockMachine`,
/// `AIDock.cpp:52`).
enum DockNext {
    Sub(DockSub),
    ExitSuccess,
    ExitFailure,
}

fn dock_transition(sub: DockSub, success: bool) -> DockNext {
    use DockNext::*;
    match (sub, success) {
        (DockSub::Approach, true) => Sub(DockSub::WaitForClearance),
        (DockSub::WaitForClearance, true) => Sub(DockSub::MoveToEntry),
        (DockSub::Advance, true) => Sub(DockSub::WaitForClearance),
        (DockSub::MoveToEntry, true) => Sub(DockSub::MoveToDock),
        (DockSub::MoveToEntry, false) => Sub(DockSub::MoveToExit),
        (DockSub::MoveToDock, true) => Sub(DockSub::ProcessDock),
        (DockSub::MoveToDock, false) => Sub(DockSub::MoveToExit),
        (DockSub::ProcessDock, _) => Sub(DockSub::MoveToExit),
        (DockSub::MoveToExit, true) => Sub(DockSub::MoveToRally),
        (DockSub::MoveToRally, true) => ExitSuccess,
        _ => ExitFailure,
    }
}

impl GameLogic {
    /// `AIUpdateInterface::aiDock` (`AIUpdate.cpp:3801`): dock with `dock`, as an AI command.
    pub fn ai_dock(&mut self, id: ObjectId, dock: ObjectId) {
        self.ai_dock_command(id, dock, CommandSource::Ai);
    }

    /// `AIUpdateInterface::privateDock` plus `SupplyTruckAIUpdate::privateDock`
    /// (`SupplyTruckAIUpdate.cpp:172`): a player's dock order becomes the truck's preferred dock.
    pub(crate) fn ai_dock_command(&mut self, id: ObjectId, dock: ObjectId, source: CommandSource) {
        if !self.is_mobile(id) {
            return;
        }
        // `privateDock` (`AIUpdate.cpp:3806`) calls `getStateMachine()->clear()` *before*
        // `setGoalObject`, so an old `AI_DOCK`'s `onExit` -> `AIDockMachine::halt` cancels its
        // reservation with the old goal. Setting the new goal first leaked the old dock's
        // `active_docker`/`docker_inside` forever.
        self.ai_clear(id);
        let dock_pos = self.objects.get(&dock).map_or(Coord3D::ZERO, |d| d.position);
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack = None;
            o.combat.attack_move = false;
            o.combat.guard = None;
            o.ai.machine.goal_object = dock;
            o.ai.machine.goal_position = dock_pos;
            o.ai.machine.dock_sub = DockSub::Approach;
            o.ai.machine.dock_approach = -1;
            o.ai.machine.last_command_source = source;
        }
        self.ai_set_state(id, AiStateType::Dock);
        if source == CommandSource::Player && dock != ObjectId::INVALID {
            if let Some(o) = self.objects.get_mut(&id) {
                if o.template.supply_truck.is_some() {
                    o.supply_ai.preferred_dock = dock;
                }
            }
        }
    }

    fn ai_dock_goal(&self, id: ObjectId) -> ObjectId {
        self.objects.get(&id).map_or(ObjectId::INVALID, |o| o.ai.machine.goal_object)
    }

    /// `AIStateMachine::getGoalObject` + `getDockUpdateInterface`: the dock, if it still exists and
    /// has one.
    fn ai_dock_target(&self, id: ObjectId) -> Option<ObjectId> {
        let dock = self.ai_dock_goal(id);
        self.has_dock_interface(dock).then_some(dock)
    }

    pub(crate) fn ai_set_ignore_obstacle(&mut self, id: ObjectId, obstacle: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.ignore_obstacle_id = obstacle;
        }
    }

    fn ai_set_machine_locked(&mut self, id: ObjectId, locked: bool) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.locked = locked;
        }
    }

    /// `AIDockState::onEnter` (`AIStates.cpp:6110`).
    fn ai_dock_enter(&mut self, id: ObjectId) -> StateReturn {
        let Some(dock) = self.ai_dock_target(id) else { return StateReturn::Failure };
        // Tell the pathfinder to ignore the object we are docking with, so it doesn't block us.
        self.ai_set_ignore_obstacle(id, dock);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.dock_approach = -1;
            o.ai.machine.dock_sub = DockSub::Approach;
        }
        // `m_dockMachine->initDefaultState()`: enter `AI_DOCK_APPROACH`.
        let ret = self.ai_dock_sub_enter(id, DockSub::Approach);
        self.ai_dock_check_transitions(id, DockSub::Approach, ret)
    }

    /// `AIDockState::update` (`AIStates.cpp:6174`): run the dock sub-machine.
    fn ai_dock_update(&mut self, id: ObjectId) -> StateReturn {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.can_path_through_units = true;
        }
        let sub = self.objects.get(&id).map_or(DockSub::Approach, |o| o.ai.machine.dock_sub);
        let ret = self.ai_dock_sub_update(id, sub);
        self.ai_dock_check_transitions(id, sub, ret)
    }

    /// `AIDockState::onExit` (`AIStates.cpp:6149-6168`): `m_dockMachine->halt()` and the delete.
    /// `AIDockMachine::halt` (`AIDock.cpp:70-86`) only calls `dock->cancelDock(getOwner())` and then
    /// `StateMachine::halt`, which **does not run the current sub-state's `onExit`**
    /// (`StateMachine.cpp:753-763`: `m_locked = true; m_currentState = nullptr;`). So no
    /// `onApproachReached`/`onEnterReached`/`onExitReached` and no `AIInternalMoveToState::onExit`
    /// (`friend_endingMove`) runs on an interrupted dock; the sub-state's exit side effects exist only
    /// on a real transition (`internalSetState`, `StateMachine.cpp:608-610`). `AIDockState::onExit`
    /// then clears the passthrough flag and the ignored obstacle.
    pub(crate) fn ai_dock_exit(&mut self, id: ObjectId) {
        let dock = self.ai_dock_goal(id);
        if self.has_dock_interface(dock) {
            self.dock_cancel(dock, id);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.dock_sub = DockSub::Approach;
            o.ai.machine.dock_approach = -1;
            // `setCanPathThroughUnits(false); ignoreObstacle(nullptr)` (`AIStates.cpp:6164-6165`).
            o.ai.can_path_through_units = false;
            o.ai.ignore_obstacle_id = ObjectId::INVALID;
        }
    }

    /// `State::friend_checkForTransitions` for the dock sub-machine (`StateMachine.cpp:112`).
    fn ai_dock_check_transitions(&mut self, id: ObjectId, sub: DockSub, ret: StateReturn) -> StateReturn {
        match ret {
            StateReturn::Continue | StateReturn::Sleep(_) => {
                // `AIDockMachine::ableToAdvance` (`AIDock.cpp:128`), a WaitForClearance condition.
                if sub == DockSub::WaitForClearance {
                    let dock = self.ai_dock_goal(id);
                    let idx = self.objects.get(&id).map_or(-1, |o| o.ai.machine.dock_approach);
                    if self.has_dock_interface(dock) && self.dock_is_clear_to_advance(dock, id, idx) {
                        return self.ai_dock_set_sub(id, DockSub::Advance);
                    }
                }
                StateReturn::Continue
            }
            StateReturn::Success | StateReturn::Failure => match dock_transition(sub, ret == StateReturn::Success) {
                DockNext::Sub(next) => self.ai_dock_set_sub(id, next),
                DockNext::ExitSuccess => {
                    self.ai_dock_sub_exit(id, sub);
                    StateReturn::Success
                }
                DockNext::ExitFailure => {
                    self.ai_dock_sub_exit(id, sub);
                    StateReturn::Failure
                }
            },
        }
    }

    /// `StateMachine::internalSetState` on the dock machine (`StateMachine.cpp:585`).
    fn ai_dock_set_sub(&mut self, id: ObjectId, new: DockSub) -> StateReturn {
        let old = self.objects.get(&id).map_or(DockSub::Approach, |o| o.ai.machine.dock_sub);
        self.ai_dock_sub_exit(id, old);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.dock_sub = new;
        }
        let ret = self.ai_dock_sub_enter(id, new);
        self.ai_dock_check_transitions(id, new, ret)
    }

    /// The sub-states' `onEnter` (`AIDock.cpp`).
    fn ai_dock_sub_enter(&mut self, id: ObjectId, sub: DockSub) -> StateReturn {
        let now = self.frame;
        match sub {
            // `AIDockApproachState::onEnter` (`AIDock.cpp:167`).
            DockSub::Approach => {
                let Some(dock) = self.ai_dock_target(id) else { return StateReturn::Failure };
                if !self.dock_is_open(dock) {
                    self.dock_cancel(dock, id);
                    return StateReturn::Failure;
                }
                let Some((pos, idx)) = self.dock_reserve_approach(dock, id) else { return StateReturn::Failure };
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.dock_approach = idx;
                }
                self.ai_set_ignore_obstacle(id, ObjectId::INVALID);
                self.ai_internal_move_enter(id, pos, true)
            }
            // `AIDockWaitForClearanceState::onEnter` (`AIDock.cpp:248`).
            DockSub::WaitForClearance => {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.dock_enter_frame = now;
                }
                StateReturn::Continue
            }
            // `AIDockAdvancePositionState::onEnter` (`AIDock.cpp:313`).
            DockSub::Advance => {
                let Some(dock) = self.ai_dock_target(id) else { return StateReturn::Failure };
                if !self.dock_is_open(dock) {
                    self.dock_cancel(dock, id);
                    return StateReturn::Failure;
                }
                let idx = self.objects.get(&id).map_or(-1, |o| o.ai.machine.dock_approach);
                let Some((pos, new_idx)) = self.dock_advance_approach(dock, id, idx) else { return StateReturn::Failure };
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.dock_approach = new_idx;
                }
                self.ai_set_ignore_obstacle(id, ObjectId::INVALID);
                self.ai_internal_move_enter(id, pos, true)
            }
            // `AIDockMoveToEntryState::onEnter` (`AIDock.cpp:386`).
            DockSub::MoveToEntry => {
                let Some(dock) = self.ai_dock_target(id) else { return StateReturn::Failure };
                if !self.dock_is_open(dock) {
                    self.dock_cancel(dock, id);
                    return StateReturn::Failure;
                }
                if self.dock_allows_passthrough(dock) {
                    self.ai_set_ignore_obstacle(id, dock);
                }
                let pos = self.dock_enter_position(dock, id);
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.dock_approach = -1;
                }
                self.ai_internal_move_enter(id, pos, true)
            }
            // `AIDockMoveToDockState::onEnter` (`AIDock.cpp:459`).
            DockSub::MoveToDock => {
                let Some(dock) = self.ai_dock_target(id) else { return StateReturn::Failure };
                if !self.dock_is_open(dock) {
                    self.dock_cancel(dock, id);
                    return StateReturn::Failure;
                }
                let pos = self.dock_dock_position(dock, id);
                let mut adjusts = true;
                if self.dock_allows_passthrough(dock) {
                    self.ai_set_ignore_obstacle(id, dock);
                    adjusts = false; // `setAdjustsDestination(false)`
                }
                // `getMachine()->lock("AIDockMoveToDockState::onEnter")` (`AIDock.cpp:515`) locks the
                // *dock* machine, not the owner's AI machine: nothing can set the dock machine's state
                // from outside, so the lock has no effect on the main machine's commands. The port has
                // no separate dock machine to lock (see `ai_dock_exit`).
                self.ai_internal_move_enter(id, pos, adjusts)
            }
            // `AIDockProcessDockState::onEnter` (`AIDock.cpp:552`).
            DockSub::ProcessDock => {
                if self.ai_dock_target(id).is_none() {
                    return StateReturn::Failure;
                }
                self.ai_dock_set_next_action_frame(id);
                StateReturn::Continue
            }
            // `AIDockMoveToExitState::onEnter` (`AIDock.cpp:675`).
            DockSub::MoveToExit => {
                let Some(dock) = self.ai_dock_target(id) else { return StateReturn::Failure };
                let pos = self.dock_exit_position(dock, id);
                let mut adjusts = true;
                if self.dock_allows_passthrough(dock) {
                    self.ai_set_ignore_obstacle(id, dock);
                    adjusts = false;
                }
                self.ai_internal_move_enter(id, pos, adjusts)
            }
            // `AIDockMoveToRallyState::onEnter` (`AIDock.cpp:772`): only repair docks send the
            // docker on to their rally point.
            DockSub::MoveToRally => {
                let Some(dock) = self.ai_dock_target(id) else { return StateReturn::Failure };
                let is_repair = self.dock_module(dock).is_some_and(|(c, _, _)| c == crate::dock::DockClass::Repair);
                let rally = self.objects.get(&dock).and_then(|d| d.production.as_ref().and_then(|p| p.rally_point));
                match (is_repair, rally) {
                    (true, Some(r)) => self.ai_internal_move_enter(id, r, true),
                    _ => StateReturn::Success,
                }
            }
        }
    }

    /// `AIDockProcessDockState::setNextDockActionFrame` (`AIDock.cpp:531`).
    fn ai_dock_set_next_action_frame(&mut self, id: ObjectId) {
        let dock = self.ai_dock_goal(id);
        let delay = self.dock_action_delay(id, dock);
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.dock_next_action = now + delay;
        }
    }

    /// The sub-states' `update` (`AIDock.cpp`).
    fn ai_dock_sub_update(&mut self, id: ObjectId, sub: DockSub) -> StateReturn {
        let now = self.frame;
        match sub {
            DockSub::Approach | DockSub::Advance | DockSub::MoveToEntry | DockSub::MoveToExit => {
                let goal = self.ai_dock_goal(id);
                if goal == ObjectId::INVALID || !self.objects.contains_key(&goal) {
                    return StateReturn::Failure;
                }
                self.ai_internal_move_update(id)
            }
            // `AIDockWaitForClearanceState::update` (`AIDock.cpp:262`).
            DockSub::WaitForClearance => {
                let Some(dock) = self.ai_dock_target(id) else { return StateReturn::Failure };
                if !self.dock_is_open(dock) {
                    self.dock_cancel(dock, id);
                    return StateReturn::Failure;
                }
                if self.dock_is_clear_to_enter(dock, id) {
                    return StateReturn::Success;
                }
                let enter = self.objects.get(&id).map_or(0, |o| o.ai.machine.dock_enter_frame);
                if enter + 30 * LOGICFRAMES_PER_SECOND < now {
                    return StateReturn::Failure;
                }
                StateReturn::Continue
            }
            // `AIDockMoveToDockState::update` (`AIDock.cpp:508`).
            DockSub::MoveToDock => {
                let Some(dock) = self.ai_dock_target(id) else { return StateReturn::Failure };
                if !self.dock_is_open(dock) {
                    return StateReturn::Failure;
                }
                self.ai_internal_move_update(id)
            }
            // `AIDockProcessDockState::update` (`AIDock.cpp:578`): invoke the dock's action until
            // it says it is done or the dock closes.
            DockSub::ProcessDock => {
                let Some(dock) = self.ai_dock_target(id) else { return StateReturn::Failure };
                let next = self.objects.get(&id).map_or(0, |o| o.ai.machine.dock_next_action);
                if now < next {
                    return StateReturn::Continue;
                }
                self.ai_dock_set_next_action_frame(id);
                let drone = self.dock_find_drone(id);
                if !self.dock_is_open(dock) || !self.dock_action(id, dock, drone) {
                    return StateReturn::Success;
                }
                StateReturn::Continue
            }
            // `AIDockMoveToRallyState::update`: fine with losing the goal object after the move starts.
            DockSub::MoveToRally => self.ai_internal_move_update(id),
        }
    }

    /// The sub-states' `onExit(EXIT_NORMAL)` (`AIDock.cpp`).
    fn ai_dock_sub_exit(&mut self, id: ObjectId, sub: DockSub) {
        let dock = self.ai_dock_target(id);
        match sub {
            DockSub::Approach | DockSub::Advance => {
                if let Some(dock) = dock {
                    if !self.dock_is_open(dock) {
                        self.dock_cancel(dock, id);
                    } else {
                        self.dock_on_approach_reached(dock, id);
                    }
                }
                self.ai_internal_move_exit(id);
            }
            DockSub::WaitForClearance => {
                if let Some(dock) = dock {
                    if !self.dock_is_open(dock) {
                        self.dock_cancel(dock, id);
                    }
                }
            }
            DockSub::MoveToEntry => {
                if let Some(dock) = dock {
                    if !self.dock_is_open(dock) {
                        self.dock_cancel(dock, id);
                    } else {
                        self.dock_on_enter_reached(dock, id);
                    }
                }
                self.ai_internal_move_exit(id);
            }
            DockSub::MoveToDock => {
                // `onDockReached` only sets model conditions, which the sim derives (`AIDock.cpp:549-557`).
                if let Some(dock) = dock {
                    if !self.dock_is_open(dock) {
                        self.dock_cancel(dock, id);
                    }
                }
                // `getMachine()->unlock()` (`:560`) unlocks the dock machine; see `ai_dock_exit`.
                self.ai_internal_move_exit(id);
            }
            // `AIDockProcessDockState::onExit` (`:694-698`) only unlocks the dock machine.
            DockSub::ProcessDock => {}
            DockSub::MoveToExit => {
                // `dock->onExitReached(getMachineOwner())` runs for every exit status (`:754-756`).
                if let Some(dock) = dock {
                    self.dock_on_exit_reached(dock, id);
                }
                self.ai_internal_move_exit(id);
            }
            DockSub::MoveToRally => self.ai_internal_move_exit(id),
        }
    }
}

// ---- AI_ENTER / AI_EXIT -----------------------------------------------------------------------

impl GameLogic {
    /// `AIEnterState::onEnter` (`AIStates.cpp:6237`).
    fn ai_enter_enter(&mut self, id: ObjectId) -> StateReturn {
        let goal = self.objects.get(&id).map(|o| o.ai.machine.goal_object).unwrap_or(ObjectId::INVALID);
        // `canEnterObject(..., CHECK_CAPACITY)`: a contain-less enter target (a Hijacker's vehicle,
        // a Terrorist's carbomb car, a Saboteur's building) passes through the collide early-out.
        if goal == ObjectId::INVALID || !self.can_enter_object(id, goal, true) {
            return StateReturn::Failure;
        }
        let cpos = self.objects.get(&goal).map(|c| c.position).unwrap_or(Coord3D::ZERO);
        if let Some(o) = self.objects.get_mut(&id) {
            o.enter_target = Some(goal);
            o.ai.machine.entry_to_clear = goal;
            o.ai.machine.goal_position = cpos;
            o.ai.machine.goal_object = goal;
            o.combat.attack = None;
            // `ai->ignoreObstacle(getMachineGoalObject())`: path onto the target instead of around
            // it, so a saboteur can walk into the building's footprint and the collide fires.
            o.ai.ignore_obstacle_id = goal;
        }
        self.ai_internal_move_enter(id, cpos, false)
    }

    /// `AIEnterState::update` (`AIStates.cpp:6317`).
    fn ai_enter_update(&mut self, id: ObjectId) -> StateReturn {
        let goal = self.objects.get(&id).map(|o| o.ai.machine.goal_object).unwrap_or(ObjectId::INVALID);
        if goal == ObjectId::INVALID {
            return StateReturn::Failure;
        }
        // "if our goal is contained by something else, give up. this is for the following bug:
        // -- tell some rangers to enter a humvee
        // -- tell the humvee to enter a chinook
        // -- fly the chinook around; the rangers follow the chinook like dopes"
        // (`AIStates.cpp:6326-6331`). The goal must be contained, above the terrain, and the owner not.
        if self.is_contained(goal) && self.object_above_terrain(goal) && !self.object_above_terrain(id) {
            return StateReturn::Failure;
        }
        if self.objects.get(&id).is_some_and(|o| o.contained_by == Some(goal)) {
            return StateReturn::Success;
        }
        let Some(cpos) = self.objects.get(&goal).map(|c| c.position) else {
            return StateReturn::Failure;
        };
        // `if (!TheActionManager->canEnterObject(obj, goal, ..., CHECK_CAPACITY)) return STATE_FAILURE`
        // (AIStates.cpp:6337): the capacity/validity check runs every update, not only onEnter, so a
        // second passenger that lost the race for the last slot gives up.
        if !self.can_enter_object(id, goal, true) {
            // Special case (`AIStates.cpp:6339-6372`): "if it's an enemy, try attacking it instead".
            // A building the unit was trying to garrison that an enemy garrisoned first must be
            // attacked, not dropped. `getCanAttackObject(..., ATTACK_NEW_TARGET)` must return
            // `ATTACKRESULT_POSSIBLE` or `_POSSIBLE_AFTER_MOVING` (`ActionManager.cpp:730-745`), then
            // `aiAttackObject(goal, NO_MAX_SHOTS_LIMIT, getLastCommandSource())`, and the state
            // returns `STATE_CONTINUE` ("if we're here, we're actually an attack state").
            if self.ai_enter_fallback_attack(id, goal) {
                return StateReturn::Continue;
            }
            return StateReturn::Failure;
        }
        // Keep the goal on the (possibly moving) container.
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.goal_position = cpos;
            o.ai.move_goal = Some(cpos);
        }
        let code = self.ai_internal_move_update(id);
        if code == StateReturn::Success {
            // `AIEnterState::update` (AIStates.cpp:6376-6422): the move reports success; if the goal is
            // airborne and we are not (RETAIL_COMPATIBLE_CRC), wait. Otherwise, when we are within the
            // container's own radius, force ourselves into it (`contain->addToContain(obj)`): the
            // locomotor's close-enough distance can stop us slightly outside it, and a unit on the map
            // border generates no collision at all.
            // `Object::isAboveTerrain`: the surface under the object plus a small epsilon.
            if self.object_above_terrain(goal) && !self.object_above_terrain(id) {
                return StateReturn::Continue;
            }
            let close = match (self.objects.get(&goal), self.objects.get(&id)) {
                (Some(c), Some(u)) => {
                    let radius = match c.template.geometry.shape {
                        crate::template::GeometryShape::Box => c.template.geometry.minor_radius,
                        _ => c.template.geometry.major_radius,
                    };
                    let dx = u.position.x - c.position.x;
                    let dy = u.position.y - c.position.y;
                    dx * dx + dy * dy < radius * radius
                }
                _ => false,
            };
            if close {
                self.add_to_contain(goal, id);
            }
        }
        // The port's collision dispatch does not reproduce the engine's contact list exactly (a unit
        // stopped just outside a structure's footprint never overlaps it), so the proximity add the
        // port used to run in a global sweep stays here as the stand-in for `OpenContain::onCollide`
        // (OpenContain.cpp:850-907): a unit whose `AIEnterState` targets the container and is close
        // enters it. It runs after the engine-shaped success test above, so a move that really
        // arrived uses the engine's radius.
        if !self.is_contained(id) {
            let near = match (self.objects.get(&goal), self.objects.get(&id)) {
                (Some(c), Some(u)) => {
                    let range = c.template.geometry.bounding_circle_radius() + u.template.geometry.bounding_circle_radius() + 10.0;
                    u.position.dist_2d(&c.position) <= range
                }
                _ => false,
            };
            if near && self.is_valid_container_for(goal, id, true) {
                self.add_to_contain(goal, id);
            }
        }
        code
    }

    /// `Object::isAboveTerrain` (`Object.cpp`): the object's z is above the terrain surface under it
    /// plus the engine's small epsilon.
    fn object_above_terrain(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.position.z > self.terrain.surface_height(o.position.x, o.position.y, o.position.z) + 0.001)
    }

    /// `AIEnterState::update`'s enemy fallback (`AIStates.cpp:6339-6372`): when `canEnterObject`
    /// fails on an enemy goal, `getCanAttackObject(..., ATTACK_NEW_TARGET)` (`ActionManager.cpp:730`)
    /// decides between an `aiAttackObject(goal, NO_MAX_SHOTS_LIMIT, getLastCommandSource())` and a
    /// plain `STATE_FAILURE`. The `ATTACKRESULT_POSSIBLE` / `_POSSIBLE_AFTER_MOVING` pair is the
    /// port's usual `isAbleToAttack` + weapon-targetability test (see `ai_guard_look_for_target`).
    fn ai_enter_fallback_attack(&mut self, id: ObjectId, goal: ObjectId) -> bool {
        let Some((player, source)) = self.objects.get(&id).map(|o| (o.player, o.ai.machine.last_command_source)) else { return false };
        let enemy = self
            .objects
            .get(&goal)
            .is_some_and(|g| !g.destroyed && !g.effectively_dead && self.relationship(player, g.player) == crate::damage::Relationship::Enemies);
        if !enemy || !self.is_able_to_attack(id) || self.is_stealthed(goal) {
            return false;
        }
        let Some(info) = self.target_info(&AttackTarget::Object(goal), player, false) else { return false };
        if self.objects.get(&id).is_none_or(|o| Self::choose_weapon(o, Some(&info), self.frame, None).is_none()) {
            return false;
        }
        // `aiAttackObject(goal, NO_MAX_SHOTS_LIMIT, obj->getAI()->getLastCommandSource())`: no shot
        // limit (`None`), not forced, not following.
        self.ai_attack_object_from(id, goal, false, false, None, source);
        true
    }

    /// `AIEnterState::onExit` (`AIStates.cpp:6274`).
    fn ai_enter_exit(&mut self, id: ObjectId) {
        // `AIInternalMoveToState::onExit(status)` runs first (`AIStates.cpp:6276`): `friend_endingMove`,
        // so the completed-move block of this update drops the entry path and clears `isMoving`. The
        // common case is masked by `add_to_contain`'s `stop()`, but an entry that ends outside the
        // container's radius (`AIEnterState::update` reports the move's success there) or a cancelled
        // enter would otherwise keep its path and locomotor goal.
        self.ai_internal_move_exit(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.enter_target = None;
            o.ai.machine.entry_to_clear = ObjectId::INVALID;
            // `ai->ignoreObstacle(nullptr)`.
            if o.ai.ignore_obstacle_id == o.ai.machine.goal_object {
                o.ai.ignore_obstacle_id = ObjectId::INVALID;
            }
        }
    }

    /// `AIExitState::onEnter` (`AIStates.cpp:6457`).
    fn ai_exit_enter(&mut self, id: ObjectId) -> StateReturn {
        let goal = self.objects.get(&id).map(|o| o.ai.machine.goal_object).unwrap_or(ObjectId::INVALID);
        let Some(container) = self.objects.get(&goal).map(|c| c.id) else {
            return StateReturn::Failure;
        };
        // A tunnel network shares one passenger list across its entrances, so membership is tested
        // against `contained_units` (which merges the network), not `contained_by`.
        let inside = self.contained_units(container).contains(&id);
        if !inside {
            return StateReturn::Failure;
        }
        // `AIExitInstantlyState::onEnter` (AIStates.cpp:6601): the exit happens now, with no busy or door check.
        // `AIExitState::onEnter` (AIStates.cpp:6457) only notes the wish to leave; its `update` does the exit.
        if self.objects.get(&id).is_some_and(|o| o.ai.machine.current == crate::ai_states::AiStateType::ExitInstantly) {
            self.exit_object_via_door(container, id);
        }
        StateReturn::Continue
    }

    /// `AIExitState::update` (`AIStates.cpp:6480`) and `AIExitInstantlyState::update` (`:6612`), in `contain_exit.rs`.
    fn ai_exit_update(&mut self, id: ObjectId) -> StateReturn {
        let goal = self.objects.get(&id).map(|o| o.ai.machine.goal_object).unwrap_or(ObjectId::INVALID);
        self.ai_exit_state_update(id, goal)
    }

    fn ai_exit_exit(&mut self, id: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.exit_at = 0;
            o.ai.machine.entry_to_clear = ObjectId::INVALID;
        }
    }
}

// ---- AI_FACE_OBJECT / AI_FACE_POSITION --------------------------------------------------------

impl GameLogic {
    /// `AIUpdateInterface::aiFaceObject` (`AI.h:798` -> `aiDoCommand`, `AIUpdate.cpp:2839`) ->
    /// `privateFaceObject` (`:2955-2974`): clear the machine, make `target` its goal object, reset the
    /// blocked flags and enter `AI_FACE_OBJECT`. The dead do not listen
    /// (`isAllowedToRespondToAiCommands`, `:2597`); a jet's own `aiDoCommand` runs first
    /// (`JetAIUpdate.cpp:2572`), as for every other order.
    pub(crate) fn ai_face_object(&mut self, id: ObjectId, target: ObjectId, source: CommandSource) {
        if !self.is_mobile(id) || self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return;
        }
        if !self.is_jet(id) {
            self.ai_clear(id);
        }
        // `StateMachine::setGoalObject` (`StateMachine.cpp:766-776`): the id and the target's position.
        let target_pos = self.objects.get(&target).map(|t| t.position);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.goal_object = if target_pos.is_some() { target } else { ObjectId::INVALID };
            if let Some(p) = target_pos {
                o.ai.machine.goal_position = p;
            }
        }
        self.ai_reset_blocked(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.last_command_source = source;
        }
        self.ai_command_set_state(id, AiStateType::FaceObject);
    }

    /// `AIUpdateInterface::aiFacePosition` (`AI.h:805` -> `privateFacePosition`, `AIUpdate.cpp:2984`):
    /// clear the machine, set the (clipped) goal position, reset the blocked flags and enter
    /// `AI_FACE_POSITION`.
    pub(crate) fn ai_face_position(&mut self, id: ObjectId, pos: Coord3D, source: CommandSource) {
        if !self.is_mobile(id) || self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return;
        }
        if !self.is_jet(id) {
            self.ai_clear(id);
        }
        let goal = self.set_goal_position_clipped(id, pos, source);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.goal_position = goal;
        }
        self.ai_reset_blocked(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.last_command_source = source;
        }
        self.ai_command_set_state(id, AiStateType::FacePosition);
    }

    /// `AIFaceState::onEnter` (`AIStates.cpp:7459-7476`): `m_canTurnInPlace` is whether the current
    /// locomotor can stand still (`getMinSpeed() == 0`), and a face-object state whose goal object is
    /// gone fails at once.
    fn ai_face_enter(&mut self, id: ObjectId) -> StateReturn {
        let Some(o) = self.objects.get(&id) else { return StateReturn::Failure };
        let can_turn_in_place = o.locomotor().is_some_and(|l| l.min_speed == 0.0);
        let is_object = o.ai.machine.current == AiStateType::FaceObject;
        let has_target = self.objects.contains_key(&o.ai.machine.goal_object);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.face_can_turn_in_place = can_turn_in_place;
        }
        // `if (m_obj && target == nullptr) return STATE_FAILURE;` — `getMachineGoalObject()` resolves the id.
        if is_object && !has_target {
            return StateReturn::Failure;
        }
        StateReturn::Continue
    }

    /// `PartitionManager::getRelativeAngle2D` (`PartitionManager.cpp:3634-3680`): the signed angle from
    /// the object's facing to `pos`, `ACos` of the dot of the two unit vectors with the sign of their
    /// cross product's z. Not `atan2(dy, dx) - orientation`: the engine's `ACos` form is what the 0.035
    /// threshold (`AIStates.cpp:7502`) is tuned against.
    fn relative_angle_2d(&self, id: ObjectId, pos: Coord3D) -> f32 {
        let Some(o) = self.objects.get(&id) else { return 0.0 };
        let (dx, dy) = (pos.x - o.position.x, pos.y - o.position.y);
        let dist = crate::math::sqrt(dx * dx + dy * dy);
        if dist == 0.0 {
            return 0.0;
        }
        let (dirx, diry) = (crate::math::cos(o.angle), crate::math::sin(o.angle));
        let (vx, vy) = (dx / dist, dy / dist);
        let c = (dirx * vx + diry * vy).clamp(-1.0, 1.0);
        let mut value = crate::math::acos(c);
        // "Determine sign by checking Z component of dir cross v".
        if dirx * vy - diry * vx < 0.0 {
            value = -value;
        }
        value
    }

    /// `AIFaceState::update` (`AIStates.cpp:7483-7520`): succeed within 0.035 rad of the goal, else point
    /// the locomotor at it — a goal orientation when the locomotor can turn in place, an explicit goal
    /// position (a driving turn) when it cannot.
    fn ai_face_update(&mut self, id: ObjectId) -> StateReturn {
        let (is_object, goal_object, goal_position, can_turn_in_place, angle) = match self.objects.get(&id) {
            Some(o) => (
                o.ai.machine.current == AiStateType::FaceObject,
                o.ai.machine.goal_object,
                o.ai.machine.goal_position,
                o.ai.machine.face_can_turn_in_place,
                o.angle,
            ),
            None => return StateReturn::Failure,
        };
        let target = if is_object {
            match self.objects.get(&goal_object) {
                Some(t) => t.position,
                // "Nothing to face."
                None => return StateReturn::Failure,
            }
        } else {
            goal_position
        };
        let rel = self.relative_angle_2d(id, target);
        if rel.abs() < 0.035 {
            return StateReturn::Success;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            if can_turn_in_place {
                // `setLocomotorGoalOrientation(obj->getOrientation() + relAngle)`.
                o.ai.set_locomotor_goal_orientation(angle + rel);
            } else {
                // `setLocomotorGoalPositionExplicit(*pos)`.
                o.ai.set_locomotor_goal_explicit(target);
            }
        }
        StateReturn::Continue
    }
}


// ---- crate pickup and the team common target ----------------------------------------------------

impl GameLogic {
    /// `AIUpdateInterface::checkForCrateToPickup` (`AIUpdate.cpp:893`). Retail (and the maintained
    /// fork) clear `m_crateCreated` *before* looking the crate up by that same id, so the lookup
    /// always fails and the function always returns null: AI units never walk to a crate. Ported as
    /// is (the `AI_PICK_UP_CRATE` branches of the callers are unreachable); only the clearing side
    /// effect matters, because `AIGuardIdleState::update` branches on a pending crate id.
    pub(crate) fn check_for_crate_to_pickup(&mut self, id: ObjectId) -> Option<ObjectId> {
        if let Some(o) = self.objects.get_mut(&id) {
            if o.ai.crate_created != ObjectId::INVALID {
                o.ai.crate_created = ObjectId::INVALID;
            }
        }
        None
    }

    /// The key of the script team instance `id` belongs to (absent: the player's default team).
    fn team_key_of_object(&self, id: ObjectId) -> Option<&str> {
        self.scripts.teams.iter().find(|(_, t)| t.members.contains(&id)).map(|(k, _)| k.as_str())
    }

    /// `obj->getTeam()->getPrototype()->getTemplateInfo()->m_attackCommonTarget`.
    pub(crate) fn team_attacks_common_target(&self, id: ObjectId) -> bool {
        let Some(key) = self.team_key_of_object(id) else { return false };
        let proto = self.scripts.proto_of(key);
        self.scripts.team_protos.get(&proto).is_some_and(|p| p.attack_common_target)
    }

    /// `Team::setTeamTargetObject` (`Team.cpp:1500`): only computer players share a target, and
    /// never on EASY. `None` clears it.
    pub(crate) fn set_team_target_object(&mut self, id: ObjectId, target: Option<ObjectId>) {
        let Some(key) = self.team_key_of_object(id).map(str::to_owned) else { return };
        let new = match target {
            None => ObjectId::INVALID,
            Some(t) => {
                let Some(player) = self.objects.get(&id).map(|o| o.player) else { return };
                let Some(ai) = self.ai_players.get(&player) else { return };
                if ai.config.difficulty == crate::script::Difficulty::Easy {
                    return;
                }
                t
            }
        };
        if let Some(t) = self.scripts.teams.get_mut(&key) {
            t.common_attack_target = new;
        }
    }

    /// `Team::getTeamTargetObject` (`Team.cpp:1516`): the shared target unless it is stealthed and
    /// undetected, dead, contained or an aircraft (which also clears it).
    pub(crate) fn get_team_target_object(&mut self, id: ObjectId) -> Option<ObjectId> {
        let key = self.team_key_of_object(id)?.to_owned();
        let tid = self.scripts.teams.get(&key)?.common_attack_target;
        if tid == ObjectId::INVALID {
            return None;
        }
        let valid = self.objects.get(&tid).is_some_and(|t| {
            !t.destroyed && !self.is_stealthed(tid) && !self.is_effectively_dead(tid) && t.contained_by.is_none() && !t.is_kind_of(kind_of::AIRCRAFT)
        });
        if !valid {
            if let Some(t) = self.scripts.teams.get_mut(&key) {
                t.common_attack_target = ObjectId::INVALID;
            }
            return None;
        }
        Some(tid)
    }

    /// Clears the team target when it is `victim` (`AIAttackState::update`, `AIStates.cpp:5651`).
    pub(crate) fn clear_team_target_if(&mut self, id: ObjectId, victim: ObjectId) {
        if self.get_team_target_object(id) == Some(victim) {
            self.set_team_target_object(id, None);
        }
    }
}

/// Lane 168 (the R18 review): the move-field ownership and the follow-path last point (`ai_l168.rs`).
#[cfg(test)]
#[path = "ai_l168.rs"]
mod l168_tests;

/// Wave 19 wrap-up: the attack-move's own `m_pathGoalPosition` (`ai_w19.rs`).
#[cfg(test)]
#[path = "ai_w19.rs"]
mod w19_tests;

/// Lane 173 (wave 20): the R19 review's AI findings (`ai_l173.rs`).
#[cfg(test)]
#[path = "ai_l173.rs"]
mod l173_tests;

/// Lane 176 (wave 20): the R20 review's AI findings (R20-1, R20-3, R20-9, S2, S3, S4).
#[cfg(test)]
#[path = "ai_l176.rs"]
mod l176_tests;

/// Lane 187 (wave 21): the R24 review's approach install, move-away install and tighten request (`ai_l187.rs`).
#[cfg(test)]
#[path = "ai_l187.rs"]
mod l187_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::{LocomotorKind, LocomotorParams, ObjectTemplate};
    use crate::terrain::TerrainLogic;
    use std::sync::Arc;

    /// A synthetic walking unit (no game data).
    fn walker() -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new("Walker");
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 5.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        Arc::new(t)
    }

    /// A hover aircraft: not doing ground movement, and an aircraft that adjusts its destination
    /// (`AIUpdate.cpp:2408`), so `updateAircraftGoal` marks its cell.
    fn hover_aircraft() -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new("Hover");
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 5.0;
        t.kind_of |= kind_of::AIRCRAFT;
        let mut loco = LocomotorParams::from_ini(LocomotorKind::Wings, 60.0, 100.0, 100.0, 360.0);
        loco.surfaces_air = true;
        t.locomotor = Some(loco);
        Arc::new(t)
    }

    fn world_with_walker(at: Coord3D) -> (GameLogic, ObjectId) {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let u = g.create_object(&walker(), at, 0.0, 0);
        (g, u)
    }

    fn machine(g: &GameLogic, u: ObjectId) -> AiMachine {
        g.find_object(u).expect("unit").ai.machine.clone()
    }

    /// `Pathfinder::updateGoal`'s ground-movement dispatch (`AIPathfind.cpp:9816-9827`): a unit that is
    /// not doing ground movement takes `updateAircraftGoal`, which removes its old ground goal and marks
    /// the aircraft goal cells for a hover/wing aircraft. The port returned before the call, so an air
    /// unit either kept a stale ground goal or went unmarked. Revert check: restore the early `return`
    /// for `!is_doing_ground_movement` and the hover aircraft's goal disappears.
    #[test]
    fn an_air_units_goal_update_takes_the_aircraft_arm() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let h = g.create_object(&hover_aircraft(), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
        let goal = Coord3D::new(200.0, 200.0, 20.0);
        assert!(!g.is_doing_ground_movement(h), "a hover aircraft is not doing ground movement");
        g.pathfind_update_goal(h, &goal);
        assert_eq!(
            g.ai_pathfind_goal(h).map(|p| (p.x, p.y)),
            Some((205.0, 205.0)),
            "the aircraft goal cell is the centred footprint's cell centre, as `updateAircraftGoal` marks it"
        );
        // The ground arm still keys a ground goal for a walker at the same spot.
        let w = g.create_object(&walker(), Coord3D::new(50.0, 300.0, 0.0), 0.0, 0);
        g.pathfind_update_goal(w, &Coord3D::new(200.0, 300.0, 0.0));
        assert!(g.ai_pathfind_goal(w).is_some(), "a ground unit keeps its ground goal");
    }

    /// `AIUpdateInterface::requestPath` (`AIUpdate.cpp:488-497`): "if it is possible to do it without
    /// having to use the pathfinder (air units just move point to point) it generates the path
    /// immediately" — `computeQuickPath` runs inside the call and `m_waitingForPath` is never set for it,
    /// so the move's first update runs its wait block and arms the locomotor (`AIStates.cpp:1875-1876`).
    /// The port queued every request, so an air move's path was only serviced after the AI sweep.
    ///
    /// The anti-spin guard (`if (m_pathTimestamp > getFrame()-3)`, `:499-517`) sits *after* the quick
    /// path check, so it never delays an air path.
    #[test]
    fn an_air_move_builds_its_quick_path_inside_the_request() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let h = g.create_object(&hover_aircraft(), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
        let goal = Coord3D::new(200.0, 200.0, 20.0);
        g.ai_request_path(h, goal, false);
        {
            let o = g.find_object(h).expect("the air unit");
            assert_eq!(o.ai.path, vec![goal], "the quick path is installed inside the call");
            assert_eq!(o.ai.path_timestamp, g.frame(), "`computeQuickPath` timestamps it");
            assert!(!o.ai.waiting_for_path, "a quick path never waits for the pathfinder");
        }
        assert!(!g.pathfinder.is_queued_for_path(h), "and the unit is not queued");

        // A second request in the same frame: the anti-spin wait does not apply to a quick path.
        let goal2 = Coord3D::new(150.0, 250.0, 20.0);
        g.ai_request_path(h, goal2, false);
        {
            let o = g.find_object(h).expect("the air unit");
            assert_eq!(o.ai.path, vec![goal2], "the new quick path replaces the old one");
            assert_eq!(o.ai.queue_for_path_frame, 0, "no one-second anti-spin wait");
        }

        // A ground unit still goes through the queue and waits.
        let w = g.create_object(&walker(), Coord3D::new(50.0, 300.0, 0.0), 0.0, 0);
        g.ai_request_path(w, Coord3D::new(200.0, 300.0, 0.0), false);
        assert!(g.pathfinder.is_queued_for_path(w), "a ground move is queued");
        assert!(g.find_object(w).expect("the walker").ai.waiting_for_path, "and waits for its path");
    }

    /// `Pathfinder::adjustDestination` (`AIPathfind.cpp:5462-5507`) runs for every non-projectile object,
    /// aircraft included: `checkForAdjust`'s `KINDOF_AIRCRAFT` case only skips the path test
    /// (`:5334-5343`), and `checkDestination`'s aircraft branch tests the goal-cell marks alone
    /// (`:5030-5038`) — the obstacle, impassable and surface tests do not run for it.
    ///
    /// The port skipped the whole adjust for an air-only `KINDOF_AIRCRAFT`, so a Chinook flew to the raw
    /// `findPositionAround` point. AN3 s1's dock approach: port (656.7874,163.8239) where C++ steers at
    /// the adjusted (660.5,160.5) (`Pathfinder::adjustDestination` + `adjustCoordToCell`'s centred
    /// formula `(cell + 0.5) * 10`, `AIPathfind.cpp:9014-9015`; the non-centred one is `(cell + 0.05) * 10`,
    /// `:9017-9018`). The trajectories stayed identical to f1639 because the 6 deg/frame turn clamp hid the
    /// 2 degree bearing difference until the clamp released.
    ///
    /// Revert check: restore `ai_adjust_move_goal`'s early `return goal`, and the adjusted point is no longer
    /// the goal cell. (`check_destination`'s aircraft branch has its own test,
    /// `an_aircrafts_move_goal_comes_from_the_aircraft_marks_alone`; this one does not depend on it.)
    #[test]
    fn an_aircrafts_move_goal_is_adjusted_to_its_goal_cell() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let h = g.create_object(&hover_aircraft(), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
        let goal = Coord3D::new(200.0, 200.0, 20.0);
        // `AIInternalMoveToState::onEnter`'s `updateGoal` marks the aircraft goal cells; the port's
        // `adjustDestination` reads those marks in `checkDestination`'s aircraft branch.
        g.pathfind_update_goal(h, &goal);
        let adjusted = g.ai_adjust_move_goal(h, goal);
        assert_eq!((adjusted.x, adjusted.y), (205.0, 205.0), "the goal snaps to the centre of its goal cell");
        assert_eq!(adjusted.z, 0.0, "`adjustCoordToCell` takes the cell's height");
        assert_ne!((adjusted.x, adjusted.y), (goal.x, goal.y), "not the raw goal");

        // A ground unit's adjust is unchanged by this: its goal cell is marked by the ground arm.
        let w = g.create_object(&walker(), Coord3D::new(50.0, 300.0, 0.0), 0.0, 0);
        g.pathfind_update_goal(w, &Coord3D::new(200.0, 300.0, 0.0));
        let g2 = g.ai_adjust_move_goal(w, Coord3D::new(200.0, 300.0, 0.0));
        assert_eq!((g2.x, g2.y), (205.0, 305.0), "a ground unit still adjusts to its cell");
    }

    /// `checkDestination`'s aircraft branch (`AIPathfind.cpp:5030-5038`) is the whole test for an aircraft that
    /// adjusts its destination: a cell with no aircraft goal is accepted **whatever its terrain**, another
    /// aircraft's goal on it rejects it, and the unit's own goal accepts it. The ground unit keeps the surface
    /// and obstacle tests. The branch sits after the off-map check and after `checkForAdjust`'s cliff rejection
    /// (`:5320-5323`), so a cliff cell is rejected for aircraft too.
    #[test]
    fn an_aircrafts_move_goal_comes_from_the_aircraft_marks_alone() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let h = g.create_object(&hover_aircraft(), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
        let rival = g.create_object(&hover_aircraft(), Coord3D::new(60.0, 60.0, 20.0), 0.0, 0);
        // A building makes the goal cells `CELL_OBSTACLE`. A ground unit's adjust can never land on them
        // (`check_destination`'s obstacle test); the aircraft's branch `continue`s past it.
        let mut t = ObjectTemplate::new("Warehouse");
        t.kind_of |= kind_of::STRUCTURE | kind_of::IMMOBILE;
        t.geometry.major_radius = 25.0;
        t.geometry.minor_radius = 25.0;
        t.max_health = 1000.0;
        t.initial_health = 1000.0;
        let building = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        assert!(
            g.pathfinder.cell_type_at(&Coord3D::new(200.0, 200.0, 0.0)) == Some(crate::pathfind::CellType::Obstacle),
            "the building's cells are obstacles"
        );
        let goal = Coord3D::new(200.0, 200.0, 20.0);
        // No mark at all: the obstacle cell is accepted for the aircraft.
        let adjusted = g.ai_adjust_move_goal(h, goal);
        assert_eq!((adjusted.x, adjusted.y), (205.0, 205.0), "an obstacle goal cell is fine for an aircraft");
        // The same cell for a ground unit: rejected, so the spiral moves to the next cell.
        let w = g.create_object(&walker(), Coord3D::new(50.0, 300.0, 0.0), 0.0, 0);
        let ground = g.ai_adjust_move_goal(w, Coord3D::new(200.0, 200.0, 0.0));
        assert_ne!((ground.x, ground.y), (205.0, 205.0), "a ground unit cannot stand in the building");
        // Another aircraft's goal on the cell rejects it (`cell->getGoalAircraft() != objID`).
        g.pathfind_update_goal(rival, &goal);
        let rival_adjusted = g.ai_adjust_move_goal(h, goal);
        assert_ne!((rival_adjusted.x, rival_adjusted.y), (205.0, 205.0), "another aircraft's goal is not taken");
        assert_eq!((rival_adjusted.x, rival_adjusted.y), (215.0, 205.0), "the spiral takes the next cell");
        // The unit's own goal accepts it, and `setGoalAircraft` overwrites the rival's mark.
        g.pathfind_update_goal(h, &goal);
        let own = g.ai_adjust_move_goal(h, goal);
        assert_eq!((own.x, own.y), (205.0, 205.0), "its own aircraft goal is accepted");
        let _ = building;

        // A water cell: the aircraft accepts it (the surface test is never reached), the walker does not.
        let mut wet = TerrainLogic::flat(400, 400, 0);
        wet.add_water_area(zr_map::PolygonTrigger {
            name: "lake".into(),
            layer: String::new(),
            id: 3,
            is_water: true,
            is_river: false,
            river_start: 0,
            points: vec![[300, 300, 10], [340, 300, 10], [340, 340, 10], [300, 340, 10]],
        });
        let mut g = GameLogic::new(wet, 1);
        g.add_player(0, 0);
        let h = g.create_object(&hover_aircraft(), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
        let w = g.create_object(&walker(), Coord3D::new(50.0, 80.0, 0.0), 0.0, 0);
        let water_goal = Coord3D::new(320.0, 320.0, 20.0);
        assert_eq!(
            g.pathfinder.cell_type_at(&water_goal),
            Some(crate::pathfind::CellType::Water),
            "the goal cell is in the lake"
        );
        let air = g.ai_adjust_move_goal(h, water_goal);
        assert_eq!((air.x, air.y), (325.0, 325.0), "an aircraft may aim at a water cell");
        let ground = g.ai_adjust_move_goal(w, water_goal);
        assert_ne!((ground.x, ground.y), (325.0, 325.0), "a ground unit cannot");
    }

    /// `AIInternalMoveToState::onEnter` (`AIStates.cpp:1661-1667`) adjusts the destination **first** and only
    /// then calls `updateGoal` for the new goal. The aircraft marks `checkDestination` reads during the adjust
    /// are therefore the *previous* goal's: a rival aircraft's mark on the new goal cell still rejects it, and
    /// the move's own mark goes on the adjusted cell. (`updateGoal` would overwrite the rival's mark,
    /// `setGoalAircraft`, `AIPathfind.cpp:1466-1490`, so the reversed order accepts the centre cell.)
    #[test]
    fn a_move_enters_its_adjust_before_it_marks_the_new_goal() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let h = g.create_object(&hover_aircraft(), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
        let rival = g.create_object(&hover_aircraft(), Coord3D::new(60.0, 60.0, 20.0), 0.0, 0);
        let goal = Coord3D::new(200.0, 200.0, 20.0);
        g.pathfind_update_goal(rival, &goal);
        assert_eq!(g.ai_pathfind_goal(h), None, "the mover has no goal of its own yet");
        assert_eq!(g.ai_internal_move_enter(h, goal, /*adjusts=*/ true), StateReturn::Continue);
        assert_eq!(
            g.ai_pathfind_goal(h).map(|p| (p.x, p.y)),
            Some((215.0, 205.0)),
            "the rival's mark pushed the adjust off the centre cell, so the adjust ran before this move's updateGoal"
        );
        let o = g.find_object(h).expect("the aircraft");
        assert!(o.ai.move_waiting_for_path, "`computePath` set the state's waiting flag (`AIStates.cpp:1604`)");
        assert!(!o.ai.waiting_for_path, "a quick path never leaves the AI-level wait set");
        // The same move's first update consumes the wait, keys `m_pathGoalPosition` and steers along the path.
        assert_eq!(g.ai_internal_move_update(h), StateReturn::Continue);
        let o = g.find_object(h).expect("the aircraft");
        assert!(!o.ai.move_waiting_for_path, "the first update cleared it");
        assert_eq!((o.ai.path_goal_position.x, o.ai.path_goal_position.y), (215.0, 205.0), "`m_pathGoalPosition = m_goalPosition`");
        assert_eq!(o.ai.locomotor_goal_type, crate::ai::LocoGoalType::PositionOnPath, "and the locomotor follows the path");
        let _ = rival;
    }

    /// `AIInternalMoveToState::update`'s waiting block (`AIStates.cpp:1806-1808`): a move that does not adjust
    /// its destination calls `removeGoal(obj)` as soon as the path arrives, so an old goal mark does not
    /// survive it. A quick-path air move runs that block on its first update; the port used to copy
    /// `ai->isWaitingForPath()` (false after a quick path) into the state flag and skip it entirely.
    #[test]
    fn an_unadjusted_air_move_removes_its_goal_on_the_first_update() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let h = g.create_object(&hover_aircraft(), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
        g.pathfind_update_goal(h, &Coord3D::new(100.0, 100.0, 20.0));
        assert!(g.ai_pathfind_goal(h).is_some(), "an old aircraft goal");
        assert_eq!(g.ai_internal_move_enter(h, Coord3D::new(200.0, 200.0, 20.0), /*adjusts=*/ false), StateReturn::Continue);
        assert!(g.ai_pathfind_goal(h).is_some(), "the old goal is still there before the update");
        assert_eq!(g.ai_internal_move_update(h), StateReturn::Continue);
        assert_eq!(g.ai_pathfind_goal(h), None, "`removeGoal` dropped it (`AIStates.cpp:1808`)");
        assert!(g.find_object(h).is_some_and(|o| o.ai.raw_path.is_some()), "the quick path itself stays");
    }

    /// `computeQuickPath`'s aircraft arm (`AIUpdate.cpp:1639-1640`): a non-projectile `KINDOF_AIRCRAFT` takes
    /// `getAircraftPath`, so its quick path is the real two-node aircraft `Path` — start node at the object's
    /// position, destination last — clipped around a `KINDOF_AIRCRAFT_PATH_AROUND` building for a `LOCO_WINGS`
    /// locomotor (`circleClipsTallBuilding`). The old port built `vec![dest]` with no `Path` for every
    /// quick-path unit, so a jet flew at the raw goal and the trace's `pcount` was 0 where C++ prints 2.
    #[test]
    fn an_aircrafts_quick_path_is_clipped_around_a_tall_building() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let h = g.create_object(&hover_aircraft(), Coord3D::new(50.0, 100.0, 20.0), 0.0, 0);
        let mut t = ObjectTemplate::new("TallBuilding");
        t.kind_of |= kind_of::STRUCTURE | kind_of::IMMOBILE;
        t.geometry.major_radius = 10.0;
        t.geometry.minor_radius = 10.0;
        t.geometry.height = 80.0;
        t.aircraft_path_around = true;
        t.max_health = 1000.0;
        t.initial_health = 1000.0;
        g.create_object(&Arc::new(t), Coord3D::new(300.0, 100.0, 0.0), 0.0, 0);
        let goal = Coord3D::new(250.0, 100.0, 20.0);
        // `computeQuickPath`'s `destroyPath` clears the locomotor goal (`setLocomotorGoalNone`,
        // `AIUpdate.cpp:2015-2028`); the caller re-arms (`doPathfind`'s `setLocomotorGoalPositionOnPath`).
        g.objects.get_mut(&h).unwrap().ai.locomotor_goal_type = crate::ai::LocoGoalType::PositionOnPath;
        assert!(g.compute_quick_path(h, &goal));
        let o = g.find_object(h).expect("the aircraft");
        assert_eq!(o.ai.locomotor_goal_type, crate::ai::LocoGoalType::None, "`destroyPath` dropped the goal");
        let raw = o.ai.raw_path.as_ref().expect("`getAircraftPath` keeps a real Path");
        assert_eq!(raw.nodes.len(), 2, "start + destination");
        assert_eq!((raw.nodes[0].pos.x, raw.nodes[0].pos.y, raw.nodes[0].pos.z), (50.0, 100.0, 20.0), "the start node");
        let last = raw.nodes.last().expect("non-empty").pos;
        assert_eq!((last.x, last.y), (300.0, 230.0), "the wings destination is pushed out of the building's circle");
        assert_eq!(o.ai.path, vec![last], "the waypoint is the clipped destination");
        // A building farther than `100 + radius + 2 cells` from the goal changes nothing.
        assert!(g.compute_quick_path(h, &Coord3D::new(120.0, 100.0, 20.0)));
        let o = g.find_object(h).expect("the aircraft");
        assert_eq!(o.ai.raw_path.as_ref().and_then(|p| p.nodes.last().map(|n| (n.pos.x, n.pos.y))), Some((120.0, 100.0)));
    }

    /// `computeQuickPath`'s "already going there" early return (`AIUpdate.cpp:1620-1636`) through `requestPath`'s
    /// quick-path arm: a destination within 0.5 units of the current path's last node keeps the old path *and*
    /// the old `m_pathTimestamp` (the return precedes `destroyPath`/`m_pathTimestamp = frame`). A wings aircraft's
    /// quick path is a real two-node `Path` — the trace's `pcount=2`, `pfirst` = the old position, `plast`/`pnext`
    /// = the goal — and it stays that way.
    #[test]
    fn a_quick_path_to_the_same_goal_keeps_its_path_and_timestamp() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let h = g.create_object(&hover_aircraft(), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
        let goal = Coord3D::new(200.0, 200.0, 20.0);
        g.ai_request_path(h, goal, true);
        let (stamp, last) = {
            let o = g.find_object(h).expect("the aircraft");
            let raw = o.ai.raw_path.as_ref().expect("a real Path");
            assert_eq!(raw.nodes.len(), 2, "`getAircraftPath`'s two nodes");
            assert_eq!((raw.nodes[0].pos.x, raw.nodes[0].pos.y), (50.0, 50.0), "`pfirst` is the position the path was built from");
            assert_eq!(raw.nodes.last().map(|n| n.pos), Some(goal), "`plast` is the goal");
            assert_eq!(raw.nodes[0].next_optimized, Some(1), "`pnext` is the last node");
            assert_eq!(o.ai.path, vec![goal], "one waypoint ahead of the start node");
            (o.ai.path_timestamp, o.ai.raw_path.as_ref().and_then(|p| p.nodes.last().map(|n| n.pos)))
        };
        // Five frames later, a destination 0.4 units away: the old path is kept, the timestamp is not re-stamped.
        g.frame = stamp + 5;
        let near = Coord3D::new(goal.x + 0.4, goal.y, goal.z);
        g.ai_request_path(h, near, true);
        let o = g.find_object(h).expect("the aircraft");
        assert_eq!(o.ai.path_timestamp, stamp, "`computeQuickPath` returned before re-stamping");
        assert_eq!(
            o.ai.raw_path.as_ref().and_then(|p| p.nodes.last().map(|n| n.pos)),
            last,
            "the old path's last node is kept, not the near destination"
        );
        assert!(!o.ai.waiting_for_path, "a quick path never waits for the pathfinder");
    }

    /// `computeAttackPath`'s contact-weapon arm (`AIUpdate.cpp:1865-1878`): a weapon whose range is under one
    /// pathfind cell ("a car bomb") sets `getCurLocomotor()->setNoSlowDownAsApproachingDest(TRUE)` before it
    /// computes the approach, so the unit does not brake on the way in (`Locomotor.cpp:2389-2396`). A
    /// long-range weapon's attack path leaves the flag alone.
    #[test]
    fn a_contact_weapons_attack_path_turns_off_the_slow_down() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let mut t = ObjectTemplate::new("Bomber");
        t.kind_of |= kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 5.0;
        t.physics = Some(crate::physics::PhysicsTemplate { mass: 10.0, ..crate::physics::PhysicsTemplate::default() });
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        let mut w = crate::weapon::WeaponTemplate::new("Bomb");
        w.primary_damage = 1000.0;
        w.attack_range = 1.0; // under `PATHFIND_CELL_SIZE_F * 0.25` below one cell: contact
        assert!(w.is_contact_weapon());
        let mut set = crate::weapon::WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(w));
        t.combat.weapon_set = Some(set);
        let bomber = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let victim = g.create_object(&walker(), Coord3D::new(200.0, 50.0, 0.0), 0.0, 1);
        let target = Coord3D::new(200.0, 50.0, 0.0);
        {
            let o = g.objects.get_mut(&bomber).expect("the bomber");
            o.ai.request_attack_path(&mut g.pathfinder, bomber, victim, target, 0);
        }
        g.do_pathfind(bomber);
        assert!(g.objects[&bomber].loco_rt.no_slow_down_as_approaching_dest, "the contact approach runs at full speed");

        // A long-range weapon: `isContactWeapon()` false, no flag.
        let mut t = ObjectTemplate::new("Gunner");
        t.kind_of |= kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 5.0;
        t.physics = Some(crate::physics::PhysicsTemplate { mass: 10.0, ..crate::physics::PhysicsTemplate::default() });
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        let mut w = crate::weapon::WeaponTemplate::new("Gun");
        w.primary_damage = 1.0;
        w.attack_range = 100.0;
        assert!(!w.is_contact_weapon());
        let mut set = crate::weapon::WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(w));
        t.combat.weapon_set = Some(set);
        let gunner = g.create_object(&Arc::new(t), Coord3D::new(50.0, 80.0, 0.0), 0.0, 0);
        {
            let o = g.objects.get_mut(&gunner).expect("the gunner");
            o.ai.request_attack_path(&mut g.pathfinder, gunner, victim, target, 5);
        }
        g.do_pathfind(gunner);
        assert!(!g.objects[&gunner].loco_rt.no_slow_down_as_approaching_dest, "a gun keeps its slow-down");
    }

    /// `requestPath`'s recent branch (`AIUpdate.cpp:499-514`) runs its blocked-and-stuck reset for **every**
    /// caller, the move's `onEnter` included: `if (m_path && m_isBlockedAndStuck) { setIgnoreCollisionTime(2s);
    /// m_blockedFrames = 0; m_isBlocked = FALSE; m_isBlockedAndStuck = FALSE; }`. The port only did this in
    /// the move update's own pre-check, which has since moved into `requestPath`; without it a unit that
    /// starts a move inside the three-frame window keeps its stuck flags and gets no collision-ignore window.
    #[test]
    fn a_recent_request_clears_a_blocked_and_stuck_unit() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        g.frame = 10;
        g.pathfinder.queue_for_path(u);
        g.do_pathfind(u); // an empty path is still a request that stamps `path_timestamp`... (no goal yet)
        // Give it a live path and a stuck state, then request again inside the three-frame window.
        g.compute_quick_path(u, &Coord3D::new(150.0, 50.0, 0.0));
        {
            let o = g.objects.get_mut(&u).unwrap();
            o.ai.path_timestamp = 10;
            o.ai.is_blocked = true;
            o.ai.is_blocked_and_stuck = true;
            o.ai.blocked_frames = 40;
            o.ai.queue_for_path_frame = 0;
        }
        g.frame = 11;
        g.ai_request_path(u, Coord3D::new(250.0, 50.0, 0.0), true);
        let o = g.find_object(u).expect("the walker");
        assert_eq!(o.ai.queue_for_path_frame, 11 + LOGICFRAMES_PER_SECOND, "the one-second anti-spin wait");
        assert_eq!(o.ai.ignore_collisions_until, 11 + 2 * LOGICFRAMES_PER_SECOND, "two seconds of collision ignore");
        assert_eq!(o.ai.blocked_frames, 0);
        assert!(!o.ai.is_blocked);
        assert!(!o.ai.is_blocked_and_stuck);
    }

    /// `doPathfind`'s tail (`AIUpdate.cpp:463-465`):
    ///
    /// ```text
    /// if (m_queueForPathFrame > TheGameLogic->getFrame()) m_waitingForPath = TRUE;
    /// ```
    ///
    /// A request that is serviced while an *earlier* `requestPath`'s one-second anti-spin wait is
    /// still pending (`setQueueForPathTime`, `:499-514`) leaves the AI waiting for a path. The move
    /// state then keeps its waiting early return (`AIStates.cpp:1786-1793`, which stamps the state's
    /// own `m_pathTimestamp` every frame) until the deferred request comes due (`:1064-1078`). AH7 s1
    /// f378 is that shape: dozer 231's move re-enters 29 frames after its path was computed (a new
    /// path request passes the three-frame guard while the wait armed on frame 348 still stands), the
    /// engine rebuilds the path from the unit's current position and drops the already-passed first
    /// node; without the tail the port cleared the wait and walked the stale path.
    #[test]
    fn a_serviced_request_with_a_pending_anti_spin_wait_leaves_the_move_waiting() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        // A path computed on frame 10 makes a request made on frame 11 "recent".
        g.frame = 10;
        assert_eq!(g.ai_internal_move_enter(u, Coord3D::new(250.0, 50.0, 0.0), true), StateReturn::Continue);
        g.do_pathfind(u);
        assert_eq!(g.find_object(u).expect("the walker").ai.path_timestamp, 10);
        // Frame 11: a request inside the three-frame window only arms the one-second wait.
        g.frame = 11;
        g.ai_request_path(u, Coord3D::new(250.0, 150.0, 0.0), true);
        assert_eq!(
            g.find_object(u).expect("the walker").ai.queue_for_path_frame,
            11 + LOGICFRAMES_PER_SECOND,
            "the anti-spin wait"
        );
        // Frame 12: still inside the window, the wait merely re-arms.
        g.frame = 12;
        g.ai_request_path(u, Coord3D::new(250.0, 150.0, 0.0), true);
        assert_eq!(
            g.find_object(u).expect("the walker").ai.queue_for_path_frame,
            12 + LOGICFRAMES_PER_SECOND,
            "still inside the three-frame window"
        );
        // Frame 13: three frames after the computed path the next request passes the guard and is
        // serviced, while the wait armed on frame 12 is still pending.
        g.frame = 13;
        g.ai_request_path(u, Coord3D::new(250.0, 150.0, 0.0), true);
        g.do_pathfind(u);
        let o = g.find_object(u).expect("the walker");
        assert_eq!(o.ai.queue_for_path_frame, 12 + LOGICFRAMES_PER_SECOND, "the deferred request is still due");
        assert!(
            o.ai.waiting_for_path,
            "`doPathfind` leaves the AI waiting while the deferred request is pending (`AIUpdate.cpp:463-465`)"
        );
        // The move state keeps its waiting return, so it neither re-records its goal nor repaths.
        g.frame = 14;
        assert_eq!(g.ai_internal_move_update(u), StateReturn::Continue);
        assert!(
            g.find_object(u).expect("the walker").ai.move_waiting_for_path,
            "the move is still waiting for the deferred request"
        );
    }

    /// An air move's forced repath inside the three-frame window is rebuilt on that frame. `requestPath` checks
    /// `canComputeQuickPath()` *before* the anti-spin guard (`AIUpdate.cpp:493-499`), so the guard never delays
    /// a quick path. The move update used to duplicate the guard before calling `requestPath`, which left the
    /// path and `m_pathTimestamp` untouched for two or three frames.
    #[test]
    fn an_air_repath_inside_the_recent_window_is_not_delayed() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let h = g.create_object(&hover_aircraft(), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
        g.frame = 10;
        assert_eq!(g.ai_internal_move_enter(h, Coord3D::new(200.0, 200.0, 20.0), true), StateReturn::Continue);
        assert_eq!(g.ai_internal_move_update(h), StateReturn::Continue, "the first update consumes the wait block");
        let old_goal = g.find_object(h).expect("the aircraft").ai.path.last().copied();
        assert!(old_goal.is_some());
        // Blocked and stuck, with a moved goal, inside the three-frame anti-spin window.
        {
            let o = g.objects.get_mut(&h).unwrap();
            o.ai.move_goal = Some(Coord3D::new(100.0, 200.0, 20.0));
            o.ai.is_blocked_and_stuck = true;
            o.ai.blocked_frames = 61;
        }
        g.frame = 11;
        assert_eq!(g.find_object(h).expect("the aircraft").ai.path_timestamp, 10, "the path is one frame old");
        assert_eq!(g.ai_internal_move_update(h), StateReturn::Continue);
        let o = g.find_object(h).expect("the aircraft");
        assert_eq!(o.ai.path.last(), Some(&Coord3D::new(100.0, 200.0, 20.0)), "the quick path is rebuilt on this frame");
        assert_eq!(o.ai.path_timestamp, 11, "and re-stamped");
    }

    /// `AIGroup::computeIndividualDestination` applies the group offset to the *destination* once, at order
    /// time (`AIGroup.cpp:542-549`); `requestPath` has no group destination. The port keeps the pending copy on
    /// the AI until the request that consumes it; `doPathfind` clears it for a queued path, so a quick path must
    /// spend it too or it leaks into the unit's next queued path.
    #[test]
    fn a_quick_path_spends_the_group_destination() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let h = g.create_object(&hover_aircraft(), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
        let group = Coord3D::new(300.0, 300.0, 0.0);
        {
            let o = g.objects.get_mut(&h).unwrap();
            o.ai.machine.group_dest = Some(group);
        }
        g.ai_request_path(h, Coord3D::new(200.0, 200.0, 20.0), true);
        let o = g.find_object(h).expect("the aircraft");
        assert_eq!(o.ai.group_dest, None, "the request spent the group destination");
        assert_eq!(o.ai.machine.group_dest, None, "and the machine's pending copy");
    }

    /// `AIUpdateInterface::isAircraftThatAdjustsDestination()` (`AIUpdate.cpp:2408-2429`) reads the **current**
    /// locomotor's appearance, not `SET_NORMAL`: a taxiing jet drives `SET_TAXIING` (wheels) and is not an
    /// aircraft that adjusts. `updateAircraftGoal` then removes its old goal and marks no cells
    /// (`AIPathfind.cpp:9905-9951`), so it does not reserve a rival-aircraft goal. `PathObject::for_template`
    /// (SET_NORMAL = wings) said true and marked the cells for a taxiing jet.
    #[test]
    fn a_taxiing_jet_does_not_mark_aircraft_goals() {
        let f = crate::math::LOGICFRAMES_PER_SECOND as f32;
        let wings = LocomotorParams::from_engine_units(LocomotorKind::Wings, 175.0 / f, 60.0 / f, 120.0 / (f * f), 10.0 / (f * f), 2.0, false);
        let taxi = LocomotorParams::from_engine_units(LocomotorKind::Treads, 50.0 / f, 0.0, 100.0 / (f * f), 999_999.0 / (f * f), 3.0, true);
        let mut t = ObjectTemplate::new("TaxiingJet");
        t.kind_of = kind_of::AIRCRAFT | kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 14.0;
        t.max_health = 160.0;
        t.initial_health = 160.0;
        t.locomotor = Some(wings);
        t.locomotor_sets.insert(crate::template::LocomotorSet::Normal, wings);
        t.locomotor_sets.insert(crate::template::LocomotorSet::Taxiing, taxi);
        t.locomotor_entries.insert(crate::template::LocomotorSet::Normal, 1);
        t.locomotor_entries.insert(crate::template::LocomotorSet::Taxiing, 1);
        t.jet = Some(crate::modules2::JetAiTemplate {
            needs_runway: false,
            keeps_parking_space_when_airborne: false,
            takeoff_pause: 0,
            return_to_base_idle_time: 0,
            out_of_ammo_damage_per_second: 0.0,
            takeoff_dist_for_max_lift: 0.0,
            min_height: 0.0,
            parking_offset: 0.0,
            sneaky_offset_when_attacking: 0.0,
            attacking_loco: crate::template::LocomotorSet::Normal,
            returning_loco: crate::template::LocomotorSet::Normal,
            attack_loco_persist_time: 0,
            attackers_miss_persist_time: 0,
        });
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let jet = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
        assert_eq!(g.objects[&jet].locomotor_set, crate::template::LocomotorSet::Taxiing, "the jet parks on taxi");
        assert!(!g.is_doing_ground_movement(jet), "a `JetAIUpdate` is never doing ground movement");
        let po = g.path_object(jet).expect("the jet");
        assert!(!po.aircraft_that_adjusts_destination, "`SET_TAXIING` is wheels, not wings");
        assert!(po.is_aircraft);
        g.pathfind_update_goal(jet, &Coord3D::new(200.0, 200.0, 20.0));
        assert_eq!(g.ai_pathfind_goal(jet), None, "`updateAircraftGoal` removes the old goal and marks nothing");
    }

    /// `privateMoveToPosition` (`AIUpdate.cpp:2896`): an idle unit gets a normal `AI_MOVE_TO`, and a
    /// `CMD_FROM_AI` move to a unit that is already moving becomes the 20-second temporary move
    /// (`AIUpdate.cpp:2909-2913`) while the main state keeps its own move.
    #[test]
    fn ai_move_to_position_is_temporary_only_when_not_idle() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        let far = Coord3D::new(350.0, 350.0, 0.0);
        let near = Coord3D::new(60.0, 60.0, 0.0);

        g.ai_private_move_to_position(u, near, CommandSource::Ai);
        let m = machine(&g, u);
        assert_eq!(m.current, AiStateType::MoveTo, "an idle unit enters AI_MOVE_TO");
        assert_eq!(m.temporary, None, "the idle branch is not temporary");

        let t0 = g.frame();
        g.ai_private_move_to_position(u, far, CommandSource::Ai);
        let m = machine(&g, u);
        assert_eq!(m.current, AiStateType::MoveTo, "the main state is untouched");
        assert_eq!(m.temporary, Some(AiStateType::MoveTo), "a CMD_FROM_AI move to a busy unit is temporary");
        assert_eq!(m.temporary_end, t0 + 20 * LOGICFRAMES_PER_SECOND, "20 seconds");
        assert_eq!(m.goal_position, far);
    }

    /// `AIStateMachine::updateStateMachine` (`AIStates.cpp:860-884`): a temporary state that runs out
    /// of frames succeeds and the main state runs again in the same frame.
    #[test]
    fn temporary_state_times_out_and_the_main_state_resumes() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        // A normal move makes `AI_MOVE_TO` the main state.
        g.ai_private_move_to_position(u, Coord3D::new(350.0, 350.0, 0.0), CommandSource::Player);
        assert_eq!(machine(&g, u).current, AiStateType::MoveTo);

        // `AI_WAIT` never finishes on its own (it returns CONTINUE), so only the frame limit ends it.
        g.ai_set_temporary_state(u, AiStateType::Wait, 90);
        g.run_frames(60);
        assert_eq!(machine(&g, u).temporary, Some(AiStateType::Wait), "still inside its limit");
        assert_eq!(machine(&g, u).current, AiStateType::MoveTo, "the main state waits");

        g.run_frames(40);
        let m = machine(&g, u);
        assert_eq!(m.temporary, None, "the temporary state ended at its frame limit");
        assert_eq!(m.current, AiStateType::MoveTo, "the main state is back in control");
        assert!(g.find_object(u).is_some_and(|o| o.ai.is_moving()), "the main move continues");
    }

    /// `AIStateMachine::setState` notifies the AI (`friend_notifyStateMachineChanged`, which wakes it) only when
    /// the state changes (`oldID != newStateID`, AIStates.cpp:1051-1058). A same-state re-entry keeps the sleep;
    /// a real change clears it. Revert check: clearing `sleep_till` on every entry fails the first assert.
    #[test]
    fn a_same_state_reentry_keeps_the_sleep() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        let o = g.objects.get_mut(&u).unwrap();
        o.ai.suspended = true;
        o.ai.machine.current = AiStateType::Idle;
        o.ai.machine.sleep_till = 500;
        g.ai_set_state(u, AiStateType::Idle);
        assert_eq!(g.find_object(u).unwrap().ai.machine.sleep_till, 500, "a same-state re-entry does not wake the machine");
        g.ai_set_state(u, AiStateType::Exit);
        assert_eq!(g.find_object(u).unwrap().ai.machine.sleep_till, 0, "a state change wakes the machine");
    }

    /// `AIUpdateInterface::update` returns the least of the state machine's sleep and `doLocomotor`'s sleep
    /// (AIUpdate.cpp:1013-1028, :2305-2314). An idle treads unit at rest (no goal, no final position, not blocked;
    /// treads need no constant calls) reports its idle countdown, so its heap key is the machine's `sleep_till`.
    /// A unit with a move goal reports `UPDATE_SLEEP_NONE`. Revert check: `ai_update_sleep` returning
    /// `UPDATE_SLEEP_NONE` always fails the first assert (the key would be the next frame).
    #[test]
    fn an_idle_unit_at_rest_sleeps_until_its_machine_wakes() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        g.update();
        g.update();
        let h = g.objects[&u].ai_handle.expect("a walker has an AI module");
        let till = g.objects[&u].ai.machine.sleep_till;
        assert!(till > g.frame(), "the idle countdown is a sleep");
        assert_eq!(g.schedule.next_call_frame(h), till, "the heap key is the machine's wake frame");
        g.submit(0, crate::command::Command::MoveTo { ids: vec![u], pos: Coord3D::new(200.0, 50.0, 0.0) });
        g.update();
        let h = g.objects[&u].ai_handle.expect("still an AI module");
        assert_eq!(g.schedule.next_call_frame(h), g.frame(), "a unit with a move goal runs on the next frame");
    }

    /// `AIIdleState::update` skips mood targeting while the unit is paralyzed, unmanned, EMP-disabled, subdued or
    /// hacked (AIStates.cpp:1432-1437): no scan and no timer work, and the sleep still runs. Revert check: drop the
    /// disabled gate and the pending offset is consumed by the scan.
    #[test]
    fn a_paralyzed_idle_unit_does_not_scan_or_touch_the_timer() {
        use crate::disabled::DisabledType;
        use crate::template::kind_of;
        use crate::weapon::{WeaponSetTemplate, WeaponTemplate};
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let mut t = ObjectTemplate::new("Gun");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
        t.geometry.major_radius = 5.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.combat.auto_acquire_enemies = true;
        let mut w = WeaponTemplate::new("Hit");
        w.primary_damage = 1.0;
        w.attack_range = 100.0;
        let mut set = WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(w));
        t.combat.weapon_set = Some(set);
        let t = Arc::new(t);
        let unit = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let enemy = g.create_object(&t, Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        g.reveal_map_permanently_for_player(0);
        for id in [unit, enemy] {
            g.objects.get_mut(&id).unwrap().ai.suspended = true;
        }
        g.reset_next_mood_check_time(unit);
        g.run_frames(1);
        g.set_disabled(unit, DisabledType::Paralyzed);
        let ret = g.ai_idle_update(unit);
        assert!(matches!(ret, StateReturn::Sleep(_)), "no attack: the idle update sleeps");
        assert!(g.find_object(unit).unwrap().combat.randomly_offset_mood_check, "a paralyzed unit's timer is not touched");
        assert_eq!(g.find_object(unit).unwrap().ai.machine.current, AiStateType::Idle);
    }

    /// An attacker with a 100-unit gun and 300 vision (mobile, so it auto-acquires out to vision).
    fn attacker() -> Arc<ObjectTemplate> {
        use crate::weapon::{WeaponSetTemplate, WeaponTemplate};
        let mut t = ObjectTemplate::new("Attacker");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 5.0;
        t.vision_range = 300.0;
        t.combat.auto_acquire_enemies = true;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        let mut w = WeaponTemplate::new("Gun");
        w.primary_damage = 1.0;
        w.attack_range = 100.0;
        let mut set = WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(w));
        t.combat.weapon_set = Some(set);
        Arc::new(t)
    }

    fn attack_move_world() -> (GameLogic, ObjectId, ObjectId, ObjectId) {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.set_enemies(0, 1);
        g.reveal_map_permanently_for_player(0);
        let t = attacker();
        let unit = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let first = g.create_object(&t, Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
        let second = g.create_object(&t, Coord3D::new(100.0, 150.0, 0.0), 0.0, 1);
        (g, unit, first, second)
    }

    /// `AIAttackMoveToState::update` (`AIStates.cpp:3621-3661`): the frame its attack sub-machine becomes idle
    /// sets `forceRetargetThisFrame`, so the scan is `getNextMoodTarget(false, false)` — the mood timer does not
    /// gate it — and `shouldRepathThisFrame` is cleared, so a target found there is attacked instead of repathing.
    /// The sim runs an attack as a main state, so the enclosing state is re-entered and scans in its `onEnter`.
    /// Revert check: remove the force scan and the unit stays in `AI_ATTACK_MOVE_TO` with the timer far away.
    #[test]
    fn an_attack_move_rescans_without_the_mood_timer_after_its_attack() {
        let (mut g, unit, first, second) = attack_move_world();
        g.command_attack_move(0, &[unit], Coord3D::new(300.0, 100.0, 0.0), CommandSource::Player);
        let mut started = false;
        for _ in 0..120 {
            g.update();
            if g.find_object(unit).is_some_and(|o| o.ai.machine.current == AiStateType::AttackObject) {
                started = true;
                break;
            }
        }
        assert!(started, "the attack-move acquired the first enemy");
        // Park the mood timer far in the future: only the force scan can find the second enemy.
        {
            let o = g.objects.get_mut(&unit).unwrap();
            o.combat.next_mood_check = 9999;
            o.combat.randomly_offset_mood_check = false;
        }
        g.destroy_object(first);
        let frame_before = g.frame();
        g.update();
        let o = g.find_object(unit).unwrap();
        assert_eq!(o.ai.machine.current, AiStateType::AttackObject, "the post-attack scan ignores the mood timer");
        assert_eq!(o.ai.machine.goal_object, second, "it acquired the second enemy");
        // The attack's end enters the attack-move's sub-machine `AI_IDLE` before the scan, and that entry resets the
        // mood check (`AIIdleState::onEnter`, AIStates.cpp:1321 -> `resetNextMoodCheckTime`, AIUpdate.cpp:4498). The
        // scan itself still does not schedule or offset the timer, so the reset is the only change.
        assert!(o.combat.randomly_offset_mood_check, "the sub-machine's idle entry re-arms the mood check");
        assert_eq!(o.combat.next_mood_check, frame_before + g.ai_tunables.force_idle_frames, "reset to ForceIdleMSEC, not the parked 9999");
    }

    /// `AIAttackMoveToState::update` scans before the retry sleep (`AIStates.cpp:3637-3668`): "We can attack during
    /// these frames, just not move." Revert check: put the sleep gate back before the scan and the unit stays idle.
    #[test]
    fn an_attack_move_scans_while_it_waits_to_retry() {
        let (mut g, unit, _, _) = attack_move_world();
        // The AI module is registered with wake frame 1 (`registerObject`, `max(now, 1)`), so it first runs
        // on frame 1 in the sleepy heap: step past frame 0 before the order.
        g.update();
        g.command_attack_move(0, &[unit], Coord3D::new(300.0, 100.0, 0.0), CommandSource::Player);
        {
            let now = g.frame();
            let o = g.objects.get_mut(&unit).unwrap();
            o.ai.machine.attack_move_sleep_until = now + 100;
            o.combat.next_mood_check = 0;
        }
        g.update();
        let o = g.find_object(unit).unwrap();
        assert_eq!(o.ai.machine.current, AiStateType::AttackObject, "a sleeping attack-move still scans");
    }

    /// The AI modules run in sleepy-heap order (`GameLogic::update`, GameLogic.cpp:3832-3890), not in id order:
    /// the three attackers are due this frame with keys 5, 3 and 4 (`friend_setNextCallFrame`), so they run
    /// first, second, unit. Revert check: run the AI as the id walk again (`update()` calling only
    /// `update_ai_state_machines`) and the log stays empty.
    #[test]
    fn the_ai_modules_run_in_sleepy_heap_order() {
        let (mut g, unit, first, second) = attack_move_world();
        for _ in 0..6 {
            g.update();
        }
        for (id, when) in [(unit, 5), (first, 3), (second, 4)] {
            let h = g.objects[&id].ai_handle.expect("an attacker has an AI module");
            g.schedule.awaken(h, 6, when);
        }
        g.ai_pass_log.clear();
        g.update();
        assert_eq!(g.ai_pass_log, vec![first, second, unit], "the heap's order, not the id order");
    }

    /// The RNG state after `n` sleep-offset draws (`AIIdleState::onEnter`, `GameLogicRandomValue(0,
    /// IDLE_COUNTDOWN_DELAY)`, AIStates.cpp:1327) from `before`: the expected stream for exact-count checks.
    fn after_idle_draws(before: &crate::random::GameRandom, n: usize) -> [u32; 6] {
        let mut rng = before.clone();
        for _ in 0..n {
            rng.value(0, IDLE_COUNTDOWN_DELAY as i32);
        }
        *rng.state()
    }

    /// `AIAttackMoveToState::onEnter` (`AIStates.cpp:3584-3589`): a fresh entry does `m_attackMoveMachine->clear()`
    /// and `setState(AI_IDLE)`, and that entry draws the sleep offset once. Revert check: drop the fresh-entry
    /// `ai_attack_sub_idle_enter` and the stream is one draw short.
    #[test]
    fn a_fresh_attack_move_enters_its_sub_machine_idle_once() {
        let (mut g, unit, _, _) = attack_move_world();
        let before = g.rng.clone();
        g.ai_attack_move_to_position(unit, Coord3D::new(300.0, 100.0, 0.0));
        assert_eq!(g.find_object(unit).unwrap().ai.machine.current, AiStateType::AttackMoveTo);
        assert_eq!(*g.rng.state(), after_idle_draws(&before, 1), "one AI_IDLE entry of the sub-machine");
    }

    /// `AIAttackMoveToState::onExit` (`AIStates.cpp:3598-3601`): `m_attackMoveMachine->setState(AI_IDLE)` enters the
    /// sub-machine's idle (a draw) before the owner leaves the attack-move. Revert check: drop the exit's sub-idle
    /// entry and the stream is one draw short.
    #[test]
    fn leaving_an_attack_move_enters_its_sub_machine_idle() {
        let (mut g, unit, _, _) = attack_move_world();
        g.ai_attack_move_to_position(unit, Coord3D::new(300.0, 100.0, 0.0));
        let before = g.rng.clone();
        g.ai_clear(unit);
        assert_eq!(g.find_object(unit).unwrap().ai.machine.current, AiStateType::Idle);
        assert_eq!(*g.rng.state(), after_idle_draws(&before, 1), "the exit enters the sub-machine's AI_IDLE");
    }

    /// The attack-move's own attack (`AIAttackMoveToState::update`, `AIStates.cpp:3655`): the sub-machine goes from
    /// `AI_IDLE` to an attack, which is not an exit of the attack-move, so no `AI_IDLE` entry draws. Revert check:
    /// drop the hand-off flag and the attack's start exits the attack-move with a draw.
    #[test]
    fn an_attack_move_hands_over_to_its_attack_without_an_idle_entry() {
        let (mut g, unit, first, _) = attack_move_world();
        g.ai_attack_move_to_position(unit, Coord3D::new(300.0, 100.0, 0.0));
        let before = g.rng.clone();
        g.ai_start_attack(unit, AttackTarget::Object(first), false, false, None, AiStateType::AttackMoveTo);
        assert_eq!(g.find_object(unit).unwrap().ai.machine.current, AiStateType::AttackObject);
        assert_eq!(*g.rng.state(), *before.state(), "the hand-off draws nothing");
    }

    /// `AIAttackMoveToState::update` sets `AI_ATTACK_OBJECT` (AIStates.cpp:3653-3655), and `AIAttackState::onEnter` runs the
    /// attack's entry chain at once (AIStates.cpp:5586, `initDefaultState`): an out-of-range victim's approach request is
    /// queued in the hand-off frame (`requestAttackPath`, AIUpdate.cpp:489-505), not on the next update (AN3 s1 Ranger 254,
    /// f4036). Driven at call level (`ai_start_attack`), because the synthetic world's pickup frame does not reproduce the
    /// approach request (an open item). Revert check: drop the `ai_attack_enter_pass` call in `ai_attack_enter`; the victim
    /// is not yet the requested victim after the hand-off.
    #[test]
    fn an_attack_hand_off_queues_an_out_of_range_victims_approach_in_its_frame() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
        g.set_enemies(0, 1);
        g.reveal_map_permanently_for_player(0);
        let t = attacker();
        let unit = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let far = g.create_object(&t, Coord3D::new(350.0, 100.0, 0.0), 0.0, 1);
        g.ai_attack_move_to_position(unit, Coord3D::new(300.0, 100.0, 0.0));
        g.ai_start_attack(unit, AttackTarget::Object(far), false, false, None, AiStateType::AttackMoveTo);
        assert_eq!(g.find_object(unit).unwrap().ai.requested_victim, far, "the approach to the victim is requested in the hand-off frame");
    }

    /// The attack-move hand-off makes no stop (AIStates.cpp:3653-3655 call only `friend_endingMove` and the state). The move's
    /// path survives the call. Driven at call level, as above. Revert check: restore the unconditional `o.ai.stop()` in
    /// `ai_start_attack_sourced`; the path is then empty after the hand-off.
    #[test]
    fn an_attack_hand_off_keeps_the_attack_moves_path() {
        let (mut g, unit, first, _) = attack_move_world();
        g.ai_attack_move_to_position(unit, Coord3D::new(300.0, 100.0, 0.0));
        for _ in 0..30 {
            if !g.find_object(unit).unwrap().ai.path.is_empty() {
                break;
            }
            g.update();
        }
        assert!(!g.find_object(unit).unwrap().ai.path.is_empty(), "the attack move has a path to keep");
        g.ai_start_attack(unit, AttackTarget::Object(first), false, false, None, AiStateType::AttackMoveTo);
        assert!(!g.find_object(unit).unwrap().ai.path.is_empty(), "the hand-off keeps the path");
    }

    /// A player's attack command on a moving unit (`aiDoCommand` -> `privateAttackObject`, AIUpdate.cpp:3426-3446) runs
    /// `clear()`, whose `onExit(EXIT_RESET)` is the move's `friend_endingMove` (AIStates.cpp:1736-1750), then sets the attack.
    /// Nothing stops the unit at the command: the path and the locomotor goal are dropped by the completed-move block of the
    /// next update (AIUpdate.cpp:1035-1061). Revert check: restore `o.ai.stop()` for the command; the path is empty right
    /// after the command.
    #[test]
    fn a_command_attack_on_a_moving_unit_drops_its_path_in_the_next_update() {
        let (mut g, unit, first, _) = attack_move_world();
        g.ai_move_to_position_cmd(unit, Coord3D::new(300.0, 100.0, 0.0), CommandSource::Player);
        for _ in 0..30 {
            if !g.find_object(unit).unwrap().ai.path.is_empty() {
                break;
            }
            g.update();
        }
        assert!(!g.find_object(unit).unwrap().ai.path.is_empty(), "the move has a path");
        g.ai_attack_object_from(unit, first, false, false, None, CommandSource::Player);
        assert!(!g.find_object(unit).unwrap().ai.path.is_empty(), "the command itself does not stop the unit");
        g.update();
        let o = g.find_object(unit).unwrap();
        assert!(o.ai.path.is_empty(), "the completed-move block drops the path in the next update");
        assert_eq!(o.ai.locomotor_goal_type, crate::ai::LocoGoalType::None, "and the locomotor goal");
    }

    /// A hand-off to a victim that is already dead fails the attack's entry (`AIAttackState::onEnter`, AIStates.cpp:5556-5557:
    /// the null or effectively dead victim returns STATE_FAILURE before `initDefaultState`), so the entry chain does not
    /// run: no approach request and no attack are left behind.
    #[test]
    fn a_hand_off_to_a_dead_victim_queues_no_approach() {
        let (mut g, unit, first, _) = attack_move_world();
        g.ai_attack_move_to_position(unit, Coord3D::new(300.0, 100.0, 0.0));
        g.destroy_object(first);
        g.ai_start_attack(unit, AttackTarget::Object(first), false, false, None, AiStateType::AttackMoveTo);
        let o = g.find_object(unit).unwrap();
        assert_ne!(o.ai.requested_victim, first, "no approach to a dead victim");
        assert!(o.combat.attack.is_none(), "the dead victim fails the attack on entry");
    }

    /// `AIUpdateInterface::privateIdle`'s passenger loop (`AIUpdate.cpp:3108-3116`): idling a container idles the AI of
    /// every object it carries. Revert check: drop the loop and the passenger stays in `AI_ATTACK_MOVE_TO`.
    #[test]
    fn idling_a_container_idles_its_passengers() {
        let (mut g, passenger, _, _) = attack_move_world();
        let container = g.create_object(&attacker(), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        g.objects.get_mut(&container).unwrap().contained.push(passenger);
        g.objects.get_mut(&passenger).unwrap().contained_by = Some(container);
        g.ai_attack_move_to_position(passenger, Coord3D::new(300.0, 100.0, 0.0));
        assert_eq!(g.find_object(passenger).unwrap().ai.machine.current, AiStateType::AttackMoveTo);
        g.ai_idle(container, CommandSource::Ai);
        assert_eq!(g.find_object(passenger).unwrap().ai.machine.current, AiStateType::Idle, "the passenger's AI goes idle");
    }

    /// An attack the attack-move's sub-machine runs ends (`AIAttackState::onExit`, `AIStates.cpp:5714`), and the
    /// sub-machine enters `AI_IDLE` (its success transition, `AIStates.cpp:1999`). The attack-move then resumes
    /// without another entry: one draw in all. Revert check: drop the attack-exit entry and the stream is one draw
    /// short.
    #[test]
    fn an_attack_ending_enters_its_attack_moves_sub_machine_idle_once() {
        let (mut g, unit, first, second) = attack_move_world();
        g.ai_attack_move_to_position(unit, Coord3D::new(300.0, 100.0, 0.0));
        g.ai_start_attack(unit, AttackTarget::Object(first), false, false, None, AiStateType::AttackMoveTo);
        // The scan path sets the resume marker (`ai_attack_move_update`); emulate it for the direct attack start.
        g.objects.get_mut(&unit).unwrap().ai.machine.attack_move_resume = true;
        g.destroy_object(first);
        g.destroy_object(second);
        let before = g.rng.clone();
        g.ai_machine_update(unit);
        assert_eq!(g.find_object(unit).unwrap().ai.machine.current, AiStateType::AttackMoveTo, "the attack-move resumes");
        assert_eq!(*g.rng.state(), after_idle_draws(&before, 1), "the attack's end enters the sub-machine's AI_IDLE once");
    }

    /// `AIAttackFollowWaypointPathState::onEnter` (`AIStates.cpp:4445-4450`): `m_attackFollowMachine->clear()`
    /// and `setState(AI_IDLE)` is one `AI_IDLE` entry of the sub-machine, which draws its sleep offset.
    /// Revert check: drop the fresh-entry `ai_attack_sub_idle_enter` and the stream is one draw short.
    #[test]
    fn a_fresh_attack_follow_enters_its_sub_machine_idle_once() {
        let (mut g, unit, _, _) = attack_move_world();
        g.ai_set_follow_path(unit, vec![Coord3D::new(300.0, 100.0, 0.0), Coord3D::new(380.0, 100.0, 0.0)], AiStateType::FollowWaypointPath, None, None);
        let before = g.rng.clone();
        g.ai_set_state(unit, AiStateType::AttackFollowWaypointPath);
        assert_eq!(g.find_object(unit).unwrap().ai.machine.current, AiStateType::AttackFollowWaypointPath);
        assert_eq!(*g.rng.state(), after_idle_draws(&before, 1), "one AI_IDLE entry of the attack-follow sub-machine");
    }

    /// `AIAttackFollowWaypointPathState::onExit` (`AIStates.cpp:4518-4521`): the sub-machine is put into
    /// `AI_IDLE` on the way out, one more entry/draw. Revert check: send the state back to the generic
    /// move-exit arm and the stream is one draw short.
    #[test]
    fn leaving_an_attack_follow_enters_its_sub_machine_idle() {
        let (mut g, unit, _, _) = attack_move_world();
        g.ai_set_follow_path(unit, vec![Coord3D::new(300.0, 100.0, 0.0), Coord3D::new(380.0, 100.0, 0.0)], AiStateType::FollowWaypointPath, None, None);
        g.ai_set_state(unit, AiStateType::AttackFollowWaypointPath);
        let before = g.rng.clone();
        g.ai_clear(unit);
        assert_eq!(g.find_object(unit).unwrap().ai.machine.current, AiStateType::Idle);
        assert_eq!(*g.rng.state(), after_idle_draws(&before, 1), "the exit enters the sub-machine's AI_IDLE");
    }

    /// `AIAttackFollowWaypointPathState::update` (`AIStates.cpp:4462-4502`): the frame its attack
    /// sub-machine becomes idle sets `forceRetargetThisFrame`, so the scan is `getNextMoodTarget(false, false)`
    /// (no mood timer, no team victim) and a target found there is attacked instead of the path repathing.
    /// The sim re-enters the enclosing state when the attack ends, so that re-entry is the frame.
    /// Revert check: drop the re-entry's force scan and the state goes back to following the path
    /// with the parked mood timer untouched.
    #[test]
    fn an_attack_follow_rescans_without_the_mood_timer_after_its_attack() {
        let (mut g, unit, first, second) = attack_move_world();
        g.ai_set_follow_path(unit, vec![Coord3D::new(300.0, 100.0, 0.0), Coord3D::new(380.0, 100.0, 0.0)], AiStateType::FollowWaypointPath, None, None);
        g.ai_set_state(unit, AiStateType::AttackFollowWaypointPath);
        g.ai_start_attack(unit, AttackTarget::Object(first), false, false, None, AiStateType::AttackFollowWaypointPath);
        // The follow state's own pickup sets the marker (`ai_attack_follow_update`); emulate it here.
        g.objects.get_mut(&unit).unwrap().ai.machine.attack_follow_resume = true;
        // Park the mood timer far in the future: only the force scan can find the second enemy.
        {
            let o = g.objects.get_mut(&unit).unwrap();
            o.combat.next_mood_check = 9999;
            o.combat.randomly_offset_mood_check = false;
        }
        g.destroy_object(first);
        let frame_before = g.frame();
        g.ai_machine_update(unit);
        let o = g.find_object(unit).unwrap();
        assert_eq!(o.ai.machine.current, AiStateType::AttackObject, "the post-attack scan ignores the mood timer");
        assert_eq!(o.ai.machine.goal_object, second, "it acquired the second enemy");
        // The attack's end enters the sub-machine's `AI_IDLE` before the scan, resetting the mood check
        // (`AIIdleState::onEnter`, AIStates.cpp:1321 -> `resetNextMoodCheckTime`, AIUpdate.cpp:4498); the
        // force scan itself leaves the timer alone.
        assert!(o.combat.randomly_offset_mood_check, "the sub-machine's idle entry re-arms the mood check");
        assert_eq!(o.combat.next_mood_check, frame_before + g.ai_tunables.force_idle_frames, "reset to ForceIdleMSEC, not the parked 9999");
    }

    /// `AIAttackMoveToState::onEnter` saves `m_commandSrc` (`AIStates.cpp:3590`) — the source the order
    /// wrote (`privateAttackMoveToPosition` -> `setLastCommandSource`, AIUpdate.cpp:3608). Revert check:
    /// drop the save and the field keeps `CMD_FROM_AI`.
    #[test]
    fn a_fresh_attack_move_saves_its_command_source() {
        let (mut g, unit) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        g.objects.get_mut(&unit).unwrap().ai.machine.last_command_source = CommandSource::Player;
        g.ai_attack_move_enter(unit);
        assert_eq!(machine(&g, unit).attack_move_command_source, CommandSource::Player);
    }

    /// `AIAttackMoveToState::update` (`AIStates.cpp:3658`): a target the attack-move picks up from the
    /// mood is a command from the AI, so `CMD_FROM_AI` replaces the player's source. Revert check: pass
    /// no source and the player's `CMD_FROM_PLAYER` stays.
    #[test]
    fn an_attack_move_interception_sets_the_ai_command_source() {
        let (mut g, unit, _, _) = attack_move_world();
        g.command_attack_move(0, &[unit], Coord3D::new(300.0, 100.0, 0.0), CommandSource::Player);
        assert_eq!(machine(&g, unit).last_command_source, CommandSource::Player);
        let mut started = false;
        for _ in 0..120 {
            g.update();
            if g.find_object(unit).is_some_and(|o| o.ai.machine.current == AiStateType::AttackObject) {
                started = true;
                break;
            }
        }
        assert!(started, "the attack-move acquired an enemy");
        assert_eq!(machine(&g, unit).last_command_source, CommandSource::Ai, "the pickup is CMD_FROM_AI");
    }

    /// `AIAttackMoveToState::onEnter` saves `m_commandSrc` (`AIStates.cpp:3590`) and the update puts it back
    /// when the attack sub-machine becomes idle (`:3631`), because an interception sets `CMD_FROM_AI`
    /// (`:3658`). Revert check: drop the restore and the source stays `CMD_FROM_AI`.
    #[test]
    fn an_attack_move_resume_restores_the_saved_command_source() {
        let (mut g, unit) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        {
            let m = &mut g.objects.get_mut(&unit).unwrap().ai.machine;
            m.attack_move_command_source = CommandSource::Player;
            m.last_command_source = CommandSource::Ai;
            m.attack_move_resume = true;
            m.attack_move_goal = Coord3D::new(300.0, 100.0, 0.0);
        }
        g.ai_attack_move_enter(unit);
        assert_eq!(g.find_object(unit).unwrap().ai.machine.last_command_source, CommandSource::Player);
    }

    /// `privateAttackObject`'s `setLastCommandSource(cmdSource)` (`AIUpdate.cpp:3442`) records the
    /// command's own source; the player is not hard-wired. Revert check: drop the source write and the
    /// source stays `CMD_FROM_AI`.
    #[test]
    fn an_attack_order_records_its_command_source() {
        let (mut g, unit, first, _) = attack_move_world();
        g.ai_attack_object_from(unit, first, false, false, None, CommandSource::Script);
        assert_eq!(machine(&g, unit).last_command_source, CommandSource::Script);
        g.ai_attack_object_from(unit, first, false, false, None, CommandSource::Player);
        assert_eq!(machine(&g, unit).last_command_source, CommandSource::Player);
    }

    /// `AIUpdateInterface::privateIdle` (`AIUpdate.cpp:3092-3098`) clears the machine, which exits the attack's approach
    /// sub-state: `AIInternalMoveToState::onExit` calls `friend_endingMove` (AIStates.cpp:2804; AIUpdate.cpp:2046). The
    /// queued attack request is dropped by the next update's completed-move block (AIUpdate.cpp:1035-1041, :2018-2028),
    /// not by the order itself. Revert check: remove the approach's move exit from `end_attack_with`; `movement_complete`
    /// stays false and the first assertion fails.
    #[test]
    fn an_idle_order_ends_the_attack_it_interrupts() {
        let (mut g, unit, _first, _second) = attack_move_world();
        // Out of weapon range, so the attack has to request a path.
        let far = g.create_object(&attacker(), Coord3D::new(380.0, 100.0, 0.0), 0.0, 1);
        g.command_attack(0, &[unit], AttackTarget::Object(far));
        g.run_frames(2);
        assert!(g.find_object(unit).unwrap().ai.is_moving(), "the attack requested its path");
        g.ai_idle(unit, CommandSource::Player);
        let o = g.find_object(unit).unwrap();
        assert!(o.combat.attack.is_none(), "the order is dropped");
        assert!(o.ai.movement_complete, "the approach's exit ends its move (friend_endingMove)");
        g.run_frames(1);
        let o = g.find_object(unit).unwrap();
        assert_eq!(o.ai.request, crate::ai::PathRequest::None, "the completed block drops the queued attack request (destroyPath)");
        assert!(o.ai.path.is_empty(), "and the path");
    }

    /// `AIStateMachine::clear` (`AIStates.cpp:1017-1027`) drops the goal path and goal waypoint with the goal. Revert
    /// check: remove the two clears in `ai_clear` and the path and waypoint survive.
    #[test]
    fn clearing_the_machine_drops_its_goal_path_and_waypoint() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        if let Some(o) = g.objects.get_mut(&u) {
            o.ai.machine.goal_path = vec![Coord3D::new(60.0, 60.0, 0.0)];
            o.ai.machine.goal_waypoint = Some(WalkAt { waypoint: 3, open: true });
        }
        g.ai_clear(u);
        let m = machine(&g, u);
        assert!(m.goal_path.is_empty(), "the goal path is dropped");
        assert!(m.goal_waypoint.is_none(), "the goal waypoint is dropped");
    }

    /// `AIHuntState::onExit` (`AIStates.cpp:7169`) releases the temporary weapon lock. Revert check: drop the Hunt arm
    /// of `ai_state_exit` and the lock survives the hunt.
    #[test]
    fn leaving_a_hunt_releases_the_temporary_weapon_lock() {
        let (mut g, unit, _first, _second) = attack_move_world();
        assert!(g.set_weapon_lock(unit, 0, crate::weapon::WeaponLockType::Temporarily));
        assert!(g.find_object(unit).unwrap().combat.weapons.temporary_lock.is_some());
        g.ai_state_exit(unit, AiStateType::Hunt, true);
        assert!(g.find_object(unit).unwrap().combat.weapons.temporary_lock.is_none(), "the hunt's exit releases the lock");
    }

    /// `AIAttackState::onEnter` takes `isCurWeaponLocked()` (AIStates.cpp:5462, :5584): the weapon set's lock, not the
    /// attack order's slot. A plain attack with a temporary lock on its weapon records the lock. Revert check: restore
    /// the `locked_slot` read and the flag is false, because this attack carries no fire order.
    #[test]
    fn an_attack_on_enter_records_the_weapon_set_lock() {
        let (mut g, unit, first, _second) = attack_move_world();
        assert!(g.set_weapon_lock(unit, 0, crate::weapon::WeaponLockType::Temporarily));
        g.ai_start_attack(unit, AttackTarget::Object(first), false, false, None, AiStateType::Idle);
        assert!(g.find_object(unit).unwrap().ai.machine.locked_weapon_on_enter, "the locked weapon is recorded on entry");
    }

    /// `AIInternalMoveToState::onExit` (`AIStates.cpp:1747`) ends the move, and the completed-move block of the update
    /// (`AIUpdate.cpp:1035`) drops its path. The exit keeps the path until that update, and the update drops it. Revert
    /// check: remove the block's call in `ai_update_one` and the path survives the update.
    #[test]
    fn a_move_that_exits_drops_its_path_in_the_update() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        g.ai_move_to_position_cmd(u, Coord3D::new(350.0, 350.0, 0.0), CommandSource::Player);
        g.run_frames(3);
        assert!(g.find_object(u).unwrap().ai.is_moving(), "the move is under way");
        g.ai_command_set_state(u, AiStateType::Busy);
        assert!(g.find_object(u).unwrap().ai.is_moving(), "the exit itself keeps the path until the update");
        g.run_frames(1);
        let o = g.find_object(u).unwrap();
        assert_eq!(o.ai.machine.current, AiStateType::Busy);
        assert!(!o.ai.is_moving(), "the completed move's path is dropped");
    }

    /// The completed-move block runs before `doLocomotor` in `AIUpdateInterface::update` (`AIUpdate.cpp:1035` before
    /// `:1114`): the path and the locomotor goal are gone before the locomotor moves, so the update that ends a move
    /// does not step the unit along the stale path. Revert check: run `ai_do_locomotor` before the completed-move block
    /// in `ai_update_one` and the unit steps once more in the exit's update.
    #[test]
    fn a_completed_move_drops_its_path_before_the_locomotor_moves() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        g.ai_move_to_position_cmd(u, Coord3D::new(350.0, 350.0, 0.0), CommandSource::Player);
        g.run_frames(3);
        assert!(g.find_object(u).unwrap().ai.is_moving(), "the move is under way");
        g.ai_command_set_state(u, AiStateType::Busy);
        let before = g.find_object(u).unwrap().position;
        g.run_frames(1);
        let after = g.find_object(u).unwrap().position;
        assert_eq!(after, before, "the exit's update does not move the unit along the dropped path");
    }

    /// The pop to goal of the completed-move block (`AIUpdate.cpp:1044-1058`) records the goal cell's centre as the
    /// final position, through `setFinalPosition` (`AIUpdate.h:525`), which leaves `m_doFinalPosition` FALSE. Revert
    /// check: remove the `ai_pop_to_goal` call from `ai_end_completed_move` and the final position stays at the origin.
    #[test]
    fn a_completed_move_records_its_final_position_at_the_goal_cell() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        g.ai_move_to_position_cmd(u, Coord3D::new(350.0, 350.0, 0.0), CommandSource::Player);
        g.run_frames(3);
        let pos = g.find_object(u).unwrap().position;
        if let Some(o) = g.objects.get_mut(&u) {
            o.ai.movement_complete = true;
        }
        g.ai_end_completed_move(u);
        let o = g.find_object(u).unwrap();
        assert!(!o.ai.do_final_position, "setFinalPosition leaves the final-position move off");
        assert_ne!(o.ai.final_position, Coord3D::ZERO, "the pop recorded a final position");
        assert!(o.ai.final_position.dist_2d(&pos) < crate::pathfind::PATHFIND_CELL_SIZE_F, "the final position is the unit's goal cell");
    }

    /// The pop to goal reads the unit's stored goal cell (`m_pathfindGoalCell`), not the cell it stands in: a goal set in
    /// the neighbouring cell is the one recorded (`goalPosition`, AIPathfind.cpp:5303). Revert check: derive the goal
    /// from the unit's own position again and the final position lands in the unit's cell (x about 55), not the goal's.
    #[test]
    fn the_pop_to_goal_reads_the_stored_goal_cell() {
        // The unit stands near the east edge of its cell (x 58) and its goal cell lies a cell east within reach
        // (`dx*dx+dy*dy < CELL^2`, AIUpdate.cpp:1049). The goal cell's centre depends on the footprint's centring.
        let (mut g, u) = world_with_walker(Coord3D::new(58.0, 50.0, 0.0));
        let po = g.path_object(u).expect("a walker has a locomotor");
        let (_, center) = po.radius_and_center();
        let goal_pos = Coord3D::new(if center { 62.0 } else { 52.0 }, 50.0, 0.0);
        g.pathfinder.update_goal(&po, &goal_pos);
        let stored = g.pathfinder.goal_position(&g.terrain, u).expect("the goal cell is stored");
        if let Some(o) = g.objects.get_mut(&u) {
            o.ai.movement_complete = true;
        }
        g.ai_end_completed_move(u);
        let fx = g.find_object(u).unwrap().ai.final_position.x;
        assert!((fx - stored.x).abs() < 1e-3, "the final position is the stored goal cell {stored:?}, not the unit's cell: {fx}");
    }

    /// `AIInternalMoveToState::onEnter` skips the adjust and the goal update for a unit with `OBJECT_STATUS_RIDER8`
    /// (AIStates.cpp:1661). Revert check: drop the RIDER8 test and the goal moves to the adjusted cell centre.
    #[test]
    fn a_rider8_unit_keeps_its_move_goal_unadjusted() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        if let Some(o) = g.objects.get_mut(&u) {
            o.status_bits |= crate::object::status::RIDER8;
        }
        let click = Coord3D::new(200.0, 200.0, 0.0);
        g.ai_move_to_position_cmd(u, click, CommandSource::Player);
        let o = g.find_object(u).unwrap();
        assert_eq!(o.ai.move_goal, Some(click), "the goal is the click, not an adjusted cell");
        assert!(g.pathfinder.goal_position(&g.terrain, u).is_none(), "no goal cell is taken");
    }

    /// `AIIdleState::onEnter` (`AIStates.cpp:1313-1340`) does not touch the path: the move's exit keeps it until the
    /// completed-move block of the update drops it. Revert check: restore the path clear in `ai_idle_enter` and the
    /// path is gone right after the command.
    #[test]
    fn an_idle_entry_keeps_the_path_until_the_update() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        g.ai_move_to_position_cmd(u, Coord3D::new(350.0, 350.0, 0.0), CommandSource::Player);
        g.run_frames(3);
        assert!(g.find_object(u).unwrap().ai.is_moving(), "the move is under way");
        g.ai_command_set_state(u, AiStateType::Idle);
        assert!(g.find_object(u).unwrap().ai.has_path(), "the idle entry keeps the path");
        // The exit ran `friend_endingMove`, and the main machine is `AI_IDLE`: `isMoving` is FALSE (AIUpdate.cpp:3196-3198).
        assert!(!g.find_object(u).unwrap().ai.is_moving(), "an idle unit is not moving, whatever path it still holds");
        g.run_frames(1);
        let o = g.find_object(u).unwrap();
        assert_eq!(o.ai.machine.current, AiStateType::Idle);
        assert!(!o.ai.is_moving(), "the completed move's path is dropped in the update");
    }

    /// `AIInternalMoveToState::onEnter`'s adjust (AIStates.cpp:1661-1664) runs when the move begins, so the move's goal
    /// is the free cell beside the unit standing on the ordered point. Revert check: skip the adjust in
    /// `ai_internal_move_enter` and the goal stays on the occupied point.
    #[test]
    fn a_move_goal_is_adjusted_when_the_move_begins() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        let point = Coord3D::new(200.0, 200.0, 0.0);
        let _blocker = g.create_object(&walker(), point, 0.0, 0);
        g.ai_move_to_position_cmd(u, point, CommandSource::Player);
        let goal = g.find_object(u).unwrap().ai.goal().expect("a move goal");
        assert!(goal.dist_2d(&point) > 1.0 && goal.dist_2d(&point) < 20.0, "the goal moved beside the blocker: {goal:?}");
    }

    /// A move's path request (`requestPath`, AIUpdate.cpp:479) takes its destination as given: the adjust ran when the
    /// move began, and the compute (`findPath`, AIUpdate.cpp:1726) does not adjust again. Revert check: leave the
    /// request's `adjusts_destination` set and the compute adjusts the goal a second time.
    #[test]
    fn a_path_request_takes_its_destination_as_given() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        let point = Coord3D::new(200.0, 200.0, 0.0);
        g.ai_request_path(u, point, true);
        let o = g.find_object(u).unwrap();
        assert_eq!(o.ai.requested_destination, point, "the request keeps the destination it was given");
        assert!(!o.ai.adjusts_destination, "the path compute does not adjust the goal again");
    }

    /// `AIMoveAndTightenState::onEnter` (`AIStates.cpp:2234-2245`): `setAdjustsDestination(false)` and
    /// `removeGoal(obj)` before the approach request, so the tightened unit holds no goal cell while it walks.
    /// Revert check: drop the `removeGoal` and the stale goal cell survives; drop `move_adjusts = false` and the
    /// state adjusts.
    #[test]
    fn a_move_and_tighten_removes_its_old_goal_cell() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        g.ai_move_to_position_cmd(u, Coord3D::new(85.0, 85.0, 0.0), CommandSource::Player);
        assert!(g.ai_pathfind_goal(u).is_some(), "the earlier move keyed a goal cell");
        if let Some(o) = g.objects.get_mut(&u) {
            o.ai.machine.goal_position = Coord3D::new(62.0, 62.0, 0.0);
        }
        g.ai_command_set_state(u, AiStateType::MoveAndTighten);
        let m = machine(&g, u);
        assert_eq!(m.current, AiStateType::MoveAndTighten);
        assert!(!m.move_adjusts, "the tighten move never adjusts its destination (AIStates.cpp:2236)");
        assert!(g.ai_pathfind_goal(u).is_none(), "removeGoal runs before the approach request (AIStates.cpp:2241)");
    }

    /// `AIMoveOutOfTheWayState::onEnter` (`AIStates.cpp:2148-2160`) runs `AIInternalMoveToState::onEnter`, whose
    /// adjust block ends with `updateGoal` (`:1661-1668`): the stepped-aside unit's goal cell is the adjusted last
    /// node. Revert check: drop the `updateGoal` and the unit keeps no goal cell.
    #[test]
    fn a_move_out_of_the_way_keys_its_goal_cell() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        if let Some(o) = g.objects.get_mut(&u) {
            // The move-away search left this path (`getMoveAwayFromPath`, AIUpdate.cpp:3333-3335).
            o.ai.path = vec![Coord3D::new(85.0, 85.0, 0.0)];
        }
        assert!(g.ai_pathfind_goal(u).is_none(), "the unit starts with no goal cell");
        let ret = g.ai_move_out_of_way_enter(u);
        assert_eq!(ret, StateReturn::Continue);
        let goal = g.find_object(u).unwrap().ai.move_goal.expect("the last node is the goal");
        assert_eq!(g.ai_pathfind_goal(u), Some(goal), "the base move keys its goal cell (AIStates.cpp:1667)");
        assert!(machine(&g, u).move_adjusts, "the move-out-of-the-way adjusts");
    }

    /// `if (ai->getCurLocomotor()->isUltraAccurate()) setAdjustsDestination(false)` (`AIStates.cpp:1627-1632`):
    /// an ultra-accurate move (the jet taxi) keeps the request exactly as given and takes no goal cell.
    /// Revert check: drop the ultra-accurate rule and the goal is adjusted and keyed.
    #[test]
    fn an_ultra_accurate_move_does_not_adjust_or_key_its_goal() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        if let Some(o) = g.objects.get_mut(&u) {
            o.loco_rt.ultra_accurate = true;
        }
        let point = Coord3D::new(85.0, 85.0, 0.0);
        g.ai_move_to_position_cmd(u, point, CommandSource::Player);
        let o = g.find_object(u).unwrap();
        assert_eq!(o.ai.move_goal, Some(point), "the precise destination is not adjusted");
        assert!(!o.ai.machine.move_adjusts, "the move does not adjust (AIStates.cpp:1629-1632)");
        assert!(g.ai_pathfind_goal(u).is_none(), "no goal cell is keyed (AIStates.cpp:1661)");
    }

    // ---- Lane 127: the finished-move rule and the dock machine's halt ------------------------

    /// `AIInternalMoveToState::update`'s arrival (`AIStates.cpp:1929-1940`) returns `STATE_SUCCESS` and
    /// only calls `setLocomotorGoalNone` (when the move adjusts); `m_goalPosition` and `m_path` stay.
    /// The path goes away in the end-of-update `m_movementComplete` block (`AIUpdate.cpp:1035-1041`),
    /// which runs because `AIInternalMoveToState::onExit` called `friend_endingMove`
    /// (`AIStates.cpp:1747`). Revert check: the old `ai_finish_move` (clear the path, `move_goal = None`,
    /// `movement_complete = true`) fails the first three assertions.
    #[test]
    fn a_finished_move_keeps_its_path_until_the_completed_move_block() {
        // A move whose path landed and whose only node is the unit's own position: the situation a
        // docker is in when its entry cell is the cell it already stands in (the engine's `m_path`
        // holds the start node alone, `getPath() != nullptr`).
        let (mut g, u) = world_with_walker(Coord3D::new(50.5, 50.5, 0.0));
        let here = g.find_object(u).unwrap().position;
        {
            let o = g.objects.get_mut(&u).expect("unit");
            o.ai.path = Vec::new();
            o.ai.raw_path = Some(crate::pathfind::Path {
                nodes: vec![crate::pathfind::PathNode { pos: here, can_optimize: true, next_optimized: None }],
                blocked_by_ally: false,
            });
            o.ai.move_goal = Some(here);
            o.ai.machine.move_adjusts = true;
            o.ai.locomotor_goal_type = LocoGoalType::PositionOnPath;
            o.ai.move_waiting_for_path = false;
            o.ai.waiting_for_path = false;
            o.ai.path_timestamp = 0;
            o.ai.move_path_timestamp = 0;
        }
        assert!(g.find_object(u).unwrap().ai.has_path(), "the entry path exists");
        let ret = g.ai_internal_move_update(u);
        assert_eq!(ret, StateReturn::Success, "the move arrived");
        let o = g.find_object(u).unwrap();
        assert!(o.ai.has_path(), "a finished move keeps its Path");
        assert!(!o.ai.movement_complete, "friend_endingMove runs in onExit, not on arrival");
        assert_eq!(o.ai.locomotor_goal_type, LocoGoalType::None, "getAdjustsDestination -> setLocomotorGoalNone");
        assert!(o.ai.move_goal.is_some(), "m_goalPosition is not cleared by the arrival");
        // The state's exit ends the move; the same update's completed-move block drops the path.
        g.ai_internal_move_exit(u);
        g.ai_end_completed_move(u);
        let o = g.find_object(u).unwrap();
        assert!(!o.ai.has_path(), "the completed move drops the path (destroyPath, AIUpdate.cpp:1040)");
        assert_eq!(o.ai.locomotor_goal_type, LocoGoalType::None);
    }

    /// `AIInternalMoveToState::update`'s waiting gate (`AIStates.cpp:1786-1813`) returns before the
    /// `if (thePath != nullptr) ai->setLocomotorGoalPositionOnPath()` re-arm (`:1875-1876`), so a move
    /// waiting for its next path keeps the goal type it had. The port re-armed it before the gate, which
    /// made a unit that had just arrived (`goal NONE`) drive along its finished path while the next
    /// request was pending: AN3 s1 worker 238 oscillated 0.9 units inside its dock approach.
    /// Revert check: the pre-gate `ai_sync_goal_on_path` turns the goal back into `PositionOnPath`.
    #[test]
    fn the_waiting_gate_does_not_rearm_the_locomotor_goal() {
        let (mut g, u) = world_with_walker(Coord3D::new(50.0, 50.0, 0.0));
        let ahead = Coord3D::new(60.0, 50.0, 0.0);
        {
            let o = g.objects.get_mut(&u).expect("unit");
            o.ai.path = vec![ahead];
            o.ai.raw_path = Some(crate::pathfind::Path {
                nodes: vec![
                    crate::pathfind::PathNode { pos: Coord3D::new(50.0, 50.0, 0.0), can_optimize: true, next_optimized: Some(1) },
                    crate::pathfind::PathNode { pos: ahead, can_optimize: true, next_optimized: None },
                ],
                blocked_by_ally: false,
            });
            o.ai.move_goal = Some(ahead);
            // Just arrived (the arrival's `setLocomotorGoalNone`), and a new request is pending.
            o.ai.locomotor_goal_type = LocoGoalType::None;
            o.ai.move_waiting_for_path = true;
            o.ai.waiting_for_path = true;
        }
        let ret = g.ai_internal_move_update(u);
        assert_eq!(ret, StateReturn::Continue, "waiting for the path");
        assert_eq!(
            g.find_object(u).unwrap().ai.locomotor_goal_type,
            LocoGoalType::None,
            "the waiting gate returns before the locomotor goal re-arm"
        );
    }

    /// A dock to drive to: a warehouse with five approach positions (the shape the AN3 supply stash
    /// uses) and a truck-like docker.
    fn dock_world() -> (GameLogic, ObjectId, ObjectId) {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let docker = g.create_object(&walker(), Coord3D::new(50.0, 100.0, 0.0), 0.0, 0);
        let mut dock_t = ObjectTemplate::new("Dock");
        dock_t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        dock_t.geometry.major_radius = 12.0;
        dock_t.max_health = 500.0;
        dock_t.initial_health = 500.0;
        dock_t.dock = Some(crate::supply::DockTemplate {
            kind: crate::supply::DockKind::Warehouse,
            starting_boxes: 5,
            delete_when_empty: false,
            number_approach_positions: 5,
            allows_passthrough: true,
        });
        let dock = g.create_object(&Arc::new(dock_t), Coord3D::new(120.0, 100.0, 0.0), 0.0, 0);
        (g, docker, dock)
    }

    /// Step until the dock machine is in `AI_DOCK_MOVE_TO_DOCK` (`AIDock.cpp:57`).
    fn step_to_move_to_dock(g: &mut GameLogic, docker: ObjectId) -> bool {
        for _ in 0..600 {
            g.update();
            if g.find_object(docker).is_some_and(|o| o.ai.machine.dock_sub == DockSub::MoveToDock) {
                return true;
            }
        }
        false
    }

    /// `AIDockMachine::halt` (`AIDock.cpp:70-86`) only calls `dock->cancelDock`; `StateMachine::halt`
    /// (`StateMachine.cpp:753-763`) sets `m_currentState = nullptr` **without running its `onExit`**, so
    /// an interrupted dock runs no `AIInternalMoveToState::onExit`/`friend_endingMove`: the move is not
    /// marked complete and the path is not dropped by the update's completed-move block.
    /// Revert check: the old `ai_dock_exit` called `ai_internal_move_exit`, which sets
    /// `movement_complete`.
    #[test]
    fn an_interrupted_dock_does_not_end_the_move() {
        let (mut g, docker, dock) = dock_world();
        g.ai_dock(docker, dock);
        assert!(step_to_move_to_dock(&mut g, docker), "the docker reached AIDockMoveToDockState");
        assert!(!g.find_object(docker).unwrap().ai.movement_complete, "the dock move is running");
        g.ai_dock_exit(docker);
        let o = g.find_object(docker).unwrap();
        assert!(!o.ai.movement_complete, "the halt runs no sub-state onExit");
        assert!(o.ai.has_path() || o.ai.waiting_for_path, "the interrupted move keeps its path");
        // `DockUpdate::cancelDock` (`DockUpdate.cpp:381`) released the approach position, so the docker
        // may reserve the front spot again.
        assert!(g.dock_is_clear_to_approach(dock, docker), "the halt's cancelDock released the spot");
    }

    /// `AIDockMoveToDockState::onEnter`'s `getMachine()->lock("AIDockMoveToDockState::onEnter")`
    /// (`AIDock.cpp:515`) locks the *dock* machine, not the owner's `AIStateMachine`; nothing can call
    /// `setState`/`clear` on the dock machine from outside, so the owner's machine stays unlocked and a
    /// player command interrupts the dock move. Revert check: the old `ai_set_machine_locked(id, true)`
    /// sets `machine.locked`.
    #[test]
    fn a_dock_move_does_not_lock_the_owner_machine() {
        let (mut g, docker, dock) = dock_world();
        g.ai_dock(docker, dock);
        assert!(step_to_move_to_dock(&mut g, docker), "the docker reached AIDockMoveToDockState");
        assert!(!machine(&g, docker).locked, "the owner's AI machine is not locked");
        assert!(!g.find_object(docker).unwrap().ai.movement_complete);
    }

    fn supply_truck_data() -> crate::supply::SupplyTruckTemplate {
        crate::supply::SupplyTruckTemplate { max_boxes: 1, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 500.0 }
    }

    /// `SupplyTruckAIUpdate::privateIdle` latches the supply machine's Busy state for a player's stop
    /// (`SupplyTruckAIUpdate.cpp:193-200`); `WorkerAIUpdate::privateIdle` has that latch commented out
    /// (`WorkerAIUpdate.cpp:531-540`: "the worker's dozer brain will get completely screwed"). The port
    /// maps `WorkerAIUpdate` into `template.supply_truck` too, so the discriminator is the dozer half.
    /// Revert check: with the latch on `template.supply_truck.is_some()` the worker's force_busy is
    /// true and the first assertion fails.
    #[test]
    fn a_players_stop_latches_the_busy_state_only_for_a_supply_truck() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let mut worker_t = ObjectTemplate::new("Worker");
        worker_t.kind_of = kind_of::DOZER | kind_of::VEHICLE;
        worker_t.geometry.major_radius = 5.0;
        worker_t.dozer = Some(crate::dozer::DozerTemplate::default());
        worker_t.supply_truck = Some(supply_truck_data());
        let w = g.create_object(&Arc::new(worker_t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let mut truck_t = ObjectTemplate::new("Truck");
        truck_t.kind_of = kind_of::VEHICLE;
        truck_t.geometry.major_radius = 5.0;
        truck_t.supply_truck = Some(supply_truck_data());
        let t = g.create_object(&Arc::new(truck_t), Coord3D::new(160.0, 100.0, 0.0), 0.0, 0);
        g.ai_idle(w, CommandSource::Player);
        g.ai_idle(t, CommandSource::Player);
        assert!(!g.find_object(w).unwrap().supply_ai.force_busy, "a worker's stop does not latch the Busy state");
        assert!(g.find_object(t).unwrap().supply_ai.force_busy, "a supply truck's stop latches it");
    }

    /// A worker's `SupplyTruckAIUpdate` half runs inside `WorkerAIUpdate::update`'s `AS_SUPPLY_TRUCK`
    /// branch (`WorkerAIUpdate.cpp:257-309`), which the port models in `dozer_module_update`; the AI
    /// slot's supply half is only `SupplyTruckAIUpdate::update` (a plain truck or a Chinook). A worker
    /// in `AS_DOZER` must not advance its supply machine from its AI slot.
    /// Revert check: with the supply half called for every `supply_truck` template (workers carry the
    /// data), the machine runs `Busy -> Idle` and the assertion fails.
    #[test]
    fn a_worker_in_dozer_mode_does_not_run_the_supply_half_in_its_ai_slot() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let mut worker_t = ObjectTemplate::new("Worker");
        worker_t.kind_of = kind_of::DOZER | kind_of::VEHICLE;
        worker_t.geometry.major_radius = 5.0;
        worker_t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        worker_t.dozer = Some(crate::dozer::DozerTemplate::default());
        worker_t.supply_truck = Some(supply_truck_data());
        let w = g.create_object(&Arc::new(worker_t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        if let Some(o) = g.objects.get_mut(&w) {
            o.ai.wake_frame = 0;
            o.supply_ai.state = crate::supply::SupplyState::Busy;
            o.ai.machine.current = AiStateType::Idle;
        }
        g.ai_update_one(w);
        assert_eq!(
            g.find_object(w).unwrap().supply_ai.state,
            crate::supply::SupplyState::Busy,
            "a dozer-mode worker's supply machine is not run by its AI slot"
        );
    }

    fn garrison_building() -> ObjectTemplate {
        let mut t = ObjectTemplate::new("Bunker");
        t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        t.max_health = 500.0;
        t.initial_health = 500.0;
        t.geometry.major_radius = 15.0;
        t.geometry.minor_radius = 15.0;
        t.contain = Some(Arc::new(crate::contain::ContainTemplate {
            kind: crate::contain::ContainKind::Garrison,
            capacity: 4,
            allow_inside_kind_of: None,
            forbid_inside_kind_of: 0,
            allow_allies_inside: false,
            allow_enemies_inside: false,
            allow_neutral_inside: true,
            passengers_allowed_to_fire: false,
            damage_percentage_to_units: 0.0,
            number_of_exit_paths: 1,
            exit_delay: 0,
            door_open_time: 1,
            scatter_nearby_on_exit: true,
            orient_like_container_on_exit: false,
            reset_mood_check_on_exit: false,
            health_regen: 0.0,
            do_i_heal_objects: false,
            heal_frames: 0.0,
            initial_payload: Vec::new(),
            tunnel_frames_for_full_heal: 1.0,
            is_enclosing_container: true,
            fire_points: crate::contain::GarrisonFirePoints::default(),
            passengers_in_turret: false,
            weapon_bonus_passed_to_passengers: false,
            immune_to_clear_building_attacks: false,
        }));
        t
    }

    /// `AIEnterState::onExit` (`AIStates.cpp:6274-6281`) runs `AIInternalMoveToState::onExit(status)`
    /// before clearing the target, so `friend_endingMove` marks the move complete and the update's
    /// completed-move block drops the entry path (`AIUpdate.cpp:1035-1041`).
    /// Revert check: without the move exit in `ai_enter_exit`, `movement_complete` stays false and the
    /// path survives the exit.
    #[test]
    fn the_enter_states_exit_ends_its_move() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        g.add_player(1, 0);
        let u = g.create_object(&walker(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        // Neutral to player 0 (no relationship set), so the order is accepted.
        let b = g.create_object(&Arc::new(garrison_building()), Coord3D::new(300.0, 100.0, 0.0), 0.0, 1);
        g.command_enter_from(0, &[u], b, CommandSource::Player);
        for _ in 0..3 {
            g.update();
        }
        let o = g.find_object(u).unwrap();
        assert_eq!(o.ai.machine.current, AiStateType::Enter, "the enter state is running");
        assert!(o.ai.has_path(), "the entry move holds a path");
        g.ai_clear(u);
        assert!(g.find_object(u).unwrap().ai.movement_complete, "the exit ran the move's onExit");
        g.ai_end_completed_move(u);
        let o = g.find_object(u).unwrap();
        assert!(!o.ai.has_path(), "the completed-move block dropped the entry path");
        assert!(!o.ai.is_moving());
    }

    /// `ChinookAIUpdate::update` (`ChinookAIUpdate.cpp:1078-1230`) does its own work and then returns
    /// `SupplyTruckAIUpdate::update()` (supply machine) before `AIUpdateInterface::update`. On the frame
    /// a combat drop ends, the supply machine therefore already sees the Chinook as available again
    /// (`ChinookAIUpdate::isAvailableForSupplying`, `:1016-1030`) and can leave `Docking` in the same
    /// update. Revert check: with the supply half called first the state stays `Docking` for a frame and
    /// the step order is supply -> chinook.
    #[test]
    fn the_chinook_half_runs_before_its_supply_half() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let mut t = ObjectTemplate::new("Chinook");
        t.kind_of = kind_of::AIRCRAFT;
        t.max_health = 300.0;
        t.initial_health = 300.0;
        t.geometry.major_radius = 10.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Wings, 60.0, 100.0, 100.0, 360.0));
        t.chinook = Some(crate::chinook::ChinookTemplate::default());
        t.supply_truck = Some(supply_truck_data());
        let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 50.0), 0.0, 0);
        // Clear the AI module's first-update frame, then arm a drop that finishes this update.
        g.run_frames(1);
        let pos = g.find_object(id).unwrap().position;
        g.chinooks.insert(
            id,
            crate::chinook::ChinookState {
                phase: crate::chinook::ChinookPhase::CombatDrop,
                target_pos: pos,
                target_obj: None,
                dest_z: pos.z,
                ropes: Vec::new(),
                done: false,
            },
        );
        {
            let o = g.objects.get_mut(&id).unwrap();
            o.supply_ai.state = crate::supply::SupplyState::Docking;
            o.ai.machine.sleep_till = 0;
        }
        assert!(!g.supply_available(id), "the drop makes the Chinook unavailable");
        g.ai_step_log.clear();
        g.ai_update_one(id);
        assert!(g.supply_available(id), "the drop finished in the Chinook's own update");
        assert_eq!(
            g.find_object(id).unwrap().supply_ai.state,
            crate::supply::SupplyState::Wanting,
            "the supply machine saw the finished drop in the same update (Docking -> Wanting)"
        );
        assert_eq!(
            g.ai_step_log.iter().map(|(step, _)| *step).collect::<Vec<_>>(),
            vec!["chinook", "supply", "ai"],
            "ChinookAIUpdate::update order: own work, SupplyTruckAIUpdate::update, AIUpdateInterface::update"
        );
    }

    /// `AIInternalMoveToState::update`'s ground check (`AIStates.cpp:1912-1926`) measures to
    /// `ai->getPath()->getLastNode()`, the path's last *raw* node, not the last node of the optimised
    /// chain. A path whose raw tail is not part of the optimised chain (a patched/partial path) must
    /// use the raw node.
    /// Revert check: with `optimized_positions().last()` the check reads the optimised node and reports
    /// the unit at its goal.
    #[test]
    fn the_arrival_sanity_check_reads_the_raw_last_node() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let u = g.create_object(&walker(), Coord3D::new(195.0, 100.0, 0.0), 0.0, 0);
        let start = Coord3D::new(100.0, 100.0, 0.0);
        let mid = Coord3D::new(200.0, 100.0, 0.0);
        let tail = Coord3D::new(300.0, 100.0, 0.0);
        if let Some(o) = g.objects.get_mut(&u) {
            // The optimised chain is start -> mid; the raw path continues to `tail` (a raw node the
            // optimise pass of a patched path can leave behind).
            o.ai.raw_path = Some(crate::pathfind::Path {
                nodes: vec![
                    crate::pathfind::PathNode { pos: start, can_optimize: true, next_optimized: Some(1) },
                    crate::pathfind::PathNode { pos: mid, can_optimize: true, next_optimized: None },
                    crate::pathfind::PathNode { pos: tail, can_optimize: true, next_optimized: None },
                ],
                blocked_by_ally: false,
            });
            o.ai.path = vec![mid];
            o.ai.locomotor_goal_type = crate::ai::LocoGoalType::None;
        }
        assert!(!g.ai_at_goal(u, &tail), "the unit is over four cells from the raw last node");
    }

    /// Lane 157: `AIUpdateInterface::update` reports the least of its state machine's sleep, each turret's
    /// `updateTurretAI` sleep (AIUpdate.cpp:1090-1098) and `doLocomotor`'s (AIUpdate.cpp:1114-1116). The fixture
    /// sets the state's sleep to 50 frames and the turret's to 30 (or 200 for the turret-free unit's FOREVER
    /// locomotor). Revert checks: the early `turret.is_some() => UPDATE_SLEEP_NONE` gives 1 for the turreted unit;
    /// the immobile branch's `sleep = UPDATE_SLEEP_FOREVER` gives FOREVER for the turret-free unit.
    #[test]
    fn an_ai_update_sleeps_for_the_least_of_its_state_turret_and_locomotor() {
        let mut t = ObjectTemplate::new("Bunker");
        t.kind_of = crate::template::kind_of::STRUCTURE | crate::template::kind_of::IMMOBILE;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 5.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        let plain = Arc::new(t.clone());
        t.combat.turret_turn_rate = Some(0.1);
        t.combat.turret_pitch_rate = Some(0.05);
        let turreted = Arc::new(t);
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        let a = g.create_object(&turreted, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let b = g.create_object(&plain, Coord3D::new(100.0, 50.0, 0.0), 0.0, 0);
        g.run_frames(2);
        let now = g.frame();
        // The turreted unit: the turret sleeps 30 frames, the state 50. The least is the turret's 30.
        let turret_until = now + 30;
        if let Some(o) = g.objects.get_mut(&a) {
            o.combat.turret.as_mut().expect("has a turret").sleep_until = turret_until;
            o.ai.machine.sleep_till = now + 50;
        }
        assert_eq!(g.ai_update_sleep(a), 30, "the turret's sleep is the least");
        // The turret-free immobile unit: the state's 50 frames, not the locomotor's FOREVER.
        if let Some(o) = g.objects.get_mut(&b) {
            o.ai.machine.sleep_till = now + 50;
        }
        assert_eq!(g.ai_update_sleep(b), 50, "an immobile unit wakes for its state's sleep");
    }

    /// Lane 157: `AIFollowPathState::onEnter` (AIStates.cpp:3276-3310) runs the base `AIInternalMoveToState::onEnter` with the
    /// state's default `m_adjustDestinations = true` (AIStateMachine.h:257), so the first point of a plain path adjusts its
    /// goal even when more points follow. Only the update's `setAdjustsDestination(false)` for a next point is deferred
    /// (AIStates.cpp:3338-3400). AH7 s1's tunnel defenders leave along `ExitStart` -> `ExitEnd`: C++ requests the adjusted
    /// `ExitEnd` (the `to=` of the HPATH line), the port the raw one. Revert check: the `path.len() == 1` condition leaves
    /// the goal unset for a two-point path, so `ai_pathfind_goal` is `None`.
    #[test]
    fn a_plain_paths_first_point_adjusts_its_goal() {
        let mut t = ObjectTemplate::new("Walker");
        t.max_health = 100.0;
        t.initial_health = 100.0;
        // A 20-unit footprint: `getRadiusAndCenter` gives an even radius, so the goal is the non-centred `(cell + 0.05) * 10`.
        t.geometry.major_radius = 10.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        g.run_frames(1);
        g.ai_follow_path(id, vec![Coord3D::new(105.0, 105.0, 0.0), Coord3D::new(160.0, 105.0, 0.0)], false);
        let goal = g.ai_pathfind_goal(id).expect("the first point's adjusted goal cell is recorded");
        assert!((goal.x - 110.5).abs() < 1e-3 && (goal.y - 110.5).abs() < 1e-3, "adjusted goal {goal:?}");
    }

    /// `AIFollowPathState::onEnter` sets `setAdjustsDestination(false)` for a point with a next point (AIStates.cpp:3306),
    /// after the base `AIInternalMoveToState::onEnter` whose request still carries the adjust. The flag is what the
    /// request's wait block reads when the path arrives (`AIInternalMoveToState::update`, AIStates.cpp:1790-1807): a leg
    /// with a next point calls `removeGoal`, not `updateGoal(lastNode)`. Lane 159 (AH7 s1 f1953: a tunnel defender's exit
    /// leg marked its closest-path end as its goal cell, which C++ does not hold). Revert check: without the clear the
    /// first leg still adjusts.
    #[test]
    fn a_plain_paths_first_leg_stops_adjusting_once_a_point_follows() {
        let mut t = ObjectTemplate::new("Walker");
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 10.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        g.run_frames(1);
        g.ai_follow_path(id, vec![Coord3D::new(105.0, 105.0, 0.0), Coord3D::new(160.0, 105.0, 0.0)], false);
        assert!(!g.objects[&id].ai.machine.move_adjusts, "a leg with a next point does not adjust its final goal");
        g.ai_follow_path(id, vec![Coord3D::new(160.0, 105.0, 0.0)], false);
        assert!(g.objects[&id].ai.machine.move_adjusts, "the last point of a path adjusts");
    }

    /// `AIFollowPathState::update` calls `ai->ignoreObstacleID(INVALID_ID)` on every segment advance, "we have exited
    /// whatever object we are leaving" (AIStates.cpp:3377). A tunnel exit's container is ignored only for its first
    /// leg; the closest-path search of the next request must not still treat the container as free to move through
    /// (AIPathfind.cpp:8778, `startedStuck`). Lane 159 (AH7 s1 f1953). Revert check: without the clear the ignored
    /// obstacle stays set after the first leg is done.
    #[test]
    fn a_follow_path_segment_advance_clears_the_ignored_obstacle() {
        let mut t = ObjectTemplate::new("Walker");
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 5.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        g.run_frames(1);
        let container = ObjectId(7);
        g.ai_set_follow_path(id, vec![Coord3D::new(70.0, 50.0, 0.0), Coord3D::new(120.0, 50.0, 0.0)], AiStateType::FollowPath, Some(container), None);
        assert_eq!(g.objects[&id].ai.ignore_obstacle_id, container, "the container is the first leg's ignored obstacle");
        for _ in 0..200 {
            g.run_frames(1);
            if g.objects[&id].ai.machine.goal_path_index > 0 {
                break;
            }
        }
        assert!(g.objects[&id].ai.machine.goal_path_index > 0, "the unit reached its first point");
        assert_eq!(g.objects[&id].ai.ignore_obstacle_id, ObjectId::INVALID, "the segment advance cleared the ignored obstacle");
    }
}
