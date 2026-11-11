//! Projectile streams: `ProjectileStreamUpdate` (GameLogic/Object/Update/ProjectileStreamUpdate.cpp)
//! and the weapon hook that feeds it (`Weapon::newProjectileFired`, Weapon.cpp:2504-2531).
//!
//! A weapon whose `ProjectileStreamName` is set (the toxin truck, the flamethrowers, the hazmat
//! cleaner) keeps one stream object per weapon. Each projectile it fires is appended to the stream's
//! circular list of projectile ids; a target change inserts a hole (`INVALID_ID`) so the drawn strand
//! breaks there. The stream is drawn by `W3DProjectileStreamDraw` from the projectile positions, so
//! the sim only keeps the ids and returns the points (`getAllPoints`).
//!
//! The original stream is an `Object`. The sim keeps it as a lightweight record next to the
//! projectiles (no template module, no object-list slot), so it never affects AI target selection.
//! It is hashed into the CRC only while a stream exists.

use std::collections::BTreeMap;

use crate::crc::Crc;
use crate::logic::GameLogic;
use crate::math::{sqrt, Coord3D};
use crate::object::ObjectId;
use crate::template::{kind_of, Geometry, GeometryShape};

/// `ProjectileStreamUpdate.h:40` `MAX_PROJECTILE_STREAM`.
pub const MAX_PROJECTILE_STREAM: usize = 20;

/// `ProjectileStreamUpdate`'s state: the circular id list and the target tracking that breaks it.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectileStream {
    /// Sim-side handle (not an `ObjectId`: the stream is not an object here).
    pub id: u32,
    /// The stream object's template (`ProjectileStreamName`), whose `W3DProjectileStreamDraw` the client reads.
    pub template_name: String,
    /// `m_owningObject`: the first shooter. `INVALID` until the first projectile.
    pub owner: ObjectId,
    /// The stream object's position: `Weapon::newProjectileFired` sets it to the firing object each shot.
    pub position: Coord3D,
    /// `m_projectileIDs`: `None` is `INVALID_ID`, a hole.
    ids: [Option<u32>; MAX_PROJECTILE_STREAM],
    /// `m_nextFreeIndex`.
    next_free: usize,
    /// `m_firstValidIndex`.
    first_valid: usize,
    /// `m_targetObject` (`INVALID` while shooting a position).
    target_object: ObjectId,
    /// `m_targetPosition` (zero while shooting an object; the zero vector also reads as "no position").
    target_position: Coord3D,
}

impl ProjectileStream {
    /// `ProjectileStreamUpdate::ProjectileStreamUpdate`.
    pub fn new(id: u32, template_name: String, position: Coord3D) -> Self {
        ProjectileStream {
            id,
            template_name,
            owner: ObjectId::INVALID,
            position,
            ids: [None; MAX_PROJECTILE_STREAM],
            next_free: 0,
            first_valid: 0,
            target_object: ObjectId::INVALID,
            target_position: Coord3D::ZERO,
        }
    }

    /// `ProjectileStreamUpdate::addProjectile` (ProjectileStreamUpdate.cpp:75-122). `victim_pos` is
    /// `None` for a shot at an object only.
    pub fn add_projectile(&mut self, source: ObjectId, new_id: u32, victim: ObjectId, victim_pos: Option<Coord3D>) {
        if self.owner == ObjectId::INVALID {
            self.owner = source;
        }
        if victim != ObjectId::INVALID {
            // Object shot: a changed target inserts a hole to break the stream.
            if victim != self.target_object {
                self.push_raw(None);
                self.target_object = victim;
            }
            // Clear the position so this reads as an object shot.
            self.target_position = Coord3D::ZERO;
        } else if let Some(pos) = victim_pos {
            if self.target_position != pos {
                self.push_raw(None);
                self.target_position = pos;
            }
            self.target_object = ObjectId::INVALID;
        }
        self.push_raw(Some(new_id));
    }

