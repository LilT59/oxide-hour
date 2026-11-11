//! Production and production exits. Ported from `GameLogic/Object/Update/ProductionUpdate.cpp`
//! (`queueCreateUnit`, `cancelUnitCreate`, `update`, `addToProductionQueue` /
//! `removeFromProductionQueue`, `canQueueCreateUnit`), `Common/System/BuildAssistant.cpp`
//! (`canMakeUnit`), `Common/RTS/ProductionPrerequisite.cpp` (`isSatisfied`),
//! `Common/Thing/ThingTemplate.cpp` (`calcCostToBuild` / `calcTimeToBuild`, power/handicap factors
//! at 1.0) and the production exit updates (`DefaultProductionExitUpdate.cpp` and friends).
//!
//! The engine's `ProductionUpdate` is a sleepy update; here it is a dedicated per-frame pass over
//! factories in id order. Doors (`ProductionUpdate::updateDoors` and the door block of `update`) are
//! ported for every factory: a factory's own door is [`ProductionState::door`], and an airfield's
//! doors are its parking spaces (`jet_ai`). `MaxSimultaneousOfType`, command sets, power, sciences,
//! `ConstructionCompleteDuration` (the `CONSTRUCTION_COMPLETE` condition) is not ported.

use std::sync::Arc;

use crate::crc::Crc;
use crate::jet_ai::DoorRt;
use crate::logic::GameLogic;
use crate::math::{cos, sin, Coord3D};
use crate::object::{ObjectId, PlayerIndex};
use crate::template::ObjectTemplate;

/// `ProductionPrerequisite`: an OR group of object names (`Object = A B C`), or a science.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prerequisite {
    /// `Object = A B C`: the player must own at least one of the names.
    AnyOfObjects(Vec<String>),
    /// `Science = X`: sciences are not ported, so this is treated as satisfied.
    Science(String),
}

/// `ProductionUpdateModuleData::QuantityModifier`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuantityModifier {
    pub template_name: String,
    pub quantity: i32,
}

/// The producing building's exit update (`ExitInterface`, `Object::getObjectExitInterface`).
#[derive(Clone, Debug, PartialEq)]
pub enum ProductionExit {
    /// `DefaultProductionExitUpdate`.
    Default { unit_create_point: (f32, f32, f32), natural_rally_point: (f32, f32, f32) },
    /// `QueueProductionExitUpdate`: `InitialBurst` units leave with no delay, then one exit every
    /// `exit_delay_frames` (`isFreeToExit`, `QueueProductionExitUpdate.cpp:201`).
    Queue {
        unit_create_point: (f32, f32, f32),
        natural_rally_point: (f32, f32, f32),
        exit_delay_frames: u32,
        initial_burst: u32,
        /// `AllowAirborneCreation`: a create point off the ground keeps its height.
        allow_airborne_creation: bool,
    },
    /// `SupplyCenterProductionExitUpdate`. `grant_temporary_stealth` is `GrantTemporaryStealth` (frames).
    SupplyCenter { unit_create_point: (f32, f32, f32), natural_rally_point: (f32, f32, f32), grant_temporary_stealth: u32 },
    /// `SpawnPointProductionExitUpdate`: each unit stands at one of the `SpawnPointBoneName` bones and
    /// stays held there. The bones come from the model (see `zr-game::spawn_bones`).
    SpawnPoint { bones: Vec<SpawnPointBone> },
}

/// One `SpawnPointBoneName` bone (`SpawnPointProductionExitUpdate::initializeBonePositions`): its
/// position in object space and its yaw (radians) relative to the building.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpawnPointBone {
    pub pos: (f32, f32, f32),
    pub angle: f32,
}

impl Default for ProductionExit {
    fn default() -> Self {
        ProductionExit::Default { unit_create_point: (0.0, 0.0, 0.0), natural_rally_point: (0.0, 0.0, 0.0) }
    }
}

/// The C++ class of a [`ProductionExit`], for the sleepy heap's `mod=` column.
///
/// `UpdateModule` subclasses (`ProductionExitUpdate.cpp` and `ProductionExitUpdate/`): `DefaultProductionExitUpdate`,
/// `QueueProductionExitUpdate`, `SupplyCenterProductionExitUpdate`, `SpawnPointProductionExitUpdate`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProductionExitClass {
    Default,
    Queue,
    SupplyCenter,
    SpawnPoint,
}

impl ProductionExit {
    /// The C++ module class this exit data came from.
    pub(crate) fn class(&self) -> ProductionExitClass {
        match self {
            ProductionExit::Default { .. } => ProductionExitClass::Default,
            ProductionExit::Queue { .. } => ProductionExitClass::Queue,
            ProductionExit::SupplyCenter { .. } => ProductionExitClass::SupplyCenter,
            ProductionExit::SpawnPoint { .. } => ProductionExitClass::SpawnPoint,
        }
    }
}

impl ProductionExitClass {
    /// The C++ class name, as the `ZR_L99_HEAP` dump prints it.
    pub(crate) fn class_name(self) -> &'static str {
        match self {
            ProductionExitClass::Default => "DefaultProductionExitUpdate",
            ProductionExitClass::Queue => "QueueProductionExitUpdate",
            ProductionExitClass::SupplyCenter => "SupplyCenterProductionExitUpdate",
            ProductionExitClass::SpawnPoint => "SpawnPointProductionExitUpdate",
        }
    }
}

/// A model-space point (`UnitCreatePoint`, `NaturalRallyPoint`).
type ModelPoint = (f32, f32, f32);

impl ProductionExit {
    /// `UnitCreatePoint` and `NaturalRallyPoint`, in model space (`None` for a spawn point exit).
    fn points(&self) -> Option<(ModelPoint, ModelPoint)> {
        match self {
            ProductionExit::Default { unit_create_point, natural_rally_point }
            | ProductionExit::Queue { unit_create_point, natural_rally_point, .. }
            | ProductionExit::SupplyCenter { unit_create_point, natural_rally_point, .. } => {
                Some((*unit_create_point, *natural_rally_point))
            }
            ProductionExit::SpawnPoint { .. } => None,
        }
    }
}

/// Which exit a unit leaves through once `reserveDoorForExit` has taken it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExitSlot {
    /// The create point (`DOOR_1` of the non-spawn exits).
    Create,
    /// `SpawnPointProductionExitUpdate` spawn point `i`.
    Spawn(usize),
}

