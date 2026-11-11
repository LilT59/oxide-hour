//! The `ActionManager` entry queries and the `CommandButtonHuntUpdate` enter hunt.
//!
//! These are the checks that let a player order a unit to walk up to another object and enter
//! it: container boarding, the GLA Hijacker taking an enemy vehicle, the GLA Terrorist converting
//! a vehicle to a car bomb, the GLA Saboteur disabling an enemy building, and the capture-building
//! special ability. They are pure queries so the client can map a right-click on a target to the
//! retail action, exactly as `CommandXlat::evaluateContextCommand` does.
//!
//! Ported from:
//!
//! * `ActionManager::canEnterObject` (`Common/RTS/ActionManager.cpp:533`)
//! * `ActionManager::canConvertObjectToCarBomb` (`Common/RTS/ActionManager.cpp:825`)
//! * `ActionManager::canHijackVehicle` (`Common/RTS/ActionManager.cpp:860`)
//! * `ActionManager::canSabotageBuilding` (`Common/RTS/ActionManager.cpp:922`)
//! * `ActionManager::canCaptureBuilding` (`Common/RTS/ActionManager.cpp:1002`)
//! * `ActionManager::canDoSpecialPowerAtObject` / `canDoSpecialPowerAtLocation` / `canDoSpecialPower`
//!   (`ActionManager.cpp:1623`, `:1474`, `:1885`) with their hack, defector and charge helpers
//!   (`canDisableVehicleViaHacking` :1107, `canStealCashViaHacking` :1231, `canMakeObjectDefector` :967)
//! * `CommandButtonHuntUpdate::huntEnter` / `scanClosestTarget` (`CommandButtonHuntUpdate.cpp:225`)
//! * `CommandXlat::evaluateContextCommand`'s enter ordering (`Core/.../CommandXlat.cpp:2457`)
//!
//! The queries do not apply `isObjectShroudedForAction` (`ActionManager.cpp:96`): the sim treats an
//! explicit order as authoritative (the same policy as explicit attack commands), and the client
//! only offers the action for objects it can see. `KINDOF_IGNORED_IN_GUI` / `KINDOF_MOB_NEXUS` /
//! `KINDOF_REJECT_UNMANNED` and stealth/disguise are not modelled by the sim, so the corresponding
//! branches are skipped (they only matter for cargo planes / angry mobs / the rare
//! `RejectUnmanned` infantry and for stealthed occupants, none of which the shipped skirmish maps
//! rely on for these abilities).

use crate::collide::EnterCollide;
use crate::damage::Relationship;
use crate::disabled::DisabledType;
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::{status, Object, ObjectId};
use crate::shroud::CellShroudStatus;
use crate::template::{collide_kind, kind_of};

/// Whether `o` has a power module or special ability for the `SpecialPowerTemplate` `power`:
/// `Object::getSpecialPowerModule` is non-null (`ActionManager.cpp:1641`), for either the
/// `SpecialPowerModule` or the `SpecialAbilityUpdate` that names the power.
pub(crate) fn has_power_module(o: &Object, power: &str) -> bool {
    o.template.special_powers.iter().any(|m| m.power == power) || o.template.special_abilities.iter().any(|a| a.power == power)
}

/// `KINDOF_AIRCRAFT || isAirborneTarget()` (`ActionManager.cpp:1146`). `isAirborneTarget` is the
/// `OBJECT_STATUS_AIRBORNE_TARGET` bit (Object.h:211), which `doLocomotor` sets (AIUpdate.cpp:2294-2300).
fn is_airborne_or_aircraft(o: &Object) -> bool {
    o.is_kind_of(kind_of::AIRCRAFT) || o.status_bits & status::AIRBORNE_TARGET != 0
}

/// `KINDOF_CAPTURABLE` and not `KINDOF_REBUILD_HOLE`: the "hackable" test the engine writes as
/// "temp: using capturable" (`ActionManager.cpp:1698-1699`, `:1721-1722`, `:1291-1292`).
fn hackable_structure(o: &Object) -> bool {
    o.is_kind_of(kind_of::CAPTURABLE) && !o.is_kind_of(kind_of::REBUILD_HOLE)
}