    /// `m_projectileIDs[m_nextFreeIndex] = id; m_nextFreeIndex = (m_nextFreeIndex + 1) % MAX`. The
    /// original's overflow assert is not reproduced: a full ring simply wraps as the C++ does.
    fn push_raw(&mut self, id: Option<u32>) {
        self.ids[self.next_free] = id;
        self.next_free = (self.next_free + 1) % MAX_PROJECTILE_STREAM;
    }

    /// `cullFrontOfList` (ProjectileStreamUpdate.cpp:124-131): chew off the front while the ids are gone
    /// (holes included). Holes in the middle stay: they are the chain's breaks.
    pub fn cull_front(&mut self, alive: impl Fn(u32) -> bool) {
        while self.first_valid != self.next_free && !self.ids[self.first_valid].is_some_and(&alive) {
            self.first_valid = (self.first_valid + 1) % MAX_PROJECTILE_STREAM;
        }
    }

    /// `considerDying` (ProjectileStreamUpdate.cpp:133-143): no projectiles left and the owner is gone.
    pub fn considers_dying(&self, owner_alive: bool) -> bool {
        self.first_valid == self.next_free && self.owner != ObjectId::INVALID && !owner_alive
    }

    /// The ids in the ring from `first_valid` to `next_free` (holes as `None`), in draw order.
    pub fn ids_in_order(&self) -> Vec<Option<u32>> {
        let mut out = Vec::new();
        let mut i = self.first_valid;
        while i != self.next_free {
            out.push(self.ids[i]);
            i = (i + 1) % MAX_PROJECTILE_STREAM;
        }
        out
    }
}

/// Read-only copy of one stream for the client (`W3DProjectileStreamDraw::doDrawModule` input).
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectileStreamSnapshot {
    pub id: u32,
    pub owner: ObjectId,
    pub template_name: String,
    /// `getAllPoints`: one point per ring entry, oldest first. A hole, or a projectile that is
    /// gone, is the zero vector (as in the original).
    pub points: Vec<Coord3D>,
}

/// `GeometryInfo::getMaxHeightAbovePosition` (Geometry.cpp:210-223): a sphere's radius, else the height.
pub fn max_height_above_position(g: &Geometry) -> f32 {
    match g.shape {
        GeometryShape::Sphere => g.major_radius,
        GeometryShape::Box | GeometryShape::Cylinder => g.height,
    }
}

/// The sim's stream list, owned by `CombatState`.
#[derive(Default)]
pub struct StreamState {
    pub(crate) streams: Vec<ProjectileStream>,
    /// `Weapon::m_projectileStreamID` per (shooter, weapon slot). Temporary weapons (`None` slot)
    /// get a fresh stream each shot, as a fresh `Weapon` does in the original.
    by_weapon: BTreeMap<(ObjectId, usize), u32>,
}

impl StreamState {
    pub(crate) fn crc(&self, c: &mut Crc) {
        c.add_u32(self.streams.len() as u32);
        for s in &self.streams {
            c.add_u32(s.id);
            c.add_u32(s.owner.0);
            c.add_coord(&s.position);
            c.add_coord(&s.target_position);
            c.add_u32(s.target_object.0);
            c.add_u32(s.first_valid as u32);
            c.add_u32(s.next_free as u32);
            for id in &s.ids {
                c.add_u32(id.unwrap_or(0));
            }
        }
    }
}

impl GameLogic {
    /// `Weapon::newProjectileFired` (Weapon.cpp:2504-2531): the firing weapon's stream (created on
    /// first use) takes the new projectile. `slot` is `None` for a temporary weapon.
    #[track_caller]
    pub(crate) fn new_projectile_fired(&mut self, source: ObjectId, slot: Option<usize>, stream_name: &str, projectile: u32, victim: ObjectId, victim_pos: Coord3D) {
        let Some(src_pos) = self.objects.get(&source).map(|o| o.position) else { return };
        let existing = slot.and_then(|s| self.combat.streams.by_weapon.get(&(source, s)).copied()).filter(|id| self.combat.streams.streams.iter().any(|st| st.id == *id));
        let id = match existing {
            Some(id) => id,
            None => {
                // `TheThingFactory->newObject(pst, ...)` (Weapon.cpp:2515) takes the next object id; only the first
                // projectile of a stream (or the first after the stream is gone) gets here.
                let id = self.allocate_object_id(stream_name, std::panic::Location::caller()).0;
                self.combat.streams.streams.push(ProjectileStream::new(id, stream_name.to_owned(), src_pos));
                if let Some(s) = slot {
                    self.combat.streams.by_weapon.insert((source, s), id);
                }
                id
            }
        };
        let Some(stream) = self.combat.streams.streams.iter_mut().find(|st| st.id == id) else { return };
        // `update->setPosition(sourceObj->getPosition())`.
        stream.position = src_pos;
        stream.add_projectile(source, projectile, victim, Some(victim_pos));
    }