/// The offset `getNaturalRallyPoint(offset = TRUE)` adds to the natural rally point, along its model-space
/// direction: `2 * PATHFIND_CELL_SIZE_F` (`DefaultProductionExitUpdate.cpp:159-161`).
const NATURAL_RALLY_OFFSET: f32 = 2.0 * crate::pathfind::PATHFIND_CELL_SIZE_F;

/// What a factory can produce (`ProductionUpdate` + its exit update).
#[derive(Clone, Debug)]
pub struct ProductionTemplate {
    pub max_queue_entries: i32,
    pub quantity_modifiers: Vec<QuantityModifier>,
    pub exit: ProductionExit,
    /// `DisabledTypesToProcess` (default `DISABLED_HELD`, ProductionUpdate.cpp:105): the update runs
    /// while the object's disabled mask is empty or intersects this.
    pub disabled_types_to_process: u16,
    /// `NumDoorAnimations` / `DoorOpeningTime` / `DoorWaitOpenTime` / `DoorCloseTime` (frames). A factory
    /// with `num_door_animations > 0` holds each finished unit until its door is fully open; an airfield's
    /// door is also held open by `ParkingPlaceBehavior` while a jet is reserved.
    pub num_door_animations: i32,
    pub door_opening_time: u32,
    pub door_wait_open_time: u32,
    pub door_close_time: u32,
}

impl Default for ProductionTemplate {
    /// `ProductionUpdateModuleData` defaults (`MaxQueueEntries` 9, `DisabledTypesToProcess` HELD).
    fn default() -> Self {
        ProductionTemplate {
            max_queue_entries: 9,
            quantity_modifiers: Vec::new(),
            exit: ProductionExit::default(),
            disabled_types_to_process: crate::disabled::DisabledType::Held.bit(),
            num_door_animations: 0,
            door_opening_time: 0,
            door_wait_open_time: 0,
            door_close_time: 0,
        }
    }
}

/// One queued production item.
#[derive(Clone, Debug)]
pub enum ProductionKind {
    Unit(Arc<ObjectTemplate>),
    Upgrade(String),
}

/// `ProductionEntry`.
#[derive(Clone, Debug)]
pub struct ProductionEntry {
    pub kind: ProductionKind,
    pub quantity_total: i32,
    pub quantity_produced: i32,
    /// `m_framesUnderConstruction`.
    pub frames: u32,
    /// `ProductionEntry::m_exitDoor`: the door reserved for this entry — the airfield parking space
    /// (`reserveDoorForExit` at queue time) or `DOOR_1` (0) of a normal factory exit, taken when the
    /// unit is ready to leave (`ProductionUpdate.cpp:710-716`) and cleared after it exits
    /// (`oneProductionSuccessful`, `ProductionUpdate.h:89`). `None` is `DOOR_NONE_AVAILABLE`.
    pub door: Option<usize>,
    /// The upgrade is an object upgrade (`UPGRADE_TYPE_OBJECT`): it is given to the producing object when it
    /// finishes (`ProductionUpdate.cpp:900`, `us->giveUpgrade`), not to the player. Always false for units.
    pub object_upgrade: bool,
}

/// Per-factory production state (`ProductionUpdate`'s queue, rally point and the exit update's state).
#[derive(Clone, Debug, Default)]
pub struct ProductionState {
    pub queue: Vec<ProductionEntry>,
    pub rally_point: Option<Coord3D>,
    /// `QueueProductionExitUpdate::m_currentDelay`: frames until the queue exit is free again.
    pub exit_delay_left: u32,
    /// `QueueProductionExitUpdate::m_currentBurstCount`: how many more units may leave with no delay.
    pub exit_burst_left: u32,
    /// `SpawnPointProductionExitUpdate::m_spawnPointOccupier`: the unit standing at each spawn point
    /// (`None` is `INVALID_ID`). Sized to the bones on first use.
    pub spawn_occupiers: Vec<Option<ObjectId>>,
    /// The factory's own door, `ProductionUpdate::m_doors[DOOR_1]`. Every non-airfield exit
    /// (`DefaultProductionExitUpdate`, `SupplyCenterProductionExitUpdate`, and `QueueProductionExitUpdate`
    /// while free) reserves `DOOR_1` (their `reserveDoorForExit`), so a factory has exactly one door.
    pub door: DoorRt,
}

impl ProductionState {
    /// A factory's fresh state: `QueueProductionExitUpdate`'s constructor sets `m_currentBurstCount`
    /// from `InitialBurst` (`QueueProductionExitUpdate.cpp:52-57`).
    pub(crate) fn new(p: &ProductionTemplate) -> Self {
        let burst = match &p.exit {
            ProductionExit::Queue { initial_burst, .. } => *initial_burst,
            _ => 0,
        };
        ProductionState { exit_burst_left: burst, ..Default::default() }
    }
}

/// `ProductionUpdateModuleData`'s door times (frames), as `ProductionUpdate::updateDoors` reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DoorTiming {
    /// `DoorOpeningTime`.
    pub opening: u32,
    /// `DoorWaitOpenTime`.
    pub wait_open: u32,
    /// `DoorCloseTime`.
    pub closing: u32,
}

impl DoorTiming {
    pub(crate) fn of(p: &ProductionTemplate) -> Self {
        DoorTiming { opening: p.door_opening_time, wait_open: p.door_wait_open_time, closing: p.door_close_time }
    }
}

/// `ProductionUpdate::updateDoors` (`ProductionUpdate.cpp:491-561`) for one door: an opened door becomes
/// waiting-open after `DoorOpeningTime`, a waiting-open door starts closing after `DoorWaitOpenTime`
/// unless it is held, and a closing door is closed after `DoorCloseTime`.
pub(crate) fn update_door(d: &mut DoorRt, t: DoorTiming, now: u32) {
    if d.opened_frame != 0 {
        if now - d.opened_frame > t.opening {
            d.opened_frame = 0;
            d.wait_open_frame = now;
        }
    } else if d.wait_open_frame != 0 {
        if now - d.wait_open_frame > t.wait_open && !d.hold_open {
            d.wait_open_frame = 0;
            d.closed_frame = now;
        }
    } else if d.closed_frame != 0 && !d.hold_open && now - d.closed_frame > t.closing {
        d.closed_frame = 0;
    }
}

