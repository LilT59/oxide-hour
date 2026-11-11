//! Object-object collisions: the `CollideModule` families and the per-frame collision pass.
//!
//! Ported from `GameLogic/Object/Collide/*` and `GameLogic/Object/Update/PhysicsUpdate.cpp`:
//!
//! * `PartitionManager::processContactList` (`PartitionManager.cpp:2557`): the engine builds a
//!   contact list from dirty, non-immobile partition data (`PartitionManager::update`,
//!   `PartitionManager.cpp:2796`) and dispatches `Object::onCollide` (`Object.cpp:2438`) to every
//!   collide module of both objects.
//! * `PhysicsBehavior::onCollide` (`PhysicsUpdate.cpp:1155`) + `checkForOverlapCollision`
//!   (`PhysicsUpdate.cpp:1478`): object-object push-apart forces and crush.
//! * `SquishCollide::onCollide` (`SquishCollide.cpp:62`).
//! * `FireWeaponCollide::onCollide` (`FireWeaponCollide.cpp:75`).
//! * `RailroadBehavior::onCollide` (`RailroadGuideAIUpdate.cpp:210-420`, ported in
//!   [`crate::railroad`]): the train's trample/crush/shove and the rear-end crash. That module *is*
//!   a `PhysicsBehavior` (`RailroadGuideAIUpdate.h:186`), so it is dispatched where the physics
//!   module would be and does not chain to `PhysicsBehavior::onCollide`.
//! * `CrateCollide::onCollide` + `isValidToExecute` (`CrateCollide.cpp:106`, `:142`) and every
//!   shipped subclass (`Money`/`Heal`/`Veterancy`/`Unit`/`Salvage`/`Shroud`/`ConvertToCarBomb`/
//!   `ConvertToHijackedVehicle`/`Sabotage*`).
//!
//! The sim has no `PartitionContactList`, so the pass scans the partition for overlapping
//! non-immobile objects. Pairs are deduped; both directions are dispatched (the engine does the
//! same). Objects in contact are re-dirtied for the next frame (`PartitionData::makeDirty(false)`,
//! `PartitionManager.cpp:2607`), so a persistent overlap keeps colliding (needed by
//! `FireWeaponCollide`, which fires every frame).

use std::collections::BTreeSet;

use crate::body::DamageInfo;
use crate::damage::{DamageType, DeathType, HUGE_DAMAGE_AMOUNT};
use crate::logic::GameLogic;
use crate::math::{cos, sin, sqrt, Coord3D};
use crate::modules2::{CrateCollideTemplate, CrateEffect, FireWeaponCollideTemplate};
use crate::object::ObjectId;
use crate::template::{kind_of, GeometryShape};
use crate::armor::{ARMORSET_CRATE_UPGRADE_ONE, ARMORSET_CRATE_UPGRADE_TWO};
use crate::weapon::{WEAPONSET_CARBOMB, WEAPONSET_CRATEUPGRADE_ONE, WEAPONSET_CRATEUPGRADE_TWO};

/// `CrushEnum` (`CrushDie.h:42`): which crush point a `CrushDie` hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CrushEnum {
    Total,
    BackEnd,
    FrontEnd,
    NoCrush,
}

/// `perpsLogicallyEqual` (`PhysicsUpdate.cpp:1457`): equality with a wiggle fudge.
fn perps_logically_equal(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.15
}

/// The three-point `crushTarget` selection (`PhysicsUpdate.cpp:1566-1724`): pick the front, back
/// or middle crush point whose perpendicular distance to the crusher's direction ray is shortest
/// (straight-line distance breaks ties).
fn crush_target_three_points(crusher_pos: Coord3D, victim_pos: Coord3D, dir: (f32, f32), offset: (f32, f32)) -> CrushEnum {
    let perp = |p: Coord3D| {
        let fx = p.x - crusher_pos.x;
        let fy = p.y - crusher_pos.y;
        let ray = fx * dir.0 + fy * dir.1;
        let dx = ray * dir.0 - fx;
        let dy = ray * dir.1 - fy;
        let perp_length = sqrt(dx * dx + dy * dy);
        let straight = sqrt(fx * fx + fy * fy);
        (perp_length, straight)
    };
    let front = Coord3D::new(victim_pos.x + offset.0, victim_pos.y + offset.1, victim_pos.z);
    let back = Coord3D::new(victim_pos.x - offset.0, victim_pos.y - offset.1, victim_pos.z);
    let (front_perp, front_len) = perp(front);
    let (back_perp, back_len) = perp(back);
    let (center_perp, center_len) = perp(victim_pos);
    let pe = perps_logically_equal;
    if front_perp <= center_perp && front_perp <= back_perp {
        if pe(front_perp, center_perp) || pe(front_perp, back_perp) {
            if pe(front_perp, center_perp) {
                if front_len < center_len {
                    CrushEnum::FrontEnd
                } else {
                    CrushEnum::Total
                }
            } else if front_len < back_len {
                CrushEnum::FrontEnd
            } else {
                CrushEnum::BackEnd
            }
        } else {
            CrushEnum::FrontEnd
        }
    } else if back_perp <= center_perp && back_perp <= front_perp {
        if pe(back_perp, center_perp) || pe(back_perp, front_perp) {
            if pe(back_perp, center_perp) {
                if back_len < center_len {
                    CrushEnum::BackEnd
                } else {
                    CrushEnum::Total
                }
            } else if back_len < front_len {
                CrushEnum::BackEnd
            } else {
                CrushEnum::FrontEnd
            }
        } else {
            CrushEnum::BackEnd
        }
    } else if pe(center_perp, back_perp) || pe(center_perp, front_perp) {
        if pe(center_perp, front_perp) {
            if center_len < front_len {
                CrushEnum::Total
            } else {
                CrushEnum::FrontEnd
            }
        } else if center_len < back_len {
            CrushEnum::Total
        } else {
            CrushEnum::BackEnd
        }
    } else {
        CrushEnum::Total
    }
}

/// Snapshot of one object's collision geometry (position, orientation, shape, extents).
#[derive(Clone, Copy)]
struct CollideGeom {
    pos: Coord3D,
    angle: f32,
    shape: GeometryShape,
    major: f32,
    minor: f32,
    height: f32,
    radius: f32,
}

impl CollideGeom {
    fn z_top(&self) -> f32 {
        self.pos.z + self.height * 0.5
    }
    fn z_bot(&self) -> f32 {
        self.pos.z - self.height * 0.5
    }
}

/// 2D circle/circle overlap (`xy_collideTest_Circle_Circle`).
fn circle_circle(a: &CollideGeom, b: &CollideGeom) -> bool {
    let dx = b.pos.x - a.pos.x;
    let dy = b.pos.y - a.pos.y;
    dx * dx + dy * dy <= (a.radius + b.radius) * (a.radius + b.radius)
}

/// 2D oriented-rect vs circle (`xy_collideTest_Rect_Circle`, `PartitionManager.cpp:489-522`, the `#if 1` branch): the
/// cylinder becomes a square with half-side `major` (`btmp.geom.setMinorRadius(btmp.geom.getMajorRadius())`, so both
/// half-extents are `major` and the side is `2 * major`), turned by the object's own angle, and the two are tested as
/// rects. Not an exact circle test: lane 195 (AN3 s1 Ranger 258's blocker contact came 3 frames later than the C++
/// square test, `ZR_L175_ENTER f=3186`).
fn rect_circle(r: &CollideGeom, c: &CollideGeom) -> bool {
    let square = CollideGeom { minor: c.major, ..*c };
    rect_rect(r, &square)
}

/// `xy_collideTest_Rect_Rect` (`PartitionManager.cpp:554-594`): two rects touch when a corner of either lies inside
/// the other (`rectToFourPoints` and `testRotatedPointsAgainstRect`, both ways). It is not a separating-axis test:
/// two thin rects that cross with no corner inside the other fail this test (lane 195). Contact for such a pair can still
/// come from the close-enough preflight in `geom_collides`, which runs first.
fn rect_rect(a: &CollideGeom, b: &CollideGeom) -> bool {
    corner_inside(a, b) || corner_inside(b, a)
}

/// `rectToFourPoints` (`PartitionManager.cpp:437-466`): the top-left, top-right, bottom-left and bottom-right corners of
/// `a`'s oriented rect, in the C++ order and with its expressions.
fn rect_to_four_points(a: &CollideGeom) -> [(f32, f32); 4] {
    let c = crate::math::cos(a.angle);
    let s = crate::math::sin(a.angle);
    let exc = a.major * c;
    let eyc = a.minor * c;
    let exs = a.major * s;
    let eys = a.minor * s;
    [
        (a.pos.x - exc - eys, a.pos.y + eyc - exs),
        (a.pos.x + exc - eys, a.pos.y + eyc + exs),
        (a.pos.x - exc + eys, a.pos.y - eyc - exs),
        (a.pos.x + exc + eys, a.pos.y - eyc + exs),
    ]
}

/// `testRotatedPointsAgainstRect` (`PartitionManager.cpp:400-434`) for one rect: whether a corner of `a` lies inside `b`.
fn corner_inside(a: &CollideGeom, b: &CollideGeom) -> bool {
    let c = crate::math::cos(-b.angle);
    let s = crate::math::sin(-b.angle);
    rect_to_four_points(a).iter().any(|&(px, py)| {
        let ptx = px - b.pos.x;
        let pty = py - b.pos.y;
        let nx = (ptx * c - pty * s).abs();
        let ny = (ptx * s + pty * c).abs();
        nx <= b.major && ny <= b.minor
    })
}

/// `PartitionData::collidesWith` (`PartitionManager.cpp:2017`): z-overlap first, then the 2D shape
/// test: corner containment for boxes (`xy_collideTest_Rect_Rect`, box vs cylinder through the square), exact circles.
fn geom_collides(a: &CollideGeom, b: &CollideGeom) -> bool {
    if a.z_top() < b.z_bot() || a.z_bot() > b.z_top() {
        return false;
    }
    // `z_collideTest_Nonsphere_Nonsphere`'s preflight (PartitionManager.cpp:658-673): the box and cylinder pairs
    // (`collideTest_Cylinder_Cylinder`, `_Cylinder_Box`, `_Box_Cylinder`, `_Box_Box`, :742-771) declare contact when
    // the smallest of the four original half-extents, squared, exceeds the squared centre distance, with no 2D test.
    // Sphere pairs use `z_collideTest_Sphere_Nonsphere` (:597-650), which has no preflight.
    if a.shape != GeometryShape::Sphere && b.shape != GeometryShape::Sphere {
        let min_r = a.major.min(a.minor).min(b.major.min(b.minor));
        let dx = a.pos.x - b.pos.x;
        let dy = a.pos.y - b.pos.y;
        if min_r * min_r > dx * dx + dy * dy {
            return true;
        }
    }
    match (a.shape, b.shape) {
        (GeometryShape::Box, GeometryShape::Box) => rect_rect(a, b),
        (GeometryShape::Box, _) => rect_circle(a, b),
        (_, GeometryShape::Box) => rect_circle(b, a),
        _ => circle_circle(a, b),
    }
}

