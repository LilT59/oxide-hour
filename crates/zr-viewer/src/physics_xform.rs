//! The client-side vehicle physics transform: `Drawable::calcPhysicsXform*` and `applyPhysicsXform`.
//!
//! Ported from `GeneralsMD/Code/GameEngine/Source/GameClient/Drawable.cpp`:
//! [`DrawableLocoInfo`](DrawableLocoInfo) is the per-drawable state (:174-199), [`LocoInfo::update`] is
//! `Drawable::calcPhysicsXform`'s appearance switch (:1382-1429) with `calcPhysicsXformTreads`
//! (:1623-1877), `calcPhysicsXformWheels` (:1881-2176), `calcPhysicsXformMotorcycle` (:2180-2474),
//! `calcPhysicsXformHoverOrWings` (:1512-1619) and `calcPhysicsXformThrust` (:1433-1508), and [`apply`]
//! is `Drawable::applyPhysicsXform` (:1362-1378).
//!
//! The engine recomputes this once per WW3D sync frame, i.e. at the 30 Hz logic rate (:1366-1371), and
//! post-multiplies the result onto the object's transform before every draw module runs
//! (`Drawable::draw`, :2642-2660): `Translate(0, 0, m_totalZ)`, `Rotate_Y(m_totalPitch)`,
//! `Rotate_X(-m_totalRoll)`, `Rotate_Z(m_totalYaw)`. The inputs are the drawable's interpolated
//! position and orientation (:1658-1661, 1917-1919), the object's physics velocity and acceleration,
//! the terrain normal under the chassis (`TerrainLogic::getLayerHeight`, :1671, 1930), the object it is
//! crushing (:1677-1757) and the locomotor template's client fields (`Locomotor.cpp` getters). The
//! wheel steering angle and the four suspension height offsets stay in the drawable's
//! [`TWheelInfo`](WheelInfo) for the tire bones (`W3DTruckDraw::doDrawModule`,
//! `W3DTankTruckDraw::doDrawModule`).
//!
//! This is client-only: it reads the simulation's objects and never writes them, and its random draws
//! (the suspension bounce, :1977, and the rough crush vibration, :1734-1755) come from a client RNG,
//! never the simulation's.
//!
//! Not modelled: the object transform's own pitch/roll from `PhysicsBehavior` (`PhysicsUpdate.cpp`
//! keeps them in the object matrix; the viewer's base transform is yaw only), bridge-deck normals (the
//! ground heightmap's smoothed normal is used for every layer), and `MaximumWheelCompression` (its
//! clamp is commented out in the engine, :2152-2165). The thrust wobble is ported but no drawn object
//! takes it yet: projectiles are drawn by `projectile_draw` from the sim's projectile snapshots, not as
//! `sim_sync` objects.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use zr_fx::ClientRng;
use zr_map::HeightMap;
use zr_sim::physics::Physics;
use zr_sim::template::{Geometry, GeometryShape, LocomotorSet};
use zr_sim::ObjectId;
use zr_templates::{KindOf, LocomotorAppearance, LocomotorSetType, LocomotorTemplate, Templates};

use crate::shroud::GhostSnapshot;
use crate::sim_view::{SimObject, SimRes, STEP};

/// `MAP_XY_FACTOR`: world units per height cell.
use crate::source::{MAP_HEIGHT_SCALE, MAP_XY_FACTOR};

/// `OVERLAP_SHRINK_FACTOR` (`Drawable.cpp:1628`).
const OVERLAP_SHRINK_FACTOR: f32 = 0.8;
/// `FLATTENED_OBJECT_HEIGHT` (`:1629`).
const FLATTENED_OBJECT_HEIGHT: f32 = 0.5;
/// `LEAVE_OVERLAP_PITCH_KICK` (`:1630`).
const LEAVE_OVERLAP_PITCH_KICK: f32 = std::f32::consts::PI / 128.0;
/// `OVERLAP_ROUGH_VIBRATION_FACTOR` (`:1631`).
const OVERLAP_ROUGH_VIBRATION_FACTOR: f32 = 5.0;
/// `MAX_ROUGH_VIBRATION` (`:1632`).
const MAX_ROUGH_VIBRATION: f32 = 0.5;
/// `WHEEL_SMOOTHNESS` (`:2085`, `:2396`).
const WHEEL_SMOOTHNESS: f32 = 10.0;
/// `SPRING_FACTOR` (`:2088`, `:2399`).
const SPRING_FACTOR: f32 = 0.9;

/// `TWheelInfo` (`Drawable.h:124-131`): wheel offset and steering state for a wheeled locomotor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct WheelInfo {
    pub front_left: f32,
    pub front_right: f32,
    pub rear_left: f32,
    pub rear_right: f32,
    pub frames_airborne_counter: i32,
    pub frames_airborne: i32,
    /// `m_wheelAngle`: the steering angle of the front tires (radians).
    pub wheel_angle: f32,
}

/// `PhysicsXformInfo` (`Drawable.h`), the persistent `m_physicsXform`: the engine writes it in place, so
/// a branch that returns early (the airborne wheels case) keeps the previous frame's tilt.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Xform {
    pub pitch: f32,
    pub roll: f32,
    pub yaw: f32,
    pub z: f32,
}

/// `DrawableLocoInfo` (`Drawable.cpp:174-199`): the chassis spring state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LocoInfo {
    pitch: f32,
    pitch_rate: f32,
    roll: f32,
    roll_rate: f32,
    yaw: f32,
    accel_pitch: f32,
    accel_pitch_rate: f32,
    accel_roll: f32,
    accel_roll_rate: f32,
    overlap_z_vel: f32,
    overlap_z: f32,
    /// `m_wobble`: `1.0` while the thrust pitch is rising, `-1.0` while it falls.
    wobble: f32,
    pub wheel: WheelInfo,
    yaw_modulator: f32,
    pitch_modulator: f32,
}

impl Default for LocoInfo {
    fn default() -> Self {
        // `DrawableLocoInfo::DrawableLocoInfo` (`:174-199`): every value zero except `m_wobble = 1`.
        LocoInfo {
            pitch: 0.0,
            pitch_rate: 0.0,
            roll: 0.0,
            roll_rate: 0.0,
            yaw: 0.0,
            accel_pitch: 0.0,
            accel_pitch_rate: 0.0,
            accel_roll: 0.0,
            accel_roll_rate: 0.0,
            overlap_z_vel: 0.0,
            overlap_z: 0.0,
            wobble: 1.0,
            wheel: WheelInfo::default(),
            yaw_modulator: 0.0,
            pitch_modulator: 0.0,
        }
    }
}

/// One update's read-only inputs, the locals `Drawable::calcPhysicsXform*` reads from the object.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    /// `getUnitDirectionVector2D`: `(cos angle, sin angle)`.
    pub dir: [f32; 2],
    /// `PhysicsBehavior::getVelocity()` (world units per logic frame).
    pub velocity: [f32; 3],
    /// `PhysicsBehavior::getAcceleration()` (the engine has already reset it for the next frame).
    pub accel: [f32; 3],
    /// `getVelocityMagnitude()`.
    pub speed: f32,
    /// `getForwardSpeed2D()`, negative when reversing.
    pub forward_speed_2d: f32,
    /// `getTurning()`.
    pub turning: i8,
    /// `isMotive()`.
    pub motive: bool,
    /// `AIUpdateInterface::getCurLocomotorSpeed()`.
    pub max_speed: f32,
    /// `Object::isSignificantlyAboveTerrain()`.
    pub airborne: bool,
    /// `position.z - getLayerHeight(x, y, layer)`, the wheels' airborne extension test.
    pub height_above_terrain: f32,
    /// `GeometryInfo::getMajorRadius()`.
    pub major_radius: f32,
    /// `GeometryInfo::getMinorRadius()`.
    pub minor_radius: f32,
    /// The smoothed terrain normal at the chassis (`BaseHeightMap::getHeightMapHeight`).
    pub normal: [f32; 3],
    /// `obj->getAIUpdateInterface() != nullptr` (every physics branch needs an AI).
    pub has_ai: bool,
    /// The object being crushed this frame (`physics->getCurrentOverlap()`).
    pub overlap: Option<Overlap>,
    /// `physics->getPreviousOverlap() != INVALID_ID`.
    pub previous_overlap: bool,
}

