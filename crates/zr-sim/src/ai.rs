//! `AIUpdateInterface` (`Object/Update/AIUpdate.cpp`, `AI/AIStates.cpp`): the movement state the
//! pathfinder depends on. The movement kernels themselves live in [`crate::locomotor`]
//! (`Locomotor.cpp`); this module re-exports them so possession and the adapter keep using the same
//! names.
//!
//! Ported here, with the C++ cited:
//!
//! * `requestPath` / `requestAttackPath` / `requestApproachPath` and `setQueueForPathTime`
//!   (`AIUpdate.cpp:479/520/543/953`): a ground move queues the object on the pathfinder instead of
//!   computing synchronously. The first path (and every repath) is serviced over frames by
//!   [`GameLogic::process_pathfind_queue`], which reproduces `processPathfindQueue` +
//!   `AIUpdateInterface::doPathfind` and the `PATHFIND_CELLS_PER_FRAME` cell budget.
//! * `AIInternalMoveToState` (`AIStates.cpp:1600-1943`): the waiting-for-path gate, the blocked /
//!   blocked-and-stuck repath (`m_blockedRepathTimestamp`) and the `MIN_REPATH_TIME` goal-moved
//!   repath.
//! * `doLocomotor` (`AIUpdate.cpp:2127`): `m_blockedFrames` accumulation and the
//!   `m_curMaxBlockedSpeed` / `m_bumpSpeedLimit` speed limiting while blocked.
//! * `processCollision` + `blockedBy` + `calculateMaxBlockedSpeed` + `needToRotate` +
//!   `isMovingAwayFrom` + `hasHigherPathPriority` (`AIUpdate.cpp:1208-1582`): unit-unit blocking,
//!   run from the physics `onCollide` of each contact-list pair ([`GameLogic::on_collide`],
//!   `PhysicsUpdate.cpp:1278`), so the pair's own push is gated by the result.
//! * `patchPath` is called by `computePath` when `isBlockedAndStuck` (`AIUpdate.cpp:1722`).
//!
//! Reduced: the state-machine class hierarchy is not modelled (the sim drives a single reduced
//! move state); `AIUpdateInterface::isBlocked` is set by the partition scan rather than a physics
//! callback; and the full `Path::computePointOnPath` look-ahead is replaced by following the
//! optimised waypoints (as before).

use crate::crc::Crc;
use crate::math::{angle_diff, atan2, cos, sin, sqrt, Coord3D, PI};
use crate::module::{UpdateContext, UpdateModule, UpdateSleepTime, UPDATE_SLEEP_FOREVER};
use crate::object::{Object, ObjectId};
use crate::disabled::DisabledType;
use crate::pathfind::{Path, PathNode, PathObject, Pathfinder, PATHFIND_CELL_SIZE_F, PATHFIND_CLOSE_ENOUGH};
use crate::template::{collide_kind, LocomotorKind, LocomotorParams};
use crate::terrain::TerrainLogic;

pub use crate::locomotor::{maintain_current_position, move_towards_position};

/// `FAST_AS_POSSIBLE` (`AIUpdate.h`): "no explicit desired speed".
pub const FAST_AS_POSSIBLE: f32 = 999999.0;
/// `AIStateMachine::MIN_REPATH_TIME` (`AIStateMachine.h:291`).
pub const MIN_REPATH_TIME: u32 = 10;

/// `LocoGoalType` (`AIUpdate.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum LocoGoalType {
    #[default]
    None,
    PositionOnPath,
    PositionExplicit,
    Angle,
}

/// Pathfinder seam (see module docs). Straight line for now (the `computeQuickPath` case).
pub fn compute_path(_from: &Coord3D, to: &Coord3D, _terrain: &TerrainLogic) -> Vec<Coord3D> {
    vec![*to]
}

/// How a path was requested; drives `doPathfind`'s branch (`AIUpdate.cpp:396-473`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum PathRequest {
    #[default]
    None,
    /// `requestPath`: `computePath`.
    Move,
    /// `requestAttackPath`: `computeAttackPath` / `findAttackPath`.
    Attack,
    /// `requestApproachPath`: `findClosestPath`.
    Approach,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AiState {
    /// Remaining waypoints; the last one is the final goal.
    pub path: Vec<Coord3D>,
    /// Current forward speed (distance per frame).
    pub speed: f32,
    /// When set the AI state machine is suspended (possession, docs/POSSESSION.md): the update
    /// leaves the locomotor alone and goes to sleep. Whoever sets it must drive the locomotor.
    pub suspended: bool,
    /// `AIUpdateInterface::m_attitude` (`AttitudeType`): `-2` sleep, `-1` passive, `0` normal,
    /// `1` alert, `2` aggressive. Set by `TEAM_SET_ATTITUDE` / `NAMED_SET_ATTITUDE`.
    pub attitude: i8,
    /// `AIUpdateInterface::m_attackInfo`: `None` is the null pointer; `Some(name)` an
    /// `AttackPriorityInfo` (a name that is not a defined set is the engine's default info, whose
    /// priorities are all 1). Set by `TEAM_APPLY_ATTACK_PRIORITY_SET` and by joining a team.
    pub attack_info: Option<String>,

    // ---- `AIUpdateInterface` movement state -------------------------------------------------
    /// `m_requestedDestination`.
    pub requested_destination: Coord3D,
    /// `m_isFinalGoal`.
    pub is_final_goal: bool,
    /// Which `doPathfind` branch the pending request uses (`m_isAttackPath` / `m_isApproachPath`).
    pub request: PathRequest,
    /// `AIUpdateInterface::m_isAttackPath` (AIUpdate.cpp:268): set by `requestAttackPath` (:528) and kept by a computed
    /// attack path (:445), cleared by `requestPath` (:490) and by an attack path that fails (:450). The approach state reads
    /// `m_stopIfInRange = !ai->isAttackPath()` (AIStates.cpp:2673), so an attack path runs to its end, not the first in-range frame.
    pub attack_path_active: bool,
    /// `m_requestedVictimID`.
    pub requested_victim: ObjectId,
    /// `m_waitingForPath`.
    pub waiting_for_path: bool,
    /// `AIInternalMoveToState::m_waitingForPath` (`AIStates.cpp:1568`). The move states own this
    /// flag: `computePath` sets it true and only the state clears it once it has consumed the
    /// pathfinder's result (`AIStates.cpp:1605,1786-1803`). It must be separate from
    /// [`waiting_for_path`](Self::waiting_for_path) (`AIUpdateInterface::m_waitingForPath`), which
    /// `doPathfind` clears as soon as it services the request (`AIUpdate.cpp:402`) — otherwise a
    /// move that found no path never sees a null path and never fails.
    pub move_waiting_for_path: bool,
    /// `AIUpdateInterface::m_pathTimestamp` (AIUpdate.h:720): when the current path was computed (`0` = none yet).
    /// Set only by `computeQuickPath` / `computePath` / `doPathfind` (AIUpdate.cpp:1657, :1768, :2003) and read by the
    /// 3-frame quick-repath queue of `requestPath` (AIUpdate.cpp:499-503). Never reset by a new request.
    pub path_timestamp: u32,
    /// `AIInternalMoveToState::m_pathTimestamp` (AIStateMachine.h:298): the move state's own clock, set while it waits
    /// for a path (AIStates.cpp:1789) and read by its repath gate (AIStates.cpp:1879). Separate from `path_timestamp`.
    pub move_path_timestamp: u32,
    /// `m_queueForPathFrame` (`0` = none).
    pub queue_for_path_frame: u32,
    /// The `AIUpdateInterface` update module's next call frame (`UpdateModule::m_nextCallFrameAndPhase`,
    /// UpdateModule.h:141; `0` = awake). The constructor ends with
    /// `setWakeFrame(getObject(), UPDATE_SLEEP_NONE)` (`AIUpdate.cpp:299-301`), which schedules `now + 1`
    /// (`UpdateModule.cpp:80-84`), so the module's first update is one frame after creation: a map object
    /// created during `startNewGame` (frame 0) first runs `AIIdleState::update` at frame 1. The port runs
    /// the AI from per-frame passes instead of from that sleepy module, so this field gates them.
    pub wake_frame: u32,
    /// `m_blockedFrames`.
    pub blocked_frames: u16,
    /// `m_isBlocked`.
    pub is_blocked: bool,
    /// `m_isBlockedAndStuck`.
    pub is_blocked_and_stuck: bool,
    /// `m_ignoreCollisionsUntil`.
    pub ignore_collisions_until: u32,
    /// `m_canPathThroughUnits`.
    pub can_path_through_units: bool,
    /// `m_ignoredObstacleID` (`ObjectID`).
    pub ignore_obstacle_id: ObjectId,
    /// `m_desiredSpeed`.
    pub desired_speed: f32,
    /// `m_pathExtraDistance`.
    pub path_extra_distance: f32,
    /// `m_retryPath`.
    pub retry_path: bool,
    /// `AIInternalMoveToState::m_tryOneMoreRepath`.
    pub try_one_more_repath: bool,
    /// `AIInternalMoveToState::m_blockedRepathTimestamp`.
    pub blocked_repath_timestamp: u32,
    /// `AIInternalMoveToState::m_adjustDestinations`.
    pub adjusts_destination: bool,
    /// `m_doFinalPosition` + `m_finalPosition` (`setFinalPosition`).
    pub do_final_position: bool,
    pub final_position: Coord3D,
    /// `m_locomotorGoalType`.
    pub locomotor_goal_type: LocoGoalType,
    /// `m_locomotorGoalData`.
    pub locomotor_goal_data: Coord3D,
    /// `AIInternalMoveToState::m_goalPosition`; `None` when not in a move state.
    pub move_goal: Option<Coord3D>,
    /// The group destination of the pending request (`AIGroup::computeIndividualDestination`).
    pub group_dest: Option<Coord3D>,
    /// `AIInternalMoveToState::m_pathGoalPosition`: the goal the current path was computed for.
    pub path_goal_position: Coord3D,
    /// `m_curMaxBlockedSpeed`.
    pub cur_max_blocked_speed: f32,
    /// `m_bumpSpeedLimit`.
    pub bump_speed_limit: f32,
    /// The raw path the last `computePath` built, for `patchPath` (`m_path`).
    pub raw_path: Option<Path>,
    /// `m_moveOutOfWay1` / `m_moveOutOfWay2` (AIUpdate.cpp:3185). The step-aside's 10-second limit is the temporary
    /// state's expiry (`setTemporaryState`), not a field here.
    pub move_out_of_way: [ObjectId; 2],
    /// `AIUpdateInterface::m_movementComplete`: `doLocomotor` finished its path, so the move state
    /// can report success next frame (`AIUpdate.cpp:1035`).
    pub movement_complete: bool,
    /// `AIUpdateInterface::m_isMoving` (AIUpdate.cpp:273): TRUE from `friend_startingMove` until `friend_endingMove`.
    /// The move states call the pair at their onEnter / computePath / follow-advance and onExit sites (AIStates.cpp
    /// :1635, :1607, :3384, :4170, :4247 and :1747, :1673, :3653). The engine neither xfers nor CRCs it.
    pub is_moving: bool,
    /// `AIUpdateInterface::m_allowedToChase` (AIUpdate.h:799): set on a passenger of an assault transport while it is a
    /// member (AssaultTransportAIUpdate.cpp:231), cleared when it leaves the list (:185, :478). An AI-ordered approach at
    /// a human player's unit is refused unless this is set (AIStates.cpp:2613). Not xferred (AIUpdate.cpp has no xfer for it).
    pub allowed_to_chase: bool,
    /// `AssaultTransportAIUpdate`'s members and orders (`crate::assault_transport`); the default for other AI modules.
    pub assault: crate::assault_transport::AssaultTransportState,
    /// The persistent `AIStateMachine` (`crate::ai_states::AiMachine`).
    pub machine: crate::ai_states::AiMachine,
    /// The dozer's build action (`DozerActionStateMachine`, `crate::dozer::DozerAction`).
    pub dozer_action: crate::dozer::DozerAction,
    /// `Locomotor::m_maintainPos` while `MAINTAIN_POS_IS_VALID`: wings circle this point. Cleared
    /// whenever the locomotor is asked to move (`moveTowardsPosition*`).
    pub maintain_pos: Option<Coord3D>,
    /// A flying path's first node (the position it was computed from) and the last node it was
    /// latched for, for `Path::computeFlightDistToGoal` (`m_cpopRecentStart` is the first node
    /// not yet flown past).
    pub flight_start: Option<Coord3D>,
    pub flight_goal: Option<Coord3D>,
    /// `AIUpdateInterface::m_crateCreated` (`notifyCrate`, set by `CreateCrateDie` for a computer killer).
    pub crate_created: ObjectId,
    /// `AIUpdateInterface::m_isRecruitable` (default TRUE; `setIsRecruitable`).
    pub recruitable: bool,
}

impl Default for AiState {
    fn default() -> Self {
        Self {
            path: Vec::new(),
            speed: 0.0,
            suspended: false,
            attitude: 0,
            attack_info: None,
            requested_destination: Coord3D::ZERO,
            is_final_goal: false,
            request: PathRequest::None,
            attack_path_active: false,
            requested_victim: ObjectId::INVALID,
            waiting_for_path: false,
            move_waiting_for_path: false,
            path_timestamp: 0,
            move_path_timestamp: 0,
            queue_for_path_frame: 0,
            wake_frame: 0,
            blocked_frames: 0,
            is_blocked: false,
            is_blocked_and_stuck: false,
            ignore_collisions_until: 0,
            can_path_through_units: false,
            ignore_obstacle_id: ObjectId::INVALID,
            desired_speed: FAST_AS_POSSIBLE,
            path_extra_distance: 0.0,
            retry_path: false,
            try_one_more_repath: true,
            blocked_repath_timestamp: 0,
            adjusts_destination: true,
            do_final_position: false,
            final_position: Coord3D::ZERO,
            locomotor_goal_type: LocoGoalType::None,
            locomotor_goal_data: Coord3D::ZERO,
            move_goal: None,
            group_dest: None,
            path_goal_position: Coord3D::ZERO,
            cur_max_blocked_speed: FAST_AS_POSSIBLE,
            bump_speed_limit: FAST_AS_POSSIBLE,
            raw_path: None,
            move_out_of_way: [ObjectId::INVALID, ObjectId::INVALID],
            movement_complete: false,
            is_moving: false,
            allowed_to_chase: false,
            assault: crate::assault_transport::AssaultTransportState::default(),
            machine: crate::ai_states::AiMachine::default(),
            dozer_action: crate::dozer::DozerAction::default(),
            maintain_pos: None,
            flight_start: None,
            flight_goal: None,
            crate_created: ObjectId::INVALID,
            recruitable: true,
        }
    }
}

impl AiState {
    /// `AIUpdateInterface::setLocomotorGoalPositionOnPath` (`AIUpdate.cpp:2450-2454`): the goal type
    /// and a **zeroed** `m_locomotorGoalData` (the C++ setter calls `m_locomotorGoalData.zero()`).
    pub fn set_locomotor_goal_position_on_path(&mut self) {
        self.locomotor_goal_type = LocoGoalType::PositionOnPath;
        self.locomotor_goal_data = Coord3D::ZERO;
    }

    /// `AIUpdateInterface::setLocomotorGoalPositionExplicit` (`AIUpdate.cpp:2461-2466`): the goal
    /// type and the full position.
    pub fn set_locomotor_goal_explicit(&mut self, pos: Coord3D) {
        self.locomotor_goal_type = LocoGoalType::PositionExplicit;
        self.locomotor_goal_data = pos;
    }

    /// `AIUpdateInterface::setLocomotorGoalOrientation` (`AIUpdate.cpp:2478-2483`): the goal type and
    /// only `m_locomotorGoalData.x`; y and z keep their previous values.
    pub fn set_locomotor_goal_orientation(&mut self, angle: f32) {
        self.locomotor_goal_type = LocoGoalType::Angle;
        self.locomotor_goal_data.x = angle;
    }

    /// `AIUpdateInterface::setLocomotorGoalNone` (`AIUpdate.cpp:2486-2489`): the goal type only; the
    /// goal data is left as it was.
    pub fn set_locomotor_goal_none(&mut self) {
        self.locomotor_goal_type = LocoGoalType::None;
    }

    /// A locomotor-only move: the path is installed and the locomotor goal set, with no state entry and no
    /// `friend_startingMove`. Its only caller is combat's approach fallback for a unit with no pathfinder object
    /// (combat.rs), which has no C++ counterpart: the engine always queues `requestAttackPath` there. Every
    /// `aiMoveToPosition` caller uses [`GameLogic::ai_move_to_position_cmd`] instead (the `AI_MOVE_TO` state).
    pub fn move_to(&mut self, from: &Coord3D, to: &Coord3D, terrain: &TerrainLogic) {
        let _ = (from, terrain);
        self.path = compute_path(from, to, terrain);
        self.raw_path = None;
        self.start_move(*to, PathRequest::Move, /*adjusts=*/ false);
        // `compute_path` installs the path here (no pathfinder request), so this is `computePath`'s
        // `if (theNewPath) ... setLocomotorGoalPositionOnPath()` (`AIUpdate.cpp:1740-1745`), not the
        // `requestPath` that leaves the goal alone.
        if !self.path.is_empty() {
            self.set_locomotor_goal_position_on_path();
        }
    }

    /// `aiMoveToPosition` through the async pathfinder (`AIUpdateInterface::requestPath`):
    /// `computePath`. An empty result leaves the unit where it is.
    pub fn move_to_pathfound(&mut self, pf: &mut Pathfinder, obj: &PathObject, from: &Coord3D, to: &Coord3D, terrain: &TerrainLogic) {
        let _ = (from, terrain);
        self.start_move(*to, PathRequest::Move, /*adjusts=*/ true);
        self.queue_path(pf, obj.id);
    }

    /// Like [`move_to_pathfound`](Self::move_to_pathfound) for a unit in a group move: the
    /// pathfinder tightens the goal toward `group_dest` (`AIGroup::computeIndividualDestination`).
    pub fn move_to_pathfound_group(&mut self, pf: &mut Pathfinder, obj: &PathObject, from: &Coord3D, to: &Coord3D, terrain: &TerrainLogic, group_dest: Option<Coord3D>) {
        let _ = (from, terrain);
        self.start_move(*to, PathRequest::Move, /*adjusts=*/ true);
        self.group_dest = group_dest;
        self.queue_path(pf, obj.id);
    }

    /// `AIInternalMoveToState::computePath`'s setup: record the goal and flags of a new move. The
    /// engine's `Locomotor::startMove` (the donut-timer reset, `Locomotor.cpp:782-786`) is called from
    /// the surrounding `onEnter` (`AIStates.cpp:1627`) and ported as `GameLogic::loco_start_move` there.
    pub(crate) fn start_move(&mut self, goal: Coord3D, request: PathRequest, adjusts: bool) {
        self.flight_goal = None; // a new path starts where the unit is now
        self.move_goal = Some(goal);
        self.requested_destination = goal;
        self.request = request;
        self.group_dest = None;
        self.is_final_goal = adjusts;
        self.adjusts_destination = adjusts;
        self.requested_victim = ObjectId::INVALID;
        self.waiting_for_path = request != PathRequest::None || self.waiting_for_path;
        self.retry_path = false;
        self.try_one_more_repath = true;
        self.is_blocked_and_stuck = false;
        self.blocked_frames = 0;
        // `AIUpdateInterface::requestPath` (`AIUpdate.cpp:480-521`) leaves `m_locomotorGoalType` alone:
        // only a computed path sets `POSITION_ON_PATH` (`computePath`'s `if (theNewPath)`,
        // `AIUpdate.cpp:1744-1745`) and `AIInternalMoveToState::update` re-sets it each frame while a
        // path exists (`AIStates.cpp:1875-1876`). That is what scrubs a stopped unit's velocity while
        // its new path is still queued: `doLocomotor`'s `NONE` branch maintains position.
        self.movement_complete = false;
    }

    /// `Pathfinder::queueForPath` (`AIPathfind.cpp:5761`).
    fn queue_path(&mut self, pf: &mut Pathfinder, id: ObjectId) {
        if id != ObjectId::INVALID {
            self.waiting_for_path = true;
            pf.queue_for_path(id);
        }
    }

    /// `requestAttackPath` (`AIUpdate.cpp:520-540`): queue a `findAttackPath`. It stores the victim and destination only; the
    /// weapon's range, the radii and the line-of-sight flag are read when `computeAttackPath` runs (in `doPathfind`).
    pub fn request_attack_path(&mut self, pf: &mut Pathfinder, id: ObjectId, victim: ObjectId, victim_pos: Coord3D, frame: u32) -> Option<u32> {
        self.requested_victim = victim;
        self.requested_destination = victim_pos;
        self.request = PathRequest::Attack;
        // `requestAttackPath` sets `m_isAttackPath = TRUE` (AIUpdate.cpp:528).
        self.attack_path_active = true;
        self.move_goal = Some(victim_pos);
        self.waiting_for_path = true;
        // `requestAttackPath` (AIUpdate.cpp:532-539): a path computed within the last 3 frames waits two seconds
        // (`setQueueForPathTime(2*LOGICFRAMES_PER_SECOND)`) and clears the locomotor goal, instead of queueing now.
        // `frame-3` wraps in C++ for the first frames, which leaves the test false there.
        if frame >= 3 && self.path_timestamp > frame - 3 {
            self.queue_for_path_frame = frame + 2 * crate::math::LOGICFRAMES_PER_SECOND;
            self.set_locomotor_goal_none();
            // The caller applies `setQueueForPathTime`'s wake (`wake_up_for_queue_time`, AIUpdate.cpp:955-968): the
            // queue time it set is returned, since the module's heap slot lives on the game, not on this state.
            return Some(2 * crate::math::LOGICFRAMES_PER_SECOND);
        }
        pf.queue_for_path(id);
        None
    }

