//! `GarrisonContain` fire-point assignment
//! (`GameLogic/Object/Contain/GarrisonContain.cpp`).
//!
//! A garrisoned structure keeps up to [`MAX_GARRISON_POINTS`] fire points loaded from the model's
//! `FIREPOINTnn` bones (resolved by the `zr-game` adapter and carried on the sim template as
//! [`GarrisonFirePoints`](crate::contain::GarrisonFirePoints)). Each firing passenger is moved to
//! the free point closest to its target so its weapon range is measured from that bone, exactly
//! like `findClosestFreeGarrisonPointIndex` / `putObjectAtBestGarrisonPoint`. The per-frame
//! `matchObjectsToGarrisonPoints` pass removes occupants that lost their target, assigns newcomers,
//! and shuffles anyone who has a strictly closer free point (`trackTargets`).
//!
//! The engine caches the world positions once and never moves them (structures are immobile), so
//! the sim transforms the object-local bones by the structure's position and orientation when a
//! point is (re)assigned; the C++ array's entries past the model's bones stay at the centre.
//!
//! Ported from `GarrisonContain.cpp`: `findConditionIndex`, `findClosestFreeGarrisonPointIndex`,
//! `getObjectGarrisonPointIndex`, `putObjectAtGarrisonPoint`, `removeObjectFromGarrisonPoint`,
//! `putObjectAtBestGarrisonPoint`, `removeInvalidObjectsFromGarrisonPoints`,
//! `addValidObjectsToGarrisonPoints`, `trackTargets`, `matchObjectsToGarrisonPoints`.
//!
//! `DISABLED_SUBDUED` (which `GarrisonContain::isPassengerAllowedToFire` checks) has no sim
//! representation: the sim models disabled units only as `Object::disabled_until` (hacked/EMP).

use crate::condition::BodyDamageType;
use crate::body::DamageInfo;
use crate::contain::{
    ContainKind, GARRISON_POINT_DAMAGED, GARRISON_POINT_PRISTINE, GARRISON_POINT_REALLY_DAMAGED, MAX_GARRISON_POINTS,
};
use crate::logic::GameLogic;
use crate::math::{cos, sin, Coord3D};
use crate::object::ObjectId;
use crate::template::kind_of;

/// `GarrisonContain::GARRISON_INDEX_INVALID`.
pub const GARRISON_INDEX_INVALID: usize = usize::MAX;

/// One structure's `m_garrisonPointData` + `m_garrisonPointsInUse` (`GarrisonContain`).
#[derive(Clone, Debug, PartialEq)]
pub struct GarrisonPointState {
    /// `m_garrisonPointData[i].object`: the occupant at fire point `i`, or `None` when the point is
    /// free. Kept indexed by point, like the engine's fixed array.
    pub occupants: [Option<ObjectId>; MAX_GARRISON_POINTS],
    /// `m_garrisonPointsInUse`.
    pub points_in_use: u32,
    /// Non-enclosing garrisons: `m_stationPointList[i].occupantID`, sized to the model's
    /// `STATIONnn` bone count once the container first holds someone (`loadStationGarrisonPoints`).
    /// Empty for enclosing garrisons.
    pub stations: Vec<Option<ObjectId>>,
}

impl Default for GarrisonPointState {
    fn default() -> Self {
        Self { occupants: [None; MAX_GARRISON_POINTS], points_in_use: 0, stations: Vec::new() }
    }
}

impl GarrisonPointState {
    pub(crate) fn crc(&self, c: &mut crate::crc::Crc) {
        c.add_u32(self.points_in_use);
        for occupant in &self.occupants {
            c.add_u32(occupant.map_or(0xffff_ffff, |id| id.0));
        }
        // Only hash the station list when it exists, so enclosing-garrison scenarios keep their
        // previous CRC.
        if !self.stations.is_empty() {
            c.add_u32(self.stations.len() as u32);
            for occupant in &self.stations {
                c.add_u32(occupant.map_or(0xffff_ffff, |id| id.0));
            }
        }
    }
}