/// The door block of `ProductionUpdate::update` (`ProductionUpdate.cpp:719-754`) for a unit whose exit
/// door is `d`: a closed door starts opening, a waiting-open door stays open, and a closing door is
/// popped back to waiting-open. Returns whether the door is fully open (`m_doorWaitOpenFrame != 0`),
/// which gates creating the unit (`ProductionUpdate.cpp:771`).
pub(crate) fn open_door_for_exit(d: &mut DoorRt, now: u32) -> bool {
    if d.opened_frame == 0 && d.wait_open_frame == 0 && d.closed_frame == 0 {
        // "if the door is closed, open it"
        d.opened_frame = now;
    } else if d.wait_open_frame != 0 {
        // "if the door is waiting-open, keep it there"
        d.wait_open_frame = now;
    } else if d.closed_frame != 0 {
        // "if the door is closing, for now, pop it to waiting open"
        d.wait_open_frame = now;
    }
    d.wait_open_frame != 0
}

impl ProductionState {
    pub(crate) fn crc(&self, c: &mut Crc) {
        c.add_u32(self.queue.len() as u32);
        for e in &self.queue {
            match &e.kind {
                ProductionKind::Unit(u) => {
                    c.add_bool(true);
                    c.add_str(&u.name);
                }
                ProductionKind::Upgrade(n) => {
                    c.add_bool(false);
                    c.add_str(n);
                }
            }
            c.add_u32(e.quantity_total as u32);
            c.add_u32(e.quantity_produced as u32);
            c.add_u32(e.frames);
        }
        match &self.rally_point {
            Some(p) => {
                c.add_bool(true);
                c.add_coord(p);
            }
            None => c.add_bool(false),
        }
        c.add_u32(self.exit_delay_left);
        c.add_u32(self.exit_burst_left);
        c.add_u32(self.spawn_occupiers.len() as u32);
        for occupier in &self.spawn_occupiers {
            c.add_u32(occupier.map_or(u32::MAX, |id| id.0));
        }
        c.add_u32(self.door.opened_frame);
        c.add_u32(self.door.wait_open_frame);
        c.add_u32(self.door.closed_frame);
        c.add_bool(self.door.hold_open);
    }
}

impl GameLogic {
    /// `ProductionUpdate::queueCreateUnit` for a unit named `template_name`.
    pub fn queue_unit(&mut self, player: PlayerIndex, factory: ObjectId, template_name: &str) -> bool {
        let Some(unit) = self.template_source.as_ref().and_then(|s| s.find_template(template_name)) else {
            return false;
        };
        let (production, queued, factory_player, alive) = match self.objects.get(&factory) {
            Some(o) => (o.template.production.clone(), o.production.as_ref().map_or(0, |p| p.queue.len()), o.player, !o.destroyed),
            None => return false,
        };
        if !alive || factory_player != player {
            return false;
        }
        let Some(production) = production else { return false };
        if production.max_queue_entries >= 0 && queued as i32 >= production.max_queue_entries {
            return false;
        }
        // `BuildAssistant::canMakeUnit` (`BuildAssistant.cpp:1313-1343`), which `queueCreateUnit` runs first
        // (`ProductionUpdate.cpp:358`): script-disabled builders and builders whose command set lacks the unit.
        if !self.can_make_unit_from_factory(factory, &unit) {
            return false;
        }
        if !self.prerequisites_satisfied(player, &unit) {
            return false;
        }
        let cost = self.calc_cost_to_build(player, &unit);
        if self.players.get(&player).is_none_or(|p| p.money < cost) {
            return false;
        }
        // `ProductionUpdate::canQueueCreateUnit` / `queueCreateUnit` (`ProductionUpdate.cpp:233`, `:367`):
        // an airfield refuses the unit when its parking places are full, and holds a door
        // (parking space) for it from the moment it is queued.
        let mut door = None;
        if self.objects.get(&factory).is_some_and(|o| o.parking.is_some()) && !unit.is_collide_kind_of(crate::template::collide_kind::PRODUCED_AT_HELIPAD) {
            if !self.pp_has_available_space_for(factory, &unit) {
                return false; // `CANMAKE_PARKING_PLACES_FULL`
            }
            match self.pp_reserve_door_for_exit(factory, &unit) {
                Some(d) => door = d,
                None => return false, // `DOOR_NONE_AVAILABLE`
            }
        }
        let Some(p) = self.players.get_mut(&player) else { return false };
        p.withdraw(cost);
        let quantity = production
            .quantity_modifiers
            .iter()
            .find(|q| q.template_name == unit.name)
            .map_or(1, |q| q.quantity)
            .max(1);
        if let Some(o) = self.objects.get_mut(&factory) {
            if let Some(st) = o.production.as_mut() {
                st.queue.push(ProductionEntry {
                    kind: ProductionKind::Unit(unit),
                    quantity_total: quantity,
                    quantity_produced: 0,
                    frames: 0,
                    door,
                    object_upgrade: false,
                });
            }
        }
        true
    }

    /// `ProductionUpdate::cancelUnitCreate(productionID)`: refund and drop the entry at `index`.
    pub fn cancel_unit(&mut self, player: PlayerIndex, factory: ObjectId, index: usize) -> bool {
        let entry = {
            let Some(o) = self.objects.get_mut(&factory) else { return false };
            if o.destroyed || o.player != player {
                return false;
            }
            let Some(st) = o.production.as_mut() else { return false };
            if index >= st.queue.len() {
                return false;
            }
            st.queue.remove(index)
        };
        // `ProductionUpdate::cancelUnitCreate`: `unreserveDoorForExit(production->getExitDoor())`.
        if let Some(d) = entry.door {
            self.pp_unreserve_door_for_exit(factory, d);
        }
        let refund = match &entry.kind {
            // `ProductionUpdate::cancelUnitCreate` refunds `calcCostToBuild(player)` (ProductionUpdate.cpp:1098).
            ProductionKind::Unit(u) => self.calc_cost_to_build(player, u),
            ProductionKind::Upgrade(n) => self.upgrade_info.get(n).map_or(0, |i| i.cost),
        };
        if let Some(p) = self.players.get_mut(&player) {
            p.deposit(refund);
        }
        true
    }

    /// The polyline a factory's rally point draws (`W3dWaypointBuffer::drawWaypoints`): the unit create
    /// point, the natural rally point, then the rally point. `None` without a rally point, or for a spawn
    /// point exit (which has no create or natural point).
    pub fn rally_route(&self, factory: ObjectId) -> Option<Vec<Coord3D>> {
        let o = self.objects.get(&factory)?;
        let rally = o.production.as_ref()?.rally_point?;
        let exit = o.template.production.as_ref().map(|p| p.exit.clone()).unwrap_or_default();
        let (create, natural) = exit.points()?;
        // `getNaturalRallyPoint(offset = TRUE)` for the default and queue exits, not for the supply center.
        let offset = !matches!(exit, ProductionExit::SupplyCenter { .. });
        Some(vec![
            transform_model_point(&o.position, o.angle, create),
            natural_rally_world(&o.position, o.angle, natural, offset),
            rally,
        ])
    }

