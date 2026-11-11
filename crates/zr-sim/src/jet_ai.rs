//! `ParkingPlaceBehavior` and the `JetAIUpdate` runtime data: an airfield's parking spaces, runways
//! and healing list, and the per-aircraft jet state.
//!
//! Ported from `GameLogic/Object/Behavior/ParkingPlaceBehavior.cpp` (`buildInfo`, `reserveSpace`,
//! `calcPPInfo`, `releaseSpace`, `reserveRunway` / `releaseRunway` /
//! `transferRunwayReservationToNextInLineForTakeoff`, `setHealee` + `update`, `killAllParkedUnits`,
//! `reserveDoorForExit` / `exitObjectViaDoor` / `unreserveDoorForExit`) and the runtime fields of
//! `JetAIUpdate` (`Object/Update/AIUpdate/JetAIUpdate.cpp`). The jet state machine itself is in
//! [`crate::jet_states`]. Where TheSuperHackers' fork is `!RETAIL_COMPATIBLE_CRC` the fork's
//! behaviour is ported (runway reservation postponing, no-airfield failures).
//!
//! Not ported: `FlightDeckBehavior` (the Aircraft Carrier), `LandingDeckHeightOffset` /
//! `OBJECT_STATUS_DECK_HEIGHT_OFFSET`, `defectAllParkedUnits` (capture) and the client-side
//! `setInstanceMatrix` taxi lift (`MinHeight`).

use crate::crc::Crc;
use crate::logic::GameLogic;
use crate::math::{cos, normalize_angle, sin, sqrt, Coord3D, LOGICFRAMES_PER_SECOND};
use crate::module::UPDATE_SLEEP_NONE;
use crate::object::ObjectId;
use crate::production::{open_door_for_exit, update_door, DoorTiming};
use crate::template::{collide_kind, ObjectTemplate};

// ---- bones -------------------------------------------------------------------------------------

/// A bone's rest position and Z rotation in object-local space (`Object::getSingleLogicalBonePosition`
/// then `Matrix3D::Get_Z_Rotation`). A bone the model lacks reads back as the object's own position
/// and orientation (the C++ fallback), which is the zero local bone.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bone {
    pub pos: Coord3D,
    /// Z rotation relative to the object's heading.
    pub orient: f32,
}

/// One parking space's bones (`Runway{col}Park{row}Han`, `Runway{col}Parking{row}`, `Runway{col}Prep{row}`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParkingSpaceBones {
    /// `m_hangarStart` / `m_hangarStartOrient`.
    pub hangar: Bone,
    /// `m_location` / `m_orientation`.
    pub location: Bone,
    /// `m_prep`.
    pub prep: Coord3D,
}

/// One runway's bones (`RunwayStart{col}`, `RunwayEnd{col}`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RunwayBones {
    pub start: Coord3D,
    pub end: Coord3D,
}

/// The airfield bones `ParkingPlaceBehavior::buildInfo` reads, object-local. Spaces are row-major
/// (`row * NumCols + col`), as built.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParkingBones {
    pub spaces: Vec<ParkingSpaceBones>,
    pub runways: Vec<RunwayBones>,
    /// `HeliPark01` (the helipad spot).
    pub heli_park: Bone,
}

// ---- runtime state -----------------------------------------------------------------------------

/// `ParkingPlaceBehaviorInterface::PPInfo` (`BehaviorModule.h`), world space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PpInfo {
    pub parking_space: Coord3D,
    pub parking_orientation: f32,
    pub runway_prep: Coord3D,
    pub runway_start: Coord3D,
    pub runway_end: Coord3D,
    pub runway_approach: Coord3D,
    pub runway_exit: Coord3D,
    pub hangar_internal: Coord3D,
    pub hangar_internal_orient: f32,
    /// `runwayTakeoffDist`: the runway's length (start to end), before the line-up adjustment.
    pub runway_takeoff_dist: f32,
}

/// `ParkingPlaceInfo`'s runtime half.
#[derive(Clone, Debug, PartialEq)]
pub struct ParkingSpaceRt {
    /// `m_objectInSpace`.
    pub object_in_space: ObjectId,
    /// `m_reservedForExit`.
    pub reserved_for_exit: bool,
    /// `m_postponedRunwayReservationForTakeoff`.
    pub postponed: bool,
    /// `m_runway` (the column).
    pub runway: usize,
}

/// `RunwayInfo`'s runtime half.
#[derive(Clone, Debug, PartialEq)]
pub struct RunwayRt {
    pub in_use_by: ObjectId,
    pub next_in_line_for_takeoff: ObjectId,
    pub was_in_line: bool,
}

/// `ProductionUpdate::DoorInfo`: the airfield's hangar door of one parking space.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DoorRt {
    /// `m_doorOpenedFrame` (opening), `m_doorWaitOpenFrame` (open), `m_doorClosedFrame` (closing); `0` = not in that state.
    pub opened_frame: u32,
    pub wait_open_frame: u32,
    pub closed_frame: u32,
    /// `m_holdOpen` (`setHoldDoorOpen`).
    pub hold_open: bool,
}