/// The stealth test of the hack queries (`ActionManager.cpp:1168-1173`, `:1297-1302`): a stealthed
/// object that is neither detected nor disguised is not a target. `appearsToContainFriendlies` is not
/// modelled (see [`GameLogic::can_capture_building`]).
fn hidden_by_stealth(bits: u64) -> bool {
    bits & status::STEALTHED != 0 && bits & status::DETECTED == 0 && bits & status::DISGUISED == 0
}

/// The three `CommandButtonHuntUpdate` enter hunts (`GUICOMMANDMODE_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnterKind {
    /// `GUICOMMANDMODE_HIJACK_VEHICLE`.
    HijackVehicle,
    /// `GUICOMMANDMODE_CONVERT_TO_CARBOMB`.
    ConvertToCarBomb,
    /// `GUICOMMANDMODE_SABOTAGE_BUILDING`.
    SabotageBuilding,
}

/// The enter-type action the client should issue for a right-click on a target, in retail's
/// `CommandXlat::evaluateContextCommand` order (`CommandXlat.cpp:2457-2496`).
///
/// The order is: hijack, convert-to-carbomb, sabotage, enter-container, then — after the caller has
/// tried the ordinary attack check — capture-building. `CaptureBuilding` is issued as
/// `Command::DoSpecialPower`, not `Command::Enter`; the rest are all `Command::Enter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnterAction {
    HijackVehicle,
    ConvertToCarBomb,
    SabotageBuilding,
    EnterContainer,
    CaptureBuilding,
}

impl GameLogic {
    /// The `SpecialPowerTemplate` name of `unit`'s capture ability (`SPECIAL_INFANTRY_CAPTURE_BUILDING`
    /// or `SPECIAL_BLACKLOTUS_CAPTURE_BUILDING`). The engine tests the `SpecialPowerType` enum; the
    /// sim keeps the enum on the `SpecialPowerTemplate` (`SimSpecialPowerTemplate::kind`) and the
    /// ability module names the template, so resolve through the store.
    pub fn capture_power_for(&self, unit: ObjectId) -> Option<String> {
        let o = self.find_object(unit)?;
        o.template
            .special_abilities
            .iter()
            .map(|a| a.power.as_str())
            .chain(o.template.special_powers.iter().map(|m| m.power.as_str()))
            .find(|name| {
                self.special_powers.get(*name).is_some_and(|t| {
                    t.kind == "SPECIAL_INFANTRY_CAPTURE_BUILDING" || t.kind == "SPECIAL_BLACKLOTUS_CAPTURE_BUILDING"
                })
            })
            .map(str::to_string)
    }