    /// The stream part of `ProjectileStreamUpdate::update` for every stream: cull the dead front and
    /// destroy a stream whose owner is gone with nothing left to draw.
    pub(crate) fn update_projectile_streams(&mut self) {
        if self.combat.streams.streams.is_empty() {
            return;
        }
        let mut streams = std::mem::take(&mut self.combat.streams.streams);
        for s in &mut streams {
            s.cull_front(|id| self.combat.projectile_position(id).is_some());
        }
        // `findObjectByID(m_owningObject) == nullptr`: an owner that is destroyed but not yet removed is still
        // found (`destroyObject` only queues it, GameLogic.cpp:4081-4110, and `findObjectByID` reads the object
        // vector, GameLogic.h:501-512), so the stream dies the frame after the owner's removal.
        streams.retain(|s| {
            let owner_alive = self.objects.contains_key(&s.owner);
            !s.considers_dying(owner_alive)
        });
        self.combat.streams.by_weapon.retain(|_, id| streams.iter().any(|s| s.id == *id));
        self.combat.streams.streams = streams;
    }

    /// `ProjectileStreamUpdate::getAllPoints` for one stream (ProjectileStreamUpdate.cpp:145-197): the
    /// projectile positions oldest first. A vehicle owner lifts a point that is within 1.5 major
    /// radii of it to its roof (`max(z, top + 0.5)`).
    fn stream_points(&self, s: &ProjectileStream) -> Vec<Coord3D> {
        // `findObjectByID` (see `update_projectile_streams`): a queued owner is still its vehicle for the roof.
        let owner = self.objects.get(&s.owner);
        s.ids_in_order()
            .into_iter()
            .map(|id| {
                let Some(p) = id.and_then(|id| self.combat.projectile_position(id)) else { return Coord3D::ZERO };
                let mut out = p;
                if let Some(o) = owner.filter(|o| o.template.kind_of & kind_of::VEHICLE != 0) {
                    let top = max_height_above_position(&o.template.geometry) + o.position.z + 0.5;
                    let dx = o.position.x - p.x;
                    let dy = o.position.y - p.y;
                    if sqrt(dx * dx + dy * dy) <= o.template.geometry.major_radius * 1.5 {
                        out.z = out.z.max(top);
                    }
                }
                out
            })
            .collect()
    }