    /// `requestApproachPath` (`AIUpdate.cpp:543`).
    pub fn request_approach_path(&mut self, pf: &mut Pathfinder, id: ObjectId, dest: Coord3D) {
        self.requested_destination = dest;
        self.request = PathRequest::Approach;
        // `requestApproachPath` sets `m_isAttackPath = FALSE` (AIUpdate.cpp:550).
        self.attack_path_active = false;
        self.move_goal = Some(dest);
        self.is_final_goal = true;
        self.waiting_for_path = true;
        pf.queue_for_path(id);
    }

    /// `aiIdle`: drop path and goals.
    pub fn stop(&mut self) {
        self.flight_goal = None;
        self.path.clear();
        self.raw_path = None;
        self.waiting_for_path = false;
        self.request = PathRequest::None;
        self.requested_victim = ObjectId::INVALID;
        self.move_goal = None;
        self.set_locomotor_goal_none();
        self.is_blocked_and_stuck = false;
        self.blocked_frames = 0;
    }

    /// `AIUpdateInterface::getPath() != nullptr`: a `Path` exists. The engine keeps the `Path` object
    /// (a finished move keeps it until its state exits, `AIUpdate.cpp:1035`), and a path whose only
    /// node is its start node is still a path (`Path::getLastNode()` returns it) even though the port's
    /// waypoint list (`ai.path`, the nodes after the start node) is empty. Every `getPath() != nullptr`
    /// / `thePath==nullptr` test in the move states reads this, not the waypoint list.
    pub fn has_path(&self) -> bool {
        self.raw_path.is_some() || !self.path.is_empty()
    }

    /// `AIUpdateInterface::isMoving` (AIUpdate.cpp:3194-3206) for an AI with no subclass `isIdle` veto. Readers that
    /// hold the object use [`crate::object::Object::is_ai_moving`], which applies the vetoes of the virtual `isIdle`.
    pub fn is_moving(&self) -> bool {
        self.is_moving_with_idle_veto(false)
    }

    /// `AIUpdateInterface::isMoving`: `if (isIdle()) return false; if (m_locomotorGoalType != NONE) return TRUE; if
    /// (m_isMoving) return TRUE; return FALSE;`. `isIdle` is the main machine in `AI_IDLE` (AIUpdate.cpp:3120-3127) and
    /// `vetoed` is the subclass override's answer (`Object::ai_idle_vetoed`), which `isIdle` virtually dispatches to.
    pub(crate) fn is_moving_with_idle_veto(&self, vetoed: bool) -> bool {
        if !vetoed && self.machine.current == crate::ai_states::AiStateType::Idle {
            return false;
        }
        if self.locomotor_goal_type != LocoGoalType::None {
            return true;
        }
        self.is_moving
    }

    /// `AIUpdateInterface::friend_startingMove` (AIUpdate.cpp:2034-2040): a move state begins or repaths
    /// (`AIInternalMoveToState::onEnter` AIStates.cpp:1635, `computePath` :1607, the follow-path advances :3384,
    /// :4170, :4247). It also clears the blocked state that the move is about to retry.
    pub(crate) fn friend_starting_move(&mut self) {
        self.movement_complete = false;
        self.is_moving = true;
        self.blocked_frames = 0;
        self.is_blocked_and_stuck = false;
    }

    /// `AIUpdateInterface::friend_endingMove` (AIUpdate.cpp:2046-2050): the move state's exit (AIStates.cpp:1747,
    /// `AIInternalMoveToState::onExit`), its failed `computePath` (:1673) and the attack move's hand-off to an attack
    /// (:3653). `m_movementComplete` is what the update's completed-move block reads (AIUpdate.cpp:1035).
    pub(crate) fn friend_ending_move(&mut self) {
        self.movement_complete = true;
        self.is_moving = false;
    }

    pub fn goal(&self) -> Option<Coord3D> {
        self.path.last().copied().or(self.move_goal)
    }

    pub(crate) fn crc(&self, c: &mut Crc) {
        c.add_u32(self.path.len() as u32);
        for p in &self.path {
            c.add_coord(p);
        }
        c.add_f32(self.speed);
        c.add_bool(self.suspended);
        c.add_u32(self.attitude as u32);
        // Movement state (`AIUpdateInterface`).
        c.add_coord(&self.requested_destination);
        c.add_bool(self.is_final_goal);
        c.add_u32(self.request as u32);
        c.add_u32(self.requested_victim.0);
        c.add_bool(self.waiting_for_path);
        c.add_bool(self.move_waiting_for_path);
        c.add_u32(self.path_timestamp);
        c.add_u32(self.move_path_timestamp);
        c.add_u32(self.queue_for_path_frame);
        c.add_u32(self.wake_frame);
        c.add_u32(u32::from(self.blocked_frames));
        c.add_bool(self.is_blocked);
        c.add_bool(self.is_blocked_and_stuck);
        c.add_u32(self.ignore_collisions_until);
        c.add_bool(self.can_path_through_units);
        c.add_u32(self.ignore_obstacle_id.0);
        c.add_f32(self.desired_speed);
        c.add_f32(self.path_extra_distance);
        c.add_bool(self.retry_path);
        c.add_bool(self.try_one_more_repath);
        c.add_u32(self.blocked_repath_timestamp);
        c.add_bool(self.adjusts_destination);
        c.add_bool(self.do_final_position);
        c.add_coord(&self.final_position);
        c.add_u32(self.locomotor_goal_type as u32);
        c.add_coord(&self.locomotor_goal_data);
        match self.move_goal {
            Some(g) => {
                c.add_bool(true);
                c.add_coord(&g);
            }
            None => c.add_bool(false),
        }
        match self.group_dest {
            Some(g) => {
                c.add_bool(true);
                c.add_coord(&g);
            }
            None => c.add_bool(false),
        }
        c.add_coord(&self.path_goal_position);
        c.add_f32(self.cur_max_blocked_speed);
        c.add_f32(self.bump_speed_limit);
        match &self.raw_path {
            Some(p) => {
                c.add_bool(true);
                c.add_u32(p.nodes.len() as u32);
                for n in &p.nodes {
                    c.add_coord(&n.pos);
                    c.add_bool(n.can_optimize);
                    c.add_u32(n.next_optimized.map_or(u32::MAX, |x| x as u32));
                }
                c.add_bool(p.blocked_by_ally);
            }
            None => c.add_bool(false),
        }
        c.add_u32(self.move_out_of_way[0].0);
        c.add_u32(self.move_out_of_way[1].0);
        c.add_bool(self.movement_complete);
        self.machine.crc(c);
        // Only hashed while a build action is live, so units that never build keep their historical CRC.
        if self.dozer_action != crate::dozer::DozerAction::default() {
            c.add_u32(self.dozer_action.sub as u32);
            c.add_u32(self.dozer_action.target.0);
            c.add_coord(&self.dozer_action.dock);
            c.add_coord(&self.dozer_action.end);
        }
        // Only hashed when set, so aircraft that never circle keep their historical CRC.
        if let Some(p) = self.maintain_pos {
            c.add_bool(true);
            c.add_coord(&p);
        }
        if let Some(p) = self.flight_start {
            c.add_bool(true);
            c.add_coord(&p);
        }
        if !self.recruitable {
            c.add_bool(false);
        }
        if let Some(n) = &self.attack_info {
            c.add_str(n);
        }
        if self.crate_created != ObjectId::INVALID {
            c.add_u32(self.crate_created.0);
        }
    }
}

/// The `TAiData` (`AIData` INI block) tunables the target-selection code reads (`AI.cpp:159-169`).
/// The original's constructor zeroes them and `AIData.ini` always sets them; the defaults here are the
/// retail `Data\INI\Default\AIData.ini` values so a sim built without the INI behaves like retail.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AiTunables {
    /// `AlertRangeModifier`: scan-range factor for an Alert computer-controlled unit.
    pub alert_range_modifier: f32,
    /// `AggressiveRangeModifier`: scan-range factor for an Aggressive computer-controlled unit.
    pub aggressive_range_modifier: f32,
    /// `GuardOuterModifierAI` / `GuardOuterModifierHuman`: the owner-type factor of
    /// `AI::getAdjustedVisionRangeForObject(AI_VISIONFACTOR_OWNERTYPE)` for a non-inner scan.
    pub guard_outer_modifier_ai: f32,
    pub guard_outer_modifier_human: f32,
    /// `GuardInnerModifierAI` / `GuardInnerModifierHuman`: the same factor with
    /// `AI_VISIONFACTOR_GUARDINNER` (`AIGuardMachine::getStdGuardRange`, `AI.cpp:836-846`).
    pub guard_inner_modifier_ai: f32,
    pub guard_inner_modifier_human: f32,
    /// `AttackPriorityDistanceModifier`: distance that lowers an attack priority by one.
    pub attack_priority_distance_modifier: f32,
    /// `GuardChaseUnitsDuration` -> `m_guardChaseUnitFrames` (`AI.cpp:151`): how long a guarding unit
    /// chases attackers before giving up (`AIGuard.cpp:555`). Retail `AIData.ini`: 10000 ms.
    pub guard_chase_unit_frames: u32,
    /// `GuardEnemyScanRate` -> `m_guardEnemyScanRate` (`AI.cpp:929`, default `LOGICFRAMES_PER_SECOND/2`):
    /// how often `AIGuardRetaliateIdleState` scans for a target (`AIGuardRetaliate.cpp:675,689`). Retail
    /// `AIData.ini`: 500 ms.
    pub guard_enemy_scan_rate: u32,
    /// `GuardEnemyReturnScanRate` -> `m_guardEnemyReturnScanRate` (`AI.cpp:930`, default
    /// `LOGICFRAMES_PER_SECOND`): the scan rate of `AIGuardRetaliateReturnState`
    /// (`AIGuardRetaliate.cpp:603,626`). Retail `AIData.ini`: 1000 ms.
    pub guard_enemy_return_scan_rate: u32,
    /// `AIDozerBoredRadiusModifier` (`AI.h:224`): an AI dozer scans this many times farther for
    /// damaged buildings to auto-repair (`DozerAIUpdate::getBoredRange`, `DozerAIUpdate.cpp:2432`).
    /// Retail `AIData.ini`: 2.0.
    pub ai_dozer_bored_radius_modifier: f32,
    /// `ForceIdleMSEC` -> `m_forceIdleFramesCount` (`AI.cpp:141`, default 1 at `AI.cpp:920`): how far
    /// `resetNextMoodCheckTime` (`AIUpdate.cpp:4507`, from `AIIdleState::onEnter`) pushes the next mood check.
    pub force_idle_frames: u32,
    /// `AttackUsesLineOfSight` -> `m_attackUsesLineOfSight` (`AI.cpp:165`, default `TRUE` at `AI.cpp:941`):
    /// whether a scan of a unit with `KINDOF_ATTACK_NEEDS_LINE_OF_SIGHT` adds `AI::CAN_SEE`, i.e. the
    /// `PartitionFilterLineOfSight` (`AIUpdate.cpp:4648-4655`).
    pub attack_uses_line_of_sight: bool,
}

impl Default for AiTunables {
    fn default() -> Self {
        AiTunables {
            alert_range_modifier: 1.1,
            aggressive_range_modifier: 1.5,
            guard_outer_modifier_ai: 1.333,
            guard_outer_modifier_human: 2.2,
            guard_inner_modifier_ai: 1.1,
            guard_inner_modifier_human: 1.8,
            attack_priority_distance_modifier: 100.0,
            guard_chase_unit_frames: 10 * crate::math::LOGICFRAMES_PER_SECOND,
            // `AIData.ini` 500 ms / 1000 ms (`INI::parseDurationUnsignedInt` rounds up), which equal the
            // engine's constructor defaults (`AI.cpp:929-930`).
            guard_enemy_scan_rate: crate::math::LOGICFRAMES_PER_SECOND / 2,
            guard_enemy_return_scan_rate: crate::math::LOGICFRAMES_PER_SECOND,
            ai_dozer_bored_radius_modifier: 2.0,
            force_idle_frames: 1,
            attack_uses_line_of_sight: true,
        }
    }
}

/// `AIUpdateInterface` used to live here; the state machine now runs per frame from
/// [`GameLogic::update_ai_state_machines`] (`crate::ai_states`). The module is kept as the object's
/// AI marker (`o.ai_handle`) and sleeps forever.
pub struct AiUpdate;

impl UpdateModule for AiUpdate {
    fn update(&mut self, _ctx: &mut UpdateContext) -> UpdateSleepTime {
        UPDATE_SLEEP_FOREVER
    }

    fn name(&self) -> &'static str {
        "AiUpdate"
    }
}

/// `isSamePosition(ourPos, prevTargetPos, curTargetPos)` (`AIStates.cpp:203-224`): whether the goal moved since
/// the path was built, by more than `(dist/10)` squared. Only the 2D part is compared.
pub(crate) fn is_same_position(pos: Coord3D, prev_target_pos: Coord3D, cur_target_pos: Coord3D) -> bool {
    let diffx = cur_target_pos.x - prev_target_pos.x;
    let diffy = cur_target_pos.y - prev_target_pos.y;
    let to_x = cur_target_pos.x - pos.x;
    let to_y = cur_target_pos.y - pos.y;
    let tolerance_sqr = (to_x * to_x + to_y * to_y) * (1.0 / (10.0 * 10.0));
    diffx * diffx + diffy * diffy <= tolerance_sqr
}

impl GameLogic {
    /// `AIUpdateInterface::doLocomotor` (`AIUpdate.cpp:2127`): the blocked-frame bookkeeping and the
    /// per-frame movement, plus the deferred `m_queueForPathFrame` request (`AIUpdate.cpp:1064`).
    pub(crate) fn ai_do_locomotor(&mut self, id: ObjectId) {
        // `if (getObject()->isKindOf(KINDOF_IMMOBILE)) return UPDATE_SLEEP_FOREVER;` (`AIUpdate.cpp:2131`).
        let Some(loco) = self.locomotor_now(id) else { return };
        if self.objects.get(&id).is_some_and(|o| o.is_kind_of(crate::template::kind_of::IMMOBILE)) {
            return;
        }
        // `if (!getPath()) { if (m_waitingForPath) return UPDATE_SLEEP_FOREVER; }` (`AIUpdate.cpp:2177-2179`):
        // a POSITION_ON_PATH move waiting for its path returns before the status update.
        let waiting = self
            .objects
            .get(&id)
            .is_some_and(|o| o.ai.path.is_empty() && o.ai.waiting_for_path && o.ai.locomotor_goal_type == LocoGoalType::PositionOnPath);
        self.ai_do_locomotor_move(id, &loco);
        if !waiting {
            self.update_airborne_target_status(id, &loco);
        }
    }

    /// The closing `OBJECT_STATUS_AIRBORNE_TARGET` update of `doLocomotor` (`AIUpdate.cpp:2294-2300`), after the
    /// frame's movement: set while `getHeightAboveTerrain() > getAirborneTargetingHeight()` (`Thing.cpp:293-311`:
    /// position z less `getGroundHeight`), clear otherwise. `Object::isAirborneTarget` (`Object.h:211`) reads it.
    fn update_airborne_target_status(&mut self, id: ObjectId, loco: &LocomotorParams) {
        let Some(o) = self.objects.get(&id) else { return };
        // `Object::calculateHeightAboveTerrain` (`Object.cpp:2820-2825`) measures from the surface of the object's
        // layer, so a unit on a bridge deck is measured from the deck. The surface helper is the one
        // `significantly_above_terrain` (`combat.rs`) uses.
        let height = o.position.z - self.terrain.surface_height(o.position.x, o.position.y, o.position.z);
        let airborne = height > loco.airborne_targeting_height;
        if let Some(o) = self.objects.get_mut(&id) {
            if airborne {
                o.status_bits |= crate::object::status::AIRBORNE_TARGET;
            } else {
                o.status_bits &= !crate::object::status::AIRBORNE_TARGET;
            }
        }
    }