    /// `ActionManager::canEnterObject` (`ActionManager.cpp:533`): can `unit` enter `target`?
    /// `check_capacity` is `CHECK_CAPACITY` vs `DONT_CHECK_CAPACITY`. The collide early-out
    /// (`obj->getBehaviorModules()` -> `wouldLikeToCollideWith`) covers the enter-type crates
    /// (terrorist / hijacker / saboteur) whose targets have no contain module.
    pub fn can_enter_object(&self, unit: ObjectId, target: ObjectId, check_capacity: bool) -> bool {
        let (Some(u), Some(t)) = (self.find_object(unit), self.find_object(target)) else { return false };
        if unit == target {
            // "You can't contain yourself (crash fix for pow truck reselection)."
            return false;
        }
        if t.is_dying() {
            return false;
        }
        if u.under_construction || t.under_construction {
            return false;
        }
        if u.is_kind_of(kind_of::STRUCTURE) || u.is_kind_of(kind_of::IMMOBILE) {
            // Structures or immobiles can't garrison.
            return false;
        }
        if t.is_disabled_by_type(DisabledType::Subdued) {
            // A microwave tank has soldered the doors shut.
            return false;
        }
        // Unmanned-vehicle special case: any infantry unit may take over an unmanned vehicle.
        if u.is_kind_of(kind_of::INFANTRY) && t.is_disabled_by_type(DisabledType::Unmanned) {
            return true;
        }
        // `if (collide->wouldLikeToCollideWith(objectToEnter)) return TRUE;` — the enter-type
        // crates (and the pilot's `PilotFindVehicleUpdate`, not modelled).
        if self.would_like_to_enter_collide(unit, target, EnterCollide::HijackedVehicle)
            || self.would_like_to_enter_collide(unit, target, EnterCollide::CarBomb)
            || self.would_like_to_enter_collide(unit, target, EnterCollide::SabotageBuilding)
        {
            return true;
        }
        // `contain->isHealContain()` (`ActionManager.cpp:660-667`): "This container is only used for
        // the purposes of healing and we cannot enter it with full health. This is not a normal
        // container." The engine compares exactly (`body->getHealth() == body->getMaxHealth()`); the
        // port's bodies clamp every heal and their initial health to max, so the equality holds.
        if let Some(c) = t.template.contain.as_ref() {
            if c.kind == crate::contain::ContainKind::Heal {
                if let Some(body) = u.body.as_ref() {
                    if body.health() == body.max_health() {
                        return false;
                    }
                }
            }
        }
        // Otherwise the target must be a valid container (`OpenContain::isValidContainerFor`).
        self.is_valid_container_for(target, unit, check_capacity)
    }

    /// `ActionManager::canHijackVehicle` (`ActionManager.cpp:860`): the GLA Hijacker.
    pub fn can_hijack_vehicle(&self, unit: ObjectId, target: ObjectId) -> bool {
        let (Some(u), Some(t)) = (self.find_object(unit), self.find_object(target)) else { return false };
        if t.is_dying() {
            return false;
        }
        if self.relationship(u.player, t.player) != Relationship::Enemies {
            return false;
        }
        // Must be a vehicle, never an aircraft, never a drone.
        if !t.is_kind_of(kind_of::VEHICLE) || t.is_kind_of(kind_of::AIRCRAFT) || t.is_kind_of(kind_of::DRONE) {
            return false;
        }
        self.would_like_to_enter_collide(unit, target, EnterCollide::HijackedVehicle)
    }

    /// `ActionManager::canConvertObjectToCarBomb` (`ActionManager.cpp:825`): the GLA Terrorist.
    pub fn can_convert_object_to_carbomb(&self, unit: ObjectId, target: ObjectId) -> bool {
        let (Some(_u), Some(t)) = (self.find_object(unit), self.find_object(target)) else { return false };
        if t.is_dying() {
            return false;
        }
        // No relationship test in the engine: `ConvertToCarBombCrateCollide` is a neutral-only hunt
        // but can be issued on any matching vehicle the player right-clicks.
        self.would_like_to_enter_collide(unit, target, EnterCollide::CarBomb)
    }

    /// `ActionManager::canSabotageBuilding` (`ActionManager.cpp:922`): the GLA Saboteur.
    pub fn can_sabotage_building(&self, unit: ObjectId, target: ObjectId) -> bool {
        let (Some(u), Some(t)) = (self.find_object(unit), self.find_object(target)) else { return false };
        if t.is_dying() {
            return false;
        }
        if self.relationship(u.player, t.player) != Relationship::Enemies {
            return false;
        }
        self.would_like_to_enter_collide(unit, target, EnterCollide::SabotageBuilding)
    }