    /// Set a factory's rally point (`ExitInterface::setRallyPoint`).
    pub fn set_rally_point(&mut self, player: PlayerIndex, factory: ObjectId, pos: Coord3D) -> bool {
        let Some(o) = self.objects.get_mut(&factory) else { return false };
        if o.destroyed || o.player != player {
            return false;
        }
        let Some(st) = o.production.as_mut() else { return false };
        st.rally_point = Some(pos);
        // `ParkingPlaceBehavior::setRallyPoint` (the airfield's `ExitInterface`): only helicopters use it.
        if o.parking.is_some() {
            self.pp_set_heli_rally(factory, pos);
        }
        true
    }

    /// `ProductionUpdate::onDie` -> `cancelAndRefundAllProduction`: refund and drop every queued
    /// entry when the factory dies.
    pub(crate) fn refund_all_production(&mut self, factory: ObjectId) {
        let (entries, player) = {
            let Some(o) = self.objects.get_mut(&factory) else { return };
            let Some(st) = o.production.as_mut() else { return };
            let player = o.player;
            (std::mem::take(&mut st.queue), player)
        };
        for e in &entries {
            if let Some(d) = e.door {
                self.pp_unreserve_door_for_exit(factory, d);
            }
        }
        let refund: u32 = entries
            .iter()
            .map(|e| match &e.kind {
                // `ProductionUpdate::cancelUnitCreate` refunds `calcCostToBuild(player)` (ProductionUpdate.cpp:1098).
                ProductionKind::Unit(u) => self.calc_cost_to_build(player, u),
                ProductionKind::Upgrade(n) => self.upgrade_info.get(n).map_or(0, |i| i.cost),
            })
            .sum();
        if let Some(p) = self.players.get_mut(&player) {
            p.deposit(refund);
        }
    }

    /// `ProductionUpdate::update` for one factory, run from the sleepy heap. The module is awake every frame: the
    /// update returns `UPDATE_SLEEP_NONE` on every path (ProductionUpdate.cpp:565-938), and nothing wakes it from
    /// outside. The disabled-types gate (`DisabledTypesToProcess`, default HELD) is the heap's (`GameLogic.cpp:3851-3885`).
    /// `QueueProductionExitUpdate` is a second module of the object and ticks in its own family slot.
    pub(crate) fn production_update(&mut self, id: ObjectId) -> u32 {
        if self.objects.get(&id).is_some_and(|o| o.production.is_none()) {
            return crate::module::UPDATE_SLEEP_NONE;
        }
        // `ProductionUpdate::update` -> `updateDoors` first (`ProductionUpdate.cpp:574`): an airfield's
        // hangar doors, or the factory's own door.
        if self.objects.get(&id).is_some_and(|o| o.parking.is_some()) {
            self.pp_update_doors(id);
        } else {
            self.update_factory_door(id);
        }
        self.update_factory_production(id);
        crate::module::UPDATE_SLEEP_NONE
    }

    /// `QueueProductionExitUpdate::update` (`QueueProductionExitUpdate.cpp:215`): every frame, a free exit
    /// stays free with no delay, and a busy one counts its delay down. The update runs whatever the queue
    /// holds, so the delay keeps running down while the factory is idle.
    pub(crate) fn tick_queue_exit(&mut self, factory: ObjectId) {
        let Some(o) = self.objects.get_mut(&factory) else { return };
        if !matches!(o.template.production.as_ref().map(|p| &p.exit), Some(ProductionExit::Queue { .. })) {
            return;
        }
        let Some(st) = o.production.as_mut() else { return };
        if queue_exit_free(st) {
            st.exit_delay_left = 0;
        } else {
            st.exit_delay_left -= 1;
        }
    }

    /// `ExitInterface::reserveDoorForExit` for a non-airfield factory (`ProductionUpdate.cpp:704`): the
    /// slot the next unit leaves through, or `None` when the exit is busy. A queue exit is free only
    /// while `isFreeToExit`, a spawn point exit needs an unoccupied bone (`reserveDoorForExit`,
    /// `SpawnPointProductionExitUpdate.cpp:117`), and the other exits always reserve `DOOR_1`.
    pub(crate) fn reserve_exit_slot(&mut self, factory: ObjectId) -> Option<ExitSlot> {
        let o = self.objects.get(&factory)?;
        match o.template.production.as_ref().map(|p| &p.exit)? {
            ProductionExit::Queue { .. } => {
                let st = o.production.as_ref()?;
                queue_exit_free(st).then_some(ExitSlot::Create)
            }
            ProductionExit::SpawnPoint { bones } => {
                let n = bones.len();
                self.reserve_spawn_point(factory, n).map(ExitSlot::Spawn)
            }
            ProductionExit::Default { .. } | ProductionExit::SupplyCenter { .. } => Some(ExitSlot::Create),
        }
    }

    /// `SpawnPointProductionExitUpdate::reserveDoorForExit` (`:117`): forget occupants that no longer
    /// exist (`revalidateOccupiers`, `:179`), then take the first free spawn point.
    fn reserve_spawn_point(&mut self, factory: ObjectId, count: usize) -> Option<usize> {
        let occupiers = self.objects.get(&factory)?.production.as_ref()?.spawn_occupiers.clone();
        let mut occupiers: Vec<Option<ObjectId>> = (0..count).map(|i| occupiers.get(i).copied().flatten()).collect();
        for slot in occupiers.iter_mut() {
            if slot.is_some_and(|id| !self.objects.contains_key(&id)) {
                *slot = None;
            }
        }
        let free = occupiers.iter().position(|s| s.is_none());
        if let Some(st) = self.objects.get_mut(&factory).and_then(|o| o.production.as_mut()) {
            st.spawn_occupiers = occupiers;
        }
        free
    }

    /// `ProductionUpdate::updateDoors` for a factory's own door, run when `NumDoorAnimations > 0`
    /// (`ProductionUpdate.cpp:574`).
    fn update_factory_door(&mut self, factory: ObjectId) {
        let now = self.frame;
        let Some(o) = self.objects.get_mut(&factory) else { return };
        let Some(p) = o.template.production.as_ref() else { return };
        if p.num_door_animations <= 0 {
            return;
        }
        let t = DoorTiming::of(p);
        if let Some(st) = o.production.as_mut() {
            update_door(&mut st.door, t, now);
        }
    }