/// `ParkingPlaceBehavior`'s members (`m_spaces`, `m_runways`, `m_healing`, `m_nextHealFrame`, the helicopter rally).
#[derive(Clone, Debug, PartialEq)]
pub struct ParkingRt {
    pub spaces: Vec<ParkingSpaceRt>,
    /// The door of each space (`ParkingPlaceInfo::m_door` is the space index).
    pub doors: Vec<DoorRt>,
    pub runways: Vec<RunwayRt>,
    /// `m_healing`: (object, start frame).
    pub healing: Vec<(ObjectId, u32)>,
    /// `m_nextHealFrame`.
    pub next_heal_frame: u32,
    /// `m_heliRallyPoint` / `m_heliRallyPointExists`.
    pub heli_rally: Option<Coord3D>,
}

/// `FOREVER` (`UpdateModule.h`): "never wake".
const FOREVER: u32 = 0x3fff_ffff;
/// `HEAL_RATE_FRAMES` (`ParkingPlaceBehavior.cpp:592`).
const HEAL_RATE_FRAMES: u32 = LOGICFRAMES_PER_SECOND / 5;
/// `APPROACH_DIST` (`calcPPInfo`).
const APPROACH_DIST: f32 = 0.75;

impl ParkingRt {
    /// `ParkingPlaceBehavior::ParkingPlaceBehavior` + `buildInfo`: one space per row/column, one
    /// runway per column when `HasRunways`.
    pub fn new(t: &ObjectTemplate) -> Self {
        let pp = t.parking_place.unwrap_or(crate::modules2::ParkingPlaceTemplate {
            num_rows: 0,
            num_cols: 0,
            has_runways: false,
            heal_amount_per_second: 0.0,
            approach_height: 0.0,
            landing_deck_height_offset: 0.0,
            park_in_hangars: false,
        });
        let cols = pp.num_cols.max(0) as usize;
        let rows = pp.num_rows.max(0) as usize;
        let spaces = (0..rows * cols)
            .map(|i| ParkingSpaceRt { object_in_space: ObjectId::INVALID, reserved_for_exit: false, postponed: false, runway: i % cols.max(1) })
            .collect();
        let runways = if pp.has_runways {
            (0..cols).map(|_| RunwayRt { in_use_by: ObjectId::INVALID, next_in_line_for_takeoff: ObjectId::INVALID, was_in_line: false }).collect()
        } else {
            Vec::new()
        };
        let doors = vec![DoorRt::default(); rows * cols];
        ParkingRt { spaces, doors, runways, healing: Vec::new(), next_heal_frame: FOREVER, heli_rally: None }
    }

    /// `ProductionUpdate::setHoldDoorOpen` (`ProductionUpdate.cpp:1201`): holding a closed door starts it opening.
    fn set_hold_door_open(&mut self, idx: usize, hold: bool, now: u32) {
        if let Some(d) = self.doors.get_mut(idx) {
            d.hold_open = hold;
            if hold && d.opened_frame == 0 && d.wait_open_frame == 0 && d.closed_frame == 0 {
                d.opened_frame = now;
            }
        }
    }

    fn find_ppi(&self, id: ObjectId) -> Option<usize> {
        if id == ObjectId::INVALID {
            return None;
        }
        self.spaces.iter().position(|s| s.object_in_space == id)
    }

    fn find_empty_ppi(&self) -> Option<usize> {
        self.spaces.iter().position(|s| s.object_in_space == ObjectId::INVALID && !s.reserved_for_exit)
    }

    /// `ParkingPlaceBehavior::purgeDead` (`ParkingPlaceBehavior.cpp:140`): forget dead occupants,
    /// runway users and healees. `dead` is `obj == nullptr || obj->isEffectivelyDead()`.
    fn purge_dead(&mut self, now: u32, dead: &dyn Fn(ObjectId) -> bool) {
        for i in 0..self.spaces.len() {
            let s = &mut self.spaces[i];
            if s.object_in_space != ObjectId::INVALID && dead(s.object_in_space) {
                s.object_in_space = ObjectId::INVALID;
                s.reserved_for_exit = false;
                s.postponed = false;
                self.set_hold_door_open(i, false, now);
            }
        }
        for r in &mut self.runways {
            if r.in_use_by != ObjectId::INVALID && dead(r.in_use_by) {
                r.in_use_by = ObjectId::INVALID;
                r.was_in_line = false;
            }
            if r.next_in_line_for_takeoff != ObjectId::INVALID && dead(r.next_in_line_for_takeoff) {
                r.next_in_line_for_takeoff = ObjectId::INVALID;
            }
        }
        let before = self.healing.len();
        self.healing.retain(|(id, _)| *id == ObjectId::INVALID || !dead(*id));
        if self.healing.len() != before {
            self.reset_wake_frame(now);
        }
    }