    /// `ActionManager::canCaptureBuilding` (`ActionManager.cpp:1002`): Black Lotus / Ranger / Red
    /// Guard (whichever carries `SPECIAL_INFANTRY_CAPTURE_BUILDING` or the Black Lotus capture).
    /// Capture is issued as a special power, not an enter command.
    pub fn can_capture_building(&self, unit: ObjectId, target: ObjectId) -> bool {
        let (Some(u), Some(t)) = (self.find_object(unit), self.find_object(target)) else { return false };
        let Some(power) = self.capture_power_for(unit) else { return false };
        if t.template.is_collide_kind_of(collide_kind::IMMUNE_TO_CAPTURE) {
            return false;
        }
        if !self.special_power_is_ready(unit, &power) {
            // `spInterface->getPercentReady() < 1.0f`: a paused module (`StartsPaused`, before its
            // upgrade) is never fully ready (`pauseCountdown`, SpecialPowerModule.cpp:653-667).
            return false;
        }
        if t.is_dying() {
            return false;
        }
        // `objectToCapture->testStatus(OBJECT_STATUS_UNDER_CONSTRUCTION) || testStatus(OBJECT_STATUS_SOLD)`
        // (ActionManager.cpp:1059-1061).
        let bits = t.status_mask(self.frame);
        if !t.is_kind_of(kind_of::STRUCTURE) || t.under_construction || bits & status::SOLD != 0 {
            return false;
        }
        let r = self.relationship(u.player, t.player);
        // Always capturable when enemy; otherwise only `KINDOF_CAPTURABLE` non-allies.
        if !(r == Relationship::Enemies || (t.is_kind_of(kind_of::CAPTURABLE) && r != Relationship::Allies)) {
            return false;
        }
        // A stealthed enemy that is neither detected nor disguised cannot be captured
        // (ActionManager.cpp:1077-1081).
        if bits & status::STEALTHED != 0 && bits & status::DETECTED == 0 && bits & status::DISGUISED == 0 {
            return false;
        }
        // Not capturable while garrisoned (unless only stealth-garrisoned, which the sim does not
        // model, so any occupant blocks).
        if t.template.contain.as_ref().is_some_and(|ct| ct.is_garrison()) && !t.contained.is_empty() {
            return false;
        }
        // `appearsToContainFriendlies` and the stealthed/disguised test need stealth, not modelled.
        true
    }

    /// The client's right-click resolver: the first enter-type action `unit` can perform on
    /// `target`, in retail's `CommandXlat::evaluateContextCommand` order (`CommandXlat.cpp:2457`).
    ///
    /// The caller must try its normal attack check between `EnterContainer` and
    /// `CaptureBuilding`, exactly as the engine does (`CommandXlat.cpp:2485-2493`).
    pub fn enter_action_for(&self, unit: ObjectId, target: ObjectId) -> Option<EnterAction> {
        if self.can_hijack_vehicle(unit, target) {
            return Some(EnterAction::HijackVehicle);
        }
        if self.can_convert_object_to_carbomb(unit, target) {
            return Some(EnterAction::ConvertToCarBomb);
        }
        if self.can_sabotage_building(unit, target) {
            return Some(EnterAction::SabotageBuilding);
        }
        if self.can_enter_object(unit, target, false) {
            return Some(EnterAction::EnterContainer);
        }
        if self.can_capture_building(unit, target) {
            return Some(EnterAction::CaptureBuilding);
        }
        None
    }

    /// The predicate behind one [`EnterKind`], for the `CommandButtonHuntUpdate` scan.
    pub(crate) fn can_enter_kind(&self, unit: ObjectId, target: ObjectId, kind: EnterKind) -> bool {
        match kind {
            EnterKind::HijackVehicle => self.can_hijack_vehicle(unit, target),
            EnterKind::ConvertToCarBomb => self.can_convert_object_to_carbomb(unit, target),
            EnterKind::SabotageBuilding => self.can_sabotage_building(unit, target),
        }
    }