    /// The factory's door block for a finished unit (`ProductionUpdate.cpp:719-771`): whether the unit
    /// may leave yet. A factory without door animations exits at once (`d->m_numDoorAnimations == 0`).
    fn factory_door_open_for_exit(&mut self, factory: ObjectId) -> bool {
        let now = self.frame;
        let Some(o) = self.objects.get_mut(&factory) else { return false };
        let n = o.template.production.as_ref().map_or(0, |p| p.num_door_animations);
        if n <= 0 {
            return true;
        }
        match o.production.as_mut() {
            Some(st) => open_door_for_exit(&mut st.door, now),
            None => false,
        }
    }

    fn update_factory_production(&mut self, factory: ObjectId) {
        // `ProductionUpdate` upgrade entries finish and apply to the owner's upgrade set.
        let upgrade = match self.objects.get(&factory) {
            Some(o) => o.production.as_ref().and_then(|st| st.queue.first()).and_then(|e| match &e.kind {
                ProductionKind::Upgrade(n) => Some((n.clone(), o.player, e.object_upgrade)),
                _ => None,
            }),
            None => None,
        };
        if let Some((name, player, object)) = upgrade {
            let total = self.upgrade_build_frames(&name);
            let done = {
                let Some(o) = self.objects.get_mut(&factory) else { return };
                let Some(st) = o.production.as_mut() else { return };
                let Some(head) = st.queue.first_mut() else { return };
                head.frames += 1;
                head.frames >= total
            };
            if done {
                if let Some(o) = self.objects.get_mut(&factory) {
                    if let Some(st) = o.production.as_mut() {
                        st.queue.remove(0);
                    }
                }
                // `ProductionUpdate::updateProduction` (ProductionUpdate.cpp:855): the finished upgrade's
                // cost is `addMoneySpent` on the owner's keeper.
                let cost = self.upgrade_info.get(&name).map_or(0, |i| i.cost);
                self.score_money_spent(player, cost);
                // ProductionUpdate.cpp:858-861: `TheScriptEngine->notifyOfCompletedUpgrade(player, name, us->getID())`,
                // before the upgrade is given, for either type. The source is the producing object.
                self.scripts.completed_upgrades.entry(player).or_default().push((name.clone(), factory));
                // ProductionUpdate.cpp:900-906: a player upgrade completes on the player, an object upgrade
                // is given to the producing object (`us->giveUpgrade`).
                if object {
                    self.give_upgrade(factory, &name);
                } else {
                    self.apply_player_upgrade(player, &name);
                }
            }
            return;
        }
        let (unit, player, quantity_remaining) = {
            let Some(o) = self.objects.get(&factory) else { return };
            let Some(st) = o.production.as_ref() else { return };
            let Some(head) = st.queue.first() else { return };
            let ProductionKind::Unit(unit) = &head.kind else { return };
            (unit.clone(), o.player, head.quantity_total - head.quantity_produced)
        };
        // `ThingTemplate::calcTimeToBuild`: the low-power penalty applies to factory production too.
        let total = self.effective_build_time_frames(&unit, player);

        let ready = {
            let Some(o) = self.objects.get_mut(&factory) else { return };
            let Some(st) = o.production.as_mut() else { return };
            let Some(head) = st.queue.first_mut() else { return };
            head.frames += 1;
            head.frames >= total
        };
        if !ready {
            return;
        }

        // `ProductionUpdate::update` (`ProductionUpdate.cpp:698-821`): each unit still to make tries the
        // exit; a refused one stays in the queue as "overbuilding" and is retried next frame.
        let mut produced = 0;
        for _ in 0..quantity_remaining {
            // An airfield exits a unit through its reserved parking space (`ParkingPlaceBehavior` is
            // the `ExitInterface`): "if (exitDoor == DOOR_NONE_AVAILABLE) exitDoor = reserveDoorForExit".
            let parking = self.objects.get(&factory).is_some_and(|o| o.parking.is_some());
            if parking {
                let held = self.objects.get_mut(&factory).and_then(|o| o.production.as_mut()).and_then(|st| st.queue.first_mut()).and_then(|e| e.door.take());
                let door = match held {
                    Some(d) => Some(d),
                    None => match self.pp_reserve_door_for_exit(factory, &unit) {
                        Some(d) => d,
                        None => break, // no door available: keep "overbuilding" until one frees up
                    },
                };
                // "for production buildings that have door animations we will not make the object
                // until the door has been totally opened": keep the reservation on the entry.
                if let Some(d) = door {
                    if !self.pp_open_door_for_exit(factory, d) {
                        if let Some(e) = self.objects.get_mut(&factory).and_then(|o| o.production.as_mut()).and_then(|st| st.queue.first_mut()) {
                            e.door = Some(d);
                        }
                        break;
                    }
                }
                self.spawn_produced_unit_via_door(factory, &unit, door, false);
            } else {
                // `ProductionUpdate::update` (`:710-716`): a fresh reservation is stored on the entry
                // (`setExitDoor`). Every non-airfield exit reserves `DOOR_1` (`QueueProductionExitUpdate.cpp:191`,
                // `SpawnPointProductionExitUpdate.cpp:117`); the port's `ExitSlot` is which create/spawn point
                // that door means. The slot is derived again rather than remembered because a spawn point is
                // only occupied once the unit actually exits (`exitObjectViaDoor`).
                let Some(slot) = self.reserve_exit_slot(factory) else { break };
                if let Some(e) = self.objects.get_mut(&factory).and_then(|o| o.production.as_mut()).and_then(|st| st.queue.first_mut()) {
                    e.door = Some(0);
                }
                if !self.factory_door_open_for_exit(factory) {
                    // The factory's own door is not fully open yet: the unit waits ("overbuilding"), and the
                    // remaining iterations of `ProductionUpdate::update`'s loop would make the same choice.
                    // The reserved door stays on the entry (`m_exitDoor`), so it prints `DOOR_1` while it waits.
                    break;
                }
                let exited = self.exit_factory_unit(factory, &unit, slot);
                if exited != ObjectId::INVALID {
                    // `oneProductionSuccessful()` (`ProductionUpdate.h:89`): "mark door to re-reserve", after
                    // `ProductionUpdate.cpp:792` set `m_exitDoor` back to `DOOR_NONE_AVAILABLE`.
                    if let Some(e) = self.objects.get_mut(&factory).and_then(|o| o.production.as_mut()).and_then(|st| st.queue.first_mut()) {
                        e.door = None;
                    }
                }
            }
            produced += 1;
        }
        if produced == 0 {
            return;
        }
        let done = {
            let Some(o) = self.objects.get_mut(&factory) else { return };
            let Some(st) = o.production.as_mut() else { return };
            let Some(head) = st.queue.first_mut() else { return };
            head.quantity_produced += produced;
            head.quantity_produced >= head.quantity_total
        };
        if done {
            if let Some(o) = self.objects.get_mut(&factory) {
                if let Some(st) = o.production.as_mut() {
                    st.queue.remove(0);
                }
            }
        }
    }