    /// `ParkingPlaceBehavior::resetWakeFrame`.
    fn reset_wake_frame(&mut self, now: u32) {
        self.next_heal_frame = if self.healing.is_empty() { FOREVER } else { now + HEAL_RATE_FRAMES };
    }

    pub(crate) fn crc(&self, c: &mut Crc) {
        c.add_u32(self.spaces.len() as u32);
        for s in &self.spaces {
            c.add_u32(s.object_in_space.0);
            c.add_bool(s.reserved_for_exit);
            c.add_bool(s.postponed);
        }
        for d in &self.doors {
            c.add_u32(d.opened_frame);
            c.add_u32(d.wait_open_frame);
            c.add_u32(d.closed_frame);
            c.add_bool(d.hold_open);
        }
        c.add_u32(self.runways.len() as u32);
        for r in &self.runways {
            c.add_u32(r.in_use_by.0);
            c.add_u32(r.next_in_line_for_takeoff.0);
            c.add_bool(r.was_in_line);
        }
        c.add_u32(self.healing.len() as u32);
        for (id, f) in &self.healing {
            c.add_u32(id.0);
            c.add_u32(*f);
        }
        c.add_u32(self.next_heal_frame);
        match &self.heli_rally {
            Some(p) => {
                c.add_bool(true);
                c.add_coord(p);
            }
            None => c.add_bool(false),
        }
    }
}

/// `JetAIUpdate::FlagType` bits.
pub mod jet_flag {
    pub const HAS_PENDING_COMMAND: u16 = 1 << 0;
    pub const ALLOW_AIR_LOCO: u16 = 1 << 1;
    pub const HAS_PRODUCER_LOCATION: u16 = 1 << 2;
    pub const TAKEOFF_IN_PROGRESS: u16 = 1 << 3;
    pub const LANDING_IN_PROGRESS: u16 = 1 << 4;
    pub const USE_SPECIAL_RETURN_LOCO: u16 = 1 << 5;
    pub const ALLOW_CIRCLING: u16 = 1 << 6;
    pub const ALLOW_INTERRUPT_AND_RESUME_OF_CUR_STATE_FOR_RELOAD: u16 = 1 << 7;
    pub const TAXI_IN_PROGRESS: u16 = 1 << 8;
}

/// `AICommandParmsStorage` for a jet: the state a command would set plus everything the command
/// handlers wrote before calling `setState` (the sim has no parms struct, so the written fields are
/// snapshotted instead). Replayed by [`GameLogic::jet_replay_pending`].
#[derive(Clone, Debug)]
pub struct JetCommand {
    pub state: crate::ai_states::AiStateType,
    pub machine: crate::ai_states::AiMachine,
    pub attack: Option<crate::combat::AttackOrder>,
    pub force_attack: bool,
    pub attack_move: bool,
    pub guard: Option<crate::orders::GuardTarget>,
    pub guard_mode: crate::orders::GuardMode,
    pub hunt: bool,
    pub hunt_area: Option<(Coord3D, f32)>,
}

/// `JetAIUpdate`'s members (`JetAIUpdate.h:150`) plus the scratch fields of the jet states.
#[derive(Clone, Debug, Default)]
pub struct JetRt {
    /// `m_flags` ([`jet_flag`] bits).
    pub flags: u16,
    /// `m_producerLocation`.
    pub producer_location: Coord3D,
    /// `m_mostRecentCommand`.
    pub last_command: Option<Box<JetCommand>>,
    /// `m_attackLocoExpireFrame` / `m_attackersMissExpireFrame` / `m_returnToBaseFrame`.
    pub attack_loco_expire_frame: u32,
    pub attackers_miss_expire_frame: u32,
    pub return_to_base_frame: u32,
    /// `m_landingPosForHelipadStuff`.
    pub landing_pos_helipad: Coord3D,
    /// `JetPauseBeforeTakeoffState::m_whenTakeoff` / `m_whenTransfer` / `m_waitedForTaxiID`.
    pub when_takeoff: u32,
    pub when_transfer: u32,
    pub waited_for_taxi_id: ObjectId,
    /// `JetOrHeliReloadAmmoState::m_reloadTime` / `m_reloadDoneFrame`.
    pub reload_time: u32,
    pub reload_done_frame: u32,
    /// `JetOrHeliCirclingDeadAirfieldState::m_checkAirfield`.
    pub check_airfield: i32,
    /// `JetTakeoffOrLandingState::m_maxLift` / `m_maxSpeed`.
    pub saved_max_lift: f32,
    pub saved_max_speed: f32,
    /// `HeliTakeoffOrLandingState::m_path` / `m_index` / `m_parkingLoc` / `m_parkingOrientation`.
    pub heli_path: [Coord3D; 2],
    pub heli_index: u8,
    pub heli_parking_loc: Coord3D,
    pub heli_parking_orientation: f32,
}