    /// `ActionManager::canDoSpecialPowerAtObject` (`ActionManager.cpp:1623`) with
    /// `checkSourceRequirements` false, as `CommandButton::isValidToUseOn` calls it for an object target
    /// (`ControlBar.cpp:681`) and as a script's fire sees it (`CMD_FROM_SCRIPT`). The readiness test
    /// (`getPercentReady`) and `hasSpecialPower` are skipped in this form; the power module must still
    /// exist (`:1641`). `isObjectShroudedForAction` is skipped for scripts (`ActionManager.cpp:113`), so
    /// there is no shroud test here.
    pub fn can_do_special_power_at_object(&self, source: ObjectId, target: ObjectId, power: &str) -> bool {
        let (Some(src), Some(tgt)) = (self.find_object(source), self.find_object(target)) else { return false };
        // `target->isEffectivelyDead()` (:1634).
        if tgt.destroyed || tgt.is_dying() {
            return false;
        }
        if !has_power_module(src, power) {
            return false;
        }
        let Some(kind) = self.special_powers.get(power).map(|t| t.kind.as_str()) else { return false };
        let r = self.relationship(src.player, tgt.player);
        let structure = tgt.is_kind_of(kind_of::STRUCTURE);
        let vehicle = tgt.is_kind_of(kind_of::VEHICLE);
        match kind {
            // `SPECIAL_BATTLESHIP_BOMBARDMENT`: anything but an ally (:1662).
            "SPECIAL_BATTLESHIP_BOMBARDMENT" => r != Relationship::Allies,
            // `SPECIAL_TANKHUNTER_TNT_ATTACK`: a structure or a non-aircraft vehicle (:1670).
            "SPECIAL_TANKHUNTER_TNT_ATTACK" => structure || (vehicle && !tgt.is_kind_of(kind_of::AIRCRAFT)),
            // `SPECIAL_BOOBY_TRAP`: a neutral or allied structure (:1679).
            "SPECIAL_BOOBY_TRAP" => structure && matches!(r, Relationship::Neutral | Relationship::Allies),
            // `SPECIAL_MISSILE_DEFENDER_LASER_GUIDED_MISSILES`: an enemy vehicle (:1688).
            "SPECIAL_MISSILE_DEFENDER_LASER_GUIDED_MISSILES" => vehicle && r == Relationship::Enemies,
            // `SPECIAL_HACKER_DISABLE_BUILDING`: an enemy hackable structure (:1696).
            "SPECIAL_HACKER_DISABLE_BUILDING" => structure && r == Relationship::Enemies && hackable_structure(tgt),
            // `SPECIAL_INFANTRY_CAPTURE_BUILDING` / `SPECIAL_BLACKLOTUS_CAPTURE_BUILDING` (:1707).
            "SPECIAL_INFANTRY_CAPTURE_BUILDING" | "SPECIAL_BLACKLOTUS_CAPTURE_BUILDING" => self.can_capture_building(source, target),
            // `canDisableVehicleViaHacking(obj, target, commandSource, false)` (:1712).
            "SPECIAL_BLACKLOTUS_DISABLE_VEHICLE_HACK" => self.can_disable_vehicle_via_hacking(source, target),
            // `canStealCashViaHacking` (:1715).
            "SPECIAL_BLACKLOTUS_STEAL_CASH_HACK" => self.can_steal_cash_via_hacking(source, target, power),
            // `SPECIAL_CASH_HACK` (:1717): an enemy cash generator, not under construction.
            "SPECIAL_CASH_HACK" => {
                structure && r == Relationship::Enemies && hackable_structure(tgt) && !tgt.under_construction && tgt.is_kind_of(kind_of::CASH_GENERATOR)
            }
            // `SPECIAL_DISGUISE_AS_VEHICLE` (:1740): a vehicle that is not an aircraft, a boat or a cliff
            // jumper, and not a `RailroadBehavior` train. The sim keeps no `KINDOF_CLIFF_JUMPER` bit and no
            // template flag for `RailroadBehavior`, so those two exclusions are not applied.
            "SPECIAL_DISGUISE_AS_VEHICLE" => vehicle && !tgt.is_kind_of(kind_of::AIRCRAFT) && !tgt.template.is_collide_kind_of(collide_kind::BOAT),
            // `SPECIAL_DEFECTOR` (:1761): a non-structure enemy (`canMakeObjectDefector`).
            "SPECIAL_DEFECTOR" => !structure && self.can_make_object_defector(source, target),
            // `SPECIAL_REMOTE_CHARGES` / `SPECIAL_TIMED_CHARGES` / `SPECIAL_HELIX_NAPALM_BOMB` (:1828).
            "SPECIAL_REMOTE_CHARGES" | "SPECIAL_TIMED_CHARGES" | "SPECIAL_HELIX_NAPALM_BOMB" => self.charge_target_allowed(source, target, power),
            // The location-only powers (:1781-1826), `SPECIAL_CASH_BOUNTY` (:1659) and the powers the
            // engine never lists here: no object target.
            _ => false,
        }
    }

