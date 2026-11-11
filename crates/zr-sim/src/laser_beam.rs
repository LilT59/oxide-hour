//! Laser beams fired by weapons with a `LaserName` (`Weapon::createLaser`, Weapon.cpp:2534-2568).
//!
//! The original spawns the laser as an `Object` (its template has `W3DLaserDraw` + the client-side
//! `LaserUpdate`, plus a `LifetimeUpdate` or `DeletionUpdate` that removes it). The beam itself does
//! no gameplay: the weapon deals its damage directly (`Weapon.cpp:1035-1055`). The sim keeps the beam
//! as a record: its source, the `LaserBoneName`, the fixed end point, and the frame it is removed.
//! Only the lifetime rolls touch the sim's RNG, in the order `GameLogic::create_object_full` uses for
//! objects (`LifetimeUpdate` first, then `DeletionUpdate`).
//!
//! `LaserUpdate::initLaser` gives a beam with a parent its start from the parent's bone each client
//! frame (`updateStartPos`), and a fixed end (`createLaser` always passes an end position, so the
//! target is not tracked). The client resolves the bone; the sim gives the source and bone name.

use crate::crc::Crc;
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::ObjectId;

/// The two timed modules a laser template can carry (`LifetimeUpdate.cpp:42-56`, `DeletionUpdate.cpp:40-45`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaserRollKind {
    /// `LifetimeUpdate`: `kill()` at its wake frame, so the death pipeline runs (the laser's `DestroyDie`
    /// then destroys it, `DestroyDie.cpp`).
    Lifetime,
    /// `DeletionUpdate`: `destroyObject()` at its wake frame; no death pipeline runs (`DeletionUpdate.cpp:82-90`).
    Deletion,
}

/// The laser template's draw-independent data (the `LaserName` object plus the weapon's `LaserBoneName`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaserSpec {
    /// `LaserName`: the laser object template (`W3DLaserDraw` / `LaserUpdate`) the client draws.
    pub template_name: String,
    /// `LaserBoneName`: the source bone the beam starts from (empty: the source's position).
    pub bone_name: String,
    /// The laser template's timed modules, in template module order: each rolls `(min, max)` frames when the
    /// object is constructed (`calcSleepDelay`), so the rolls draw from the RNG in this order.
    pub rolls: Vec<(LaserRollKind, u32, u32)>,
}

/// One live beam. `start` is the source's position when it was fired; the client moves it to the bone.
#[derive(Clone, Debug, PartialEq)]
pub struct LaserBeam {
    pub id: u32,
    pub source: ObjectId,
    pub template_name: String,
    pub bone_name: String,
    pub start: Coord3D,
    pub end: Coord3D,
    /// The frame the beam is removed (`u32::MAX` when its template has neither module).
    pub die_frame: u32,
}

/// The sim's beam list, owned by `CombatState`.
#[derive(Default)]
pub struct LaserState {
    pub(crate) beams: Vec<LaserBeam>,
}

impl LaserState {
    pub(crate) fn crc(&self, c: &mut Crc) {
        c.add_u32(self.beams.len() as u32);
        for b in &self.beams {
            c.add_u32(b.id);
            c.add_u32(b.source.0);
            c.add_coord(&b.start);
            c.add_coord(&b.end);
            c.add_u32(b.die_frame);
        }
    }
}

impl GameLogic {
    /// `Weapon::createLaser` (Weapon.cpp:2534-2568): the beam starts at the source and ends at `end`.
    /// Each module's random lifetime is rolled here, as `create_object_full` does for an object.
    #[track_caller]
    pub(crate) fn create_laser(&mut self, source: ObjectId, spec: &LaserSpec, end: Coord3D) {
        // `Thing::newObject` (Weapon.cpp:2543) allocates the object id first (`Object::Object`, Object.cpp:294),
        // then the modules are constructed and roll their lifetimes in template order.
        let id = self.allocate_object_id(&spec.template_name, std::panic::Location::caller()).0;
        let Some(start) = self.objects.get(&source).map(|o| o.position) else { return };
        let now = self.frame;
        let mut die = u32::MAX;
        for &(_, lo, hi) in &spec.rolls {
            // `GameLogicRandomValue(min, max)` always draws (min == max still advances it), and a zero roll
            // is one frame (`LifetimeUpdate::calcSleepDelay`, `DeletionUpdate::calcSleepDelay`).
            let delay = (self.rng.value(lo as i32, hi as i32) as u32).max(1);
            die = die.min(now.wrapping_add(delay));
        }
        self.combat.lasers.beams.push(LaserBeam {
            id,
            source,
            template_name: spec.template_name.clone(),
            bone_name: spec.bone_name.clone(),
            start,
            end,
            die_frame: die,
        });
    }

    /// Removes the beams whose lifetime has run out (`LifetimeUpdate::update` / `DeletionUpdate::update`).
    pub(crate) fn update_lasers(&mut self) {
        let now = self.frame;
        self.combat.lasers.beams.retain(|b| now < b.die_frame);
    }

    /// Read-only copies of the live beams, for client drawing.
    pub fn lasers(&self) -> Vec<LaserBeam> {
        self.combat.lasers.beams.clone()
    }
}