impl GameLogic {
    /// Whether `container` is any `GarrisonContain` (enclosing or not).
    pub(crate) fn is_garrison_container(&self, container: ObjectId) -> bool {
        self.objects.get(&container).and_then(|o| o.template.contain.as_ref()).is_some_and(|c| c.kind == ContainKind::Garrison)
    }

    /// `GarrisonContain::isEnclosingContainerFor` (the module's `m_isEnclosingContainer` flag).
    pub(crate) fn is_enclosing_garrison(&self, container: ObjectId) -> bool {
        self.objects
            .get(&container)
            .and_then(|o| o.template.contain.as_ref())
            .is_some_and(|c| c.kind == ContainKind::Garrison && c.is_enclosing_container)
    }

    /// `Thing::convertBonePosToWorldPos`: an object-local point -> world (the structure is
    /// immobile, so this only depends on its position, orientation and scale 1).
    fn object_local_to_world(&self, container: ObjectId, p: Coord3D) -> Coord3D {
        let Some(c) = self.objects.get(&container) else { return Coord3D::ZERO };
        let (s, co) = (sin(c.angle), cos(c.angle));
        Coord3D::new(c.position.x + co * p.x - s * p.y, c.position.y + s * p.x + co * p.y, c.position.z + p.z)
    }

    /// `GarrisonContain::getObjectGarrisonPointIndex` (whether `unit` holds a fire point).
    pub(crate) fn garrison_unit_at_point(&self, container: ObjectId, unit: ObjectId) -> bool {
        self.garrison_points.get(&container).is_some_and(|st| st.occupants.contains(&Some(unit)))
    }

    /// `GarrisonContain::findConditionIndex`: body-damage state -> condition array index.
    fn garrison_condition_index(&self, container: ObjectId) -> usize {
        let Some(o) = self.objects.get(&container) else { return GARRISON_POINT_PRISTINE };
        let state = match &o.body {
            Some(b) => b.damage_state(),
            None => BodyDamageType::Pristine,
        };
        match state {
            BodyDamageType::Pristine => GARRISON_POINT_PRISTINE,
            BodyDamageType::Damaged => GARRISON_POINT_DAMAGED,
            BodyDamageType::ReallyDamaged | BodyDamageType::Rubble => GARRISON_POINT_REALLY_DAMAGED,
        }
    }

    /// World position of fire point `index` for `condition`. Points past the model's `FIREPOINT`
    /// count stay at the structure centre (`loadGarrisonPoints` inits the whole array to the object
    /// position before filling the bones it found).
    fn garrison_point_position(&self, container: ObjectId, condition: usize, index: usize) -> Coord3D {
        let Some(c) = self.objects.get(&container) else { return Coord3D::ZERO };
        let local = c
            .template
            .contain
            .as_ref()
            .and_then(|ct| ct.fire_points.conditions.get(condition))
            .and_then(|points| points.get(index))
            .copied();
        local.map_or(c.position, |p| self.object_local_to_world(container, p))
    }

    /// World position of `unit`'s assigned `STATION` bone for a non-enclosing garrison
    /// (`GarrisonContain::positionObjectsAtStationGarrisonPoints`), or `None` when it has no
    /// station.
    pub(crate) fn station_point_position(&self, container: ObjectId, unit: ObjectId) -> Option<Coord3D> {
        let st = self.garrison_points.get(&container)?;
        let index = st.stations.iter().position(|o| *o == Some(unit))?;
        let local = self
            .objects
            .get(&container)?
            .template
            .contain
            .as_ref()?
            .fire_points
            .station_points
            .get(index)
            .copied()?;
        Some(self.object_local_to_world(container, local))
    }

