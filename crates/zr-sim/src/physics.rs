//! `PhysicsBehavior` (`GameLogic/Object/Update/PhysicsUpdate.cpp`): mass, force integration,
//! gravity, friction, ground collision/bounce, angular rates and `KillWhenRestingOnGround`.
//!
//! `zr-sim` does not depend on the INI crate: the adapter converts `zr_templates::PhysicsData` into
//! [`PhysicsTemplate`]. The locomotor drives ground units by applying motive forces here
//! (`applyMotiveForce`); gravity, friction, ground collision and the pitch/roll/yaw rates run in
//! [`GameLogic::update_physics`].
//!
//! Ported: `applyForce` (with the `isMotive()` lateral-only filter), `applyMotiveForce` +
//! `MOTIVE_FRAMES`, the friction getters and `applyFrictionalForces`, `applyGravitationalForces`,
//! `applyYPRDamping`, the `update()` integration with the tiny-velocity clamp, the `OBJECT_STATUS_
//! BRAKING` z-only adjustment, `handleBounce` (its `setAngles` flip is dead: the same `update()`
//! overwrites the matrix), the ground slam / `STICK_TO_GROUND` snap, the `HAS_PITCHROLLYAW` rotation
//! (the rates post-multiply the object's matrix as `Rotate_X`/`Rotate_Y`/`Rotate_Z`,
//! `PhysicsUpdate.cpp:747-749`, with `CenterOfMassOffset` damping read from the matrix's X vector), the
//! bounce re-right (`setAngles(yaw, 0, 0)` at `:799-811`), the falling-damage test and
//! `KillWhenRestingOnGround`.
//!
//! The sleepy module bookkeeping is modelled: `calcSleepTime` (`PhysicsUpdate.cpp:919-936`) puts
//! a module to sleep (`UPDATE_SLEEP_FOREVER`) only when it is at rest, not spinning
//! (`HAS_PITCHROLLYAW`), not motive, on the ground layer and not above the terrain, not
//! overlapping anything and has run at least once; otherwise it runs next frame. A module's first
//! update is one frame after creation (`setWakeFrame(getObject(), UPDATE_SLEEP_NONE)` in the
//! constructor, `:231-233`), and a force applied from outside the update wakes a sleeping module
//! for the next frame (`applyForce`, `:401-402`).
//!
//! Object-object collision forces (`onCollide`/`checkForOverlapCollision`) live in
//! [`crate::collide`]; the overlap ids they record (`m_currentOverlap`/`m_previousOverlap`) are
//! part of this module because `calcSleepTime` reads them. Not modelled: shock waves and stun.
//! Possessed objects are skipped (the possession addition drives their locomotor directly).

use crate::crc::Crc;
use crate::disabled::DisabledType;
use crate::logic::GameLogic;
use crate::math::{cos, sin, sqrt, Coord3D, PI};
use crate::object::ObjectId;

const DEFAULT_MASS: f32 = 1.0;
const DEFAULT_SHOCK_YAW: f32 = 0.05;
const DEFAULT_SHOCK_PITCH: f32 = 0.025;
const DEFAULT_SHOCK_ROLL: f32 = 0.025;
const DEFAULT_FORWARD_FRICTION: f32 = 0.15;
const DEFAULT_LATERAL_FRICTION: f32 = 0.15;
const DEFAULT_Z_FRICTION: f32 = 0.8;
const DEFAULT_AERO_FRICTION: f32 = 0.0;
const MIN_AERO_FRICTION: f32 = 0.0;
const MIN_NON_AERO_FRICTION: f32 = 0.01;
const MAX_FRICTION: f32 = 0.99;
/// `MOTIVE_FRAMES = LOGICFRAMES_PER_SECOND / 3` (`PhysicsUpdate.cpp:72`).
pub(crate) const MOTIVE_FRAMES: u32 = crate::math::LOGICFRAMES_PER_SECOND / 3;
const INVALID_VEL_MAG: f32 = -1.0;

/// `PhysicsBehaviorModuleData` (already per-frame units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhysicsTemplate {
    pub mass: f32,
    pub shock_resistance: f32,
    pub shock_max_yaw: f32,
    pub shock_max_pitch: f32,
    pub shock_max_roll: f32,
    pub forward_friction: f32,
    pub lateral_friction: f32,
    pub z_friction: f32,
    pub aerodynamic_friction: f32,
    /// `CenterOfMassOffset`: pitches the object toward straight down/up while it spins.
    pub center_of_mass_offset: f32,
    pub allow_bouncing: bool,
    pub allow_collide_force: bool,
    pub kill_when_resting_on_ground: bool,
    /// `MinFallHeightForDamage` converted with `heightToSpeed` (speed per frame).
    pub min_fall_speed_for_damage: f32,
    pub fall_height_damage_factor: f32,
    /// `PitchRollYawFactor`: the angular rate is multiplied by this when integrating.
    pub pitch_roll_yaw_factor: f32,
}

impl Default for PhysicsTemplate {
    fn default() -> Self {
        PhysicsTemplate {
            mass: DEFAULT_MASS,
            shock_resistance: 0.0,
            shock_max_yaw: DEFAULT_SHOCK_YAW,
            shock_max_pitch: DEFAULT_SHOCK_PITCH,
            shock_max_roll: DEFAULT_SHOCK_ROLL,
            forward_friction: DEFAULT_FORWARD_FRICTION,
            lateral_friction: DEFAULT_LATERAL_FRICTION,
            z_friction: DEFAULT_Z_FRICTION,
            aerodynamic_friction: DEFAULT_AERO_FRICTION,
            center_of_mass_offset: 0.0,
            allow_bouncing: false,
            allow_collide_force: true,
            kill_when_resting_on_ground: false,
            // `heightToSpeed(40)` with the default gravity `-1`: `sqrt(2*40)`.
            min_fall_speed_for_damage: (2.0f32 * 40.0).sqrt(),
            fall_height_damage_factor: 1.0,
            pitch_roll_yaw_factor: 2.0,
        }
    }
}