/// The overlapping object's contribution to the treads' surface normal (`:1666-1757`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Overlap {
    pub dx: f32,
    pub dy: f32,
    pub center_dist_sqr: f32,
    /// `(ourCircle + otherCircle) * OVERLAP_SHRINK_FACTOR`.
    pub max_center_dist: f32,
    /// `getMaxHeightAbovePosition()` (or `FLATTENED_OBJECT_HEIGHT` when flat).
    pub height: f32,
    /// `KINDOF_LOW_OVERLAPPABLE` / `KINDOF_INFANTRY` / crushed front and back.
    pub flat: bool,
}

/// The locomotor template fields the client physics transform reads.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LocoParams {
    pub appearance: LocomotorAppearance,
    accel_pitch_limit: f32,
    decel_pitch_limit: f32,
    bounce_kick: f32,
    pitch_stiffness: f32,
    roll_stiffness: f32,
    pitch_damping: f32,
    roll_damping: f32,
    forward_vel_coef: f32,
    lateral_vel_coef: f32,
    pitch_by_z_vel_coef: f32,
    forward_accel_coef: f32,
    lateral_accel_coef: f32,
    uniform_axial_damping: f32,
    max_wheel_extension: f32,
    wheel_turn_angle: f32,
    has_suspension: bool,
    thrust_roll: f32,
    wobble_rate: f32,
    min_wobble: f32,
    max_wobble: f32,
    rudder_correction_degree: f32,
    rudder_correction_rate: f32,
    elevator_correction_degree: f32,
    elevator_correction_rate: f32,
}

impl LocoParams {
    pub(crate) fn from_template(t: &LocomotorTemplate) -> Self {
        LocoParams {
            appearance: t.appearance,
            accel_pitch_limit: t.accel_pitch_limit,
            decel_pitch_limit: t.decel_pitch_limit,
            bounce_kick: t.bounce_kick,
            pitch_stiffness: t.pitch_stiffness,
            roll_stiffness: t.roll_stiffness,
            pitch_damping: t.pitch_damping,
            roll_damping: t.roll_damping,
            forward_vel_coef: t.forward_vel_coef,
            lateral_vel_coef: t.lateral_vel_coef,
            pitch_by_z_vel_coef: t.pitch_by_z_vel_coef,
            forward_accel_coef: t.forward_accel_coef,
            lateral_accel_coef: t.lateral_accel_coef,
            uniform_axial_damping: t.uniform_axial_damping,
            max_wheel_extension: t.maximum_wheel_extension,
            wheel_turn_angle: t.wheel_turn_angle,
            has_suspension: t.has_suspension,
            thrust_roll: t.thrust_roll,
            wobble_rate: t.wobble_rate,
            min_wobble: t.min_wobble,
            max_wobble: t.max_wobble,
            rudder_correction_degree: t.rudder_correction_degree,
            rudder_correction_rate: t.rudder_correction_rate,
            elevator_correction_degree: t.elevator_correction_degree,
            elevator_correction_rate: t.elevator_correction_rate,
        }
    }
}

/// Whether `calcPhysicsXform` handles this appearance (`Drawable.cpp:1388-1411`): the drawable then has
/// a `m_physicsXform` and a `DrawableLocoInfo`, which is what `getWheelInfo()` reports.
pub(crate) fn is_physics_appearance(a: LocomotorAppearance) -> bool {
    matches!(
        a,
        LocomotorAppearance::FourWheels
            | LocomotorAppearance::Motorcycle
            | LocomotorAppearance::Treads
            | LocomotorAppearance::Hover
            | LocomotorAppearance::Wings
            | LocomotorAppearance::Thrust
    )
}

/// `Drawable::getLocomotor()->getAppearance()` for an object: the first resolved template of its current
/// locomotor set (the adapter's `convert_locomotor_sets` rule).
pub(crate) fn appearance_of(templates: &Templates, template: &str, set: LocomotorSet) -> Option<LocomotorAppearance> {
    locomotor_template(templates, template, set).map(|t| t.appearance)
}

/// The `LocomotorTemplate` an object's current set uses, or `None` when it has no locomotor.
fn locomotor_template<'a>(templates: &'a Templates, template: &str, set: LocomotorSet) -> Option<&'a LocomotorTemplate> {
    let ids = templates.thing(template)?.locomotor_set(set_type(set));
    let id = ids.iter().find_map(|r| r.id)?;
    Some(templates.locomotor_by_id(id))
}

fn set_type(set: LocomotorSet) -> LocomotorSetType {
    match set {
        LocomotorSet::Normal => LocomotorSetType::SetNormal,
        LocomotorSet::NormalUpgraded => LocomotorSetType::SetNormalUpgraded,
        LocomotorSet::Freefall => LocomotorSetType::SetFreefall,
        LocomotorSet::Wander => LocomotorSetType::SetWander,
        LocomotorSet::Panic => LocomotorSetType::SetPanic,
        LocomotorSet::Taxiing => LocomotorSetType::SetTaxiing,
        LocomotorSet::Supersonic => LocomotorSetType::SetSupersonic,
        LocomotorSet::Sluggish => LocomotorSetType::SetSluggish,
    }
}

fn sin(x: f32) -> f32 {
    x.sin()
}

fn cos(x: f32) -> f32 {
    x.cos()
}

impl LocoInfo {
    /// One `calcPhysicsXform` call for `p`'s appearance, written into the persistent `x`.
    pub(crate) fn update(&mut self, p: &LocoParams, f: &Frame, x: &mut Xform, rng: &mut ClientRng) {
        let has_ai = f.has_ai;
        match p.appearance {
            LocomotorAppearance::FourWheels => {
                if has_ai {
                    self.wheels(p, f, x, rng)
                }
            }
            LocomotorAppearance::Motorcycle => {
                if has_ai {
                    self.motorcycle(p, f, x, rng)
                }
            }
            LocomotorAppearance::Treads => {
                if has_ai {
                    self.treads(p, f, x, rng)
                }
            }
            LocomotorAppearance::Hover | LocomotorAppearance::Wings => {
                if has_ai {
                    self.hover_or_wings(p, f, x);
                }
            }
            LocomotorAppearance::Thrust => self.thrust(p, x),
            _ => return,
        }
        // `:1414-1426`: keep denormalized values out of the matrix.
        x.pitch = scrub(x.pitch);
        x.roll = scrub(x.roll);
        x.yaw = scrub(x.yaw);
        x.z = scrub(x.z);
    }

    /// `calcPhysicsXformThrust` (`:1433-1508`): the scud-missile wobble.
    fn thrust(&mut self, p: &LocoParams, x: &mut Xform) {
        let rate = p.wobble_rate;
        if rate != 0.0 {
            if self.wobble >= 1.0 {
                if self.pitch < p.max_wobble - rate * 2.0 {
                    self.pitch += rate;
                    self.yaw += rate;
                } else {
                    self.pitch += rate / 2.0;
                    self.yaw += rate / 2.0;
                }
                if self.pitch >= p.max_wobble {
                    self.wobble = -1.0;
                }
            } else {
                if self.pitch >= p.min_wobble + rate * 2.0 {
                    self.pitch -= rate;
                    self.yaw -= rate;
                } else {
                    self.pitch -= rate / 2.0;
                    self.yaw -= rate / 2.0;
                }
                if self.pitch <= p.min_wobble {
                    self.wobble = 1.0;
                }
            }
            x.pitch = self.pitch;
            x.yaw = self.yaw;
        }
        if p.thrust_roll != 0.0 {
            self.roll += p.thrust_roll;
            x.roll = self.roll;
        }
    }