    /// `SpawnBehavior::createSpawn`'s exit: reserve the factory's exit (`reserveDoorForExit`), then exit
    /// through it. Returns `INVALID_ID` when the exit is busy (`createSpawn` returns FALSE). The spawn is
    /// created and `Player::onUnitCreated` runs before the exit (`SpawnBehavior.cpp:635`, then `:686` or
    /// `:728`), so the exit's path replaces what the AI did on creation (a supply truck's dock).
    pub(crate) fn spawn_produced_unit(&mut self, factory: ObjectId, unit: &Arc<ObjectTemplate>) -> ObjectId {
        if self.objects.get(&factory).is_some_and(|o| o.parking.is_some()) {
            return self.spawn_produced_unit_via_door(factory, unit, None, true);
        }
        match self.reserve_exit_slot(factory) {
            Some(slot) => self.exit_factory_unit_ordered(factory, unit, slot, true),
            None => ObjectId::INVALID,
        }
    }

    /// [`spawn_produced_unit`](Self::spawn_produced_unit) through a reserved airfield `door`
    /// (`ParkingPlaceBehavior::exitObjectViaDoor`) when the factory has a parking place. `created_first`
    /// is the `SpawnBehavior` order (creation hooks before the exit); `ProductionUpdate` exits first
    /// (`ProductionUpdate.cpp:788`, `:800`).
    pub(crate) fn spawn_produced_unit_via_door(&mut self, factory: ObjectId, unit: &Arc<ObjectTemplate>, door: Option<usize>, created_first: bool) -> ObjectId {
        if self.objects.get(&factory).is_some_and(|o| o.parking.is_some()) {
            let (fpos, fangle, fplayer) = match self.objects.get(&factory) {
                Some(o) => (o.position, o.angle, o.player),
                None => return ObjectId::INVALID,
            };
            let id = self.create_object(unit, fpos, fangle, fplayer);
            // `newObj->setProducer(creationBuilding)`.
            if let Some(o) = self.objects.get_mut(&id) {
                o.producer_id = factory;
            }
            if created_first {
                self.unit_created(factory, id, fplayer);
                // `SpawnBehavior::createSpawn`'s `onEnslave` before the exit (SpawnBehavior.cpp:657-666); the parking
                // place exit is the spawn's door.
                self.slaved_on_enslave(id, factory);
            }
            self.pp_exit_object_via_door(factory, id, door);
            if !created_first {
                self.unit_created(factory, id, fplayer);
                // The `ProductionUpdate` create-module loop of `ProductionUpdate.cpp:793-805` (see
                // `exit_factory_unit_ordered`).
                self.create_modules_on_build_complete(id);
            }
            return id;
        }
        self.exit_factory_unit_ordered(factory, unit, ExitSlot::Create, created_first)
    }

    /// `Player::onUnitCreated` (`ProductionUpdate.cpp:800`, `SpawnBehavior.cpp:635`): the score, then
    /// `AIPlayer::onUnitProduced` (`AIPlayer.cpp:1054`) for an AI player.
    pub(crate) fn unit_created(&mut self, factory: ObjectId, id: ObjectId, player: PlayerIndex) {
        // `Player::onUnitCreated` (ProductionUpdate.cpp:791): `m_scoreKeeper.addObjectBuilt(unit)`.
        self.score_unit_created(id);
        // `AIPlayer::onUnitProduced` (`AIPlayer.cpp:1054`): only a unit that completes a resource-gatherer
        // work order is forced Wanting and assigned a supply center.
        if self.is_ai_player(player) {
            self.ai_on_unit_produced(factory, id);
        }
    }

    /// `ExitInterface::exitObjectViaDoor` for a factory without a parking place, through `slot` (the
    /// slot `reserve_exit_slot` took). Returns the new object.
    pub(crate) fn exit_factory_unit(&mut self, factory: ObjectId, unit: &Arc<ObjectTemplate>, slot: ExitSlot) -> ObjectId {
        self.exit_factory_unit_ordered(factory, unit, slot, false)
    }