    /// The movement half of `doLocomotor` for `loco` (the locomotor with the body's damage state applied).
    fn ai_do_locomotor_move(&mut self, id: ObjectId, loco: &LocomotorParams) {
        let now = self.frame;
        // No disabled-type check here: `doLocomotor` has none (AIUpdate.cpp:2127-2301). A unit disabled by a type
        // other than HELD is deferred by the heap gate before this runs (`run_sleepy_updates_with`). Lane 165.
        // `m_queueForPathFrame` (`AIUpdate.cpp:1064-1078`): a deferred path request.
        let (queue_frame, has_goal) = match self.objects.get(&id) {
            Some(o) => (o.ai.queue_for_path_frame, o.ai.move_goal.is_some()),
            None => return,
        };
        if queue_frame != 0 {
            if now >= queue_frame {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.queue_for_path_frame = 0;
                }
                if has_goal {
                    self.pathfinder.queue_for_path(id);
                }
            } else {
                let flight = self.ai_flight_goal(id, loco);
                let look = if flight.is_some() { flight } else { self.ai_goal_on_path(id, loco) };
                let Some(blocked) = do_locomotor_obj(self.objects.get_mut(&id).expect("exists"), loco, &self.terrain, &self.pathfinder, now, self.rules.gravity, look, flight.is_some()) else {
                    return;
                };
                finish_locomotor_frame(self.objects.get_mut(&id).expect("exists"), blocked);
                return;
            }
        }
        let flight = self.ai_flight_goal(id, loco);
        let look = if flight.is_some() { flight } else { self.ai_goal_on_path(id, loco) };
        // `None`: the move is still waiting for its path, and `doLocomotor` returns before its tail (`AIUpdate.cpp:2177-2179`).
        let Some(blocked) = do_locomotor_obj(self.objects.get_mut(&id).expect("exists"), loco, &self.terrain, &self.pathfinder, now, self.rules.gravity, look, flight.is_some()) else {
            return;
        };
        finish_locomotor_frame(self.objects.get_mut(&id).expect("exists"), blocked);
    }

    /// `AIUpdateInterface::isDoingGroundMovement` (`AIUpdate.cpp:2359`): air-only locomotors are not
    /// ground movement, and neither is a `JetAIUpdate` ("Air units should never be doing ground
    /// movement, even when taxiing", `JetAIUpdate.cpp:2419`). An unmanned helicopter is grounded, so it
    /// is ground movement (`AIUpdate.cpp:2362`). A held unit and an airborne unit that is allowed to
    /// fall are not (`:2385`, `:2393`).
    pub(crate) fn is_doing_ground_movement(&self, id: ObjectId) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        if o.template.jet.is_some() {
            return false;
        }
        if o.is_disabled_by_type(DisabledType::Unmanned) && o.template.is_collide_kind_of(collide_kind::PRODUCED_AT_HELIPAD) {
            return true;
        }
        if !o.locomotor().is_some_and(|l| !l.is_airborne()) {
            return false;
        }
        if o.is_disabled_by_type(DisabledType::Held) {
            return false;
        }
        // `Object::isAboveTerrain` with `PhysicsBehavior::getAllowToFall`.
        let ground = self.terrain.surface_height(o.position.x, o.position.y, o.position.z);
        let above_terrain = o.position.z > ground + 0.001;
        if above_terrain && o.physics.as_ref().is_some_and(|p| p.allow_to_fall) {
            return false;
        }
        true
    }

    /// `AIUpdateInterface::getTreatAsAircraftForLocoDistToGoal` (`AIUpdate.cpp:2432`) and the
    /// `JetAIUpdate` override (`JetAIUpdate.cpp:2427`): a taxiing jet measures the ground way. Any other
    /// unit that is not doing ground movement, a hover, or a path with extra distance beyond
    /// `PATHFIND_CLOSE_ENOUGH` uses the flight distance.
    fn treat_as_aircraft_for_loco_dist(&self, id: ObjectId, loco: &LocomotorParams) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        if o.template.jet.is_some() && o.jet.flag(crate::jet_ai::jet_flag::TAXI_IN_PROGRESS) {
            return false;
        }
        !self.is_doing_ground_movement(id) || o.ai.path_extra_distance > PATHFIND_CLOSE_ENOUGH || loco.kind == LocomotorKind::Hover
    }

    /// `AIUpdateInterface::getLocomotorDistanceToGoal` (`AIUpdate.cpp:2449`) for the move state's goal.
    /// Only a `POSITION_ON_PATH` goal with a path and a current locomotor measures anything; every other
    /// case is "already there" (0). The 3D branch (`CloseEnoughDist3D` / projectile) is the straight
    /// 3D distance to the last node. Aircraft use `computeFlightDistToGoal`; ground units use the
    /// `distAlongPath` of `computePointOnPath` on the raw path. Either way the straight distance to the
    /// last node is returned when the path distance is under a cell or shorter than it, so a unit is
    /// never "there" while it is still farther than its path says.
    pub(crate) fn locomotor_distance_to_goal(&self, id: ObjectId) -> f32 {
        let Some(o) = self.objects.get(&id) else { return 0.0 };
        if o.ai.locomotor_goal_type != LocoGoalType::PositionOnPath {
            return 0.0;
        }
        let Some(&last) = o.ai.path.last() else { return 0.0 };
        let Some(loco) = o.locomotor() else { return 0.0 };
        // `m_curLocomotor->isCloseEnoughDist3D() || getObject()->isKindOf(KINDOF_PROJECTILE)` (`AIUpdate.cpp:2468`).
        if loco.close_enough_dist_3d || o.is_kind_of(crate::template::kind_of::PROJECTILE) {
            let (dx, dy, dz) = (last.x - o.position.x, last.y - o.position.y, last.z - o.position.z);
            return sqrt(dx * dx + dy * dy + dz * dz);
        }
        let (dx, dy) = (last.x - o.position.x, last.y - o.position.y);
        let dist_sqr = dx * dx + dy * dy;
        if self.treat_as_aircraft_for_loco_dist(id, &loco) {
            // `Path::computeFlightDistToGoal`, from the latched first node not yet flown past.
            let start = flight_path_start(o, &last);
            let (dist, _, _) = flight_dist_to_goal(&start, &o.ai.path, &o.position);
            // "If the actual distance is farther, then use the actual distance so we get there."
            return if dist * dist > dist_sqr { sqrt(dist_sqr) } else { dist };
        }
        // `Path::computePointOnPath` on the raw path (`m_path`), the same chain doLocomotor follows. The
        // raw path must still describe the waypoints; without one the path distance is unknown (0), so
        // the straight distance decides (a port-only approximation for patched or script paths).
        let dist = o
            .ai
            .raw_path
            .as_ref()
            .filter(|raw| raw.optimized_positions().last() == o.ai.path.last())
            .and_then(|raw| {
                let po = PathObject::for_object(o)?;
                raw.compute_point_on_path(&self.pathfinder, &po, &o.position)
            })
            .map_or(0.0, |info| info.dist_along_path);
        if dist < PATHFIND_CELL_SIZE_F || dist * dist < dist_sqr {
            sqrt(dist_sqr)
        } else {
            dist
        }
    }

    /// `doLocomotor`'s flying branch (`AIUpdate.cpp:2190`): for a unit that is not doing ground
    /// movement `Path::computeFlightDistToGoal` gives the node to steer toward and the distance to go
    /// (the projection of the way to each node onto its segment, zero once the unit has flown past
    /// the last one). Nodes already flown past are dropped from `ai.path`.
    fn ai_flight_goal(&mut self, id: ObjectId, loco: &LocomotorParams) -> Option<(Coord3D, f32)> {
        let o = self.objects.get(&id)?;
        if o.physics.is_none() || !crate::locomotor::is_force_driven(loco) || o.ai.path.is_empty() || self.is_doing_ground_movement(id) {
            return None;
        }
        let pos = o.position;
        let last = *o.ai.path.last()?;
        // The path's first node is "where the path was computed from": latched the first time a path
        // to this goal is seen.
        let start = flight_path_start(o, &last);
        let (dist, goal, passed) = flight_dist_to_goal(&start, &o.ai.path, &pos);
        let o = self.objects.get_mut(&id)?;
        apply_flight_latch(o, last, start, passed);
        Some((goal, dist))
    }

    /// The flight distance query's latch (`getLocomotorDistanceToGoal`, AIUpdate.cpp:2481-2486): the airborne
    /// POSITION_ON_PATH branch calls `Path::computeFlightDistToGoal`, which advances `m_cpopRecentStart` past the
    /// segments the unit has flown past, as the steer does (`ai_flight_goal`). The arrival test runs this first.
    /// The 3D and projectile branches (AIUpdate.cpp:2468) do not call it.
    pub(crate) fn flight_distance_latch(&mut self, id: ObjectId) {
        let Some(o) = self.objects.get(&id) else { return };
        if o.ai.locomotor_goal_type != LocoGoalType::PositionOnPath || !o.ai.has_path() {
            return;
        }
        let Some(loco) = o.locomotor() else { return };
        if loco.close_enough_dist_3d || o.is_kind_of(crate::template::kind_of::PROJECTILE) || !self.treat_as_aircraft_for_loco_dist(id, &loco) {
            return;
        }
        let Some(o) = self.objects.get(&id) else { return };
        let Some(&last) = o.ai.path.last() else { return };
        let start = flight_path_start(o, &last);
        let (_, _, passed) = flight_dist_to_goal(&start, &o.ai.path, &o.position);
        if let Some(o) = self.objects.get_mut(&id) {
            apply_flight_latch(o, last, start, passed);
        }
    }

    /// `doLocomotor`'s `POSITION_ON_PATH` goal for a ground, force-driven unit
    /// (`AIUpdate.cpp:2188`): `Path::computePointOnPath` gives the point to steer toward and the
    /// distance still to go along the path. The waypoints behind the closest segment are dropped so
    /// `ai.path` stays "the nodes ahead". Other units keep chasing the next waypoint.
    fn ai_goal_on_path(&mut self, id: ObjectId, loco: &LocomotorParams) -> Option<(Coord3D, f32)> {
        let o = self.objects.get(&id)?;
        // `getPath() == nullptr` returns None: `doLocomotor`'s `POSITION_ON_PATH` branch
        // (`AIUpdate.cpp:2177-2189`) needs a `Path`, whose only node may be its start node.
        if loco.is_airborne() || o.physics.is_none() || !crate::locomotor::is_force_driven(loco) || !o.ai.has_path() {
            return None;
        }
        let raw = o.ai.raw_path.as_ref()?;
        let positions = raw.optimized_positions();
        // `computePointOnPath` with no segment (a path whose only node is its start node): the point on
        // the path is the last node (`out.posOnPath = *getLastNode()->getPosition()`,
        // `AIPathfind.cpp:767-768`) at distance 0, so the locomotor is asked to stop where it is.
        if positions.len() == 1 {
            return Some((positions[0], 0.0));
        }
        // The raw path must still describe the waypoints (a patched/straight path does not).
        if positions.last() != o.ai.path.last() {
            return None;
        }
        let po = PathObject::for_object(o)?;
        let info = raw.compute_point_on_path(&self.pathfinder, &po, &o.position)?;
        let ahead: Vec<Coord3D> = raw.optimized_positions().split_off(info.next_index);
        let o = self.objects.get_mut(&id)?;
        if !ahead.is_empty() {
            o.ai.path = ahead;
        }
        Some((info.pos_on_path, info.dist_along_path))
    }
}

/// The start of `Path::computeFlightDistToGoal` (`AIPathfind.cpp:1018-1027`): the node the path is latched on
/// (`m_cpopRecentStart`, the first node not yet flown past), else the path's first node (`getFirstNode`). It is not
/// the unit's position: the first segment runs from the path's first node to the next one.
fn flight_path_start(o: &crate::object::Object, last: &Coord3D) -> Coord3D {
    if o.ai.flight_goal == Some(*last) {
        if let Some(p) = o.ai.flight_start {
            return p;
        }
    }
    o.ai.raw_path.as_ref().and_then(|raw| raw.nodes.first().map(|n| n.pos)).unwrap_or(o.position)
}

/// `m_cpopRecentStart` after a `computeFlightDistToGoal` pass (AIPathfind.cpp:1028-1065): the latch is the node
/// the unit stopped on (`passed` leading nodes flown past), and the nodes behind it are dropped. A latch on the
/// last node keeps that node as the path, so later passes see the same flight line (none left: distance 0).
fn apply_flight_latch(o: &mut crate::object::Object, last: Coord3D, start: Coord3D, passed: usize) {
    if o.ai.flight_goal != Some(last) {
        o.ai.flight_goal = Some(last);
        o.ai.flight_start = Some(start);
    }
    if passed > 0 && !o.ai.path.is_empty() {
        let keep = passed.min(o.ai.path.len() - 1);
        o.ai.flight_start = Some(o.ai.path[passed - 1]);
        o.ai.path.drain(..keep);
    }
}

/// `Path::computeFlightDistToGoal` (`AIPathfind.cpp:1018`) over the nodes `start, nodes...`: the
/// summed projection of the way from `pos` to each segment end onto that segment (a segment already
/// flown past adds nothing), the first node not yet flown past (the last node when all are past)
/// and how many leading `nodes` were flown past.
pub(crate) fn flight_dist_to_goal(start: &Coord3D, nodes: &[Coord3D], pos: &Coord3D) -> (f32, Coord3D, usize) {
    let mut cur = *start;
    let mut distance = 0.0f32;
    let mut use_next = true;
    let mut goal = *start;
    let mut passed = 0usize;
    for (i, next) in nodes.iter().enumerate() {
        if use_next {
            goal = *next;
        }
        let (px, py) = (next.x - pos.x, next.y - pos.y);
        let (mut vx, mut vy) = (next.x - cur.x, next.y - cur.y);
        let len = sqrt(vx * vx + vy * vy);
        if len > 0.0 {
            vx /= len;
            vy /= len;
        }
        let dot = px * vx + py * vy;
        if dot >= 0.0 {
            distance += dot;
            use_next = false;
        } else if use_next {
            passed = i + 1;
        }
        cur = *next;
    }
    (distance, goal, passed)
}

/// `AIUpdateInterface::doLocomotor` body (`AIUpdate.cpp:2127`). `flight` marks a flying path
/// ([`flight_dist_to_goal`]): the unit never "arrives" at a node, the move state ends once it has
/// flown past the last one.
///
/// Returns the `blocked` flag after the goal's move: the kernel's output for the force-driven goals
/// (`locoUpdate_moveTowardsPosition(..., &blocked)`, `AIUpdate.cpp:2240`), the entry value for the others.
/// The caller runs the tail of `doLocomotor` on it ([`finish_locomotor_frame`]). `None` is the
/// `UPDATE_SLEEP_FOREVER` return of a move still waiting for its path, which skips that tail.
#[allow(clippy::too_many_arguments)]
fn do_locomotor_obj(
    obj: &mut Object,
    loco: &LocomotorParams,
    terrain: &TerrainLogic,
    pathfinder: &crate::pathfind::Pathfinder,
    now: u32,
    gravity: f32,
    look: Option<(Coord3D, f32)>,
    flight: bool,
) -> Option<bool> {
    // `doLocomotor`: `m_isBlocked` was set by `processCollision` during the previous frame.
    if obj.ai.is_blocked {
        obj.ai.blocked_frames = obj.ai.blocked_frames.saturating_add(1);
    } else {
        obj.ai.blocked_frames = 0;
    }
    obj.ai.is_blocked = false;
    // `Bool blocked = m_blockedFrames > 0;` (`AIUpdate.cpp:2146`).
    let mut blocked = obj.ai.blocked_frames > 0;
    let force_driven = obj.physics.is_some() && crate::locomotor::is_force_driven(loco);
    // `doLocomotor`'s `POSITION_EXPLICIT` / `ANGLE` goals (`AIUpdate.cpp:2158`, `:2236`): used by the
    // jet takeoff / landing states and `AIFaceState`.
    if force_driven {
        match obj.ai.locomotor_goal_type {
            LocoGoalType::PositionExplicit => {
                let goal = obj.ai.locomotor_goal_data;
                let speed = if obj.ai.desired_speed == FAST_AS_POSSIBLE || obj.ai.desired_speed > loco.max_speed { loco.max_speed } else { obj.ai.desired_speed };
                crate::locomotor::move_towards_position_force_blocked_out(obj, loco, &goal, terrain, pathfinder, now, speed, gravity, Some(0.0), &mut blocked);
                obj.ai.speed = obj.physics.as_ref().map(|p| p.speed_2d()).unwrap_or(0.0);
                return Some(blocked);
            }
            LocoGoalType::Angle => {
                let angle = obj.ai.locomotor_goal_data.x;
                crate::locomotor::move_towards_angle_force(obj, loco, angle, terrain, pathfinder, now, gravity);
                obj.ai.speed = obj.physics.as_ref().map(|p| p.speed_2d()).unwrap_or(0.0);
                return Some(blocked);
            }
            _ => {}
        }
    }
    // `case NONE: { ... requiresConstantCalling = m_curLocomotor->locoUpdate_maintainCurrentPosition(
    // getObject()); }` (`AIUpdate.cpp:2254-2285`). A goal that is not a position does not drive along
    // the path: the engine keeps the *finished* move's `m_path` until the state exits
    // (`AIUpdate.cpp:1035`) and then only maintains position, so following the stale path here would
    // move a unit the engine leaves standing.
    if obj.ai.locomotor_goal_type == LocoGoalType::None {
        if force_driven {
            crate::locomotor::maintain_current_position_force(obj, loco, now, gravity, terrain);
            obj.ai.speed = obj.physics.as_ref().map(|p| p.speed_2d()).unwrap_or(0.0);
        } else if obj.ai.speed > 0.0 || loco.is_airborne() {
            let mut speed = obj.ai.speed;
            let mut pos = obj.position;
            maintain_current_position(loco, &mut pos, obj.angle, &mut speed, terrain);
            obj.ai.speed = speed;
            obj.set_position(pos);
        }
        return Some(blocked);
    }
    // `if (!getPath())` (`AIUpdate.cpp:2177`): the engine's "no path" test is the `Path` object, not
    // the waypoint list; a path whose only node is its start node still has `getPath() != nullptr`
    // and is followed (`computePointOnPath` then has no segment and yields that last node,
    // `AIPathfind.cpp:767-768`). `computePointOnPath`'s default is the path's last *raw* node
    // (`getLastNode`).
    let path_goal = obj
        .ai
        .path
        .first()
        .copied()
        .or_else(|| obj.ai.raw_path.as_ref().and_then(|raw| raw.nodes.last().map(|n| n.pos)));
    let Some(path_goal) = path_goal else {
        if force_driven {
            // `doLocomotor` (`AIUpdate.cpp:2177-2179`): `if (!getPath()) { if (m_waitingForPath) return
            // UPDATE_SLEEP_FOREVER; }`. The move waits for its path and the tail does not run. Any other
            // missing path is a `DEBUG_CRASH` and falls through to the tail (`:2214-2228`).
            obj.ai.speed = obj.physics.as_ref().map(|p| p.speed_2d()).unwrap_or(0.0);
            if obj.ai.waiting_for_path && obj.ai.locomotor_goal_type == LocoGoalType::PositionOnPath {
                return None;
            }
            return Some(blocked);
        }
        // Airborne units keep holding their preferred height even when idle.
        if obj.ai.speed > 0.0 || loco.is_airborne() {
            let mut speed = obj.ai.speed;
            let mut pos = obj.position;
            maintain_current_position(loco, &mut pos, obj.angle, &mut speed, terrain);
            obj.ai.speed = speed;
            obj.set_position(pos);
        }
        return Some(blocked);
    };

    let goal = look.map_or(path_goal, |(g, _)| g);
    let on_path_dist = look.map(|(_, d)| d + obj.ai.path_extra_distance);
    // `AIUpdate.cpp:2212`: clamp the desired speed; while blocked cap it by `m_curMaxBlockedSpeed`
    // and decay `m_bumpSpeedLimit`.
    let loco_max = loco.max_speed;
    let mut speed = if obj.ai.desired_speed == FAST_AS_POSSIBLE || obj.ai.desired_speed > loco_max { loco_max } else { obj.ai.desired_speed };
    if blocked && speed > obj.ai.cur_max_blocked_speed {
        speed = obj.ai.cur_max_blocked_speed;
        if obj.ai.bump_speed_limit > speed {
            obj.ai.bump_speed_limit = speed;
        }
        obj.ai.bump_speed_limit *= 0.95;
        speed = obj.ai.bump_speed_limit;
    } else {
        // `else { blocked = FALSE; ... }` (`AIUpdate.cpp:2228`): a speed under the blocked cap moves the unit
        // as if it were not blocked, so the kernel gets FALSE.
        blocked = false;
        if obj.ai.bump_speed_limit < FAST_AS_POSSIBLE {
            if obj.ai.bump_speed_limit < speed * 0.2 {
                obj.ai.bump_speed_limit = speed * 0.2;
            }
            obj.ai.bump_speed_limit *= 1.05;
        }
        if speed > obj.ai.bump_speed_limit {
            speed = obj.ai.bump_speed_limit;
        }
    }
    // `m_curMaxBlockedSpeed = FAST_AS_POSSIBLE` is not here: `doLocomotor` resets it after the switch for every
    // goal (`AIUpdate.cpp:2301`), in [`finish_locomotor_frame`].

    let arrived = if force_driven {
        crate::locomotor::move_towards_position_force_blocked_out(obj, loco, &goal, terrain, pathfinder, now, speed, gravity, on_path_dist, &mut blocked)
    } else {
        let (mut pos, mut angle, mut spd) = (obj.position, obj.angle, obj.ai.speed);
        let a = move_towards_position(loco, &mut pos, &mut angle, &mut spd, &goal, terrain);
        // A `DISABLED_HELD` passenger's physics never integrates (`PhysicsUpdate.cpp:641`): its
        // container carries it, so it only "arrives" when the container does.
        if obj.is_disabled_by_type(crate::disabled::DisabledType::Held) {
            obj.ai.speed = 0.0;
        } else {
            obj.set_position(pos);
            obj.set_orientation(angle);
            obj.ai.speed = spd;
        }
        a
    };
    if force_driven {
        obj.ai.speed = obj.physics.as_ref().map(|p| p.speed_2d()).unwrap_or(0.0);
    }
    // With a look-ahead goal the unit only arrives at the end of the path.
    let arrived = arrived && !flight && look.is_none_or(|_| obj.ai.path.last().is_some_and(|l| l.dist_2d(&goal) < 0.01));
    if arrived {
        if look.is_some() {
            // The unit is at the path's last node. C++ keeps `m_path` until the move state exits
            // (`destroyPath` runs from `friend_endingMove`, `AIUpdate.cpp:1035`), and the state's
            // arrival test measures to that node (`AIStates.cpp:1908`), so keep it for that test.
            if let Some(&last) = obj.ai.path.last() {
                obj.ai.path = vec![last];
            }
        } else if obj.ai.path.len() > 1 {
            // A waypoint is reached: follow to the next one. The last node stays for the move state's
            // arrival test, as C++ keeps `m_path` (`AIStates.cpp:1908`).
            obj.ai.path.remove(0);
        }
    }
    Some(blocked)
}

/// The tail of `doLocomotor` inside `if (m_curLocomotor)`, after the goal's switch (`AIUpdate.cpp:2290-2301`):
/// a clear frame keeps the blocked counter at one (`if (!blocked && m_blockedFrames>1) m_blockedFrames = 1;`),
/// and `m_curMaxBlockedSpeed` is reset for every goal type. The reset used to sit on the ground-path move
/// only, so an explicit or angle goal left a lower cap for a later `calculateMaxBlockedSpeed` `min()`.
fn finish_locomotor_frame(obj: &mut Object, blocked: bool) {
    if !blocked && obj.ai.blocked_frames > 1 {
        obj.ai.blocked_frames = 1;
    }
    obj.ai.cur_max_blocked_speed = FAST_AS_POSSIBLE;
}

// ---- GameLogic: doPathfind, the queue and unit collisions ---------------------------------------

use crate::logic::GameLogic;

/// A read-only snapshot of one object used by the collision scan.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CollisionUnit {
    pub id: ObjectId,
    pub pos: Coord3D,
    pub angle: f32,
    pub infantry: bool,

    pub ground: bool,
    pub dead_state: bool,
    pub moving: bool,
    pub waiting_for_path: bool,
    pub blocked_frames: u16,
    /// `getStateMachine()->getGoalPosition()` (`AIUpdate.cpp:1297`): the near-goal test of `blockedBy` measures
    /// from this point, not from the pathfinder's goal cell.
    pub machine_goal: Coord3D,
    /// `getPathfindGoalCell()` is set: `goalCell.x > 0 && goalCell.y > 0` (`AIUpdate.cpp:1295-1296`).
    pub goal_cell_valid: bool,
    /// `m_locomotorGoalType != NONE`: the `otherMoving` of `blockedBy` (`AIUpdate.cpp:1310`).
    pub loco_goal: bool,
}

impl GameLogic {
    /// `Pathfinder::processPathfindQueue` + `AIUpdateInterface::doPathfind` (`AIPathfind.cpp:5989`,
    /// `AIUpdate.cpp:396`). Serviced after the sleepy updates (`TheAI->UPDATE()`), in queue order,
    /// stopping once the per-frame cell budget (`PATHFIND_CELLS_PER_FRAME`) is spent.
    pub(crate) fn process_pathfind_queue(&mut self) {
        // The dozer exception's relationship test reads the obstacle's controlling player off the
        // live object (`TheGameLogic->findObjectByID(newCell->getObstacleID())`,
        // `Object::getRelationship`, `AIPathfind.cpp:6386-6392`). A `PathfindCell` only stores the
        // obstacle id, so refresh the pathfinder's copy from this frame's object list, and the
        // enemy table (`Player::getRelationship(...) == ENEMIES`; neutral is not an enemy).
        {
            let mut enemies = [0u32; 32];
            for a in 0..32u32 {
                for b in 0..32u32 {
                    if self.relationship(a as crate::object::PlayerIndex, b as crate::object::PlayerIndex) == crate::damage::Relationship::Enemies {
                        enemies[a as usize] |= 1u32 << b;
                    }
                }
            }
            self.pathfinder.set_enemies(enemies);
            let owners: Vec<(u32, crate::object::PlayerIndex)> = self
                .objects
                .values()
                .filter(|o| !o.destroyed && o.template.locomotor.is_none())
                .map(|o| (o.id.0, o.player))
                .collect();
            self.pathfinder.set_obstacle_owners(owners);
        }
        // `Pathfinder::processPathfindQueue` (AIPathfind.cpp:6010-6013): a due zone calculation takes the
        // whole call, so requests queued before it wait one more frame.
        if self.pathfinder.zones_need_calculation() {
            self.pathfinder.calculate_zones();
            return;
        }
        self.pathfinder.reset_cell_budget();
        let mut serviced = 0u32;
        while self.pathfinder.cells_allocated() < crate::pathfind::PATHFIND_CELLS_PER_FRAME {
            let Some(id) = self.pathfinder.pop_queued_path() else { break };
            self.do_pathfind(id);
            serviced += 1;
            // Safety valve: the engine's loop is also bounded by the queue itself; the cell budget
            // normally ends it. A request that allocates no cells could otherwise spin.
            if serviced > 4096 {
                break;
            }
        }
        self.stats.pathfind_requests += u64::from(serviced);
    }