impl GameLogic {
    /// `PartitionData::collidesWith` (`PartitionManager.cpp:2017`) for two live objects: the geometry test
    /// that makes a contact in `PartitionContactList::processContactList`, and so the `onCollide` calls
    /// (`PartitionManager.cpp:2586`, `2595`) that reach `AIUpdateInterface::processCollision`.
    /// `KINDOF_NO_COLLIDE` objects never collide (`:2022`). The contact *pairs* are the partition's COI
    /// cells (`addPossibleCollisions`, `:1980`), not a range scan. Test-only since the collision pass
    /// works on its own geometry snapshot.
    #[cfg(test)]
    pub(crate) fn geometry_contact(&self, a: ObjectId, b: ObjectId) -> bool {
        let geom = |id: ObjectId| {
            let o = self.objects.get(&id)?;
            if o.is_kind_of(crate::template::kind_of::NO_COLLIDE) {
                return None;
            }
            let g = &o.template.geometry;
            Some(CollideGeom {
                pos: o.position,
                angle: o.angle,
                shape: g.shape,
                major: g.major_radius,
                minor: g.minor_radius,
                height: g.height,
                radius: g.bounding_circle_radius(),
            })
        };
        match (geom(a), geom(b)) {
            (Some(x), Some(y)) => geom_collides(&x, &y),
            _ => false,
        }
    }
}

// Test-only record of every `GameLogic::on_collide` call (`obj`, `other`, `normal`), in call order: the contact
// dispatch's order is what the tests check.
#[cfg(test)]
thread_local! {
    pub(crate) static ON_COLLIDE_LOG: std::cell::RefCell<Vec<(ObjectId, ObjectId, Coord3D)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The point between the two objects where the collision happened, and the normal pointing from
/// `a` to `b` (`CollideLocAndNormal`). Only used by modules that want it; none of the ported ones
/// read it, so an approximate midpoint is enough.
fn collide_loc_normal(a: &CollideGeom, b: &CollideGeom) -> (Coord3D, Coord3D) {
    let loc = Coord3D::new((a.pos.x + b.pos.x) * 0.5, (a.pos.y + b.pos.y) * 0.5, (a.pos.z + b.pos.z) * 0.5);
    let dx = b.pos.x - a.pos.x;
    let dy = b.pos.y - a.pos.y;
    let d = sqrt(dx * dx + dy * dy).max(0.0001);
    (loc, Coord3D::new(dx / d, dy / d, 0.0))
}

impl GameLogic {
    /// `ThePartitionManager->UPDATE()`'s contact half (`PartitionManager::update`, `PartitionManager.cpp:2808-2840`):
    /// gather the contact list from the dirty modules, then process it. See [`gather_contacts`](Self::gather_contacts)
    /// and [`dispatch_contacts`](Self::dispatch_contacts).
    pub(crate) fn process_object_collisions(&mut self) {
        let contacts = self.gather_contacts();
        self.dispatch_contacts(&contacts);
    }

    /// `PartitionManager::update`'s loop over the dirty modules (`PartitionManager.cpp:2810-2838`) and the
    /// `addPossibleCollisions` pairs (`:1980-2014`) they add to the contact list (`addToContactList`,
    /// `:2440-2478`), in the engine's order.
    ///
    /// The dirty list is walked from its most recent entry (`prependToDirtyModules` prepends, `PartitionManager.h:564`),
    /// which is the largest [`dirty_stamp`](crate::object::Object::dirty_stamp) first. Each module is taken off the
    /// list (`removeFromDirtyModules`, `:2827`), places its cells when it needs a cell update (`updateCellsTouched`,
    /// `:2831`), and then, if it needs a collision check and is mobile, pairs itself with the objects in its COI
    /// cells (`:2834-2837`). That is interleaved: a module placed later in the walk is still in its old cells when an
    /// earlier one gathers, and an object outside the partition has no module at all.
    ///
    /// The result is the contact list in insertion order. The engine prepends each new pair, so `processContactList`
    /// (`:2557`) walks it backwards; [`dispatch_contacts`](Self::dispatch_contacts) takes this order reversed.
    pub(crate) fn gather_contacts(&mut self) -> Vec<(ObjectId, ObjectId)> {
        let mut dirty: Vec<(u64, ObjectId)> = self.objects.values().filter(|o| o.in_dirty_list()).map(|o| (o.dirty_stamp, o.id)).collect();
        dirty.sort_unstable_by(|a, b| b.cmp(a));
        // `addToContactList`'s "already noted" test (`:2454-2463`): the pair is kept once, in either order.
        let mut noted: BTreeSet<(ObjectId, ObjectId)> = BTreeSet::new();
        let mut contacts: Vec<(ObjectId, ObjectId)> = Vec::new();
        for (_, id) in dirty {
            let Some(o) = self.objects.get(&id) else { continue };
            let (update_cells, collide, immobile) = (o.partition_dirty, o.in_dirty_list(), o.template.kind_of & kind_of::IMMOBILE != 0);
            // `removeFromDirtyModules` (`:2827`): the flags are cleared before the module is handled.
            if let Some(o) = self.objects.get_mut(&id) {
                o.clear_dirty();
            }
            // A module with no partition data is not in the list: an object outside the partition
            // (contained, hijacked, destroyed) was removed from it (`PartitionData::removeFromPartitionManager`).
            if !self.in_partition(id) {
                continue;
            }
            // `updateCellsTouched` (`:2086-2166`) places the object's COI cells from its current pose.
            if update_cells {
                let o = &self.objects[&id];
                let g = &o.template.geometry;
                let cells = self.partition.coi_cells(o.position, o.angle, g.shape, g.major_radius, g.minor_radius, g.is_small);
                self.partition.set_coi_cells(id, cells);
            }
            // `addPossibleCollisions` (`:2001-2013`): every cell it covers with at least two objects in it.
            if !collide || immobile {
                continue;
            }
            for cell in self.partition.coi_cells_of(id).to_vec() {
                let list = self.partition.coi_list(cell);
                if list.len() < 2 {
                    continue;
                }
                for &other in list.iter().rev() {
                    if other == id {
                        continue;
                    }
                    let key = if id < other { (id, other) } else { (other, id) };
                    if noted.insert(key) {
                        contacts.push((id, other));
                    }
                }
            }
        }
        contacts
    }

    /// `PartitionContactList::processContactList` (`PartitionManager.cpp:2557-2615`): the contacts are walked from the
    /// head of the list, which is the last pair gathered. Each pair is tested again at dispatch, with the objects'
    /// positions as they are now (`friend_collidesWith`, `:2567`), because an earlier pair's `onCollide` may have moved
    /// them. `onCollide` runs for the gathered object (`obj`, the dirty side) and then for `other`, with the normal
    /// flipped for the second call (`:2586-2595`). Both are re-dirtied for the next frame (`:2604-2613`).
    pub(crate) fn dispatch_contacts(&mut self, contacts: &[(ObjectId, ObjectId)]) {
        for &(a, b) in contacts.iter().rev() {
            // A pair whose object left the partition during this walk was nulled (`removeSpecificPartitionData`).
            let (Some(ga), Some(gb)) = (self.live_collide_geom(a), self.live_collide_geom(b)) else { continue };
            // `friend_collidesWith(other, &cinfo)` (`:2567-2570`): no overlap now, no contact.
            if !geom_collides(&ga, &gb) {
                continue;
            }
            // `OBJECT_STATUS_NO_COLLISIONS` on either side (`:2573-2575`).
            if self.has_no_collisions(a) || self.has_no_collisions(b) {
                continue;
            }
            let (loc, normal) = collide_loc_normal(&ga, &gb);
            self.stats.collision_checks += 1;
            self.on_collide(a, b, loc, normal);
            // `flipCoord3D(&cinfo.normal)` (`:2587`): the second call gets the reversed normal.
            let flipped = Coord3D::new(-normal.x, -normal.y, -normal.z);
            if !self.objects.get(&a).is_some_and(|o| o.destroyed) && !self.objects.get(&b).is_some_and(|o| o.destroyed) {
                self.on_collide(b, a, loc, flipped);
            }
            // `makeDirty(false)` for each mobile object still in the partition (`:2604-2613`): it checks again next frame.
            for id in [a, b] {
                if self.in_partition(id) && self.objects.get(&id).is_some_and(|o| o.template.kind_of & kind_of::IMMOBILE == 0) {
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.mark_collision_dirty();
                    }
                }
            }
        }
    }

    /// Whether the object is in the partition: alive, not in a container and not a hijacker riding a vehicle
    /// (`HijackerUpdate`). Dying objects stay in: `PartitionManager.cpp:1982-1990` keeps dead AIs colliding (dead
    /// technicals in the air must hit immobile structures), which `physics_on_collide` relies on.
    pub(crate) fn in_partition(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| !o.destroyed && o.contained_by.is_none() && o.behavior.hijacker_vehicle.is_none())
    }

    /// Whether `OBJECT_STATUS_NO_COLLISIONS` is set (`Object::getStatusBits`, `Object.cpp:2447`).
    pub(crate) fn has_no_collisions(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.status_bits & crate::object::status::NO_COLLISIONS != 0)
    }

    /// `Object::onCollide` leaves the collide modules alone once the object is gone or `NO_COLLISIONS` is set
    /// (`Object.cpp:2447`, checked before each module).
    fn collisions_stopped(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_none_or(|o| o.destroyed || o.status_bits & crate::object::status::NO_COLLISIONS != 0)
    }

    /// The collision geometry of an object now (`PartitionData::collidesWith`, `PartitionManager.cpp:2017`): `None`
    /// for an object outside the partition, and for `KINDOF_NO_COLLIDE` (`:2022`).
    fn live_collide_geom(&self, id: ObjectId) -> Option<CollideGeom> {
        if !self.in_partition(id) {
            return None;
        }
        let o = self.objects.get(&id)?;
        if o.template.kind_of & kind_of::NO_COLLIDE != 0 {
            return None;
        }
        let g = &o.template.geometry;
        Some(CollideGeom {
            pos: o.position,
            angle: o.angle,
            shape: g.shape,
            major: g.major_radius,
            minor: g.minor_radius,
            height: g.height,
            radius: g.bounding_circle_radius(),
        })
    }
}