    /// [`exit_factory_unit`](Self::exit_factory_unit) with the caller's order of the creation hooks:
    /// `created_first` runs `Player::onUnitCreated` before the exit (`SpawnBehavior::createSpawn`),
    /// otherwise after it (`ProductionUpdate::update`).
    fn exit_factory_unit_ordered(&mut self, factory: ObjectId, unit: &Arc<ObjectTemplate>, slot: ExitSlot, created_first: bool) -> ObjectId {
        let (fpos, fangle, fplayer, exit, rally) = match self.objects.get(&factory) {
            Some(o) => {
                let exit = o.template.production.as_ref().map(|p| p.exit.clone()).unwrap_or_default();
                let rally = o.production.as_ref().and_then(|p| p.rally_point);
                (o.position, o.angle, o.player, exit, rally)
            }
            None => return ObjectId::INVALID,
        };
        let id = match (&exit, slot) {
            (ProductionExit::SpawnPoint { bones }, ExitSlot::Spawn(i)) => {
                let Some(bone) = bones.get(i).copied() else { return ObjectId::INVALID };
                // `SpawnPointProductionExitUpdate::exitObjectViaDoor` (`:64`): the bone's position on the
                // ground at the building's angle plus the bone's own yaw; the unit is recorded as the
                // occupant and then held in place ("You are stuck here, little man.").
                let c = model_to_world(&fpos, fangle, bone.pos);
                let pos = Coord3D::new(c.x, c.y, self.terrain.ground_height(c.x, c.y));
                let id = self.create_object(unit, pos, fangle + bone.angle, fplayer);
                if let Some(o) = self.objects.get_mut(&id) {
                    o.producer_id = factory;
                }
                if created_first {
                    self.unit_created(factory, id, fplayer);
                    // `SpawnBehavior::createSpawn`'s `onEnslave` after `onUnitCreated` and `setProducer` (SpawnBehavior.cpp:
                    // 635-666), before the exit. Lane 165: the spawn-point arm drew nothing (AH7 s1 f1977, draw 1699).
                    self.slaved_on_enslave(id, factory);
                }
                if let Some(st) = self.objects.get_mut(&factory).and_then(|o| o.production.as_mut()) {
                    if st.spawn_occupiers.len() < bones.len() {
                        st.spawn_occupiers.resize(bones.len(), None);
                    }
                    st.spawn_occupiers[i] = Some(id);
                }
                self.set_disabled(id, crate::disabled::DisabledType::Held);
                id
            }
            (_, ExitSlot::Create) => {
                let Some((create_point, _)) = exit.points() else { return ObjectId::INVALID };
                // `DefaultProductionExitUpdate::exitObjectViaDoor` (DefaultProductionExitUpdate.cpp:64-69): the create point is
                // `Transform_Vector(factory matrix, unitCreatePoint)` (`Matrix3D::Transform_Vector`, matrix3d.h:1741-1756), not
                // the `model_to_world` order.
                // The create point is on the ground, except that `AllowAirborneCreation` keeps its height
                let c = transform_model_point(&fpos, fangle, create_point);
                // (`QueueProductionExitUpdate.cpp:88-96`; the other exits always use the ground).
                let z = match &exit {
                    ProductionExit::Queue { allow_airborne_creation: true, .. } => c.z,
                    _ => self.terrain.ground_height(c.x, c.y),
                };
                let id = self.create_object(unit, Coord3D::new(c.x, c.y, z), fangle, fplayer);
                // `Object::setProducer`: the factory is the produced unit's producer (`UpgradeDie`).
                if let Some(o) = self.objects.get_mut(&id) {
                    o.producer_id = factory;
                }
                if created_first {
                    self.unit_created(factory, id, fplayer);
                    // `createSpawn`'s `onEnslave` before the exit (SpawnBehavior.cpp:657-666, :727).
                    self.slaved_on_enslave(id, factory);
                }
                if self.objects.get(&id).is_some_and(|o| o.ai_handle.is_some()) {
                    let path = self.exit_path(id, &exit, &fpos, fangle, rally);
                    self.exit_follow_path(id, path, factory);
                }
                // `SupplyCenterProductionExitUpdate::exitObjectViaDoor` (`SupplyCenterProductionExitUpdate.cpp:127`):
                // "force wanting on SupplyTruck types", so the new truck starts its harvest loop.
                if matches!(exit, ProductionExit::SupplyCenter { .. }) {
                    self.set_supply_force_wanting(id, true);
                }
                // `SupplyCenterProductionExitUpdate.cpp:132-145`: a stealthed center grants its new units
                // temporary stealth, unless the unit has its own stealth that is not a temporary grant.
                if let ProductionExit::SupplyCenter { grant_temporary_stealth, .. } = exit {
                    let factory_stealthed = self.objects.get(&factory).is_some_and(|o| o.status_bits & crate::object::status::STEALTHED != 0);
                    let unit_takes_grant = self.objects.get(&id).is_some_and(|o| {
                        o.behavior.stealth_frames_granted > 0 || o.status_bits & crate::object::status::CAN_STEALTH == 0
                    });
                    if grant_temporary_stealth > 0 && factory_stealthed && unit_takes_grant {
                        self.stealth_receive_grant(id, true, grant_temporary_stealth);
                    }
                }
                // `exitObjectViaDoor` sets the exit's delay and spends one burst unit (`:155-158`).
                if let ProductionExit::Queue { exit_delay_frames, .. } = exit {
                    if let Some(st) = self.objects.get_mut(&factory).and_then(|o| o.production.as_mut()) {
                        st.exit_delay_left = exit_delay_frames;
                        st.exit_burst_left = st.exit_burst_left.saturating_sub(1);
                    }
                }
                id
            }
            _ => return ObjectId::INVALID,
        };
        if !created_first {
            self.unit_created(factory, id, fplayer);
            // `ProductionUpdate.cpp:793-805`: "onCreates have been called on newObj, and after that the
            // owner was set, so now is the time to call the game side of CreateModules" -- the
            // `onBuildComplete` loop, after `Player::onUnitCreated` (`:791`). `SpawnBehavior`'s
            // `createSpawn` does not run it (the `created_first` order).
            self.create_modules_on_build_complete(id);
        }
        id
    }

    /// The exit path of `exitObjectViaDoor` (`aiFollowExitProductionPath`): the natural rally point,
    /// then the rally point. For a ground unit the rally point is the adjusted destination
    /// (`adjustDestination`, `DefaultProductionExitUpdate.cpp:107`, `QueueProductionExitUpdate.cpp:144`);
    /// the supply center takes its rally point as it is (`SupplyCenterProductionExitUpdate.cpp:115`).
    /// A queue exit with no rally point repeats the natural point to keep units from stacking
    /// (`QueueProductionExitUpdate.cpp:149`).
    pub(crate) fn exit_path(&mut self, id: ObjectId, exit: &ProductionExit, origin: &Coord3D, angle: f32, rally: Option<Coord3D>) -> Vec<Coord3D> {
        let (natural_model, offset) = match exit {
            ProductionExit::Default { natural_rally_point, .. } | ProductionExit::Queue { natural_rally_point, .. } => {
                (*natural_rally_point, true)
            }
            ProductionExit::SupplyCenter { natural_rally_point, .. } => (*natural_rally_point, false),
            ProductionExit::SpawnPoint { .. } => return Vec::new(),
        };
        let mut natural = natural_rally_world(origin, angle, natural_model, offset);
        // `QueueProductionExitUpdate::exitObjectViaDoor` snaps the natural point to the new unit's grid cell before it
        // is pushed (`TheAI->pathfinder()->snapPosition(newObj, &tmp)`, QueueProductionExitUpdate.cpp:132-136). The
        // default and supply-center exits do not (DefaultProductionExitUpdate.cpp:97-99, SupplyCenter...:113).
        if matches!(exit, ProductionExit::Queue { .. }) {
            if let Some(po) = self.objects.get(&id).and_then(crate::pathfind::PathObject::for_object) {
                natural = self.pathfinder.snap_position(&self.terrain, &po, &natural);
            }
        }
        let mut path = vec![natural];
        match (exit, rally) {
            (ProductionExit::SupplyCenter { .. }, Some(r)) => path.push(r),
            (ProductionExit::SupplyCenter { .. }, None) => {}
            (ProductionExit::Queue { .. }, None) => path.push(natural),
            (_, Some(r)) => {
                if self.is_doing_ground_movement(id) {
                    if let Some(a) = self.adjust_exit_destination(id, &r) {
                        path.push(a);
                    }
                }
            }
            (_, None) => {}
        }
        path
    }