    /// `AIUpdateInterface::doPathfind` (`AIUpdate.cpp:396`): service one queued request according
    /// to its `m_isAttackPath` / `m_isApproachPath` / `m_isSafePath` flags.
    pub(crate) fn do_pathfind(&mut self, id: ObjectId) {
        // Gather everything from the object first, then release the borrow.
        let (request, dest, blocked, group_dest, from, po_opt, air_only, victim, attack, adjusts, contact) = {
            let Some(obj) = self.objects.get(&id) else { return };
            if obj.destroyed || obj.ai.suspended || obj.ai_handle.is_none() || !obj.ai.waiting_for_path {
                return;
            }
            // `AIUpdateInterface::canComputeQuickPath()` (`AIUpdate.cpp:1681-1706`): a unit whose
            // locomotor set can fly and that is not doing ground movement skips the pathfinder and
            // flies straight (`computeQuickPath`). The kind is not the test — a thrust or wing
            // *vehicle* (a missile, a scripted flyer) is a quick-path unit too.
            // `doPathfind` tests `m_isAttackPath` before any quick path (AIUpdate.cpp:430-440), and `requestAttackPath` has no
            // `canComputeQuickPath` test (AIUpdate.cpp:522-540): an attacking flyer runs `computeAttackPath`, not `computeQuickPath`.
            let air_only = self.can_compute_quick_path(id) && obj.ai.request != PathRequest::Attack;
            let mut po = if air_only { None } else { PathObject::for_object(obj) };
            // `AIUpdateInterface::canPathThroughUnits()` (`AIUpdate.cpp:3252`) is the live AI flag the
            // dock machine raises (`AIDockState::update`, AIStates.cpp:6155) and the exit-production
            // path clears. `checkForMovement`'s `setBlockedByAlly` and `findClosestPath`'s goal test
            // read it; `PathObject::for_template` starts false.
            if let Some(p) = po.as_mut() {
                p.can_path_through_units = obj.ai.can_path_through_units;
            }
            // `computeAttackPath` reads the current weapon at compute time: its range with the source's bonuses (`getAttackRange`),
            // its minimum range, and the source's radius (AIUpdate.cpp:1800-1830, `findAttackPath`'s `getRadiusAndCenter`). The
            // request stores none of these (`requestAttackPath`, AIUpdate.cpp:520-540). No weapon gives range 0 (the search fails).
            let src_radius = obj.template.geometry.bounding_circle_radius();
            let attack = match obj.combat.weapons.weapon(obj.combat.current_weapon_slot) {
                Some(w) => {
                    let wt = w.template.clone();
                    let bonus = self.combined_weapon_bonus(id, &wt);
                    (wt.attack_range() * bonus.get(crate::bonus::WeaponBonusField::Range), wt.min_attack_range(), src_radius)
                }
                None => (0.0, 0.0, src_radius),
            };
            // `source->getCurrentWeapon()->isContactWeapon()` (`AIUpdate.cpp:1865`).
            let contact = obj.combat.weapons.weapon(obj.combat.current_weapon_slot).is_some_and(|w| w.template.is_contact_weapon());
            (
                obj.ai.request,
                obj.ai.requested_destination,
                obj.ai.is_blocked_and_stuck,
                obj.ai.group_dest,
                obj.position,
                po,
                air_only,
                obj.ai.requested_victim,
                attack,
                obj.ai.adjusts_destination,
                contact,
            )
        };
        // `doPathfind` clears `m_waitingForPath` at the top.
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.waiting_for_path = false;
            // The group destination belongs to the one adjust of the move (`AIGroup::computeIndividualDestination`,
            // `AIGroup.cpp:546`). A repath asks `findPath` for the goal alone (`AIUpdate.cpp:1726`), so the
            // pending copy is spent here and a later `requestPath` carries none.
            o.ai.group_dest = None;
            o.ai.machine.group_dest = None;
        }