/// Per-object physics state (`PhysicsBehavior`'s `m_vel` / `m_accel` / angular rates / flags).
#[derive(Clone, Debug)]
pub struct Physics {
    pub velocity: Coord3D,
    pub accel: Coord3D,
    /// `m_prevAccel`: last frame's acceleration (not used by the sim logic, kept for parity).
    pub prev_accel: Coord3D,
    /// `ALLOW_TO_FALL`: `STICK_TO_GROUND` is ignored and z is never snapped to the ground.
    pub allow_to_fall: bool,
    /// `STICK_TO_GROUND`: the ground collision snaps z instead of bouncing.
    pub stick_to_ground: bool,
    /// `ALLOW_BOUNCE`.
    pub allow_bounce: bool,
    /// `m_originalAllowBounce`: restored once the bounce is over.
    pub original_allow_bounce: bool,
    /// `APPLY_FRICTION2D_WHEN_AIRBORNE`.
    pub apply_friction_2d_when_airborne: bool,
    /// `ALLOW_COLLIDE_FORCE`.
    pub allow_collide_force: bool,
    /// `UPDATE_EVER_RUN`.
    pub update_ever_run: bool,
    /// `WAS_AIRBORNE_LAST_FRAME`.
    pub was_airborne_last_frame: bool,
    /// `IS_IN_FREEFALL`.
    pub is_in_freefall: bool,
    /// `IS_STUNNED`.
    pub is_stunned: bool,
    /// `Locomotor::IS_BRAKING` (`Locomotor.cpp:950-967`, `:1073`): the kernel flag the ground kernels
    /// set and the top-level clear drops. `locoUpdate_moveTowardsPosition` copies it to
    /// `OBJECT_STATUS_BRAKING` (`:1111`), which is what `update()` reads to only adjust z for a
    /// non-projectile (`PhysicsUpdate.cpp:664`); the flag itself is only the kernels' input, and
    /// `wasBraking` (`:1073`) reads the status bit.
    pub braking: bool,
    /// `PhysicsBehavior::m_yawRate` / `m_rollRate` / `m_pitchRate` (radians per frame).
    pub yaw_rate: f32,
    pub roll_rate: f32,
    pub pitch_rate: f32,
    /// `m_pitch` / `m_roll`: a *diagnostic* mirror of the orientation the angular rates build into the
    /// matrix, kept for the tests that assert a fling tilted the object. The engine has no such state
    /// (`PhysicsBehavior` keeps only the three rates, `PhysicsUpdate.h:263-265`; `setAngles` writes the
    /// matrix and keeps nothing, `PhysicsUpdate.cpp:1074-1086`), and no sim code reads them.
    pub pitch: f32,
    pub roll: f32,
    /// `m_motiveForceExpires`: frame at which a recent `applyMotiveForce` stops marking us driven.
    pub motive_force_expires: u32,
    /// A motive force was applied by a kernel that has no `GameLogic` (locomotor.rs): `applyForce` from an external
    /// module wakes the module with `UPDATE_SLEEP_NONE` (PhysicsUpdate.cpp:401-402). `GameLogic::update` flushes the flag
    /// into the heap wake for the next frame (`flush_physics_wake_requests`); the sleep itself is the heap key.
    pub wake_requested: bool,
    /// `m_extraFriction` / `m_extraBounciness` (`Locomotor::setPhysicsOptions`, fling).
    pub extra_friction: f32,
    pub extra_bounciness: f32,
    /// `m_turning` (`PhysicsTurningType`): `-1` negative, `0` none, `1` positive.
    pub turning: i8,
    /// `Locomotor::MOVING_BACKWARDS` / `DOING_THREE_POINT_TURN` (wheeled vehicles).
    pub moving_backwards: bool,
    pub doing_three_point_turn: bool,
    /// `Locomotor::m_donutTimer`: frame at which a circling wheeled unit starts braking.
    pub donut_timer: u32,
    /// `Locomotor::m_brakingFactor` (`Locomotor.cpp:445`): the multiplier the treads kernel applies to
    /// `getBraking()` (`:1255`). It persists across frames: the top-level clear and the wheels kernel set
    /// it to 1.0 (`:966`, `:1441`), the treads kernel sets 1.1 when it starts braking (`:1218`) and then
    /// `(slowDownDist/onPathDistToGoal)^2` capped at `MAX_BRAKING_FACTOR` 5.0 while braking (`:1228-1231`).
    /// A Locomotor is built per object (`chooseLocomotorSetExplicit`), so the ctor value (`:658`) applies
    /// at creation and on a set change.
    pub braking_factor: f32,
    /// `m_currentOverlap` (`PhysicsUpdate.cpp:1049-1056`): set by a moving crusher's
    /// `checkForOverlapCollision`; an overlap keeps the module awake (`:927`).
    pub current_overlap: Option<ObjectId>,
    /// `m_previousOverlap`: last frame's `m_currentOverlap` (`PhysicsUpdate.cpp:824-825`).
    pub previous_overlap: Option<ObjectId>,
    /// Cached velocity magnitude; `INVALID_VEL_MAG` means "recompute".
    vel_mag: f32,
}

impl Default for Physics {
    fn default() -> Self {
        Physics {
            velocity: Coord3D::ZERO,
            accel: Coord3D::ZERO,
            prev_accel: Coord3D::ZERO,
            allow_to_fall: false,
            // `PhysicsBehavior`'s flags start at 0; the locomotor sets `STICK_TO_GROUND` via
            // `setPhysicsOptions` (`Locomotor.cpp:932`). Objects without a locomotor fall.
            stick_to_ground: false,
            allow_bounce: false,
            original_allow_bounce: false,
            apply_friction_2d_when_airborne: false,
            allow_collide_force: true,
            update_ever_run: false,
            was_airborne_last_frame: false,
            is_in_freefall: false,
            is_stunned: false,
            braking: false,
            yaw_rate: 0.0,
            roll_rate: 0.0,
            pitch_rate: 0.0,
            pitch: 0.0,
            roll: 0.0,
            motive_force_expires: 0,
            wake_requested: false,
            extra_friction: 0.0,
            extra_bounciness: 0.0,
            turning: 0,
            moving_backwards: false,
            doing_three_point_turn: false,
            donut_timer: 0,
            braking_factor: 1.0, // `Locomotor::Locomotor`: `m_brakingFactor = 1.0f` (`Locomotor.cpp:658`)
            current_overlap: None,
            previous_overlap: None,
            vel_mag: INVALID_VEL_MAG,
        }
    }
}

impl Physics {
    pub fn new() -> Self {
        Physics::default()
    }

    pub fn speed_2d(&self) -> f32 {
        sqrt(self.velocity.x * self.velocity.x + self.velocity.y * self.velocity.y)
    }

    /// `getVelocityMagnitude()` (cached like the original's `m_velMag`).
    pub fn velocity_magnitude(&self) -> f32 {
        if self.vel_mag == INVALID_VEL_MAG {
            let v = self.velocity;
            // `sqrtf(sqr(x)+sqr(y)+sqr(z))`; the cache is mutable in the original, recompute here.
            return sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
        }
        self.vel_mag
    }

    /// `PhysicsBehavior::scrubVelocity2D` (`PhysicsUpdate.cpp:1028-1047`): scale the 2D velocity to
    /// `desired` (0 clears it) and invalidate the cached magnitude.
    pub fn scrub_velocity_2d(&mut self, desired: f32) {
        if desired < 0.001 {
            self.velocity.x = 0.0;
            self.velocity.y = 0.0;
        } else {
            let cur = sqrt(self.velocity.x * self.velocity.x + self.velocity.y * self.velocity.y);
            if desired > cur {
                return;
            }
            let scale = desired / cur;
            self.velocity.x *= scale;
            self.velocity.y *= scale;
        }
        self.vel_mag = INVALID_VEL_MAG;
    }

    /// `getForwardSpeed2D()`: signed speed along the object's 2D facing vector.
    pub fn forward_speed_2d(&self, angle: f32) -> f32 {
        let (dx, dy) = (cos(angle), sin(angle));
        let vx = self.velocity.x * dx;
        let vy = self.velocity.y * dy;
        let dot = vx + vy;
        let speed = sqrt(vx * vx + vy * vy);
        if dot >= 0.0 {
            speed
        } else {
            -speed
        }
    }

    /// `getForwardSpeed3D()` (`PhysicsUpdate.cpp:978`): signed speed along the object's full 3D facing
    /// vector, `getTransformMatrix()->Get_X_Vector()`. The engine multiplies component by component (not a
    /// dot product), and so does this.
    pub fn forward_speed_3d(&self, dir: Coord3D) -> f32 {
        let (dx, dy, dz) = (dir.x, dir.y, dir.z);
        let vx = self.velocity.x * dx;
        let vy = self.velocity.y * dy;
        let vz = self.velocity.z * dz;
        let dot = vx + vy + vz;
        let speed = sqrt(vx * vx + vy * vy + vz * vz);
        if dot >= 0.0 {
            speed
        } else {
            -speed
        }
    }

    /// `isMotive()`: `m_motiveForceExpires > frame`.
    pub fn is_motive(&self, frame: u32) -> bool {
        self.motive_force_expires > frame
    }

    /// `PhysicsBehavior::resetDynamicPhysics` (`PhysicsUpdate.cpp:388-406`): zero the acceleration, the
    /// velocity and the three angular rates; `HAS_PITCHROLLYAW` drops with them (the port integrates the
    /// rotation only while a rate is non-zero). The `SLEEPY_PHYSICS` re-key
    /// (`setWakeFrame(getObject(), calcSleepTime())`) is the heap key the next physics update computes.
    pub fn reset_dynamic_physics(&mut self) {
        self.accel = Coord3D::ZERO;
        self.prev_accel = Coord3D::ZERO;
        self.velocity = Coord3D::ZERO;
        self.vel_mag = 0.0;
        self.turning = 0;
        self.yaw_rate = 0.0;
        self.roll_rate = 0.0;
        self.pitch_rate = 0.0;
    }