impl GameLogic {
    /// `Object::onCollide` (`Object.cpp:2438`): run `obj`'s collide modules against `other`, in the
    /// template's module order (the sim keeps a fixed order; none of the ported modules depend on
    /// it). `ObjectId::INVALID` is the engine's null `other` (a ground contact, `PhysicsUpdate.cpp:845`).
    pub(crate) fn on_collide(&mut self, obj: ObjectId, other: ObjectId, loc: Coord3D, normal: Coord3D) {
        #[cfg(test)]
        ON_COLLIDE_LOG.with(|l| l.borrow_mut().push((obj, other, normal)));
        // `Object::onCollide` checks `OBJECT_STATUS_NO_COLLISIONS` before each collide module (`Object.cpp:2447`).
        if self.collisions_stopped(obj) {
            return;
        }
        // `RailroadBehavior` is itself the train's `CollideModule`: it derives from `PhysicsBehavior`
        // (`RailroadGuideAIUpdate.h:186`) and overrides `onCollide` (`:222`) without chaining to the
        // base, so a train runs the railroad collision and not the physics push-apart. No shipped
        // template has both modules; if one had, the engine would run both in template order.
        if self.objects.get(&obj).is_some_and(|o| o.template.railroad.is_some()) {
            self.railroad_on_collide(obj, other);
        }
        if self.collisions_stopped(obj) {
            return;
        }
        if self.objects.get(&obj).is_some_and(|o| o.template.physics.is_some()) {
            self.physics_on_collide(obj, other, loc, normal);
        }
        if self.collisions_stopped(obj) {
            return;
        }
        if self.objects.get(&obj).is_some_and(|o| o.template.crush.squish_collide) {
            self.squish_collide_on_collide(obj, other);
        }
        if self.collisions_stopped(obj) {
            return;
        }
        if let Some(t) = self.objects.get(&obj).and_then(|o| o.template.fire_weapon_collide.clone()) {
            self.fire_weapon_collide_on_collide(obj, other, &t);
        }
        if self.collisions_stopped(obj) {
            return;
        }
        if let Some(ts) = self.objects.get(&obj).map(|o| o.template.crate_collides.clone()) {
            for t in &ts {
                // `CrateCollide::onCollide` destroys the crate on success, so stop at the first
                // variant that executes (the GLA saboteur's six variants are mutually exclusive).
                if self.crate_collide_on_collide(obj, other, t) {
                    break;
                }
            }
        }
        if self.collisions_stopped(obj) {
            return;
        }
        // `OpenContain::onCollide` (OpenContain.cpp:850-907): the container is a `CollideModule`; a
        // colliding unit whose `AIEnterState` targets it is added to the contain list.
        if self.objects.get(&obj).and_then(|o| o.template.contain.as_ref()).is_some() {
            self.contain_on_collide(obj, other);
        }
        let _ = (loc, normal);
    }

    /// `AIUpdateInterface::processCollision` (`AIUpdate.cpp:1427`) for two AI units that do not push
    /// by force. Ground units only (`isDoingGroundMovement`, `:1446-1447`). A unit that is moving is
    /// the blocked case, which `process_unit_collisions` handles. When neither moves and the two
    /// stand less than half a cell apart (`:1558`), an idle unit is sent to a legal spot next to
    /// where it stands (`adjustToPossibleDestination` + `aiMoveToPosition`, `CMD_FROM_AI`,
    /// `:1568-1574`). `AI_BUSY` (`:1560`) and `IS_USING_ABILITY` (`:1564`) units are left alone.
    fn ai_unit_overlap(&mut self, obj: ObjectId, other: ObjectId) {
        let frame = self.frame;
        let (Some(me), Some(them)) = (self.objects.get(&obj), self.objects.get(&other)) else { return };
        let ground = |o: &crate::object::Object| o.locomotor().is_some_and(|l| !l.is_airborne());
        if !ground(me) || !ground(them) {
            return;
        }
        if me.ai.ignore_collisions_until > frame || me.ai.can_path_through_units {
            return;
        }
        if me.is_ai_moving() || them.is_ai_moving() {
            return;
        }
        let d = me.position.dist_2d(&them.position);
        if d * d >= crate::pathfind::PATHFIND_CELL_SIZE_F * crate::pathfind::PATHFIND_CELL_SIZE_F * 0.25 {
            return;
        }
        if me.ai.machine.current == crate::ai_states::AiStateType::Busy || me.status_mask(frame) & crate::object::status::USING_ABILITY != 0 {
            return;
        }
        // `isIdle` is state-only (`AIUpdate.cpp:3120`). A dead AI is in `AI_DEAD` (`AIUpdate.cpp:1102`,
        // `ai_dead_update`), which is not idle, so a dying unit fails this test on its own.
        let me_idle = me.ai.machine.current == crate::ai_states::AiStateType::Idle;
        let them_idle = them.ai.machine.current == crate::ai_states::AiStateType::Idle;
        let (my_pos, their_pos) = (me.position, them.position);
        let my_path = crate::pathfind::PathObject::for_object(me);
        let their_path = crate::pathfind::PathObject::for_object(them);
        if me_idle {
            let safe = match &my_path {
                Some(p) => self.pathfinder.adjust_destination(&self.terrain, p, &my_pos, None).unwrap_or(my_pos),
                None => my_pos,
            };
            self.ai_move_to_position_cmd(obj, safe, crate::ai_states::CommandSource::Ai);
        }
        if them_idle {
            let safe = match &their_path {
                Some(p) => self.pathfinder.adjust_destination(&self.terrain, p, &their_pos, None).unwrap_or(their_pos),
                None => their_pos,
            };
            self.ai_move_to_position_cmd(other, safe, crate::ai_states::CommandSource::Ai);
        }
    }

    /// `Object::canCrushOrSquish(victim, TEST_SQUISH_ONLY)` (`Object.cpp:1121`): `crusher` can squish
    /// `victim` (the victim has a `SquishCollide` module). No crush-level test in this mode.
    fn can_squish(&self, crusher: ObjectId, victim: ObjectId) -> bool {
        let (Some(a), Some(b)) = (self.objects.get(&crusher), self.objects.get(&victim)) else { return false };
        !a.unmanned() && a.template.crush.crusher_level > 0 && a.player != b.player && b.template.crush.squish_collide
    }

