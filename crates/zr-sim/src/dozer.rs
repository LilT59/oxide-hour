//! Dozer / worker task handling that the existing `build.rs` / `repair.rs` do not cover:
//! `ActionManager::canRepairObject`, `ActionManager::canResumeConstructionOf`,
//! `DozerAIUpdate::privateResumeConstruction`, the `DozerPrimaryIdleState` auto-repair
//! (`getBoredTime` / `getBoredRange`, `findObjectToRepair`), and the `DozerAIUpdateModuleData` /
//! `WorkerAIUpdateModuleData` fields.
//!
//! The low-level build/repair mechanics live in `build.rs` (construction) and `repair.rs` (healing);
//! this module owns the *task selection*: whether a dozer may be sent to repair or to resume an
//! unfinished structure, and what an idle dozer picks up on its own.

use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::{ObjectId, PlayerIndex};
use crate::partition::DistanceMode;
use crate::shroud::ObjectShroudStatus;
use crate::template::kind_of;

/// `DozerAIUpdateModuleData` / `WorkerAIUpdateModuleData` (`DozerAIUpdate.cpp:1441-1443`,
/// `WorkerAIUpdate.h:98-100`). `RepairHealthPercentPerSecond` is a fraction of the target's max
/// health restored per second; `BoredTime`/`BoredRange` drive the idle auto-repair.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DozerTemplate {
    /// `RepairHealthPercentPerSecond` (`INI::parsePercentToReal`): 0.0 if unset.
    pub repair_health_percent_per_second: f32,
    /// `BoredTime` (`INI::parseDurationReal`): frames of idle before the dozer looks for work.
    pub bored_time_frames: f32,
    /// `BoredRange` (`INI::parseReal`): scan radius for `findObjectToRepair`.
    pub bored_range: f32,
}

/// The states of `DozerActionStateMachine` (`DozerAIUpdate.cpp:788`) for a build task, as the dozer
/// walks to its start point, then to its build dock. Repair and resume-free paths are not modelled
/// here; see [`DozerAction`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DozerActionSub {
    /// No build action runs through this machine.
    #[default]
    Inactive,
    /// `DOZER_ACTION_PICK_ACTION_POS` (`DozerActionPickActionPosState::update`, `:149`).
    PickActionPos,
    /// `DOZER_ACTION_MOVE_TO_ACTION_POS` (`DozerActionMoveToActionPosState::update`, `:292`).
    MoveToActionPos,
    /// `DOZER_SELECT_BUILD_DOCK_LOCATION` (`DozerActionDoActionState::update`, `:500`).
    SelectBuildDock,
    /// `DOZER_MOVING_TO_BUILD_DOCK_LOCATION` (`:512`): wait for the dock move to end.
    MovingToBuildDock,
    /// `DOZER_DO_BUILD_AT_DOCK` (`:524`): the structure's construction advances.
    DoBuildAtDock,
}

/// A dozer's build task in its action machine (`DozerAIUpdate::m_dockPoint`, `m_task`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DozerAction {
    pub sub: DozerActionSub,
    /// The structure being built, `DozerAIUpdate::m_task[DOZER_TASK_BUILD].m_targetObjectID`.
    pub target: ObjectId,
    /// `DOZER_DOCK_POINT_START` and `DOZER_DOCK_POINT_ACTION` (`DozerAIUpdate::newTask`,
    /// `DozerAIUpdate.cpp:2025-2028`): both the start point and the build dock.
    pub dock: Coord3D,
    /// `DOZER_DOCK_POINT_END` (`DozerAIUpdate.cpp:2031-2035`): five cells from the dock, away from the
    /// structure. The dozer moves here when the build completes.
    pub end: Coord3D,
}

/// `MIN_ACTION_TOLERANCE` (`DozerAIUpdate.cpp:63`).
const MIN_ACTION_TOLERANCE: f32 = 70.0;
/// `SLOP` in `DozerActionMoveToActionPosState::update` (`DozerAIUpdate.cpp:340`).
const ACTION_SLOP: f32 = 15.0;
/// `5*PATHFIND_CELL_SIZE_F` (`DozerAIUpdate.cpp:2031`).
const END_DOCK_OFFSET: f32 = 5.0 * crate::pathfind::PATHFIND_CELL_SIZE_F;

