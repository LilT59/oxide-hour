//! Containment: `OpenContain` / `TransportContain` / `GarrisonContain` / `TunnelContain`.
//!
//! Ported (scoped) from `GameLogic/Object/Contain/OpenContain.cpp` (`isValidContainerFor`,
//! `addToContain`, `removeFromContain`, `onDie`, `exitObjectViaDoor`) and `TransportContain.cpp` /
//! `GarrisonContain.cpp` capacity rules.
//!
//! A contained unit is removed from the partition and the pathfinder (so it is not drawn or
//! targetable), has its AI stopped and its position pinned to the container. Entering units first
//! path to the container, then are contained once close. Exiting places them around the container
//! (through `NumberOfExitPaths`, optionally after `ExitDelay`). When a container dies,
//! `DamagePercentToUnits` is applied to every passenger and the survivors are ejected
//! (`OpenContain::onDie` -> `removeAllContained`).
//!
//! `PassengersAllowedToFire` passengers run their combat state machine while contained (see
//! `combat.rs`) and fire from the container. `TunnelContain` redirects its whole list to the
//! player's [`TunnelTracker`](crate::tunnel::TunnelTracker) (see `tunnel.rs`).

use crate::logic::GameLogic;
use crate::math::{cos, sin, Coord3D, TWO_PI};
use crate::object::{ObjectId, PlayerIndex};
use crate::pathfind::PathObject;
use crate::template::kind_of;
use crate::condition::BodyDamageType;
use crate::damage::{Relationship, HUGE_DAMAGE_AMOUNT};
use crate::body::DamageInfo;

/// Which contain module class this is, one variant per C++ class in `GameLogic/Object/Contain/`.
/// Every one derives from `OpenContain`, which is an `UpdateModule` (`OpenContain.h:80`), so each is a
/// sleepy-heap entry (`registerObject`, GameLogic.cpp:4013-4046).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainKind {
    /// `OpenContain` (`OpenContain.cpp`).
    Open,
    /// `TransportContain` (`TransportContain.cpp`).
    Transport,
    /// `GarrisonContain` (`GarrisonContain.cpp`).
    Garrison,
    /// `TunnelContain` (`TunnelContain.cpp`).
    Tunnel,
    /// `HealContain` (`HealContain.cpp`).
    Heal,
    /// `OverlordContain` (`OverlordContain.cpp`): a `TransportContain` subclass.
    Overlord,
    /// `HelixContain` (`HelixContain.cpp`): a `TransportContain` subclass.
    Helix,
    /// `ParachuteContain` (`ParachuteContain.cpp`): an `OpenContain` subclass.
    Parachute,
    /// `InternetHackContain` (`InternetHackContain.cpp`): a `TransportContain` subclass.
    InternetHack,
    /// `RailedTransportContain` (`RailedTransportContain.cpp`): a `TransportContain` subclass.
    RailedTransport,
    /// `RiderChangeContain` (`RiderChangeContain.cpp`): a `TransportContain` subclass.
    RiderChange,
    /// `MobNexusContain` (`MobNexusContain.cpp`): an `OpenContain` subclass.
    MobNexus,
    /// `CaveContain` (`CaveContain.cpp`): an `OpenContain` subclass.
    Cave,
}

impl ContainKind {
    /// The C++ class name (`ZR_L99_HEAP`'s `mod` field).
    pub fn class_name(self) -> &'static str {
        match self {
            ContainKind::Open => "OpenContain",
            ContainKind::Transport => "TransportContain",
            ContainKind::Garrison => "GarrisonContain",
            ContainKind::Tunnel => "TunnelContain",
            ContainKind::Heal => "HealContain",
            ContainKind::Overlord => "OverlordContain",
            ContainKind::Helix => "HelixContain",
            ContainKind::Parachute => "ParachuteContain",
            ContainKind::InternetHack => "InternetHackContain",
            ContainKind::RailedTransport => "RailedTransportContain",
            ContainKind::RiderChange => "RiderChangeContain",
            ContainKind::MobNexus => "MobNexusContain",
            ContainKind::Cave => "CaveContain",
        }
    }

    /// The C++ `TransportContain` family: `isValidContainerFor`'s own-units/slot rules and the
    /// `HealthRegen%PerSec` update belong to `TransportContain` and every subclass that does not
    /// override them (`OverlordContain`, `HelixContain`, `InternetHackContain`,
    /// `RailedTransportContain`, `RiderChangeContain`). `MobNexusContain` is an `OpenContain`
    /// subclass and has its own rules.
    pub fn is_transport(self) -> bool {
        matches!(
            self,
            ContainKind::Transport
                | ContainKind::Overlord
                | ContainKind::Helix
                | ContainKind::InternetHack
                | ContainKind::RailedTransport
                | ContainKind::RiderChange
        )
    }

    /// The [`crate::template::SleepyModuleKind`] of this class, for the template's INI order.
    pub(crate) fn sleepy_kind(self) -> crate::template::SleepyModuleKind {
        use crate::template::SleepyModuleKind as K;
        match self {
            ContainKind::Open => K::OpenContain,
            ContainKind::Transport => K::TransportContain,
            ContainKind::Garrison => K::GarrisonContain,
            ContainKind::Tunnel => K::TunnelContain,
            ContainKind::Heal => K::HealContain,
            ContainKind::Overlord => K::OverlordContain,
            ContainKind::Helix => K::HelixContain,
            ContainKind::Parachute => K::ParachuteContain,
            ContainKind::InternetHack => K::InternetHackContain,
            ContainKind::RailedTransport => K::RailedTransportContain,
            ContainKind::RiderChange => K::RiderChangeContain,
            ContainKind::MobNexus => K::MobNexusContain,
            ContainKind::Cave => K::CaveContain,
        }
    }
}

/// `GarrisonContain::MAX_GARRISON_POINTS`.
pub const MAX_GARRISON_POINTS: usize = 40;
/// `GarrisonContain::GARRISON_POINT_PRISTINE` / `..._DAMAGED` / `..._REALLY_DAMAGED`.
pub const GARRISON_POINT_PRISTINE: usize = 0;
pub const GARRISON_POINT_DAMAGED: usize = 1;
pub const GARRISON_POINT_REALLY_DAMAGED: usize = 2;
/// `GarrisonContain::MAX_GARRISON_POINT_CONDITIONS`.
pub const GARRISON_POINT_CONDITION_COUNT: usize = 3;

/// A garrison structure's `FIREPOINTnn` bones in **object-local** space, per body-damage
/// condition (pristine / damaged / really damaged). The adapter resolves them from the W3D model
/// (`GarrisonContain::loadGarrisonPoints`); the sim transforms them by the structure's position
/// and orientation and assigns firing passengers to the closest free one. A shorter (or empty)
/// list mirrors the C++ array: entries past the model's bones stay at the structure centre.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GarrisonFirePoints {
    pub conditions: [Vec<Coord3D>; GARRISON_POINT_CONDITION_COUNT],
    /// Non-enclosing garrisons (`IsEnclosingContainer = No`, e.g. firebases): the model's
    /// `STATIONnn` bones in object-local space, resolved from the `GARRISONED` pristine state
    /// (`GarrisonContain::loadStationGarrisonPoints`). Empty for enclosing garrisons or models
    /// without `STATION` bones.
    pub station_points: Vec<Coord3D>,
    /// `PassengersInTurret` containers (Overlord / Helix / Battle Bus): the default-condition
    /// `FIREPOINTnn` bones in object-local space (`OpenContain::putObjAtNextFirePoint`). The sim
    /// rotates them by the container's turret at runtime.
    pub turret_points: Vec<Coord3D>,
    /// `PassengersInTurret` containers: the `Turret` bone's object-local rest position, the pivot
    /// the `FIREPOINTnn` offsets rotate about (`Object::getSingleLogicalBonePositionOnTurret`).
    pub turret_pivot: Option<Coord3D>,
    /// `OpenContain`'s exit bones for a tunnel network (`TunnelContain`, which takes `OpenContain::exitObjectViaDoor`
    /// unmodified): entry `k` is the `ExitStart` / `ExitEnd` pair of exit path `k + 1`, named `ExitStart` /
    /// `ExitEnd` when `NumberOfExitPaths` is 1 and `ExitStart01` .. `ExitStartnn` otherwise
    /// (`OpenContain.cpp:1038-1057`, `getNaturalRallyPoint`). `None` is a bone the model lacks; the
    /// engine then reads the container's own position (`Object::getSingleLogicalBonePosition`).
    pub exit_bones: Vec<(Option<Coord3D>, Option<Coord3D>)>,
}