    pub fn has_pitch_roll_yaw(&self) -> bool {
        self.pitch_rate != 0.0 || self.roll_rate != 0.0 || self.yaw_rate != 0.0
    }

    /// `getForwardFriction()`: `m_forwardFriction + m_extraFriction`, clamped.
    pub fn forward_friction(&self, t: &PhysicsTemplate) -> f32 {
        (t.forward_friction + self.extra_friction).clamp(MIN_NON_AERO_FRICTION, MAX_FRICTION)
    }

    /// `getLateralFriction()`.
    pub fn lateral_friction(&self, t: &PhysicsTemplate) -> f32 {
        (t.lateral_friction + self.extra_friction).clamp(MIN_NON_AERO_FRICTION, MAX_FRICTION)
    }

    /// `getZFriction()`.
    pub fn z_friction(&self, t: &PhysicsTemplate) -> f32 {
        (t.z_friction + self.extra_friction).clamp(MIN_NON_AERO_FRICTION, MAX_FRICTION)
    }

    /// `getAerodynamicFriction()`.
    pub fn aerodynamic_friction(&self, t: &PhysicsTemplate) -> f32 {
        (t.aerodynamic_friction + self.extra_friction).clamp(MIN_AERO_FRICTION, MAX_FRICTION)
    }

    pub(crate) fn crc(&self, c: &mut Crc) {
        c.add_coord(&self.velocity);
        c.add_coord(&self.accel);
        c.add_bool(self.allow_to_fall);
        c.add_bool(self.stick_to_ground);
        c.add_bool(self.braking);
        // Both are `Locomotor` members in the engine and are part of its save/CRC (`Locomotor::xfer`,
        // `Locomotor.cpp:750-755`), so they are simulation state: `m_donutTimer` decides when a circling
        // wheeled unit starts braking and `m_brakingFactor` scales the next frames' braking force.
        c.add_u32(self.donut_timer);
        c.add_f32(self.braking_factor);
        // The crusher's overlap ids affect `calcSleepTime`, so they are simulation state.
        c.add_u32(self.current_overlap.map_or(0xffff_ffff, |x| x.0));
        c.add_u32(self.previous_overlap.map_or(0xffff_ffff, |x| x.0));
        // Angular state is only hashed when non-zero so games without flings keep a stable CRC. The
        // engine's `PhysicsBehavior::crc` is empty (only `UpdateModule::crc`, `PhysicsUpdate.cpp:1851-1859`)
        // and `PhysicsBehavior` keeps no Euler angles — only the three rates (`PhysicsUpdate.h:263-265`),
        // so the port's `pitch`/`roll` diagnostic mirror is not part of the digest.
        let has_angles = self.yaw_rate != 0.0 || self.roll_rate != 0.0 || self.pitch_rate != 0.0;
        c.add_bool(has_angles);
        if has_angles {
            c.add_f32(self.yaw_rate);
            c.add_f32(self.roll_rate);
            c.add_f32(self.pitch_rate);
        }
    }
}

impl GameLogic {
    /// `PhysicsBehavior::getMass()` (`PhysicsUpdate.cpp:1089-1096`): the physics module's own
    /// `m_mass` plus `ContainModuleInterface::getContainedItemsMass()`. `OpenContain` sums
    /// `phys->getMass()` over its `m_containList` (`OpenContain.cpp:835-845`), so a loaded transport
    /// weighs what it carries, recursively. Returns 0 for an object without physics; every caller
    /// here only asks about objects that have it.
    pub(crate) fn get_mass(&self, id: ObjectId) -> f32 {
        self.objects.get(&id).and_then(|o| o.get_mass()).unwrap_or(0.0)
    }