    /// The charge branch of `canDoSpecialPowerAtObject` (`ActionManager.cpp:1828-1877`): the target is a
    /// structure or vehicle that is not a bridge, and the unit's `SpecialAbilityUpdate` for `power` has a
    /// free charge slot (`getSpecialObjectCount() < getSpecialObjectMax()`, :1843). The producer tests
    /// (`findSpecialObjectWithProducerID`, :1847 and :1867, including the other charge kind) are not
    /// modelled: the sim does not record the target a charge was planted on, so they never reject.
    fn charge_target_allowed(&self, source: ObjectId, target: ObjectId, power: &str) -> bool {
        let (Some(src), Some(tgt)) = (self.find_object(source), self.find_object(target)) else { return false };
        if tgt.is_kind_of(kind_of::BRIDGE) || tgt.is_kind_of(kind_of::BRIDGE_TOWER) {
            return false;
        }
        if !(tgt.is_kind_of(kind_of::STRUCTURE) || tgt.is_kind_of(kind_of::VEHICLE)) {
            return false;
        }
        // `obj->findSpecialAbilityUpdate(type)`: the update module, not the `SpecialAbility` power module.
        let Some(update) = src.template.special_abilities.iter().find(|a| a.power == power && !a.power_module) else {
            return false;
        };
        let placed = src.ability.as_ref().filter(|a| a.power == power).map_or(0, |a| a.special_objects.len());
        (placed as u32) < update.max_special_objects
    }

    /// `ActionManager::canDisableVehicleViaHacking` (`ActionManager.cpp:1107`) with `checkSourceRequirements`
    /// false, the only form `canDoSpecialPowerAtObject` uses (:1712). The source's power and readiness are
    /// not tested in this form. `appearsToContainFriendlies` is not modelled.
    pub fn can_disable_vehicle_via_hacking(&self, unit: ObjectId, target: ObjectId) -> bool {
        let (Some(u), Some(t)) = (self.find_object(unit), self.find_object(target)) else { return false };
        // `objectToHack->isEffectivelyDead()` (:1141).
        if t.destroyed || t.is_dying() {
            return false;
        }
        // `isKindOf(KINDOF_AIRCRAFT) || isAirborneTarget()` (:1146).
        if is_airborne_or_aircraft(t) {
            return false;
        }
        // The shroud test is skipped for scripts (:1152, `isObjectShroudedForAction`).
        if self.relationship(u.player, t.player) != Relationship::Enemies {
            return false;
        }
        // "Make sure we are targeting a building!" is a vehicle here (:1162).
        if !t.is_kind_of(kind_of::VEHICLE) {
            return false;
        }
        !hidden_by_stealth(t.status_mask(self.frame))
    }

    /// `ActionManager::canStealCashViaHacking` (`ActionManager.cpp:1231`). The source must carry the power
    /// and its module must be fully ready, whatever the caller passed (:1238, :1252-1257): the readiness
    /// test is `getPercentReady() < 1.0f`, which `special_power_is_ready` answers.
    pub fn can_steal_cash_via_hacking(&self, unit: ObjectId, target: ObjectId, power: &str) -> bool {
        let (Some(u), Some(t)) = (self.find_object(unit), self.find_object(target)) else { return false };
        if !has_power_module(u, power) || !self.special_power_is_ready(unit, power) {
            return false;
        }
        // `isEffectivelyDead()` (:1259) and `OBJECT_STATUS_UNDER_CONSTRUCTION` (:1264).
        if t.destroyed || t.is_dying() || t.under_construction {
            return false;
        }
        if self.relationship(u.player, t.player) != Relationship::Enemies {
            return false;
        }
        // "Make sure we are targeting something that contains cash!" (:1280), then the hackable test (:1292).
        if !t.is_kind_of(kind_of::CASH_GENERATOR) || !hackable_structure(t) {
            return false;
        }
        !hidden_by_stealth(t.status_mask(self.frame))
    }