impl JetRt {
    pub fn flag(&self, f: u16) -> bool {
        self.flags & f != 0
    }
    pub fn set_flag(&mut self, f: u16, v: bool) {
        if v {
            self.flags |= f;
        } else {
            self.flags &= !f;
        }
    }

    pub(crate) fn crc(&self, c: &mut Crc) {
        c.add_u32(u32::from(self.flags));
        c.add_coord(&self.producer_location);
        c.add_bool(self.last_command.is_some());
        c.add_u32(self.attack_loco_expire_frame);
        c.add_u32(self.attackers_miss_expire_frame);
        c.add_u32(self.return_to_base_frame);
        c.add_coord(&self.landing_pos_helipad);
        c.add_u32(self.when_takeoff);
        c.add_u32(self.when_transfer);
        c.add_u32(self.waited_for_taxi_id.0);
        c.add_u32(self.reload_time);
        c.add_u32(self.reload_done_frame);
        c.add_u32(self.check_airfield as u32);
        c.add_f32(self.saved_max_lift);
        c.add_f32(self.saved_max_speed);
        c.add_coord(&self.heli_path[0]);
        c.add_coord(&self.heli_path[1]);
        c.add_u32(u32::from(self.heli_index));
        c.add_coord(&self.heli_parking_loc);
        c.add_f32(self.heli_parking_orientation);
    }
}

/// Object-local point -> world (`Object::convertBonePosToWorldPos` with no extra transform).
fn to_world(origin: &Coord3D, angle: f32, p: &Coord3D) -> Coord3D {
    let (s, c) = (sin(angle), cos(angle));
    Coord3D::new(origin.x + p.x * c - p.y * s, origin.y + p.x * s + p.y * c, origin.z + p.z)
}

// ---- ParkingPlaceBehavior ----------------------------------------------------------------------

impl GameLogic {
    /// `getPP(id)` (`JetAIUpdate.cpp:122`): the airfield `id` when it is alive, a
    /// `KINDOF_FS_AIRFIELD` and has a parking place.
    pub(crate) fn jet_getpp(&self, id: ObjectId) -> Option<ObjectId> {
        if id == ObjectId::INVALID {
            return None;
        }
        let a = self.find_object(id)?;
        // `OBJECT_STATUS_SOLD` is not modelled (selling removes the building at once).
        if a.is_dying() || !a.template.is_collide_kind_of(collide_kind::FS_AIRFIELD) || a.parking.is_none() {
            return None;
        }
        Some(id)
    }

    /// `ParkingPlaceBehavior::purgeDead` on object `af`.
    fn pp_purge(&mut self, af: ObjectId) {
        let Some(mut rt) = self.objects.get_mut(&af).and_then(|o| o.parking.take()) else { return };
        {
            let dead = |id: ObjectId| self.objects.get(&id).is_none_or(|o| o.destroyed || o.is_dying());
            rt.purge_dead(self.frame, &dead);
        }
        if let Some(o) = self.objects.get_mut(&af) {
            o.parking = Some(rt);
        }
    }

    /// `ParkingPlaceBehavior::hasReservedSpace`.
    pub(crate) fn pp_has_reserved_space(&self, af: ObjectId, id: ObjectId) -> bool {
        if id == ObjectId::INVALID {
            return false;
        }
        self.objects.get(&af).and_then(|o| o.parking.as_ref()).is_some_and(|rt| rt.find_ppi(id).is_some())
    }

    /// `ParkingPlaceBehavior::hasAvailableSpaceFor` (`canQueueCreateUnit`'s test).
    pub(crate) fn pp_has_available_space_for(&self, af: ObjectId, unit: &ObjectTemplate) -> bool {
        if unit.is_collide_kind_of(collide_kind::PRODUCED_AT_HELIPAD) {
            return true;
        }
        let Some(rt) = self.objects.get(&af).and_then(|o| o.parking.as_ref()) else { return false };
        rt.spaces.iter().any(|s| {
            let mut id = s.object_in_space;
            // "just peek and see if we have a dead thing."
            if id != ObjectId::INVALID && self.objects.get(&id).is_none_or(|o| o.destroyed || o.is_dying()) {
                id = ObjectId::INVALID;
            }
            id == ObjectId::INVALID && !s.reserved_for_exit
        })
    }