    /// `PhysicsBehavior::applyForce`: `a = F / m`. While the object is `isMotive()` only the lateral
    /// component of the force is accepted (`PhysicsUpdate.cpp:326`).
    pub fn apply_force(&mut self, id: ObjectId, force: Coord3D) {
        if force.x.is_nan() || force.y.is_nan() || force.z.is_nan() {
            return;
        }
        let frame = self.frame;
        let Some(o) = self.objects.get(&id) else { return };
        if o.template.physics.is_none() {
            return;
        }
        let (dx, dy) = (cos(o.angle), sin(o.angle));
        let motive = o.physics.as_ref().is_some_and(|p| p.is_motive(frame));
        let (mut fx, mut fy, fz) = (force.x, force.y, force.z);
        if motive {
            // Only accept the lateral acceleration.
            let lateral = force.x * (-dy) + force.y * dx;
            fx = lateral * -dy;
            fy = lateral * dx;
        }
        // `F = ma --> a = F/m`: the original takes one reciprocal and multiplies each component,
        // without clamping the mass (`PhysicsUpdate.cpp:335-338`).
        let mass_inv = 1.0 / self.get_mass(id);
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.accel.x += fx * mass_inv;
            p.accel.y += fy * mass_inv;
            p.accel.z += fz * mass_inv;
        }
        // `applyForce` outside the module's own update wakes the module (PhysicsUpdate.cpp:401-402).
        self.wake_physics(id);
    }

    /// `PhysicsBehavior::applyMotiveForce`: accept the force unquestioningly, then mark the object
    /// driven for `MOTIVE_FRAMES` so external forces only push it sideways.
    pub fn apply_motive_force(&mut self, id: ObjectId, force: Coord3D) {
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.motive_force_expires = 0;
        }
        self.apply_force(id, force);
        let expires = self.frame + MOTIVE_FRAMES;
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.motive_force_expires = expires;
        }
    }

    /// `PhysicsBehavior::setYawRate` / `setRollRate` / `setPitchRate`.
    pub fn set_angular_rates(&mut self, id: ObjectId, yaw: f32, roll: f32, pitch: f32) {
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.yaw_rate = yaw;
            p.roll_rate = roll;
            p.pitch_rate = pitch;
        }
    }

    /// `PhysicsBehavior::setAngles` (`PhysicsUpdate.cpp:1074-1086`): a fresh matrix at the object's position,
    /// pre-rotated `X(-roll)`, `Y(pitch)`, `Z(yaw)`, and `setTransformMatrix` takes it (so the heading is its
    /// `Get_Z_Rotation`, not the yaw as given).
    pub fn set_angles(&mut self, id: ObjectId, yaw: f32, pitch: f32, roll: f32) {
        if let Some(o) = self.objects.get_mut(&id) {
            let mut xfrm = crate::transform::Matrix3D::from_translation(o.position);
            xfrm.in_place_pre_rotate_x(-roll);
            xfrm.in_place_pre_rotate_y(pitch);
            xfrm.in_place_pre_rotate_z(yaw);
            o.set_transform_matrix(xfrm);
            if let Some(p) = o.physics.as_mut() {
                p.pitch = pitch;
                p.roll = roll;
            }
        }
    }

    /// `PhysicsBehavior::setAllowToFall` / `setStickToGround` / `setAllowBouncing` /
    /// `setAllowAirborneFriction` / `setExtraFriction` / `setExtraBounciness` (used by the locomotor
    /// and the fling).
    pub fn set_allow_to_fall(&mut self, id: ObjectId, allow: bool) {
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.allow_to_fall = allow;
        }
    }
    pub fn set_stick_to_ground(&mut self, id: ObjectId, allow: bool) {
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.stick_to_ground = allow;
        }
    }
    pub fn set_allow_bouncing(&mut self, id: ObjectId, allow: bool) {
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.allow_bounce = allow;
            p.original_allow_bounce = allow;
        }
    }
    pub fn set_allow_airborne_friction(&mut self, id: ObjectId, allow: bool) {
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.apply_friction_2d_when_airborne = allow;
        }
    }
    pub fn set_extra_friction(&mut self, id: ObjectId, extra: f32) {
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.extra_friction = extra;
        }
    }
    pub fn set_extra_bounciness(&mut self, id: ObjectId, extra: f32) {
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.extra_bounciness = extra;
        }
    }
    pub fn set_physics_turning(&mut self, id: ObjectId, turning: i8) {
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.turning = turning;
        }
    }

    /// `PhysicsBehavior::scrubVelocity2D`.
    pub fn scrub_velocity_2d(&mut self, id: ObjectId, desired: f32) {
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.scrub_velocity_2d(desired);
        }
    }

    /// `PhysicsBehavior::addVelocityTo`.
    pub fn add_velocity_to(&mut self, id: ObjectId, vel: Coord3D) {
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.velocity.x += vel.x;
            p.velocity.y += vel.y;
            p.velocity.z += vel.z;
            p.vel_mag = INVALID_VEL_MAG;
        }
    }

    /// `PhysicsBehavior::update` for one object, run from the sleepy heap (`PHASE_PHYSICS`). Returns the sleep of
    /// `calcSleepTime`.
    pub(crate) fn physics_update(&mut self, id: ObjectId, now: u32) -> u32 {
        self.update_object_physics(id);
        self.physics_sleep(id, now)
    }

    /// Wake `id`'s physics if a kernel flagged it (`wake_requested`): the wake `applyForce` makes from the AI update that
    /// called the locomotor (PhysicsUpdate.cpp:401-402). The sleepy loop calls this right after each AI update, so the
    /// wake lands at the same point of the frame as in C++.
    pub(crate) fn wake_flagged_physics(&mut self, id: ObjectId) {
        let flagged = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()).is_some_and(|p| std::mem::take(&mut p.wake_requested));
        if flagged {
            self.wake_physics(id);
        }
    }

    /// Wake the physics of every object a motive force reached through a kernel that had no `GameLogic`
    /// (`wake_requested`), for the next frame. The kernels run inside the frame, so the wake target is the same as
    /// `applyForce`'s (`now + 1`, PhysicsUpdate.cpp:401-402); flushing at the end of the frame is therefore the same
    /// heap state as waking inside the kernel.
    pub(crate) fn flush_physics_wake_requests(&mut self) {
        let requested: Vec<ObjectId> = self
            .objects
            .values_mut()
            .filter_map(|o| {
                let p = o.physics.as_mut()?;
                if std::mem::take(&mut p.wake_requested) {
                    Some(o.id)
                } else {
                    None
                }
            })
            .collect();
        for id in requested {
            self.wake_physics(id);
        }
    }

    /// The physics heap wake of `id`: its next call frame, `u32::MAX` while it sleeps forever (the `wake_frame` of
    /// the old per-object field). Tests only.
    #[cfg(test)]
    pub(crate) fn physics_wake_frame(&self, id: ObjectId) -> u32 {
        let Some(h) = self.objects.get(&id).and_then(|o| o.behavior.physics_handle) else { return u32::MAX - 1 };
        let next = self.schedule.next_call_frame(h);
        if next >= crate::module::UPDATE_SLEEP_FOREVER {
            u32::MAX
        } else {
            next
        }
    }

    /// Make the physics module of `id` due at the current frame, as the old `wake_frame = 0` did. Tests only.
    #[cfg(test)]
    pub(crate) fn physics_due_now(&mut self, id: ObjectId) {
        let now = self.frame;
        if let Some(h) = self.objects.get(&id).and_then(|o| o.behavior.physics_handle) {
            self.schedule.awaken(h, now, now);
        }
    }

    /// `PhysicsBehavior::calcSleepTime` (PhysicsUpdate.cpp:919-936): at rest on the ground layer,
    /// not above the terrain, not spinning, with no motive force, no overlap and having run at least
    /// once, the module sleeps forever; otherwise it runs next frame.
    fn physics_sleep(&mut self, id: ObjectId, now: u32) -> u32 {
        let Some(o) = self.objects.get(&id) else { return crate::module::UPDATE_SLEEP_NONE };
        let Some(p) = o.physics.as_ref() else { return crate::module::UPDATE_SLEEP_NONE };
        // `getObject()->getLayer() == LAYER_GROUND` (`:925`): an object standing on a bridge deck is
        // on the bridge's layer, so it never sleeps. The port has no per-object partition layer; the
        // closest faithful test is `TerrainLogic::getLayerForDestination` on the object's position
        // (the layer whose height is closest to its z). A wall-top unit needs no separate test: the
        // wall layer is only chosen when the object is above half the wall height, so it is always
        // "above the terrain" below.
        let on_ground_layer = self.terrain.layer_for_destination(&o.position) == crate::bridge::LAYER_GROUND;
        // `!getObject()->isAboveTerrain()` (`:925`): `pos.z - getGroundHeight(x, y) > 0` is false
        // (Thing.cpp:293-300, Thing.h:129). `getGroundHeight` is the terrain, not a bridge deck, and
        // there is no epsilon.
        let above_terrain = o.position.z - self.terrain.ground_height(o.position.x, o.position.y) > 0.0;
        let at_rest = p.velocity.x == 0.0
            && p.velocity.y == 0.0
            && p.velocity.z == 0.0
            && p.accel.x == 0.0
            && p.accel.y == 0.0
            && p.accel.z == 0.0;
        let sleepable = at_rest
            && !p.has_pitch_roll_yaw()
            && !p.is_motive(now)
            && on_ground_layer
            && !above_terrain
            && p.current_overlap.is_none()
            && p.previous_overlap.is_none()
            && p.update_ever_run;
        if sleepable {
            crate::module::UPDATE_SLEEP_FOREVER
        } else {
            crate::module::UPDATE_SLEEP_NONE
        }
    }

    /// Is this object driven by a ground locomotor's motive force? Such objects run the full
    /// `PhysicsBehavior::update` every frame even while resting on the ground.
    fn is_force_driven(o: &crate::object::Object) -> bool {
        o.locomotor().is_some_and(|l| crate::locomotor::is_force_driven(&l))
    }

    fn update_object_physics(&mut self, id: ObjectId) {
        let gravity = self.rules.gravity;
        let stiffness = self.rules.ground_stiffness.clamp(0.01, 0.99);
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.physics) else { return };
        // `Bool airborneAtStart = obj->isAboveTerrain();` (`PhysicsUpdate.cpp:624`, `Thing.h:129`): height above the
        // *terrain* (no epsilon), not the layer surface.
        let (mut pos, angle, force_driven, disabled_held, airborne_at_start) = {
            let Some(o) = self.objects.get(&id) else { return };
            (
                o.position,
                o.angle,
                Self::is_force_driven(o),
                o.is_disabled_by_type(DisabledType::Held),
                o.position.z - self.terrain.ground_height(o.position.x, o.position.y) > 0.0,
            )
        };
        // `m_prevAccel = m_accel` and the first-run airborne flag (`PhysicsUpdate.cpp:632-639`) run before the HELD
        // guard, so every object gets them.
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.prev_accel = p.accel;
            if !p.update_ever_run {
                p.was_airborne_last_frame = airborne_at_start;
            }
        }

        // `DISABLED_HELD` objects skip the integration (`PhysicsUpdate.cpp:641`); the fling clears it so the object
        // can fly. Only the integration and the position write are inside the held guard: the update's tail still
        // runs — `m_accel.zero()`, the overlap roll, the landing block, the `KillWhenRestingOnGround` test,
        // `UPDATE_EVER_RUN`, `WAS_AIRBORNE_LAST_FRAME` and `calcSleepTime` (`:818-913`; the values it reads,
        // `activeVelZ` (:761) and `gotBounceForce`, are the ones the skipped integration would have produced: 0 and
        // false). Lane 167: returning here left a held spawn-point unit's `update_ever_run` false, so it never slept
        // (C++ sleeps it forever, the heap key FOREVER).
        if disabled_held {
            let vel = self.objects.get(&id).and_then(|o| o.physics.as_ref()).map_or(Coord3D::ZERO, |p| p.velocity);
            if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
                p.accel = Coord3D::ZERO;
                p.previous_overlap = p.current_overlap;
                p.current_overlap = None;
            }
            let airborne_at_end = self.objects.get(&id).is_some_and(|o| {
                o.position.z - self.terrain.ground_height(o.position.x, o.position.y) > 0.0
            });
            self.physics_landing_tail(id, &t, vel, 0.0, airborne_at_end);
            return;
        }

        let has_loco = self.objects.get(&id).is_some_and(|o| o.locomotor().is_some());
        // A possessed *air* object is driven directly by `update_possession` (a documented addition)
        // and a hijacker riding in a vehicle is carried by `HijackerUpdate`; neither may be
        // integrated. Ground possession keeps running physics as before (its velocity is zero).
        let suspended = self.objects.get(&id).is_some_and(|o| {
            o.behavior.hijacker_vehicle.is_some()
                || (o.ai.suspended
                    && o.locomotor()
                        .is_some_and(|l| l.is_airborne() || l.kind == crate::template::LocomotorKind::Hover))
        });
        let (run, allow_to_fall, is_in_freefall, has_angles) = {
            let Some(p) = self.objects.get(&id).and_then(|o| o.physics.as_ref()) else { return };
            (
                // Every object with `PhysicsBehavior` runs physics, except a kinematic (non-force-
                // driven) locomotor that has nothing pending.
                !suspended
                    && (!has_loco
                        || force_driven
                        || p.allow_to_fall
                        || p.is_in_freefall
                        || p.accel != Coord3D::ZERO
                        || p.velocity != Coord3D::ZERO
                        || p.has_pitch_roll_yaw()),
                p.allow_to_fall,
                p.is_in_freefall,
                p.has_pitch_roll_yaw(),
            )
        };
        // Kinematic units (hover/thrust/wings with no force applied) keep their locomotor's
        // position stepping: physics leaves them alone.
        if !run {
            if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
                p.velocity = Coord3D::ZERO;
                p.accel = Coord3D::ZERO;
            }
            return;
        }

        // `applyGravitationalForces` (`PhysicsUpdate.cpp:440-443`): `m_accel.z += gravity`, i.e. into
        // the object's own accumulator, not into a temporary one. `applyFrictionalForces` then adds
        // its force through `applyForce` on top of it; grouping the two sums differently would round
        // differently (the accumulator already holds the locomotor's motive force from this frame).
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.accel.z += gravity;
        }
        // applyFrictionalForces (2D friction / aerodynamic friction).
        self.apply_frictional_forces(id, &t, angle, gravity);

        let (old_z, braking) = {
            let Some(o) = self.objects.get(&id) else { return };
            if o.physics.is_none() {
                return;
            }
            // `obj->testStatus(OBJECT_STATUS_BRAKING)` (`PhysicsUpdate.cpp:664`): the status bit the
            // locomotor sets from its own `IS_BRAKING` flag (`Locomotor.cpp:1111`), not that flag.
            (pos.z, o.status_bits & crate::object::status::IS_BRAKING != 0)
        };

        // integrate acceleration into velocity, then clamp tiny values.
        let mut vel = {
            let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) else { return };
            p.velocity.x += p.accel.x;
            p.velocity.y += p.accel.y;
            p.velocity.z += p.accel.z;
            const THRESH: f32 = 0.001;
            if p.velocity.x.abs() < THRESH {
                p.velocity.x = 0.0;
            }
            if p.velocity.y.abs() < THRESH {
                p.velocity.y = 0.0;
            }
            if p.velocity.z.abs() < THRESH {
                p.velocity.z = 0.0;
            }
            p.vel_mag = INVALID_VEL_MAG;
            p.velocity
        };

        // integrate velocity into position (`PhysicsUpdate.cpp:663-678`): `OBJECT_STATUS_BRAKING` freezes
        // x/y, and a braking KINDOF_PROJECTILE is not moved at all (no z either — "things other than
        // projectiles don't cheat in z").
        let is_projectile = {
            let Some(o) = self.objects.get(&id) else { return };
            o.is_kind_of(crate::template::kind_of::PROJECTILE)
        };
        if braking {
            if !is_projectile {
                pos.z += vel.z;
            }
        } else {
            pos.x += vel.x;
            pos.y += vel.y;
            pos.z += vel.z;
        }
        if pos.x.is_nan() || pos.y.is_nan() || pos.z.is_nan() {
            self.destroy_object(id);
            return;
        }

        // `HAS_PITCHROLLYAW`: add the angular rates to the object's matrix (`PhysicsUpdate.cpp:700-750`). The
        // rates are scaled by `PitchRollYawFactor` and post-multiply the rotation part, X then Y then Z, so
        // pitch and roll turn the matrix and feed back into its heading (`Get_Z_Rotation`) and X vector.
        // `CenterOfMassOffset` damps the pitch rate by the sine of the angle left to straight up/down, read
        // from the matrix's own X vector (`:735-741`) — not from an Euler angle.
        let mut mtx = {
            let Some(o) = self.objects.get(&id) else { return };
            o.transform
        };
        let mut new_pitch_roll = None;
        if has_angles {
            let Some(p) = self.objects.get(&id).and_then(|o| o.physics.as_ref()) else { return };
            let yaw_rate = p.yaw_rate * t.pitch_roll_yaw_factor;
            let mut pitch_rate = p.pitch_rate * t.pitch_roll_yaw_factor;
            let roll_rate = p.roll_rate * t.pitch_roll_yaw_factor;
            if t.center_of_mass_offset != 0.0 {
                let xvec = mtx.x_vector();
                let xy = sqrt(xvec.x * xvec.x + xvec.y * xvec.y);
                let pitch_angle = crate::math::atan2(xvec.z, xy);
                let remaining = if t.center_of_mass_offset > 0.0 { (PI / 2.0) - pitch_angle } else { -(PI / 2.0) + pitch_angle };
                pitch_rate *= sin(remaining);
            }
            mtx.rotate_x(sin(roll_rate), cos(roll_rate));
            mtx.rotate_y(sin(pitch_rate), cos(pitch_rate));
            mtx.rotate_z(sin(yaw_rate), cos(yaw_rate));
            new_pitch_roll = Some((p.pitch + pitch_rate, p.roll + roll_rate));
        }

        // Ground collision (`handleBounce`, PhysicsUpdate.cpp:500-556, inlined).
        let ground = self.terrain.surface_height(pos.x, pos.y, pos.z);
        let mut got_bounce = false;
        let mut bounce_force = Coord3D::ZERO;
        let bouncing = self
            .objects
            .get(&id)
            .and_then(|o| o.physics.as_ref())
            .is_some_and(|p| p.allow_bounce && pos.z <= ground);
        if bouncing {
            let vz = vel.z;
            let desired = if old_z > ground && vz < 0.0 { vz.abs() * stiffness } else { 0.0 };
            // `bounceForce->z = getMass() * desiredAccelZ;` (`PhysicsUpdate.cpp:519`).
            bounce_force = Coord3D::new(0.0, 0.0, self.get_mass(id) * desired);
            if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
                p.yaw_rate *= 0.7;
                p.roll_rate *= 0.7;
                p.pitch_rate *= 0.7;
            }
            // `handleBounce`'s `if (vz < 0) setAngles(yaw, 0, rollAngle)` (`:526-532`) is not modelled: its
            // matrix write is always overwritten in the same `update()` — by the re-right at `:799-811` when
            // `bounceForce.z > 0`, and by `setTransformMatrix(&mtx)` at `:814` otherwise — and `setAngles` uses
            // pre-rotations, so `Get_Z_Rotation()` reads the same yaw before and after the flip.
            if bounce_force.z > 0.0 {
                got_bounce = true;
            } else if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
                p.allow_bounce = p.original_allow_bounce;
            }
        }

        // remember z-vel prior to the ground slam (falling damage).
        let active_vel_z = vel.z;
        if pos.z <= ground {
            let dz = ground - pos.z;
            vel.z += dz;
            if vel.z > 0.0 {
                vel.z = 0.0;
            }
            pos.z = ground;
            if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
                p.velocity = vel;
                p.vel_mag = INVALID_VEL_MAG;
                p.allow_to_fall = false;
            }
        } else if pos.z > ground {
            if is_in_freefall {
                // `IS_IN_FREEFALL`: leave z; the freefall state is set by the caller.
            } else if let Some(p) = self.objects.get(&id).and_then(|o| o.physics.as_ref()) {
                if p.stick_to_ground && !allow_to_fall {
                    pos.z = ground;
                }
            }
        }

        // `m_accel.zero(); m_previousOverlap = m_currentOverlap; m_currentOverlap = INVALID_ID`
        // (`PhysicsUpdate.cpp:818-823`): this frame's partition pass fills the current overlap again
        // (`checkForOverlapCollision`).
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.accel = Coord3D::ZERO;
            p.previous_overlap = p.current_overlap;
            p.current_overlap = None;
        }
        if !self.objects.contains_key(&id) {
            return;
        }
        // `PhysicsUpdate.cpp:799-815`: a bounce re-rights the object. `setAngles(yaw, 0, 0)` reads the yaw from
        // the object's *own* matrix, which is still last frame's (the integrated `mtx` is not written yet), so
        // this frame's angular rates are discarded; the translation then comes from the integrated matrix. The
        // no-bounce path takes the integrated matrix, rotations included.
        if got_bounce {
            let yaw = self.objects.get(&id).map_or(0.0, |o| o.transform.z_rotation());
            self.set_angles(id, yaw, 0.0, 0.0);
            if let Some(o) = self.objects.get_mut(&id) {
                let mut after = o.transform;
                after.set_translation(pos);
                o.set_transform_matrix(after);
            }
        } else if let Some(o) = self.objects.get_mut(&id) {
            mtx.set_translation(pos);
            o.set_transform_matrix(mtx);
            if let Some(p) = o.physics.as_mut() {
                if let Some((pitch, roll)) = new_pitch_roll {
                    p.pitch = pitch;
                    p.roll = roll;
                }
            }
        }
        // `if (gotBounceForce && getFlag(ALLOW_BOUNCE)) applyForce(&bounceForce);` (`PhysicsUpdate.cpp:825-828`).
        if got_bounce {
            self.apply_force(id, bounce_force);
        }
        let airborne_at_end = self
            .objects
            .get(&id)
            .is_some_and(|o| o.position.z - self.terrain.ground_height(o.position.x, o.position.y) > 0.0);
        self.physics_landing_tail(id, &t, vel, active_vel_z, airborne_at_end);
    }

    /// The tail every `PhysicsBehavior::update` runs after the integration, HELD or not (`PhysicsUpdate.cpp:830-913`):
    /// `airborneAtEnd = obj->isAboveTerrain()`, the `WAS_AIRBORNE_LAST_FRAME && !airborneAtEnd` landing block (the
    /// ground collision notification and the falling damage), the freefall clear, `KillWhenRestingOnGround` and the
    /// two flags `calcSleepTime` reads. `vel` is `m_vel` (`:649-657`; not integrated while HELD) and `active_vel_z`
    /// the z velocity remembered before the ground slam (`:761`; 0 while HELD, since that assignment is inside the
    /// integration guard).
    fn physics_landing_tail(&mut self, id: ObjectId, t: &PhysicsTemplate, vel: Coord3D, active_vel_z: f32, airborne_at_end: bool) {
        let was_airborne = self.objects.get(&id).and_then(|o| o.physics.as_ref()).is_some_and(|p| p.was_airborne_last_frame);
        // `if (getFlag(WAS_AIRBORNE_LAST_FRAME) && !airborneAtEnd && !getFlag(IMMUNE_TO_FALLING_DAMAGE))` (`:836`).
        // No shipped template sets `ImmuneToFallingDamage` (the port carries no such field), so the test is absent.
        if was_airborne && !airborne_at_end {
            // `doBounceSound(prevPos)` (`:838`) is audio only.
            // `obj->onCollide(nullptr, obj->getPosition(), &normal)` with a straight-down normal (`:842-845`).
            self.ground_landing_collision(id);
            let net = -active_vel_z - t.min_fall_speed_for_damage;
            if net > 0.0 {
                const MIN_ANGLE_TAN: f32 = 3.0;
                const TINY_DELTA: f32 = 0.01;
                let vx_ok = vel.x.abs() <= TINY_DELTA || (active_vel_z / vel.x).abs() >= MIN_ANGLE_TAN;
                let vy_ok = vel.y.abs() <= TINY_DELTA || (active_vel_z / vel.y).abs() >= MIN_ANGLE_TAN;
                if vx_ok && vy_ok {
                    // `Real damageAmt = netSpeed * getMass() * d->m_fallHeightDamageFactor;` (`PhysicsUpdate.cpp:866`).
                    let damage = net * self.get_mass(id) * t.fall_height_damage_factor;
                    self.damage_object_with_death(
                        id,
                        crate::body::DamageInfo {
                            source: id,
                            amount: damage,
                            kind: crate::damage::DamageType::Falling,
                        },
                        crate::damage::DeathType::Splatted,
                    );
                }
            }
        }
        // `if (!airborneAtEnd) { setFlag(IS_IN_FREEFALL, false); clearDisabled(DISABLED_FREEFALL);
        // clearModelConditionState(MODELCONDITION_FREEFALL); }` (`:886-893`).
        if !airborne_at_end {
            if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
                p.is_in_freefall = false;
            }
            self.clear_disabled(id, DisabledType::Freefall);
        }
        // `if (d->m_killWhenRestingOnGround && !airborneAtEnd && isVerySmall3D(m_vel))` (`:896-907`): "Must be one of
        // the following cases in order to splat: 1) Not a drone, 2) Dead drone, 3) Unmanned drone".
        let small = vel.x.abs() < 0.01 && vel.y.abs() < 0.01 && vel.z.abs() < 0.01;
        if t.kill_when_resting_on_ground && !airborne_at_end && small {
            let splat = self.objects.get(&id).is_some_and(|o| {
                !o.is_kind_of(crate::template::kind_of::DRONE)
                    || o.effectively_dead
                    || o.is_disabled_by_type(DisabledType::Unmanned)
            });
            if splat {
                self.destroy_object(id);
            }
        }
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            // `setFlag(UPDATE_EVER_RUN, true); setFlag(WAS_AIRBORNE_LAST_FRAME, airborneAtEnd);` (`:909-910`).
            p.update_ever_run = true;
            p.was_airborne_last_frame = airborne_at_end;
        }
    }

    /// `obj->onCollide(nullptr, obj->getPosition(), &normal)` (`PhysicsUpdate.cpp:842-845`): a null `other`
    /// means the ground. The object runs its *own* collide modules with that null (`Object::onCollide`,
    /// `Object.cpp:2438-2459` in the engine's module order); `PhysicsBehavior::onCollide`'s null branch
    /// (`:1167-1175`) forwards the contact to the object's container and `ParachuteContain::onCollide`
    /// (`ParachuteContain.cpp:674-687`) is what acts on it. The location is the *rider's* position.
    fn ground_landing_collision(&mut self, id: ObjectId) {
        let Some(pos) = self.objects.get(&id).map(|o| o.position) else { return };
        let normal = Coord3D::new(0.0, 0.0, -1.0);
        self.on_collide(id, ObjectId::INVALID, pos, normal);
    }

    /// `PhysicsBehavior::applyFrictionalForces` (`PhysicsUpdate.cpp:446-496`): lateral + forward
    /// friction when on the ground, aerodynamic drag when significantly airborne. The friction is a
    /// *force* in the original (`lf = mass * lateralFriction`, then `applyForce` divides by the mass
    /// again and applies the motive force's lateral-only filter); the port keeps those two roundings
    /// and the filter, so the acceleration matches to the last bit.
    fn apply_frictional_forces(&mut self, id: ObjectId, t: &PhysicsTemplate, angle: f32, gravity: f32) {
        let (vel, height) = {
            let Some(o) = self.objects.get(&id) else { return };
            let g = self.terrain.surface_height(o.position.x, o.position.y, o.position.z);
            (o.physics.as_ref().map(|p| p.velocity).unwrap_or(Coord3D::ZERO), o.position.z - g)
        };
        let significantly_above = height > -(9.0) * gravity;
        let apply_2d = {
            let Some(p) = self.objects.get(&id).and_then(|o| o.physics.as_ref()) else { return };
            p.apply_friction_2d_when_airborne
        };

        if apply_2d || !significantly_above {
            // `applyYPRDamping(1 - DEFAULT_LATERAL_FRICTION)`.
            if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
                let f = 1.0 - DEFAULT_LATERAL_FRICTION;
                p.pitch_rate *= f;
                p.roll_rate *= f;
                p.yaw_rate *= f;
            }
            if vel.x != 0.0 || vel.y != 0.0 {
                let (dx, dy) = (cos(angle), sin(angle));
                // `Real mass = getMass();` (`PhysicsUpdate.cpp:460`): the loaded transport's friction
                // force scales with what it carries, and `applyForce` divides by the same value.
                let mass = self.get_mass(id);
                let lateral = vel.x * (-dy) + vel.y * dx;
                let lateral_x = lateral * -dy;
                let lateral_y = lateral * dx;
                let lf = {
                    let Some(p) = self.objects.get(&id).and_then(|o| o.physics.as_ref()) else { return };
                    mass * p.lateral_friction(t)
                };
                let mut force = Coord3D::new(-(lf * lateral_x), -(lf * lateral_y), 0.0);
                let motive = self.objects.get(&id).and_then(|o| o.physics.as_ref()).is_some_and(|p| p.is_motive(self.frame));
                if !motive {
                    let forward = vel.x * dx + vel.y * dy;
                    let ff = {
                        let Some(p) = self.objects.get(&id).and_then(|o| o.physics.as_ref()) else { return };
                        mass * p.forward_friction(t)
                    };
                    force.x += -(ff * (forward * dx));
                    force.y += -(ff * (forward * dy));
                }
                // `applyForce(&accel)` (`PhysicsUpdate.cpp:322-343`), without its wake: the physics
                // module is inside its own update, where the original's `setWakeFrame` is skipped.
                let (mut fx, mut fy) = (force.x, force.y);
                if motive {
                    let lateral = force.x * (-dy) + force.y * dx;
                    fx = lateral * -dy;
                    fy = lateral * dx;
                }
                let mass_inv = 1.0 / mass;
                if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
                    p.accel.x += fx * mass_inv;
                    p.accel.y += fy * mass_inv;
                }
            }
        } else {
            let aero = {
                let Some(p) = self.objects.get(&id).and_then(|o| o.physics.as_ref()) else { return };
                -p.aerodynamic_friction(t)
            };
            if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
                p.accel.x += vel.x * aero;
                p.accel.y += vel.y * aero;
                p.accel.z += vel.z * aero;
                let f = 1.0 + aero;
                p.pitch_rate *= f;
                p.roll_rate *= f;
                p.yaw_rate *= f;
            }
        }
    }
}