        // `AIUpdateInterface::computePath`'s exit special case (`AIUpdate.cpp:1692`): a unit leaving
        // production that may path through units takes the straight line, moves its allies off the
        // destination, stops passing through and clips its goal to the destination.
        let exit_pass_through = self
            .objects
            .get(&id)
            .is_some_and(|o| o.ai.machine.current == crate::ai_states::AiStateType::FollowExitProductionPath && o.ai.can_path_through_units);
        if exit_pass_through {
            if self.compute_quick_path(id, &dest) {
                self.move_allies_away_from_destination(id, &dest);
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.can_path_through_units = false;
                    // `setGoalPositionClipped(destination, CMD_FROM_AI)` (`AIUpdate.cpp:1712`).
                    o.ai.machine.goal_position = dest;
                }
                self.ai_update_goal_to_path_end(id, true);
            }
            return;
        }

        // `computePath` (`AIUpdate.cpp:1682-1686`): `if (!m_isBlockedAndStuck) destroyPath(); if
        // (canComputeQuickPath()) return computeQuickPath(destination);`. A blocked-and-stuck unit keeps its
        // old path for `computeQuickPath`'s "already going there" early return.
        if air_only {
            if !blocked {
                self.destroy_ai_path(id);
            }
            self.compute_quick_path(id, &dest);
            // `computeQuickPath` leaves the locomotor goal `NONE` (`destroyPath`) and does not re-arm it; the
            // move state's update does that on its next frame (`AIStates.cpp:1875-1876`). The port's attack
            // states drive the locomotor directly instead of running the move state, so the arming happens
            // where the path lands (the next frame's `doLocomotor`, as in C++). `doPathfind`'s
            // `if (m_isFinalGoal && isDoingGroundMovement() && getPath()) updateGoal(...)` (`:462-465`) does
            // not run here: the unit is not doing ground movement.
            self.ai_goal_on_path_if_any(id);
            return;
        }
        if request == PathRequest::None {
            if !blocked {
                self.destroy_ai_path(id);
            }
            self.compute_quick_path(id, &dest);
            self.ai_goal_on_path_if_any(id);
            self.ai_update_goal_to_path_end(id, true);
            return;
        }
        let Some(po) = po_opt else {
            if !blocked {
                self.destroy_ai_path(id);
            }
            self.compute_quick_path(id, &dest);
            self.ai_update_goal_to_path_end(id, true);
            return;
        };

        if request == PathRequest::Approach {
            // `destroyPath()` (AIUpdate.cpp:424), then `findClosestPath`: approach to the requested point.
            self.destroy_ai_path(id);
            let terrain = &self.terrain;
            let mut raw_to = dest;
            let path = self.pathfinder.find_closest_path(terrain, &po, &from, &mut raw_to, blocked);
            // `m_path = findClosestPath(...)` (AIUpdate.cpp:426-428) writes the path and nothing else: no
            // `m_pathTimestamp`, no `m_blockedFrames` / `m_isBlockedAndStuck` (those are `computePath`'s and
            // `computeAttackPath`'s, AIUpdate.cpp:1767-1771, :2003-2006). No `moveAllies` either (the branch returns
            // after `updateGoal`, AIUpdate.cpp:429-432). Lane 187 (R24-3).
            self.set_ai_path(id, path, /*retry=*/ false);
            // `if (isDoingGroundMovement() && getPath()) updateGoal(lastNode)` (AIUpdate.cpp:429-432).
            self.ai_update_goal_to_path_end(id, false);
            return;
        }

        if request == PathRequest::Attack {
            // `computeAttackPath` -> `findAttackPath` (`AIPathfind.cpp:10709`).
            let (vpos, vradius) = match self.objects.get(&victim) {
                Some(v) => (v.position, v.template.geometry.bounding_circle_radius()),
                None => (dest, 0.0),
            };
            // `computeAttackPath`'s in-range arm (AIUpdate.cpp:1828-1845): a victim already in weapon range, and in view (a unit
            // on the ground is checked against obstacles, `isDoingGroundMovement`), gets `destroyPath()` and no path. `doPathfind`
            // returns with `m_isAttackPath` TRUE (AIUpdate.cpp:446), so the approach's empty path ends it (AIStates.cpp:1797-1801).
            {
                let d2 = crate::combat::boundary_dist_sqr(&from, attack.2, &vpos, vradius);
                let in_range = d2 <= attack.0 * attack.0 && d2 >= attack.1 * attack.1;
                if in_range {
                    let view_blocked = self.is_doing_ground_movement(id)
                        && !(victim != ObjectId::INVALID && self.significantly_above_terrain(victim))
                        && self.attack_view(id, (victim != ObjectId::INVALID).then_some(victim)).is_some_and(|v| self.pathfinder.attack_view_blocked(&self.terrain, &v, &from, &vpos));
                    if !view_blocked {
                        self.destroy_ai_path(id);
                        if let Some(o) = self.objects.get_mut(&id) {
                            o.ai.attack_path_active = true;
                        }
                        return;
                    }
                }
            }
            if contact {
                // `computeAttackPath`'s contact-weapon arm (`AIUpdate.cpp:1865-1878`): "Weapon is basically
                // a contact weapon, like a car bomb ... just approach the target position", and
                // `getCurLocomotor()->setNoSlowDownAsApproachingDest(TRUE)` so the approach does not brake
                // near the victim. (The engine runs `computePath` for this arm, not `findAttackPath`, and
                // jams the victim position into the last node; the port keeps its one attack-path builder
                // and applies the flag to it.)
                if let Some(o) = self.objects.get_mut(&id) {
                    o.loco_rt.no_slow_down_as_approaching_dest = true;
                }
            }
            // `computeAttackPath`'s airborne arm (AIUpdate.cpp:1924-1956): a flyer above the terrain (`isAboveTerrain`, and
            // `!landBound`: its surfaces include AIR, :1799-1805) builds a trivial two-node path, its position at the victim's ground
            // height and the approach target. "for now, airborne objects don't pathfind". The arm is not taken for a contact weapon,
            // whose arm returns first (:1865-1878). An existing two-node path that already ends within 0.5 units is kept (:1944-1946).
            let airborne = !contact
                && self.objects.get(&id).is_some_and(|o| {
                    o.position.z > self.terrain.surface_height(o.position.x, o.position.y, o.position.z) + 0.001
                        && o.locomotor().is_some_and(|l| l.surfaces_air)
                });
            if airborne {
                // `localVictimPos.z = getLayerHeight(x, y, victimLayer)` (AIUpdate.cpp:1921). The port has no layers: ground height.
                let ground_z = self.terrain.ground_height(vpos.x, vpos.y);
                let target = Coord3D::new(vpos.x, vpos.y, ground_z);
                // `weapon->computeApproachTarget` (AIUpdate.cpp:1929).
                let aim = crate::combat::approach_point(&from, attack.2, &target, vradius, attack.0, attack.1);
                let keep = self.objects.get(&id).and_then(|o| o.ai.raw_path.as_ref()).is_some_and(|p| {
                    let close = p.nodes.first().and_then(|n| n.next_optimized).and_then(|i| p.nodes.get(i));
                    close.is_some_and(|c| c.next_optimized.is_none() && {
                        let (dx, dy) = (aim.x - c.pos.x, aim.y - c.pos.y);
                        dx * dx + dy * dy < 0.25
                    })
                });
                if keep {
                    self.ai_update_goal_to_path_end(id, false);
                } else {
                    // `destroyPath()` (AIUpdate.cpp:1951), then the two nodes, the unit's position at the target's height first.
                    self.destroy_ai_path(id);
                    let path = Path {
                        nodes: vec![
                            PathNode { pos: Coord3D::new(from.x, from.y, ground_z), can_optimize: true, next_optimized: Some(1) },
                            PathNode { pos: aim, can_optimize: true, next_optimized: None },
                        ],
                        blocked_by_ally: false,
                    };
                    self.install_ai_path(id, Some(path), false);
                    self.ai_update_goal_to_path_end(id, false);
                    self.ai_goal_on_path_if_any(id);
                }
                // `doPathfind` sets `m_isAttackPath = TRUE` once `computeAttackPath` succeeds (AIUpdate.cpp:446).
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.attack_path_active = true;
                }
                return;
            }
            // `findAttackPath`'s view inputs (`AIPathfind.cpp:10750`, `:10869`) and its `checkLOS` (`:10844-10850`).
            let view = self.attack_view(id, Some(victim));
            let check_los = !self.significantly_above_terrain(victim);
            let terrain = &self.terrain;
            let path = self.pathfinder.find_attack_path(terrain, &po, &from, &vpos, view.as_ref(), attack.0, attack.1, attack.2, vradius, check_los);
            match path {
                Some(p) => {
                    // `doPathfind` sets `m_isAttackPath = TRUE` once `computeAttackPath` succeeds (AIUpdate.cpp:445-446).
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.ai.attack_path_active = true;
                    }
                    // `computeAttackPath` (AIUpdate.cpp:1982-1996): the path is installed, `updateGoal(lastNode)` runs (:1992),
                    // and only then does `moveAllies` run, and only for a path blocked by an ally (:1995-1996). Lane 185 (R23-18).
                    let for_allies = p.clone();
                    self.install_ai_path(id, Some(p), /*retry=*/ false);
                    // `if (getPath()) updateGoal(lastNode)` (AIUpdate.cpp:441-445).
                    self.ai_update_goal_to_path_end(id, false);
                    if for_allies.blocked_by_ally {
                        self.move_allies_unless_no_collide(id, &for_allies);
                    }
                    // The attack's approach/pursue run as move states in the engine, and their
                    // `AIInternalMoveToState::update` re-arms `setLocomotorGoalPositionOnPath()` on any
                    // frame with a path (`AIStates.cpp:1875-1876`); `computeAttackPath` itself does not
                    // (`AIUpdate.cpp:1980-2011`). The port's attack drives the locomotor directly, so
                    // the path's arrival is where that re-arm lands (the next frame's `doLocomotor`,
                    // as in C++).
                    self.ai_goal_on_path_if_any(id);
                }
                None => {
                    // `m_isAttackPath = FALSE` once `computeAttackPath` fails (AIUpdate.cpp:450).
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.ai.attack_path_active = false;
                    }
                    // `doPathfind`'s fallback (`AIUpdate.cpp:450-457`):
                    //   m_requestedDestination = *victim->getPosition();
                    //   TheAI->pathfinder()->adjustToPossibleDestination(getObject(), getLocomotorSet(), &m_requestedDestination);
                    //   ignoreObstacle(victim);
                    //   computePath(pathfinder, &m_requestedDestination);
                    // The adjust reads the unit's *previous* ignored obstacle (`checkDestination` ->
                    // `getIgnoredObstacleID`), because `ignoreObstacle(victim)` comes after it.
                    let mut target = if victim == ObjectId::INVALID { dest } else { vpos };
                    if victim != ObjectId::INVALID {
                        let unit_players: std::collections::BTreeMap<u32, crate::object::PlayerIndex> =
                            self.objects.values().map(|o| (o.id.0, o.player)).collect();
                        let previous_ignore = self.objects.get(&id).map_or(ObjectId::INVALID, |o| o.ai.ignore_obstacle_id);
                        let mut adjust_obj = po;
                        adjust_obj.ignore_obstacle = previous_ignore;
                        let _ = self.pathfinder.adjust_to_possible_destination(
                            terrain,
                            &adjust_obj,
                            &from,
                            &mut target,
                            &|u| unit_players.get(&u).copied(),
                        );
                        // `ignoreObstacle(victim)` (AIUpdate.cpp:455, `m_ignoreObstacleID` at :4305-4308): it persists until the
                        // move completes (`ignoreObstacle(nullptr)` at AIUpdate.cpp:1061), so `collide`, `moveAllies` and the next
                        // search read the victim, not only this search's explicit argument (R26-5).
                        if let Some(o) = self.objects.get_mut(&id) {
                            o.ai.ignore_obstacle_id = victim;
                        }
                    }
                    // `computePath` itself does not adjust: `adjusts` is false, and the ignored
                    // obstacle is the victim. The port's `PathRequest::Attack` fallback flags the
                    // retry path like `computePath`'s closest-path fallback.
                    // The fallback comes through `computePath` (AIUpdate.cpp:461), so its straight-line shortcut
                    // (`:1709-1712`, `if (!m_isFinalGoal && isLinePassable(...)) return computeQuickPath`) applies first.
                    // `m_isFinalGoal` is the request's stale value: `requestAttackPath` (AIUpdate.cpp:520-540) does not set it.
                    let not_final_goal = self.objects.get(&id).is_some_and(|o| !o.ai.is_final_goal);
                    if not_final_goal && self.pathfinder.line_passable_for_path(&self.terrain, &po, &from, &target, victim) {
                        if !blocked {
                            self.destroy_ai_path(id);
                        }
                        self.compute_quick_path(id, &target);
                        self.ai_pathfind_tail(id);
                        return;
                    }
                    let closest = self.pathfinder.compute_ai_path(terrain, &po, &from, &target, None, false, None, victim, false);
                    let for_allies = closest.clone();
                    self.install_ai_path(id, closest, /*retry=*/ true);
                    // `computePath`'s `if (theNewPath) ... setLocomotorGoalPositionOnPath()`
                    // (`AIUpdate.cpp:1740-1745`): the fallback comes through `computePath` too, and its `moveAllies` (:1747)
                    // follows the goal (lane 185, R23-19).
                    self.ai_goal_on_path_if_any(id);
                    if let Some(p) = for_allies {
                        self.move_allies_unless_no_collide(id, &p);
                    }
                    self.ai_update_goal_to_path_end(id, true);
                    // The failed attack path falls through to `computePath`, so `doPathfind`'s tail runs.
                    self.ai_pathfind_tail(id);
                }
            }
            return;
        }

        // `computePath`: `setIgnoreObstacleID(m_ignoreObstacleID)` (`AIUpdate.cpp:1703`).
        let ignore = self.objects.get(&id).map_or(ObjectId::INVALID, |o| o.ai.ignore_obstacle_id);
        // `computePath`'s straight-line shortcut (`AIUpdate.cpp:1709-1712`): `if (!m_isFinalGoal && isLinePassable(...))
        // return computeQuickPath(destination);`. A quick path is built with no search and no goal: the locomotor goal
        // stays `NONE` (`destroyPath`), and `doPathfind`'s tail runs for the non-final goal (`:458-465`).
        let not_final_goal = self.objects.get(&id).is_some_and(|o| !o.ai.is_final_goal);
        if not_final_goal && self.pathfinder.line_passable_for_path(&self.terrain, &po, &from, &dest, ignore) {
            if !blocked {
                self.destroy_ai_path(id);
            }
            self.compute_quick_path(id, &dest);
            self.ai_pathfind_tail(id);
            return;
        }

        let terrain = &self.terrain;
        let original = self.objects.get(&id).and_then(|o| o.ai.raw_path.clone());
        let path = self.pathfinder.compute_ai_path(terrain, &po, &from, &dest, group_dest, blocked, original.as_ref(), ignore, adjusts);
        match path {
            Some(p) => {
                let for_allies = p.clone();
                self.install_ai_path(id, Some(p), /*retry=*/ false);
                // `if (theNewPath) { destroyPath(); m_path = theNewPath; ... setLocomotorGoalPositionOnPath(); }`
                // (`AIUpdate.cpp:1740-1745`), then `moveAllies(getObject(), theNewPath)` (:1747). The goal is set before the
                // push (lane 185, R23-19).
                self.ai_goal_on_path_if_any(id);
                self.move_allies_unless_no_collide(id, &for_allies);
                self.ai_update_goal_to_path_end(id, true);
            }
            None => {
                // `computePath` (`AIUpdate.cpp:1748-1780`). A blocked-and-stuck unit keeps its old
                // path, and the engine then stops it and waits one second on the spot; otherwise the
                // path is gone (`destroyPath` at the top of `computePath`) and the closest-path
                // fallback inside `computePath` already ran and failed.
                let kept = blocked && self.objects.get(&id).is_some_and(|o| o.ai.raw_path.is_some() || !o.ai.path.is_empty());
                if kept {
                    self.ai_stop_and_wait(id, &po);
                } else {
                    self.store_ai_path(id, None, /*retry=*/ true);
                    self.ai_update_goal_to_path_end(id, true);
                }
            }
        }
        self.ai_pathfind_tail(id);
    }

    /// The tail of `AIUpdateInterface::doPathfind` (`AIUpdate.cpp:463-465`):
    ///
    /// ```text
    /// if (m_queueForPathFrame > TheGameLogic->getFrame()) {
    ///     m_waitingForPath = TRUE;
    /// }
    /// ```
    ///
    /// A request that was serviced while an anti-spin wait from an *earlier* `requestPath`
    /// (`setQueueForPathTime`, `:499-514`) is still pending leaves the AI waiting: the move state's
    /// update keeps taking its waiting early return (`AIStates.cpp:1786-1793`, which also stamps the
    /// state's own `m_pathTimestamp` every frame) and the engine only recomputes the path when the
    /// deferred request comes due (`:1064-1078`). Without this, a move could drift past its own path
    /// for as long as the deferred request stayed queued (AH7 s1 f378: dozer 231's path is rebuilt
    /// from its current position 29 frames after the one it is still following).
    pub(crate) fn ai_pathfind_tail(&mut self, id: ObjectId) {
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&id) {
            if o.ai.queue_for_path_frame > now {
                o.ai.waiting_for_path = true;
            }
        }
        // `if (!m_waitingForPath) wakeUpNow();` (AIUpdate.cpp:466-472, SLEEPY_AI): a computed path wakes the module.
        let waiting = self.objects.get(&id).is_some_and(|o| o.ai.waiting_for_path);
        if !waiting {
            self.wake_up_now(id);
        }
    }

    /// The goal update `doPathfind` makes after a path is computed (AIUpdate.cpp:429-432, :441-445, :458-461): the
    /// path's last node becomes the goal cell (`updateGoal(lastNode)`). With `final_only`, only a final-goal move
    /// (`m_isFinalGoal`, the move's adjust flag) does it. The ground check is `updateGoal`'s own. `lastNode` is
    /// the `Path`'s last *raw* node (`getLastNode`), not the last waypoint of the optimised chain; the waypoint
    /// list is only a fallback for the port's waypoint-only taxi path (`jet_states`, no `Path` object).
    fn ai_update_goal_to_path_end(&mut self, id: ObjectId, final_only: bool) {
        let Some((final_goal, last)) = self.objects.get(&id).map(|o| {
            let last = o.ai.raw_path.as_ref().and_then(|raw| raw.nodes.last().map(|n| n.pos)).or_else(|| o.ai.path.last().copied());
            (o.ai.is_final_goal, last)
        }) else {
            return;
        };
        if final_only && !final_goal {
            return;
        }
        if let Some(last) = last {
            self.pathfind_update_goal(id, &last);
        }
    }

    /// The `computePath` else-branch for a blocked and stuck unit with its path kept (`AIUpdate.cpp:1749-1759`):
    /// destroy the path, wait one second (`setQueueForPathTime(LOGICFRAMES_PER_SECOND)`), stop where it
    /// stands (`snapPosition`, `setFinalPosition`), drop the locomotor goal and clear the blocked flags.
    pub(crate) fn ai_stop_and_wait(&mut self, id: ObjectId, po: &PathObject) {
        let now = self.frame;
        let here = self.objects.get(&id).map_or(Coord3D::ZERO, |o| o.position);
        let snapped = self.pathfinder.snap_position(&self.terrain, po, &here);
        self.store_ai_path(id, None, /*retry=*/ false);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.queue_for_path_frame = now + crate::math::LOGICFRAMES_PER_SECOND;
            // `setFinalPosition` (`AIUpdate.h:525`): the final position is recorded, `m_doFinalPosition` FALSE.
            o.ai.final_position = snapped;
            o.ai.do_final_position = false;
            o.ai.set_locomotor_goal_none();
            o.ai.blocked_frames = 0;
            o.ai.is_blocked = false;
            o.ai.is_blocked_and_stuck = false;
        }
        // `setQueueForPathTime(LOGICFRAMES_PER_SECOND)` (AIUpdate.cpp:1754) pulls a module asleep past that queue time.
        self.wake_up_for_queue_time(id, crate::math::LOGICFRAMES_PER_SECOND);
    }

    /// Store the path `doPathfind` produced and clear the request's transient flags.
    ///
    /// The state-level `m_pathGoalPosition` is **not** written here: `computePath`'s tail sets only
    /// `m_pathTimestamp`, `m_blockedFrames` and `m_isBlockedAndStuck` (`AIUpdate.cpp:1767-1771`). The state object's
    /// own value is written by its wait block (`AIStates.cpp:1804`) and by `forceRepath` (`AIStateMachine.h:275-279`).
    /// Writing it from the queue would hit whichever state's fields are live when `processPathfindQueue` runs (after
    /// `updateStateMachine`, so a temporary's path would land on the main move's value).
    fn store_ai_path(&mut self, id: ObjectId, path: Option<Path>, retry: bool) {
        self.store_ai_path_moving_allies(id, path, retry, false);
    }

    /// [`store_ai_path`](Self::store_ai_path) and the engine's `moveAllies` on the new path (AIUpdate.cpp:1747,
    /// `computePath`'s `if (theNewPath) ... moveAllies`). `blocked_only` is the attack path's gate (`computeAttackPath`,
    /// AIUpdate.cpp:1995, `if (m_path->getBlockedByAlly())`).
    fn store_ai_path_moving_allies(&mut self, id: ObjectId, path: Option<Path>, retry: bool, blocked_only: bool) {
        let for_allies = path.clone();
        self.install_ai_path(id, path, retry);
        if let Some(p) = for_allies {
            if !blocked_only || p.blocked_by_ally {
                self.move_allies_unless_no_collide(id, &p);
            }
        }
    }

    /// `if (!getObject()->isKindOf(KINDOF_NO_COLLIDE)) TheAI->pathfinder()->moveAllies(getObject(), path)`
    /// (AIUpdate.cpp:1745-1747 for a new path, :1995-1996 for an attack path blocked by an ally).
    fn move_allies_unless_no_collide(&mut self, id: ObjectId, path: &Path) {
        let no_collide = self.objects.get(&id).is_some_and(|o| o.is_kind_of(crate::template::kind_of::NO_COLLIDE));
        if !no_collide {
            self.move_allies(id, path);
        }
    }

    /// The install half of [`store_ai_path_moving_allies`](Self::store_ai_path_moving_allies): the path and its
    /// `computePath` tail (`AIUpdate.cpp:1767-1771`, the attack path's `:2003-2006`), with no `moveAllies`. The approach
    /// branch of `doPathfind` does not come through here: it installs the path alone ([`set_ai_path`](Self::set_ai_path)).
    fn install_ai_path(&mut self, id: ObjectId, path: Option<Path>, retry: bool) {
        self.set_ai_path(id, path, retry);
        if let Some(o) = self.objects.get_mut(&id) {
            // `m_pathTimestamp = now; m_blockedFrames = 0; m_isBlockedAndStuck = FALSE` at the end of
            // `computePath` (`AIUpdate.cpp:1767-1771`).
            o.ai.path_timestamp = self.frame;
            o.ai.blocked_frames = 0;
            o.ai.is_blocked_and_stuck = false;
        }
    }

    /// The path half of an install: `m_path` and its raw path (`AIUpdate.cpp:426-428`, `computePath`'s
    /// `m_path = theNewPath`). Writes no timestamp and no blocked flags, which is what the approach branch needs.
    fn set_ai_path(&mut self, id: ObjectId, path: Option<Path>, retry: bool) {
        if let Some(o) = self.objects.get_mut(&id) {
            match path {
                Some(p) => {
                    o.ai.path = p.waypoints();
                    o.ai.raw_path = Some(p);
                }
                None => {
                    o.ai.path.clear();
                    o.ai.raw_path = None;
                }
            }
            o.ai.retry_path = retry;
        }
    }

    /// `computePath`'s `if (theNewPath) ... setLocomotorGoalPositionOnPath()` (`AIUpdate.cpp:1744-1745`):
    /// a computed path makes the locomotor follow it. The engine arms on the *path object*
    /// (`getPath() != nullptr`), and a path whose only node is its start node is still a path, so the
    /// test is `has_path()`, not the waypoint list. A failed search leaves the goal type alone (the
    /// engine only clears it when a completed move ends, `AIUpdate.cpp:1035-1041`).
    fn ai_goal_on_path_if_any(&mut self, id: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            if o.ai.has_path() {
                o.ai.set_locomotor_goal_position_on_path();
            }
        }
    }

    /// `Pathfinder::moveAlliesAwayFromDestination` (`AIPathfind.cpp:6985`): the allies standing along the
    /// line to `dest` that are not moving are asked to step aside (`aiMoveAwayFromUnit`). The
    /// `ALLIES` relationship, the ignored obstacle, `isMoving` and the busy / ability vetoes are the callback's
    /// tests (`moveAlliesDestinationCallback`, AIPathfind.cpp:6950-6980). The callback runs for every cell of the
    /// line, so a unit covering two cells is asked twice, and each visit reads the unit's state live: a unit whose
    /// first push failed (its temporary's enter found no path) is still pushable on its second visit.
    fn move_allies_away_from_destination(&mut self, id: ObjectId, dest: &Coord3D) {
        let Some(o) = self.objects.get(&id) else { return };
        let (from, player, ignore) = (o.position, o.player, o.ai.ignore_obstacle_id);
        for unit in self.pathfinder.units_along_line(&from, dest) {
            let other = ObjectId(unit);
            if other == id || other == ignore {
                continue;
            }
            let Some(a) = self.objects.get(&other) else { continue };
            if a.ai_handle.is_none() || !self.pathfinder.are_allies(player, a.player) || a.is_ai_moving() {
                continue;
            }
            // "Black Lotus exploit fix -- moving while hacking." (AIPathfind.cpp:6971-6972).
            let using_ability = a.status_mask(self.frame) & crate::object::status::USING_ABILITY != 0;
            if using_ability || a.ai.machine.current == crate::ai_states::AiStateType::Busy {
                continue;
            }
            self.ai_move_away_from_unit(other, id);
        }
    }

    /// `AIUpdateInterface::computeQuickPath` (`AIUpdate.cpp:1615-1662`) for the sim: the path a quick-path
    /// unit gets without the pathfinder. Returns whether a path exists.
    ///
    /// "First, see if our path already goes to the destination" (`:1620-1636`): when the current path's
    /// last node has no next optimised node and is within 0.5 units of the destination (the squared 3D
    /// distance is under 0.25), the old path is kept **as it is** — the start node, detour nodes, timestamp
    /// and blocked flags all stay, because the early return precedes `destroyPath`/`m_pathTimestamp`.
    ///
    /// Otherwise `destroyPath()` (`:1638`) drops the old path (path, raw path, `m_waitingForPath` and the
    /// locomotor goal) and a real two-node `Path` is built: `KINDOF_AIRCRAFT` that is not a
    /// `KINDOF_PROJECTILE` takes `getAircraftPath` (`:1639-1640`), anything else the start/destination pair
    /// (`:1641-1650`). Keeping the `Path` (not just the destination) is what gives `getPath()` a node to walk:
    /// `getMoveAwayFromPath` reads another unit's path through it (`AIPathfind.cpp:10291`) and
    /// `computeFlightDistToGoal` walks its node chain, and the per-object trace prints its `pcount`/`pfirst`/`plast`/`pnext`.
    pub(crate) fn compute_quick_path(&mut self, id: ObjectId, dest: &Coord3D) -> bool {
        let already = self.objects.get(&id).is_some_and(|o| {
            o.ai.raw_path.as_ref().is_some_and(|raw| {
                raw.nodes.last().is_some_and(|last| {
                    last.next_optimized.is_none() && {
                        let (dx, dy, dz) = (dest.x - last.pos.x, dest.y - last.pos.y, dest.z - last.pos.z);
                        dx * dx + dy * dy + dz * dz < 0.25
                    }
                })
            })
        });
        if already {
            return true;
        }
        // `destroyPath()` (`:1638`): the old path and the locomotor goal go before the new path is built.
        self.destroy_ai_path(id);
        let Some(o) = self.objects.get(&id) else { return false };
        let aircraft = o.is_kind_of(crate::template::kind_of::AIRCRAFT) && !o.is_kind_of(crate::template::kind_of::PROJECTILE);
        let path = if aircraft {
            // `getAircraftPath` (`:1639-1640`): clips the destination for a `LOCO_WINGS` locomotor and jogs
            // around `KINDOF_AIRCRAFT_PATH_AROUND` buildings. `getBuildingToNotPathAround()` is the Chinook's
            // combat-drop target (`ChinookAIUpdate.cpp:1043-1052`), `INVALID_ID` for every other AI.
            let clips = o.locomotor().is_some_and(|l| l.kind == LocomotorKind::Wings);
            let from = o.position;
            let to_z = dest.z;
            let avoid = self.building_to_not_path_around(id);
            let closest_tall = |p: &Coord3D| -> Option<(u32, Coord3D, f32)> {
                for candidate in self.partition.objects_in_range(p, 100.0, crate::partition::DistanceMode::FromBoundingSphere2D) {
                    let Some(b) = self.objects.get(&candidate) else { continue };
                    if !b.template.aircraft_path_around {
                        continue;
                    }
                    return Some((candidate.0, b.position, b.template.geometry.bounding_circle_radius()));
                }
                None
            };
            let tall_by_id = |oid: u32| -> Option<(Coord3D, f32)> {
                let b = self.objects.get(&ObjectId(oid))?;
                if !b.template.aircraft_path_around {
                    return None;
                }
                Some((b.position, b.template.geometry.bounding_circle_radius()))
            };
            self.pathfinder.aircraft_path(&from, to_z, dest, clips, avoid, &closest_tall, &tall_by_id)
        } else {
            // `m_path->prependNode(destination, LAYER_GROUND)` then the object's position at the
            // destination's z (`:1643-1648`), so the first node is the unit's own start point.
            let start = Coord3D::new(o.position.x, o.position.y, dest.z);
            Path {
                nodes: vec![
                    PathNode { pos: start, can_optimize: false, next_optimized: Some(1) },
                    PathNode { pos: *dest, can_optimize: false, next_optimized: None },
                ],
                blocked_by_ally: false,
            }
        };
        let waypoints = path.waypoints();
        let frame = self.frame;
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.flight_goal = None;
            o.ai.path = waypoints;
            o.ai.raw_path = Some(path);
            // `m_pathTimestamp = TheGameLogic->getFrame(); m_blockedFrames = 0; m_isBlockedAndStuck = FALSE`
            // (`:1657-1660`). No state-level `m_pathGoalPosition` write: `computeQuickPath` has none either
            // (`AIUpdate.cpp:1615-1662`); the requesting state's wait block stamps its own value
            // (`AIStates.cpp:1804`). A quick path can also land from `doPathfind` (the queue tail of
            // `computePath`), where the live state fields belong to the main move, not the requester.
            o.ai.path_timestamp = frame;
            o.ai.waiting_for_path = false;
            o.ai.blocked_frames = 0;
            o.ai.is_blocked_and_stuck = false;
        }
        true
    }

    /// `AIUpdateInterface::destroyPath` (`AIUpdate.cpp:2018-2028`): delete the path and clear
    /// `m_waitingForPath` and the locomotor goal (`setLocomotorGoalNone`, `:2015-2019`). The port's
    /// `AiState::request` is the `doPathfind` branch selector, not `m_isAttackPath`, so it is the request
    /// site's to clear.
    fn destroy_ai_path(&mut self, id: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.path.clear();
            o.ai.raw_path = None;
            o.ai.waiting_for_path = false;
            // `destroyPath` sets `m_isAttackPath = FALSE` (AIUpdate.cpp:2026).
            o.ai.attack_path_active = false;
            o.ai.set_locomotor_goal_none();
        }
    }

    /// `AIUpdateInterface::getBuildingToNotPathAround()` (`AIUpdate.cpp:342`): only `ChinookAIUpdate`
    /// overrides it (`ChinookAIUpdate.cpp:1043-1052`) — in `MOVE_TO_COMBAT_DROP`/`DO_COMBAT_DROP` the
    /// state's goal object is the building the aircraft must not path around (it is dropping into it).
    fn building_to_not_path_around(&self, id: ObjectId) -> ObjectId {
        match self.chinooks.get(&id) {
            Some(st) => match st.phase {
                crate::chinook::ChinookPhase::MoveToDrop | crate::chinook::ChinookPhase::CombatDrop => st.target_obj.unwrap_or(ObjectId::INVALID),
            },
            None => ObjectId::INVALID,
        }
    }

    /// `AIUpdateInterface::canComputeQuickPath` (`AIUpdate.cpp:1588-1605`): "if a unit is moving
    /// through the air, we can quick path" — `m_locomotorSet.getValidSurfaces() & LOCOMOTORSURFACE_AIR` and
    /// the unit is not doing ground movement. Such a unit's path is built inside `requestPath`
    /// (`AIUpdate.cpp:493-497`), not queued.
    ///
    /// The port tests the current locomotor's `AIR` surface rather than the set's union: `m_curLocomotor` is
    /// always an entry of the current set (`chooseLocomotorSetExplicit` builds one per entry,
    /// `AIUpdate.cpp:830-841`; `chooseLocomotor` picks among them, `:860-895`), so a union with `AIR` and a
    /// ground current locomotor still fails `!isDoingGroundMovement()` (`:2363-2385` returns TRUE for a
    /// non-air current locomotor), which is exactly the current-locomotor answer. Same for
    /// [`is_doing_ground_movement`](Self::is_doing_ground_movement), whose surface test is
    /// `m_curLocomotor->getLegalSurfaces() & AIR` (`:2374-2378`).
    pub(crate) fn can_compute_quick_path(&self, id: ObjectId) -> bool {
        self.objects.get(&id).and_then(|o| o.locomotor()).is_some_and(|l| l.surfaces_air) && !self.is_doing_ground_movement(id)
    }

    /// The [`CollisionUnit`] snapshot of one object: the fields `processCollision` and `blockedBy`
    /// read (`AIUpdate.cpp:1289`, `:1427`). `None` for an object with no locomotor.
    pub(crate) fn collision_unit(&self, id: ObjectId) -> Option<CollisionUnit> {
        let o = self.objects.get(&id)?;
        let loco = o.locomotor()?;
        let goal_cell_valid = self.pathfinder.goal_cell(id).is_some_and(|(cx, cy)| cx > 0 && cy > 0);
        Some(CollisionUnit {
            id: o.id,
            pos: o.position,
            angle: o.angle,
            infantry: o.is_kind_of(crate::template::kind_of::INFANTRY),
            ground: !loco.is_airborne(),
            dead_state: o.is_dying(),
            moving: o.is_ai_moving(),
            waiting_for_path: o.ai.waiting_for_path,
            blocked_frames: o.ai.blocked_frames,
            machine_goal: o.ai.machine.goal_position,
            goal_cell_valid,
            loco_goal: o.ai.locomotor_goal_type != LocoGoalType::None,
        })
    }

    /// `AIUpdateInterface::blockedBy(other)` (`AIUpdate.cpp:1289`) for one pair, as the physics
    /// callback asks it (`processCollision`, `:1450`).
    pub(crate) fn unit_blocked_by(&self, me: ObjectId, other: ObjectId) -> bool {
        // `if (getCurLocomotor() && getCurLocomotor()->isMovingBackwards()) return false; // don't collide.` (AIUpdate.cpp:1316):
        // a unit backing up (the crawler's `CanMoveBackwards` locomotor) is not blocked by anything. Lane 194 round 4: the port
        // lacked this, and the crawler stopped against a rider it had just passed (AN5 s1 f3763, `ZR_L194_COL`).
        if self.objects.get(&me).and_then(|o| o.physics.as_ref()).is_some_and(|p| p.moving_backwards) {
            return false;
        }
        match (self.collision_unit(me), self.collision_unit(other)) {
            (Some(a), Some(b)) => !self.can_crush_or_squish(me, other) && blocked_by_pri(&a, &b, self.has_higher_path_priority(me, other)),
            _ => false,
        }
    }

    /// `AIUpdateInterface::processCollision` (`AIUpdate.cpp:1427-1581`), the branch for a unit that is moving
    /// (`selfMoving`), for one contact of `me` with `other`. Called from `PhysicsBehavior::onCollide` (`PhysicsUpdate.cpp:1278`)
    /// on the contact's own `onCollide`, so it sees the positions the earlier contacts of the walk left. Reads the live
    /// state; sets `m_isBlocked`, `m_curMaxBlockedSpeed`, `m_blockedFrames` and `m_isBlockedAndStuck` on `me` and runs
    /// the move-away calls. Returns the engine's TRUE (the force is kept): only the panic infantry bounce (`:1450-1458`).
    /// The idle branch (`:1517-1574`) is `ai_unit_overlap` in `collide.rs`.
    pub(crate) fn process_collision_moving(&mut self, me: ObjectId, other: ObjectId) -> bool {
        let frame = self.frame;
        let (Some(a), Some(b)) = (self.collision_unit(me), self.collision_unit(other)) else { return false };
        // `if (m_ignoreCollisionsUntil > frame) return FALSE; if (m_canPathThroughUnits) return FALSE;` (`:1434-1438`).
        if self.objects.get(&me).is_some_and(|o| o.ai.ignore_collisions_until > frame || o.ai.can_path_through_units) {
            return false;
        }
        // `if (!isDoingGroundMovement()) return FALSE; if (!aiOther->isDoingGroundMovement()) return FALSE;` (`:1446-1447`).
        if !a.ground || !b.ground || !a.moving {
            return false;
        }
        // `Bool otherMoving = (aiOther && aiOther->isMoving())` (`:1444`).
        let other_moving = b.moving;
        // `Bool blocked = blockedBy(other)` (`:1450`).
        if !self.unit_blocked_by(me, other) {
            return false;
        }
        // Panic infantry bounce off other humans (`:1452-1454`): TRUE, the force is kept.
        if a.infantry && self.objects.get(&me).is_some_and(|o| o.ai.machine.current == crate::ai_states::AiStateType::Panic) {
            return true;
        }
        self.with_ai(me, |ai| ai.is_blocked = true); // `m_isBlocked = TRUE` (`:1457`)
        // `AIUpdate.cpp:1458-1459`: let a unit waiting for its path get it.
        if other_moving && b.waiting_for_path {
            return false;
        }
        // `Real maxSpeed = calculateMaxBlockedSpeed(other)` (`:1461`), kept if it is the lowest (`:1462-1464`).
        let max_speed = self.calculate_max_blocked_speed(&a, &b);
        self.with_ai(me, |ai| {
            if max_speed < ai.cur_max_blocked_speed {
                ai.cur_max_blocked_speed = max_speed;
            }
        });
        // `if (!aiOther->isMovingAwayFrom(getObject()))` (`:1466`).
        if !self.is_moving_away_from(other, me) && b.infantry && !a.infantry {
            // Busy units are not told to move out of the way (`:1470-1475`).
            let busy = self.objects.get(&other).is_some_and(|o| {
                o.status_mask(frame) & crate::object::status::USING_ABILITY != 0 || o.ai.machine.current == crate::ai_states::AiStateType::Busy
            });
            if busy {
                return false;
            }
            // `aiOther->aiMoveAwayFromUnit(getObject(), CMD_FROM_AI)` (`:1476`).
            self.ai_move_away_from_unit(other, me);
            return false;
        }
        // `if (m_blockedFrames==0) m_blockedFrames = 1;` (`:1503`).
        self.with_ai(me, |ai| {
            if ai.blocked_frames == 0 {
                ai.blocked_frames = 1;
            }
        });
        if !self.need_to_rotate(me) {
            // "If we are already pointing in the right direction, we may be stuck" (`:1507-1512`).
            if !other_moving {
                self.with_ai(me, |ai| ai.is_blocked_and_stuck = true);
                return false;
            }
            // See if other is blocked by us (`:1513-1528`): a deadlock, the lower-priority unit gets out of the way.
            if self.unit_blocked_by(other, me) && !self.need_to_rotate(other) && !self.has_higher_path_priority(me, other) {
                self.ai_move_away_from_unit(me, other);
            }
        } else {
            // "We are rotating, so don't accumulate blocked frames" (`:1536-1540`).
            self.with_ai(me, |ai| ai.blocked_frames = 1);
        }
        false
    }

    /// Runs `f` on the AI state of `id`, if the object is alive in the table.
    fn with_ai(&mut self, id: ObjectId, f: impl FnOnce(&mut crate::ai::AiState)) {
        if let Some(o) = self.objects.get_mut(&id) {
            f(&mut o.ai);
        }
    }

    /// `AIUpdateInterface::isMovingAwayFrom(obj)` (`AIUpdate.cpp:3181`): `id` is moving out of the way of `obj`
    /// (its temporary `AI_MOVE_OUT_OF_THE_WAY` state names `obj`).
    fn is_moving_away_from(&self, id: ObjectId, obj: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| {
            o.ai.machine.temporary == Some(crate::ai_states::AiStateType::MoveOutOfTheWay)
                && (o.ai.move_out_of_way[0] == obj || o.ai.move_out_of_way[1] == obj)
        })
    }

    /// `AIUpdateInterface::hasHigherPathPriority` (`AIUpdate.cpp:1208-1245`): whether `me` takes priority over
    /// `other` when they are in each other's way. Dozers first, then vehicles over infantry, then the
    /// direction the units face; the object id breaks ties.
    pub(crate) fn has_higher_path_priority(&self, me: ObjectId, other: ObjectId) -> bool {
        use crate::template::kind_of::{DOZER, INFANTRY, VEHICLE};
        let (Some(a), Some(b)) = (self.objects.get(&me), self.objects.get(&other)) else { return false };
        let (a_dozer, b_dozer) = (a.is_kind_of(DOZER), b.is_kind_of(DOZER));
        if a_dozer && !b_dozer {
            return true;
        }
        if !a_dozer && b_dozer {
            return false;
        }
        let (a_veh, b_inf) = (a.is_kind_of(VEHICLE), b.is_kind_of(INFANTRY));
        let (a_inf, b_veh) = (a.is_kind_of(INFANTRY), b.is_kind_of(VEHICLE));
        if a_veh && b_inf {
            return true;
        }
        if a_inf && b_veh {
            return false;
        }
        // "The paths aren't of the same group, so see which unit is in front" (`:1229-1245`).
        let (our_x, our_y) = (cos(a.angle), sin(a.angle));
        let (their_x, their_y) = (cos(b.angle), sin(b.angle));
        if our_x * their_x + our_y * their_y <= 0.0 {
            return me.0 < other.0;
        }
        let (comb_x, comb_y) = (our_x + their_x, our_y + their_y);
        let (to_x, to_y) = (b.position.x - a.position.x, b.position.y - a.position.y);
        let dot = comb_x * to_x + comb_y * to_y;
        if dot > 0.0 {
            return false; // other is ahead of us along our directional vector
        }
        if dot < 0.0 {
            return true; // we are ahead of other
        }
        me.0 < other.0
    }

    /// `AIUpdateInterface::needToRotate` (`AIUpdate.cpp:1400-1426`): the unit is not yet pointing along its path
    /// (`|deltaAngle| > PI/30` to the point on the path). A waiting unit always needs to rotate. Wanderers
    /// (`getWanderWidthFactor() > 0`) are not ported (no wander in the locomotor).
    pub(crate) fn need_to_rotate(&self, id: ObjectId) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        if o.ai.waiting_for_path {
            return true;
        }
        let mut delta = 0.0f32;
        if let Some(raw) = o.ai.raw_path.as_ref() {
            // `computePointOnPath` (`:1411`); with no segment the point is the path's last node (`AIPathfind.cpp:767`).
            let point = PathObject::for_object(o)
                .and_then(|po| raw.compute_point_on_path(&self.pathfinder, &po, &o.position))
                .map(|info| info.pos_on_path)
                .or_else(|| raw.optimized_positions().last().copied());
            if let Some(p) = point {
                let desired = atan2(p.y - o.position.y, p.x - o.position.x);
                delta = angle_diff(desired, o.angle);
            }
        }
        delta.abs() > PI / 30.0
    }

    /// `AIUpdateInterface::privateMoveAwayFromUnit` (`AIUpdate.cpp:3210-3345`): route `id` off `other`'s
    /// path (`getMoveAwayFromPath`) and run the temporary `AI_MOVE_OUT_OF_THE_WAY` state on it.
    pub(crate) fn ai_move_away_from_unit(&mut self, id: ObjectId, other: ObjectId) {
        let frame = self.frame;
        let Some(a) = self.objects.get(&id) else { return };
        // `privateMoveAwayFromUnit`'s guard (AIUpdate.cpp:3215): `isAiInDeadState() || getObject()->isMobile() == FALSE ||
        // !isAllowedToMoveAwayFromUnit()`. `isMobile` (Object.cpp:2947-2956) is "not KINDOF_IMMOBILE and not `isDisabled()`".
        // `isAiInDeadState()` is `m_isAiDead`, set by `markAsDead` (AIUpdate.cpp:1193-1198), which is the port's
        // `effectively_dead` (`mark_effectively_dead`, die.rs). Lane 187 (R24-8): the port's `is_dying()` also counts zero
        // health before the die pipeline has run, which C++ does not.
        // `isAllowedToMoveAwayFromUnit` is true for the base unit (AIUpdate.h:340); a jet is refused while it is parked
        // (no `ALLOW_AIR_LOCO`), taking off or landing (JetAIUpdate.cpp:2409-2416).
        let jet_refuses = a.template.jet.is_some()
            && (!a.jet.flag(crate::jet_ai::jet_flag::ALLOW_AIR_LOCO)
                || a.jet.flag(crate::jet_ai::jet_flag::TAKEOFF_IN_PROGRESS)
                || a.jet.flag(crate::jet_ai::jet_flag::LANDING_IN_PROGRESS));
        // `a.ai.suspended` is NOT a C++ test: it is the possession-only flag (`possession.rs`, set when a player takes
        // the unit over, docs/POSSESSION.md). A possessed unit is steered by its player, so an ally's push must not
        // move it. Port-only addition, documented per AGENTS.md; with the feature off it is never set.
        if a.destroyed
            || a.effectively_dead
            || a.ai.suspended
            || a.ai_handle.is_none()
            || a.is_kind_of(crate::template::kind_of::IMMOBILE)
            || a.is_disabled()
            || jet_refuses
        {
            return;
        }
        // `if (m_stateMachine->getTemporaryState() == AI_MOVE_OUT_OF_THE_WAY)` (`:3300-3313`): already
        // stepping aside for this unit (or the previous one) — cheat collisions for 2 s, no new path.
        if a.ai.machine.temporary == Some(crate::ai_states::AiStateType::MoveOutOfTheWay)
            && (a.ai.move_out_of_way[0] == other || a.ai.move_out_of_way[1] == other)
        {
            if a.ai.is_blocked {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.ignore_collisions_until = frame + 2 * crate::math::LOGICFRAMES_PER_SECOND;
                }
            }
            return;
        }
        // `m_moveOutOfWay2 = m_moveOutOfWay1; m_moveOutOfWay1 = id` (`:3316-3317`): both ids are
        // recorded *before* the search, so they stay recorded when it fails (the next call for the
        // same unit then takes the cheat branch above).
        let previous = a.ai.move_out_of_way[0];
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.move_out_of_way[1] = o.ai.move_out_of_way[0];
            o.ai.move_out_of_way[0] = other;
        }
        // `getMoveAwayFromPath(obj, unit, unitPath, obj2, path2)`: avoid the other's path and the path
        // of the unit avoided last time (`Object *obj2 = findObjectByID(m_moveOutOfWay2)`, `:3318-3324`).
        let mut avoid: Vec<Coord3D> = Vec::new();
        let mut avoid2: Vec<Coord3D> = Vec::new();
        let (from, mut po, other_radius, other_center) = {
            let Some(a) = self.objects.get(&id) else { return };
            let Some(b) = self.objects.get(&other) else { return };
            let Some(po) = PathObject::for_object(a) else { return };
            if let Some(p) = b.ai.raw_path.as_ref() {
                avoid = p.optimized_positions();
            }
            if let Some(b2) = self.objects.get(&previous) {
                if let Some(p) = b2.ai.raw_path.as_ref() {
                    avoid2 = p.optimized_positions();
                }
            }
            let (orr, oc) = PathObject::for_object(b).map(|o| o.radius_and_center()).unwrap_or((0, false));
            (a.position, po, orr, oc)
        };
        // `Path *unitPath = unit->getAI()->getPath(); if (unitPath == nullptr) return;` (`:3325-3326`):
        // only a missing path stops the search — a one-node path still runs it (`getPath()` is the
        // `Path` object, which `computeQuickPath` builds even for a single destination).
        let unit_path_exists = {
            let Some(b) = self.objects.get(&other) else { return };
            b.ai.raw_path.is_some() || !b.ai.path.is_empty()
        };
        if !unit_path_exists {
            return;
        }
        let terrain = &self.terrain;
        let mut new_path = self.pathfinder.move_away_from_unit(terrain, &po, &from, &avoid, &avoid2, other_radius, other_center);
        if new_path.is_none() && !po.can_path_through_units {
            // `setCanPathThroughUnits(TRUE)` (`:3330-3333`) then the same search again. The flag is
            // left set whatever the retry does (the engine never clears it here).
            po.can_path_through_units = true;
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.can_path_through_units = true;
            }
            new_path = self.pathfinder.move_away_from_unit(terrain, &po, &from, &avoid, &avoid2, other_radius, other_center);
        }
        let Some(path) = new_path else { return };
        // `destroyPath(); m_path = newPath;` (`:3333-3335`).
        // `AIUpdate.cpp:3259-3263` writes only `m_path` here; no state's `m_pathGoalPosition` changes. The move-away's
        // own `m_pathGoalPosition` is set by its enter (`AIStates.cpp:2109`), so the main move keeps its own.
        let waypoints = path.waypoints();
        // `destroyPath()` (AIUpdate.cpp:3333, body :2018-2028): the old path goes, `m_waitingForPath` clears and the
        // locomotor goal becomes NONE. Then `m_path = newPath` (:3334). Neither the path timestamp (lane 185, R23-14) nor
        // the blocked-and-stuck flag is written by this install (R24-4): `privateMoveAwayFromUnit` has no such writes.
        self.destroy_ai_path(id);
        {
            let Some(o) = self.objects.get_mut(&id) else { return };
            o.ai.flight_goal = None;
            o.ai.path = waypoints;
            o.ai.raw_path = Some(path.clone());
        }
        // `wakeUpNow()` (AIUpdate.cpp:3262).
        self.wake_up_now(id);
        // `m_stateMachine->setTemporaryState(AI_MOVE_OUT_OF_THE_WAY, 10*LOGICFRAMES_PER_SECOND)`
        // (`:3337`).
        self.ai_set_temporary_state(id, crate::ai_states::AiStateType::MoveOutOfTheWay, 10 * crate::math::LOGICFRAMES_PER_SECOND);
        // `if (m_path) { if (!KINDOF_NO_COLLIDE) TheAI->pathfinder()->moveAllies(getObject(), m_path); }`
        // (AIUpdate.cpp:3264-3268): the pushed unit's own new path pushes its allies, one level deeper.
        let no_collide = self.objects.get(&id).is_some_and(|o| o.is_kind_of(crate::template::kind_of::NO_COLLIDE));
        if !no_collide {
            self.move_allies(id, &path);
        }
    }

    /// `AIUpdateInterface::calculateMaxBlockedSpeed` (`AIUpdate.cpp:1251`): the speed cap so the
    /// mover does not run into a blocking unit that is moving away.
    pub(crate) fn calculate_max_blocked_speed(&self, me: &CollisionUnit, other: &CollisionUnit) -> f32 {
        // `m_curMaxBlockedSpeed`: the early exits return it, and the result is capped by it (`:1257-1296`).
        let cur = self.objects.get(&me.id).map_or(FAST_AS_POSSIBLE, |o| o.ai.cur_max_blocked_speed);
        let (our_dx, our_dy) = (cos(me.angle), sin(me.angle));
        let (other_dx, other_dy) = (cos(other.angle), sin(other.angle));
        let (mut vx, mut vy) = (other.pos.x - me.pos.x, other.pos.y - me.pos.y);
        let len = sqrt(vx * vx + vy * vy);
        if len > 0.0 {
            vx /= len;
            vy /= len;
        }
        let mut dot = vx * other_dx + vy * other_dy;
        if dot < 0.0 {
            return 0.0; // They are running into us.
        }
        // `if (!otherPhysics) return m_curMaxBlockedSpeed;` (`:1266`).
        let Some(other_phys) = self.objects.get(&other.id).and_then(|o| o.physics.as_ref()) else { return cur };
        // `otherVel.z = 0; awaySpeed = otherVel.length() * speedFactor` (`:1268-1273`).
        let other_speed = sqrt(other_phys.velocity.x * other_phys.velocity.x + other_phys.velocity.y * other_phys.velocity.y);
        let away_speed = other_speed * dot;
        // "Unexpected - we are moving away. Shouldn't be blocked..." (`:1280-1283`).
        dot = vx * our_dx + vy * our_dy;
        if dot <= 0.0 {
            return cur;
        }
        let max_speed = away_speed / dot;
        if max_speed > cur { cur } else { max_speed }
    }
}