    /// `ParkingPlaceBehavior::calcPPInfo` (`ParkingPlaceBehavior.cpp:370`).
    pub(crate) fn pp_calc_info(&self, af: ObjectId, id: ObjectId) -> Option<PpInfo> {
        let a = self.objects.get(&af)?;
        let rt = a.parking.as_ref()?;
        let idx = rt.find_ppi(id)?;
        let pp = a.template.parking_place?;
        let bones = &a.template.parking_bones;
        let sb = bones.spaces.get(idx).copied().unwrap_or_default();
        let runway = rt.spaces[idx].runway;
        let rb = bones.runways.get(runway).copied().unwrap_or_default();
        let (o, ang) = (a.position, a.angle);
        let w = |p: &Coord3D| to_world(&o, ang, p);
        let hangar = w(&sb.hangar.pos);
        let hangar_orient = normalize_angle(ang + sb.hangar.orient);
        let location = w(&sb.location.pos);
        let orientation = normalize_angle(ang + sb.location.orient);
        let start = w(&rb.start);
        let end = w(&rb.end);
        let mut info = PpInfo {
            parking_space: if pp.park_in_hangars { hangar } else { location },
            runway_prep: w(&sb.prep),
            parking_orientation: if pp.park_in_hangars { hangar_orient } else { orientation },
            runway_start: start,
            runway_end: end,
            runway_approach: end,
            runway_exit: end,
            hangar_internal: hangar,
            hangar_internal_orient: hangar_orient,
            runway_takeoff_dist: 0.0,
        };
        info.runway_approach.x += (end.x - start.x) * APPROACH_DIST;
        info.runway_approach.y += (end.y - start.y) * APPROACH_DIST;
        info.runway_approach.z = end.z + pp.approach_height + pp.landing_deck_height_offset;
        info.runway_exit = info.runway_approach;
        // "Cache the runway's takeoff distance used by JetAIUpdate for calculating lift."
        let (dx, dy, dz) = (start.x - end.x, start.y - end.y, start.z - end.z);
        info.runway_takeoff_dist = sqrt(dx * dx + dy * dy + dz * dz);
        if let Some(r) = rt.runways.iter().find(|r| r.in_use_by == id) {
            if r.was_in_line {
                info.runway_start = info.runway_prep;
            }
        }
        Some(info)
    }

    /// `ParkingPlaceBehavior::reserveSpace` (`ParkingPlaceBehavior.cpp:322`). Returns the info the
    /// caller may ask for (`PPInfo`), with `parkingOffset` applied to the parking space.
    pub(crate) fn pp_reserve_space(&mut self, af: ObjectId, id: ObjectId, parking_offset: f32) -> Option<PpInfo> {
        if self.objects.get(&af).is_none_or(|o| o.under_construction) {
            // `buildInfo` is deferred while under construction, so there are no spaces yet.
            return None;
        }
        self.pp_purge(af);
        let rt = self.objects.get_mut(&af)?.parking.as_mut()?;
        let idx = match rt.find_ppi(id) {
            Some(i) => i,
            None => rt.find_empty_ppi()?, // "No parking places!"
        };
        rt.spaces[idx].object_in_space = id;
        rt.spaces[idx].reserved_for_exit = false;
        // `pu->setHoldDoorOpen(ppi->m_door, true)`.
        let now = self.frame;
        if let Some(rt) = self.objects.get_mut(&af).and_then(|o| o.parking.as_mut()) {
            rt.set_hold_door_open(idx, true, now);
        }
        let mut info = self.pp_calc_info(af, id)?;
        if parking_offset != 0.0 {
            // `info->parkingSpace.x += parkingOffset * Cos(ppi->m_orientation)`: the *location*
            // bone's orientation, not the hangar's.
            let a = self.objects.get(&af)?;
            let sb = a.template.parking_bones.spaces.get(idx).copied().unwrap_or_default();
            let orient = normalize_angle(a.angle + sb.location.orient);
            info.parking_space.x += parking_offset * cos(orient);
            info.parking_space.y += parking_offset * sin(orient);
        }
        Some(info)
    }

    /// `ParkingPlaceBehavior::releaseSpace`.
    pub(crate) fn pp_release_space(&mut self, af: ObjectId, id: ObjectId) {
        self.pp_purge(af);
        let now = self.frame;
        if let Some(rt) = self.objects.get_mut(&af).and_then(|o| o.parking.as_mut()) {
            if let Some(i) = rt.spaces.iter().position(|s| s.object_in_space == id) {
                let s = &mut rt.spaces[i];
                s.object_in_space = ObjectId::INVALID;
                s.reserved_for_exit = false;
                s.postponed = false;
                rt.set_hold_door_open(i, false, now);
            }
        }
    }

    /// `ProductionUpdate::updateDoors` (`ProductionUpdate.cpp:491`) for an airfield: step each hangar
    /// door through opening -> open -> closing -> closed.
    pub(crate) fn pp_update_doors(&mut self, af: ObjectId) {
        let now = self.frame;
        let Some(o) = self.objects.get_mut(&af) else { return };
        let (t, n) = match o.template.production.as_ref() {
            Some(p) => (DoorTiming::of(p), p.num_door_animations),
            None => return,
        };
        if n <= 0 {
            return;
        }
        let Some(rt) = o.parking.as_mut() else { return };
        for d in &mut rt.doors {
            update_door(d, t, now);
        }
    }