    /// `ActionManager::canMakeObjectDefector` (`ActionManager.cpp:967`): an enemy that is alive. The shroud
    /// test is skipped for scripts (:990, `isObjectShroudedForAction`).
    pub fn can_make_object_defector(&self, unit: ObjectId, target: ObjectId) -> bool {
        let (Some(u), Some(t)) = (self.find_object(unit), self.find_object(target)) else { return false };
        self.relationship(u.player, t.player) == Relationship::Enemies && !(t.destroyed || t.is_dying())
    }

    /// `ActionManager::canDoSpecialPowerAtLocation` (`ActionManager.cpp:1474`) with `checkSourceRequirements`
    /// false. An underwater drop point refuses the paradrops and the crate drop (:1505). The damaging
    /// powers refuse a point the player has never seen (`CELLSHROUD_SHROUDED`, :1558 and :1597 for the
    /// sneak attack, `RETAIL_COMPATIBLE_CRC` 1). The map-only powers need a point on the map (:1566). The
    /// Baikonur launch is always allowed (:1568). The object-target powers and the no-target powers refuse
    /// (:1571-1587, and the `default` of the switch).
    pub fn can_do_special_power_at_location(&self, source: ObjectId, loc: &Coord3D, power: &str) -> bool {
        let Some(src) = self.find_object(source) else { return false };
        if !has_power_module(src, power) {
            return false;
        }
        let Some(kind) = self.special_powers.get(power).map(|t| t.kind.as_str()) else { return false };
        if matches!(kind, "SPECIAL_PARADROP_AMERICA" | "INFA_SPECIAL_PARADROP_AMERICA" | "SPECIAL_TANK_PARADROP" | "SPECIAL_CRATE_DROP")
            && self.terrain().is_underwater(loc.x, loc.y)
        {
            return false;
        }
        let shrouded = || self.shroud().cell_status_at(src.player, loc) == CellShroudStatus::Shrouded;
        match kind {
            // `CELLSHROUD_SHROUDED` refused; "fogged are okay" (:1557-1558).
            "SPECIAL_DAISY_CUTTER"
            | "AIRF_SPECIAL_DAISY_CUTTER"
            | "SPECIAL_PARADROP_AMERICA"
            | "SPECIAL_TANK_PARADROP"
            | "INFA_SPECIAL_PARADROP_AMERICA"
            | "SPECIAL_CARPET_BOMB"
            | "SPECIAL_CHINA_CARPET_BOMB"
            | "SPECIAL_LEAFLET_DROP"
            | "EARLY_SPECIAL_LEAFLET_DROP"
            | "EARLY_SPECIAL_CHINA_CARPET_BOMB"
            | "AIRF_SPECIAL_CARPET_BOMB"
            | "SUPR_SPECIAL_CRUISE_MISSILE"
            | "SPECIAL_CLUSTER_MINES"
            | "NUKE_SPECIAL_CLUSTER_MINES"
            | "SPECIAL_EMP_PULSE"
            | "SPECIAL_CRATE_DROP"
            | "SPECIAL_NAPALM_STRIKE"
            | "SPECIAL_BLACK_MARKET_NUKE"
            | "SPECIAL_ANTHRAX_BOMB"
            | "SPECIAL_TERROR_CELL"
            | "SPECIAL_AMBUSH"
            | "SPECIAL_NEUTRON_MISSILE"
            | "NUKE_SPECIAL_NEUTRON_MISSILE"
            | "SUPW_SPECIAL_NEUTRON_MISSILE"
            | "SPECIAL_SCUD_STORM"
            | "SPECIAL_A10_THUNDERBOLT_STRIKE"
            | "AIRF_SPECIAL_A10_THUNDERBOLT_STRIKE"
            | "SPECIAL_SPECTRE_GUNSHIP"
            | "AIRF_SPECIAL_SPECTRE_GUNSHIP"
            | "SPECIAL_REPAIR_VEHICLES"
            | "EARLY_SPECIAL_REPAIR_VEHICLES"
            | "SPECIAL_GPS_SCRAMBLER"
            | "SLTH_SPECIAL_GPS_SCRAMBLER"
            | "SPECIAL_ARTILLERY_BARRAGE"
            | "SPECIAL_FRENZY"
            | "EARLY_SPECIAL_FRENZY"
            | "SPECIAL_PARTICLE_UPLINK_CANNON"
            | "SUPW_SPECIAL_PARTICLE_UPLINK_CANNON"
            | "LAZR_SPECIAL_PARTICLE_UPLINK_CANNON"
            | "SPECIAL_CLEANUP_AREA"
            | "SPECIAL_BATTLESHIP_BOMBARDMENT" => !shrouded(),
            // `isPointOnMap` (:1566): "These specials can be used anywhere" on the map.
            "SPECIAL_SPY_SATELLITE" | "SPECIAL_RADAR_VAN_SCAN" | "SPECIAL_SPY_DRONE" | "SPECIAL_HELIX_NAPALM_BOMB" => self.is_point_on_map(loc),
            // `SPECIAL_LAUNCH_BAIKONUR_ROCKET` (:1567).
            "SPECIAL_LAUNCH_BAIKONUR_ROCKET" => true,
            // `SPECIAL_SNEAK_ATTACK` (:1594, `RETAIL_COMPATIBLE_CRC` branch): the cell must not be shrouded.
            "SPECIAL_SNEAK_ATTACK" => !shrouded(),
            _ => false,
        }
    }