/// Lane 168 (the R18 review, R18-7): the heading written into the transform by `set_angles` and the yaw (`physics_l168.rs`).
#[cfg(test)]
#[path = "physics_l168.rs"]
mod l168_tests;

/// Lane 172 (ROADMAP 1(d)): the pitch/roll rates folded into the matrix and the bounce re-right (`physics_l172.rs`).
#[cfg(test)]
#[path = "physics_l172.rs"]
mod l172_tests;

/// Lane 173 (wave 20): the full HELD physics tail, R19-5 (`physics_l173.rs`).
#[cfg(test)]
#[path = "physics_l173.rs"]
mod l173_tests;

/// Lane 176 (wave 20): `getMass` includes the contained items' mass (R20-6, `physics_l176.rs`).
#[cfg(test)]
#[path = "physics_l176.rs"]
mod l176_tests;

#[cfg(test)]
mod sleep_and_force_tests {
    //! `applyForce`'s reciprocal mass (`PhysicsUpdate.cpp:335-338`) and `calcSleepTime`
    //! (`:919-936`) with its `HAS_PITCHROLLYAW`, layer, `isAboveTerrain`, overlap and
    //! `UPDATE_EVER_RUN` terms.
    use super::*;
    use crate::bridge::{Bridge, LAYER_GROUND};
    use crate::template::{kind_of, ObjectTemplate};
    use crate::terrain::TerrainLogic;
    use std::sync::Arc;