    /// The door block of `ProductionUpdate::update` (`ProductionUpdate.cpp:711-764`): open `door` (or
    /// keep it open) for a unit that finished production; the unit is only created once the door is
    /// fully open (`m_doorWaitOpenFrame != 0`).
    pub(crate) fn pp_open_door_for_exit(&mut self, af: ObjectId, door: usize) -> bool {
        let now = self.frame;
        let Some(o) = self.objects.get_mut(&af) else { return false };
        let n = o.template.production.as_ref().map_or(0, |p| p.num_door_animations);
        if n <= 0 {
            return true;
        }
        let Some(d) = o.parking.as_mut().and_then(|rt| rt.doors.get_mut(door)) else { return true };
        open_door_for_exit(d, now)
    }

    /// `ParkingPlaceBehavior::getRunwayIndex`: `None` is `InvalidRunway`.
    pub(crate) fn pp_runway_index(&mut self, af: ObjectId, id: ObjectId) -> Option<usize> {
        self.pp_purge(af);
        let rt = self.objects.get(&af)?.parking.as_ref()?;
        rt.spaces.iter().find(|s| s.object_in_space == id).map(|s| s.runway)
    }

    /// `getRunwayCount`.
    pub(crate) fn pp_runway_count(&self, af: ObjectId) -> usize {
        self.objects.get(&af).and_then(|o| o.parking.as_ref()).map_or(0, |rt| rt.runways.len())
    }

    /// `getRunwayReservation(runway, ...)`: the runway's current user (the type is ignored, the
    /// runways are shared for takeoff and landing).
    pub(crate) fn pp_runway_reservation(&mut self, af: ObjectId, runway: usize) -> ObjectId {
        self.pp_purge(af);
        self.objects
            .get(&af)
            .and_then(|o| o.parking.as_ref())
            .and_then(|rt| rt.runways.get(runway))
            .map_or(ObjectId::INVALID, |r| r.in_use_by)
    }

    /// `transferRunwayReservationToNextInLineForTakeoff` (`ParkingPlaceBehavior.cpp:471`).
    pub(crate) fn pp_transfer_runway_to_next(&mut self, af: ObjectId, id: ObjectId) {
        self.pp_purge(af);
        let Some(rt) = self.objects.get_mut(&af).and_then(|o| o.parking.as_mut()) else { return };
        if let Some(r) = rt.runways.iter_mut().find(|r| r.in_use_by == id) {
            if r.next_in_line_for_takeoff != ObjectId::INVALID {
                r.in_use_by = r.next_in_line_for_takeoff;
                r.was_in_line = true;
                r.next_in_line_for_takeoff = ObjectId::INVALID;
            }
        }
    }

    /// `postponeRunwayReservation` (TheSuperHackers, `ParkingPlaceBehavior.cpp:491`): block the first
    /// takeoff reservation attempt of the 'upper' spaces so the 'lower' ones take the runway first.
    fn pp_postpone_runway(rt: &mut ParkingRt, space_index: usize, for_landing: bool) -> bool {
        if rt.spaces.len() > rt.runways.len() && space_index >= rt.runways.len() {
            let postponed = &mut rt.spaces[space_index].postponed;
            if for_landing {
                *postponed = false;
            } else if !*postponed {
                *postponed = true;
                return true;
            }
        }
        false
    }

    /// `ParkingPlaceBehavior::reserveRunway` (`ParkingPlaceBehavior.cpp:514`).
    pub(crate) fn pp_reserve_runway(&mut self, af: ObjectId, id: ObjectId, for_landing: bool) -> bool {
        self.pp_purge(af);
        let Some(rt) = self.objects.get_mut(&af).and_then(|o| o.parking.as_mut()) else { return false };
        let Some(i) = rt.spaces.iter().position(|s| s.object_in_space == id) else {
            return false; // "only planes with reserved spaces can reserve runways"
        };
        if Self::pp_postpone_runway(rt, i, for_landing) {
            return false;
        }
        let runway = rt.spaces[i].runway;
        let Some(info) = rt.runways.get_mut(runway) else { return false };
        if info.in_use_by == id {
            true
        } else if info.in_use_by == ObjectId::INVALID {
            info.in_use_by = id;
            if info.next_in_line_for_takeoff == id {
                info.next_in_line_for_takeoff = ObjectId::INVALID;
                info.was_in_line = true;
            } else {
                info.was_in_line = false;
            }
            true
        } else if !for_landing && info.next_in_line_for_takeoff == ObjectId::INVALID {
            info.next_in_line_for_takeoff = id;
            false // "yes, that's right"
        } else {
            false
        }
    }

    /// `ParkingPlaceBehavior::releaseRunway`.
    pub(crate) fn pp_release_runway(&mut self, af: ObjectId, id: ObjectId) {
        self.pp_purge(af);
        if let Some(rt) = self.objects.get_mut(&af).and_then(|o| o.parking.as_mut()) {
            for r in &mut rt.runways {
                if r.in_use_by == id {
                    r.in_use_by = ObjectId::INVALID;
                    r.was_in_line = false;
                }
                if r.next_in_line_for_takeoff == id {
                    r.next_in_line_for_takeoff = ObjectId::INVALID;
                }
            }
        }
    }