    /// `isPointOnMap` (`ActionManager.cpp:1463`): `TheTerrainLogic->getExtent` contains the point, with
    /// `isInRegionNoZ` (x and y only, both ends inclusive).
    fn is_point_on_map(&self, loc: &Coord3D) -> bool {
        let (lo, hi) = self.terrain().world_extent();
        loc.x >= lo.x && loc.x <= hi.x && loc.y >= lo.y && loc.y <= hi.y
    }

    /// `ActionManager::canDoSpecialPower` (`ActionManager.cpp:1885`) with no target and
    /// `checkSourceRequirements` false: the power module must exist, and only the powers the engine
    /// returns `TRUE` for pass (`:1961-1968`). Every other power needs a target.
    pub fn can_do_special_power_no_target(&self, source: ObjectId, power: &str) -> bool {
        if !self.find_object(source).is_some_and(|o| has_power_module(o, power)) {
            return false;
        }
        matches!(
            self.special_powers.get(power).map(|t| t.kind.as_str()),
            Some(
                "SPECIAL_REMOTE_CHARGES"
                    | "SPECIAL_CIA_INTELLIGENCE"
                    | "SPECIAL_COMMUNICATIONS_DOWNLOAD"
                    | "SPECIAL_DETONATE_DIRTY_NUKE"
                    | "SPECIAL_CHANGE_BATTLE_PLANS"
                    | "SPECIAL_LAUNCH_BAIKONUR_ROCKET"
            )
        )
    }
}

#[cfg(test)]
mod airborne_tests {
    use super::*;
    use crate::logic::GameLogic;
    use crate::math::Coord3D;
    use crate::{ObjectTemplate, TerrainLogic};
    use std::sync::Arc;

    /// `ActionManager::canHijackVehicle`'s `isKindOf(KINDOF_AIRCRAFT) || isAirborneTarget()` (ActionManager.cpp:1146): a
    /// ground vehicle is airborne when it has the `OBJECT_STATUS_AIRBORNE_TARGET` bit, and not without it.
    #[test]
    fn the_airborne_test_reads_the_status_bit() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let id = g.create_object(&Arc::new(ObjectTemplate::new("Plain")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        assert!(!is_airborne_or_aircraft(g.find_object(id).unwrap()));
        g.objects.get_mut(&id).unwrap().status_bits |= status::AIRBORNE_TARGET;
        assert!(is_airborne_or_aircraft(g.find_object(id).unwrap()));
    }
}
