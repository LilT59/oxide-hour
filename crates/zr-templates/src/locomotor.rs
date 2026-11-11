//! `LocomotorTemplate`. Ported from `GameLogic/Object/Locomotor.cpp`
//! (`LocomotorTemplate::getFieldParse`, constructor defaults, `validate`,
//! `LocomotorStore::parseLocomotorTemplateDefinition`).
//!
//! Speeds are distance per logic frame, accelerations distance per frame^2, turn rates radians per
//! frame and angles radians, as the engine stores them after `INI::parseVelocityReal` and friends.

use zr_ini::{Block, Item};

use crate::diag::{Diag, WarningKind};
use crate::flags::Mask;
use crate::names::*;
use crate::value::{friction_per_sec_to_per_frame, Args};

/// `BIGNUM` in `Locomotor.cpp`.
const BIGNUM: f32 = 99999.0;

#[derive(Debug, Clone, PartialEq)]
pub struct LocomotorTemplate {
    pub name: String,
    pub surfaces: Mask<LocomotorSurface>,
    /// `Speed`, units per frame.
    pub max_speed: f32,
    /// `SpeedDamaged`; after validation equals `max_speed` if it was omitted.
    pub max_speed_damaged: f32,
    /// `TurnRate`, radians per frame.
    pub max_turn_rate: f32,
    pub max_turn_rate_damaged: f32,
    /// `Acceleration`, units per frame^2.
    pub acceleration: f32,
    pub acceleration_damaged: f32,
    pub lift: f32,
    pub lift_damaged: f32,
    pub braking: f32,
    pub min_speed: f32,
    pub min_turn_speed: f32,
    pub preferred_height: f32,
    pub preferred_height_damping: f32,
    pub circling_radius: f32,
    /// `Extra2DFriction`, per frame.
    pub extra_2d_friction: f32,
    pub speed_limit_z: f32,
    /// `MaxThrustAngle`, radians.
    pub max_thrust_angle: f32,
    pub behavior_z: LocomotorBehaviorZ,
    pub appearance: LocomotorAppearance,
    pub move_priority: LocomotorPriority,
    pub accel_pitch_limit: f32,
    pub decel_pitch_limit: f32,
    pub bounce_kick: f32,
    pub pitch_stiffness: f32,
    pub roll_stiffness: f32,
    pub pitch_damping: f32,
    pub roll_damping: f32,
    pub thrust_roll: f32,
    pub wobble_rate: f32,
    pub min_wobble: f32,
    pub max_wobble: f32,
    pub pitch_by_z_vel_coef: f32,
    pub forward_vel_coef: f32,
    pub lateral_vel_coef: f32,
    pub forward_accel_coef: f32,
    pub lateral_accel_coef: f32,
    pub uniform_axial_damping: f32,
    pub turn_pivot_offset: f32,
    pub apply_2d_friction_when_airborne: bool,
    pub downhill_only: bool,
    pub allow_motive_force_while_airborne: bool,
    pub locomotor_works_when_dead: bool,
    pub airborne_targeting_height: i32,
    pub stick_to_ground: bool,
    pub can_move_backward: bool,
    pub has_suspension: bool,
    pub wheel_turn_angle: f32,
    pub maximum_wheel_extension: f32,
    pub maximum_wheel_compression: f32,
    pub close_enough_dist: f32,
    pub is_close_enough_dist_3d: bool,
    /// `SlideIntoPlaceTime`: milliseconds converted to (fractional) frames.
    pub ultra_accurate_slide_into_place_factor: f32,
    pub wander_width_factor: f32,
    pub wander_length_factor: f32,
    pub wander_about_point_radius: f32,
    pub rudder_correction_degree: f32,
    pub rudder_correction_rate: f32,
    pub elevator_correction_degree: f32,
    pub elevator_correction_rate: f32,
}