    /// `calcPhysicsXformHoverOrWings` (`:1512-1619`): the chassis springs toward level, pitching and
    /// rolling with speed and acceleration, plus the rudder/elevator correction wobble.
    fn hover_or_wings(&mut self, p: &LocoParams, f: &Frame, x: &mut Xform) {
        let (dir, vel, accel) = (f.dir, f.velocity, f.accel);
        self.pitch_rate += (-p.pitch_stiffness * self.pitch) + (-p.pitch_damping * self.pitch_rate);
        self.roll_rate += (-p.roll_stiffness * self.roll) + (-p.roll_damping * self.roll_rate);
        self.pitch += self.pitch_rate * p.uniform_axial_damping;
        self.roll += self.roll_rate * p.uniform_axial_damping;

        self.accel_pitch_rate += (-p.pitch_stiffness * self.accel_pitch) + (-p.pitch_damping * self.accel_pitch_rate);
        self.accel_pitch += self.accel_pitch_rate;
        self.accel_roll_rate += (-p.roll_stiffness * self.accel_roll) + (-p.roll_damping * self.accel_roll_rate);
        self.accel_roll += self.accel_roll_rate;

        x.pitch = self.pitch + self.accel_pitch;
        x.roll = self.roll + self.accel_roll;

        if f.motive {
            if p.pitch_by_z_vel_coef != 0.0 {
                const TINY_DZ: f32 = 0.001;
                if vel[2].abs() > TINY_DZ {
                    let pitch = vel[2].atan2((vel[0] * vel[0] + vel[1] * vel[1]).sqrt());
                    self.pitch -= p.pitch_by_z_vel_coef * pitch;
                }
            }
            let forward_vel = dir[0] * vel[0] + dir[1] * vel[1];
            self.pitch += -(p.forward_vel_coef * forward_vel);
            let lateral_vel = -dir[1] * vel[0] + dir[0] * vel[1];
            self.roll += -(p.lateral_vel_coef * lateral_vel);
            let forward_accel = dir[0] * accel[0] + dir[1] * accel[1];
            self.accel_pitch_rate += -(p.forward_accel_coef * forward_accel);
            let lateral_accel = -dir[1] * accel[0] + dir[0] * accel[1];
            self.accel_roll_rate += -(p.lateral_accel_coef * lateral_accel);
        }

        clamp_accel(p, self);
        // `sin( m_yawModulator += RATE )` / `cos( m_pitchModulator += RATE )`.
        self.yaw_modulator += p.rudder_correction_rate;
        x.yaw = p.rudder_correction_degree * sin(self.yaw_modulator);
        self.pitch_modulator += p.elevator_correction_rate;
        x.pitch += p.elevator_correction_degree * cos(self.pitch_modulator);
        x.z = 0.0;
    }

    /// `calcPhysicsXformTreads` (`:1623-1877`): the tank chassis follows the terrain normal, is kicked
    /// when leaving a crushed object, and offsets up while riding over one.
    fn treads(&mut self, p: &LocoParams, f: &Frame, x: &mut Xform, rng: &mut ClientRng) {
        let (perp_x, perp_y) = (-f.dir[1], f.dir[0]);
        let mut normal = f.normal;
        // `:1673-1757`: override the surface normal while overlapping (crushing) another object.
        let mut overlap_z = 0.0;
        if let Some(o) = &f.overlap {
            let vs = f.velocity[0] * f.velocity[0] + f.velocity[1] * f.velocity[1];
            if o.center_dist_sqr < o.max_center_dist * o.max_center_dist {
                let center_dist = o.center_dist_sqr.sqrt();
                let amount = (1.0 - center_dist / o.max_center_dist).clamp(0.0, 1.0);
                let mut rough = vs * OVERLAP_ROUGH_VIBRATION_FACTOR;
                if rough > MAX_ROUGH_VIBRATION {
                    rough = MAX_ROUGH_VIBRATION;
                }
                let mut height = o.height;
                if o.flat {
                    height = FLATTENED_OBJECT_HEIGHT;
                }
                if amount < FLATTENED_OBJECT_HEIGHT && !o.flat {
                    overlap_z = height * 2.0 * amount;
                    // A vector along the "surface" of the thing we drive over.
                    let v = [o.dx / center_dist, o.dy / center_dist, 0.2];
                    let mut up = [rng.real(-rough, rough), rng.real(-rough, rough), 1.0];
                    up = normalize3(up);
                    let prp = cross3(v, up);
                    normal = normalize3(cross3(prp, v));
                } else {
                    // Sitting on top of the object.
                    overlap_z = height;
                    normal = normalize3([rng.real(-rough, rough), rng.real(-rough, rough), 1.0]);
                }
            }
        } else if f.previous_overlap && self.overlap_z > 0.0 {
            // We had an overlap last frame and are now in the air: kick the pitch.
            self.pitch_rate += LEAVE_OVERLAP_PITCH_KICK;
        }

        let dot = normal[0] * f.dir[0] + normal[1] * f.dir[1];
        let ground_pitch = dot * (std::f32::consts::PI / 2.0);
        let dot = normal[0] * perp_x + normal[1] * perp_y;
        let ground_roll = dot * (std::f32::consts::PI / 2.0);

        if f.overlap.is_some() || self.overlap_z <= 0.0 {
            self.pitch_rate += (-p.pitch_stiffness * (self.pitch - ground_pitch)) + (-p.pitch_damping * self.pitch_rate);
            if self.pitch_rate > 0.0 {
                self.pitch_rate *= 0.5;
            }
            self.roll_rate += (-p.roll_stiffness * (self.roll - ground_roll)) + (-p.roll_damping * self.roll_rate);
        }
        self.pitch += self.pitch_rate * p.uniform_axial_damping;
        self.roll += self.roll_rate * p.uniform_axial_damping;

        self.accel_pitch_rate += (-p.pitch_stiffness * self.accel_pitch) + (-p.pitch_damping * self.accel_pitch_rate);
        self.accel_pitch += self.accel_pitch_rate;
        self.accel_roll_rate += (-p.roll_stiffness * self.accel_roll) + (-p.roll_damping * self.accel_roll_rate);
        self.accel_roll += self.accel_roll_rate;

        x.pitch = self.pitch + self.accel_pitch;
        x.roll = self.roll + self.accel_roll;

        if f.motive {
            let forward_accel = f.dir[0] * f.accel[0] + f.dir[1] * f.accel[1];
            self.accel_pitch_rate += -(p.forward_accel_coef * forward_accel);
            let lateral_accel = -f.dir[1] * f.accel[0] + f.dir[0] * f.accel[1];
            self.accel_roll_rate += -(p.lateral_accel_coef * lateral_accel);
        }

        clamp_accel(p, self);

        if overlap_z > self.overlap_z {
            self.overlap_z = overlap_z;
            self.overlap_z_vel = 0.0;
        }
        let ztmp = self.overlap_z / 2.0;
        if self.overlap_z > 0.0 {
            self.overlap_z_vel -= 0.2;
            self.overlap_z += self.overlap_z_vel;
        }
        if self.overlap_z <= 0.0 {
            self.overlap_z = 0.0;
            self.overlap_z_vel = 0.0;
        }
        x.z = ztmp;
    }