    /// `PhysicsBehavior::onCollide` (`PhysicsUpdate.cpp:1155`): push two mobile objects apart, and
    /// crush the other if this object is a moving crusher. `other == INVALID` is the engine's null
    /// pointer ("collide with ground"): the module hands the contact to the object's container and
    /// returns (`:1163-1173`), which is how a parachute learns its rider has landed.
    fn physics_on_collide(&mut self, obj: ObjectId, other: ObjectId, loc: Coord3D, normal: Coord3D) {
        if other == ObjectId::INVALID {
            // `Object* objContainedBy = obj->getContainedBy(); if (other == nullptr) { if
            // (objContainedBy) objContainedBy->onCollide(other, loc, normal); return; }`
            // (`PhysicsUpdate.cpp:1167-1175`). The location is the rider's, as the caller passed it.
            if let Some(container) = self.objects.get(&obj).and_then(|o| o.contained_by) {
                self.on_collide(container, ObjectId::INVALID, loc, normal);
            }
            return;
        }
        let (obj_contained, other_contained) = match (self.objects.get(&obj), self.objects.get(&other)) {
            (Some(a), Some(b)) => (a.contained_by, b.contained_by),
            _ => return,
        };
        // `if (otherContainedBy == obj || objContainedBy == other) return;`
        if other_contained == Some(obj) || obj_contained == Some(other) {
            return;
        }
        // `if (obj->testStatus(OBJECT_STATUS_PARACHUTING) && other->testStatus(OBJECT_STATUS_PARACHUTING))
        // return;` (`PhysicsUpdate.cpp:1192`): two parachutists never collide with each other.
        let parachuting = |id: ObjectId| {
            self.objects.get(&id).is_some_and(|o| o.status_mask(self.frame) & crate::object::status::PARACHUTING != 0)
        };
        if parachuting(obj) && parachuting(other) {
            return;
        }
        // Ignore collisions with our "ignore" obstacle, and vice versa (`PhysicsUpdate.cpp:1196`):
        // a docker ignores the dock it is passing through.
        let obstacle_ignored = |a: ObjectId, b: ObjectId, logic: &GameLogic| {
            logic.objects.get(&a).is_some_and(|o| o.ai_handle.is_some() && o.ai.ignore_obstacle_id == b)
        };
        if obstacle_ignored(obj, other, self) || obstacle_ignored(other, obj, self) {
            return;
        }
        let obj_immobile = self.is_immobile(obj);
        let other_immobile = self.is_immobile(other);
        let other_has_physics = self.objects.get(&other).is_some_and(|o| o.template.physics.is_some());
        // `if other has no physics, it is insubstantial (except immobile).`
        if !other_has_physics && !other_immobile {
            return;
        }
        // `checkForOverlapCollision`: crushing replaces the normal push-apart.
        if self.check_for_overlap_collision(obj, other) {
            return;
        }
        let mass = if obj_immobile {
            999999.0
        } else {
            // `Real mass = getMass(); if (immobile) mass = 999999.0f;` (`PhysicsUpdate.cpp:1266-1268`):
            // the same contained-items mass as `applyForce` divides by.
            self.get_mass(obj)
        };
        // `if (ai)`: AI objects move under their own initiative, not by getting bounced, unless they
        // are dead or parachuting and colliding with something immobile (`PhysicsUpdate.cpp:1271-1273`),
        // or `AIUpdateInterface::processCollision` returns TRUE (`:1274-1278`).
        let frame = self.frame;
        let (has_ai, dying, parachuting) = self.objects.get(&obj).map_or((false, false, false), |o| {
            (o.ai_handle.is_some(), o.is_dying(), o.status_mask(frame) & crate::object::status::PARACHUTING != 0)
        });
        if has_ai && !((dying || parachuting) && other_immobile) {
            // `aiOther == nullptr` -> `processCollision` returns FALSE (`AIUpdate.cpp:1442`).
            let other_has_ai = self.objects.get(&other).is_some_and(|o| o.ai_handle.is_some());
            if !other_has_ai {
                return;
            }
            // `processCollision` returns FALSE unless both units do ground movement (`AIUpdate.cpp:1446-1447`).
            // That covers every air pair and every air/ground pair: an air-only locomotor is not ground
            // movement (`AIUpdate.cpp:2368`), and `JetAIUpdate::isDoingGroundMovement` is always false
            // (`JetAIUpdate.cpp:2423`). The objects then ride through each other (the countermeasure
            // flares start on top of their aircraft, `CountermeasuresBehavior.cpp:347`).
            let ground_pair = [obj, other].iter().all(|id| self.objects.get(id).and_then(|o| o.locomotor()).is_some_and(|l| !l.is_airborne()));
            if !ground_pair {
                return;
            }
            // `AIUpdate.cpp:1433-1436`: `m_ignoreCollisionsUntil` and `m_canPathThroughUnits` return FALSE.
            if self.objects.get(&obj).is_some_and(|o| o.ai.ignore_collisions_until > frame || o.ai.can_path_through_units) {
                return;
            }
            // `processCollision` runs inside this `onCollide`, before the push, and its TRUE return keeps it
            // (`PhysicsUpdate.cpp:1278-1280`). The two branches split on `selfMoving` (`AIUpdate.cpp:1444`).
            let (moving, infantry) = self.objects.get(&obj).map_or((false, false), |o| (o.is_ai_moving(), o.is_kind_of(kind_of::INFANTRY)));
            let do_force = if moving {
                // The blocked case (`AIUpdate.cpp:1448-1541`): TRUE only for an infantry in `AI_PANIC` that is
                // blocked (`:1450-1458`), which bounces off. Every other moving case gets no force.
                self.process_collision_moving(obj, other)
            } else {
                // A unit not moving (`:1543`): dead infantry is pushed around by a squisher (`:1545-1552`), which
                // keeps the force. Otherwise the idle-overlap branch runs (`:1558-1574`), with no force.
                let dead_pushed = infantry && dying && self.can_squish(other, obj);
                if !dead_pushed {
                    self.ai_unit_overlap(obj, other);
                }
                dead_pushed
            };
            if !do_force {
                return;
            }
        }
        let allow_collide_force = self.objects.get(&obj).and_then(|o| o.physics.as_ref()).is_some_and(|p| p.allow_collide_force);
        if !allow_collide_force {
            return;
        }

        // Geometry centres: the sim has no per-geometry centre offset, so the object position is
        // the centre.
        let (us_center, them_center, airborne) = match (self.objects.get(&obj), self.objects.get(&other)) {
            (Some(a), Some(b)) => {
                let ground = self.terrain.surface_height(a.position.x, a.position.y, a.position.z);
                (a.position, b.position, a.position.z > ground + 0.001)
            }
            _ => return,
        };
        let mut delta = Coord3D::new(them_center.x - us_center.x, them_center.y - us_center.y, them_center.z - us_center.z);
        let (us_r, them_r, dist_sqr) = if airborne {
            let ur = self.objects.get(&obj).map_or(0.0, |o| o.template.geometry.bounding_sphere_radius());
            let tr = self.objects.get(&other).map_or(0.0, |o| o.template.geometry.bounding_sphere_radius());
            (ur, tr, delta.x * delta.x + delta.y * delta.y + delta.z * delta.z)
        } else {
            let ur = self.objects.get(&obj).map_or(0.0, |o| o.template.geometry.bounding_circle_radius());
            let tr = self.objects.get(&other).map_or(0.0, |o| o.template.geometry.bounding_circle_radius());
            delta.z = 0.0;
            (ur, tr, delta.x * delta.x + delta.y * delta.y)
        };
        if dist_sqr > (us_r + them_r) * (us_r + them_r) {
            return;
        }
        let mut dist = sqrt(dist_sqr);
        let mut overlap = us_r + them_r - dist;
        if dist < 1.0 {
            dist = 1.0;
        }
        let factor = if other_immobile && !self.objects.get(&obj).is_some_and(|o| o.destroyed) {
            // Bounce off an immobile structure like the ground (`PhysicsUpdate.cpp:1356`).
            const MIN_STIFF: f32 = 0.01;
            const MAX_STIFF: f32 = 0.99;
            let stiffness = self.rules.structure_stiffness.clamp(MIN_STIFF, MAX_STIFF);
            let mut mag = self.objects.get(&obj).and_then(|o| o.physics.as_ref()).map_or(0.0, |p| p.velocity_magnitude());
            let min_bounce_speed: f32 = 1.0 / (crate::math::LOGICFRAMES_PER_SECOND as f32 * 5.0);
            if mag < min_bounce_speed {
                mag = min_bounce_speed;
            }
            if let Some(p) = self.objects.get_mut(&obj).and_then(|o| o.physics.as_mut()) {
                p.velocity = Coord3D::ZERO;
            }
            -mag * mass * stiffness
        } else {
            if overlap > 5.0 {
                overlap = 5.0;
            }
            -overlap
        };
        let force = Coord3D::new(factor * delta.x / dist, factor * delta.y / dist, factor * delta.z / dist);
        self.apply_force(obj, force);
    }

    /// `PhysicsBehavior::checkForOverlapCollision` (`PhysicsUpdate.cpp:1478`): a moving crusher
    /// tests the victim's front/back/middle crush points and kills it once it has passed the
    /// closest one. `CrushDie::onDie`'s front/back flags are set from the closest point
    /// (`CrushDie.cpp:49`) so the crushed model condition matches.
    fn check_for_overlap_collision(&mut self, obj: ObjectId, other: ObjectId) -> bool {
        // "unless I am moving right now, I may not crush anything!"
        let moving = self
            .objects
            .get(&obj)
            .and_then(|o| o.physics.as_ref())
            .is_some_and(|p| p.velocity.x.abs() > 0.0001 || p.velocity.y.abs() > 0.0001 || p.velocity.z.abs() > 0.0001);
        if !moving {
            return false;
        }
        let self_crushing = self.can_crush_or_squish(obj, other);
        let self_being_crushed = self.can_crush_or_squish(other, obj);
        if self_crushing && self_being_crushed {
            return false;
        }
        if self_being_crushed {
            return true; // passive to the overlap
        }
        if !self_crushing {
            return false;
        }
        // `crusherPhysics->addOverlap(crusheeOther)` (`PhysicsUpdate.cpp:1050-1056`, called at
        // `:1523`): the overlap keeps the crusher's `calcSleepTime` awake (`:927-928`) while it sits
        // on its victim.
        if let Some(p) = self.objects.get_mut(&obj).and_then(|o| o.physics.as_mut()) {
            p.current_overlap = Some(other);
        }
        // The one-time 0-damage `DAMAGE_CRUSH` (a minor DamageFX) is client-only and not modelled.
        let (front, back) = match self.objects.get(&other).and_then(|o| o.body.as_ref()) {
            Some(b) => (b.front_crushed(), b.back_crushed()),
            None => return true,
        };
        if front && back {
            return true;
        }
        let (crush_target, crush_it) = self.crush_target_and_test(obj, other, front, back);
        if crush_it {
            let crush = self.crush_location_check(obj, other);
            if let Some(b) = self.objects.get_mut(&other).and_then(|o| o.body.as_mut()) {
                b.set_front_crushed(matches!(crush, CrushEnum::Total | CrushEnum::FrontEnd));
                b.set_back_crushed(matches!(crush, CrushEnum::Total | CrushEnum::BackEnd));
            }
            let _ = crush_target;
            self.damage_object_with_death(
                other,
                DamageInfo { source: obj, amount: HUGE_DAMAGE_AMOUNT, kind: DamageType::Crush },
                DeathType::Crushed,
            );
        }
        true
    }

    /// The `crushTarget` selection + "have we passed it" test from
    /// `PhysicsBehavior::checkForOverlapCollision` (`PhysicsUpdate.cpp:1554-1786`). Returns the
    /// chosen point and whether the crusher has crossed it (and is close enough).
    fn crush_target_and_test(&self, obj: ObjectId, other: ObjectId, front_crushed: bool, back_crushed: bool) -> (CrushEnum, bool) {
        let (Some(a), Some(b)) = (self.objects.get(&obj), self.objects.get(&other)) else {
            return (CrushEnum::NoCrush, false);
        };
        let dir = (cos(a.angle), sin(a.angle));
        let radius = b.template.geometry.major_radius / 2.0;
        let offset = (cos(b.angle) * radius, sin(b.angle) * radius);
        let target = if front_crushed || back_crushed {
            if front_crushed {
                CrushEnum::BackEnd
            } else {
                CrushEnum::FrontEnd
            }
        } else {
            crush_target_three_points(a.position, b.position, dir, offset)
        };
        // Comparison point for the chosen target.
        let comparison = match target {
            CrushEnum::Total | CrushEnum::NoCrush => b.position,
            CrushEnum::FrontEnd => Coord3D::new(b.position.x + offset.0, b.position.y + offset.1, b.position.z),
            CrushEnum::BackEnd => Coord3D::new(b.position.x - offset.0, b.position.y - offset.1, b.position.z),
        };
        let dx = comparison.x - a.position.x;
        let dy = comparison.y - a.position.y;
        let dot = dir.0 * dx + dir.1 * dy;
        let distance_squared = dx * dx + dy * dy;
        let too_far_squared = 2.25 * radius * radius;
        let crush_it = dot < 0.0 && distance_squared < too_far_squared;
        (target, crush_it)
    }

