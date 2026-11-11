//! Supply gathering: `SupplyTruckAIUpdate` and its `SupplyTruckStateMachine`
//! (`Object/Update/AIUpdate/SupplyTruckAIUpdate.cpp`), `ResourceGatheringManager`
//! (`Common/RTS/ResourceGatheringManager.cpp`) and the dock actions of
//! `SupplyWarehouseDockUpdate` / `SupplyCenterDockUpdate`.
//!
//! The truck's own machine (Busy / Idle / Wanting / Regrouping / Docking) only decides *what to
//! dock with*: the `SupplyTruckWantsToPickUpOrDeliverBoxesState` finds the best warehouse (no
//! boxes) or center (carrying boxes) and calls `aiDock`, and the docking itself is the main
//! machine's `AI_DOCK` state (`crate::ai_states`, `AIDock.cpp`) calling [`GameLogic::dock_action`]
//! through the `DockUpdate` positions in [`crate::dock`]. A truck only starts the loop once it is
//! forced into the Wanting state (`setForceWantingState`: a Supply Center's exit, an idle resume,
//! an interrupted dock) or ordered to dock by the player.
//!
//! A worker (`WorkerAIUpdate`) runs this machine only in its `AS_SUPPLY_TRUCK` mode
//! ([`GameLogic::worker_supply_step`], after its AI update); its dozer mode and the
//! `exitingSupplyTruckState` hook are sim logic here too. Reductions: the `DOZER`/`HARVESTER`
//! dock-animation tweak in `DockUpdate::update` is a client concern; the `UpgradedSupplyBoost`
//! bonus of `ChinookAIUpdate` is not ported; the drawable supply-status bar and the "supplies
//! depleted" voice are not simulated.

use crate::ai_states::{AiStateType, CommandSource, StateReturn};
use crate::dock::FindPositionOptions;
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::ObjectId;
use crate::template::kind_of;

/// Which supply dock this is (`SupplyWarehouseDockUpdate` / `SupplyCenterDockUpdate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockKind {
    Warehouse,
    Center,
}

/// The supply dock fields the sim needs (`DockUpdateModuleData` + the warehouse fields).
#[derive(Clone, Debug, PartialEq)]
pub struct DockTemplate {
    pub kind: DockKind,
    pub starting_boxes: i32,
    pub delete_when_empty: bool,
    /// `NumberApproachPositions` (`-1` = dynamic, `DYNAMIC_APPROACH_VECTOR_FLAG`).
    pub number_approach_positions: i32,
    /// `AllowsPassthrough`.
    pub allows_passthrough: bool,
}

/// The supply truck fields (`SupplyTruckAIUpdate`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SupplyTruckTemplate {
    pub max_boxes: i32,
    pub center_action_delay: u32,
    pub warehouse_action_delay: u32,
    pub scan_distance: f32,
}

/// `SupplyTruckStateMachine`'s states (`SupplyTruckAIUpdate.h`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SupplyState {
    /// `ST_BUSY` (the default state).
    #[default]
    Busy,
    Idle,
    Wanting,
    Regrouping,
    Docking,
}

/// `SupplyTruckAIUpdate`'s runtime fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SupplyAiState {
    pub state: SupplyState,
    /// `m_preferredDock`.
    pub preferred_dock: ObjectId,
    /// `m_forcePending` (`setForceWantingState`).
    pub force_wanting: bool,
    /// `m_forcedBusyPending` (`setForceBusyState`).
    pub force_busy: bool,
    /// `WorkerStateMachine` is in `AS_SUPPLY_TRUCK` (a worker only; `WorkerAIUpdate.cpp:1257`). A
    /// worker in `AS_DOZER` does not run `state` at all. Always false for other trucks.
    pub worker_supply: bool,
}

impl SupplyAiState {
    pub(crate) fn crc(&self, c: &mut crate::crc::Crc) {
        c.add_u32(self.state as u32);
        c.add_u32(self.preferred_dock.0);
        c.add_bool(self.force_wanting);
        c.add_bool(self.force_busy);
        c.add_bool(self.worker_supply);
    }
}

/// `StateConditionInfo` tests of `SupplyTruckStateMachine`.
#[derive(Clone, Copy)]
enum Cond {
    ForcedBusy,
    ForcedWanting,
    OwnerDocking,
    OwnerIdle,
    OwnerNotDockingOrIdle,
    OwnerPlayerCommanded,
    OwnerAvailableForSupplying,
}

/// `REGROUP_SUCCESS_DISTANCE_SQUARED`.
const REGROUP_SUCCESS_DISTANCE_SQUARED: f32 = 225.0;

