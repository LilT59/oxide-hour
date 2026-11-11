//! Line of sight for target acquisition: `PartitionFilterLineOfSight` (`PartitionManager.cpp:5426-5437`),
//! `PartitionManager::isClearLineOfSightTerrain` (`:4582-4650`), `TerrainLogic::isClearLineOfSight`
//! (`BaseHeightMap.cpp:1020-1150`) and `Pathfinder::isViewBlockedByObstacle` /
//! `isAttackViewBlockedByObstacle` (`AIPathfind.cpp:9377-9480`).
//!
//! `AI::findClosestEnemy` adds the filter when the scan's qualifiers carry `AI::CAN_SEE`, which
//! `AIUpdateInterface::getNextMoodTarget` sets for a unit with `KINDOF_ATTACK_NEEDS_LINE_OF_SIGHT` when
//! `AIData`'s `AttackUsesLineOfSight` is on (`AIUpdate.cpp:4640-4645`, the default is on, `AI.cpp:941`).
//! The port had no LOS test: AH7 s1 f2816 showed Ranger 241 acquiring rebel 240 *through* the Tech Oil
//! Derrick it was capturing.
//!
//! Read-only. Documented deviations: the port's pathfinder has one ground grid (a bridge deck is the same
//! cell with a bridge index), so the C++'s `PathfindLayerEnum` argument is always the ground layer; and
//! `Object::m_layer` is not ported, so an attacker on a bridge or rooftop does not get the `skipCount = 3`
//! of `AIPathfind.cpp:9522-9524` (R21-4, open). `KINDOF_CAN_SEE_THROUGH_STRUCTURE` is carried in the second
//! KindOf word (`template::collide_kind`), because the 64-bit set is full.
//!
//! One implementation of `isAttackViewBlockedByObstacle` (lane 180): [`GameLogic::attack_view`] reads the
//! object-side inputs into an [`AttackView`], and [`Pathfinder::attack_view_blocked`] runs the terrain test and
//! the obstacle march. `findAttackPath` (`AIPathfind.cpp:10750`, `:10869`) and the LOS filter both use it.

use crate::logic::GameLogic;
use crate::math::{real_to_int_floor, Coord3D, MAP_HEIGHT_SCALE, MAP_XY_FACTOR};
use crate::object::ObjectId;
use crate::pathfind::{CellType, Pathfinder};
use crate::terrain::TerrainLogic;

impl TerrainLogic {
    /// `BaseHeightMapRenderObjClass::isClearLineOfSight` (`BaseHeightMap.cpp:1020-1150`): a Bresenham
    /// march over the height-map samples from `pos` to `pos_other`, both at eye height. A sample whose
    /// height (the max of its four corners) rises more than `LOS_FUDGE` above the line blocks the view;
    /// the march stops once it is above the map's highest sample and still climbing.
    pub fn is_clear_line_of_sight(&self, pos: &Coord3D, pos_other: &Coord3D) -> bool {
        if self.x_extent() <= 0 || self.y_extent() <= 0 {
            // `if (m_map == nullptr) return false;`
            return false;
        }
        let inv = 1.0 / MAP_XY_FACTOR;
        let border = self.border_size();
        let start_x = real_to_int_floor(pos.x * inv) + border;
        let start_y = real_to_int_floor(pos.y * inv) + border;
        let end_x = real_to_int_floor(pos_other.x * inv) + border;
        let end_y = real_to_int_floor(pos_other.y * inv) + border;
        let delta_x = (end_x - start_x).abs();
        let delta_y = (end_y - start_y).abs();
        let mut x = start_x;
        let mut y = start_y;
        let (mut xinc1, mut xinc2) = if end_x >= start_x { (1, 1) } else { (-1, -1) };
        let (mut yinc1, mut yinc2) = if end_y >= start_y { (1, 1) } else { (-1, -1) };
        let (den, mut num, numadd, numpixels);
        if delta_x >= delta_y {
            xinc1 = 0;
            yinc2 = 0;
            den = delta_x;
            num = delta_x / 2;
            numadd = delta_y;
            numpixels = delta_x;
        } else {
            xinc2 = 0;
            yinc1 = 0;
            den = delta_y;
            num = delta_y / 2;
            numadd = delta_x;
            numpixels = delta_y;
        }
        // `Real nsInv = 1.0f / numpixels;` — zero pixels leaves `zinc` infinite and the loop empty.
        let ns_inv = 1.0 / numpixels as f32;
        let mut z = pos.z;
        let zinc = (pos_other.z - z) * ns_inv;
        let max_height = self.max_sample() as f32 * MAP_HEIGHT_SCALE;
        const LOS_FUDGE: f32 = 0.5;
        let (xe, ye) = (self.x_extent(), self.y_extent());
        for _ in 0..numpixels {
            if x < 0 || y < 0 || x >= xe - 1 || y >= ye - 1 {
                // "once we go off the map, we're done".
                break;
            }
            let (Some(a), Some(b), Some(c), Some(d)) =
                (self.raw_sample(x, y), self.raw_sample(x + 1, y), self.raw_sample(x, y + 1), self.raw_sample(x + 1, y + 1))
            else {
                break;
            };
            let height = a.max(b).max(c).max(d) as f32 * MAP_HEIGHT_SCALE;
            if height > z + LOS_FUDGE {
                return false;
            }
            // "we're above the max height of the terrain and still looking up, so we're done".
            if z >= max_height && zinc > 0.0 {
                break;
            }
            z += zinc;
            num += numadd;
            if num >= den {
                num -= den;
                x += xinc1;
                y += yinc1;
            }
            x += xinc2;
            y += yinc2;
        }
        true
    }
}