    /// `CrushDie::crushLocationCheck` (`CrushDie.cpp:49`): the closest crush point to the crusher,
    /// using straight-line distance (the flags `CrushDie::onDie` sets).
    fn crush_location_check(&self, obj: ObjectId, other: ObjectId) -> CrushEnum {
        let (Some(a), Some(b)) = (self.objects.get(&obj), self.objects.get(&other)) else {
            return CrushEnum::NoCrush;
        };
        let front_crushed = b.body.as_ref().is_some_and(|bd| bd.front_crushed());
        let back_crushed = b.body.as_ref().is_some_and(|bd| bd.back_crushed());
        let radius = b.template.geometry.major_radius * 0.5;
        let offset = (cos(b.angle) * radius, sin(b.angle) * radius);
        let dist_sq = |p: Coord3D| {
            let dx = p.x - a.position.x;
            let dy = p.y - a.position.y;
            dx * dx + dy * dy
        };
        let mut best = 99999.0f32;
        let mut retval = CrushEnum::NoCrush;
        if !front_crushed && !back_crushed {
            retval = CrushEnum::Total;
            best = dist_sq(b.position);
        }
        if !front_crushed {
            let p = Coord3D::new(b.position.x + offset.0, b.position.y + offset.1, b.position.z);
            let dist = dist_sq(p);
            if dist < best {
                retval = if back_crushed { CrushEnum::Total } else { CrushEnum::FrontEnd };
                best = dist;
            }
        }
        if !back_crushed {
            let p = Coord3D::new(b.position.x - offset.0, b.position.y - offset.1, b.position.z);
            let dist = dist_sq(p);
            if dist < best {
                retval = if front_crushed { CrushEnum::Total } else { CrushEnum::BackEnd };
            }
        }
        retval
    }

    /// `SquishCollide::onCollide` (`SquishCollide.cpp:62`): a moving enemy vehicle crushes this
    /// object (an infantry). Uses a 1.0 crush radius so the tank must actually hit.
    fn squish_collide_on_collide(&mut self, obj: ObjectId, other: ObjectId) {
        let (other_crusher, allied, other_has_physics) = match (self.objects.get(&obj), self.objects.get(&other)) {
            (Some(a), Some(b)) => (
                b.template.crush.crusher_level > 0,
                self.relationship(b.player, a.player) == crate::damage::Relationship::Allies,
                // `otherPhysics == nullptr` -> return (`SquishCollide.cpp:94`).
                b.template.physics.is_some(),
            ),
            _ => return,
        };
        if !other_crusher || allied || !other_has_physics {
            return;
        }
        // `geomCollidesWithGeom(other, otherGeom, otherAngle, self, 1.0 circle, selfAngle)`.
        let (Some(a), Some(b)) = (self.objects.get(&obj), self.objects.get(&other)) else { return };
        let self_geom = CollideGeom {
            pos: a.position,
            angle: a.angle,
            shape: GeometryShape::Cylinder,
            major: 1.0,
            minor: 1.0,
            height: 1.0,
            radius: 1.0,
        };
        let g = &b.template.geometry;
        let other_geom = CollideGeom {
            pos: b.position,
            angle: b.angle,
            shape: g.shape,
            major: g.major_radius,
            minor: g.minor_radius,
            height: g.height,
            radius: g.bounding_circle_radius(),
        };
        if !geom_collides(&other_geom, &self_geom) {
            return;
        }
        // Only squish if the tank is moving toward the victim (`to · vel > 0`).
        let (vel, other_pos, my_pos) = match (self.objects.get(&other), self.objects.get(&obj)) {
            (Some(ob), Some(oa)) => (ob.physics.as_ref().map(|p| p.velocity).unwrap_or(Coord3D::ZERO), ob.position, oa.position),
            _ => return,
        };
        let to = Coord3D::new(my_pos.x - other_pos.x, my_pos.y - other_pos.y, my_pos.z - other_pos.z);
        if to.x * vel.x + to.y * vel.y <= 0.0 {
            return;
        }
        self.damage_object_with_death(
            obj,
            DamageInfo { source: other, amount: HUGE_DAMAGE_AMOUNT, kind: DamageType::Crush },
            DeathType::Crushed,
        );
    }

    /// `FireWeaponCollide::onCollide` (`FireWeaponCollide.cpp:75`): shoot the colliding object.
    fn fire_weapon_collide_on_collide(&mut self, obj: ObjectId, other: ObjectId, t: &FireWeaponCollideTemplate) {
        // `if (other == nullptr) return; //Don't shoot the ground` (`FireWeaponCollide.cpp:77-78`): the
        // test comes before `shouldFireWeapon`, so a landing never consumes the module's fire-once flag.
        if other == ObjectId::INVALID {
            return;
        }
        // `FireWeaponCollide::shouldFireWeapon` (`FireWeaponCollide.cpp:94`), on `getStatusBits`.
        let status = self.objects.get(&obj).map_or(0, |o| o.status_mask(self.frame));
        if !fire_weapon_status_allows(status, t.required_status, t.forbidden_status) {
            return;
        }
        if t.fire_once && self.objects.get(&obj).is_some_and(|o| o.behavior.fire_collide_fired) {
            return;
        }
        if let Some(o) = self.objects.get_mut(&obj) {
            o.behavior.fire_collide_fired = true;
        }
        let pos = match self.objects.get(&other) {
            Some(o) => o.position,
            None => return,
        };
        self.fire_temp_weapon_at(obj, t.weapon.clone(), pos);
    }

    /// `CrateCollide::onCollide` (`CrateCollide.cpp:106`) and `isValidToExecute` (`:142`).
    fn crate_collide_on_collide(&mut self, crate_id: ObjectId, other: ObjectId, t: &CrateCollideTemplate) -> bool {
        if !self.crate_is_valid_to_execute(crate_id, other, t) {
            return false;
        }
        if self.execute_crate_behavior(crate_id, other, t) {
            self.destroy_object(crate_id);
            true
        } else {
            false
        }
    }

    /// `CrateCollide::isValidToExecute` (`CrateCollide.cpp:142`) plus each subclass's override.
    fn crate_is_valid_to_execute(&self, crate_id: ObjectId, other: ObjectId, t: &CrateCollideTemplate) -> bool {
        let (Some(crate_obj), Some(other_obj)) = (self.objects.get(&crate_id), self.objects.get(&other)) else { return false };
        // The ground never picks up a crate.
        // Nothing Neutral can pick up any type of crate.
        if self.neutral_player() == Some(other_obj.player) {
            return false;
        }
        let valid_building_attempt = t.building_pickup && other_obj.template.is_kind_of(kind_of::STRUCTURE);
        // Must be a "Unit" type thing (real Game Object) unless it is a building pickup.
        if other_obj.ai_handle.is_none() && !valid_building_attempt {
            return false;
        }
        if !crate_kindof_ok(other_obj.template.kind_of, t.required_kind_of, t.forbidden_kind_of) {
            return false;
        }
        if other_obj.is_dying() {
            return false;
        }
        // Crates cannot be claimed while in the air, except by buildings.
        if self.is_above_terrain(crate_id) && !valid_building_attempt {
            return false;
        }
        // `if (getObject()->isDestroyed() && !allowMultiPickup) return FALSE;`
        if crate_obj.destroyed && !t.allow_multi_pickup {
            return false;
        }
        if t.forbid_owner_player && crate_obj.player == other_obj.player {
            return false;
        }
        if t.human_only {
            // The sim does not model player types; the shipped skirmish crates never set this.
        }
        if let Some(science) = &t.pickup_science {
            if !self.player_has_science(other_obj.player, science) {
                return false;
            }
        }
        if other_obj.template.is_collide_kind_of(crate::template::collide_kind::PARACHUTE) {
            return false;
        }
        // Subclass-specific vetoes.
        match &t.effect {
            CrateEffect::Veterancy { effect_range, adds_owner_veterancy, is_pilot } => {
                if other_obj.is_dying() {
                    return false;
                }
                if self.is_significantly_above_terrain(other) {
                    return false;
                }
                let levels = if *adds_owner_veterancy { crate_obj.experience.level.index() as i32 } else { 1 };
                if levels <= 0 {
                    return false;
                }
                // `ExperienceTracker::canGainExpForLevel`: not already at the top.
                if !other_obj.experience.is_trainable
                    || other_obj.experience.level.index() + levels as usize >= crate::experience::VeterancyLevel::COUNT
                {
                    return false;
                }
                if *is_pilot {
                    if crate_obj.player != other_obj.player {
                        return false;
                    }
                    // `other->isUsingAirborneLocomotor()` (`VeterancyCrateCollide.cpp:112`): the current locomotor's
                    // surfaces include AIR, which `is_airborne()` (Thrust / Wings kinds) does not test.
                    if self.is_airborne_locomotor(other) {
                        return false;
                    }
                }
                let _ = effect_range;
            }
            CrateEffect::Salvage { .. } => {
                if !other_obj.template.is_kind_of(kind_of::SALVAGER) {
                    return false;
                }
            }
            CrateEffect::ConvertToCarBomb => {
                if other_obj.is_dying() {
                    return false;
                }
                if other_obj.template.is_kind_of(kind_of::AIRCRAFT) || other_obj.template.is_collide_kind_of(crate::template::collide_kind::BOAT) {
                    return false;
                }
                if other_obj.weapon_set_flags & WEAPONSET_CARBOMB != 0 {
                    return false;
                }
                if !other_obj.has_carbomb_weapon_set() {
                    return false;
                }
            }
            CrateEffect::ConvertToHijackedVehicle => {
                if other_obj.is_dying() {
                    return false;
                }
                if other_obj.template.is_collide_kind_of(crate::template::collide_kind::IMMUNE_TO_CAPTURE)
                    || other_obj.template.is_kind_of(kind_of::AIRCRAFT)
                    || other_obj.template.is_collide_kind_of(crate::template::collide_kind::BOAT)
                    || other_obj.template.is_kind_of(kind_of::DRONE)
                {
                    return false;
                }
                if other_obj.status_bits & crate::object::status::HIJACKED != 0 {
                    return false;
                }
                if self.relationship(crate_obj.player, other_obj.player) != crate::damage::Relationship::Enemies {
                    return false;
                }
                if other_obj.template.is_kind_of(kind_of::TRANSPORT) && !other_obj.contained.is_empty() {
                    return false;
                }
            }
            CrateEffect::SabotageCommandCenter => {
                if !self.sabotage_common_ok(crate_obj, other_obj) {
                    return false;
                }
                if !other_obj.template.is_kind_of(kind_of::COMMANDCENTER) {
                    return false;
                }
            }
            CrateEffect::SabotageSuperweapon => {
                if !self.sabotage_common_ok(crate_obj, other_obj) {
                    return false;
                }
                if !other_obj.template.is_kind_of(kind_of::FS_SUPERWEAPON) && !other_obj.template.is_kind_of(kind_of::FS_STRATEGY_CENTER) {
                    return false;
                }
            }
            CrateEffect::SabotageFakeBuilding => {
                if !self.sabotage_common_ok(crate_obj, other_obj) {
                    return false;
                }
                if !other_obj.template.is_collide_kind_of(crate::template::collide_kind::FS_FAKE) {
                    return false;
                }
            }
            CrateEffect::SabotageInternetCenter { .. } => {
                if !self.sabotage_common_ok(crate_obj, other_obj) {
                    return false;
                }
                if !other_obj.template.is_kind_of(kind_of::FS_INTERNET_CENTER) {
                    return false;
                }
            }
            CrateEffect::SabotageMilitaryFactory { .. } => {
                if !self.sabotage_common_ok(crate_obj, other_obj) {
                    return false;
                }
                if other_obj.template.is_collide_kind_of(crate::template::collide_kind::AIRCRAFT_CARRIER) {
                    return false;
                }
                if !other_obj.template.is_collide_kind_of(crate::template::collide_kind::FS_BARRACKS)
                    && !other_obj.template.is_collide_kind_of(crate::template::collide_kind::FS_WARFACTORY)
                    && !other_obj.template.is_collide_kind_of(crate::template::collide_kind::FS_AIRFIELD)
                {
                    return false;
                }
            }
            CrateEffect::SabotagePowerPlant { .. } => {
                if !self.sabotage_common_ok(crate_obj, other_obj) {
                    return false;
                }
                if !other_obj.template.is_kind_of(kind_of::FS_POWER) {
                    return false;
                }
            }
            CrateEffect::SabotageSupplyCenter { .. } => {
                if !self.sabotage_common_ok(crate_obj, other_obj) {
                    return false;
                }
                if !other_obj.template.is_collide_kind_of(crate::template::collide_kind::FS_SUPPLY_CENTER) {
                    return false;
                }
            }
            CrateEffect::SabotageSupplyDropzone { .. } => {
                if !self.sabotage_common_ok(crate_obj, other_obj) {
                    return false;
                }
                if !other_obj.template.is_collide_kind_of(crate::template::collide_kind::FS_SUPPLY_DROPZONE) {
                    return false;
                }
            }
            _ => {}
        }
        true
    }