    /// `calcPhysicsXformWheels` (`:1881-2176`): the chassis spring, the bouncy kick, the front-wheel
    /// steering angle and the four suspension height offsets.
    fn wheels(&mut self, p: &LocoParams, f: &Frame, x: &mut Xform, rng: &mut ClientRng) {
        let (perp_x, perp_y) = (-f.dir[1], f.dir[0]);
        let dot = f.normal[0] * f.dir[0] + f.normal[1] * f.dir[1];
        let ground_pitch = dot * (std::f32::consts::PI / 2.0);
        let dot = f.normal[0] * perp_x + f.normal[1] * perp_y;
        let ground_roll = dot * (std::f32::consts::PI / 2.0);
        let do_wheels = p.has_suspension;

        if f.airborne {
            if do_wheels {
                // Wheels extend when airborne.
                self.wheel.frames_airborne = 0;
                self.wheel.frames_airborne_counter += 1;
                if f.height_above_terrain > -p.max_wheel_extension {
                    self.wheel.rear_left += (p.max_wheel_extension - self.wheel.rear_left) / 2.0;
                    self.wheel.rear_right += (p.max_wheel_extension - self.wheel.rear_right) / 2.0;
                } else {
                    self.wheel.rear_left += (0.0 - self.wheel.rear_left) / 2.0;
                    self.wheel.rear_right += (0.0 - self.wheel.rear_right) / 2.0;
                }
            }
            let length = f.major_radius;
            let width = f.minor_radius;
            let pitch_height = length * sin(self.pitch + self.accel_pitch - ground_pitch);
            let roll_height = width * sin(self.roll + self.accel_roll - ground_roll);
            x.z = pitch_height.abs() / 4.0 + roll_height.abs() / 4.0;
            return; // Maintain the same orientation while we fly through the air.
        }

        // Bouncy.
        let cur_speed = f.speed;
        let max_speed = f.max_speed;
        if cur_speed > max_speed / 10.0 {
            let factor = cur_speed / max_speed;
            if self.pitch_rate.abs() < factor * p.bounce_kick / 4.0 && self.roll_rate.abs() < factor * p.bounce_kick / 8.0 {
                // `GameClientRandomValue(0, 3)`.
                match rng.int(0, 3) {
                    0 => {
                        self.pitch_rate -= p.bounce_kick * factor;
                        self.roll_rate -= p.bounce_kick * factor / 2.0;
                    }
                    1 => {
                        self.pitch_rate += p.bounce_kick * factor;
                        self.roll_rate -= p.bounce_kick * factor / 2.0;
                    }
                    2 => {
                        self.pitch_rate -= p.bounce_kick * factor;
                        self.roll_rate += p.bounce_kick * factor / 2.0;
                    }
                    _ => {
                        self.pitch_rate += p.bounce_kick * factor;
                        self.roll_rate += p.bounce_kick * factor / 2.0;
                    }
                }
            }
        }

        // Chassis suspension dynamics: damp back towards groundPitch/groundRoll.
        self.pitch_rate += (-p.pitch_stiffness * (self.pitch - ground_pitch)) + (-p.pitch_damping * self.pitch_rate);
        if self.pitch_rate > 0.0 {
            self.pitch_rate *= 0.5;
        }
        self.roll_rate += (-p.roll_stiffness * (self.roll - ground_roll)) + (-p.roll_damping * self.roll_rate);

        self.pitch += self.pitch_rate * p.uniform_axial_damping;
        self.roll += self.roll_rate * p.uniform_axial_damping;

        self.accel_pitch_rate += (-p.pitch_stiffness * self.accel_pitch) + (-p.pitch_damping * self.accel_pitch_rate);
        self.accel_pitch += self.accel_pitch_rate;
        self.accel_roll_rate += (-p.roll_stiffness * self.accel_roll) + (-p.roll_damping * self.accel_roll_rate);
        self.accel_roll += self.accel_roll_rate;

        x.pitch = self.pitch + self.accel_pitch;
        x.roll = self.roll + self.accel_roll;

        if f.motive {
            let forward_accel = f.dir[0] * f.accel[0] + f.dir[1] * f.accel[1];
            self.accel_pitch_rate += -(p.forward_accel_coef * forward_accel);
            let lateral_accel = -f.dir[1] * f.accel[0] + f.dir[0] * f.accel[1];
            self.accel_roll_rate += -(p.lateral_accel_coef * lateral_accel);
        }

        clamp_accel(p, self);
        x.z = 0.0;

        // Calculate the suspension.
        let length = f.major_radius;
        let width = f.minor_radius;
        let pitch_height = length * sin(x.pitch - ground_pitch);
        let roll_height = width * sin(x.roll - ground_roll);
        if do_wheels {
            self.suspension(f, p, pitch_height, roll_height);
        }
        // Above 22 degrees the chassis needs to rise less per unit of tilt.
        let divisor = height_divisor(x.pitch - ground_pitch);
        x.z += pitch_height.abs() / divisor;
        x.z += roll_height.abs() / divisor;
    }

    /// The wheel height offsets and steering angle, shared by `wheels` and `motorcycle`
    /// (`Drawable.cpp:2057-2166`).
    fn suspension(&mut self, f: &Frame, p: &LocoParams, pitch_height: f32, roll_height: f32) {
        self.wheel.frames_airborne = self.wheel.frames_airborne_counter;
        self.wheel.frames_airborne_counter = 0;
        let mut wheel_angle = if f.turning == -1 {
            -p.wheel_turn_angle
        } else if f.turning == 1 {
            p.wheel_turn_angle
        } else {
            0.0
        };
        if f.forward_speed_2d < 0.0 {
            // Moving backwards turns the wheels the other way.
            wheel_angle = -wheel_angle;
        }
        self.wheel.wheel_angle += (wheel_angle - self.wheel.wheel_angle) / WHEEL_SMOOTHNESS;

        let (mut fl, mut fr, mut rl, mut rr);
        if pitch_height < 0.0 {
            // Front raising up.
            fl = SPRING_FACTOR * (pitch_height / 3.0 + pitch_height / 2.0);
            fr = fl;
            rl = -pitch_height / 2.0 + pitch_height / 4.0;
            rr = rl;
        } else {
            // Back raising up.
            fl = -pitch_height / 4.0 + pitch_height / 2.0;
            fr = fl;
            rl = SPRING_FACTOR * (-pitch_height / 2.0 + -pitch_height / 3.0);
            rr = rl;
        }
        if roll_height > 0.0 {
            // Right raising up.
            fr += -SPRING_FACTOR * (roll_height / 3.0 + roll_height / 2.0);
            rr += -SPRING_FACTOR * (roll_height / 3.0 + roll_height / 2.0);
            rl += roll_height / 2.0 - roll_height / 4.0;
            fl += roll_height / 2.0 - roll_height / 4.0;
        } else {
            // Left raising up.
            fr += -roll_height / 2.0 + roll_height / 4.0;
            rr += -roll_height / 2.0 + roll_height / 4.0;
            rl += SPRING_FACTOR * (roll_height / 3.0 + roll_height / 2.0);
            fl += SPRING_FACTOR * (roll_height / 3.0 + roll_height / 2.0);
        }
        // The offsets only rise when the target rises; going down is damped by half.
        self.wheel.front_left = dampen(self.wheel.front_left, fl);
        self.wheel.front_right = dampen(self.wheel.front_right, fr);
        self.wheel.rear_left = dampen(self.wheel.rear_left, rl);
        self.wheel.rear_right = dampen(self.wheel.rear_right, rr);
        for v in [&mut self.wheel.front_left, &mut self.wheel.front_right, &mut self.wheel.rear_left, &mut self.wheel.rear_right] {
            if *v < p.max_wheel_extension {
                *v = p.max_wheel_extension;
            }
        }
    }