impl GameLogic {
    /// `DozerAIUpdate::findGoodBuildOrRepairPosition` (`DozerAIUpdate.cpp:1925`, its `findPositionAround` at :1960): the spot a dozer
    /// builds or repairs from. The search starts half the target's major radius toward the dozer, then
    /// `findPositionAround` (min 0, max 100, `maxZDelta` 10 for ground units, `sourceToPathToDest` the
    /// dozer; flyers ignore the target) picks the nearest legal spot. Without one, the start is used.
    pub(crate) fn find_good_build_or_repair_position(&mut self, me: ObjectId, target: ObjectId) -> Coord3D {
        let (Some(d), Some(t)) = (self.objects.get(&me), self.objects.get(&target)) else { return Coord3D::ZERO };
        let (ours, theirs, major) = (d.position, t.position, t.template.geometry.major_radius);
        // `Vector3 offset(ours - theirs); offset.Normalize(); offset *= majorRadius / 2;` (3-D).
        let (dx, dy, dz) = (ours.x - theirs.x, ours.y - theirs.y, ours.z - theirs.z);
        let len = crate::math::sqrt(dx * dx + dy * dy + dz * dz);
        let (nx, ny, nz) = if len > 0.0 { (dx / len, dy / len, dz / len) } else { (0.0, 0.0, 0.0) };
        let half = major / 2.0;
        let working = Coord3D::new(theirs.x + nx * half, theirs.y + ny * half, theirs.z + nz * half);
        let airborne = self.is_airborne_locomotor(me);
        let opts = crate::dock::FindPositionOptions {
            min_radius: 0.0,
            max_radius: 100.0,
            // `if( !me->isUsingAirborneLocomotor() ) fpOptions.maxZDelta = MAX_Z_DELTA;` (`:1922-1923`).
            max_z_delta: if airborne { 1e10 } else { 10.0 },
            // Flyers can approach right over the target (`:1924-1925`).
            ignore_object: if airborne { target } else { ObjectId::INVALID },
            source_to_path_to_dest: me,
            start_angle: None,
        };
        self.find_position_around(&working, &opts).unwrap_or(working)
    }