/// What the sim needs from a contain module (`zr-templates` `ContainData`, converted by the adapter).
#[derive(Clone, Debug, PartialEq)]
pub struct ContainTemplate {
    pub kind: ContainKind,
    /// `Slots` for transports, `ContainMax` otherwise; negative means unlimited.
    pub capacity: i32,
    /// `AllowInsideKindOf` as sim kind-of bits; `None` means "all allowed".
    pub allow_inside_kind_of: Option<u64>,
    /// `ForbidInsideKindOf` as sim kind-of bits (0 = none).
    pub forbid_inside_kind_of: u64,
    pub allow_allies_inside: bool,
    pub allow_enemies_inside: bool,
    pub allow_neutral_inside: bool,
    pub passengers_allowed_to_fire: bool,
    pub damage_percentage_to_units: f32,
    pub number_of_exit_paths: i32,
    pub exit_delay: u32,
    pub door_open_time: u32,
    pub scatter_nearby_on_exit: bool,
    pub orient_like_container_on_exit: bool,
    /// `TransportContainModuleData::m_resetMoodCheckTimeOnExit`: a rider that leaves wakes its AI to look for a
    /// target (`TransportContain::onRemoving`, TransportContain.cpp:407). Transports only.
    pub reset_mood_check_on_exit: bool,
    /// `TransportContainModuleData::m_healthRegen` (`HealthRegen%PerSec`, TransportContain.cpp:158): a percentage
    /// of max health per second, healed by `TransportContain::update` (and `MobNexusContain::update`, which
    /// copies the loop). Zero for every other class.
    pub health_regen: f32,
    /// `GarrisonContainModuleData::m_doIHealObjects` (`HealObjects`, GarrisonContain.h:52,69): whether a
    /// garrison heals its occupants at all (`GarrisonContain::healObjects`, GarrisonContain.cpp:1126-1143).
    pub do_i_heal_objects: bool,
    /// `TimeForFullHeal` in frames, shared by `HealContain` and `GarrisonContain`
    /// (`HealContainModuleData::m_framesForFullHeal` / `GarrisonContainModuleData::m_framesForFullHeal`,
    /// HealContain.h:46, GarrisonContain.h:53). Tunnels keep their own `tunnel_frames_for_full_heal`.
    pub heal_frames: f32,
    /// `InitialPayload` (template name, count) / `InitialRoster`.
    pub initial_payload: Vec<(String, u32)>,
    /// `TunnelContainModuleData::m_framesForFullHeal`: frames for a tunnel passenger to reach full
    /// health (`TimeForFullHeal`, TunnelContain.h:46).
    pub tunnel_frames_for_full_heal: f32,
    /// `GarrisonContainModuleData::m_isEnclosingContainer`. Non-enclosing garrisons (firebases) do
    /// not use fire points.
    pub is_enclosing_container: bool,
    /// The containment bones resolved from the model: `GarrisonContain::m_garrisonPoint` (enclosing
    /// garrisons), `STATION` (non-enclosing) and the turret fire points (`PassengersInTurret`), all
    /// in object-local space. Empty for models without the bones.
    pub fire_points: GarrisonFirePoints,
    /// `OpenContainModuleData::m_passengersInTurret`: the passengers' `FIREPOINT` bones are
    /// recomputed on the container's `TURRET_MAIN` at fire time rather than using the cached
    /// model-space matrices (`OpenContain::putObjAtNextFirePoint`).
    pub passengers_in_turret: bool,
    /// `OpenContainModuleData::m_weaponBonusPassedToPassengers`: the container's weapon bonus
    /// condition bits are added to a contained passenger's flags (`Weapon::computeBonus`).
    pub weapon_bonus_passed_to_passengers: bool,
    /// `GarrisonContainModuleData::m_immuneToClearBuildingAttacks` (`ImmuneToClearBuildingAttacks`): clear-building
    /// attacks cannot touch this building's garrison (`GarrisonContain::isImmuneToClearBuildingAttacks`,
    /// GarrisonContain.h:113). The China Bunker, GLA Palace and America FireBase set it in the shipped data. False
    /// for every other contain class (`OpenContain`/`TunnelContain`/`HelixContain`/`OverlordContain` return true from
    /// their own `isImmuneToClearBuildingAttacks`, but they are not garrisonable, so no clear-building attack reaches
    /// them).
    pub immune_to_clear_building_attacks: bool,
}

impl ContainTemplate {
    /// `Object::isAnyKindOf(allow)` / `forbid` from `OpenContain::isValidContainerFor`.
    pub fn kind_allowed(&self, kind_of_bits: u64) -> bool {
        let allow = self.allow_inside_kind_of.is_none_or(|m| kind_of_bits & m != 0);
        allow && kind_of_bits & self.forbid_inside_kind_of == 0
    }
    pub fn is_garrison(&self) -> bool {
        self.kind == ContainKind::Garrison
    }
    /// `ContainModuleInterface::isGarrisonable()`: only `GarrisonContain` answers true (`OpenContain`, `TunnelContain`,
    /// `CaveContain`, `HelixContain` and `TunnelContain` all return false, and the sim maps `OverlordContain`,
    /// `HelixContain`, `RailedTransport`, `RiderChange`, `Heal` and `InternetHack` to non-garrison kinds).
    /// `OverlordContain::isGarrisonable` redirects to the *first passenger's* contain module; the Overlord is a
    /// transport here, so a clear-building attack cannot reach it (`TransportContain::isGarrisonable` is false too).
    pub fn is_garrisonable(&self) -> bool {
        self.is_garrison()
    }
    /// `ContainModuleInterface::isImmuneToClearBuildingAttacks()` (`GarrisonContain.h:113`).
    pub fn is_immune_to_clear_building_attacks(&self) -> bool {
        self.immune_to_clear_building_attacks
    }
    pub fn is_transport(&self) -> bool {
        self.kind.is_transport()
    }
    pub fn is_tunnel(&self) -> bool {
        self.kind == ContainKind::Tunnel
    }
}

impl GameLogic {
    // ---- queries ---------------------------------------------------------------------------

    /// Is this object currently inside a container?
    pub fn is_contained(&self, id: ObjectId) -> bool {
        self.find_object(id).is_some_and(|o| o.contained_by.is_some())
    }

    /// The container holding `id`, if any.
    pub fn container_of(&self, id: ObjectId) -> Option<ObjectId> {
        self.find_object(id).and_then(|o| o.contained_by)
    }

    /// The units currently inside `container` (in insertion order). Tunnels return the whole
    /// player network's shared list (`TunnelContain::getContainedItemsList`).
    pub fn contained_units(&self, container: ObjectId) -> &[ObjectId] {
        match self.objects.get(&container) {
            Some(o) if o.template.contain.as_ref().is_some_and(|c| c.is_tunnel()) => self.tunnel_passengers(o.player),
            Some(o) => o.contained.as_slice(),
            None => &[],
        }
    }