/// Angle from `from`'s facing to `to`, signed (`PartitionManager::getRelativeAngle2D`).
fn relative_angle(from: &CollisionUnit, to_pos: &Coord3D) -> f32 {
    let desired = atan2(to_pos.y - from.pos.y, to_pos.x - from.pos.x);
    angle_diff(desired, from.angle)
}

/// `AIUpdateInterface::blockedBy` (`AIUpdate.cpp:1289-1389`), with `hasHigherPathPriority(aiOther)` passed in as
/// `me_higher` (the caller reads it from the objects, see [`GameLogic::has_higher_path_priority`]).
fn blocked_by_pri(me: &CollisionUnit, other: &CollisionUnit, me_higher: bool) -> bool {
    // Near the final goal, ignore obstacles (`AIUpdate.cpp:1295-1302`): only when the pathfind goal cell is set
    // (`goalCell.x>0 && goalCell.y>0`), and measured from the state machine's goal position, not the goal cell.
    if me.goal_cell_valid {
        let goal = me.machine_goal;
        if (goal.x - me.pos.x).abs() < PATHFIND_CELL_SIZE_F && (goal.y - me.pos.y).abs() < PATHFIND_CELL_SIZE_F {
            return false;
        }
    }
    // `canCrush = obj->canCrushOrSquish(other, TEST_CRUSH_OR_SQUISH)` (`AIUpdate.cpp:1304`) is checked by the
    // caller with the objects' crush levels (`GameLogic::can_crush_or_squish`).
    // `if (!aiOther->isDoingGroundMovement()) return FALSE` (`:1310`).
    if !other.ground {
        return false;
    }
    // `INFANTRY_MOVES_THROUGH_INFANTRY` is defined unconditionally (`Core/.../AIPathfind.h:56`), so
    // infantry run through infantry (`AIUpdate.cpp:1327`).
    if me.infantry && other.infantry {
        return false;
    }
    let dx = me.pos.x - other.pos.x;
    let dy = me.pos.y - other.pos.y;
    let cur_dsq = dx * dx + dy * dy;
    if cur_dsq < PATHFIND_CELL_SIZE_F * PATHFIND_CELL_SIZE_F * 0.0001 {
        // "Somehow 2 units ended up on the same grid": lowest path priority wins (`:1346`).
        return me_higher;
    }
    // `dotProduct` of the unit direction vectors (`:1349-1352`).
    let (our_dx, our_dy) = (cos(me.angle), sin(me.angle));
    let (their_dx, their_dy) = (cos(other.angle), sin(other.angle));
    let dot = our_dx * their_dx + our_dy * their_dy;
    if me.blocked_frames as u32 > crate::math::LOGICFRAMES_PER_SECOND && dot <= 0.0 {
        return false; // crossing after a while: move through (`:1357-1360`)
    }
    let collision_angle = relative_angle(me, &other.pos);
    let other_angle = relative_angle(other, &me.pos);
    let mut angle_limit = PI / 4.0;
    if !other.loco_goal {
        angle_limit *= 0.75;
    }
    if !(-PI / 2.0..=PI / 2.0).contains(&collision_angle) {
        return false; // moving away (`:1366-1367`)
    }
    if collision_angle > angle_limit || collision_angle < -angle_limit {
        if dot <= 0.0 {
            return false;
        }
        if other.loco_goal && (other_angle > angle_limit || other_angle < -angle_limit) {
            // "See if we're running into each other" (`:1377-1392`): the higher priority keeps going.
            let ndx = dx + our_dx - their_dx;
            let ndy = dy + our_dy - their_dy;
            if cur_dsq > ndx * ndx + ndy * ndy {
                if me_higher {
                    return false;
                }
            } else {
                return false;
            }
        } else {
            return false;
        }
    }
    // `if (!aiOther->isAiInDeadState()) return TRUE` (`:1397-1398`).
    !other.dead_state
}

/// [`blocked_by_pri`] with the priority read from the two ids (the test helper's stand-in for
/// `hasHigherPathPriority`, which needs the objects).
#[cfg(test)]
fn blocked_by(me: &CollisionUnit, other: &CollisionUnit) -> bool {
    blocked_by_pri(me, other, me.id.0 < other.id.0)
}

/// The `LocomotorParams` a unit uses right now (for tests/possession).
pub fn current_locomotor(template: &crate::template::ObjectTemplate, set: crate::template::LocomotorSet) -> Option<LocomotorParams> {
    template.locomotor_for(set)
}

#[cfg(test)]
mod quick_path_and_move_away_tests {
    //! `AIUpdateInterface::computeQuickPath` (`AIUpdate.cpp:1676-1721`) and
    //! `privateMoveAwayFromUnit` -> `AIMoveOutOfTheWayState` (`AIUpdate.cpp:3210-3345`,
    //! `AIStates.cpp:2149-2183`). AN3 s1 worker 229: C++ walked its production exit line as a real
    //! two-node `Path`, so the move-away search of worker 229 could avoid it and picked cell (128,190);
    //! the port had no `Path` there, found nothing to avoid and returned the start cell.
    use super::*;
    use crate::ai_states::AiStateType;
    use crate::template::{kind_of, ObjectTemplate};
    use std::sync::Arc;