impl LocomotorTemplate {
    /// `LocomotorTemplate::LocomotorTemplate()`.
    pub fn new(name: &str) -> Self {
        LocomotorTemplate {
            name: name.to_string(),
            // -1 means "same as undamaged if not specified" until `validate`.
            max_speed_damaged: -1.0,
            max_turn_rate_damaged: -1.0,
            acceleration_damaged: -1.0,
            lift_damaged: -1.0,
            surfaces: Mask::EMPTY,
            max_speed: 0.0,
            max_turn_rate: 0.0,
            acceleration: 0.0,
            lift: 0.0,
            braking: BIGNUM,
            min_speed: 0.0,
            min_turn_speed: BIGNUM,
            behavior_z: LocomotorBehaviorZ::NoZMotiveForce,
            appearance: LocomotorAppearance::Other,
            move_priority: LocomotorPriority::MovesMiddle,
            preferred_height: 0.0,
            preferred_height_damping: 1.0,
            circling_radius: 0.0,
            max_thrust_angle: 0.0,
            speed_limit_z: 999999.0,
            extra_2d_friction: 0.0,
            accel_pitch_limit: 0.0,
            decel_pitch_limit: 0.0,
            bounce_kick: 0.0,
            pitch_stiffness: 0.1,
            roll_stiffness: 0.1,
            pitch_damping: 0.9,
            roll_damping: 0.9,
            forward_vel_coef: 0.0,
            pitch_by_z_vel_coef: 0.0,
            thrust_roll: 0.0,
            wobble_rate: 0.0,
            min_wobble: 0.0,
            max_wobble: 0.0,
            lateral_vel_coef: 0.0,
            forward_accel_coef: 0.0,
            lateral_accel_coef: 0.0,
            uniform_axial_damping: 1.0,
            turn_pivot_offset: 0.0,
            apply_2d_friction_when_airborne: false,
            downhill_only: false,
            allow_motive_force_while_airborne: false,
            locomotor_works_when_dead: false,
            airborne_targeting_height: i32::MAX,
            stick_to_ground: false,
            can_move_backward: false,
            has_suspension: false,
            wheel_turn_angle: 0.0,
            maximum_wheel_extension: 0.0,
            maximum_wheel_compression: 0.0,
            close_enough_dist: 1.0,
            is_close_enough_dist_3d: false,
            ultra_accurate_slide_into_place_factor: 0.0,
            wander_width_factor: 0.0,
            wander_length_factor: 1.0,
            wander_about_point_radius: 0.0,
            rudder_correction_degree: 0.0,
            rudder_correction_rate: 0.0,
            elevator_correction_degree: 0.0,
            elevator_correction_rate: 0.0,
        }
    }

    /// Parses one definition on top of `self` (a repeated `Locomotor` block overlays the earlier one,
    /// `LocomotorStore::parseLocomotorTemplateDefinition`), then runs `validate`.
    pub(crate) fn apply_block(&mut self, b: &Block, d: &mut Diag) {
        d.push_scope(format!("Locomotor {}", self.name));
        for item in &b.items {
            let Item::Field(f) = item else {
                d.warn(WarningKind::Structure, "", 0, "unexpected nested block");
                continue;
            };
            let mut a = Args::new(&f.value);
            match self.field(&f.key, &mut a) {
                Ok(true) => {}
                Ok(false) => d.unknown_field(f),
                Err(e) => d.bad_value(f, e),
            }
        }
        if let Err(msg) = self.validate() {
            d.warn(WarningKind::Structure, "", b.line, msg);
        }
        d.pop_scope();
    }