    /// `ParkingPlaceBehavior::setHealee` (`ParkingPlaceBehavior.cpp:608`).
    pub(crate) fn pp_set_healee(&mut self, af: ObjectId, healee: ObjectId, add: bool) {
        let now = self.frame;
        let Some(rt) = self.objects.get_mut(&af).and_then(|o| o.parking.as_mut()) else { return };
        if add {
            if rt.healing.iter().any(|(id, _)| *id == healee) {
                return;
            }
            rt.healing.push((healee, now));
            rt.reset_wake_frame(now);
        } else {
            let before = rt.healing.len();
            rt.healing.retain(|(id, _)| *id != healee);
            if rt.healing.len() != before {
                rt.reset_wake_frame(now);
            }
        }
    }

    /// `ParkingPlaceBehavior::killAllParkedUnits` (`:680`): kill every occupant that is on the ground
    /// (or mid takeoff / landing); airborne ones keep flying and circle the dead airfield.
    pub(crate) fn pp_kill_all_parked_units(&mut self, af: ObjectId) {
        self.pp_purge(af);
        let occupants: Vec<ObjectId> = self
            .objects
            .get(&af)
            .and_then(|o| o.parking.as_ref())
            .map(|rt| rt.spaces.iter().map(|s| s.object_in_space).filter(|id| *id != ObjectId::INVALID).collect())
            .unwrap_or_default();
        for id in occupants {
            let Some(o) = self.objects.get(&id) else { continue };
            if o.destroyed || o.is_dying() {
                continue;
            }
            let ground = self.terrain.surface_height(o.position.x, o.position.y, o.position.z);
            let above_terrain = o.position.z > ground + 0.001;
            let takeoff_or_landing = o.jet.flag(jet_flag::TAKEOFF_IN_PROGRESS) || o.jet.flag(jet_flag::LANDING_IN_PROGRESS);
            if above_terrain && !takeoff_or_landing {
                continue;
            }
            // `Object::kill()`.
            self.damage_object_with_death(
                id,
                crate::body::DamageInfo {
                    source: ObjectId::INVALID,
                    amount: crate::damage::HUGE_DAMAGE_AMOUNT,
                    kind: crate::damage::DamageType::Unresistable,
                },
                crate::damage::DeathType::Normal,
            );
        }
        self.pp_purge(af);
    }

    /// `ParkingPlaceBehavior::update` for one airfield (`ParkingPlaceBehavior.cpp:715`): keep the dead
    /// purged and heal the aircraft that are parked (`HealAmountPerSecond`, every `HEAL_RATE_FRAMES`).
    /// The C++ module is an `UpdateModule` whose constructor sets `UPDATE_SLEEP_NONE`
    /// (`ParkingPlaceBehavior.cpp:57`) and whose `update` returns `UPDATE_SLEEP_NONE` (`:755`).
    pub(crate) fn parking_place_update(&mut self, af: ObjectId) -> u32 {
        let now = self.frame;
        self.pp_purge(af);
        let (due, amount, healees) = {
            let Some(o) = self.objects.get(&af) else { return UPDATE_SLEEP_NONE };
            let Some(rt) = o.parking.as_ref() else { return UPDATE_SLEEP_NONE };
            let amount = o.template.parking_place.map_or(0.0, |p| p.heal_amount_per_second);
            (now >= rt.next_heal_frame, amount, rt.healing.clone())
        };
        if due {
            if let Some(rt) = self.objects.get_mut(&af).and_then(|o| o.parking.as_mut()) {
                rt.next_heal_frame = now + HEAL_RATE_FRAMES;
            }
            for (id, _) in healees {
                if self.objects.get(&id).is_none_or(|o| o.destroyed || o.is_dying()) {
                    if let Some(rt) = self.objects.get_mut(&af).and_then(|o| o.parking.as_mut()) {
                        rt.healing.retain(|(h, _)| *h != id);
                    }
                    continue;
                }
                // `m_amount = HEAL_RATE_FRAMES * m_healAmount * SECONDS_PER_LOGICFRAME_REAL`.
                let healed = HEAL_RATE_FRAMES as f32 * amount * (1.0 / LOGICFRAMES_PER_SECOND as f32);
                self.heal_object(id, healed);
            }
        }
        UPDATE_SLEEP_NONE
    }

    // ---- ProductionUpdate / ExitInterface ------------------------------------------------------