    /// `calcPhysicsXformMotorcycle` (`:2180-2474`): the four-wheels spring with the airborne
    /// autolevel, the roll clamp (which the engine's `&&` makes always zero, :2332-2333) and a pitch-only
    /// suspension.
    fn motorcycle(&mut self, p: &LocoParams, f: &Frame, x: &mut Xform, rng: &mut ClientRng) {
        let (perp_x, perp_y) = (-f.dir[1], f.dir[0]);
        let dot = f.normal[0] * f.dir[0] + f.normal[1] * f.dir[1];
        let ground_pitch = dot * (std::f32::consts::PI / 2.0);
        let dot = f.normal[0] * perp_x + f.normal[1] * perp_y;
        let ground_roll = dot * (std::f32::consts::PI / 2.0);
        let do_wheels = p.has_suspension;

        if f.airborne {
            if do_wheels {
                self.wheel.frames_airborne = 0;
                self.wheel.frames_airborne_counter += 1;
                if f.height_above_terrain > -p.max_wheel_extension {
                    self.wheel.rear_left += (p.max_wheel_extension - self.wheel.rear_left) / 2.0;
                    self.wheel.rear_right = self.wheel.rear_left;
                } else {
                    self.wheel.rear_left += (0.0 - self.wheel.rear_left) / 2.0;
                    self.wheel.rear_right = self.wheel.rear_left;
                }
            }
            let length = f.major_radius;
            let pitch_height = length * sin(self.pitch + self.accel_pitch - ground_pitch);
            x.z = pitch_height.abs() / 4.0;
        }

        let cur_speed = f.speed;
        let max_speed = f.max_speed;
        if !f.airborne && cur_speed > max_speed / 10.0 {
            let factor = cur_speed / max_speed;
            if self.pitch_rate.abs() < factor * p.bounce_kick / 4.0 && self.roll_rate.abs() < factor * p.bounce_kick / 8.0 {
                match rng.int(0, 3) {
                    0 => {
                        self.pitch_rate -= p.bounce_kick * factor;
                        self.roll_rate -= p.bounce_kick * factor / 2.0;
                    }
                    1 => {
                        self.pitch_rate += p.bounce_kick * factor;
                        self.roll_rate -= p.bounce_kick * factor / 2.0;
                    }
                    2 => {
                        self.pitch_rate -= p.bounce_kick * factor;
                        self.roll_rate += p.bounce_kick * factor / 2.0;
                    }
                    _ => {
                        self.pitch_rate += p.bounce_kick * factor;
                        self.roll_rate += p.bounce_kick * factor / 2.0;
                    }
                }
            }
        }

        if !f.airborne {
            self.pitch_rate += (-p.pitch_stiffness * (self.pitch - ground_pitch)) + (-p.pitch_damping * self.pitch_rate);
            self.roll_rate += (-p.roll_stiffness * (self.roll - ground_roll)) + (-p.roll_damping * self.roll_rate);
        } else {
            // Autolevel.
            self.pitch_rate += (-p.pitch_stiffness * self.pitch) + (-p.pitch_damping * self.pitch_rate);
            self.roll_rate += (-p.roll_stiffness * self.roll) + (-p.roll_damping * self.roll_rate);
        }
        self.pitch += self.pitch_rate * p.uniform_axial_damping;
        self.roll += self.roll_rate * p.uniform_axial_damping;

        self.accel_pitch_rate += (-p.pitch_stiffness * self.accel_pitch) + (-p.pitch_damping * self.accel_pitch_rate);
        self.accel_pitch += self.accel_pitch_rate;
        self.accel_roll_rate += (-p.roll_stiffness * self.accel_roll) + (-p.roll_damping * self.accel_roll_rate);
        self.accel_roll += self.accel_roll_rate;

        x.pitch = self.pitch + self.accel_pitch;
        // `unclampedRoll > 0.5 && unclampedRoll < -0.5` is never true, so a motorcycle never rolls.
        x.roll = 0.0;

        if f.motive {
            let forward_accel = f.dir[0] * f.accel[0] + f.dir[1] * f.accel[1];
            self.accel_pitch_rate += -(p.forward_accel_coef * forward_accel);
            let lateral_accel = -f.dir[1] * f.accel[0] + f.dir[0] * f.accel[1];
            self.accel_roll_rate += -(p.lateral_accel_coef * lateral_accel);
        }

        clamp_accel(p, self);
        x.z = 0.0;

        let length = f.major_radius;
        let width = f.minor_radius;
        let pitch_height = length * sin(x.pitch - ground_pitch);
        let roll_height = width * sin(x.roll - ground_roll);
        if do_wheels {
            // The motorcycle's roll offsets are commented out in the engine (`:2416-2426`).
            self.suspension(f, p, pitch_height, 0.0);
        }
        let divisor = height_divisor(x.pitch - ground_pitch);
        if !f.airborne {
            x.z += pitch_height.abs() / divisor;
            x.z += roll_height.abs() / divisor;
        }
    }
}

/// The engine's half-speed dampen for a wheel offset going down, plain assignment going up
/// (`Drawable.cpp:2115-2138`).
fn dampen(cur: f32, want: f32) -> f32 {
    if want < cur {
        cur + (want - cur) / 2.0
    } else {
        want
    }
}

/// `Drawable::calcPhysicsXformWheels`'s height divisor (`:2167-2175`).
fn height_divisor(tilt: f32) -> f32 {
    let pitch = tilt.abs();
    if pitch > std::f32::consts::PI / 8.0 {
        ((4.0 * std::f32::consts::PI / 8.0) + (pitch - std::f32::consts::PI / 8.0)) / pitch
    } else {
        4.0
    }
}

fn clamp_accel(p: &LocoParams, s: &mut LocoInfo) {
    if s.accel_pitch > p.decel_pitch_limit {
        s.accel_pitch = p.decel_pitch_limit;
    } else if s.accel_pitch < -p.accel_pitch_limit {
        s.accel_pitch = -p.accel_pitch_limit;
    }
    if s.accel_roll > p.decel_pitch_limit {
        s.accel_roll = p.decel_pitch_limit;
    } else if s.accel_roll < -p.accel_pitch_limit {
        s.accel_roll = -p.accel_pitch_limit;
    }
}