/// The state table (`SupplyTruckStateMachine::SupplyTruckStateMachine`):
/// `(success state, failure state, conditions)`.
fn state_table(s: SupplyState) -> (SupplyState, SupplyState, &'static [(Cond, SupplyState)]) {
    use SupplyState::*;
    match s {
        Busy => (Busy, Busy, &[(Cond::OwnerIdle, Idle), (Cond::OwnerDocking, Docking)]),
        Idle => (
            Busy,
            Busy,
            &[
                (Cond::ForcedBusy, Busy),
                (Cond::ForcedWanting, Wanting),
                (Cond::OwnerDocking, Docking),
                (Cond::OwnerNotDockingOrIdle, Busy),
            ],
        ),
        Wanting => (Busy, Regrouping, &[(Cond::OwnerDocking, Docking), (Cond::OwnerNotDockingOrIdle, Busy)]),
        Regrouping => (Wanting, Busy, &[(Cond::OwnerPlayerCommanded, Busy)]),
        Docking => (
            Busy,
            Busy,
            &[(Cond::ForcedBusy, Busy), (Cond::OwnerAvailableForSupplying, Wanting), (Cond::OwnerNotDockingOrIdle, Busy)],
        ),
    }
}

impl GameLogic {
    /// `SupplyTruckAIInterface::setForceWantingState`.
    pub fn set_supply_force_wanting(&mut self, id: ObjectId, v: bool) {
        if let Some(o) = self.objects.get_mut(&id) {
            if o.template.supply_truck.is_some() {
                o.supply_ai.force_wanting = v;
            }
        }
    }

    /// The `SupplyTruckAIUpdate::update` half that runs before `AIUpdateInterface::update`
    /// (`SupplyTruckAIUpdate.cpp:97-107`): the truck's own supply state machine (the base AI runs
    /// later in `ai_update_one`). It runs from the object's AI slot
    /// ([`GameLogic::ai_update_one`](crate::logic::GameLogic)), so it is gated by the AI module's
    /// disabled mask and pops at the module's heap entry.
    ///
    /// There is no availability gate here: `isAvailableForSupplying` is the state machine's
    /// `ownerAvailableForSupplying` condition and a query (`SupplyTruckAIUpdate.cpp:795-815`), not a
    /// gate on `update`, so the machine keeps running (and can transition, e.g. `Docking` ->
    /// `Wanting`) while a Chinook carries passengers or is in a drop.
    pub(crate) fn supply_ai_update_one(&mut self, id: ObjectId) {
        self.supply_machine_update(id);
        self.supply_mirror_dock_state(id);
    }