    /// `CrateCollide::wouldLikeToCollideWith` (`CrateCollide.h:92`, which is `isValidToExecute`)
    /// restricted to one enter-type subclass, as the `ActionManager` entry queries key on it
    /// (`canHijackVehicle` / `canConvertObjectToCarBomb` / `canSabotageBuilding`).
    pub(crate) fn would_like_to_enter_collide(&self, id: ObjectId, other: ObjectId, kind: EnterCollide) -> bool {
        let Some(ts) = self.objects.get(&id).map(|o| o.template.crate_collides.clone()) else { return false };
        ts.iter().any(|t| enter_collide_matches(&t.effect, kind) && self.crate_is_valid_to_execute(id, other, t))
    }

    /// The `isValidToExecute` tail shared by every `Sabotage*CrateCollide`: not dead, enemy, not
    /// under construction or being sold.
    fn sabotage_common_ok(&self, crate_obj: &crate::object::Object, other: &crate::object::Object) -> bool {
        if other.is_dying() {
            return false;
        }
        if other.under_construction {
            return false;
        }
        self.relationship(crate_obj.player, other.player) == crate::damage::Relationship::Enemies
    }

    /// The `ai->getGoalObject() == other` gate every convert/sabotage crate shares
    /// (`CrateCollide.cpp` subclasses). The sim's AI records the attacked object on the attack
    /// order; a move-only order has no object goal and fails the gate, as in the engine.
    fn ai_goal_is(&self, id: ObjectId, other: ObjectId) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        if o.ai.machine.goal_object == other {
            return true;
        }
        matches!(o.combat.attack.as_ref().map(|a| a.target), Some(crate::combat::AttackTarget::Object(t)) if t == other)
    }

    /// `executeCrateBehavior` for every shipped subclass (`CrateCollide.cpp` subclasses). Returns
    /// `true` when the crate object should be destroyed.
    fn execute_crate_behavior(&mut self, crate_id: ObjectId, other: ObjectId, t: &CrateCollideTemplate) -> bool {
        let (crate_player, crate_pos, crate_level) = match self.objects.get(&crate_id) {
            Some(o) => (o.player, o.position, o.experience.level.index()),
            None => return false,
        };
        let picker_player = self.objects.get(&other).map(|o| o.player);
        match &t.effect {
            CrateEffect::Money { money_provided, upgrade_boost } => {
                let boost: i32 = upgrade_boost
                    .iter()
                    .find(|(n, _)| {
                        picker_player.is_some_and(|p| self.player_upgrade_set(p).is_some_and(|s| s.contains(n)))
                    })
                    .map_or(0, |(_, b)| *b);
                let amount = (*money_provided as i32 + boost).max(0) as u32;
                if let Some(p) = picker_player.and_then(|p| self.players.get_mut(&p)) {
                    p.deposit(amount);
                }
                // `MoneyCrateCollide::executeCrateBehavior` (MoneyCrateCollide.cpp:61): `addMoneyEarned(money)`.
                if let Some(p) = picker_player {
                    self.score_money_earned(p, amount);
                }
                true
            }
            CrateEffect::Heal => {
                // `Player::healAllObjects`: heal every object the player owns.
                if let Some(pp) = picker_player {
                    let ids: Vec<ObjectId> = self.objects.values().filter(|o| o.player == pp && !o.destroyed).map(|o| o.id).collect();
                    for id in ids {
                        let max = self.objects.get(&id).and_then(|o| o.body.as_ref()).map_or(0.0, |b| b.max_health());
                        self.heal_object(id, max);
                    }
                }
                true
            }
            CrateEffect::Veterancy { effect_range, .. } => {
                if !self.ai_goal_is(crate_id, other) {
                    return false;
                }
                if *effect_range == 0 {
                    self.add_veterancy(other);
                } else {
                    let ids: Vec<ObjectId> = self
                        .objects
                        .values()
                        .filter(|o| Some(o.player) == picker_player && !o.destroyed && o.position.dist_2d(&crate_pos) <= *effect_range as f32)
                        .map(|o| o.id)
                        .collect();
                    for id in ids {
                        self.add_veterancy(id);
                    }
                }
                true
            }
            CrateEffect::Unit { unit_count, unit_name } => {
                if let (Some(pp), Some(tt)) = (picker_player, self.template_source.as_ref().and_then(|s| s.find_template(unit_name))) {
                    for i in 0..*unit_count {
                        let pos = Coord3D::new(crate_pos.x + i as f32 * 5.0, crate_pos.y, crate_pos.z);
                        self.create_object(&tt, pos, 0.0, pp);
                    }
                    true
                } else {
                    false
                }
            }
            CrateEffect::Salvage { weapon_chance, level_chance, money_chance, min_money, max_money } => {
                // `SalvageCrateCollide::executeCrateBehavior` has no AI-goal gate (unlike the
                // convert/sabotage/veterancy subclasses).
                // `eligibleForWeaponSet` / `eligibleForArmorSet` / `eligibleForLevel` / money.
                let (weapon_salvager, armor_salvager, max_level, trainable) = match self.objects.get(&other) {
                    Some(o) => (
                        o.template.is_kind_of(kind_of::WEAPON_SALVAGER),
                        o.template.is_collide_kind_of(crate::template::collide_kind::ARMOR_SALVAGER),
                        o.experience.level.index() == crate::experience::VeterancyLevel::COUNT - 1,
                        o.experience.is_trainable,
                    ),
                    None => return false,
                };
                let weapon_ok = weapon_salvager && (self.objects.get(&other).map_or(0, |o| o.weapon_set_flags) & WEAPONSET_CRATEUPGRADE_TWO == 0);
                let armor_ok = armor_salvager && (self.objects.get(&other).map_or(0, |o| o.armor_set_flags) & ARMORSET_CRATE_UPGRADE_TWO == 0);
                let level_ok = !max_level && trainable;
                if armor_ok {
                    self.do_armor_set(other);
                } else if weapon_ok && salvage_chance(*weapon_chance, &mut self.rng) {
                    self.do_weapon_set(other);
                } else if level_ok && salvage_chance(*level_chance, &mut self.rng) {
                    self.add_veterancy(other);
                } else {
                    let amount = if *max_money > *min_money {
                        *min_money + self.rng.value(0, *max_money - *min_money)
                    } else {
                        *min_money
                    };
                    if let Some(p) = picker_player.and_then(|p| self.players.get_mut(&p)) {
                        p.deposit(amount.max(0) as u32);
                    }
                    // `SalvageCrateCollide` (SalvageCrateCollide.cpp:249): `addMoneyEarned(money)`.
                    if let Some(p) = picker_player {
                        self.score_money_earned(p, amount.max(0) as u32);
                    }
                    let _ = money_chance;
                }
                true
            }
            CrateEffect::Shroud => {
                if let Some(pp) = picker_player {
                    self.shroud.reveal_map_permanently(pp);
                }
                true
            }
            CrateEffect::ConvertToCarBomb => {
                if !self.ai_goal_is(crate_id, other) {
                    return false;
                }
                // `other->setWeaponSetFlag( WEAPONSET_CARBOMB )` (`ConvertToCarBombCrateCollide.cpp:129`): re-selects the weapons.
                self.set_weapon_set_flag(other, WEAPONSET_CARBOMB);
                // `other->defect(getObject()->getControllingPlayer()->getDefaultTeam(), 0)` (:133): the switch,
                // with no undetected-defector time, and an AI idles (`defect_l193.rs`).
                self.object_defect(other, crate_player, 0);
                // `other->setStatus(OBJECT_STATUS_IS_CARBOMB)` comes after the defect (:142), so it is set last.
                if let Some(o) = self.objects.get_mut(&other) {
                    o.status_bits |= crate::object::status::IS_CARBOMB;
                }
                // The terrorist's veterancy transfers to the car.
                self.set_veterancy_level(other, crate::experience::VeterancyLevel::from_index(crate_level));
                true
            }
            CrateEffect::ConvertToHijackedVehicle => {
                if !self.ai_goal_is(crate_id, other) {
                    return false;
                }
                let target_can_eject = self.objects.get(&other).is_some_and(|o| o.template.eject_pilot.is_some());
                if let Some(o) = self.objects.get_mut(&other) {
                    o.status_bits |= crate::object::status::HIJACKED;
                }
                self.set_object_team(other, crate_player);
                // `targetAI->aiMoveToPosition(other->getPosition(), CMD_FROM_AI); targetAI->aiIdle(CMD_FROM_AI);`
                // (`ConvertToHijackedVehicleCrateCollide.cpp:167-168`): a `CMD_FROM_AI` move of a busy unit is
                // the 20-second temporary `AI_MOVE_TO` (`AIUpdate.cpp:2900-2909`), and the idle that follows
                // clears the machine and enters `AI_IDLE` (`:3092-3117`). `privateIdle` does not clear the
                // path: the completed-move block of the unit's next update does (`:1035-1041`).
                let here = self.objects.get(&other).map_or(Coord3D::ZERO, |o| o.position);
                self.ai_move_to_position_cmd(other, here, crate::ai_states::CommandSource::Ai);
                self.ai_idle(other, crate::ai_states::CommandSource::Ai);
                // Highest veterancy of the two wins.
                let max_level = crate_level.max(self.objects.get(&other).map_or(0, |o| o.experience.level.index()));
                self.set_veterancy_level(other, crate::experience::VeterancyLevel::from_index(max_level));
                // `ConvertToHijackedVehicleCrateCollide::executeCrateBehavior`
                // (`ConvertToHijackedVehicleCrateCollide.cpp:196`): a vehicle that ejects a pilot
                // lets the hijacker ride inside (`HijackerUpdate`); any other vehicle consumes him.
                if target_can_eject {
                    let now_airborne = self.is_above_terrain(other);
                    if let Some(o) = self.objects.get_mut(&crate_id) {
                        o.behavior.hijacker_vehicle = Some(other);
                        o.behavior.hijacker_eject_pos = o.position;
                        o.behavior.hijacker_was_airborne = now_airborne;
                    }
                    // `ai->aiIdle(CMD_FROM_AI)` prevents re-entering the vehicle (`:234`); returning false keeps
                    // the hijacker. (The engine also copies the hijacker's vision/shroud ranges onto the
                    // vehicle; the sim has no per-object vision override, so that part is reduced.)
                    self.ai_idle(crate_id, crate::ai_states::CommandSource::Ai);
                    false
                } else {
                    true
                }
            }
            CrateEffect::SabotageCommandCenter => {
                if !self.ai_goal_is(crate_id, other) {
                    return false;
                }
                self.reset_all_special_powers(other);
                true
            }
            CrateEffect::SabotageSuperweapon => {
                if !self.ai_goal_is(crate_id, other) {
                    return false;
                }
                self.reset_all_special_powers(other);
                true
            }
            CrateEffect::SabotageFakeBuilding => {
                if !self.ai_goal_is(crate_id, other) {
                    return false;
                }
                let max = self.objects.get(&other).and_then(|o| o.body.as_ref()).map_or(0.0, |b| b.max_health());
                self.damage_object_with_death(
                    other,
                    DamageInfo { source: crate_id, amount: max, kind: DamageType::Unresistable },
                    DeathType::Detonated,
                );
                true
            }
            CrateEffect::SabotageInternetCenter { sabotage_frames } => {
                if !self.ai_goal_is(crate_id, other) {
                    return false;
                }
                let frame = self.frame + *sabotage_frames;
                self.set_disabled_until(other, crate::disabled::DisabledType::Hacked, frame);
                // Disable every internet center's spy vision for the same window.
                let centers: Vec<ObjectId> = self
                    .objects
                    .values()
                    .filter(|o| o.player == picker_player.map_or(255, |p| p) || o.template.is_kind_of(kind_of::FS_INTERNET_CENTER))
                    .filter(|o| o.template.is_kind_of(kind_of::FS_INTERNET_CENTER))
                    .map(|o| o.id)
                    .collect();
                for c in centers {
                    self.disable_spy_vision(c, frame);
                }
                true
            }
            CrateEffect::SabotageMilitaryFactory { sabotage_frames } => {
                if !self.ai_goal_is(crate_id, other) {
                    return false;
                }
                self.set_disabled_until(other, crate::disabled::DisabledType::Hacked, self.frame + *sabotage_frames);
                true
            }
            CrateEffect::SabotagePowerPlant { power_sabotage_frames } => {
                if !self.ai_goal_is(crate_id, other) {
                    return false;
                }
                if let Some(pp) = self.objects.get(&other).map(|o| o.player) {
                    self.power_sabotaged_till.insert(pp, self.frame + *power_sabotage_frames);
                }
                true
            }
            CrateEffect::SabotageSupplyCenter { steal_cash_amount } => {
                if !self.ai_goal_is(crate_id, other) {
                    return false;
                }
                self.steal_cash(crate_player, other, *steal_cash_amount);
                true
            }
            CrateEffect::SabotageSupplyDropzone { steal_cash_amount } => {
                if !self.ai_goal_is(crate_id, other) {
                    return false;
                }
                self.steal_cash(crate_player, other, *steal_cash_amount);
                true
            }
            CrateEffect::Other => false,
        }
    }

    /// `Money::withdraw`/`deposit` for the sabotage cash theft (`SabotageSupplyCenter.cpp`).
    fn steal_cash(&mut self, thief: crate::object::PlayerIndex, target: ObjectId, desired: u32) {
        let Some(victim_player) = self.objects.get(&target).map(|o| o.player) else { return };
        if victim_player == thief {
            return;
        }
        let taken = self.players.get_mut(&victim_player).map_or(0, |p| p.withdraw(desired));
        if taken > 0 {
            if let Some(p) = self.players.get_mut(&thief) {
                p.deposit(taken);
            }
            // `SabotageSupplyCenter` / `SabotageSupplyDropzone` (SabotageSupplyCenterCrateCollide.cpp:152):
            // `controller->getScoreKeeper()->addMoneyEarned(cash)`.
            self.score_money_earned(thief, taken);
        }
    }

    /// `SalvageCrateCollide::doWeaponSet` (`SalvageCrateCollide.cpp:198`): ONE -> TWO, else set ONE.
    /// Each flag change goes through `setWeaponSetFlag` / `clearWeaponSetFlag`, which re-select the set.
    fn do_weapon_set(&mut self, id: ObjectId) {
        let has_one = self.objects.get(&id).is_some_and(|o| o.weapon_set_flags & WEAPONSET_CRATEUPGRADE_ONE != 0);
        if has_one {
            self.clear_weapon_set_flag(id, WEAPONSET_CRATEUPGRADE_ONE);
            self.set_weapon_set_flag(id, WEAPONSET_CRATEUPGRADE_TWO);
        } else {
            self.set_weapon_set_flag(id, WEAPONSET_CRATEUPGRADE_ONE);
        }
    }

    /// `SalvageCrateCollide::doArmorSet` (`SalvageCrateCollide.cpp:212`): ONE -> TWO, else set ONE.
    /// The model-condition swap (`MODELCONDITION_ARMORSET_CRATEUPGRADE_*`) is client state and not modelled here.
    fn do_armor_set(&mut self, id: ObjectId) {
        let has_one = self.objects.get(&id).is_some_and(|o| o.armor_set_flags & ARMORSET_CRATE_UPGRADE_ONE != 0);
        if has_one {
            self.clear_armor_set_flag(id, ARMORSET_CRATE_UPGRADE_ONE);
            self.set_armor_set_flag(id, ARMORSET_CRATE_UPGRADE_TWO);
        } else {
            self.set_armor_set_flag(id, ARMORSET_CRATE_UPGRADE_ONE);
        }
    }

    /// `Sabotage*::executeCrateBehavior`'s special-power reset: restart every power's recharge.
    fn reset_all_special_powers(&mut self, target: ObjectId) {
        let now = self.frame;
        let powers: Vec<String> = self
            .objects
            .get(&target)
            .map(|o| o.template.special_powers.iter().map(|p| p.power.clone()).collect())
            .unwrap_or_default();
        for name in powers {
            let reload = self.special_power_reload(&name);
            let shared = self.special_powers.get(&name).is_some_and(|t| t.shared_synced_timer);
            if shared {
                if let Some(pp) = self.objects.get(&target).map(|o| o.player) {
                    self.shared_power_ready.insert((pp, name), now + reload);
                }
            } else if let Some(o) = self.objects.get_mut(&target) {
                o.special_power_ready.insert(name, now + reload);
            }
        }
    }

    /// `SpyVisionUpdate::setDisabledUntilFrame`.
    fn disable_spy_vision(&mut self, target: ObjectId, frame: u32) {
        if let Some(o) = self.objects.get_mut(&target) {
            o.superweapon.spy_vision_disabled_until = frame;
        }
    }

    /// `Object::isAboveTerrain`.
    fn is_above_terrain(&self, id: ObjectId) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        let g = self.terrain.surface_height(o.position.x, o.position.y, o.position.z);
        o.position.z > g + 0.001
    }

    /// `Object::isSignificantlyAboveTerrain` (`height > -(3*3) * gravity`): the shared helper in `combat.rs`.
    fn is_significantly_above_terrain(&self, id: ObjectId) -> bool {
        self.significantly_above_terrain(id)
    }

    fn is_immobile(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.template.kind_of & kind_of::IMMOBILE != 0)
    }
}