    /// `GarrisonContain::findClosestFreeGarrisonPointIndex` over an explicit state.
    fn closest_free_point_in(&self, st: &GarrisonPointState, container: ObjectId, condition: usize, target: &Coord3D) -> usize {
        if st.points_in_use as usize >= MAX_GARRISON_POINTS {
            return GARRISON_INDEX_INVALID;
        }
        let mut closest = GARRISON_INDEX_INVALID;
        let mut closest_d2 = -1.0f32;
        for i in 0..MAX_GARRISON_POINTS {
            if st.occupants[i].is_some() {
                continue;
            }
            let p = self.garrison_point_position(container, condition, i);
            let (dx, dy, dz) = (target.x - p.x, target.y - p.y, target.z - p.z);
            let d2 = dx * dx + dy * dy + dz * dz;
            if closest_d2 < 0.0 || d2 < closest_d2 {
                closest_d2 = d2;
                closest = i;
            }
        }
        closest
    }

    /// `GarrisonContain::findClosestFreeGarrisonPointIndex`: the free point closest to `target`
    /// (strictly closer wins, ties keep the lowest index), or `GARRISON_INDEX_INVALID`.
    fn find_closest_free_garrison_point(&self, container: ObjectId, condition: usize, target: &Coord3D) -> usize {
        let default = GarrisonPointState::default();
        let st = self.garrison_points.get(&container).unwrap_or(&default);
        self.closest_free_point_in(st, container, condition, target)
    }

    /// `GarrisonContain::calcBestGarrisonPosition`: the world position of the free fire point
    /// closest to `target`, used as the weapon range check origin before an attack exists
    /// (`WeaponSet::getAbleToUseWeaponAgainstTarget`). `None` for non-garrisons or no free point.
    pub(crate) fn best_garrison_position(&self, container: ObjectId, target: Coord3D) -> Option<Coord3D> {
        if !self.is_enclosing_garrison(container) {
            return None;
        }
        let condition = self.garrison_condition_index(container);
        let index = self.find_closest_free_garrison_point(container, condition, &target);
        (index != GARRISON_INDEX_INVALID).then(|| self.garrison_point_position(container, condition, index))
    }

    /// `GarrisonContain::putObjectAtGarrisonPoint`.
    fn put_object_at_garrison_point(&mut self, container: ObjectId, unit: ObjectId, condition: usize, index: usize) {
        if index >= MAX_GARRISON_POINTS {
            return;
        }
        let pos = self.garrison_point_position(container, condition, index);
        if let Some(o) = self.objects.get_mut(&unit) {
            o.set_position(pos);
        }
        if let Some(st) = self.garrison_points.get_mut(&container) {
            if st.occupants[index].is_none() {
                st.occupants[index] = Some(unit);
                st.points_in_use += 1;
            }
        }
    }

    /// `GarrisonContain::removeObjectFromGarrisonPoint`: free the point and put the passenger back
    /// at the structure centre.
    pub(crate) fn remove_object_from_garrison_point(&mut self, container: ObjectId, unit: ObjectId) {
        // `GarrisonContain::removeObjectFromGarrisonPoint` early-returns for non-enclosing
        // containers (they use station points instead).
        if !self.is_enclosing_garrison(container) {
            return;
        }
        let Some(st) = self.garrison_points.get_mut(&container) else { return };
        let Some(index) = st.occupants.iter().position(|o| *o == Some(unit)) else { return };
        st.occupants[index] = None;
        st.points_in_use = st.points_in_use.saturating_sub(1);
        if let Some(pos) = self.objects.get(&container).map(|c| c.position) {
            if let Some(o) = self.objects.get_mut(&unit) {
                o.set_position(pos);
            }
        }
    }

    /// `GarrisonContain::loadStationGarrisonPoints` reduced to the data the adapter already loaded:
    /// size the per-container station list to the model's `STATIONnn` bones.
    fn ensure_station_points(&mut self, container: ObjectId) -> bool {
        let Some(c) = self.objects.get(&container) else { return false };
        let Some(ct) = c.template.contain.as_ref() else { return false };
        self.garrison_points.entry(container).or_default();
        if let Some(st) = self.garrison_points.get_mut(&container) {
            if st.stations.is_empty() && !ct.fire_points.station_points.is_empty() {
                st.stations = vec![None; ct.fire_points.station_points.len()];
            }
            return !st.stations.is_empty();
        }
        false
    }