    /// `DozerAIUpdate::newTask(DOZER_TASK_BUILD)` (`DozerAIUpdate.cpp:2018`, called from `construct` at :1787):
    /// record the dock points (the start point from `find_good_build_or_repair_position`, `:2011`) and let
    /// the primary machine re-evaluate. `newTask` ends with `m_dozerMachine->resetToDefaultState()`
    /// (`:2044`), which re-enters `DOZER_PRIMARY_IDLE` and runs its transition conditions inside the same
    /// call (`StateMachine::internalSetState` -> `friend_checkForTransitions`, StateMachine.cpp:644-650):
    /// with the AI idle and BUILD the most recent task, `isBuildMostImportant` (`DozerAIUpdate.cpp:1346-1366`)
    /// enters `DOZER_PRIMARY_BUILD`. `DozerActionState::onEnter` only resets the action machine to its default
    /// `PickActionPos` (`:1230-1240`), whose update runs on the module's *next* update — so the move is
    /// ordered there, not here (AN3 s1: the task is set in the f=1 update, `PickActionPos` runs in the f=2
    /// update for both the US dozer and the GLA worker).
    pub(crate) fn dozer_begin_build(&mut self, dozer: ObjectId, target: ObjectId) {
        let start = self.find_good_build_or_repair_position(dozer, target);
        let Some(site) = self.objects.get(&target).map(|t| t.position) else { return };
        // `offset = start - site` (z 0), normalized, scaled by 5 cells, added to the start point.
        let (dx, dy) = (start.x - site.x, start.y - site.y);
        let len = crate::math::sqrt(dx * dx + dy * dy);
        let (ox, oy) = if len > 0.0 { (dx / len * END_DOCK_OFFSET, dy / len * END_DOCK_OFFSET) } else { (0.0, 0.0) };
        // `WorkerAIUpdate::newTask` (WorkerAIUpdate.cpp:658-666) stores START, ACTION and END as the same position:
        // only `DozerAIUpdate::newTask` (DozerAIUpdate.cpp:2073-2083) moves END away from the structure.
        let end = if self.is_worker(dozer) { start } else { Coord3D::new(start.x + ox, start.y + oy, start.z) };
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&dozer) {
            // `DOZER_PRIMARY_IDLE` is the default state; the conditions decide on BUILD (below).
            o.ai.dozer_action = DozerAction { sub: DozerActionSub::Inactive, target, dock: start, end };
            // `resetToDefaultState` re-enters `DOZER_PRIMARY_IDLE`, whose `onEnter` resets the bored
            // timestamp (`DozerAIUpdate.cpp:1042-1051`).
            o.dozer_idle_frame = now;
        }
        self.dozer_enter_build_if_idle(dozer);
    }

    /// `DozerActionPickActionPosState::update` (`DozerAIUpdate.cpp:149`): order the move to the start
    /// point (`ignoreObstacle(goal)` and `aiMoveToPosition(CMD_FROM_AI)`, `:229-230`).
    fn dozer_pick_action_pos(&mut self, dozer: ObjectId) {
        let Some(a) = self.objects.get(&dozer).map(|o| o.ai.dozer_action) else { return };
        self.ai_move_to_position_ignoring(dozer, a.dock, a.target);
        if let Some(o) = self.objects.get_mut(&dozer) {
            o.ai.dozer_action.sub = DozerActionSub::MoveToActionPos;
        }
    }

    /// `DozerActionMoveToActionPosState::update`'s arrival test (`DozerAIUpdate.cpp:339-341`):
    /// `ThePartitionManager->getDistanceSquared(dozer, goalPos, FROM_BOUNDINGSPHERE_2D)` against
    /// `sqr(max(MIN_ACTION_TOLERANCE, boundingSphereRadius + SLOP))`.
    fn dozer_within_action_tolerance(&self, dozer: ObjectId, point: Coord3D) -> bool {
        let Some(d) = self.objects.get(&dozer) else { return false };
        let reach = (d.template.geometry.bounding_sphere_radius() + ACTION_SLOP).max(MIN_ACTION_TOLERANCE);
        // `distCalcProc_BoundaryAndBoundary_2D` with no object at the point: the gap to the dozer's
        // bounding circle, clamped at zero.
        let gap = (d.position.dist_2d(&point) - d.template.geometry.bounding_circle_radius()).max(0.0);
        gap * gap <= reach * reach
    }

    /// `DozerActionDoActionState` dock steps for `MovingToBuildDock` (`DozerAIUpdate.cpp:512`): once
    /// the AI is idle the dozer is at its dock and construction may advance.
    fn dozer_check_at_dock(&mut self, dozer: ObjectId) -> bool {
        if !self.objects.get(&dozer).is_some_and(|o| o.ai.machine.current == crate::ai_states::AiStateType::Idle) {
            return false;
        }
        if let Some(o) = self.objects.get_mut(&dozer) {
            o.ai.dozer_action.sub = DozerActionSub::DoBuildAtDock;
        }
        true
    }

    /// One update of the dozer's action machine for `target` (`DozerActionState::update`,
    /// `DozerAIUpdate.cpp:1173` -> `DozerActionStateMachine::updateStateMachine`), returning `true` when
    /// the machine is at `DOZER_DO_BUILD_AT_DOCK` and construction advances this frame.
    ///
    /// The primary machine decides whether this runs at all: `DOZER_PRIMARY_BUILD` (`sub != Inactive`)
    /// keeps running its action machine regardless of the AI state, while a pending task with the primary
    /// machine still in `DOZER_PRIMARY_IDLE` first has to pass the idle conditions, which only enter BUILD
    /// (and thereby reset the action machine to `PickActionPos`) — the pick itself is the *next* update
    /// (`DozerActionState::onEnter`, `:1230-1240`).
    ///
    /// A failed arrival test sends the machine back to `PickActionPos` (`DozerActionStateMachine` ctor,
    /// `DozerAIUpdate.cpp:822`), which orders the start move again on the next update.
    /// A target without an action started by [`dozer_begin_build`](Self::dozer_begin_build) (the
    /// hole-rebuild workers of `construct_rebuild`) keeps the old gate: construction waits until the
    /// move ends.
    pub(crate) fn dozer_build_step(&mut self, dozer: ObjectId, target: ObjectId) -> bool {
        let Some(a) = self.objects.get(&dozer).map(|o| o.ai.dozer_action) else { return false };
        if a.target != target {
            return !self.objects.get(&dozer).is_some_and(|o| o.is_ai_moving());
        }
        match a.sub {
            DozerActionSub::Inactive => {
                self.dozer_enter_build_if_idle(dozer);
                false
            }
            DozerActionSub::PickActionPos => {
                self.dozer_pick_action_pos(dozer);
                false
            }
            DozerActionSub::MoveToActionPos => {
                if self.dozer_within_action_tolerance(dozer, a.dock) {
                    // `DozerAIUpdate.cpp:356-360`: the structure stops awaiting construction on arrival.
                    if let Some(t) = self.objects.get_mut(&a.target) {
                        t.construction_arrived = true;
                    }
                    // Success: the next update runs `DozerActionDoActionState` (`onEnter` sets the
                    // select sub-task, `:453`).
                    self.dozer_set_sub(dozer, DozerActionSub::SelectBuildDock);
                } else if self.objects.get(&dozer).is_some_and(|o| o.ai.machine.current == crate::ai_states::AiStateType::Idle) {
                    // `if( ai && ai->isIdle() ) return STATE_FAILURE;` (`:368`).
                    self.dozer_set_sub(dozer, DozerActionSub::PickActionPos);
                }
                false
            }
            DozerActionSub::SelectBuildDock => {
                // `ai->aiMoveToPosition(dockLocation, CMD_FROM_AI)` (`:504`). The dock move runs as a
                // temporary move when the start move is still active (see `ai_private_move_to_position`).
                self.ai_private_move_to_position(dozer, a.dock, crate::ai_states::CommandSource::Ai);
                self.dozer_set_sub(dozer, DozerActionSub::MovingToBuildDock);
                self.dozer_check_at_dock(dozer)
            }
            DozerActionSub::MovingToBuildDock => self.dozer_check_at_dock(dozer),
            DozerActionSub::DoBuildAtDock => true,
        }
    }

    fn dozer_set_sub(&mut self, dozer: ObjectId, sub: DozerActionSub) {
        if let Some(o) = self.objects.get_mut(&dozer) {
            o.ai.dozer_action.sub = sub;
        }
    }

    /// The `DOZER_PRIMARY_IDLE` transition conditions (`DozerPrimaryStateMachine` ctor,
    /// `DozerAIUpdate.cpp:1292-1301`; `isBuildMostImportant`, `:1346-1366`): with `AIUpdateInterface::isIdle`
    /// and BUILD the most recent pending task, the primary machine enters `DOZER_PRIMARY_BUILD`.
    /// `DozerActionState::onEnter` (`:1230-1240`) then only resets the action machine to its default state
    /// (`PickActionPos`), so the pick happens on the module's next update, not now.
    fn dozer_enter_build_if_idle(&mut self, dozer: ObjectId) {
        // `if (!ai->isIdle()) return FALSE;` (`:1357-1358`).
        if !self.is_ai_idle(dozer) {
            return;
        }
        self.dozer_set_sub(dozer, DozerActionSub::PickActionPos);
    }

    /// `DozerAIUpdate::update` (`DozerAIUpdate.cpp:1596-1668`) / `WorkerAIUpdate::update`
    /// (`WorkerAIUpdate.cpp:236-311`): the dozer's or worker's half of its single update module, called
    /// right after `AIUpdateInterface::update` for the same object (`:1619`, `:257`). The engine's
    /// `DozerAIUpdate` *is* an `AIUpdateInterface` (and `WorkerAIUpdate` both a dozer and a supply truck),
    /// so the AI state machine, `doLocomotor` and `m_dozerMachine->updateStateMachine()` (`:1666`) happen
    /// in one scheduled call, at one wake frame, and the module returns `UPDATE_SLEEP_NONE` (`:1668`,
    /// `WorkerAIUpdate.cpp:310`) — it never sleeps past the next frame.
    pub(crate) fn dozer_module_update(&mut self, id: ObjectId) {
        // `if (getObject()->isEffectivelyDead()) return UPDATE_SLEEP_NONE;` (`:1623-1624`, `:261-262`).
        if self.objects.get(&id).is_none_or(|o| o.destroyed || o.is_dying() || o.effectively_dead) {
            return;
        }
        if self.is_worker(id) {
            self.worker_module_update(id);
            return;
        }
        if self.dozer_data(id).is_none() {
            return;
        }
        self.dozer_primary_update(id);
    }

    /// `WorkerAIUpdate::update`'s second half (`WorkerAIUpdate.cpp:271-309`): `WorkerStateMachine` picks
    /// `AS_DOZER` or `AS_SUPPLY_TRUCK`; in `AS_DOZER` the dozer machine runs, otherwise the supply-truck
    /// machine (and the mine-clearing weapon flag). [`worker_supply_step`](Self::worker_supply_step) does
    /// the switch and the supply half and reports the mode.
    fn worker_module_update(&mut self, id: ObjectId) {
        if self.worker_supply_step(id) {
            return;
        }
        // `createMachines` happens in the module's first update; both machines exist for a worker.
        self.dozer_primary_update(id);
    }

    /// `m_dozerMachine->updateStateMachine()` (`DozerAIUpdate.cpp:1666`): the primary machine
    /// (`DozerPrimaryStateMachine`). `DOZER_PRIMARY_BUILD` runs the build action machine; otherwise the
    /// primary machine is `DOZER_PRIMARY_IDLE`, whose own update runs first and whose transition
    /// conditions may then move it to BUILD this frame (`StateMachine.cpp:412-467`;
    /// [`dozer_build_step`](Self::dozer_build_step)).
    fn dozer_primary_update(&mut self, id: ObjectId) {
        let (sub, build, repair) = match self.objects.get(&id) {
            Some(o) => (o.ai.dozer_action.sub, o.build_target, o.repair_target),
            None => return,
        };
        // The primary machine stays in `DOZER_PRIMARY_IDLE` until a pending BUILD/REPAIR task's
        // condition sees an idle AI (`DozerAIUpdate.cpp:1292-1301`, `:1346-1399`), so the idle state's
        // update also runs while a task is pending (and while the dozer is still moving). The port has
        // no primary REPAIR state: a pending repair with an idle AI stands in for it (it has entered,
        // since `isRepairMostImportant` needs `ai->isIdle()`).
        let in_repair_state = repair.is_some() && self.is_ai_idle(id);
        if sub == DozerActionSub::Inactive && !in_repair_state {
            self.dozer_idle_step(id);
        }
        if let Some(target) = build {
            self.dozer_build_action_update(id, target);
            return;
        }
        // A live action without a task target: the structure is gone. `DozerActionPickActionPosState`'s
        // null goal cancels the task (`:170-179`), which returns the primary machine to IDLE.
        if sub != DozerActionSub::Inactive {
            self.dozer_primary_idle_enter(id);
        }
        if let Some(target) = repair {
            self.dozer_repair_step(id, target);
        }
    }

    /// `DozerPrimaryIdleState::onEnter` (`DozerAIUpdate.cpp:1042-1051`): entering the primary machine's
    /// IDLE state clears the action machine's state (it is persistent) and sets
    /// `m_idleTooLongTimestamp = TheGameLogic->getFrame()`.
    fn dozer_primary_idle_enter(&mut self, dozer: ObjectId) {
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&dozer) {
            o.ai.dozer_action.sub = DozerActionSub::Inactive;
            o.dozer_idle_frame = now;
        }
    }

    /// Construction completed for `target` (`DozerActionDoActionState::update`, `DozerAIUpdate.cpp:645-652`):
    /// the dozer moves to its end dock point with `aiMoveToPosition(CMD_FROM_AI)`. A dozer whose action
    /// is for another structure (a hole rebuild) does nothing.
    ///
    /// The end point is first adjusted to a pathable spot (`adjustToPossibleDestination`, DozerAIUpdate.cpp:673).
    pub(crate) fn dozer_build_finished(&mut self, dozer: ObjectId, target: ObjectId) {
        let Some(a) = self.objects.get(&dozer).map(|o| o.ai.dozer_action) else { return };
        if a.target != target || a.sub == DozerActionSub::Inactive {
            return;
        }
        // `complete = TRUE` -> the action state reports success and the primary machine re-enters
        // `DOZER_PRIMARY_IDLE` (`DozerAIUpdate.cpp:1292-1301`), whose `onEnter` resets the bored timestamp.
        self.dozer_primary_idle_enter(dozer);
        // `pos = *endPos`, then `TheAI->pathfinder()->adjustToPossibleDestination(dozer, locomotorSet, &pos)`
        // (DozerAIUpdate.cpp:664-673), then `aiMoveToPosition(&pos, CMD_FROM_AI)` (:674).
        let mut pos = a.end;
        if let Some(po) = self.objects.get(&dozer).and_then(crate::pathfind::PathObject::for_object) {
            let from = self.objects.get(&dozer).map_or(pos, |o| o.position);
            let objects = &self.objects;
            let _ = self.pathfinder.adjust_to_possible_destination(&self.terrain, &po, &from, &mut pos, &|u| objects.get(&ObjectId(u)).map(|o| o.player));
        }
        self.ai_private_move_to_position(dozer, pos, crate::ai_states::CommandSource::Ai);
        self.wake_up_now(dozer);
    }

    /// `ObjectTemplate`'s dozer data, when this object is a dozer/worker.
    pub fn dozer_data(&self, id: ObjectId) -> Option<DozerTemplate> {
        self.objects.get(&id).and_then(|o| o.template.dozer)
    }

    /// `DozerAIUpdate::getRepairHealthPerSecond`: the fraction of max health healed per second.
    pub fn repair_health_per_second(&self, id: ObjectId) -> f32 {
        self.dozer_data(id).map_or(0.0, |d| d.repair_health_percent_per_second)
    }

    /// `ActionManager::canRepairObject` (`ActionManager.cpp:379`), the view/command-side gate.
    ///
    /// Retail rejects repairing bridges and bridge towers ("GS repairing bridges cut 12/12/02"), so
    /// the player/AI repair command never reaches the bridge path (the mechanics in `repair.rs`
    /// remain reachable through the low-level `command_repair`, which the bridge test uses).
    pub fn can_repair_object(&self, dozer: ObjectId, target: ObjectId) -> bool {
        self.can_repair_object_from(dozer, target, false)
    }

    /// As [`can_repair_object`](Self::can_repair_object), but `ai` skips the human-shroud gate
    /// (`isObjectShroudedForAction` is only true for a human source, `ActionManager.cpp:96`).
    pub fn can_repair_object_from(&self, dozer: ObjectId, target: ObjectId, ai: bool) -> bool {
        let Some(d) = self.objects.get(&dozer) else { return false };
        if d.destroyed || d.is_dying() {
            return false;
        }
        // You can only repair allies; enemies are rejected, neutral (civilian) buildings allowed.
        if self.relationship(d.player, self.objects.get(&target).map_or(d.player, |t| t.player))
            == crate::damage::Relationship::Enemies
        {
            return false;
        }
        let Some(t) = self.objects.get(&target) else { return false };
        // Bridges (towers) are not repairable in retail.
        if t.template.is_bridge || t.is_kind_of(kind_of::BRIDGE) || t.is_kind_of(kind_of::BRIDGE_TOWER) {
            return false;
        }
        // Nothing can be done with things under construction.
        if d.under_construction || t.under_construction {
            return false;
        }
        // Regeneration holes cannot be manually repaired.
        if t.is_kind_of(kind_of::REBUILD_HOLE) {
            return false;
        }
        // Only dozers go repair, and only buildings.
        if !d.is_kind_of(kind_of::DOZER) || !t.is_kind_of(kind_of::STRUCTURE) {
            return false;
        }
        // A building at full health needs no repair.
        let Some(body) = &t.body else { return false };
        if body.health() >= body.max_health() || body.max_health() <= 0.0 {
            return false;
        }
        // Shrouded targets are not actionable by a human player (`isObjectShroudedForAction` only
        // applies to a human source). Own/allied structures are always visible to their owner.
        if !ai
            && !self.is_ai_player(d.player)
            && self.relationship(d.player, t.player) != crate::damage::Relationship::Allies
            && self.object_shroud_status(d.player, target) >= ObjectShroudStatus::Fogged
        {
            return false;
        }
        // Cannot heal while inside a transport.
        if d.contained_by.is_some() {
            return false;
        }
        true
    }

    /// `ActionManager::canResumeConstructionOf` (`ActionManager.cpp:449`), the view/command-side gate.
    pub fn can_resume_construction(&self, dozer: ObjectId, target: ObjectId) -> bool {
        self.can_resume_construction_from(dozer, target, false)
    }

    /// As [`can_resume_construction`](Self::can_resume_construction), with the AI/script shroud bypass.
    pub fn can_resume_construction_from(&self, dozer: ObjectId, target: ObjectId, ai: bool) -> bool {
        let Some(d) = self.objects.get(&dozer) else { return false };
        // Only dozers or workers can resume construction.
        if !d.is_kind_of(kind_of::DOZER) {
            return false;
        }
        let Some(t) = self.objects.get(&target) else { return false };
        // TheSuperHackers @bugfix Stubbjax 06/01/2026: only the owner may resume it.
        if d.player != t.player {
            return false;
        }
        // The object must actually be under construction.
        if !t.under_construction {
            return false;
        }
        if d.is_dying() {
            return false;
        }
        // If another (live) dozer is already building it, no doubling up.
        if let Some(b) = t.builder {
            if let Some(bo) = self.objects.get(&b) {
                if !bo.is_dying() && !bo.destroyed && bo.build_target == Some(target) {
                    return false;
                }
            }
        }
        // Shrouded targets are not actionable by a human player; the owner always sees its own
        // construction, so this only matters for a hypothetical non-allied resume.
        if !ai
            && !self.is_ai_player(d.player)
            && self.relationship(d.player, t.player) != crate::damage::Relationship::Allies
            && self.object_shroud_status(d.player, target) >= ObjectShroudStatus::Fogged
        {
            return false;
        }
        true
    }

    /// `DozerAIUpdate::privateResumeConstruction` (`DozerAIUpdate.cpp:1874`): order a dozer to resume
    /// the construction of `target`.
    pub fn command_resume_construction(&mut self, player: PlayerIndex, dozer: ObjectId, target: ObjectId) -> bool {
        if self.objects.get(&dozer).is_none_or(|o| o.player != player) {
            return false;
        }
        if !self.can_resume_construction(dozer, target) {
            return false;
        }
        self.begin_build_task(dozer, target);
        true
    }

    /// Start (or resume) a dozer's build task on `target` and walk it to the approach point.
    pub(crate) fn begin_build_task(&mut self, dozer: ObjectId, target: ObjectId) {
        // `WorkerAIUpdate::newTask` (`WorkerAIUpdate.cpp:~650-680`): a worker acting as a supply truck
        // is idled and returns to dozer mode before the new dozer task.
        self.exiting_supply_truck_state(dozer);
        if !self.objects.contains_key(&target) || !self.objects.contains_key(&dozer) {
            return;
        }
        if let Some(o) = self.objects.get_mut(&target) {
            // `Object::setBuilder`: whoever "thinks" about building is the current builder, so a
            // later dozer cannot resume it
            // (`DozerAIUpdate::newTask`, `DozerAIUpdate.cpp:2023`).
            o.builder = Some(dozer);
        }
        if let Some(o) = self.objects.get_mut(&dozer) {
            o.build_target = Some(target);
        }
        self.dozer_begin_build(dozer, target);
    }

    /// `DozerPrimaryIdleState::update` (`DozerAIUpdate.cpp:1074-1156`): a dozer idle for longer than
    /// `BoredTime` with no pending task looks for a damaged friendly structure within `BoredRange`
    /// and repairs the closest one (`findObjectToRepair`, `DozerAIUpdate.cpp:876`). One call per module
    /// update while the primary machine is in `DOZER_PRIMARY_IDLE`.
    pub(crate) fn dozer_idle_step(&mut self, dozer: ObjectId) {
        let now = self.frame;
        // A worker in `AS_SUPPLY_TRUCK` does not run its dozer machine at all
        // (`WorkerAIUpdate::update`, `WorkerAIUpdate.cpp:271-309`), so its bored timestamp is left
        // alone too.
        if self.objects.get(&dozer).is_none_or(|o| o.supply_ai.worker_supply) {
            return;
        }
        // `if( !ai->isIdle() ) m_idleTooLongTimestamp = TheGameLogic->getFrame();` (`:1112-1113`):
        // refreshed on every frame the AI is not idle, whether or not a task is pending. `AIUpdateInterface
        // ::isIdle` is the state machine's own test, so a sub-machine's idle state does not count.
        if !self.is_ai_idle(dozer) {
            if let Some(o) = self.objects.get_mut(&dozer) {
                o.dozer_idle_frame = now;
            }
            return;
        }
        // `isAnyTaskPending() == FALSE` (`:1117-1120`, `:2138-2147`): a pending build/repair task
        // blocks the search, but the timestamp keeps ageing (the refresh above is for a busy AI only).
        if self.objects.get(&dozer).is_some_and(|o| o.build_target.is_some() || o.repair_target.is_some()) {
            return;
        }
        let Some(data) = self.objects.get(&dozer).and_then(|o| o.template.dozer) else { return };
        let idle_since = self.objects.get(&dozer).map_or(now, |o| o.dozer_idle_frame);
        if (now.saturating_sub(idle_since)) as f32 <= data.bored_time_frames {
            return;
        }
        // Reset the timestamp before the (expensive) search, like the original.
        if let Some(o) = self.objects.get_mut(&dozer) {
            o.dozer_idle_frame = now;
        }
        if let Some(target) = self.find_object_to_repair(dozer, data.bored_range) {
            let player = self.objects.get(&dozer).map_or(0, |o| o.player);
            // `ai->aiRepair(repairTarget, CMD_FROM_AI)`.
            self.command_repair(player, dozer, target);
        }
    }

    /// A repair task ended (repaired to full health, or its target is gone): the action machine reports
    /// success and the primary machine returns to `DOZER_PRIMARY_IDLE`, whose `onEnter` resets the bored
    /// timestamp (`DozerAIUpdate.cpp:1042-1051`).
    pub(crate) fn dozer_repair_finished(&mut self, dozer: ObjectId) {
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&dozer) {
            o.repair_target = None;
            o.dozer_idle_frame = now;
        }
    }

    /// `findObjectToRepair` (`DozerAIUpdate.cpp:876`): closest damaged friendly structure within
    /// `bored_range` the dozer may repair. AI players scan `AIDozerBoredRadiusModifier` times farther
    /// (`getBoredRange`, `DozerAIUpdate.cpp:2428`).
    fn find_object_to_repair(&self, dozer: ObjectId, bored_range: f32) -> Option<ObjectId> {
        let d = self.objects.get(&dozer)?;
        let pos: Coord3D = d.position;
        let player = d.player;
        let range = if self.is_ai_player(player) {
            bored_range * self.ai_tunables.ai_dozer_bored_radius_modifier
        } else {
            bored_range
        };
        let ids = self.partition.objects_in_range(&pos, range, DistanceMode::FromCenter2D);
        let mut best: Option<(ObjectId, f32)> = None;
        for id in ids {
            // `if (TheActionManager->canRepairObject(dozer, obj, CMD_FROM_AI) == FALSE) continue;`
            if !self.can_repair_object_from(dozer, id, true) {
                continue;
            }
            let Some(t) = self.objects.get(&id) else { continue };
            let dist = pos.dist_sqr_2d(&t.position);
            match best {
                None => best = Some((id, dist)),
                Some((_, bd)) if dist < bd => best = Some((id, dist)),
                _ => {}
            }
        }
        best.map(|(id, _)| id)
    }
}