/// `KindOfMask` test (`Object::isKindOfMulti`): all required bits, no forbidden bits.
fn crate_kindof_ok(kind_of: u64, required: u64, forbidden: u64) -> bool {
    kind_of & required == required && kind_of & forbidden == 0
}

/// The enter-type `CrateCollide` subclass an `ActionManager` entry query tests (the engine's
/// `isHijackedVehicleCrateCollide` / `isCarBombCrateCollide` / `isSabotageBuildingCrateCollide`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EnterCollide {
    HijackedVehicle,
    CarBomb,
    SabotageBuilding,
}

/// Maps a sim [`CrateEffect`] onto the C++ `CrateCollide` subclass kind it was built from.
fn enter_collide_matches(effect: &CrateEffect, kind: EnterCollide) -> bool {
    match kind {
        EnterCollide::HijackedVehicle => matches!(effect, CrateEffect::ConvertToHijackedVehicle),
        EnterCollide::CarBomb => matches!(effect, CrateEffect::ConvertToCarBomb),
        EnterCollide::SabotageBuilding => matches!(
            effect,
            CrateEffect::SabotageCommandCenter
                | CrateEffect::SabotageFakeBuilding
                | CrateEffect::SabotageInternetCenter { .. }
                | CrateEffect::SabotageMilitaryFactory { .. }
                | CrateEffect::SabotagePowerPlant { .. }
                | CrateEffect::SabotageSuperweapon
                | CrateEffect::SabotageSupplyCenter { .. }
                | CrateEffect::SabotageSupplyDropzone { .. }
        ),
    }
}