    /// `GarrisonContain::pickAStationForMe`: give `unit` the first vacant `STATION` slot.
    fn pick_a_station_for_me(&mut self, container: ObjectId, unit: ObjectId) -> bool {
        let Some(st) = self.garrison_points.get_mut(&container) else { return false };
        if let Some(slot) = st.stations.iter_mut().find(|o| o.is_none()) {
            *slot = Some(unit);
            true
        } else {
            false
        }
    }

    /// `GarrisonContain::removeObjectFromStationPoint`: free `unit`'s station slot.
    pub(crate) fn remove_object_from_station_point(&mut self, container: ObjectId, unit: ObjectId) {
        if let Some(st) = self.garrison_points.get_mut(&container) {
            if let Some(slot) = st.stations.iter_mut().find(|o| **o == Some(unit)) {
                *slot = None;
            }
        }
    }

    /// `GarrisonContain::positionObjectsAtStationGarrisonPoints` for a non-enclosing garrison:
    /// every occupant stays at its pre-assigned `STATION` bone, newcomers take the first vacancy.
    fn position_objects_at_station_points(&mut self, container: ObjectId) {
        if !self.ensure_station_points(container) {
            return;
        }
        // Drop stale assignments (occupants no longer contained) so the slots stay consistent with
        // `onRemoving`, which frees them explicitly.
        let contained: Vec<ObjectId> = self.objects.get(&container).map(|c| c.contained.clone()).unwrap_or_default();
        if let Some(st) = self.garrison_points.get_mut(&container) {
            for slot in st.stations.iter_mut() {
                if slot.is_some_and(|id| !contained.contains(&id)) {
                    *slot = None;
                }
            }
        }
        for unit in contained {
            let already = self.garrison_points.get(&container).is_some_and(|st| st.stations.contains(&Some(unit)));
            if !already && !self.pick_a_station_for_me(container, unit) {
                continue;
            }
            if let Some(pos) = self.station_point_position(container, unit) {
                if let Some(o) = self.objects.get_mut(&unit) {
                    o.set_position(pos);
                }
            }
        }
    }

    /// `GarrisonContain::onBodyDamageStateChange`: a really-damaged garrison that is not
    /// `GARRISONABLE_UNTIL_DESTROYED` orders all occupants out when it crosses the threshold.
    fn eject_from_really_damaged_garrison(&mut self, container: ObjectId) {
        let Some(c) = self.objects.get(&container) else { return };
        let Some(ct) = c.template.contain.as_ref() else { return };
        if !ct.is_garrison() || c.template.kind_of & kind_of::GARRISONABLE_UNTIL_DESTROYED != 0 {
            return;
        }
        let really_damaged = c.body.as_ref().is_some_and(|b| b.damage_state() == BodyDamageType::ReallyDamaged);
        if !really_damaged {
            return;
        }
        // `OpenContain::orderAllPassengersToExit(CMD_FROM_AI, FALSE)` (OpenContain.cpp:1451-1475): only riders with an
        // AI are ordered out (`aiExit`), and they leave through the AI exit state and the garrison's exit.
        let ids: Vec<ObjectId> = c.contained.iter().copied().filter(|id| self.objects.get(id).is_some_and(|o| o.ai_handle.is_some())).collect();
        self.exit_passengers(container, &ids);
    }

    /// The position `unit` is targeting, if it is attacking (`AIUpdateInterface::getCurrentVictim`
    /// / `getCurrentVictimPos`).
    fn garrison_attack_target(&self, unit: ObjectId) -> Option<Coord3D> {
        let o = self.objects.get(&unit)?;
        match o.combat.attack?.target {
            crate::combat::AttackTarget::Object(id) => self.objects.get(&id).map(|t| t.position),
            crate::combat::AttackTarget::Ground(p) => Some(p),
        }
    }