impl GameLogic {
    /// `PartitionManager::isClearLineOfSightTerrain` (`PartitionManager.cpp:4582-4650`): the terrain
    /// march between two objects' *eye* heights (`getMaxHeightAbovePosition` above each object's live
    /// position). A `None` side uses the position passed in as-is (the weapon's firing-line origin).
    pub(crate) fn is_clear_line_of_sight_terrain(
        &self,
        obj: Option<ObjectId>,
        obj_pos: Coord3D,
        other: Option<ObjectId>,
        other_pos: Coord3D,
    ) -> bool {
        let eye = |id: Option<ObjectId>, fallback: Coord3D| match id.and_then(|i| self.objects.get(&i)) {
            Some(o) => Coord3D::new(o.position.x, o.position.y, o.position.z + o.template.geometry.max_height_above_position()),
            None => fallback,
        };
        let pos = eye(obj, obj_pos);
        let pos_other = eye(other, other_pos);
        self.terrain.is_clear_line_of_sight(&pos, &pos_other)
    }

    /// `Pathfinder::isViewBlockedByObstacle` (`AIPathfind.cpp:9436-9455`): no LOS check to a flying
    /// object, then the attack view test.
    pub(crate) fn is_view_blocked_by_obstacle(&self, id: ObjectId, other: ObjectId) -> bool {
        if self.significantly_above_terrain(other) {
            return false; // "We don't check los to flying objects."
        }
        let (Some(a), Some(b)) = (self.objects.get(&id), self.objects.get(&other)) else { return false };
        self.attack_view(id, Some(other)).is_some_and(|v| self.pathfinder.attack_view_blocked(&self.terrain, &v, &a.position, &b.position))
    }

    /// The object-side inputs of `Pathfinder::isAttackViewBlockedByObstacle` (`AIPathfind.cpp:9459-9536`) for
    /// `attacker` against `victim` (`None` once the victim object is gone). `None` means the view is never
    /// blocked: the `AttackUsesLineOfSight` switch is off (`:9468`) or the attacker does not need line of
    /// sight (`:9475`). The positions are not read here, so a search can test each candidate cell with the
    /// same view ([`Pathfinder::attack_view_blocked`]).
    pub(crate) fn attack_view(&self, attacker: ObjectId, victim: Option<ObjectId>) -> Option<AttackView> {
        if !self.ai_tunables.attack_uses_line_of_sight {
            return None;
        }
        let a = self.objects.get(&attacker)?;
        if !a.template.attack_needs_line_of_sight {
            return None;
        }
        // `w = attacker->getCurrentWeapon()`, cleared for an immobile attacker (`:9484-9489`): only a mobile
        // attacker with a weapon runs the firing-line terrain test.
        let terrain_test = !a.is_kind_of(crate::template::kind_of::IMMOBILE) && a.combat.weapons.weapon(a.combat.current_weapon_slot).is_some();
        let victim = victim.and_then(|id| {
            let v = self.objects.get(&id)?;
            Some(VictimView {
                id,
                slaver: v.master.map(|m| m.0),
                center: Coord3D::new(v.position.x, v.position.y, v.position.z + v.template.geometry.z_delta_to_center_position()),
            })
        });
        Some(AttackView {
            attacker,
            attacker_container: a.contained_by.map(|c| c.0),
            attacker_slaver: a.master.map(|m| m.0),
            victim,
            terrain_test,
            attacker_eye_height: a.template.geometry.max_height_above_position(),
        })
    }
}