    /// Read-only snapshots of the live streams, for client drawing.
    pub fn projectile_streams(&self) -> Vec<ProjectileStreamSnapshot> {
        self.combat
            .streams
            .streams
            .iter()
            .map(|s| ProjectileStreamSnapshot { id: s.id, owner: s.owner, template_name: s.template_name.clone(), points: self.stream_points(s) })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(n: u32) -> ObjectId {
        ObjectId(n)
    }

    fn stream() -> ProjectileStream {
        ProjectileStream::new(1, "S".into(), Coord3D::ZERO)
    }

    /// The first shot always starts with a hole: `m_targetObject` is `INVALID_ID` until a target is set
    /// (ProjectileStreamUpdate.cpp:82-89), so the strand's first point follows a break.
    #[test]
    fn shots_at_one_target_keep_one_strand() {
        let mut s = stream();
        s.add_projectile(obj(7), 101, obj(9), Some(Coord3D::ZERO));
        s.add_projectile(obj(7), 102, obj(9), Some(Coord3D::ZERO));
        assert_eq!(s.owner, obj(7), "the first shooter owns the stream");
        assert_eq!(s.ids_in_order(), vec![None, Some(101), Some(102)]);
    }

    /// `addProjectile` (ProjectileStreamUpdate.cpp:82-93): a new target inserts a hole, so the strand
    /// breaks there. Reverting the hole insertion makes this fail.
    #[test]
    fn a_new_target_inserts_a_hole() {
        let mut s = stream();
        s.add_projectile(obj(7), 101, obj(9), None);
        s.add_projectile(obj(7), 102, obj(10), None);
        assert_eq!(s.ids_in_order(), vec![None, Some(101), None, Some(102)]);
    }

    /// Shooting a position (`victimPos`) breaks the chain when the position changes
    /// (ProjectileStreamUpdate.cpp:98-108).
    #[test]
    fn a_new_position_inserts_a_hole_but_the_same_one_does_not() {
        let mut s = stream();
        let a = Coord3D::new(1.0, 2.0, 0.0);
        let b = Coord3D::new(5.0, 2.0, 0.0);
        s.add_projectile(obj(7), 101, ObjectId::INVALID, Some(a));
        s.add_projectile(obj(7), 102, ObjectId::INVALID, Some(a));
        s.add_projectile(obj(7), 103, ObjectId::INVALID, Some(b));
        assert_eq!(s.ids_in_order(), vec![None, Some(101), Some(102), None, Some(103)]);
    }

    /// `cullFrontOfList` drops dead ids from the front (holes included) and keeps the holes in the middle.
    #[test]
    fn cull_drops_the_dead_front_and_keeps_middle_holes() {
        let mut s = stream();
        s.add_projectile(obj(7), 101, obj(9), None);
        s.add_projectile(obj(7), 102, obj(10), None);
        s.add_projectile(obj(7), 103, obj(10), None);
        // 101 is dead, the hole after it is culled from the front, 102 is dead too.
        s.cull_front(|id| id == 103);
        assert_eq!(s.ids_in_order(), vec![Some(103)]);
    }

    #[test]
    fn a_hole_in_the_middle_is_not_culled() {
        let mut s = stream();
        s.add_projectile(obj(7), 101, obj(9), None);
        s.add_projectile(obj(7), 102, obj(10), None);
        s.cull_front(|id| id == 101 || id == 102);
        assert_eq!(s.ids_in_order(), vec![Some(101), None, Some(102)]);
    }

    /// `considerDying`: the stream dies only once it is empty and its owner is gone.
    /// Reverting the owner check makes the first assertion fail.
    #[test]
    fn it_dies_only_when_empty_and_the_owner_is_gone() {
        let mut s = stream();
        assert!(!s.considers_dying(false), "no owner yet");
        s.add_projectile(obj(7), 101, obj(9), None);
        s.cull_front(|_| false);
        assert!(s.considers_dying(false), "empty and the owner is dead");
        assert!(!s.considers_dying(true), "the owner is alive");
        let mut t = stream();
        t.add_projectile(obj(7), 101, obj(9), None);
        assert!(!t.considers_dying(false), "a live projectile keeps it");
    }

    /// The ring holds `MAX_PROJECTILE_STREAM` entries (ProjectileStreamUpdate.h:40): the leading hole
    /// and 18 shots fill 19 of the 20 slots, so all of them are kept (a full ring reads as empty, as the
    /// original's wrap does).
    #[test]
    fn eighteen_live_shots_all_fit() {
        let mut s = stream();
        for i in 0..18 {
            s.add_projectile(obj(7), 100 + i, obj(9), None);
        }
        assert_eq!(s.ids_in_order().len(), MAX_PROJECTILE_STREAM - 1);
    }

    #[test]
    fn roof_height_is_a_sphere_radius_or_the_box_height() {
        let sphere = Geometry { shape: GeometryShape::Sphere, major_radius: 3.0, minor_radius: 3.0, height: 10.0, is_small: false };
        let boxed = Geometry { shape: GeometryShape::Box, major_radius: 4.0, minor_radius: 4.0, height: 6.0, is_small: false };
        assert_eq!(max_height_above_position(&sphere), 3.0);
        assert_eq!(max_height_above_position(&boxed), 6.0);
    }
}