/// The `@todo` hotfix at `Drawable.cpp:1414-1426`: tiny values become exactly zero.
fn scrub(v: f32) -> f32 {
    if v > -1e-20 && v < 1e-20 {
        0.0
    } else {
        v
    }
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if m > 0.0 {
        [v[0] / m, v[1] / m, v[2] / m]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// `Coord3D::crossProduct(a, b, out)`.
fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// `Drawable::applyPhysicsXform` (`Drawable.cpp:1362-1378`) on a Bevy transform. Generals is z-up, Bevy
/// y-up (`convert::to_bevy` maps `(x, y, z)` to `(x, z, -y)`), so the engine's `Rotate_Y(pitch)` about
/// Bevy -Z, `Rotate_X(-roll)` about +X and `Rotate_Z(yaw)` about +Y, each post-multiplied in the model's
/// own frame, and `Translate(0, 0, z)` along the model's up (+Y).
pub(crate) fn apply(base: Transform, x: &Xform) -> Transform {
    let pitch = Quat::from_axis_angle(Vec3::NEG_Z, x.pitch);
    let roll = Quat::from_rotation_x(-x.roll);
    let yaw = Quat::from_rotation_y(x.yaw);
    Transform {
        translation: base.translation + base.rotation * Vec3::new(0.0, x.z, 0.0),
        rotation: base.rotation * pitch * roll * yaw,
        scale: base.scale,
    }
}

/// `BaseHeightMapRenderObjClass::getHeightMapHeight`'s smoothed terrain normal
/// (`Core/GameEngineDevice/Source/W3DDevice/GameClient/BaseHeightMap.cpp:869-1017`). Outside the
/// heightmap it is straight up, as there.
pub(crate) fn height_normal(hm: &HeightMap, x: f32, y: f32) -> [f32; 3] {
    const UP: [f32; 3] = [0.0, 0.0, 1.0];
    let inv = 1.0 / MAP_XY_FACTOR;
    let xdiv = x * inv;
    let ydiv = y * inv;
    let ixf = xdiv.floor();
    let iyf = ydiv.floor();
    let (fx, fy) = (xdiv - ixf, ydiv - iyf);
    let ix = ixf as i32 + hm.border_size;
    let iy = iyf as i32 + hm.border_size;
    let xe = hm.width;
    // The smoothed normal reads one more row/column, so the margin is extent-3.
    if ix > hm.width - 3 || iy > hm.height - 3 || iy < 1 || ix < 1 {
        return UP;
    }
    let at = |x: i32, y: i32| -> u8 { hm.data[(x + y * xe) as usize] };
    let (d0, d1, d2, d3) = (at(ix, iy), at(ix + 1, iy), at(ix + 1, iy + 1), at(ix, iy + 1));
    let (d4, d5, d6, d7) = (at(ix, iy - 1), at(ix + 1, iy - 1), at(ix + 2, iy), at(ix + 2, iy + 1));
    let (d8, d9, d11) = (at(ix + 1, iy + 2), at(ix, iy + 2), at(ix - 1, iy));
    let f = |a: u8, b: u8| f32::from(a) - f32::from(b);

    let dz_x0 = f(d1, d11);
    let dz_x1 = f(d6, d0);
    let dz_x2 = f(d7, d3);
    let dz_x3 = f(d6, d0);
    let dz_y0 = f(d3, d4);
    let dz_y1 = f(d2, d5);
    let dz_y2 = f(d8, d1);
    let dz_y3 = f(d9, d0);

    let dz_x_left = dz_x0 * (1.0 - fx) + fx * dz_x3;
    let dz_x_right = dz_x1 * (1.0 - fx) + fx * dz_x2;
    let dz_x = dz_x_left * (1.0 - fy) + fy * dz_x_right;
    let dz_y_left = dz_y0 * (1.0 - fx) + fx * dz_y3;
    let dz_y_right = dz_y1 * (1.0 - fx) + fx * dz_y2;
    let dz_y = dz_y_left * (1.0 - fy) + fy * dz_y_right;

    let l2r = [2.0 * MAP_XY_FACTOR / MAP_HEIGHT_SCALE, 0.0, dz_x];
    let n2f = [0.0, 2.0 * MAP_XY_FACTOR / MAP_HEIGHT_SCALE, dz_y];
    normalize3(cross3(l2r, n2f))
}

/// `GeometryInfo::getMaxHeightAbovePosition` (`Geometry.cpp:210`).
fn max_height_above_position(g: &Geometry) -> f32 {
    match g.shape {
        GeometryShape::Sphere => g.major_radius,
        GeometryShape::Box | GeometryShape::Cylinder => g.height,
    }
}

/// `isSignificantlyAboveTerrain` (`Thing.cpp:338`): `9 * -gravity` above the ground.
const SIGNIFICANT_FALL: f32 = 9.0;

/// Per-object client state: the chassis spring state and the last computed transform.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct VehiclePhysics {
    pub info: LocoInfo,
    pub xform: Xform,
    acc: f32,
}

/// `m_wheelInfo` as the tire bones see it (`Drawable::getWheelInfo`).
pub(crate) fn wheel_info(p: Option<&VehiclePhysics>) -> Option<WheelInfo> {
    p.map(|p| p.info.wheel)
}

/// Client RNG for the suspension bounce and the crush-over vibration. The engine has one global client
/// RNG (`GameClientRandomValue`); the viewer keeps one per client subsystem, so this never touches the
/// simulation's RNG or the effects'.
#[derive(Resource)]
struct PhysicsRng(ClientRng);

impl Default for PhysicsRng {
    fn default() -> Self {
        PhysicsRng(ClientRng::new(0x5eed_0080))
    }
}

/// Client params by template and locomotor set, so a unit that switches sets gets the right appearance
/// and stiffnesses without a lookup per frame.
#[derive(Resource, Default)]
struct ParamsCache {
    map: HashMap<String, HashMap<LocomotorSet, Option<Arc<LocoParams>>>>,
}

impl ParamsCache {
    fn get(&mut self, templates: &Templates, template: &str, set: LocomotorSet) -> Option<Arc<LocoParams>> {
        if let Some(sets) = self.map.get(template) {
            if let Some(p) = sets.get(&set) {
                return p.clone();
            }
        }
        let p = locomotor_template(templates, template, set).map(|t| Arc::new(LocoParams::from_template(t)));
        self.map.entry(template.to_owned()).or_default().insert(set, p.clone());
        p
    }
}

pub fn add_systems(app: &mut App) {
    app.init_resource::<PhysicsRng>();
    app.init_resource::<ParamsCache>();
    app.add_systems(
        Update,
        update_physics_xform
            .after(crate::sim_view::sim_sync)
            .before(crate::vehicle_draw::update_vehicles)
            .before(crate::anim::animate_rigs)
            .run_if(resource_exists::<crate::sim_view::SimActive>),
    );
}

/// Builds the engine's [`Frame`] from the object's read-only sim state.
fn frame_of(sim: &SimRes, templates: &Templates, map: Option<&crate::MapRes>, o: &zr_sim::Object, p: &Physics, dir_angle: f32, pos: [f32; 3]) -> Frame {
    let logic = sim.game.logic();
    let ground = logic.terrain().surface_height(pos[0], pos[1], pos[2]);
    let normal = map
        .and_then(|m| m.0.height_map.as_ref())
        .map_or([0.0, 0.0, 1.0], |hm| height_normal(hm, pos[0], pos[1]));
    let overlap = p.current_overlap.and_then(|id| overlap_of(templates, logic, o, pos, id));
    Frame {
        dir: [dir_angle.cos(), dir_angle.sin()],
        velocity: [p.velocity.x, p.velocity.y, p.velocity.z],
        accel: [p.accel.x, p.accel.y, p.accel.z],
        speed: p.velocity_magnitude(),
        forward_speed_2d: p.forward_speed_2d(dir_angle),
        turning: p.turning,
        motive: p.is_motive(sim.game.frame()),
        max_speed: o.locomotor().map_or(0.0, |l| l.max_speed),
        airborne: pos[2] - ground > SIGNIFICANT_FALL * -logic.rules().gravity,
        height_above_terrain: pos[2] - ground,
        major_radius: o.template.geometry.major_radius,
        minor_radius: o.template.geometry.minor_radius,
        normal,
        has_ai: o.template.ai_update.is_some(),
        overlap,
        previous_overlap: p.previous_overlap.is_some(),
    }
}

/// The overlap test's inputs (`Drawable.cpp:1682-1704`), or `None` when the overlapped object is a bush
/// (`:1678-1680`).
fn overlap_of(templates: &Templates, logic: &zr_sim::GameLogic, o: &zr_sim::Object, pos: [f32; 3], id: ObjectId) -> Option<Overlap> {
    let other = logic.find_object(id)?;
    let thing = templates.thing(&other.template.name);
    if thing.is_some_and(|t| t.is_kind_of(KindOf::Shrubbery)) {
        return None;
    }
    let dx = other.position.x - pos[0];
    let dy = other.position.y - pos[1];
    let center_dist_sqr = dx * dx + dy * dy;
    let our_size = o.template.geometry.bounding_circle_radius();
    let other_size = other.template.geometry.bounding_circle_radius();
    let max_center_dist = (other_size + our_size) * OVERLAP_SHRINK_FACTOR;
    let flat_kind = thing.is_some_and(|t| t.is_kind_of(KindOf::LowOverlappable) || t.is_kind_of(KindOf::Infantry));
    let crushed = other.body.as_ref().is_some_and(|b| b.front_crushed() && b.back_crushed());
    Some(Overlap {
        dx,
        dy,
        center_dist_sqr,
        max_center_dist,
        height: max_height_above_position(&other.template.geometry),
        flat: flat_kind || crushed,
    })
}

/// Ticks each physics-transformed object at the logic rate and writes its model transform: the base
/// `sim_sync` produced plus `Drawable::applyPhysicsXform`.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_physics_xform(
    time: Res<Time>,
    sim: NonSend<SimRes>,
    map: Option<Res<crate::MapRes>>,
    mut params: ResMut<ParamsCache>,
    mut rng: ResMut<PhysicsRng>,
    mut commands: Commands,
    mut objs: Query<(Entity, &SimObject, &mut Transform, Option<&mut VehiclePhysics>, Option<&GhostSnapshot>)>,
) {
    let dt = time.delta_secs();
    let templates = sim.game.data().templates();
    for (e, so, mut t, state, ghost) in &mut objs {
        if ghost.is_some() {
            continue;
        }
        let Some(o) = sim.game.logic().find_object(so.0) else { continue };
        let Some(phys) = o.physics.as_ref() else { continue };
        let Some(p) = params.get(templates, &o.template.name, o.locomotor_set) else { continue };
        if !is_physics_appearance(p.appearance) {
            continue;
        }
        // `Drawable::draw` applies the physics transform only while the object is not
        // `DISABLED_HELD` (`:2652`); a held unit keeps the transform it last had.
        if o.is_disabled_by_type(zr_sim::disabled::DisabledType::Held) {
            continue;
        }
        let Some((pos, angle)) = crate::sim_view::interpolated(&sim, so.0, (sim.acc / STEP).clamp(0.0, 1.0)) else { continue };
        let frame = frame_of(&sim, templates, map.as_deref(), o, phys, angle, pos);
        // The drawable's own transform, before the physics xform: the interpolated position and
        // heading. (The construction lift is a draw-module offset, not part of `transformMtx`, and no
        // vehicle is ever under construction.)
        let base = crate::convert::object_transform(pos, angle);
        match state {
            Some(mut state) => {
                let s = &mut *state;
                s.acc += dt;
                // The engine recalcs on its 30 Hz sync frame only (`:1366-1371`).
                while s.acc >= STEP {
                    s.acc -= STEP;
                    s.info.update(&p, &frame, &mut s.xform, &mut rng.0);
                }
                *t = apply(base, &s.xform);
                if std::env::var_os("ZR_LOG_OBJECTS").is_some() && sim.game.frame() % 30 == 0 {
                    info!(
                        "physics xform {} {}: pitch {:.3} roll {:.3} yaw {:.3} z {:.3} (normal {:.3},{:.3})",
                        so.0.0, o.template.name, s.xform.pitch, s.xform.roll, s.xform.yaw, s.xform.z, frame.normal[0], frame.normal[1]
                    );
                }
            }
            None => {
                // First sight: the bind-time calc, then the same transform.
                let mut state = VehiclePhysics { info: LocoInfo::default(), xform: Xform::default(), acc: 0.0 };
                state.info.update(&p, &frame, &mut state.xform, &mut rng.0);
                *t = apply(base, &state.xform);
                commands.entity(e).insert(state);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A neutral locomotor: no springs, no bounce, no suspension.
    fn params(appearance: LocomotorAppearance) -> LocoParams {
        LocoParams {
            appearance,
            accel_pitch_limit: 0.2,
            decel_pitch_limit: 0.2,
            bounce_kick: 0.0,
            pitch_stiffness: 0.0,
            roll_stiffness: 0.0,
            pitch_damping: 0.0,
            roll_damping: 0.0,
            forward_vel_coef: 0.0,
            lateral_vel_coef: 0.0,
            pitch_by_z_vel_coef: 0.0,
            forward_accel_coef: 0.0,
            lateral_accel_coef: 0.0,
            uniform_axial_damping: 1.0,
            max_wheel_extension: -2.3,
            wheel_turn_angle: std::f32::consts::PI / 8.0,
            has_suspension: true,
            thrust_roll: 0.0,
            wobble_rate: 0.0,
            min_wobble: 0.0,
            max_wobble: 0.0,
            rudder_correction_degree: 0.0,
            rudder_correction_rate: 0.0,
            elevator_correction_degree: 0.0,
            elevator_correction_rate: 0.0,
        }
    }

    fn frame(dir: [f32; 2], normal: [f32; 3]) -> Frame {
        Frame {
            dir,
            velocity: [0.0; 3],
            accel: [0.0; 3],
            speed: 0.0,
            forward_speed_2d: 0.0,
            turning: 0,
            motive: false,
            max_speed: 1.0,
            airborne: false,
            height_above_terrain: 0.0,
            major_radius: 2.0,
            minor_radius: 1.0,
            normal,
            has_ai: true,
            overlap: None,
            previous_overlap: false,
        }
    }

    fn rng() -> ClientRng {
        ClientRng::new(7)
    }

    /// A flat heightmap with `border` cells of padding, `rise` bytes per cell towards +X.
    fn slope_map(rise: u8) -> HeightMap {
        let (w, h) = (32, 32);
        let mut data = vec![0u8; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let v = (x as u32 * u32::from(rise)).min(255) as u8;
                data[(x + y * w) as usize] = v;
            }
        }
        HeightMap { version: 4, width: w, height: h, border_size: 2, boundaries: vec![(w - 4, h - 4)], data }
    }

    #[test]
    fn the_terrain_normal_tilts_away_from_the_uphill_side() {
        // Rising towards +X: the normal leans towards -X, straight up on flat ground.
        let n = height_normal(&slope_map(1), 50.0, 50.0);
        assert!(n[0] < 0.0, "normal leans -X on a +X slope: {n:?}");
        assert!(n[1].abs() < 1e-4, "no Y tilt on an X slope: {n:?}");
        assert!(n[2] > 0.99, "{n:?}");
        // One byte per 10-unit cell is 0.625 / 10 of slope.
        let expect = -0.625 / (10.0f32 * 10.0 + 0.625 * 0.625).sqrt();
        assert!((n[0] - expect).abs() < 0.002, "{n:?} vs {expect}");
        assert_eq!(height_normal(&slope_map(0), 50.0, 50.0), [0.0, 0.0, 1.0]);
        // Outside the map (its outer ring) the normal points up.
        assert_eq!(height_normal(&slope_map(1), -50.0, 50.0), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn treads_pitch_up_the_slope_and_nose_down_off_it() {
        let mut p = params(LocomotorAppearance::Treads);
        p.pitch_stiffness = 0.2;
        p.pitch_damping = 0.8;
        let mut info = LocoInfo::default();
        let mut x = Xform::default();
        let uphill = frame([1.0, 0.0], height_normal(&slope_map(4), 50.0, 50.0));
        for _ in 0..400 {
            info.update(&p, &uphill, &mut x, &mut rng());
        }
        let ground_pitch = uphill.normal[0] * (std::f32::consts::PI / 2.0);
        assert!(ground_pitch < 0.0);
        assert!((x.pitch - ground_pitch).abs() < 0.01, "pitch {} vs ground {ground_pitch}", x.pitch);
        // The model's forward axis tips up (Bevy +Y) on the uphill.
        let fwd = apply(Transform::IDENTITY, &x).rotation * Vec3::X;
        assert!(fwd.y > 0.0, "nose up on a +X uphill: {fwd:?}");
        // Driving the other way (downhill) gives the opposite tilt.
        let downhill = frame([-1.0, 0.0], uphill.normal);
        let mut info2 = LocoInfo::default();
        let mut x2 = Xform::default();
        for _ in 0..400 {
            info2.update(&p, &downhill, &mut x2, &mut rng());
        }
        assert!(x2.pitch > 0.0, "pitch {} downhill", x2.pitch);
        assert!((x2.pitch - -ground_pitch).abs() < 0.01);
    }

    #[test]
    fn the_spring_damper_integrates_the_engine_step() {
        let mut p = params(LocomotorAppearance::Treads);
        p.pitch_stiffness = 0.5;
        p.pitch_damping = 0.5;
        p.uniform_axial_damping = 0.5;
        let mut info = LocoInfo { pitch: 0.1, ..LocoInfo::default() };
        let mut x = Xform::default();
        info.treads(&p, &frame([1.0, 0.0], [0.0, 0.0, 1.0]), &mut x, &mut rng());
        // rate += -k*pitch - c*rate = -0.05; pitch += rate * uad = -0.025.
        assert!((info.pitch_rate - -0.05).abs() < 1e-6, "{}", info.pitch_rate);
        assert!((info.pitch - 0.075).abs() < 1e-6, "{}", info.pitch);
        assert!((x.pitch - 0.075).abs() < 1e-6);
        // A positive rate is halved before it is integrated (`:1780-1781`).
        info.pitch = 0.0;
        info.pitch_rate = 0.2;
        info.update(&p, &frame([1.0, 0.0], [0.0, 0.0, 1.0]), &mut x, &mut rng());
        // rate = 0.2 - 0.5*0.2 = 0.1, halved to 0.05; pitch += 0.05 * 0.5.
        assert!((info.pitch_rate - 0.05).abs() < 1e-6, "{}", info.pitch_rate);
        assert!((info.pitch - 0.025).abs() < 1e-6, "{}", info.pitch);
    }

    #[test]
    fn wheels_steer_towards_the_turn_and_reverse_when_backing_up() {
        let p = params(LocomotorAppearance::FourWheels);
        let mut info = LocoInfo::default();
        let mut x = Xform::default();
        let mut f = frame([1.0, 0.0], [0.0, 0.0, 1.0]);
        f.turning = 1;
        info.update(&p, &f, &mut x, &mut rng());
        // `m_wheelAngle += (WHEEL_ANGLE - m_wheelAngle) / WHEEL_SMOOTHNESS`.
        let first = p.wheel_turn_angle / WHEEL_SMOOTHNESS;
        assert!((info.wheel.wheel_angle - first).abs() < 1e-6, "{}", info.wheel.wheel_angle);
        for _ in 0..100 {
            info.update(&p, &f, &mut x, &mut rng());
        }
        assert!((info.wheel.wheel_angle - p.wheel_turn_angle).abs() < 1e-4);
        // Turning the other way converges on the negative angle.
        f.turning = -1;
        for _ in 0..100 {
            info.update(&p, &f, &mut x, &mut rng());
        }
        assert!((info.wheel.wheel_angle + p.wheel_turn_angle).abs() < 1e-4);
        // Reversing while turning flips the steering.
        f.turning = 1;
        f.forward_speed_2d = -1.0;
        for _ in 0..100 {
            info.update(&p, &f, &mut x, &mut rng());
        }
        assert!((info.wheel.wheel_angle + p.wheel_turn_angle).abs() < 1e-4);
        // A unit not turning holds the wheels straight.
        f.turning = 0;
        f.forward_speed_2d = 0.0;
        for _ in 0..100 {
            info.update(&p, &f, &mut x, &mut rng());
        }
        assert!(info.wheel.wheel_angle.abs() < 1e-4);
    }

    #[test]
    fn the_suspension_lowers_the_front_wheels_of_a_nose_up_chassis() {
        let p = params(LocomotorAppearance::FourWheels);
        let mut info = LocoInfo::default();
        let f = Frame { major_radius: 2.0, ..frame([1.0, 0.0], [0.0, 0.0, 1.0]) };
        // Nose up: `pitchHeight = length * sin(totalPitch - groundPitch)` is negative. The front wheels
        // are pushed down (a negative bone offset) and the rear ones up, which lifts the body's nose.
        let pitch_height = 2.0 * (-0.2f32).sin();
        info.suspension(&f, &p, pitch_height, 0.0);
        assert!(info.wheel.front_left < 0.0, "front wheels drop: {:?}", info.wheel);
        assert!(info.wheel.rear_left > 0.0, "rear wheels rise: {:?}", info.wheel);
        // The first frame dampens a downward move by half (`:2115-2120`).
        let want = SPRING_FACTOR * (pitch_height / 3.0 + pitch_height / 2.0);
        assert!((info.wheel.front_left - want / 2.0).abs() < 1e-6);
        // A front offset is clamped at `MaximumWheelExtension` and its downward travel is damped by half
        // each frame (`:2115-2120`, `:2140-2142`).
        let down = 2.0 * (-2.0f32).sin();
        let want = SPRING_FACTOR * (down / 3.0 + down / 2.0);
        let before = info.wheel.front_left;
        info.suspension(&f, &p, down, 0.0);
        assert!((info.wheel.front_left - (before + (want - before) / 2.0)).abs() < 1e-6);
        info.wheel.front_left = 0.0;
        info.suspension(&f, &p, -100.0, 0.0);
        assert_eq!(info.wheel.front_left, p.max_wheel_extension);
    }

    #[test]
    fn an_airborne_wheels_frame_keeps_the_previous_tilt() {
        let mut p = params(LocomotorAppearance::FourWheels);
        p.max_wheel_extension = -2.3;
        let mut info = LocoInfo { pitch: 0.3, roll: -0.1, ..LocoInfo::default() };
        let mut x = Xform::default();
        let mut f = frame([1.0, 0.0], [0.0, 0.0, 1.0]);
        f.airborne = true;
        f.height_above_terrain = 5.0;
        info.update(&p, &f, &mut x, &mut rng());
        // The airborne branch returns before the springs write `m_totalPitch`/`m_totalRoll`, so the
        // persistent info keeps the previous frame's tilt.
        assert!(x.pitch.abs() < 1e-6 && x.roll.abs() < 1e-6, "left as the last written tilt");
        // It still raises the chassis by the suspension travel (the wheels hanging down).
        assert!(x.z > 0.0, "the chassis rises by the suspension travel: {}", x.z);
        assert!((x.z - (2.0 * 0.3f32.sin()).abs() / 4.0 - (1.0 * (-0.1f32).sin()).abs() / 4.0).abs() < 1e-6);
        // The wheels hang down: they extend while airborne.
        assert!(info.wheel.rear_left < 0.0, "{:?}", info.wheel.rear_left);
        assert_eq!(info.wheel.frames_airborne_counter, 1);
    }

    #[test]
    fn thrust_wobbles_pitch_and_yaw_between_the_limits_and_rolls_steadily() {
        let mut p = params(LocomotorAppearance::Thrust);
        p.wobble_rate = 0.05;
        p.min_wobble = -0.2;
        p.max_wobble = 0.2;
        p.thrust_roll = 0.01;
        let mut info = LocoInfo::default();
        let mut x = Xform::default();
        let f = frame([1.0, 0.0], [0.0, 0.0, 1.0]);
        info.update(&p, &f, &mut x, &mut rng());
        assert!((x.pitch - 0.05).abs() < 1e-6 && (x.yaw - 0.05).abs() < 1e-6);
        assert!((x.roll - 0.01).abs() < 1e-6);
        for _ in 0..40 {
            info.update(&p, &f, &mut x, &mut rng());
        }
        // It turns around at the top and comes back inside the limits.
        assert!(info.pitch <= p.max_wobble && info.pitch >= p.min_wobble, "{}", info.pitch);
        assert!(info.yaw.abs() <= p.max_wobble + 1e-6);
        assert!((x.roll - 0.41).abs() < 1e-5, "roll keeps accumulating: {}", x.roll);
    }

    #[test]
    fn hover_correction_wobbles_the_yaw_and_pitch() {
        let mut p = params(LocomotorAppearance::Hover);
        p.rudder_correction_degree = 0.1;
        p.rudder_correction_rate = std::f32::consts::FRAC_PI_2;
        p.elevator_correction_degree = 0.2;
        p.elevator_correction_rate = std::f32::consts::FRAC_PI_2;
        let mut info = LocoInfo::default();
        let mut x = Xform::default();
        info.update(&p, &frame([1.0, 0.0], [0.0, 0.0, 1.0]), &mut x, &mut rng());
        assert!((x.yaw - 0.1).abs() < 1e-5, "sin(pi/2) = 1: {}", x.yaw);
        assert!(x.pitch.abs() < 1e-5, "cos(pi/2) = 0: {}", x.pitch);
        info.update(&p, &frame([1.0, 0.0], [0.0, 0.0, 1.0]), &mut x, &mut rng());
        assert!(x.yaw.abs() < 1e-6, "sin(pi) = 0: {}", x.yaw);
        assert!((x.pitch + 0.2).abs() < 1e-5, "cos(pi) = -1: {}", x.pitch);
    }

    #[test]
    fn the_plane_of_the_transform_follows_the_engine_order() {
        // A pure pitch: the model's forward axis tips up for a negative pitch and its up axis tips
        // forward by the same amount.
        let t = apply(Transform::IDENTITY, &Xform { pitch: -0.2, roll: 0.0, yaw: 0.0, z: 1.5 });
        assert!((t.translation.y - 1.5).abs() < 1e-6, "the z offset goes up the model's +Y");
        let fwd = t.rotation * Vec3::X;
        assert!((fwd.y - 0.2f32.sin()).abs() < 1e-5, "{fwd:?}");
        // A yaw about the model's up axis turns the forward axis in the horizontal plane.
        let t = apply(Transform::IDENTITY, &Xform { pitch: 0.0, roll: 0.0, yaw: std::f32::consts::FRAC_PI_2, z: 0.0 });
        let fwd = t.rotation * Vec3::X;
        assert!(fwd.x.abs() < 1e-6 && (fwd.z + 1.0).abs() < 1e-6, "{fwd:?}");
    }
}