    /// `OpenContain::isValidContainerFor` + the `TransportContain` / `GarrisonContain` /
    /// `MobNexusContain` / `HelixContain` extensions. `TunnelContain` overrides it entirely
    /// (`TunnelTracker::isValidContainerFor`).
    pub fn is_valid_container_for(&self, container: ObjectId, unit: ObjectId, check_capacity: bool) -> bool {
        let (Some(c), Some(u)) = (self.find_object(container), self.find_object(unit)) else { return false };
        let Some(ct) = c.template.contain.as_ref() else { return false };
        // `HelixContain::isValidContainerFor` (HelixContain.cpp:308-314): a portable structure is
        // always accepted while the Helix carries none (the portable-structure move is not modelled,
        // so `m_portableStructureID` is always `INVALID_ID`). The test is the override's first
        // statement and bypasses `TransportContain::isValidContainerFor` entirely.
        if ct.kind == ContainKind::Helix && u.is_kind_of(kind_of::PORTABLE_STRUCTURE) {
            return true;
        }
        if u.contained_by.is_some() || u.is_dying() || c.is_dying() {
            return false;
        }
        if ct.is_tunnel() {
            return self.tunnel_is_valid_container_for(container, unit, check_capacity);
        }
        if !ct.kind_allowed(u.template.kind_of) {
            return false;
        }
        // Relationship of the unit towards the container (`obj->getRelationship(us)`).
        let allowed = match self.relationship(u.player, c.player) {
            Relationship::Allies => ct.allow_allies_inside,
            Relationship::Enemies => ct.allow_enemies_inside,
            Relationship::Neutral => ct.allow_neutral_inside,
        };
        if !allowed {
            return false;
        }
        // `MobNexusContain::isValidContainerFor` (MobNexusContain.cpp:138-192): after
        // `OpenContain::isValidContainerFor`, the *rider's* relationship to the container must be
        // ALLIES (independent of the INI allow flags), a zero-slot rider is not MobNexusable, and
        // capacity counts transport slots (no `max(1)`).
        if ct.kind == ContainKind::MobNexus {
            if self.relationship(u.player, c.player) != Relationship::Allies {
                return false;
            }
            let slots = u.template.transport_slot_count;
            if slots == 0 {
                return false;
            }
            if check_capacity && ct.capacity >= 0 {
                let used: i32 = c
                    .contained
                    .iter()
                    .filter_map(|id| self.objects.get(id))
                    .map(|o| o.template.transport_slot_count as i32)
                    .sum();
                if used + slots as i32 > ct.capacity {
                    return false;
                }
            }
            return true;
        }
        if ct.is_transport() {
            // Only our own units, and only transportable ones.
            if u.player != c.player || u.template.transport_slot_count == 0 {
                return false;
            }
        }
        // `RiderChangeContain::isValidContainerFor`: a listed rider template on a bike that is not scuttled.
        // The bike's capacity is not checked, because the rider replaces the one already aboard.
        let rider_change = c.template.rider_change.is_some();
        if rider_change && !self.rider_accepts(container, unit) {
            return false;
        }
        if ct.is_garrison() {
            // `GarrisonContain::isValidContainerFor`: zero-health buildings are not garrisonable,
            // and really-damaged buildings reject new occupants unless they are
            // `GARRISONABLE_UNTIL_DESTROYED`. `NO_GARRISON` objects can never be garrisoned.
            if u.template.kind_of & kind_of::NO_GARRISON != 0 {
                return false;
            }
            if c.body.as_ref().is_some_and(|b| b.health() <= 0.0) {
                return false;
            }
            let really_damaged = c.body.as_ref().is_some_and(|b| b.damage_state() == BodyDamageType::ReallyDamaged);
            if really_damaged && c.template.kind_of & kind_of::GARRISONABLE_UNTIL_DESTROYED == 0 {
                return false;
            }
        }
        if check_capacity && ct.capacity >= 0 && !rider_change {
            // Garrisons count occupants (`GarrisonContain::isValidContainerFor`); transports count
            // transport slots.
            let (used, need) = if ct.kind == ContainKind::Garrison {
                (c.contained.len() as i32, 1)
            } else {
                let used: i32 = c.contained.iter().filter_map(|id| self.objects.get(id)).map(|o| o.template.transport_slot_count.max(1) as i32).sum();
                (used, u.template.transport_slot_count.max(1) as i32)
            };
            if used + need > ct.capacity {
                return false;
            }
        }
        true
    }