    /// `TheAI->pathfinder()->adjustDestination(newObj, locomotorSet, &dest)` for a unit's exit.
    fn adjust_exit_destination(&mut self, id: ObjectId, dest: &Coord3D) -> Option<Coord3D> {
        let o = self.objects.get(&id)?;
        let po = crate::pathfind::PathObject::for_object(o)?;
        self.pathfinder.adjust_destination(&self.terrain, &po, dest, None)
    }

    /// `aiFollowExitProductionPath(&exitPath, creationObject, CMD_FROM_AI)`: the unit walks the exit path in
    /// `AI_FOLLOW_EXITPRODUCTION_PATH` (pass-through, no final-destination adjust), ignoring its factory.
    fn exit_follow_path(&mut self, id: ObjectId, path: Vec<Coord3D>, factory: ObjectId) {
        if path.is_empty() {
            return;
        }
        self.ai_follow_exit_production_path(id, path, factory);
        self.wake_up_now(id);
    }

    /// `ai->ignoreObstacle(goal); ai->aiMoveToPosition(pos, CMD_FROM_AI)` (`DozerAIUpdate.cpp:229-230`,
    /// `DozerActionPickActionPosState` and `DozerActionDoActionState`). The move is a `CMD_FROM_AI`
    /// `privateMoveToPosition`, so a dozer that is still moving gets the 20-second temporary
    /// `AI_MOVE_TO` (see [`ai_private_move_to_position`](Self::ai_private_move_to_position)).
    /// `SupplyTruckAIUpdate` docks go through `aiDock` -> the same pathfound approach.
    pub(crate) fn ai_move_to_position_ignoring(&mut self, id: ObjectId, pos: Coord3D, goal: ObjectId) {
        // A new dozer order replaces whatever the main machine was doing (a dock sequence is
        // halted, which cancels its reservation).
        if self.objects.get(&id).is_some_and(|o| o.ai.machine.current == crate::ai_states::AiStateType::Dock) {
            self.ai_idle(id, crate::ai_states::CommandSource::Ai);
        }
        if self.objects.get(&id).is_none_or(|o| o.ai_handle.is_none()) {
            return;
        }
        self.ai_set_ignore_obstacle(id, goal);
        self.ai_private_move_to_position(id, pos, crate::ai_states::CommandSource::Ai);
        self.wake_up_now(id);
    }

    /// `ProductionPrerequisite::isSatisfied`: all `Object` OR groups must be owned; sciences are
    /// skipped. An object still under construction does not count (`calcNumPrereqUnitsOwned`,
    /// `ProductionPrerequisite.cpp:111`, uses the default `ignoreUnderConstruction = TRUE`).
    pub(crate) fn prerequisites_satisfied(&self, player: PlayerIndex, unit: &ObjectTemplate) -> bool {
        for p in &unit.prerequisites {
            match p {
                Prerequisite::AnyOfObjects(names) => {
                    let owned = names.iter().any(|n| {
                        self.objects
                            .values()
                            .any(|o| !o.destroyed && !o.under_construction && o.player == player && o.template.name == *n)
                    });
                    if !owned {
                        return false;
                    }
                }
                Prerequisite::Science(_) => {}
            }
        }
        true
    }
}

/// Transform a model-space point by the building's position and heading (a Z rotation).
pub(crate) fn model_to_world(origin: &Coord3D, angle: f32, p: (f32, f32, f32)) -> Coord3D {
    let (s, c) = (sin(angle), cos(angle));
    Coord3D::new(origin.x + p.0 * c - p.1 * s, origin.y + p.0 * s + p.1 * c, origin.z + p.2)
}

/// `QueueProductionExitUpdate::isFreeToExit` (`QueueProductionExitUpdate.cpp:201`): a burst unit always
/// leaves, otherwise the exit delay must have run out.
fn queue_exit_free(st: &ProductionState) -> bool {
    st.exit_burst_left > 0 || st.exit_delay_left == 0
}

/// `getNaturalRallyPoint(offset)` (`DefaultProductionExitUpdate.cpp:144`): the natural rally point in
/// model space, optionally pushed out by [`NATURAL_RALLY_OFFSET`] along its own direction, then placed
/// in the world. A zero point has no direction and stays put.
fn natural_rally_world(origin: &Coord3D, angle: f32, p: (f32, f32, f32), offset: bool) -> Coord3D {
    let mut q = p;
    if offset {
        // `Vector3 offset = p; offset.Normalize(); offset *= (2*PATHFIND_CELL_SIZE_F); p += offset;` (DefaultProductionExitUpdate.cpp:
        // 150-154): `Vector3::Normalize` multiplies by `WWMath::Inv_Sqrt` of the squared length (vector3.h:417-426), so the
        // result is not `p / len` (lane 194 round 3; the zero point stays zero).
        let n = crate::vector_math::vec3_normalize(p);
        let k = NATURAL_RALLY_OFFSET;
        let off = (n.0 * k, n.1 * k, n.2 * k);
        q = (p.0 + off.0, p.1 + off.1, p.2 + off.2);
    }
    transform_model_point(origin, angle, q)
}

/// `Matrix3D::Transform_Vector` of a model-space point through the object's matrix at `origin` and `angle`
/// (`Thing::setOrientation`, see [`crate::vector_math::z_rotation_transform`]). The engine's `Cos`/`Sin` are the
/// `crate::math` functions.
pub(crate) fn transform_model_point(origin: &Coord3D, angle: f32, p: (f32, f32, f32)) -> Coord3D {
    let w = crate::vector_math::z_rotation_transform((origin.x, origin.y, origin.z), cos(angle), sin(angle), p);
    Coord3D::new(w.0, w.1, w.2)
}

/// Lane 165: the enslave draw of a spawn through a spawn point or a supply center's exit (`spawn_l165.rs`).
#[cfg(test)]
#[path = "spawn_l165.rs"]
mod spawn_l165;

/// Lane 173 (wave 20): the slave's master set by `onEnslave`, R19-7 (`spawn_l173.rs`).
#[cfg(test)]
#[path = "spawn_l173.rs"]
mod spawn_l173;