/// `FireWeaponCollide::shouldFireWeapon` status test (`FireWeaponCollide.cpp:94-107`): every
/// `RequiredStatus` bit must be set (`testForAll`), and no `ForbiddenStatus` bit may be
/// (`testForAny`). `status` is the object's [`Object::status_mask`](crate::object::Object::status_mask).
pub(crate) fn fire_weapon_status_allows(status: u64, required: u64, forbidden: u64) -> bool {
    status & required == required && status & forbidden == 0
}

/// Lane 195 (wave 23): the partition's shape tests are the C++ ones (`xy_collideTest_Rect_Rect` corner containment,
/// `xy_collideTest_Rect_Circle`'s square). AN3 seed 1: Ranger 258 (cylinder, radius 7) and the Humvee 253 (box 14 x 7 at
/// angle -0.1112) touch from C++'s update 3186 (`ZR_L175_ENTER f=3186 me=258 other=253`, which reads the positions the
/// trace writes at f3187). The exact clamp test first found the contact at f3190 (the port's `processCollision` for 258 came
/// at its update 3189, C++'s at update 3186, `ZR_L175_ENTER`).
#[cfg(test)]
mod rect_shape_l195_tests {
    use super::*;

    fn shape(shape: GeometryShape, x: f32, y: f32, angle: f32, major: f32, minor: f32) -> CollideGeom {
        CollideGeom { pos: Coord3D::new(x, y, 0.0), angle, shape, major, minor, height: 10.0, radius: major }
    }

    /// Revert check: the exact clamp test (`rect_circle` before lane 195) finds no contact at 955.9346,404.5928 (it is
    /// first true at 954.2061,406.7493), so the second assert fails there. The first assert holds under both tests.
    #[test]
    fn a_box_touches_a_cylinder_by_the_square_corner_test_on_the_c_frame() {
        let humvee = shape(GeometryShape::Box, 934.2665, 415.0819, -0.1112, 14.0, 7.0);
        let ranger = |x: f32, y: f32| shape(GeometryShape::Cylinder, x, y, 2.2465, 7.0, 7.0);
        assert!(!geom_collides(&humvee, &ranger(956.5107, 403.8739)), "the trace's f3186 positions do not touch");
        assert!(geom_collides(&humvee, &ranger(955.9346, 404.5928)), "the trace's f3187 positions touch (C++ update 3186)");
    }

    /// Revert check: without the close-enough preflight (PartitionManager.cpp:663-675) this pair returns false. Both
    /// centres are at the origin, the smallest half-extent is 2, and no corner of either rect lies in the other, so the
    /// corner test alone says no. C++ `z_collideTest_Nonsphere_Nonsphere` declares contact (`sqr(2) > 0`).
    #[test]
    fn concentric_crossing_thin_rects_touch_by_the_close_enough_preflight() {
        let a = shape(GeometryShape::Box, 0.0, 0.0, 0.0, 14.0, 2.0);
        let b = shape(GeometryShape::Box, 0.0, 0.0, std::f32::consts::FRAC_PI_2, 14.0, 2.0);
        assert!(geom_collides(&a, &b));
    }

    /// The xy corner rule once the centres are farther apart than the smallest half-extent (`sqr(2) = 4 < 9`), so the
    /// preflight does not apply: the same crossing rects, 3 units apart, do not touch (no corner of either lies in the
    /// other). Revert check: a separating-axis test would call them a collision.
    #[test]
    fn crossing_thin_rects_three_apart_with_no_corner_inside_do_not_touch() {
        let a = shape(GeometryShape::Box, 0.0, 0.0, 0.0, 14.0, 2.0);
        let b = shape(GeometryShape::Box, 3.0, 0.0, std::f32::consts::FRAC_PI_2, 14.0, 2.0);
        assert!(!geom_collides(&a, &b));
    }
}

#[cfg(test)]
mod fire_weapon_status_tests {
    use super::fire_weapon_status_allows;
    use crate::object::status;

    #[test]
    fn required_bits_must_all_be_set() {
        let need = status::RIDER1 | status::AFLAME;
        assert!(!fire_weapon_status_allows(status::RIDER1, need, 0));
        assert!(!fire_weapon_status_allows(status::AFLAME, need, 0));
        assert!(fire_weapon_status_allows(status::RIDER1 | status::AFLAME | status::IMMOBILE, need, 0));
        // An empty requirement always passes.
        assert!(fire_weapon_status_allows(0, 0, 0));
    }

    #[test]
    fn any_forbidden_bit_blocks() {
        let forbid = status::RIDER2 | status::HIJACKED;
        assert!(fire_weapon_status_allows(status::RIDER1, 0, forbid));
        assert!(!fire_weapon_status_allows(status::RIDER1 | status::HIJACKED, 0, forbid));
        assert!(!fire_weapon_status_allows(status::RIDER2, 0, forbid));
    }
}

/// `SalvageCrateCollide::testWeaponChance` / `testLevelChance` (`SalvageCrateCollide.cpp:170`, `:184`):
/// exactly 1.0 passes without a draw. Any other chance draws `GameLogicRandomValueReal(0, 1)` and
/// compares, even when the chance is <= 0 (the draw then fails), so the RNG call order matches.
fn salvage_chance(chance: f32, rng: &mut crate::random::GameRandom) -> bool {
    if chance == 1.0 {
        return true;
    }
    rng.value_real(0.0, 1.0) < chance
}

#[cfg(test)]
mod salvage_chance_tests {
    use super::salvage_chance;
    use crate::random::GameRandom;

    #[test]
    fn full_chance_draws_nothing() {
        let mut rng = GameRandom::new(7);
        let before = rng.clone();
        assert!(salvage_chance(1.0, &mut rng));
        assert_eq!(rng, before, "100% passes without a random number");
    }

    #[test]
    fn other_chances_always_draw_once() {
        // `testWeaponChance` draws `GameLogicRandomValueReal(0, 1)` for 0.0 and for chances above 1.0 too.
        for chance in [0.0_f32, 0.5, 1.5] {
            let mut rng = GameRandom::new(7);
            let mut expected = rng.clone();
            let roll = expected.value_real(0.0, 1.0);
            let got = salvage_chance(chance, &mut rng);
            assert_eq!(got, roll < chance, "chance {chance}");
            assert_eq!(rng, expected, "chance {chance} consumes exactly one draw");
        }
    }
}

#[cfg(test)]
mod pilot_crate_tests {
    use super::*;
    use crate::modules2::{CrateCollideTemplate, CrateEffect};
    use crate::physics::PhysicsTemplate;
    use crate::template::{LocomotorKind, LocomotorParams, ObjectTemplate};
    use crate::TerrainLogic;
    use std::sync::Arc;

    fn vehicle(name: &str, loco: LocomotorParams) -> ObjectTemplate {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 8.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.physics = Some(PhysicsTemplate::default());
        t.locomotor = Some(loco);
        t.is_trainable = true; // `ExperienceTracker::canGainExpForLevel` needs a trainable unit
        t
    }

    /// `VeterancyCrateCollide::isValidToExecute` (`VeterancyCrateCollide.cpp:112`): `isUsingAirborneLocomotor`
    /// (`Object.cpp:3414`) refuses a pilot whose current locomotor's surfaces include AIR. A Thrust locomotor whose
    /// surfaces lack AIR is not airborne in that sense, so a pilot may board it.
    #[test]
    fn pilot_crate_refuses_only_an_airborne_locomotor() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        g.add_player(0, 0);
        let mut crate_t = ObjectTemplate::new("PilotCrate");
        crate_t.max_health = 10.0;
        crate_t.initial_health = 10.0;
        let crate_id = g.create_object(&Arc::new(crate_t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let mut ground_thrust = LocomotorParams::from_ini(LocomotorKind::Thrust, 60.0, 300.0, 300.0, 360.0);
        ground_thrust.surfaces_air = false;
        let ground = g.create_object(&Arc::new(vehicle("GroundThrust", ground_thrust)), Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
        let air = g.create_object(&Arc::new(vehicle("Jet", LocomotorParams::from_ini(LocomotorKind::Thrust, 60.0, 300.0, 300.0, 360.0))), Coord3D::new(70.0, 50.0, 0.0), 0.0, 0);
        let t = CrateCollideTemplate {
            required_kind_of: 0,
            forbidden_kind_of: 0,
            forbid_owner_player: false,
            building_pickup: false,
            human_only: false,
            allow_multi_pickup: false,
            pickup_science: None,
            effect: CrateEffect::Veterancy { effect_range: 0, adds_owner_veterancy: false, is_pilot: true },
        };
        let plain = g.create_object(&Arc::new(vehicle("Legs", LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 300.0, 300.0, 360.0))), Coord3D::new(80.0, 50.0, 0.0), 0.0, 0);
        assert!(g.crate_is_valid_to_execute(crate_id, plain, &t), "a ground vehicle takes the pilot");
        assert!(g.crate_is_valid_to_execute(crate_id, ground, &t), "no AIR surface: not an airborne locomotor");
        assert!(!g.crate_is_valid_to_execute(crate_id, air, &t), "an AIR locomotor refuses the pilot");
    }
}

#[cfg(test)]
mod parachute_pair_tests {
    use super::*;
    use crate::physics::PhysicsTemplate;
    use crate::template::ObjectTemplate;
    use crate::TerrainLogic;
    use std::sync::Arc;

    fn body(name: &str) -> ObjectTemplate {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 8.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.physics = Some(PhysicsTemplate::default());
        t
    }

    /// `PhysicsBehavior::onCollide` (`PhysicsUpdate.cpp:1190-1193`): two parachutists never collide.
    /// The control (no status) is the push-apart test in `tests/collide.rs`.
    #[test]
    fn two_parachutists_do_not_push_each_other() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        let a = g.create_object(&Arc::new(body("A")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let b = g.create_object(&Arc::new(body("B")), Coord3D::new(101.0, 100.0, 0.0), 0.0, 1);
        for id in [a, b] {
            g.objects.get_mut(&id).unwrap().status_bits |= crate::object::status::PARACHUTING;
        }
        g.run_frames(5);
        let pa = g.find_object(a).unwrap().position;
        let pb = g.find_object(b).unwrap().position;
        assert!(pa.dist_2d(&pb) < 1.5, "the parachutists overlapped: {pa:?} vs {pb:?}");
    }
}