    /// `ParkingPlaceBehavior::reserveDoorForExit`: `Some(Some(door))` a door, `Some(None)` is
    /// `DOOR_NONE_NEEDED` (a helipad unit), `None` is `DOOR_NONE_AVAILABLE`.
    pub(crate) fn pp_reserve_door_for_exit(&mut self, af: ObjectId, unit: &ObjectTemplate) -> Option<Option<usize>> {
        self.pp_purge(af);
        if unit.is_collide_kind_of(collide_kind::PRODUCED_AT_HELIPAD) {
            return Some(None);
        }
        let rt = self.objects.get_mut(&af)?.parking.as_mut()?;
        // "note that this never allocates a per-unit parking space".
        let i = rt.find_empty_ppi()?;
        rt.spaces[i].object_in_space = ObjectId::INVALID;
        rt.spaces[i].reserved_for_exit = true;
        Some(Some(i))
    }

    /// `ParkingPlaceBehavior::unreserveDoorForExit`.
    pub(crate) fn pp_unreserve_door_for_exit(&mut self, af: ObjectId, door: usize) {
        if let Some(rt) = self.objects.get_mut(&af).and_then(|o| o.parking.as_mut()) {
            if let Some(s) = rt.spaces.get_mut(door) {
                s.object_in_space = ObjectId::INVALID;
                s.reserved_for_exit = false;
            }
        }
    }

    /// `ParkingPlaceBehavior::exitObjectViaDoor` (`:785`): put `new_obj` in its reserved space at the
    /// hangar and send it taxiing out (`aiFollowExitProductionPath` -> `TAXI_FROM_HANGAR`), or, for
    /// a helipad unit, move it to the rally point / parking spot (`aiMoveToPosition`).
    pub(crate) fn pp_exit_object_via_door(&mut self, af: ObjectId, new_obj: ObjectId, door: Option<usize>) {
        let helipad = self.objects.get(&new_obj).is_some_and(|o| o.template.is_collide_kind_of(collide_kind::PRODUCED_AT_HELIPAD));
        if let Some(d) = door {
            // `it->m_objectInSpace == INVALID && it->m_reservedForExit && door == exitDoor`.
            let ok = self
                .objects
                .get(&af)
                .and_then(|o| o.parking.as_ref())
                .and_then(|rt| rt.spaces.get(d))
                .is_some_and(|s| s.object_in_space == ObjectId::INVALID && s.reserved_for_exit);
            if !ok {
                return; // "could not find the space. what?"
            }
            if let Some(s) = self.objects.get_mut(&af).and_then(|o| o.parking.as_mut()).and_then(|rt| rt.spaces.get_mut(d)) {
                s.object_in_space = new_obj;
                s.reserved_for_exit = false;
            }
        }
        let offset = self.objects.get(&new_obj).and_then(|o| o.template.jet.as_ref()).map_or(0.0, |j| j.parking_offset);
        let (hangar_internal, hangar_orient, parking_space);
        if helipad {
            // `HeliPark01`: `ppinfo.hangarInternal = ppinfo.parkingSpace`.
            let Some(a) = self.objects.get(&af) else { return };
            let b = a.template.parking_bones.heli_park;
            hangar_internal = to_world(&a.position, a.angle, &b.pos);
            hangar_orient = normalize_angle(a.angle + b.orient);
            parking_space = hangar_internal;
        } else {
            match self.pp_reserve_space(af, new_obj, offset) {
                Some(info) => {
                    hangar_internal = info.hangar_internal;
                    hangar_orient = info.hangar_internal_orient;
                    parking_space = info.parking_space;
                }
                None => {
                    let Some(a) = self.objects.get(&af) else { return };
                    hangar_internal = a.position;
                    hangar_orient = a.angle;
                    parking_space = a.position;
                }
            }
        }
        if let Some(o) = self.objects.get_mut(&new_obj) {
            o.set_position(hangar_internal);
            o.set_orientation(hangar_orient);
            o.combat.turret_angle = hangar_orient;
        }
        if self.objects.get(&new_obj).is_none_or(|o| o.ai_handle.is_none()) {
            return;
        }
        let rally = if helipad { self.objects.get(&af).and_then(|o| o.parking.as_ref()).and_then(|rt| rt.heli_rally) } else { None };
        if let Some(r) = rally {
            self.jet_ai_move_to_position(new_obj, r);
        } else if !helipad {
            // `aiFollowExitProductionPath`: `JetAIUpdate::privateFollowPath` with `exitProduction`.
            self.jet_follow_exit_production_path(new_obj, af);
        } else {
            // "Lorenzen sez: aiMoveToPosition has an added benefit. It invokes the pathfinder to find
            // a vacant destination."
            self.jet_ai_move_to_position(new_obj, parking_space);
        }
    }

    /// `ParkingPlaceBehavior::setRallyPoint` (the helicopters' rally point).
    pub(crate) fn pp_set_heli_rally(&mut self, af: ObjectId, pos: Coord3D) {
        if let Some(rt) = self.objects.get_mut(&af).and_then(|o| o.parking.as_mut()) {
            rt.heli_rally = Some(pos);
        }
    }
}