    /// A worker: both `dozer` and `supply_truck` data (`WorkerAIUpdate` derives from
    /// `SupplyTruckAIUpdate` and `DozerAIUpdate`; the templates carry no separate worker flag).
    pub(crate) fn is_worker(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.template.dozer.is_some() && o.template.supply_truck.is_some())
    }

    /// A worker's `WorkerStateMachine` and, in `AS_SUPPLY_TRUCK`, its supply-truck machine — the half of
    /// `WorkerAIUpdate::update` that runs after `AIUpdateInterface::update` (`WorkerAIUpdate.cpp:271-309`),
    /// called from [`dozer_module_update`](Self::dozer_module_update) for the same object. Returns `true`
    /// when the worker is in `AS_SUPPLY_TRUCK` (its dozer machine does not run that frame).
    ///
    /// A worker enters supply mode when forced into Wanting or when its AI is docking (`:1312`); it leaves
    /// when its supply brain is Busy and neither applies (`:1330`). Only in supply mode does the worker run
    /// the supply machine, and then it keeps `WEAPONSET_MINE_CLEARING_DETAIL` set.
    pub(crate) fn worker_supply_step(&mut self, id: ObjectId) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        let wants = o.supply_ai.force_wanting || o.ai.machine.current == AiStateType::Dock;
        if !o.supply_ai.worker_supply {
            if wants {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.supply_ai.worker_supply = true;
                }
            }
        } else if !wants && o.supply_ai.state == SupplyState::Busy {
            // `ActAsSupplyTruckState` leaves for `AS_DOZER`; its exit resets the supply brain.
            if let Some(o) = self.objects.get_mut(&id) {
                o.supply_ai.worker_supply = false;
            }
        }
        let supply = self.objects.get(&id).is_some_and(|o| o.supply_ai.worker_supply);
        if supply {
            self.supply_machine_update(id);
            // `setWeaponSetFlag(WEAPONSET_MINE_CLEARING_DETAIL)` each frame in supply mode (`:290`).
            self.set_weapon_set_flag(id, crate::weapon::WEAPONSET_MINE_CLEARING_DETAIL);
        }
        self.supply_mirror_dock_state(id);
        supply
    }

    /// `WorkerAIUpdate::exitingSupplyTruckState` (`WorkerAIUpdate.cpp:455`): a worker in supply mode
    /// that is given a dozer order (or whose supply brain goes idle) goes back to `AS_DOZER` and its
    /// AI is idled. The supply brain then resets to its default (Busy) state.
    pub(crate) fn exiting_supply_truck_state(&mut self, id: ObjectId) {
        if !self.objects.get(&id).is_some_and(|o| o.supply_ai.worker_supply) {
            return;
        }
        self.ai_idle(id, CommandSource::Ai);
        if let Some(o) = self.objects.get_mut(&id) {
            o.supply_ai.worker_supply = false;
        }
        // `supplyTruckSubMachineWantsToEnter` (`WorkerAIUpdate.cpp:1328-1342`) is the AS_DOZER state's
        // transition, checked as soon as `setState(AS_DOZER)` completes. The force it reads is the one
        // pending when the worker leaves supply mode: the brain's reset below (`resetSupplyTruckBrain`,
        // `WorkerAIUpdate.cpp:1221`) must not consume it first. A worker still forced into Wanting returns
        // to `AS_SUPPLY_TRUCK` in the same update (C++ AN3 seed 1, worker 238 at frame 1163:
        // `ZR_L81_WKEXIT ... worker=1 sup=2`).
        let forced = self.objects.get(&id).is_some_and(|o| o.supply_ai.force_wanting || o.ai.machine.current == AiStateType::Dock);
        // `ActAsSupplyTruckState::onExit` -> `resetSupplyTruckBrain` (`SupplyTruckStateMachine` default).
        self.supply_set_state(id, SupplyState::Busy, 0);
        if forced {
            if let Some(o) = self.objects.get_mut(&id) {
                o.supply_ai.worker_supply = true;
            }
        }
    }

    /// Keep the observable mirrors of the dock state (`supply_target`, `supply_timer`).
    fn supply_mirror_dock_state(&mut self, id: ObjectId) {
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&id) {
            let docking = o.ai.machine.current == AiStateType::Dock;
            o.supply_target = docking.then_some(o.ai.machine.goal_object);
            o.supply_timer = if docking && o.ai.machine.dock_sub == crate::ai_states::DockSub::ProcessDock {
                o.ai.machine.dock_next_action.saturating_sub(now)
            } else {
                0
            };
        }
    }

    /// `SupplyTruckAIInterface::isAvailableForSupplying` (`ChinookAIUpdate::isAvailableForSupplying`).
    pub(crate) fn supply_available(&self, id: ObjectId) -> bool {
        match self.objects.get(&id) {
            Some(o) => !(o.template.chinook.is_some() && (!o.contained.is_empty() || self.chinooks.contains_key(&id))),
            None => false,
        }
    }

    /// `AIUpdateInterface::isIdle` (`AIUpdate.cpp:3120`): the main machine is in an idle state. The
    /// state alone decides; a unit still walking out of an idle state is not "moving" here.
    fn supply_ai_idle(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.ai.machine.current == AiStateType::Idle)
    }

    fn supply_cond(&self, id: ObjectId, c: Cond) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        let docking = o.ai.machine.current == AiStateType::Dock;
        match c {
            Cond::ForcedBusy => o.supply_ai.force_busy,
            Cond::ForcedWanting => o.supply_ai.force_wanting,
            Cond::OwnerDocking => docking,
            Cond::OwnerIdle => self.supply_ai_idle(id),
            Cond::OwnerNotDockingOrIdle => !self.supply_ai_idle(id) && !docking,
            Cond::OwnerPlayerCommanded => o.ai.machine.last_command_source == CommandSource::Player,
            Cond::OwnerAvailableForSupplying => self.supply_available(id) && self.supply_ai_idle(id),
        }
    }

    /// `StateMachine::updateStateMachine` for the supply machine (`StateMachine.cpp:412`).
    fn supply_machine_update(&mut self, id: ObjectId) {
        let state = self.objects.get(&id).map_or(SupplyState::Busy, |o| o.supply_ai.state);
        let status = self.supply_state_update(id, state);
        // A state's update never changes the supply state itself.
        self.supply_check_transitions(id, state, status, 0);
    }

    /// `State::friend_checkForTransitions` (`StateMachine.cpp:112`).
    fn supply_check_transitions(&mut self, id: ObjectId, state: SupplyState, status: StateReturn, depth: u32) {
        if depth >= 20 {
            return;
        }
        let (success, failure, conds) = state_table(state);
        match status {
            StateReturn::Success => self.supply_set_state(id, success, depth),
            StateReturn::Failure => self.supply_set_state(id, failure, depth),
            StateReturn::Continue | StateReturn::Sleep(_) => {
                for &(cond, to) in conds {
                    if self.supply_cond(id, cond) {
                        self.supply_set_state(id, to, depth);
                        return;
                    }
                }
            }
        }
    }

    /// `StateMachine::internalSetState` (`StateMachine.cpp:585`): onEnter, then check the result.
    /// If onEnter changed the state (a nested reset), the nested state's transitions run with
    /// `CONTINUE` (`StateMachine.cpp:644-650`), not the stale sleep or success of the old one.
    fn supply_set_state(&mut self, id: ObjectId, new: SupplyState, depth: u32) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.supply_ai.state = new;
        }
        let status = self.supply_state_enter(id, new);
        let cur = self.objects.get(&id).map_or(new, |o| o.supply_ai.state);
        let status = if cur == new { status } else { StateReturn::Continue };
        self.supply_check_transitions(id, cur, status, depth + 1);
    }

    fn supply_state_enter(&mut self, id: ObjectId, s: SupplyState) -> StateReturn {
        match s {
            // `SupplyTruckBusyState::onEnter`: turn off the Busy latch.
            SupplyState::Busy => {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.supply_ai.force_busy = false;
                }
                StateReturn::Continue
            }
            // `SupplyTruckIdleState::onEnter` (`SupplyTruckAIUpdate.cpp:307`): a worker tells its
            // worker machine it is idle, which returns it to `AS_DOZER` (the idle-worker hook).
            SupplyState::Idle => {
                self.exiting_supply_truck_state(id);
                StateReturn::Continue
            }
            // `SupplyTruckWantsToPickUpOrDeliverBoxesState::onEnter`: one go at Wanting per force.
            SupplyState::Wanting | SupplyState::Docking => {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.supply_ai.force_wanting = false;
                }
                StateReturn::Continue
            }
            SupplyState::Regrouping => self.supply_regroup_enter(id),
        }
    }

    fn supply_state_update(&mut self, id: ObjectId, s: SupplyState) -> StateReturn {
        match s {
            SupplyState::Busy | SupplyState::Idle | SupplyState::Docking => StateReturn::Continue,
            // `RegroupingState::update`: once regrouped (idle), succeed to Wanting.
            SupplyState::Regrouping => {
                if self.supply_ai_idle(id) {
                    StateReturn::Success
                } else {
                    StateReturn::Continue
                }
            }
            SupplyState::Wanting => self.supply_wanting_update(id),
        }
    }

    /// `SupplyTruckWantsToPickUpOrDeliverBoxesState::update` (`SupplyTruckAIUpdate.cpp:430`).
    fn supply_wanting_update(&mut self, id: ObjectId) -> StateReturn {
        if !self.supply_available(id) {
            return StateReturn::Failure;
        }
        let boxes = self.objects.get(&id).map_or(0, |o| o.boxes);
        let dest = if boxes > 0 { self.find_best_supply_center(id) } else { self.find_best_supply_warehouse(id) };
        match dest {
            Some(d) => {
                self.ai_dock_command(id, d, CommandSource::Ai);
                StateReturn::Success
            }
            // We aren't going to wait right here; go back to base (Regrouping).
            None => StateReturn::Failure,
        }
    }

    /// `RegroupingState::onEnter` (`SupplyTruckAIUpdate.cpp:489`): go hang out at a Supply Center,
    /// else a Command Center, else any friendly structure.
    fn supply_regroup_enter(&mut self, id: ObjectId) -> StateReturn {
        let Some((player, pos)) = self.objects.get(&id).map(|o| (o.player, o.position)) else { return StateReturn::Failure };
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.ignore_obstacle_id = ObjectId::INVALID;
        }
        let closest = |logic: &GameLogic, kind: u64| -> Option<ObjectId> {
            // `Player::findClosestByKindOf`: the closest own object with the kind.
            let mut best: Option<(f32, ObjectId)> = None;
            for o in logic.objects.values() {
                if o.destroyed || o.player != player || o.id == id || !o.is_kind_of(kind) {
                    continue;
                }
                let d = pos.dist_2d(&o.position);
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, o.id));
                }
            }
            best.map(|(_, i)| i)
        };
        let dest = closest(self, kind_of::CASH_GENERATOR)
            .or_else(|| closest(self, kind_of::COMMANDCENTER))
            .or_else(|| closest(self, kind_of::STRUCTURE));
        let Some(dest) = dest else { return StateReturn::Failure };
        if self.boundary_dist_sqr_2d(id, dest) < REGROUP_SUCCESS_DISTANCE_SQUARED {
            return StateReturn::Continue; // Don't say Success so we don't spin the machine.
        }
        let dpos = self.objects.get(&dest).map_or(Coord3D::ZERO, |o| o.position);
        let opts = FindPositionOptions { min_radius: 0.0, max_radius: 100.0, ..Default::default() };
        let Some(target) = self.find_position_around(&dpos, &opts) else { return StateReturn::Failure };
        self.ai_move_to_position_cmd(id, target, CommandSource::Ai);
        StateReturn::Continue
    }

    /// `AIUpdateInterface::aiMoveToPosition` (`MSG_DO_MOVETO` for one unit).
    pub fn ai_move_to_position_cmd(&mut self, id: ObjectId, pos: Coord3D, source: CommandSource) {
        // `AssaultTransportAIUpdate::aiDoCommand`'s default branch for a command not from the AI resets the transport
        // (AssaultTransportAIUpdate.cpp:112-115), before the base command's checks.
        self.assault_command(id, crate::assault_transport::AssaultCommand::Other, source);
        // `aiDoCommand` refuses what `isAllowedToRespondToAiCommands` refuses (AIUpdate.cpp:2627, DeliverPayloadAIUpdate.cpp:145-155):
        // a transport whose exit is ordered (lane 189, R25-11).
        if self.delivery_refuses_commands(id) {
            return;
        }
        // `privateMoveToPosition` (`AIUpdate.cpp:2889`): a `CMD_FROM_AI` move to a non-idle unit is the
        // 20-second temporary `AI_MOVE_TO` and leaves the current state (and its attack) alone. Any other
        // move runs `clear()`, which drops the attack and the orders (the `ai_clear` below, the combat
        // fields here for the sim's command-side state).
        let temp = source == CommandSource::Ai && self.objects.get(&id).is_some_and(|o| o.ai.machine.current != AiStateType::Idle);
        if !temp {
            if let Some(o) = self.objects.get_mut(&id) {
                o.combat.attack = None;
                o.combat.attack_move = false;
                o.combat.guard = None;
                o.combat.hunt = false;
                o.ai.machine.group_dest = None;
            }
        }
        self.ai_private_move_to_position(id, pos, source);
    }

    // ---- ResourceGatheringManager ------------------------------------------------------------

    /// `ActionManager::canTransferSuppliesAt` (`ActionManager.cpp:207`).
    pub(crate) fn can_transfer_supplies_at(&self, truck: ObjectId, dest: ObjectId) -> bool {
        let (Some(t), Some(d)) = (self.objects.get(&truck), self.objects.get(&dest)) else { return false };
        if d.destroyed || d.is_dying() {
            return false;
        }
        if t.under_construction || d.under_construction {
            return false;
        }
        if t.template.supply_truck.is_none() {
            return false;
        }
        match d.template.dock.as_ref().map(|x| x.kind) {
            Some(DockKind::Warehouse) => {
                if d.boxes == 0 || self.relationship(d.player, t.player) == crate::damage::Relationship::Enemies {
                    return false;
                }
            }
            Some(DockKind::Center) => {
                if t.boxes == 0 || d.player != t.player {
                    return false;
                }
            }
            None => return false,
        }
        self.supply_available(truck)
    }

    /// `computeRelativeCost` (`ResourceGatheringManager.cpp:107`): `FROM_CENTER_3D` squared
    /// distance, or `None` for `FLT_MAX`.
    fn supply_relative_cost(&mut self, truck: ObjectId, dest: ObjectId) -> Option<f32> {
        if !self.can_transfer_supplies_at(truck, dest) {
            return None;
        }
        if !self.dock_is_clear_to_approach(dest, truck) {
            return None;
        }
        let (a, b) = (self.objects.get(&truck)?.position, self.objects.get(&dest)?.position);
        let (dx, dy, dz) = (b.x - a.x, b.y - a.y, b.z - a.z);
        Some(dx * dx + dy * dy + dz * dz)
    }

    /// `SupplyTruckAIUpdate::getWarehouseScanDistance`: computer players scan twice as far.
    fn warehouse_scan_distance(&self, truck: ObjectId) -> f32 {
        let Some(o) = self.objects.get(&truck) else { return 0.0 };
        let base = o.template.supply_truck.map_or(0.0, |t| t.scan_distance);
        if self.is_ai_player(o.player) {
            base * 2.0
        } else {
            base
        }
    }

    /// `ResourceGatheringManager::findBestSupplyWarehouse` (`ResourceGatheringManager.cpp:149`).
    pub(crate) fn find_best_supply_warehouse(&mut self, truck: ObjectId) -> Option<ObjectId> {
        self.objects.get(&truck)?.template.supply_truck?;
        // The player's preferred dock wins when it is remotely okay.
        let preferred = self.objects.get(&truck)?.supply_ai.preferred_dock;
        if self.objects.get(&preferred).is_some_and(|d| d.template.dock.as_ref().is_some_and(|x| x.kind == DockKind::Warehouse))
            && self.supply_relative_cost(truck, preferred).is_some()
        {
            return Some(preferred);
        }
        let scan = self.warehouse_scan_distance(truck);
        let max_dist_sqr = scan * scan;
        let candidates: Vec<ObjectId> = self
            .objects
            .values()
            .filter(|o| !o.destroyed && o.template.dock.as_ref().is_some_and(|d| d.kind == DockKind::Warehouse))
            .map(|o| o.id)
            .collect();
        let mut best: Option<(f32, ObjectId)> = None;
        for w in candidates {
            if let Some(cost) = self.supply_relative_cost(truck, w) {
                if best.is_none_or(|(bc, _)| cost < bc) && cost < max_dist_sqr {
                    best = Some((cost, w));
                }
            }
        }
        best.map(|(_, w)| w)
    }

    /// `ResourceGatheringManager::findBestSupplyCenter` (`ResourceGatheringManager.cpp:216`).
    pub(crate) fn find_best_supply_center(&mut self, truck: ObjectId) -> Option<ObjectId> {
        let player = self.objects.get(&truck)?.player;
        let preferred = self.objects.get(&truck)?.supply_ai.preferred_dock;
        if self.objects.get(&preferred).is_some_and(|d| d.template.dock.as_ref().is_some_and(|x| x.kind == DockKind::Center))
            && self.supply_relative_cost(truck, preferred).is_some()
        {
            return Some(preferred);
        }
        let candidates: Vec<ObjectId> = self
            .objects
            .values()
            .filter(|o| !o.destroyed && o.player == player && o.template.dock.as_ref().is_some_and(|d| d.kind == DockKind::Center))
            .map(|o| o.id)
            .collect();
        let mut best: Option<(f32, ObjectId)> = None;
        for c in candidates {
            if let Some(cost) = self.supply_relative_cost(truck, c) {
                if best.is_none_or(|(bc, _)| cost < bc) {
                    best = Some((cost, c));
                }
            }
        }
        best.map(|(_, c)| c)
    }

    // ---- dock actions ------------------------------------------------------------------------

    /// `SupplyTruckAIUpdate::getActionDelayForDock` (`SupplyTruckAIUpdate.cpp:208`); also
    /// `AIDockProcessDockState::setNextDockActionFrame` (`AIDock.cpp:575`): a docker with no supply
    /// interface may action right away.
    pub(crate) fn dock_action_delay(&self, id: ObjectId, dock: ObjectId) -> u32 {
        let Some(truck) = self.objects.get(&id).and_then(|o| o.template.supply_truck) else { return 0 };
        match self.objects.get(&dock).and_then(|d| d.template.dock.as_ref()).map(|d| d.kind) {
            Some(DockKind::Warehouse) => truck.warehouse_action_delay,
            Some(DockKind::Center) => truck.center_action_delay,
            None => 0,
        }
    }

    /// `SupplyWarehouseDockUpdate::action` (`SupplyWarehouseDockUpdate.cpp:84`).
    pub(crate) fn supply_warehouse_action(&mut self, docker: ObjectId, dock: ObjectId) -> bool {
        let stored = self.objects.get(&dock).map_or(0, |d| d.boxes);
        if stored == 0 {
            return false;
        }
        let Some(truck) = self.objects.get(&docker).and_then(|o| o.template.supply_truck) else { return false };
        // The docker must be reasonably close: one bounding diameter of space or less.
        let radius = self.objects.get(&docker).map_or(0.0, |o| o.template.geometry.bounding_circle_radius());
        let close_enough_sqr = (radius * 2.0) * (radius * 2.0);
        if self.boundary_dist_sqr_2d(docker, dock) > close_enough_sqr {
            // Make it twitch a little.
            let range = (0.4 * crate::pathfind::PATHFIND_CELL_SIZE_F) as i32;
            let dx = self.rng.value(-range, range) as f32;
            let dy = self.rng.value(-range, range) as f32;
            if let Some(o) = self.objects.get_mut(&docker) {
                o.set_position(crate::math::Coord3D::new(o.position.x + dx, o.position.y + dy, o.position.z));
                o.mark_partition_dirty();
            }
            return false;
        }
        // `--m_boxesStored`, so the docker sees us one box shy within `gainOneBox`.
        let remaining = stored - 1;
        if let Some(d) = self.objects.get_mut(&dock) {
            d.boxes = remaining;
        }
        // `SupplyTruckAIUpdate::gainOneBox`.
        let carried = self.objects.get(&docker).map_or(0, |o| o.boxes);
        if carried < truck.max_boxes {
            if let Some(o) = self.objects.get_mut(&docker) {
                o.boxes = carried + 1;
            }
            let delete = remaining == 0 && self.objects.get(&dock).is_some_and(|d| d.template.dock.as_ref().is_some_and(|x| x.delete_when_empty));
            if delete {
                self.destroy_object(dock);
                return false; // Yer done. And so am I.
            }
            return true;
        }
        // Nobody to gain the box: take it back.
        if let Some(d) = self.objects.get_mut(&dock) {
            d.boxes = stored;
        }
        false
    }

    /// `SupplyCenterDockUpdate::action` (`SupplyCenterDockUpdate.cpp:96`): deposit the load.
    pub(crate) fn supply_center_action(&mut self, docker: ObjectId, dock: ObjectId) -> bool {
        if self.objects.get(&docker).is_none_or(|o| o.template.supply_truck.is_none()) {
            return false;
        }
        let owner = self.objects.get(&dock).map_or(0, |d| d.player);
        let boxes = self.objects.get(&docker).map_or(0, |o| o.boxes);
        // `while (loseOneBox()) value += getSupplyBoxValue()`.
        let value = boxes.max(0) as u32 * self.rules.value_per_supply_box;
        if let Some(o) = self.objects.get_mut(&docker) {
            o.boxes = 0;
        }
        if value > 0 {
            if let Some(p) = self.players.get_mut(&owner) {
                p.deposit(value);
            }
            // `SupplyCenterDockUpdate::action` (SupplyCenterDockUpdate.cpp:126): `addMoneyEarned(value)`.
            self.score_money_earned(owner, value);
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::{LocomotorKind, LocomotorParams};
    use crate::{ObjectTemplate, TemplateSource, TerrainLogic};
    use std::sync::Arc;

    struct Store(Vec<Arc<ObjectTemplate>>);

    impl TemplateSource for Store {
        fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
            self.0.iter().find(|t| t.name == name).cloned()
        }
    }

    /// A worker: a dozer that is also a supply truck (`WorkerAIUpdate`, `is_worker`).
    fn worker_template() -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new("Worker");
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 5.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        t.kind_of |= crate::template::kind_of::DOZER;
        t.dozer = Some(crate::dozer::DozerTemplate { repair_health_percent_per_second: 0.0, bored_time_frames: 1.0, bored_range: 0.0 });
        t.supply_truck = Some(SupplyTruckTemplate { max_boxes: 1, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 100.0 });
        Arc::new(t)
    }

    fn world(t: &Arc<ObjectTemplate>) -> (GameLogic, ObjectId) {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        g.add_player(0, 1000);
        g.set_template_source(Arc::new(Store(vec![t.clone()])));
        let w = g.create_object(t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        (g, w)
    }

    /// A supply truck's machine is part of its `SupplyTruckAIUpdate::update`
    /// (`SupplyTruckAIUpdate.cpp:97-107`), which the engine runs inside the object's one AI module:
    /// it is gated by the module's disabled mask and pops at the module's heap entry. A unit disabled
    /// by EMP has its AI module deferred (`getDisabledTypesToProcess` is `DISABLED_HELD`,
    /// AIUpdate.h:305), so the supply machine does not run; the old pre-AI `update_supply` sweep ran
    /// it anyway. Revert check: with the sweep back (or the call removed from `ai_update_one`) the
    /// first assertion fails (the state advanced while disabled / never advances).
    #[test]
    fn a_plain_trucks_supply_half_runs_in_its_ai_slot_and_is_gated() {
        let mut t = ObjectTemplate::new("Truck");
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 5.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        t.supply_truck = Some(SupplyTruckTemplate { max_boxes: 1, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 100.0 });
        let t = Arc::new(t);
        let (mut g, id) = world(&t);
        g.run_frames(2);
        g.set_supply_force_wanting(id, true);
        g.set_disabled(id, crate::disabled::DisabledType::Emp);
        let before = g.find_object(id).unwrap().supply_ai.state;
        g.run_frames(3);
        assert_eq!(
            g.find_object(id).unwrap().supply_ai.state,
            before,
            "a disabled truck's supply machine does not run (the AI module is deferred)"
        );
        assert!(g.find_object(id).unwrap().supply_ai.force_wanting, "nothing consumed the pending force");
        // Clearing the EMP lets the AI module run; its supply half then advances the machine.
        assert!(g.clear_disabled(id, crate::disabled::DisabledType::Emp));
        g.update();
        assert_ne!(
            g.find_object(id).unwrap().supply_ai.state,
            before,
            "the truck's AI slot runs its supply half once it is enabled"
        );
    }

    /// `SupplyTruckAIUpdate::update` has no availability gate (`SupplyTruckAIUpdate.cpp:97-107`);
    /// `isAvailableForSupplying` is only the `ownerAvailableForSupplying` state condition
    /// (`:795-815`). A Chinook carrying passengers (or in a drop) therefore still advances its supply
    /// machine, e.g. `Idle` -> `Wanting` on a pending force. Revert check: with the early-out back the
    /// state stays `Idle`.
    #[test]
    fn an_unavailable_chinooks_supply_machine_keeps_running() {
        let mut t = ObjectTemplate::new("Chinook");
        t.max_health = 300.0;
        t.initial_health = 300.0;
        t.geometry.major_radius = 10.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Wings, 60.0, 100.0, 100.0, 360.0));
        t.kind_of |= crate::template::kind_of::AIRCRAFT;
        t.chinook = Some(crate::chinook::ChinookTemplate::default());
        t.supply_truck = Some(SupplyTruckTemplate { max_boxes: 1, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 100.0 });
        let t = Arc::new(t);
        let (mut g, id) = world(&t);
        g.run_frames(2);
        // A combat-drop entry in the port's Chinook map is one of the two `isAvailableForSupplying`
        // conditions (`ChinookAIUpdate.cpp:1016-1030`).
        g.chinooks.insert(
            id,
            crate::chinook::ChinookState {
                phase: crate::chinook::ChinookPhase::CombatDrop,
                target_pos: Coord3D::ZERO,
                target_obj: None,
                dest_z: 0.0,
                ropes: Vec::new(),
                done: false,
            },
        );
        {
            let o = g.objects.get_mut(&id).unwrap();
            o.supply_ai.state = SupplyState::Idle;
            o.supply_ai.force_wanting = true;
        }
        assert!(!g.supply_available(id), "the drop makes the Chinook unavailable");
        g.supply_ai_update_one(id);
        assert_eq!(
            g.find_object(id).unwrap().supply_ai.state,
            SupplyState::Wanting,
            "the unavailable Chinook's machine still transitions (Idle -> Wanting)"
        );
        assert!(!g.find_object(id).unwrap().supply_ai.force_wanting, "Wanting's onEnter consumes the force");
        // The Chinook's own `AIUpdateInterface` state is idle, so `OwnerAvailableForSupplying` is false
        // and the machine does not advance further.
        assert_eq!(g.find_object(id).unwrap().supply_ai.state, SupplyState::Wanting);
    }

    /// `WorkerAIUpdate::exitingSupplyTruckState` (WorkerAIUpdate.cpp:467-482) goes to `AS_DOZER`, and that
    /// state's `supplyTruckSubMachineWantsToEnter` (WorkerAIUpdate.cpp:1328-1342) is checked at once: a
    /// pending force brings the worker straight back to `AS_SUPPLY_TRUCK` in the same update.
    #[test]
    fn a_pending_force_returns_an_idling_worker_to_supply_mode() {
        let t = worker_template();
        let (mut g, w) = world(&t);
        g.run_frames(3);
        {
            let o = g.objects.get_mut(&w).unwrap();
            o.supply_ai.worker_supply = true;
            o.supply_ai.force_wanting = true;
        }
        g.exiting_supply_truck_state(w);
        assert!(g.find_object(w).unwrap().supply_ai.worker_supply, "a forced worker stays in supply mode");
        {
            let o = g.objects.get_mut(&w).unwrap();
            o.supply_ai.worker_supply = true;
            o.supply_ai.force_wanting = false;
        }
        g.exiting_supply_truck_state(w);
        assert!(!g.find_object(w).unwrap().supply_ai.worker_supply, "an unforced worker goes back to AS_DOZER");
    }
}