    /// `GarrisonContain::putObjectAtBestGarrisonPoint`: assign an unassigned passenger to the free
    /// point closest to `target`.
    fn put_object_at_best_garrison_point(&mut self, container: ObjectId, unit: ObjectId, target: Coord3D) {
        if self.garrison_points.get(&container).and_then(|st| st.occupants.iter().position(|o| *o == Some(unit))).is_some() {
            return;
        }
        self.garrison_points.entry(container).or_default();
        let condition = self.garrison_condition_index(container);
        let index = self.find_closest_free_garrison_point(container, condition, &target);
        if index != GARRISON_INDEX_INVALID {
            self.put_object_at_garrison_point(container, unit, condition, index);
        }
    }

    /// `GarrisonContain::attemptBestFirePointPosition`: move the passenger to the free point
    /// closest to `target` before the weapon range check (`AIAttackAimAtTargetState::onEnter`).
    /// The sim calls this from the combat pass so a target that only the building's edge can reach
    /// is still fired on.
    pub(crate) fn assign_passenger_fire_point(&mut self, container: ObjectId, unit: ObjectId, target: Coord3D) {
        if self.garrison_points.get(&container).and_then(|st| st.occupants.iter().position(|o| *o == Some(unit))).is_some() {
            self.remove_object_from_garrison_point(container, unit);
        }
        self.put_object_at_best_garrison_point(container, unit, target);
    }

    /// `GarrisonContain::matchObjectsToGarrisonPoints` for every garrison. An enclosing garrison
    /// removes occupants that lost their target, assigns passengers that gained one, and shuffles
    /// occupants to a strictly closer free point (`trackTargets`); a non-enclosing garrison keeps
    /// everyone at their assigned `STATION` bone (`positionObjectsAtStationGarrisonPoints`). A
    /// garrison that becomes really damaged evacuates (`onBodyDamageStateChange`).
    /// `GarrisonContain::update`'s point matching for one garrison container (GarrisonContain.cpp:910-969: the
    /// `matchObjectsToGarrisonPoints` call, before `healObjects`). Run from the container's own heap slot.
    pub(crate) fn match_garrison_point_container(&mut self, container: ObjectId) {
        {
            if !self.is_enclosing_garrison(container) {
                self.position_objects_at_station_points(container);
                self.eject_from_really_damaged_garrison(container);
                let empty = self.objects.get(&container).is_none_or(|c| c.contained.is_empty());
                if empty {
                    self.garrison_points.remove(&container);
                }
                return;
            }
            let condition = self.garrison_condition_index(container);
            // removeInvalidObjectsFromGarrisonPoints: a passenger with no attack leaves its point.
            // (The C++ also drops an object target that left weapon range; the sim's combat pass
            // already ends a contained passenger's attack when it is out of range.)
            let occupants: Vec<ObjectId> =
                self.garrison_points.get(&container).map(|st| st.occupants.iter().flatten().copied().collect()).unwrap_or_default();
            for occupant in occupants {
                if self.objects.get(&occupant).and_then(|o| o.combat.attack).is_none() {
                    self.remove_object_from_garrison_point(container, occupant);
                }
            }
            // addValidObjectsToGarrisonPoints: a passenger with a target takes the closest free
            // point, in contain-list order.
            let contained: Vec<ObjectId> = self.objects.get(&container).map(|c| c.contained.clone()).unwrap_or_default();
            for obj in contained {
                if let Some(target) = self.garrison_attack_target(obj) {
                    self.put_object_at_best_garrison_point(container, obj, target);
                }
            }
            // trackTargets: move to a strictly closer free point.
            let occupants: Vec<ObjectId> =
                self.garrison_points.get(&container).map(|st| st.occupants.iter().flatten().copied().collect()).unwrap_or_default();
            for obj in occupants {
                let Some(target) = self.garrison_attack_target(obj) else { continue };
                let new_index = self.find_closest_free_garrison_point(container, condition, &target);
                if new_index == GARRISON_INDEX_INVALID {
                    continue;
                }
                let new_dist = {
                    let p = self.garrison_point_position(container, condition, new_index);
                    let (dx, dy, dz) = (target.x - p.x, target.y - p.y, target.z - p.z);
                    dx * dx + dy * dy + dz * dz
                };
                let current_dist = self.objects.get(&obj).map(|o| o.position).map_or(f32::INFINITY, |p| {
                    let (dx, dy, dz) = (target.x - p.x, target.y - p.y, target.z - p.z);
                    dx * dx + dy * dy + dz * dz
                });
                if new_dist < current_dist {
                    self.remove_object_from_garrison_point(container, obj);
                    self.put_object_at_garrison_point(container, obj, condition, new_index);
                }
            }
            self.eject_from_really_damaged_garrison(container);
            // Drop the state once an emptied garrison has no occupants, keeping the map (and CRC)
            // small for scenarios without garrisons.
            let empty = self.garrison_points.get(&container).is_some_and(|st| st.points_in_use == 0)
                && self.objects.get(&container).is_none_or(|c| c.contained.is_empty());
            if empty {
                self.garrison_points.remove(&container);
            }
        }
    }