    fn vehicle(name: &str) -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 5.0;
        t.physics = Some(crate::physics::PhysicsTemplate { mass: 10.0, ..crate::physics::PhysicsTemplate::default() });
        t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.2, 0.0556, 0.1, true));
        Arc::new(t)
    }

    fn world() -> GameLogic {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 10000);
        g
    }

    /// `computeQuickPath` builds `new Path; prependNode(destination); prependNode(obj position)` and
    /// points the first node at the second (`AIUpdate.cpp:1706-1711`), so `getPath()` — and with it
    /// every `getMoveAwayFromPath` reading another unit's path — holds a real two-node path, not just
    /// the destination. A second call for the same destination keeps the path (`:1682-1696`).
    #[test]
    fn a_quick_path_holds_the_start_and_the_destination() {
        let mut g = world();
        let id = g.create_object(&vehicle("Mover"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let dest = Coord3D::new(160.0, 140.0, 0.0);
        assert!(g.compute_quick_path(id, &dest));
        let o = g.objects.get(&id).unwrap();
        let raw = o.ai.raw_path.as_ref().expect("a quick path is a real Path");
        let pts = raw.optimized_positions();
        assert_eq!(pts.len(), 2, "start and destination: {pts:?}");
        assert_eq!((pts[0].x, pts[0].y), (100.0, 100.0), "the first node is the unit's position");
        assert_eq!((pts[1].x, pts[1].y), (160.0, 140.0), "the last node is the destination");
        assert_eq!(o.ai.path.len(), 1, "the waypoints are the destination alone");
        // "First, see if our path already goes to the destination": within 0.25 the path is kept.
        let stamp = o.ai.path_timestamp;
        g.frame = stamp + 5;
        assert!(g.compute_quick_path(id, &Coord3D::new(160.1, 140.1, 0.0)));
        assert_eq!(g.objects.get(&id).unwrap().ai.path_timestamp, stamp, "the old path is kept");
    }

    /// `privateMoveAwayFromUnit` installs the new path and runs the temporary `AI_MOVE_OUT_OF_THE_WAY`
    /// state (`AIUpdate.cpp:3333-3341`), whose `update` is `AIInternalMoveToState::update`: the move
    /// ends when the locomotor distance to the goal drops under `CloseEnoughDist`, then `onExit`
    /// destroys the path and the unit stops dead (no drifting onto the goal cell and no jitter).
    #[test]
    fn a_unit_pushed_aside_walks_the_move_away_path_and_then_idles() {
        let mut g = world();
        let mover = g.create_object(&vehicle("Mover"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let other = g.create_object(&vehicle("Other"), Coord3D::new(110.0, 100.0, 0.0), 0.0, 0);
        // The other unit's path runs east through the mover's cell: a quick path, start + destination.
        let dest = Coord3D::new(200.0, 100.0, 0.0);
        assert!(g.compute_quick_path(other, &dest));
        g.ai_move_away_from_unit(mover, other);
        let o = g.objects.get(&mover).unwrap();
        assert_eq!(o.ai.machine.temporary, Some(AiStateType::MoveOutOfTheWay), "the move-away is a temporary state");
        assert!(o.ai.raw_path.is_some(), "the move-away path is a real Path");
        let goal = o.ai.path.last().copied().expect("a move-away goal");
        assert!(goal.dist_2d(&Coord3D::new(100.0, 100.0, 0.0)) >= 10.0, "it moves at least a cell: {goal:?}");
        let mut frames = 0;
        while g.objects.get(&mover).unwrap().ai.machine.temporary.is_some() && frames < 300 {
            g.run_frames(1);
            frames += 1;
        }
        assert!(frames < 300, "the move-away state ends");
        let o = g.objects.get(&mover).unwrap();
        assert!(o.ai.path.is_empty(), "the path is destroyed at the end of the move");
        assert_eq!(o.ai.locomotor_goal_type, LocoGoalType::None);
        let rest = o.position;
        // `CloseEnoughDist` stops it short of the cell centre; the old path-only port walked to the
        // centre and then jittered around it for dozens of frames.
        assert!(rest.dist_2d(&goal) > 0.2, "it stops at the locomotor's close-enough distance, not on the node");
        assert!(rest.dist_2d(&Coord3D::new(110.0, 100.0, 0.0)) > 10.0, "it clears the other unit's line");
        // `AIMoveOutOfTheWayState::onExit` (`AIStates.cpp:2188-2196`): `clearMoveOutOfWay()` and
        // `setCanPathThroughUnits(false)`.
        assert_eq!(o.ai.move_out_of_way, [ObjectId::INVALID, ObjectId::INVALID], "the exit clears the avoided ids");
        assert!(!o.ai.can_path_through_units, "and clears setCanPathThroughUnits");
        for _ in 0..10 {
            g.run_frames(1);
        }
        assert_eq!(g.objects.get(&mover).unwrap().position, rest, "and it does not drift afterwards");
    }

    /// `m_moveOutOfWay2 = m_moveOutOfWay1; m_moveOutOfWay1 = id` (`AIUpdate.cpp:3316-3317`) happens
    /// *before* the search, so both ids are recorded even when the search does not run or fails: the
    /// `if (unitPath == nullptr) return;` (`:3326`) and a failed `getMoveAwayFromPath` both keep them.
    #[test]
    fn the_move_away_ids_are_recorded_before_the_search() {
        let mut g = world();
        let mover = g.create_object(&vehicle("Mover"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let a = g.create_object(&vehicle("A"), Coord3D::new(110.0, 100.0, 0.0), 0.0, 0);
        let b = g.create_object(&vehicle("B"), Coord3D::new(120.0, 100.0, 0.0), 0.0, 0);
        assert!(g.compute_quick_path(a, &Coord3D::new(200.0, 100.0, 0.0)));
        g.ai_move_away_from_unit(mover, a);
        let o = g.objects.get(&mover).unwrap();
        assert_eq!(o.ai.move_out_of_way, [a, ObjectId::INVALID], "the first step-aside records its unit");
        // `b` has no path at all, so the search never runs — but the ids still shift.
        g.ai_move_away_from_unit(mover, b);
        let o = g.objects.get(&mover).unwrap();
        assert_eq!(o.ai.move_out_of_way, [b, a], "the ids are recorded before the `unitPath == nullptr` return");
    }

    /// `Path *unitPath = unit->getAI()->getPath(); if (unitPath == nullptr) return;` (`AIUpdate.cpp:3325-3326`):
    /// only a *missing* path stops the search — `getPath()` is the `Path` object, which
    /// `computeQuickPath` builds (`[start, destination]`, `AIUpdate.cpp:1641-1650`) even when the unit is
    /// standing on its destination. The port's old test was `avoid.len() < 2`, which also skipped a unit
    /// whose path holds one optimised node.
    #[test]
    fn a_one_node_path_still_runs_the_move_away_search() {
        let mut g = world();
        let mover = g.create_object(&vehicle("Mover"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let other = g.create_object(&vehicle("Other"), Coord3D::new(110.0, 100.0, 0.0), 0.0, 0);
        // One *waypoint* (the destination) ahead of the start node, and a real `Path` behind it.
        g.compute_quick_path(other, &Coord3D::new(200.0, 100.0, 0.0));
        let o = g.objects.get(&other).unwrap();
        assert_eq!(o.ai.path.len(), 1, "the optimised chain after the start node is the destination");
        assert_eq!(o.ai.raw_path.as_ref().map(|p| p.nodes.len()), Some(2), "a two-node `Path` (`getPath() != nullptr`)");
        g.ai_move_away_from_unit(mover, other);
        assert_eq!(
            g.objects.get(&mover).unwrap().ai.machine.temporary,
            Some(AiStateType::MoveOutOfTheWay),
            "a one-waypoint path is not `unitPath == nullptr`"
        );
    }

    /// `AIUpdateInterface::requestPath` (`AIUpdate.cpp:480-521`) never touches `m_locomotorGoalType`:
    /// only a computed path sets it (`computePath`'s `if (theNewPath) ... setLocomotorGoalPositionOnPath()`,
    /// `AIUpdate.cpp:1740-1745`), and `AIInternalMoveToState::update` re-sets it while a path exists
    /// (`AIStates.cpp:1875-1876`). With the goal still `NONE` and no path, `doLocomotor` maintains position
    /// (`AIUpdate.cpp:2306`), which for a treaded unit scrubs its 2D velocity dead
    /// (`Locomotor.cpp:2616-2626`) instead of coasting: the US dozer at AN3 s1 f1024 must stop where the
    /// engine does. The old code set `POSITION_ON_PATH` in `start_move`, so the pending-path frames kept
    /// the residual velocity and the physics stayed awake (a different sleepy-heap layout).
    #[test]
    fn a_move_request_leaves_the_locomotor_goal_alone_until_a_path_is_computed() {
        let mut g = world();
        let id = g.create_object(&vehicle("Mover"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        // The move finishes and a new one is requested while no path exists yet (`requestPath`).
        g.objects.get_mut(&id).unwrap().ai.locomotor_goal_type = LocoGoalType::None;
        g.objects.get_mut(&id).unwrap().ai.start_move(Coord3D::new(160.0, 100.0, 0.0), PathRequest::Move, true);
        assert_eq!(
            g.objects.get(&id).unwrap().ai.locomotor_goal_type,
            LocoGoalType::None,
            "a queued path request must not turn the locomotor on"
        );
        // `doLocomotor` with `NONE` and no path maintains position, so nothing moves and no motive force is keyed.
        g.ai_do_locomotor(id);
        let o = g.objects.get(&id).unwrap();
        assert_eq!(o.ai.locomotor_goal_type, LocoGoalType::None);
        assert_eq!(o.position, Coord3D::new(100.0, 100.0, 0.0), "a pending path does not move the unit");

        // Once `computePath` installs the path, the goal follows it (`setLocomotorGoalPositionOnPath`).
        let po = crate::pathfind::PathObject::for_template(&o.template, id, o.player).expect("ground unit");
        let path = Path {
            nodes: vec![
                PathNode { pos: Coord3D::new(100.0, 100.0, 0.0), can_optimize: true, next_optimized: Some(1) },
                PathNode { pos: Coord3D::new(160.0, 100.0, 0.0), can_optimize: true, next_optimized: None },
            ],
            blocked_by_ally: false,
        };
        let _ = &po;
        g.store_ai_path(id, Some(path), false);
        g.ai_goal_on_path_if_any(id);
        assert_eq!(
            g.objects.get(&id).unwrap().ai.locomotor_goal_type,
            LocoGoalType::PositionOnPath,
            "a computed path keys POSITION_ON_PATH like `computePath`"
        );
    }

    /// `doPathfind`'s failed attack path falls back to `computePath` with the victim's position first
    /// run through `adjustToPossibleDestination` (`AIUpdate.cpp:450-457`), and `computePath` does not
    /// adjust again (its own `adjustDestination` call is the AI state's job). The recorded goal is the
    /// adjusted cell, which is the path's last node; with the old direct `compute_ai_path(target)`
    /// call it stayed the victim's raw position.
    #[test]
    fn a_failed_attack_path_adjusts_to_a_possible_destination() {
        use crate::template::kind_of;
        let mut g = world();
        let id = g.create_object(&vehicle("Attacker"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        // The victim stands on a structure, so its own position is not a possible destination and
        // `adjustToPossibleDestination` has to spiral out to a clear cell.
        let mut block = ObjectTemplate::new("Block");
        block.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        block.geometry.major_radius = 10.0;
        block.geometry.minor_radius = 10.0;
        let block = std::sync::Arc::new(block);
        let vpos = Coord3D::new(123.456, 178.901, 0.0);
        g.create_object(&block, vpos, 0.0, 0);
        let victim = g.create_object(&vehicle("Victim"), vpos, 0.0, 1);
        {
            let o = g.objects.get_mut(&id).unwrap();
            o.ai.request = PathRequest::Attack;
            o.ai.requested_victim = victim;
            o.ai.requested_destination = vpos;
            // The vehicle has no weapon, so `computeAttackPath`'s weapon range is 0 and no cell is ever "in range" (the
            // search fails, as in C++). The range is read at compute time, not stored with the request.
            o.ai.is_final_goal = true; // `requestPath`'s `isFinalGoal` (`AIUpdate.cpp:459`)
            // A request never adjusts the goal again (`ai_request_path` clears it, `AIUpdate.cpp:479-516`).
            o.ai.adjusts_destination = false;
            o.ai.waiting_for_path = true;
        }
        g.do_pathfind(id);
        let o = g.objects.get(&id).unwrap();
        // `computePath` writes no state-level `m_pathGoalPosition` (`AIUpdate.cpp:1767-1771`: only the timestamp and
        // the blocked flags); the adjusted goal is the path's last node, as `doPathfind`'s `updateGoal(lastNode)` uses.
        let goal = o.ai.raw_path.as_ref().and_then(|p| p.nodes.last()).map(|n| n.pos).expect("a path");
        assert!(
            (goal.x - vpos.x).abs() > 0.001 || (goal.y - vpos.y).abs() > 0.001,
            "the goal is the adjusted cell, not the victim's raw position: {goal:?} vs {vpos:?}"
        );
        // `adjustCoordToCell` output for this centred footprint: `(cell + 0.5) * 10`. The end is
        // `computePath`'s closest-reachable cell (`findClosestPath`, `AIUpdate.cpp:1728-1733`), which the search writes
        // into `dest` (`compute_ai_path`); `computePath` writes no state-level `m_pathGoalPosition` (`:1767-1771`).
        assert_eq!((goal.x / 10.0 - 0.5, goal.y / 10.0 - 0.5), (11.0, 15.0), "the goal is a cell centre: {goal:?}");
        // `doPathfind`'s `updateGoal(getPath()->getLastNode())` records the same goal in the pathfinder.
        let recorded = g.ai_pathfind_goal(id).expect("the path end is the goal cell");
        let last = o.ai.raw_path.as_ref().and_then(|p| p.nodes.last()).map(|n| n.pos).expect("a path");
        assert_eq!((recorded.x, recorded.y), (last.x, last.y), "the goal cell is the path's last node");
    }

    /// `computePath` ends with `m_pathTimestamp = now; m_blockedFrames = 0; m_isBlockedAndStuck =
    /// FALSE` (`AIUpdate.cpp:1840-1844`), so a repath clears the blocked counter; `store_ai_path`
    /// used to keep it.
    #[test]
    fn storing_a_new_path_zeroes_the_blocked_frames() {
        let mut g = world();
        let id = g.create_object(&vehicle("Mover"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        {
            let o = g.objects.get_mut(&id).unwrap();
            o.ai.request = PathRequest::Move;
            o.ai.requested_destination = Coord3D::new(200.0, 100.0, 0.0);
            o.ai.move_goal = Some(Coord3D::new(200.0, 100.0, 0.0));
            o.ai.waiting_for_path = true;
            o.ai.blocked_frames = 5;
        }
        g.do_pathfind(id);
        let o = g.objects.get(&id).unwrap();
        assert!(o.ai.raw_path.is_some(), "the request produced a path");
        assert_eq!(o.ai.blocked_frames, 0, "computePath's trailing clear zeroes the counter");
    }

    /// `setCanPathThroughUnits(TRUE)` before the retry (`AIUpdate.cpp:3330-3333`): the flag is left
    /// set whatever the retry does (the engine never clears it there), so a unit whose step-aside
    /// search failed permanently stops treating allies as blocking (`AIPathfind.cpp:6419`).
    #[test]
    fn a_failed_move_away_search_leaves_can_path_through_units_set() {
        // A degenerate, cell-less pathfinder grid: `getMoveAwayFromPath` finds no start cell
        // (`getClippedCell`), so both the first search and the retry fail.
        let mut g = GameLogic::new(TerrainLogic::flat(-2, -2, 0), 1);
        g.add_player(0, 10000);
        let mover = g.create_object(&vehicle("Mover"), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let other = g.create_object(&vehicle("Other"), Coord3D::new(210.0, 200.0, 0.0), 0.0, 0);
        assert!(g.compute_quick_path(other, &Coord3D::new(300.0, 200.0, 0.0)));
        g.ai_move_away_from_unit(mover, other);
        let o = g.objects.get(&mover).unwrap();
        assert!(o.ai.raw_path.is_none(), "the step-aside search failed: goal {:?}", o.ai.path.last());
        assert_eq!(o.ai.machine.temporary, None, "no temporary state without a path");
        assert!(o.ai.can_path_through_units, "the retry's setCanPathThroughUnits(TRUE) stays set");
        assert_eq!(o.ai.move_out_of_way, [other, ObjectId::INVALID], "the id is recorded on failure too");
    }

    /// `AIGroup::computeIndividualDestination` hands the group destination to the one `adjustDestination` of a group
    /// move (`AIGroup.cpp:546`). A repath calls `findPath` for the goal alone (`AIUpdate.cpp:1726`), so a later
    /// `requestPath` carries no group destination. Revert check: without the spend in `do_pathfind` the machine's
    /// copy is re-applied by the repath request, and `ai.group_dest` is `Some` again.
    #[test]
    fn a_repath_does_not_reuse_the_group_destination() {
        let mut g = world();
        let id = g.create_object(&vehicle("Mover"), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let goal = Coord3D::new(200.0, 100.0, 0.0);
        let group = Coord3D::new(210.0, 100.0, 0.0);
        g.objects.get_mut(&id).unwrap().ai.machine.group_dest = Some(group);
        g.ai_request_path(id, goal, true);
        assert_eq!(g.objects.get(&id).unwrap().ai.group_dest, Some(group), "the first request carries the group destination");
        g.do_pathfind(id);
        g.ai_request_path(id, goal, true);
        assert_eq!(g.objects.get(&id).unwrap().ai.group_dest, None, "a repath carries none");
    }

    /// `AIUpdateInterface::doLocomotor` sets `OBJECT_STATUS_AIRBORNE_TARGET` from `getHeightAboveTerrain()`
    /// (`AIUpdate.cpp:2294-2300`), which is `Object::calculateHeightAboveTerrain` (`Object.cpp:2820-2825`): the
    /// height above the surface of the object's layer. A unit 8 up on a bridge deck is 0 above the deck, so it is not
    /// an airborne target with a 5-unit threshold; a flier 12 above the deck is. Revert check: measuring from the
    /// ground (0) flags the deck unit.
    #[test]
    fn a_unit_on_a_bridge_deck_is_not_an_airborne_target() {
        use crate::bridge::Bridge;
        let mut terrain = TerrainLogic::flat(400, 400, 0);
        terrain.add_bridge(Bridge::new(Coord3D::new(0.0, 100.0, 8.0), Coord3D::new(400.0, 100.0, 8.0), 20.0, 2));
        let mut g = GameLogic::new(terrain, 1);
        g.add_player(0, 10000);
        let deck = g.create_object(&vehicle("Deck"), Coord3D::new(200.0, 100.0, 0.0), 0.0, 0);
        let flier = g.create_object(&vehicle("Flier"), Coord3D::new(200.0, 100.0, 0.0), 0.0, 0);
        g.objects.get_mut(&deck).unwrap().position.z = 8.0;
        g.objects.get_mut(&flier).unwrap().position.z = 20.0;
        let mut loco = LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.2, 0.0556, 0.1, true);
        loco.airborne_targeting_height = 5.0;
        g.update_airborne_target_status(deck, &loco);
        g.update_airborne_target_status(flier, &loco);
        let bit = crate::object::status::AIRBORNE_TARGET;
        assert_eq!(g.objects.get(&deck).unwrap().status_bits & bit, 0, "on the deck: 0 above the surface");
        assert_ne!(g.objects.get(&flier).unwrap().status_bits & bit, 0, "12 above the deck");
    }
}

#[cfg(test)]
#[path = "ai_l154.rs"]
mod l154_tests;

#[cfg(test)]
#[path = "ai_l158.rs"]
mod l158_tests;

#[cfg(test)]
#[path = "ai_l160.rs"]
mod l160_tests;

#[cfg(test)]
#[path = "ai_l161.rs"]
mod l161_tests;

#[cfg(test)]
#[path = "ai_l163.rs"]
mod l163_tests;

#[cfg(test)]
#[path = "ai_l177b.rs"]
mod l177b_tests;

/// Lane 182: `computePath`'s straight-line quick path (`ai_l182.rs`).
#[cfg(test)]
#[path = "ai_l182.rs"]
mod l182_tests;

/// Lane 184: area guards and the guard entry's attack-exit check (`ai_l184.rs`).
#[cfg(test)]
#[path = "ai_l184.rs"]
mod l184_tests;

/// Lane 183: wave-21 review fixes (`ai_l183.rs`).
#[cfg(test)]
#[path = "ai_l183.rs"]
mod l183_tests;

/// Lane 186: a guard's attack entry in the same frame, the current victim on update, the pursue's non-final request (`ai_l186.rs`).
#[cfg(test)]
#[path = "ai_l186.rs"]
mod l186_tests;

/// Lane 188: the approach entry's `friend_startingMove` before the completed-move block (`ai_l188.rs`).
#[cfg(test)]
#[path = "ai_l188.rs"]
mod l188_tests;

/// Lane 185: `isMoving` sees the `isIdle` subclass overrides (`ai_l185.rs`).
#[cfg(test)]
#[path = "ai_l185.rs"]
mod l185_tests;

/// Lane 189 (R25): area guard centres and the jet guard-area clear (`ai_l189.rs`).
#[cfg(test)]
#[path = "ai_l189.rs"]
mod l189_tests;

/// Lane 190 (R26): the pursue entry, the approach refusals, the attack fallback's ignored obstacle, and the same-ID
/// temporary move (`ai_l190.rs`).
#[cfg(test)]
#[path = "ai_l190.rs"]
mod l190_tests;

/// Lane 191 (AN5 s1 f3129): the guard attack entry keeps the unit's move until the attack path is installed (`ai_l191.rs`).
#[cfg(test)]
#[path = "ai_l191.rs"]
mod l191_tests;

/// Lane 194 round 4 (AN5 s1 f3763): a unit moving backwards is not blocked by the unit in front (`ai_l194.rs`).
#[cfg(test)]
#[path = "ai_l194.rs"]
mod l194_tests;

/// Lane 165: the AI module's disabled-type gate (`ai_l165.rs`).
#[cfg(test)]
#[path = "ai_l165.rs"]
mod l165_tests;

#[cfg(test)]
#[path = "ai_l166.rs"]
mod l166_tests;

#[cfg(test)]
mod blocked_by_tests {
    use super::*;

    /// A ground unit at `(x, 0)` facing `angle`.
    fn unit(id: u32, x: f32, angle: f32, infantry: bool, moving: bool) -> CollisionUnit {
        CollisionUnit {
            id: ObjectId(id),
            pos: Coord3D::new(x, 0.0, 0.0),
            angle,
            infantry,
            ground: true,
            dead_state: false,
            moving,
            waiting_for_path: false,
            blocked_frames: 0,
            machine_goal: Coord3D::ZERO,
            goal_cell_valid: false,
            loco_goal: moving,
        }
    }

    #[test]
    fn a_vehicle_running_into_a_stationary_unit_is_blocked() {
        let me = unit(1, 0.0, 0.0, false, true);
        let other = unit(2, 5.0, 0.0, false, false);
        assert!(blocked_by(&me, &other));
    }

    #[test]
    fn a_vehicle_facing_away_from_the_other_is_not_blocked() {
        // `collisionAngle > PI/2` or `< -PI/2`: "we're moving away" (`AIUpdate.cpp:1376`).
        let me = unit(1, 0.0, std::f32::consts::PI, false, true);
        let other = unit(2, 5.0, 0.0, false, false);
        assert!(!blocked_by(&me, &other));
    }

    #[test]
    fn infantry_run_through_infantry() {
        // `INFANTRY_MOVES_THROUGH_INFANTRY` is defined (`Core/.../AIPathfind.h:56`), so `blockedBy` is
        // FALSE for two infantry whatever their headings (`AIUpdate.cpp:1327`). Same heading, and
        // a head-on approach, both pass.
        let same = unit(2, 5.0, 0.0, true, true);
        assert!(!blocked_by(&unit(1, 0.0, 0.0, true, true), &same));
        let head_on = unit(2, 5.0, std::f32::consts::PI, true, true);
        assert!(!blocked_by(&unit(1, 0.0, 0.0, true, true), &head_on));
    }

    #[test]
    fn a_dead_unit_never_blocks() {
        // `blockedBy` ends with `if (!aiOther->isAiInDeadState()) return TRUE` (`AIUpdate.cpp:1388`).
        let me = unit(1, 0.0, 0.0, false, true);
        let mut other = unit(2, 5.0, 0.0, false, false);
        other.dead_state = true;
        assert!(!blocked_by(&me, &other));
    }
}

#[cfg(test)]
mod locomotor_distance_tests {
    use super::*;
    use crate::pathfind::PathNode;
    use crate::template::{kind_of, ObjectTemplate};
    use std::sync::Arc;

    /// A unit of `kind` at `at` whose move follows `pts` (the raw path, its first node the unit's start)
    /// with the goal set as `doLocomotor` leaves it (`POSITION_ON_PATH`).
    fn unit_on_path(kind: LocomotorKind, at: Coord3D, pts: &[Coord3D], projectile: bool) -> (GameLogic, ObjectId) {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let mut t = ObjectTemplate::new("Unit");
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 5.0;
        t.locomotor = Some(LocomotorParams::from_ini(kind, 60.0, 100.0, 100.0, 360.0));
        if projectile {
            t.kind_of |= kind_of::PROJECTILE;
        }
        let id = g.create_object(&Arc::new(t), at, 0.0, 0);
        let nodes = pts
            .iter()
            .enumerate()
            .map(|(i, p)| PathNode { pos: *p, can_optimize: true, next_optimized: (i + 1 < pts.len()).then_some(i + 1) })
            .collect();
        let raw = Path { nodes, blocked_by_ally: false };
        let o = g.objects.get_mut(&id).expect("unit");
        o.ai.path = raw.waypoints();
        o.ai.raw_path = Some(raw);
        o.ai.locomotor_goal_type = LocoGoalType::PositionOnPath;
        (g, id)
    }

    /// The route a ground unit takes around a wall: 60 up, 10 across, 60 back down (130 long), while the
    /// goal is 10 away in a straight line.
    fn detour() -> [Coord3D; 4] {
        [Coord3D::new(0.0, 0.0, 0.0), Coord3D::new(0.0, 60.0, 0.0), Coord3D::new(10.0, 60.0, 0.0), Coord3D::new(10.0, 0.0, 0.0)]
    }

    #[test]
    fn ground_unit_measures_the_path_when_it_is_longer_than_the_straight_line() {
        let (g, id) = unit_on_path(LocomotorKind::Treads, Coord3D::new(0.0, 0.0, 0.0), &detour(), false);
        let d = g.locomotor_distance_to_goal(id);
        assert!((d - 130.0).abs() < 0.01, "ground distance should be the 130-unit route, got {d}");
    }

    #[test]
    fn ground_unit_on_a_straight_path_measures_the_straight_line() {
        let pts = [Coord3D::new(0.0, 0.0, 0.0), Coord3D::new(100.0, 0.0, 0.0)];
        let (g, id) = unit_on_path(LocomotorKind::Treads, Coord3D::new(0.0, 0.0, 0.0), &pts, false);
        let d = g.locomotor_distance_to_goal(id);
        assert!((d - 100.0).abs() < 0.01, "straight path: got {d}");
    }

    #[test]
    fn ground_unit_under_a_pathfind_cell_from_its_goal_measures_the_straight_line() {
        // A 5-unit straight path is under `PATHFIND_CELL_SIZE_F`: the straight distance decides.
        let pts = [Coord3D::new(0.0, 0.0, 0.0), Coord3D::new(5.0, 0.0, 0.0)];
        let (g, id) = unit_on_path(LocomotorKind::Treads, Coord3D::new(0.0, 0.0, 0.0), &pts, false);
        assert!((g.locomotor_distance_to_goal(id) - 5.0).abs() < 0.01);
    }

    #[test]
    fn aircraft_measure_the_flight_line_capped_by_the_straight_distance() {
        // The flight line runs 60 up, then 10 across: 70 along the way, but the goal is only 10 away, so
        // "if the actual distance is farther, use the actual distance": the straight 10 wins.
        let (g, id) = unit_on_path(LocomotorKind::Thrust, Coord3D::new(0.0, 0.0, 0.0), &detour(), false);
        let d = g.locomotor_distance_to_goal(id);
        assert!((d - 10.0).abs() < 0.01, "air distance should be the straight 10, got {d}");
    }

    #[test]
    fn hover_treats_a_ground_path_as_aircraft() {
        // `getAppearance() == LOCO_HOVER` (`AIUpdate.cpp:2440`): the flight rule applies to a hover.
        let (g, id) = unit_on_path(LocomotorKind::Hover, Coord3D::new(0.0, 0.0, 0.0), &detour(), false);
        assert!((g.locomotor_distance_to_goal(id) - 10.0).abs() < 0.01);
    }

    #[test]
    fn projectile_measures_the_3d_distance_to_the_last_node() {
        // `isKindOf(KINDOF_PROJECTILE)` takes the 3D branch (`AIUpdate.cpp:2468`): sqrt(9 + 16 + 144).
        let pts = [Coord3D::new(0.0, 0.0, 0.0), Coord3D::new(3.0, 4.0, 12.0)];
        let (g, id) = unit_on_path(LocomotorKind::Thrust, Coord3D::new(0.0, 0.0, 0.0), &pts, true);
        assert!((g.locomotor_distance_to_goal(id) - 13.0).abs() < 0.01);
    }

    #[test]
    fn aircraft_flight_distance_starts_at_the_paths_first_node_not_the_unit() {
        // `Path::computeFlightDistToGoal` (AIPathfind.cpp:1018-1027) runs its first segment from `getFirstNode()`
        // (the path's start) to the next node. The unit at (12, 0) has flown past the start of that segment (the goal
        // at (10, 0) is behind it along the path), so the flight distance is 0. Measured from the unit it would be 2.
        // Revert check: `flight_path_start` returns `o.position` and the distance is 2, so the assert fails.
        let pts = [Coord3D::new(0.0, 0.0, 0.0), Coord3D::new(10.0, 0.0, 0.0)];
        let (g, id) = unit_on_path(LocomotorKind::Thrust, Coord3D::new(12.0, 0.0, 0.0), &pts, false);
        let d = g.locomotor_distance_to_goal(id);
        assert!(d.abs() < 0.01, "flown past the first segment: the flight distance is 0, got {d}");
    }

    #[test]
    fn the_flight_distance_query_latches_past_the_segments_flown_past() {
        // `computeFlightDistToGoal` keeps `m_cpopRecentStart` on the path between calls (AIPathfind.cpp:1028-1065),
        // and the distance query calls it (AIUpdate.cpp:2486). Path (0,0),(10,0),(20,0), the unit at (12,0) has flown
        // past (0,0)->(10,0): the query latches (10,0). From (5,5) the only segment left is (10,0)->(20,0), projection 15.
        // Without the latch the (0,0)->(10,0) segment adds 5 as well (sum 20), and the 20 is capped at the straight
        // distance sqrt(250) = 15.81. Revert check: `flight_distance_latch` does nothing, so the second distance is 15.81
        // and the assert fails.
        let pts = [Coord3D::new(0.0, 0.0, 0.0), Coord3D::new(10.0, 0.0, 0.0), Coord3D::new(20.0, 0.0, 0.0)];
        let (mut g, id) = unit_on_path(LocomotorKind::Thrust, Coord3D::new(12.0, 0.0, 0.0), &pts, false);
        g.flight_distance_latch(id);
        g.objects.get_mut(&id).unwrap().position = Coord3D::new(5.0, 5.0, 0.0);
        g.flight_distance_latch(id);
        let d = g.locomotor_distance_to_goal(id);
        assert!((d - 15.0).abs() < 0.01, "the latched segment (10,0)->(20,0) gives 15, got {d}");
    }

    #[test]
    fn an_exit_production_path_passes_through_units_and_ignores_its_factory() {
        // `privateFollowPath(..., exitProduction)` (`AIUpdate.cpp:3402`) and `AIFollowPathState::onEnter`
        // (`AIStates.cpp:3292-3296`): the state, the factory as ignored obstacle, pass-through on.
        let (mut g, id) = unit_on_path(LocomotorKind::Treads, Coord3D::new(0.0, 0.0, 0.0), &detour(), false);
        let factory = ObjectId(9999);
        let path = vec![Coord3D::new(0.0, 60.0, 0.0), Coord3D::new(10.0, 60.0, 0.0)];
        g.ai_follow_exit_production_path(id, path, factory);
        let o = g.objects.get(&id).expect("unit");
        assert_eq!(o.ai.machine.current, crate::ai_states::AiStateType::FollowExitProductionPath);
        assert_eq!(o.ai.ignore_obstacle_id, factory);
        assert!(o.ai.can_path_through_units);
    }

    #[test]
    fn a_goal_that_is_not_a_position_on_path_is_already_there() {
        let (mut g, id) = unit_on_path(LocomotorKind::Treads, Coord3D::new(0.0, 0.0, 0.0), &detour(), false);
        g.objects.get_mut(&id).expect("unit").ai.locomotor_goal_type = LocoGoalType::None;
        assert_eq!(g.locomotor_distance_to_goal(id), 0.0);
    }
}

#[cfg(test)]
mod path_timestamp_tests {
    use super::*;
    use crate::pathfind::Pathfinder;
    use crate::terrain::TerrainLogic;

    /// `AIUpdateInterface::requestPath` and `requestAttackPath` never reset `m_pathTimestamp` (AIUpdate.cpp:231 is
    /// the only reset; :1657, :1768 and :2003 set it). The 3-frame quick-repath gate (AIUpdate.cpp:499-503) must see
    /// the age of the last computed path, not of the last request. Revert check: restoring `path_timestamp = 0` in
    /// `start_move` or `request_attack_path` fails the assertions below.
    #[test]
    fn requests_keep_the_last_computed_path_timestamp() {
        let mut ai = AiState { path_timestamp: 100, ..Default::default() };
        ai.start_move(Coord3D::new(10.0, 0.0, 0.0), PathRequest::Move, false);
        assert_eq!(ai.path_timestamp, 100, "start_move");

        let terrain = TerrainLogic::flat(400, 400, 0);
        let mut pf = Pathfinder::new(&terrain);
        ai.request_attack_path(&mut pf, ObjectId(1), ObjectId(2), Coord3D::new(20.0, 0.0, 0.0), 1000);
        assert_eq!(ai.path_timestamp, 100, "request_attack_path");
    }

    /// `requestAttackPath` (AIUpdate.cpp:532-539): a path computed in the last 3 frames is not requeued at once; the
    /// queue waits two seconds and the locomotor goal clears. Revert check: removing the gate queues the request now
    /// (`queue_for_path_frame` stays 0 and the goal is kept).
    #[test]
    fn attack_path_right_after_a_path_waits_two_seconds() {
        let terrain = TerrainLogic::flat(400, 400, 0);
        let mut pf = Pathfinder::new(&terrain);
        let mut ai = AiState { path_timestamp: 998, locomotor_goal_type: LocoGoalType::PositionOnPath, ..Default::default() };
        let queued = ai.request_attack_path(&mut pf, ObjectId(1), ObjectId(2), Coord3D::new(20.0, 0.0, 0.0), 1000);
        // The queue time comes back to the caller, which applies `setQueueForPathTime`'s wake (AIUpdate.cpp:955-968).
        assert_eq!(queued, Some(2 * crate::math::LOGICFRAMES_PER_SECOND), "the caller wakes the module");
        assert_eq!(ai.queue_for_path_frame, 1000 + 2 * crate::math::LOGICFRAMES_PER_SECOND);
        assert_eq!(ai.locomotor_goal_type, LocoGoalType::None);
        // An old path: queued at once (no timer), goal kept.
        let mut ai = AiState { path_timestamp: 100, locomotor_goal_type: LocoGoalType::PositionOnPath, ..Default::default() };
        ai.request_attack_path(&mut pf, ObjectId(1), ObjectId(2), Coord3D::new(20.0, 0.0, 0.0), 1000);
        assert_eq!(ai.queue_for_path_frame, 0);
        assert_eq!(ai.locomotor_goal_type, LocoGoalType::PositionOnPath);
    }
}

#[cfg(test)]
mod finished_path_tests {
    //! Lane 127: the finished-move and `doLocomotor` goal rules. `AIInternalMoveToState::update`'s
    //! arrival (`AIStates.cpp:1929-1940`) keeps `m_goalPosition` and `m_path`; only the end-of-update
    //! `m_movementComplete` block drops the path (`AIUpdate.cpp:1035`). `doLocomotor` switches on
    //! `m_locomotorGoalType` (`AIUpdate.cpp:2161-2286`): a `NONE` goal runs
    //! `locoUpdate_maintainCurrentPosition` and must *not* follow a `m_path` the move state still holds.
    use super::*;
    use crate::template::{kind_of, ObjectTemplate};
    use crate::terrain::TerrainLogic;
    use std::sync::Arc;

    fn tank(name: &str, kind: LocomotorKind) -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 5.0;
        t.physics = Some(crate::physics::PhysicsTemplate { mass: 10.0, ..crate::physics::PhysicsTemplate::default() });
        t.locomotor = Some(LocomotorParams::from_engine_units(kind, 1.0, 0.0, 0.2, 0.0556, 0.1, true));
        Arc::new(t)
    }

    fn world() -> GameLogic {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 10000);
        g
    }

    /// A `Path` whose only node is its start node: `Path::getLastNode()` is that node, but
    /// `Path::waypoints()` (the port's `ai.path`) is empty. `getPath() != nullptr` holds either way
    /// (`AIUpdate.cpp:2177-2189`), which is how the engine sees the path a move leaves behind when its
    /// destination is the cell the unit already stands in.
    fn start_only_path(at: Coord3D) -> Path {
        Path { nodes: vec![PathNode { pos: at, can_optimize: true, next_optimized: None }], blocked_by_ally: false }
    }

    /// `AIUpdateInterface::getPath() != nullptr` / `isMoving()`: a path with only its start node is a
    /// path even though the waypoint list is empty. Revert check: the old `!path.is_empty()` tests
    /// (`has_path` did not exist) report no path and "not moving".
    #[test]
    fn a_start_only_path_is_a_path() {
        let at = Coord3D::new(10.0, 10.0, 0.0);
        let mut ai = AiState { raw_path: Some(start_only_path(at)), path: Vec::new(), ..AiState::default() };
        assert!(ai.path.is_empty(), "no waypoints after the start node");
        assert!(ai.has_path(), "the Path itself exists");
        // An idle unit is not moving whatever path it still holds (`isIdle`, AIUpdate.cpp:3196-3198).
        assert!(!ai.is_moving(), "an idle unit with a path is not moving");
        // In its move state, after `friend_startingMove` (AIStates.cpp:1635), the same path is a move in progress.
        ai.machine.current = crate::ai_states::AiStateType::MoveTo;
        ai.friend_starting_move();
        assert!(ai.is_moving(), "a move that still holds its path is a move in progress");
        // No path at all is the `thePath==nullptr` side.
        let none = AiState::default();
        assert!(!none.has_path());
        assert!(!none.is_moving());
    }

    /// `case NONE: ... locoUpdate_maintainCurrentPosition` (`AIUpdate.cpp:2254-2285`): a unit whose
    /// locomotor goal is `NONE` does not drive along the path its move state still holds. The engine
    /// keeps a finished move's `m_path` until the state exits (`AIUpdate.cpp:1035`), so a stale path is
    /// the normal state of a unit that just arrived; following it would move a unit the engine leaves
    /// standing (AN3 s1 worker 238 crept 0.9 units into its dock this way).
    /// Revert check: with the old `let Some(&path_goal) = obj.ai.path.first()` dispatch the tank
    /// advances toward the second node and the assertion fails.
    #[test]
    fn a_none_goal_does_not_follow_the_path() {
        let mut g = world();
        let at = Coord3D::new(100.0, 100.0, 0.0);
        let id = g.create_object(&tank("Tank", LocomotorKind::Treads), at, 0.0, 0);
        let ahead = Coord3D::new(140.0, 100.0, 0.0);
        {
            let o = g.objects.get_mut(&id).unwrap();
            o.ai.path = vec![ahead];
            o.ai.raw_path = Some(Path {
                nodes: vec![
                    PathNode { pos: at, can_optimize: true, next_optimized: Some(1) },
                    PathNode { pos: ahead, can_optimize: true, next_optimized: None },
                ],
                blocked_by_ally: false,
            });
            o.ai.move_goal = Some(ahead);
            o.ai.locomotor_goal_type = LocoGoalType::None;
        }
        for _ in 0..30 {
            g.ai_do_locomotor(id);
            // The locomotor's motive forces reach the position through `PhysicsBehavior::update`.
            g.physics_update(id, g.frame);
        }
        let o = g.find_object(id).unwrap();
        assert!(o.position.dist_2d(&at) < 0.001, "a NONE goal maintains position, got ({:.3},{:.3})", o.position.x, o.position.y);
    }

    /// `Path::computePointOnPath` with no segment (`AIPathfind.cpp:767-768`: `out.posOnPath` defaults to
    /// `getLastNode()->getPosition()`, `distAlongPath` stays 0) still gives the locomotor a goal, so the
    /// unit brakes/stops instead of coasting. Revert check: without the `positions.len() == 1` branch
    /// `ai_goal_on_path` returns `None` for a start-only path.
    #[test]
    fn a_start_only_path_gives_the_locomotor_its_node() {
        let mut g = world();
        let at = Coord3D::new(100.0, 100.0, 0.0);
        let id = g.create_object(&tank("Tank", LocomotorKind::Treads), at, 0.0, 0);
        {
            let o = g.objects.get_mut(&id).unwrap();
            o.ai.path = Vec::new();
            o.ai.raw_path = Some(start_only_path(at));
            o.ai.locomotor_goal_type = LocoGoalType::PositionOnPath;
        }
        let loco = g.locomotor_now(id).expect("locomotor");
        let goal = g.ai_goal_on_path(id, &loco).expect("a start-only path has a point on it");
        assert!(goal.0.dist_2d(&at) < 0.001, "the point on the path is its only node");
        assert_eq!(goal.1, 0.0, "no segment, so no distance along the path");
    }

    /// `computePath` arms the locomotor on the *path object*: `if (theNewPath) { ... setLocomotorGoalPositionOnPath(); }`
    /// (`AIUpdate.cpp:1740-1745`). A path whose only node is its start node is still `theNewPath`, so the
    /// waypoint list being empty must not skip the arming (a move to the unit's own cell otherwise keeps
    /// `NONE` and `doLocomotor` maintains position instead of computing a point on the path).
    /// Revert check: with the old `!o.ai.path.is_empty()` test the goal stays `None`.
    #[test]
    fn a_start_only_path_arms_the_locomotor_goal() {
        let mut g = world();
        let at = Coord3D::new(100.0, 100.0, 0.0);
        let id = g.create_object(&tank("Tank", LocomotorKind::Treads), at, 0.0, 0);
        {
            let o = g.objects.get_mut(&id).unwrap();
            o.ai.path = Vec::new();
            o.ai.raw_path = Some(start_only_path(at));
            o.ai.locomotor_goal_type = LocoGoalType::None;
        }
        g.ai_goal_on_path_if_any(id);
        assert_eq!(g.find_object(id).unwrap().ai.locomotor_goal_type, LocoGoalType::PositionOnPath);
    }

    /// The `doPathfind` goal update keys the path's last *raw* node (`getPath()->getLastNode()`,
    /// `AIUpdate.cpp:429-432`), not the last waypoint of the optimised chain (`Path::waypoints`).
    /// Revert check: with `o.ai.path.last()` the goal cell is the old waypoint, not the raw tail.
    #[test]
    fn the_goal_cell_is_the_paths_raw_last_node() {
        let mut g = world();
        let at = Coord3D::new(100.0, 100.0, 0.0);
        let id = g.create_object(&tank("Tank", LocomotorKind::Treads), at, 0.0, 0);
        let waypoint = Coord3D::new(160.0, 100.0, 0.0);
        let tail = Coord3D::new(280.0, 100.0, 0.0);
        {
            let o = g.objects.get_mut(&id).unwrap();
            o.ai.raw_path = Some(Path {
                nodes: vec![
                    PathNode { pos: at, can_optimize: true, next_optimized: Some(1) },
                    PathNode { pos: waypoint, can_optimize: true, next_optimized: None },
                    PathNode { pos: tail, can_optimize: true, next_optimized: None },
                ],
                blocked_by_ally: false,
            });
            o.ai.path = vec![waypoint];
            o.ai.is_final_goal = true;
        }
        g.ai_update_goal_to_path_end(id, true);
        let goal = g.pathfinder.goal_position(&g.terrain, id).expect("the goal cell was keyed");
        assert!(goal.dist_2d(&tail) <= PATHFIND_CELL_SIZE_F, "the raw last node's cell is the goal, got ({:.2},{:.2})", goal.x, goal.y);
        assert!(goal.dist_2d(&waypoint) > PATHFIND_CELL_SIZE_F, "not the stale waypoint");
    }
}