/// The object-side inputs of `Pathfinder::isAttackViewBlockedByObstacle` (see `GameLogic::attack_view`). The
/// obstacle ids are the `u32` values the pathfinder's cells hold.
#[derive(Clone, Copy, Debug)]
pub struct AttackView {
    /// `attacker->getID()`: the attacker's own obstacle never blocks its view (`AIPathfind.cpp:9396`).
    pub attacker: ObjectId,
    /// `getContainerID(attacker)`: exempt (`:9409`).
    pub attacker_container: Option<u32>,
    /// `getSlaverID(attacker)`: exempt (`:9415`).
    pub attacker_slaver: Option<u32>,
    /// The victim object, when it exists (`:9399-9406`); `None` for an attack on a position.
    pub victim: Option<VictimView>,
    /// The firing-line terrain test runs: the attacker has a current weapon and is not immobile (`:9489`).
    pub terrain_test: bool,
    /// `getMaxHeightAbovePosition()` of the attacker, which lifts the firing-line origin (`Weapon.cpp:3159`).
    pub attacker_eye_height: f32,
}

/// The victim object's part of an [`AttackView`].
#[derive(Clone, Copy, Debug)]
pub struct VictimView {
    pub id: ObjectId,
    /// `getSlaverID(victim)`: exempt (`AIPathfind.cpp:9404`).
    pub slaver: Option<u32>,
    /// The victim's geometry centre, the firing-line target (`Weapon.cpp:3221`).
    pub center: Coord3D,
}

impl Pathfinder {
    /// `Pathfinder::isAttackViewBlockedByObstacle` (`AIPathfind.cpp:9459-9536`) from an attacker at
    /// `attacker_pos` to a victim at `victim_pos`: the weapon's firing-line terrain test
    /// (`Weapon::isClearGoalFiringLineOfSightTerrain`, `Weapon.cpp:3214-3238`), then the obstacle cells on the
    /// line (`iterateCellsAlongLine` with `attackBlockedByObstacleCallback`, `:9385-9433`). `RETAIL_COMPATIBLE_CRC`
    /// is on, so the contact-weapon early-out (`#if !RETAIL_COMPATIBLE_CRC`) is not part of it. The `skipCount`
    /// of an attacker off the ground layer (`:9522-9524`) is not applied (see the module docs).
    pub(crate) fn attack_view_blocked(&self, terrain: &TerrainLogic, view: &AttackView, attacker_pos: &Coord3D, victim_pos: &Coord3D) -> bool {
        if view.terrain_test {
            // The origin is the goal position lifted by the attacker's height (`getFiringLineOfSightOrigin`,
            // `Weapon.cpp:3159`); the target is the victim's centre, or the given position without a victim.
            let origin = Coord3D::new(attacker_pos.x, attacker_pos.y, attacker_pos.z + view.attacker_eye_height);
            let target = view.victim.map_or(*victim_pos, |v| v.center);
            if !terrain.is_clear_line_of_sight(&origin, &target) {
                return true;
            }
        }
        // `info.victimCell = getCell(layer, &victimPos)` (`:9519`): null off the grid, so no victim cell (R21-5).
        let victim_cell_obstacle = self.los_cell_obstacle_at(victim_pos);
        for (ty, obstacle, transparent) in self.los_cells_along_line(attacker_pos, victim_pos) {
            if ty != CellType::Obstacle {
                continue;
            }
            // "we never block our own view!" (`:9396`).
            if obstacle == view.attacker.0 {
                continue;
            }
            // "nor does the object we're trying to attack!", and the victim's slaver (`:9399-9406`).
            if let Some(v) = view.victim {
                if obstacle == v.id.0 || Some(obstacle) == v.slaver {
                    continue;
                }
            }
            // The attacker's container (`:9409`). The victim's container is not exempt: `:9412` is a `@todo`.
            if Some(obstacle) == view.attacker_container {
                continue;
            }
            // The item the attacker is slaved to (`:9415`).
            if Some(obstacle) == view.attacker_slaver {
                continue;
            }
            // `isObstacleTransparent` (`:9418`).
            if transparent {
                continue;
            }
            // "Victim is inside the bounds of another object" (`:9423`): the victim's cell's obstacle does not block.
            if victim_cell_obstacle == Some(obstacle) {
                continue;
            }
            return true;
        }
        false
    }
}

/// Lane 179 (R21-2): the victim's container is not exempt from the obstacle march.
#[cfg(test)]
#[path = "los_l179.rs"]
mod l179_tests;

/// Lane 180 (R21-3, R21-5, item 2): the no-weapon terrain rule, the off-grid victim cell, and the shared view.
#[cfg(test)]
#[path = "los_l180.rs"]
mod l180_tests;