    fn chunk(name: &str) -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::VEHICLE;
        t.geometry.major_radius = 2.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.physics = Some(PhysicsTemplate { mass: 1.0, ..PhysicsTemplate::default() });
        Arc::new(t)
    }

    /// `setWakeFrame(getObject(), UPDATE_SLEEP_NONE)` in the constructor (`PhysicsUpdate.cpp:231-233`,
    /// `UpdateModule.cpp:80-84`): the module's first update is one frame after creation, so a force
    /// applied on the creation frame is not integrated until then.
    #[test]
    fn the_first_physics_update_is_the_frame_after_creation() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        let id = g.create_object(&chunk("Faller"), Coord3D::new(50.0, 50.0, 20.0), 0.0, 0);
        assert_eq!(g.physics_wake_frame(id), 1, "scheduled for creation frame + 1");
        g.add_velocity_to(id, Coord3D::new(0.0, 0.0, -1.0));
        g.update(); // frame 0
        assert_eq!(g.objects.get(&id).unwrap().position.z, 20.0, "no physics on the creation frame");
        g.update(); // frame 1
        assert!(g.objects.get(&id).unwrap().position.z < 20.0, "the frame after creation integrates the velocity");
    }

    /// `!getFlag(HAS_PITCHROLLYAW)` (`PhysicsUpdate.cpp:924`): a spinning object does not sleep even
    /// with zero velocity and acceleration.
    #[test]
    fn a_spinning_object_does_not_sleep() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        let id = g.create_object(&chunk("Spinner"), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        g.set_angular_rates(id, 0.1, 0.2, 0.3);
        g.update();
        g.update(); // the first physics update
        let p = g.objects.get(&id).unwrap().physics.as_ref().unwrap();
        assert!(p.has_pitch_roll_yaw(), "still spinning (damped but non-zero)");
        assert_ne!(g.physics_wake_frame(id), u32::MAX, "a spinning module stays awake");
    }

    /// `getObject()->getLayer() == LAYER_GROUND` (`PhysicsUpdate.cpp:925`): an object standing on a
    /// bridge deck is on the bridge's layer, so it never sleeps.
    #[test]
    fn a_unit_on_a_bridge_deck_does_not_sleep() {
        let mut terrain = TerrainLogic::flat(100, 100, 0);
        terrain.add_bridge(Bridge::new(Coord3D::new(20.0, 50.0, 10.0), Coord3D::new(80.0, 50.0, 10.0), 40.0, 2));
        assert_eq!(terrain.layer_for_destination(&Coord3D::new(50.0, 50.0, 10.0)), 2, "the deck is the closest layer");
        let mut g = GameLogic::new(terrain, 1);
        // No gravity: the object then rests exactly on the deck (with gravity its first update leaves a
        // sub-microscopic z residue that keeps it awake on its own).
        g.rules.gravity = 0.0;
        let id = g.create_object(&chunk("Walker"), Coord3D::new(50.0, 50.0, 10.0), 0.0, 0);
        g.update();
        g.update();
        assert_ne!(g.physics_wake_frame(id), u32::MAX, "a unit on a bridge deck is not on LAYER_GROUND");
    }

    /// `!isAboveTerrain()` (`PhysicsUpdate.cpp:925`, Thing.cpp:293-300, Thing.h:129):
    /// `pos.z - getGroundHeight(x, y) > 0` strictly, with no epsilon.
    #[test]
    fn a_unit_hair_above_the_terrain_does_not_sleep() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        // No gravity, so the 0.0005 offset survives the update (the engine's units would sink and land).
        g.rules.gravity = 0.0;
        let id = g.create_object(&chunk("Floater"), Coord3D::new(50.0, 50.0, 0.0005), 0.0, 0);
        g.update();
        g.update();
        assert_ne!(g.physics_wake_frame(id), u32::MAX, "0.0005 above the ground is still above the terrain");
        // A unit exactly on the ground does sleep (nothing else keeps it awake).
        let grounded = g.create_object(&chunk("Sleeper"), Coord3D::new(60.0, 60.0, 0.0), 0.0, 0);
        g.update();
        g.update();
        assert_eq!(g.physics_wake_frame(grounded), u32::MAX, "resting on the ground sleeps");
    }

    /// `getCurrentOverlap() == INVALID_ID && getPreviousOverlap() == INVALID_ID`
    /// (`PhysicsUpdate.cpp:927-928`) and the reset at the end of `update()` (`:824-825`): an overlap
    /// keeps the module awake for the frame after it, then it may sleep again.
    #[test]
    fn an_overlap_keeps_the_physics_awake_for_a_frame() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        let id = g.create_object(&chunk("Sleeper"), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        {
            let p = g.objects.get_mut(&id).unwrap().physics.as_mut().unwrap();
            p.current_overlap = Some(ObjectId(99));
        }
        g.physics_due_now(id);
        g.update(); // frame 0: the overlap is moved to `previous_overlap`
        {
            let p = g.objects.get(&id).unwrap().physics.as_ref().unwrap();
            assert_eq!(p.current_overlap, None, "reset for this frame's collide pass");
            assert_eq!(p.previous_overlap, Some(ObjectId(99)));
            assert_ne!(g.physics_wake_frame(id), u32::MAX, "the overlap keeps it awake");
        }
        g.update(); // frame 1: both overlaps are clear now
        assert_eq!(g.physics_wake_frame(id), u32::MAX, "no overlap: it sleeps");
    }

    /// `checkForOverlapCollision` records the crusher's overlap (`addOverlap`,
    /// `PhysicsUpdate.cpp:1050-1056` called at `:1523`), which `calcSleepTime` then reads.
    #[test]
    fn a_moving_crusher_records_its_overlap() {
        let mut crusher = ObjectTemplate::new("Crusher");
        crusher.kind_of = kind_of::VEHICLE;
        crusher.geometry.major_radius = 5.0;
        crusher.max_health = 100.0;
        crusher.initial_health = 100.0;
        crusher.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
        crusher.crush.crusher_level = 1;
        let mut victim = ObjectTemplate::new("Victim");
        victim.kind_of = kind_of::VEHICLE;
        victim.geometry.major_radius = 5.0;
        victim.max_health = 100.0;
        victim.initial_health = 100.0;
        victim.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        let a = g.create_object(&Arc::new(crusher), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let b = g.create_object(&Arc::new(victim), Coord3D::new(52.0, 50.0, 0.0), 0.0, 1);
        // Only a moving crusher may crush (and record the overlap) (`PhysicsUpdate.cpp:1480-1482`).
        g.add_velocity_to(a, Coord3D::new(1.0, 0.0, 0.0));
        g.process_object_collisions();
        assert_eq!(g.objects.get(&a).unwrap().physics.as_ref().unwrap().current_overlap, Some(b));
        assert_eq!(g.objects.get(&b).unwrap().physics.as_ref().unwrap().current_overlap, None, "only the crusher records it");
    }

    /// `massInv = 1.0f / mass; m_accel += modForce * massInv` (`PhysicsUpdate.cpp:335-338`):
    /// `2.5 * (1/3)` is not the same f32 as `2.5 / 3` (they differ by one ulp).
    #[test]
    fn a_force_is_multiplied_by_the_reciprocal_mass() {
        assert_ne!(2.5f32 * (1.0f32 / 3.0), 2.5f32 / 3.0, "the two spellings differ in f32");
        let mut t = ObjectTemplate::new("Heavy");
        t.kind_of = kind_of::VEHICLE;
        t.geometry.major_radius = 2.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.physics = Some(PhysicsTemplate { mass: 3.0, ..PhysicsTemplate::default() });
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        g.apply_force(id, Coord3D::new(2.5, 0.0, 0.0));
        let accel = g.objects.get(&id).unwrap().physics.as_ref().unwrap().accel.x;
        assert_eq!(accel, 2.5f32 * (1.0f32 / 3.0), "the reciprocal multiply");
        assert_ne!(accel, 2.5f32 / 3.0, "not a division");
    }

    /// The ground layer test uses `LAYER_GROUND` (`PathfindLayerEnum`), which is 1 here; the helper
    /// above relies on `TerrainLogic::getLayerForDestination` returning it for open ground.
    #[test]
    fn open_ground_is_the_ground_layer() {
        let terrain = TerrainLogic::flat(100, 100, 0);
        assert_eq!(terrain.layer_for_destination(&Coord3D::new(50.0, 50.0, 0.0)), LAYER_GROUND);
    }

    /// A unit standing on a wall is on the wall layer (`Object::getLayer()`), never `LAYER_GROUND`;
    /// for a wall the port's port of that test is `isAboveTerrain` (a wall-top unit is always above
    /// the terrain: the wall layer is only chosen above half the wall height).
    #[test]
    fn a_unit_on_a_wall_never_counts_as_on_the_ground_layer() {
        use crate::template::{Geometry, GeometryShape};
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        g.rules.gravity = 0.0;
        g.pathfinder.set_wall_height(30.0);
        let wall = Geometry { shape: GeometryShape::Box, major_radius: 100.0, minor_radius: 20.0, height: 40.0, is_small: false };
        g.pathfinder.add_wall_piece(ObjectId(3), &wall, &Coord3D::new(200.0, 100.0, 0.0), 0.0);
        let id = g.create_object(&chunk("WallWalker"), Coord3D::new(200.0, 100.0, 30.0), 0.0, 0);
        assert_eq!(
            g.pathfinder.layer_for_destination(&g.terrain, &Coord3D::new(200.0, 100.0, 30.0)),
            crate::pathfind::PathLayer::Wall,
            "the position is on the wall layer"
        );
        g.update();
        g.update();
        assert_ne!(g.physics_wake_frame(id), u32::MAX, "a unit on a wall is not on LAYER_GROUND");
    }

    /// `PhysicsUpdate.cpp:663-678`: `OBJECT_STATUS_BRAKING` freezes x/y, and a braking `KINDOF_PROJECTILE`
    /// is not moved at all — not even in z ("Things other than projectiles don't cheat in z"). The port
    /// still integrated `vel.z` for a braking projectile.
    #[test]
    fn a_braking_projectile_is_not_moved_in_z_either() {
        let mut shell = ObjectTemplate::new("Shell");
        shell.kind_of = kind_of::PROJECTILE | kind_of::VEHICLE;
        shell.geometry.major_radius = 1.0;
        shell.physics = Some(PhysicsTemplate { mass: 1.0, ..PhysicsTemplate::default() });
        let shell = Arc::new(shell);
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        // High above the terrain and allowed to fall, so only the position step moves z.
        let id = g.create_object(&shell, Coord3D::new(50.0, 50.0, 40.0), 0.0, 0);
        let rock = g.create_object(&chunk("Rock"), Coord3D::new(20.0, 20.0, 40.0), 0.0, 0);
        for who in [id, rock] {
            let o = g.objects.get_mut(&who).unwrap();
            o.status_bits |= crate::object::status::IS_BRAKING;
            let p = o.physics.as_mut().unwrap();
            p.velocity = Coord3D::new(0.0, 0.0, -1.0);
            p.allow_to_fall = true;
        }
        g.physics_due_now(id);
        g.physics_due_now(rock);
        g.update();
        assert_eq!(g.objects[&id].position, Coord3D::new(50.0, 50.0, 40.0), "a braking projectile does not move at all");
        assert!(g.objects[&rock].position.z < 40.0, "a braking non-projectile still moves in z");
    }
}