    /// `OpenContain::isPassengerAllowedToFire`, cascaded up through any enclosing containers.
    /// `GarrisonContain::isPassengerAllowedToFire` (GarrisonContain.cpp:1716) is `TRUE` unless the
    /// building is `DISABLED_SUBDUED`.
    pub fn is_passenger_allowed_to_fire(&self, id: ObjectId) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        let Some(ct) = o.template.contain.as_ref() else { return false };
        let allowed = if ct.kind == ContainKind::Garrison {
            !o.is_disabled_by_type(crate::disabled::DisabledType::Subdued)
        } else {
            // `OpenContain::isPassengerAllowedToFire` (OpenContain.cpp:1225-1237) reads the module
            // instance's flag, which `PassengersFireUpgrade` can set (`setPassengerAllowedToFire(TRUE)`).
            ct.passengers_allowed_to_fire || o.contain_passengers_allowed_to_fire
        };
        if !allowed {
            return false;
        }
        match o.contained_by {
            Some(by) => self.is_passenger_allowed_to_fire(by),
            None => true,
        }
    }

    /// The world position a contained passenger occupies right now. An enclosing garrison's
    /// assigned fire point (`GarrisonContain::getObjectGarrisonPointIndex`), otherwise the
    /// container centre. Transports and non-enclosing garrisons fire from the centre
    /// (`OpenContain::putObjAtNextFirePoint` places an object with no `FIREPOINT` bones there).
    ///
    /// The old "point on the bounding circle facing the target" approximation is gone: the
    /// assigned point comes from the model's `FIREPOINT` bones resolved by the adapter.
    pub fn passenger_fire_position(&self, container: ObjectId, unit: ObjectId) -> Coord3D {
        let Some(c) = self.objects.get(&container) else { return Coord3D::ZERO };
        let Some(ct) = c.template.contain.as_ref() else { return c.position };
        if ct.is_garrison() {
            // A non-enclosing garrison (firebase) never moves its passengers: they stay at their
            // assigned `STATION` bone (`positionObjectsAtStationGarrisonPoints`).
            if !ct.is_enclosing_container {
                return self.station_point_position(container, unit).unwrap_or(c.position);
            }
            if self.garrison_unit_at_point(container, unit) {
                return self.objects.get(&unit).map_or(c.position, |u| u.position);
            }
            return c.position;
        }
        // `PassengersInTurret`: the fire point is recomputed on the container's turret
        // (`OpenContain::putObjAtNextFirePoint` + `Object::getSingleLogicalBonePositionOnTurret`).
        if ct.passengers_in_turret && !ct.fire_points.turret_points.is_empty() {
            return self.turret_passenger_position(container, unit);
        }
        c.position
    }

    /// World position of the `FIREPOINTnn` bone a `PassengersInTurret` passenger occupies, rotated
    /// about the container turret's pivot by the turret's yaw relative to the body, then by the
    /// body's orientation. Mirrors the launch-bone turret transform (`combat::launch_position`).
    fn turret_passenger_position(&self, container: ObjectId, unit: ObjectId) -> Coord3D {
        let Some(c) = self.objects.get(&container) else { return Coord3D::ZERO };
        let ct = c.template.contain.as_ref().expect("passengers-in-turret template");
        let points = &ct.fire_points.turret_points;
        let index = c.contained.iter().position(|&x| x == unit).unwrap_or(0) % points.len();
        let bone = points[index];
        // `getSingleLogicalBonePositionOnTurret` rotates about the turret pivot. The sim stores the
        // turret yaw in world terms, so the relative angle is used (`combat::launch_position`).
        let mut p = [bone.x, bone.y, bone.z];
        if let Some(piv) = ct.fire_points.turret_pivot {
            let rel = crate::math::angle_diff(c.combat.turret_angle, c.angle);
            p = crate::combat::rotate_z_about(p, [piv.x, piv.y, piv.z], rel);
        }
        let (s, co) = (sin(c.angle), cos(c.angle));
        Coord3D::new(c.position.x + p[0] * co - p[1] * s, c.position.y + p[0] * s + p[1] * co, c.position.z + p[2])
    }

    /// Where a contained passenger should sit this frame: its assigned fire point, or the
    /// container centre when not firing.
    pub(crate) fn passenger_anchor_position(&self, container: ObjectId, unit: ObjectId) -> Coord3D {
        self.passenger_fire_position(container, unit)
    }

    // ---- containment -----------------------------------------------------------------------

    /// Recompute [`Object::contained_mass`] (`OpenContain::getContainedItemsMass`,
    /// `OpenContain.cpp:835-845`) after a unit joins or leaves `container`'s list, and for every
    /// container above it in turn (a nested container's mass feeds its own parent).
    pub(crate) fn refresh_contained_mass(&mut self, container: ObjectId) {
        let (mass, parent) = match self.objects.get(&container) {
            Some(c) => {
                let mass = c
                    .contained
                    .iter()
                    .filter_map(|u| self.objects.get(u))
                    .filter(|u| u.template.physics.is_some())
                    .map(|u| u.get_mass().unwrap_or(0.0))
                    .sum();
                (mass, c.contained_by)
            }
            None => return,
        };
        if let Some(c) = self.objects.get_mut(&container) {
            c.contained_mass = mass;
        }
        if let Some(parent) = parent {
            self.refresh_contained_mass(parent);
        }
    }

    /// `OpenContain::addToContain`: hide the unit from the world and pin it to the container.
    pub(crate) fn add_to_contain(&mut self, container: ObjectId, unit: ObjectId) {
        // "'%s' is trying to contain '%s', but '%s' is already contained by '%s'" (OpenContain.cpp:349-360).
        if self.objects.get(&unit).is_none_or(|u| u.contained_by.is_some() || u.destroyed) {
            return;
        }
        self.partition.unregister(unit);
        self.pathfinder.clear_unit(unit);
        let now = self.frame;
        if let Some(u) = self.objects.get_mut(&unit) {
            u.contained_by = Some(container);
            u.enter_target = None;
            u.exit_at = 0;
            u.contained_frame = now;
            u.ai.stop();
            u.combat.attack = None;
        }
        let pos = self.find_object(container).map_or(Coord3D::ZERO, |c| c.position);
        if let Some(u) = self.objects.get_mut(&unit) {
            u.set_position(pos);
        }
        // `TunnelContain::addToContainList` stores into the player's network instead.
        if self.is_tunnel_container(container) {
            self.tunnel_add_to_contain(container, unit);
        } else {
            if let Some(c) = self.objects.get_mut(&container) {
                c.contained.push(unit);
            }
            self.refresh_contained_mass(container);
        }
        // `RiderChangeContain::onContaining` runs after the rider joins the list (`OpenContain::addToContain`).
        if self.is_rider_change(container) {
            self.rider_on_containing(container, unit);
        }
        // `onContaining` -> `setDisabled(DISABLED_HELD)` (GarrisonContain.cpp:1624,
        // TransportContain.cpp:283, TunnelContain.cpp:222).
        self.set_disabled(unit, crate::disabled::DisabledType::Held);
    }

    /// `OpenContain::addToContain` for a unit that is already at the container (used by
    /// `AIRappelState`'s combat-drop arrival): validate and contain immediately.
    pub(crate) fn force_enter_contain(&mut self, container: ObjectId, unit: ObjectId) {
        if self.is_valid_container_for(container, unit, true) {
            self.add_to_contain(container, unit);
        }
    }

    /// `OpenContain::removeFromContain` + `exitObjectViaDoor`: put the unit back in the world near
    /// the container, through the next exit path.
    pub(crate) fn remove_from_contain(&mut self, container: ObjectId, unit: ObjectId) {
        self.remove_from_contain_ex(container, unit, false);
    }

    /// As [`remove_from_contain`](Self::remove_from_contain). `replacing` is `m_containing` of
    /// `RiderChangeContain`: the unit leaves because another rider is taking its place, so the bike
    /// is not scuttled.
    pub(crate) fn remove_from_contain_ex(&mut self, container: ObjectId, unit: ObjectId, replacing: bool) {
        if !self.contain_remove_unit(container, unit, replacing) {
            return;
        }
        // The non-exit callers (destruction, a collision ejection without an AI) keep the port's ring placement;
        // the exit paths place the unit themselves (`exit_object_via_door`, contain_exit.rs).
        let pos = self.exit_position(container, unit);
        self.place_removed_unit(unit, pos);
    }

    /// The bookkeeping half of `OpenContain::removeFromContain` (OpenContain.cpp:424-447, `removeFromContainViaIterator`
    /// :689-746) and of the class's `onRemoving`: the list and garrison/station points, the `RiderChangeContain` hook, and
    /// `TransportContain::onRemoving`'s exit timer (`m_frameExitNotBusy`, TransportContain.cpp:410). It does not place the
    /// unit: C++ leaves the placement to the caller (`exitObjectViaDoor` sets the exit bone, `TunnelContain::onRemoving`
    /// the container's position). Returns false when the rider died in its `RiderChangeContain::onRemoving`.
    pub(crate) fn contain_remove_unit(&mut self, container: ObjectId, unit: ObjectId, replacing: bool) -> bool {
        if self.is_tunnel_container(container) {
            self.tunnel_remove_from_contain(unit);
        } else {
            if let Some(c) = self.objects.get_mut(&container) {
                c.contained.retain(|&x| x != unit);
            }
            self.refresh_contained_mass(container);
            // `GarrisonContain::onRemoving` -> `removeObjectFromGarrisonPoint` (enclosing) or
            // `removeObjectFromStationPoint` (non-enclosing); each is a no-op for the other kind.
            self.remove_object_from_garrison_point(container, unit);
            self.remove_object_from_station_point(container, unit);
        }
        // `RiderChangeContain::onRemoving`. A dead bike destroys the rider instead of releasing it.
        if self.is_rider_change(container) && self.rider_on_removing(container, unit, replacing) {
            return false;
        }
        // `TransportContain::onRemoving` (TransportContain.cpp:386-391): a container above the ground lets its rider fall,
        // "temporarily mark the guy as being allowed to fall ... this will be reset (by PhysicsBehavior) when he touches
        // the ground". The test is `Object::isAboveTerrain` (`Thing.h:129`, `Thing.cpp:293-311`): height above the
        // *terrain* ground height, `> 0` with no epsilon (not the bridge-layer surface the physics uses). The flag is
        // what `exitObjectViaDoor` reads and restores around the exit path (OpenContain.cpp:1069-1110), so the rider
        // falls after it. `KeepContainerVelocityOnExit` (TransportContain.cpp:353-368) is not ported: `ContainTemplate`
        // has no field for it.
        let transport = self.objects.get(&container).and_then(|o| o.template.contain.as_ref()).is_some_and(|c| c.is_transport());
        if transport && self.objects.get(&container).is_some_and(|o| o.position.z - self.terrain.ground_height(o.position.x, o.position.y) > 0.0) {
            self.set_allow_to_fall(unit, true);
        }
        // `TransportContain::onRemoving`: `m_frameExitNotBusy = now + m_exitDelay` (TransportContain.cpp:410).
        let timer = self.objects.get(&container).and_then(|o| o.template.contain.as_ref()).filter(|c| c.is_transport()).map(|c| c.exit_delay);
        if let Some(delay) = timer {
            let now = self.frame;
            if let Some(o) = self.objects.get_mut(&container) {
                o.behavior.frame_exit_not_busy = now + delay;
            }
        }
        true
    }

    /// The placement half of `removeFromContain`: the unit leaves the container (`contained_by` cleared), stands at
    /// `pos`, and is re-registered in the partition and the pathfinder (`OpenContain::addOrRemoveObjFromWorld`,
    /// OpenContain.cpp:222-237), with its held state cleared.
    pub(crate) fn place_removed_unit(&mut self, unit: ObjectId, pos: Coord3D) {
        if let Some(u) = self.objects.get_mut(&unit) {
            u.contained_by = None;
            u.enter_target = None;
            u.exit_at = 0;
            u.contained_frame = 0;
            u.set_position(pos);
        }
        // Re-register in the world.
        if let Some(u) = self.objects.get(&unit) {
            let radius = u.template.geometry.bounding_circle_radius();
            let template = u.template.clone();
            // `OpenContain::addOrRemoveObjFromWorld` (OpenContain.cpp:237) -> `registerObject`: the module is
            // dirty and its cells are placed by the partition pass (PartitionManager.cpp:2913, :2796).
            self.partition.register_deferred(unit, pos, radius);
            // `registerObject` -> `makeDirty(true)` on a fresh list entry: a flag set while the unit was
            // contained does not keep its older place (`PartitionManager.cpp:2270`).
            if let Some(o) = self.objects.get_mut(&unit) {
                o.clear_dirty();
                o.mark_partition_dirty();
            }
            if template.locomotor.is_some() && template.kind_of & kind_of::IMMOBILE == 0 {
                if let Some(po) = self.objects.get(&unit).and_then(PathObject::for_object) {
                    self.pathfinder.update_unit_pos(&po, &pos, false);
                }
            }
        }
        // `onRemoving` clears the `DISABLED_HELD` set on entry (GarrisonContain.cpp:1673,
        // TransportContain.cpp:330, TunnelContain.cpp:240).
        self.clear_disabled(unit, crate::disabled::DisabledType::Held);
        // No wake (R28 F2): `OpenContain::onRemoving`, `GarrisonContain::onRemoving` (GarrisonContain.cpp:1654-1680) and
        // `Object::clearDisabled` never wake the AI module. A held unit's module is deferred one frame at a time
        // (GameLogic.cpp:3851-3905), so it runs again on the frame after the release.
    }

    /// A point on the exit ring, alternating through `NumberOfExitPaths` and snapped to terrain.
    fn exit_position(&self, container: ObjectId, unit: ObjectId) -> Coord3D {
        let Some(c) = self.find_object(container) else { return Coord3D::ZERO };
        let paths = c.template.contain.as_ref().map_or(1, |ct| ct.number_of_exit_paths).max(1);
        let index = self.contained_units(container).len() as i32;
        let unit_r = self.find_object(unit).map_or(0.0, |u| u.template.geometry.bounding_circle_radius());
        let dist = c.template.geometry.bounding_circle_radius() + unit_r + 5.0;
        let angle = TWO_PI * (index % paths) as f32 / paths as f32;
        let x = c.position.x + cos(angle) * dist;
        let y = c.position.y + sin(angle) * dist;
        Coord3D::new(x, y, self.terrain.ground_height(x, y))
    }

    /// `OpenContain::killAllContained`. A tunnel clears its shared list first
    /// (`TunnelContain::killAllContained` swaps the list away before killing).
    pub fn kill_all_contained(&mut self, container: ObjectId) {
        if self.is_tunnel_container(container) {
            let player = self.objects.get(&container).map_or(0, |o| o.player);
            let ids: Vec<ObjectId> = self.tunnel_trackers.get_mut(&player).map(|t| std::mem::take(&mut t.passengers)).unwrap_or_default();
            for id in ids {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.contained_by = None;
                    o.contained_frame = 0;
                }
                self.apply_damage(id, DamageInfo { source: ObjectId::INVALID, amount: HUGE_DAMAGE_AMOUNT, kind: crate::damage::DamageType::Unresistable }, crate::damage::DeathType::Normal);
            }
            return;
        }
        let ids = self.find_object(container).map(|c| c.contained.clone()).unwrap_or_default();
        for id in ids {
            self.apply_damage(id, DamageInfo { source: ObjectId::INVALID, amount: HUGE_DAMAGE_AMOUNT, kind: crate::damage::DamageType::Unresistable }, crate::damage::DeathType::Normal);
        }
    }

    // ---- commands --------------------------------------------------------------------------

    /// `MSG_DO_ENTER` (AIEnter): `privateEnter` (`AIUpdate.cpp:3776`) then `AIEnterState`.
    ///
    /// Despite the name, the engine's `canEnterObject` also accepts the enter-type targets that
    /// have no contain module — a Hijacker's enemy vehicle, a Terrorist's car-bomb target and a
    /// Saboteur's building all reach their effect through the collide modules, not containment.
    pub(crate) fn command_enter(&mut self, player: PlayerIndex, ids: &[ObjectId], container: ObjectId) {
        self.command_enter_from(player, ids, container, crate::ai_states::CommandSource::Player);
    }

    /// As [`command_enter`](Self::command_enter), with the engine's `CommandSourceType` (the
    /// `CommandButtonHuntUpdate` enter hunt issues its `MSG_ENTER` with `CMD_FROM_AI`).
    pub(crate) fn command_enter_from(
        &mut self,
        player: PlayerIndex,
        ids: &[ObjectId],
        container: ObjectId,
        source: crate::ai_states::CommandSource,
    ) {
        for &id in ids {
            // `JetAIUpdate::privateEnter`: a jet "enters" an airfield by landing on it.
            if self.jet_command_enter(player, id, container) {
                continue;
            }
            // `privateEnter` -> `canEnterObject(..., DONT_CHECK_CAPACITY)`.
            if id == container || !self.can_enter_object(id, container, false) {
                continue;
            }
            let Some(u) = self.objects.get(&id) else { continue };
            if u.player != player || u.ai.suspended {
                continue;
            }
            // `privateEnter` (`AIUpdate.cpp:3790`) clears the machine before `setGoalObject`, so an
            // old `AI_ENTER`/`AI_DOCK` `onExit` reads the old goal.
            if !self.is_jet(id) {
                self.ai_clear(id);
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.combat.attack = None;
                o.ai.machine.goal_object = container;
                o.ai.machine.last_command_source = source;
            }
            self.ai_set_state(id, crate::ai_states::AiStateType::Enter);
        }
    }

    /// `MSG_DO_EXIT`: `privateExit` (`AIUpdate.cpp:3826`) then `AIExitState`; a passenger leaves
    /// after `ExitDelay`.
    pub(crate) fn command_exit(&mut self, player: PlayerIndex, container: ObjectId, ids: &[ObjectId]) {
        let Some(c) = self.find_object(container) else { return };
        if c.player != player {
            return;
        }
        self.exit_passengers(container, ids);
    }

    /// `OpenContain::orderAllPassengersToExit` for the passengers `ids` of `container`, with no owner
    /// check. Used by the script evacuate paths (`AIUpdateInterface::privateEvacuate`,
    /// `AIUpdate.cpp:3922`, and the structure branch of `AIGroup::groupEvacuate`, `AIGroup.cpp:2496-2505`).
    pub(crate) fn exit_passengers(&mut self, container: ObjectId, ids: &[ObjectId]) {
        for &id in ids {
            if !self.contained_units(container).contains(&id) {
                continue;
            }
            // `TransportContain::onRemoving` (TransportContain.cpp:404-407): a transport with
            // `ResetMoodCheckTimeOnExit` wakes the rider while it is still idle.
            let reset = self.objects.get(&container).and_then(|c| c.template.contain.as_ref()).is_some_and(|t| t.reset_mood_check_on_exit);
            if reset {
                self.wake_up_and_attempt_to_target(id);
            }
            if let Some(u) = self.objects.get_mut(&id) {
                u.ai.machine.goal_object = container;
            }
            self.ai_set_state(id, crate::ai_states::AiStateType::Exit);
        }
    }

    /// `Object::onCapture`'s containment part. A transport that changes hands ejects its riders
    /// (`TransportContain::onCapture`). A tunnel re-registers with the new owner's tracker
    /// (`TunnelContain::onCapture`); garrisons keep their occupants (`GarrisonContain` does not
    /// override `onCapture`). The engine distinguishes a sniped (unmanned) vehicle, which dumps
    /// everyone immediately, from an ordinary capture, which only ejects when the new owner is not
    /// allied with the old (`TheSuperHackers` bugfix).
    pub(crate) fn on_object_captured(&mut self, container: ObjectId, old_player: PlayerIndex, new_player: PlayerIndex) {
        if old_player == new_player {
            return;
        }
        // `Object::onCapture` (Object.cpp:4593): `newOwner->getScoreKeeper()->addObjectCaptured(this)`.
        self.score_object_captured(container);
        // `TechBuildingBehavior::onCapture` (TechBuildingBehavior.cpp:139-145): wake the module next frame so it
        // re-evaluates the captured model condition.
        self.tech_building_on_capture(container);
        // The upgrade modules' `onCapture` (`Object::onCapture`'s behaviour loop): a `RadarUpgrade` and a
        // `CostModifierUpgrade` move their player-level effect to the new owner (a captured command
        // center / tech oil refinery).
        self.upgrade_module_capture(container, old_player, new_player);
        let Some(c) = self.find_object(container) else { return };
        let Some(ct) = c.template.contain.as_ref() else { return };
        // `TunnelContain::onCapture` (`TunnelContain.cpp:506`): unregister from the old owner's
        // tunnel system and register with the new owner's. The engine asserts the tunnel is empty
        // ("you shouldn't force a capture of a Tunnel with people in it"), so the base
        // `OpenContain::onCapture` has nothing to do.
        if ct.kind == ContainKind::Tunnel {
            let registered = self.tunnel_trackers.get(&old_player).is_some_and(|t| t.tunnel_ids.contains(&container));
            if registered {
                self.on_tunnel_destroyed_for(old_player, container);
                self.on_tunnel_created_for(new_player, container);
            }
            return;
        }
        if !ct.is_transport() {
            return;
        }
        if !c.unmanned() && self.relationship(old_player, new_player) == Relationship::Allies {
            return;
        }
        let ids = c.contained.clone();
        for id in ids {
            self.remove_from_contain(container, id);
        }
    }

    /// `MSG_DO_EVACUATE` (`orderAllPassengersToExit`): everyone leaves.
    pub(crate) fn command_evacuate(&mut self, player: PlayerIndex, container: ObjectId, source: crate::ai_states::CommandSource) {
        let Some(c) = self.find_object(container) else { return };
        if c.player != player {
            return;
        }
        // `AIUpdateInterface::privateEvacuate` is an `aiDoCommand` (AICMD_EVACUATE) on the container; an assault transport's
        // override resets for it, with the command's real source (AssaultTransportAIUpdate.cpp:112-115; the reset is skipped for
        // CMD_FROM_AI, :86).
        self.assault_command(container, crate::assault_transport::AssaultCommand::Other, source);
        let ids = self.contained_units(container).to_vec();
        self.command_exit(player, container, &ids);
    }

    // ---- per-frame -------------------------------------------------------------------------

    /// The update of one contain module, run from its own sleepy-heap slot (`OpenContain` is an
    /// `UpdateModule`, `OpenContain.h:80`; `registerObject` pushes it, GameLogic.cpp:4013-4046).
    /// Every class's `update()` ends in `OpenContain::update` and returns `UPDATE_SLEEP_NONE`
    /// (`OpenContain.cpp:199-218`, `TransportContain.cpp:468-526`, `TunnelContain.cpp:552-595`,
    /// `GarrisonContain.cpp:910-970`, `HealContain.cpp:92-132`, `RiderChangeContain.cpp:363-379`).
    pub(crate) fn contain_family_update(&mut self, container: ObjectId, kind: ContainKind) -> u32 {
        match kind {
            ContainKind::Open => self.open_contain_base_update(container),
            ContainKind::Cave => {
                // `CaveContain` has no `update` override; its cave-system list is not modelled.
                self.open_contain_base_update(container);
            }
            ContainKind::Parachute => {
                // `ParachuteContain::update` (ParachuteContain.cpp:277-444) drives the chute's sway and
                // landing through the drawable and locomotor; the port models neither, so only the
                // `OpenContain` half runs.
                self.open_contain_base_update(container);
            }
            ContainKind::Transport
            | ContainKind::InternetHack
            | ContainKind::RailedTransport => self.transport_contain_update(container),
            // `OverlordContain` and `HelixContain` override `onObjectCreated` to create their payload at
            // once and inherit `TransportContain::update` for the rest (OverlordContain.cpp:107-155,
            // HelixContain.cpp:108-189); `contain_on_object_created` created it, so the inherited
            // `createPayload` call is a no-op and the class update is the transport one.
            ContainKind::Overlord | ContainKind::Helix => self.transport_contain_update(container),
            // `RiderChangeContain::update` (:363-379) then `TransportContain::update`.
            ContainKind::RiderChange => {
                self.rider_update_one(container);
                self.transport_contain_update(container)
            }
            ContainKind::Tunnel => self.tunnel_contain_update(container),
            ContainKind::Garrison => self.garrison_contain_update(container),
            ContainKind::Heal => self.heal_contain_slot_update(container),
            // `MobNexusContain::update` (MobNexusContain.cpp:335-380) heals like a transport and ends in
            // `OpenContain::update`.
            ContainKind::MobNexus => self.mob_nexus_contain_update(container),
        }
        crate::module::UPDATE_SLEEP_NONE
    }

    /// `OpenContain::update` (OpenContain.cpp:199-218) plus the port's per-container bookkeeping that
    /// stands in for the engine's AIExit arrival checks and the pinned positions of occupants: the
    /// contained units' anchor positions and the exit-delay queue. The engine's own base update only
    /// resets the entered mask, runs `monitorConditionChanges` (drawable state), the door-close
    /// countdown and `pruneDeadWanters`; none of those have sim state, so this is the whole base.
    fn open_contain_base_update(&mut self, container: ObjectId) {
        // Contained units follow their container (and are never elsewhere). A passenger allowed to
        // fire sits at its assigned fire point.
        let follows: Vec<ObjectId> = self
            .objects
            .values()
            .filter(|o| !o.destroyed && o.contained_by == Some(container))
            .map(|o| o.id)
            .collect();
        for unit in follows {
            let pos = self.passenger_anchor_position(container, unit);
            if let Some(u) = self.objects.get_mut(&unit) {
                u.set_position(pos);
            }
        }
    }

    /// `TransportContain::update` (TransportContain.cpp:468-526): `createPayload` on the first update,
    /// then `HealthRegen%PerSec` for every hurt passenger (`maxHealth * m_healthRegen / 100 *
    /// SECONDS_PER_LOGICFRAME_REAL`), then `OpenContain::update`.
    fn transport_contain_update(&mut self, container: ObjectId) {
        self.create_payload(container);
        let percent = self
            .objects
            .get(&container)
            .and_then(|o| o.template.contain.as_ref())
            .map_or(0.0, |c| c.health_regen);
        if percent != 0.0 {
            let contained = self.objects.get(&container).map_or(Vec::new(), |o| o.contained.clone());
            for unit in contained {
                let Some(o) = self.objects.get(&unit) else { continue };
                if o.destroyed || o.is_dying() {
                    continue;
                }
                let Some(body) = o.body.as_ref() else { continue };
                // "Determine if we need healing or not."
                if body.health() < body.max_health() {
                    let regen = body.max_health() * percent / 100.0 / crate::math::LOGICFRAMES_PER_SECOND as f32;
                    self.heal_object(unit, regen);
                }
            }
        }
        self.open_contain_base_update(container);
    }

    /// `MobNexusContain::update` (MobNexusContain.cpp:335-380): the same `HealthRegen%PerSec` loop as a
    /// transport, then `OpenContain::update`. (Its `createPayload` runs in `onObjectCreated`, so the
    /// base update is all this class adds.)
    fn mob_nexus_contain_update(&mut self, container: ObjectId) {
        let percent = self
            .objects
            .get(&container)
            .and_then(|o| o.template.contain.as_ref())
            .map_or(0.0, |c| c.health_regen);
        if percent != 0.0 {
            let contained = self.objects.get(&container).map_or(Vec::new(), |o| o.contained.clone());
            for unit in contained {
                let Some(o) = self.objects.get(&unit) else { continue };
                if o.destroyed || o.is_dying() {
                    continue;
                }
                let Some(body) = o.body.as_ref() else { continue };
                if body.health() < body.max_health() {
                    let regen = body.max_health() * percent / 100.0 / crate::math::LOGICFRAMES_PER_SECOND as f32;
                    self.heal_object(unit, regen);
                }
            }
        }
        self.open_contain_base_update(container);
    }

    /// `TunnelContain::update` (TunnelContain.cpp:552-595): `OpenContain::update`, then
    /// `TunnelTracker::healObjects(m_framesForFullHeal)` for the whole network and the "recently
    /// attacked" nemesis update.
    fn tunnel_contain_update(&mut self, container: ObjectId) {
        self.open_contain_base_update(container);
        // The tunnel's own `TimeForFullHeal` is passed to `healObjects`, so several entrances stack.
        let frames = self
            .objects
            .get(&container)
            .and_then(|o| o.template.contain.as_ref())
            .map_or(0.0, |c| c.tunnel_frames_for_full_heal);
        let player = self.objects.get(&container).map(|o| o.player);
        if let Some(player) = player {
            self.tunnel_heal_objects(player, frames);
            // `if (body->getLastDamageTimestamp() + LOGICFRAMES_PER_SECOND > now)` -> `updateNemesis`.
            let now = self.frame;
            let Some((timestamp, source)) = self
                .objects
                .get(&container)
                .map(|o| (o.last_damage_timestamp, o.last_damage_info.source))
            else {
                return;
            };
            if timestamp + crate::math::LOGICFRAMES_PER_SECOND > now && self.find_object(source).is_some() {
                self.update_tunnel_nemesis(container, source);
            }
        }
    }

    /// `GarrisonContain::update` (GarrisonContain.cpp:910-970): `OpenContain::update`, remove the
    /// effectively dead, `matchObjectsToGarrisonPoints`, `healObjects` and `moveObjectsWithMe`.
    fn garrison_contain_update(&mut self, container: ObjectId) {
        self.open_contain_base_update(container);
        let contained = self.objects.get(&container).map_or(Vec::new(), |o| o.contained.clone());
        for unit in contained {
            if self.is_effectively_dead(unit) {
                self.remove_from_contain(container, unit);
            }
        }
        // `matchObjectsToGarrisonPoints` (GarrisonContain.cpp:978-1001).
        self.match_garrison_point_container(container);
        // `healObjects` (:1126-1143): only while `DoIHealObjects`; `TimeForFullHeal` frames to full.
        let (heals, frames) = self
            .objects
            .get(&container)
            .and_then(|o| o.template.contain.as_ref())
            .map_or((false, 0.0), |c| (c.do_i_heal_objects, c.heal_frames));
        if heals {
            let now = self.frame;
            let contained = self.objects.get(&container).map_or(Vec::new(), |o| o.contained.clone());
            for unit in contained {
                let Some(o) = self.objects.get(&unit) else { continue };
                if o.destroyed || o.is_dying() {
                    continue;
                }
                let Some(body) = o.body.as_ref() else { continue };
                let max = body.max_health();
                if now.saturating_sub(o.contained_frame) as f32 >= frames {
                    self.heal_object(unit, max);
                } else if frames > 0.0 {
                    self.heal_object(unit, max / frames);
                }
            }
        }
        // `moveObjectsWithMe` (:1158-...) only runs for a `MobileGarrison`; no shipped garrison sets it.
    }

    /// The `HealContain::update` half of the slot (HealContain.cpp:92-132): `OpenContain::update` then
    /// `doHeal` each passenger and eject the ones that are done.
    fn heal_contain_slot_update(&mut self, container: ObjectId) {
        self.open_contain_base_update(container);
        self.heal_contain_update(container);
    }

    /// `TransportContain::update` -> `createPayload` (TransportContain.cpp:468-526,
    /// `createPayload` at :434-462): a transport with an `InitialPayload` creates its passengers once,
    /// on its first update, and loads each into itself (`newObject` for the payload's owner, then
    /// `addToContain` when it is a valid passenger). `OverlordContain`/`HelixContain` inherit this
    /// call but have already created their payload in `onObjectCreated`, so `payload_created` makes
    /// it a no-op (`OverlordContain.cpp:107-155`, `HelixContain.cpp:108-189`). A passenger that is
    /// not valid stays in the world, as in C++.
    fn create_payload(&mut self, container: ObjectId) {
        let Some(c) = self.objects.get_mut(&container) else { return };
        if c.payload_created || c.destroyed {
            return;
        }
        c.payload_created = true;
        let (pos, angle, player) = (c.position, c.angle, c.player);
        let payload = c.template.contain.as_ref().map(|t| t.initial_payload.clone()).unwrap_or_default();
        for (name, count) in payload {
            let Some(t) = self.find_template(&name) else { continue };
            for _ in 0..count {
                let unit = self.create_object(&t, pos, angle, player);
                if self.is_valid_container_for(container, unit, true) {
                    self.add_to_contain(container, unit);
                }
            }
        }
    }

    /// The `onObjectCreated` half of the four contain classes that create their payload at creation
    /// instead of on the first update: `OverlordContain::onObjectCreated` (OverlordContain.cpp:107-111),
    /// `HelixContain::onObjectCreated` (HelixContain.cpp:108-112), `MobNexusContain::onObjectCreated`
    /// (MobNexusContain.cpp:308-333) and `GarrisonContain::onObjectCreated` (GarrisonContain.cpp:1762-1784).
    /// `Object::initObject` runs every module's `onObjectCreated` before `GameLogic::registerObject`
    /// (Object.cpp:473-484), i.e. inside `createObject` and before any other module sees the object —
    /// so the payloads take their ids right after the container's, in template module order.
    /// `TransportContain` (and its plain subclasses) keep the first-update `createPayload`.
    pub(crate) fn contain_on_object_created(&mut self, container: ObjectId) {
        let Some(kind) = self.objects.get(&container).and_then(|o| o.template.contain.as_ref()).map(|c| c.kind) else {
            return;
        };
        if matches!(kind, ContainKind::Overlord | ContainKind::Helix | ContainKind::MobNexus | ContainKind::Garrison) {
            self.create_payload(container);
        }
    }

    /// `HealContain::update` (`HealContain.cpp:92-126`), run from the sleepy heap: every frame, heal each
    /// contained unit by `maxHealth / framesForFullHeal`; a unit that has been inside for `framesForFullHeal`
    /// frames is healed to full and leaves through a door (`doHeal`, `:128-180`). Returns `UPDATE_SLEEP_NONE`.
    pub(crate) fn heal_contain_update(&mut self, container: ObjectId) -> u32 {
        let Some(frames) = self
            .objects
            .get(&container)
            .and_then(|o| o.template.contain.as_ref())
            .map(|c| c.heal_frames)
        else {
            return crate::module::UPDATE_SLEEP_NONE;
        };
        let now = self.frame;
        let contained = self.objects.get(&container).map_or(Vec::new(), |o| o.contained.clone());
        for unit in contained {
            let Some(o) = self.objects.get(&unit) else { continue };
            if o.destroyed || o.is_dying() {
                continue;
            }
            let max = o.body.as_ref().map_or(0.0, |b| b.max_health());
            if max <= 0.0 {
                continue;
            }
            if now.saturating_sub(o.contained_frame) as f32 >= frames {
                // `healInfo.in.m_amount = body->getMaxHealth()` then `doneHealing = TRUE` (`:155-167`). A zero
                // `TimeForFullHeal` (the module data's default) is always done, as in C++.
                self.heal_object(unit, max);
                // `reserveDoorForExit` + `exitObjectViaDoor` (`:113-118`).
                if self.find_object(container).is_some() {
                    self.exit_object_via_door(container, unit);
                }
            } else {
                self.heal_object(unit, max / frames);
            }
        }
        crate::module::UPDATE_SLEEP_NONE
    }

    /// `OpenContain::onCollide` (OpenContain.cpp:850-907): a unit that is trying to enter this
    /// container and touches it is added here (the collision is what lets it walk onto the
    /// container's footprint). Riders of a different controlling player are ordered out first;
    /// the capacity/validity check is repeated before the add.
    pub(crate) fn contain_on_collide(&mut self, container: ObjectId, other: ObjectId) {
        let Some(ct) = self.objects.get(&container).and_then(|o| o.template.contain.as_ref()) else {
            return;
        };
        // `ParachuteContain::onCollide` (`ParachuteContain.cpp:674-687`) replaces `OpenContain::onCollide`,
        // whose own first line is `if (other == nullptr) return;` (`OpenContain.cpp:850-853`): `other ==
        // nullptr` means "collide with ground". A chute still inside a transport (a paradrop from a plane)
        // ignores the contact; otherwise it releases its riders and kills itself, so the chute's own
        // `SlowDeath` runs.
        if other == ObjectId::INVALID {
            if ct.kind != ContainKind::Parachute {
                return;
            }
            if self.objects.get(&container).is_some_and(|o| o.contained_by.is_some()) {
                return;
            }
            // `removeAllContained()` (`OpenContain.cpp:469-481`).
            let riders = self.objects.get(&container).map_or(Vec::new(), |c| c.contained.clone());
            for rider in riders {
                if self.find_object(rider).is_some() {
                    self.remove_from_contain_ex(container, rider, false);
                }
            }
            // `getObject()->kill()` (`Object::kill`, `Object.cpp:2010-2024`).
            self.kill_object(container);
            return;
        }
        // A chute does nothing for any other object: `ParachuteContain::onCollide` has only the `other == nullptr`
        // body (`ParachuteContain.cpp:674-687`), so `OpenContain::onCollide`'s entering logic is not reached.
        if ct.kind == ContainKind::Parachute {
            return;
        }
        // "must be an AI object ... and must be trying to enter our object."
        let trying = self
            .objects
            .get(&other)
            .is_some_and(|o| o.ai_handle.is_some() && o.enter_target == Some(container));
        if !trying {
            return;
        }
        let player = self.objects.get(&other).map(|o| o.player);
        // Eject everyone not controlled by the entering unit's player (the engine ejects them with
        // `aiExit(..., CMD_FROM_AI)` when they have an AI, else `removeFromContain`).
        let contained = self.objects.get(&container).map_or(Vec::new(), |c| c.contained.clone());
        for rider in contained {
            let same = self.objects.get(&rider).is_some_and(|r| Some(r.player) == player);
            if same {
                continue;
            }
            if self.objects.get(&rider).is_some_and(|r| r.ai_handle.is_some()) {
                self.exit_passengers(container, &[rider]);
            } else {
                self.remove_from_contain(container, rider);
            }
        }
        // "finally, we must have space to contain it."
        if self.is_valid_container_for(container, other, true) {
            self.add_to_contain(container, other);
        }
    }

    /// `OpenContain::onDie`: damage the passengers (`DamagePercentToUnits`), then eject survivors.
    /// `TunnelContain::onDie` instead hands the death to the network (no super call).
    pub(crate) fn on_container_destroyed(&mut self, container: ObjectId) {
        if self.is_tunnel_container(container) {
            self.on_tunnel_destroyed(container);
            return;
        }
        self.garrison_points.remove(&container);
        let (ids, percent, max_healths) = match self.objects.get(&container) {
            Some(c) => {
                let pct = c.template.contain.as_ref().map_or(0.0, |ct| ct.damage_percentage_to_units);
                let ids = c.contained.clone();
                let mh: Vec<f32> = ids.iter().map(|id| self.objects.get(id).map_or(0.0, |o| o.template.max_health)).collect();
                (ids, pct, mh)
            }
            None => return,
        };
        for (id, mh) in ids.iter().zip(max_healths) {
            if percent > 0.0 {
                self.damage_object_with_death(*id, DamageInfo { source: ObjectId::INVALID, amount: mh * percent, kind: crate::damage::DamageType::Unresistable }, crate::damage::DeathType::Normal);
            }
        }
        // Eject survivors (`removeAllContained`).
        for id in ids {
            if self.find_object(id).is_some() {
                self.remove_from_contain(container, id);
            } else {
                if let Some(c) = self.objects.get_mut(&container) {
                    c.contained.retain(|&x| x != id);
                }
                self.refresh_contained_mass(container);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::{kind_of, LocomotorKind, LocomotorParams};
    use crate::{ObjectTemplate, TerrainLogic};
    use std::sync::Arc;

    fn transport_template() -> ObjectTemplate {
        let mut t = ObjectTemplate::new("Transport");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 10.0;
        t.max_health = 200.0;
        t.initial_health = 200.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
        t.contain = Some(Arc::new(ContainTemplate {
            kind: ContainKind::Transport,
            capacity: 2,
            allow_inside_kind_of: None,
            forbid_inside_kind_of: 0,
            allow_allies_inside: true,
            allow_enemies_inside: true,
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
            fire_points: GarrisonFirePoints::default(),
            passengers_in_turret: false,
            weapon_bonus_passed_to_passengers: false,
            immune_to_clear_building_attacks: false,
        }));
        t
    }

    fn infantry_template() -> ObjectTemplate {
        let mut t = ObjectTemplate::new("Infantry");
        t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
        t.geometry.major_radius = 4.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.transport_slot_count = 1;
        t
    }

    /// `OpenContain::onCollide` (`OpenContain.cpp:850-907`): a unit whose `AIEnterState` targets the
    /// container is added when the collision pass reports contact. Revert check: without the
    /// `add_to_contain` call the unit stays outside.
    #[test]
    fn a_colliding_enter_target_is_contained() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let c = g.create_object(&Arc::new(transport_template()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let mut it = infantry_template();
        it.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
        let u = g.create_object(&Arc::new(it), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        // The AIEnterState would have set this on its `onEnter`.
        g.objects.get_mut(&u).unwrap().enter_target = Some(c);
        g.contain_on_collide(c, u);
        assert!(g.is_contained(u), "the colliding enter target is contained");
    }

    #[cfg(feature = "possession")]
    #[test]
    fn possession_releases_when_the_unit_is_contained() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let c = g.create_object(&Arc::new(transport_template()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let u = g.create_object(&Arc::new(infantry_template()), Coord3D::new(120.0, 100.0, 0.0), 0.0, 0);
        g.submit(0, crate::Command::Possess { id: u });
        g.update();
        assert!(g.is_possessed(u));
        // Something contains the unit (script / transport); possession must auto-release.
        g.add_to_contain(c, u);
        g.update_possession();
        assert!(!g.is_possessed(u), "a contained unit cannot stay possessed");
    }

    #[test]
    fn capture_ejects_a_transports_riders() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let c = g.create_object(&Arc::new(transport_template()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let u1 = g.create_object(&Arc::new(infantry_template()), Coord3D::new(120.0, 100.0, 0.0), 0.0, 0);
        let u2 = g.create_object(&Arc::new(infantry_template()), Coord3D::new(120.0, 90.0, 0.0), 0.0, 0);
        g.add_to_contain(c, u1);
        g.add_to_contain(c, u2);
        assert_eq!(g.contained_units(c).len(), 2);
        // A capture by a non-ally (`TransportContain::onCapture`) ejects the riders.
        g.on_object_captured(c, 0, 1);
        assert!(g.contained_units(c).is_empty(), "capture ejects riders");
        // An allied capture keeps them (`TheSuperHackers` bugfix).
        let u3 = g.create_object(&Arc::new(infantry_template()), Coord3D::new(120.0, 110.0, 0.0), 0.0, 0);
        g.add_to_contain(c, u3);
        g.set_allied(0, 2);
        g.on_object_captured(c, 0, 2);
        assert_eq!(g.contained_units(c).len(), 1, "an allied capture keeps riders");
    }

    #[test]
    fn passengers_in_turret_ride_the_rotating_fire_point() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let mut t = transport_template();
        let mut c = (*t.contain.clone().unwrap()).clone();
        c.passengers_in_turret = true;
        c.fire_points = GarrisonFirePoints {
            turret_points: vec![Coord3D::new(0.0, 10.0, 5.0)],
            turret_pivot: Some(Coord3D::new(0.0, 0.0, 2.0)),
            ..GarrisonFirePoints::default()
        };
        t.contain = Some(Arc::new(c));
        let bld = g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let u = g.create_object(&Arc::new(infantry_template()), Coord3D::new(220.0, 200.0, 0.0), 0.0, 0);
        g.add_to_contain(bld, u);
        // Turret at the body angle: no relative rotation, so the raw bone offset is used.
        let p = g.passenger_fire_position(bld, u);
        assert!((p.x - 200.0).abs() < 1e-3 && (p.y - 210.0).abs() < 1e-3 && (p.z - 5.0).abs() < 1e-3, "{p:?}");
        // Turret turned 90 degrees: the (0, 10) offset rotates to (-10, 0) about the pivot.
        g.objects.get_mut(&bld).unwrap().combat.turret_angle = std::f32::consts::FRAC_PI_2;
        let p = g.passenger_fire_position(bld, u);
        assert!((p.x - 190.0).abs() < 1e-3 && (p.y - 200.0).abs() < 1e-3, "{p:?}");
    }
}

/// Lane 173 (wave 20): the `AllowToFall` test on a transport's exit is `Object::isAboveTerrain`, R19-9
/// (`contain_l173.rs`).
#[cfg(test)]
#[path = "contain_l173.rs"]
mod l173_tests;

/// Lane 180 (R21-21): a parachute ignores a non-ground contact (`ParachuteContain::onCollide`).
#[cfg(test)]
#[path = "contain_l180.rs"]
mod l180_tests;