    fn field(&mut self, key: &str, a: &mut Args) -> Result<bool, String> {
        match key {
            "Surfaces" => self.surfaces = self.surfaces.parse(a.rest())?,
            "Speed" => self.max_speed = a.velocity()?,
            "SpeedDamaged" => self.max_speed_damaged = a.velocity()?,
            "TurnRate" => self.max_turn_rate = a.angular_velocity()?,
            "TurnRateDamaged" => self.max_turn_rate_damaged = a.angular_velocity()?,
            "Acceleration" => self.acceleration = a.acceleration()?,
            "AccelerationDamaged" => self.acceleration_damaged = a.acceleration()?,
            "Lift" => self.lift = a.acceleration()?,
            "LiftDamaged" => self.lift_damaged = a.acceleration()?,
            "Braking" => self.braking = a.acceleration()?,
            "MinSpeed" => self.min_speed = a.velocity()?,
            "MinTurnSpeed" => self.min_turn_speed = a.velocity()?,
            "PreferredHeight" => self.preferred_height = a.real()?,
            "PreferredHeightDamping" => self.preferred_height_damping = a.real()?,
            "CirclingRadius" => self.circling_radius = a.real()?,
            "Extra2DFriction" => self.extra_2d_friction = friction_per_sec_to_per_frame(a.real()?),
            "SpeedLimitZ" => self.speed_limit_z = a.velocity()?,
            "MaxThrustAngle" => self.max_thrust_angle = a.angle()?,
            "ZAxisBehavior" => self.behavior_z = a.index()?,
            "Appearance" => self.appearance = a.index()?,
            "GroupMovementPriority" => self.move_priority = a.index()?,
            "AccelerationPitchLimit" => self.accel_pitch_limit = a.angle()?,
            "DecelerationPitchLimit" => self.decel_pitch_limit = a.angle()?,
            "BounceAmount" => self.bounce_kick = a.angular_velocity()?,
            "PitchStiffness" => self.pitch_stiffness = a.real()?,
            "RollStiffness" => self.roll_stiffness = a.real()?,
            "PitchDamping" => self.pitch_damping = a.real()?,
            "RollDamping" => self.roll_damping = a.real()?,
            "ThrustRoll" => self.thrust_roll = a.real()?,
            "ThrustWobbleRate" => self.wobble_rate = a.real()?,
            "ThrustMinWobble" => self.min_wobble = a.real()?,
            "ThrustMaxWobble" => self.max_wobble = a.real()?,
            "PitchInDirectionOfZVelFactor" => self.pitch_by_z_vel_coef = a.real()?,
            "ForwardVelocityPitchFactor" => self.forward_vel_coef = a.real()?,
            "LateralVelocityRollFactor" => self.lateral_vel_coef = a.real()?,
            "ForwardAccelerationPitchFactor" => self.forward_accel_coef = a.real()?,
            "LateralAccelerationRollFactor" => self.lateral_accel_coef = a.real()?,
            "UniformAxialDamping" => self.uniform_axial_damping = a.real()?,
            "TurnPivotOffset" => self.turn_pivot_offset = a.real()?,
            "Apply2DFrictionWhenAirborne" => self.apply_2d_friction_when_airborne = a.boolean()?,
            "DownhillOnly" => self.downhill_only = a.boolean()?,
            "AllowAirborneMotiveForce" => self.allow_motive_force_while_airborne = a.boolean()?,
            "LocomotorWorksWhenDead" => self.locomotor_works_when_dead = a.boolean()?,
            "AirborneTargetingHeight" => self.airborne_targeting_height = a.int()?,
            "StickToGround" => self.stick_to_ground = a.boolean()?,
            "CanMoveBackwards" => self.can_move_backward = a.boolean()?,
            "HasSuspension" => self.has_suspension = a.boolean()?,
            "FrontWheelTurnAngle" => self.wheel_turn_angle = a.angle()?,
            "MaximumWheelExtension" => self.maximum_wheel_extension = a.real()?,
            "MaximumWheelCompression" => self.maximum_wheel_compression = a.real()?,
            "CloseEnoughDist" => self.close_enough_dist = a.real()?,
            "CloseEnoughDist3D" => self.is_close_enough_dist_3d = a.boolean()?,
            "SlideIntoPlaceTime" => self.ultra_accurate_slide_into_place_factor = a.duration_real()?,
            "WanderWidthFactor" => self.wander_width_factor = a.real()?,
            "WanderLengthFactor" => self.wander_length_factor = a.real()?,
            "WanderAboutPointRadius" => self.wander_about_point_radius = a.real()?,
            "RudderCorrectionDegree" => self.rudder_correction_degree = a.real()?,
            "RudderCorrectionRate" => self.rudder_correction_rate = a.real()?,
            "ElevatorCorrectionDegree" => self.elevator_correction_degree = a.real()?,
            "ElevatorCorrectionRate" => self.elevator_correction_rate = a.real()?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// `LocomotorTemplate::validate` (Zero Hour build: no `m_decelPitchLimit` defaulting).
    pub fn validate(&mut self) -> Result<(), String> {
        // omitted "damaged" values default to the undamaged ones
        if self.max_speed_damaged < 0.0 {
            self.max_speed_damaged = self.max_speed;
        }
        if self.max_turn_rate_damaged < 0.0 {
            self.max_turn_rate_damaged = self.max_turn_rate;
        }
        if self.acceleration_damaged < 0.0 {
            self.acceleration_damaged = self.acceleration;
        }
        if self.lift_damaged < 0.0 {
            self.lift_damaged = self.lift;
        }

        if self.appearance == LocomotorAppearance::Wings {
            if self.min_speed <= 0.0 {
                self.min_speed = 0.01;
            }
            if self.min_turn_speed <= 0.0 {
                self.min_turn_speed = 0.01;
            }
        }

        if self.appearance == LocomotorAppearance::Thrust {
            if self.behavior_z != LocomotorBehaviorZ::NoZMotiveForce || self.lift != 0.0 || self.lift_damaged != 0.0 {
                // the engine throws INI_INVALID_DATA here
                return Err("THRUST locomotors may not use ZAxisBehavior or lift".into());
            }
            // zero speeds are quietly healed
            if self.max_speed <= 0.0 {
                self.max_speed = 0.01;
            }
            if self.max_speed_damaged <= 0.0 {
                self.max_speed_damaged = 0.01;
            }
            if self.min_speed <= 0.0 {
                self.min_speed = 0.01;
            }
        }
        Ok(())
    }
}