    /// `ContainModuleInterface::isGarrisonable()` + `isImmuneToClearBuildingAttacks()` with occupants
    /// (`ActiveBody::estimateDamage` ActiveBody.cpp:294-302, `ActiveBody::attemptDamage`'s `DAMAGE_KILL_GARRISONED`
    /// case `:456`, `DumbProjectileBehavior.cpp:483`, `MissileAIUpdate.cpp:338`): only a `GarrisonContain` is
    /// garrisonable, and a building whose module sets `ImmuneToClearBuildingAttacks` (the China Bunker, GLA Palace and
    /// America FireBase in the shipped data) is never cleared. `OverlordContain::isGarrisonable` redirects to the first
    /// passenger's contain; the sim maps it to a transport, which is not garrisonable.
    pub(crate) fn is_clearable_garrison(&self, container: ObjectId) -> bool {
        let clearable = self
            .objects
            .get(&container)
            .and_then(|o| o.template.contain.as_ref())
            .is_some_and(|ct| ct.is_garrisonable() && !ct.is_immune_to_clear_building_attacks());
        clearable && !self.contained_units(container).is_empty()
    }

    /// Kill up to `kills` occupants of a garrison building, in contain-list order, skipping the
    /// effectively dead and those outside `isKindOfMulti(required, forbidden)`. The killer scores each
    /// kill (`damager->scoreTheKill(thingToKill)`); the units die with `Object::kill()`
    /// (`DAMAGE_UNRESISTABLE`, `DEATH_NORMAL`, no source). Used by the `DAMAGE_KILL_GARRISONED`
    /// damage case (ActiveBody.cpp:445-478) and the garrison block of the projectile behaviors
    /// (DumbProjectileBehavior.cpp:485-515, MissileAIUpdate.cpp:333-364). Returns the number killed.
    pub(crate) fn kill_garrisoned(&mut self, container: ObjectId, kills: i32, damager: ObjectId, required: u64, forbidden: u64) -> u32 {
        if kills <= 0 || !self.is_clearable_garrison(container) {
            return 0;
        }
        let items = self.contained_units(container).to_vec();
        let mut killed = 0u32;
        for unit in items {
            if killed as i32 >= kills {
                break;
            }
            let Some(o) = self.objects.get(&unit) else { continue };
            if self.is_effectively_dead(unit) || o.template.kind_of & required != required || o.template.kind_of & forbidden != 0 {
                continue;
            }
            // `GarrisonContain::onRemoving` -> `removeObjectFromGarrisonPoint`, which the port's
            // `destroy_object` does not run for a garrisoned unit.
            self.remove_object_from_garrison_point(container, unit);
            if damager != ObjectId::INVALID {
                self.score_the_kill(unit, damager);
            }
            // `Object::kill()` (`Object.cpp:2010-2024`): unresistable max-health damage with no source, so the
            // damage pipeline records neither the loss nor a kill of its own.
            self.damage_object_with_death(
                unit,
                DamageInfo { source: ObjectId::INVALID, amount: crate::damage::HUGE_DAMAGE_AMOUNT, kind: crate::damage::DamageType::Unresistable },
                crate::damage::DeathType::Normal,
            );
            killed += 1;
        }
        killed
    }
}
