//! Projectile behaviors: `DumbProjectileBehavior`'s cubic-Bezier ballistic arc and
//! `MissileAIUpdate`'s homing state machine.
//!
//! Ported from `GameLogic/Object/Behavior/DumbProjectileBehavior.cpp` (`calcFlightPath`,
//! `projectileFireAtObjectOrPosition`, `update`, `projectileHandleCollision`) and
//! `GameLogic/Object/Update/AIUpdate/MissileAIUpdate.cpp` (state machine), with the Bezier maths
//! from `Core/Common/Bezier/BezierSegment.cpp` / `BezFwdIterator.cpp`.
//!
//! `zr-sim` keeps projectiles as lightweight entities rather than full `Object`s (the module's
//! `updatePos`/collision seam is the partition), but they carry the template's real behavior:
//! `Dumb` samples the arc and detonates on the first terrain/object it meets; `Missile` runs the
//! launch → ignition → attack → kill states, homes on the victim's *current* position and blows up
//! when it runs out of fuel (`DetonateOnNoFuel`).

use crate::math::{ceil, sqrt, Coord3D};
use crate::superweapon::NeutronMissileTemplate;

/// `DumbProjectileBehaviorModuleData` (sim copy; the adapter converts `zr-templates`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DumbProjectileParams {
    pub first_height: f32,
    pub second_height: f32,
    pub first_percent_indent: f32,
    pub second_percent_indent: f32,
    pub max_lifespan: u32,
    pub orient_to_flight_path: bool,
    pub flight_path_adjust_dist_per_frame: f32,
    /// The shell has a `PhysicsBehavior` (every shipped `DumbProjectileBehavior` shell does). It takes the
    /// launcher's velocity (`Weapon::positionProjectileForLaunch` -> `transferVelocityTo`, Weapon.cpp:3145,
    /// called from `DumbProjectileBehavior::projectileLaunchAtObjectOrPosition`, DumbProjectileBehavior.cpp:345)
    /// and integrates gravity and friction into it each frame, in the PHASE_PHYSICS pass before the shell's own update.
    pub physics: bool,
    /// The shell's `PhysicsBehavior` `ForwardFriction`, `LateralFriction`, `AerodynamicFriction` (per frame).
    pub friction: [f32; 3],
    /// `TumbleRandomly` (DumbProjectileBehavior.cpp:381-387): three rates drawn at launch, applied each physics frame.
    pub tumble: bool,
}

impl Default for DumbProjectileParams {
    fn default() -> Self {
        DumbProjectileParams {
            first_height: 0.0,
            second_height: 0.0,
            first_percent_indent: 0.0,
            second_percent_indent: 0.0,
            max_lifespan: 10 * crate::math::LOGICFRAMES_PER_SECOND,
            orient_to_flight_path: true,
            flight_path_adjust_dist_per_frame: 0.0,
            physics: false,
            friction: PHYSICS_DEFAULT_FRICTION,
            tumble: false,
        }
    }
}

/// `DEFAULT_FORWARD_FRICTION`, `DEFAULT_LATERAL_FRICTION`, `DEFAULT_AERO_FRICTION` (PhysicsUpdate.cpp:57-60).
pub const PHYSICS_DEFAULT_FRICTION: [f32; 3] = [0.15, 0.15, 0.0];

/// `MissileAIUpdateModuleData` (sim copy).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MissileParams {
    pub try_to_follow_target: bool,
    pub fuel_lifetime: u32,
    pub ignition_delay: u32,
    pub initial_velocity: f32,
    pub initial_dist: f32,
    pub dive_distance: f32,
    pub lock_distance: f32,
    pub use_weapon_speed: bool,
    pub detonate_on_no_fuel: bool,
    pub kill_self_delay: u32,
    /// The projectile's locomotor `Speed` (per frame): the thrust's max speed (`setMaxSpeed`). Set to
    /// the weapon speed by `use_weapon_speed`.
    pub max_speed: f32,
    /// The locomotor `Acceleration` (per frame²): the thrust (`m_maxAccel`). Set to the weapon speed by
    /// `use_weapon_speed`.
    pub max_accel: f32,
    /// The locomotor `MaxThrustAngle` (radians): how far the thrust may lean off the heading.
    pub max_thrust_angle: f32,
    /// The locomotor `PreferredHeight` (`Locomotor::m_preferredHeight`): the cruise height above the terrain
    /// under the missile (0 = none). Retail: SCUD 120, Scud Storm 240, Tomahawk 120.
    pub preferred_height: f32,
    /// `PreferredHeightDamping`: the fraction of the height error corrected per frame.
    pub preferred_height_damping: f32,
    /// The locomotor `MinSpeed` (per frame): the floor of the desired speed (`moveTowardsPositionThrust`).
    pub min_speed: f32,
    /// The locomotor `Braking` (per frame²): the slowdown toward `min_speed` near the goal (0 = none).
    pub braking: f32,
    /// The locomotor `CloseEnoughDist`: how close along the path the AI counts as arrived, which gates the
    /// `KILL` detonation (`AIInternalMoveToState::update`, `doKillState`'s `isIdle`).
    pub close_enough_dist: f32,
    /// The projectile's `PhysicsBehavior` `[ForwardFriction, LateralFriction, AerodynamicFriction]`.
    pub friction: [f32; 3],
}

/// `LOGICFRAMES_PER_SECOND / 3`: `PhysicsUpdate.cpp` `MOTIVE_FRAMES`, how long a motive force keeps
/// `isMotive` set.
pub const MOTIVE_FRAMES: u32 = 10;

impl Default for MissileParams {
    /// The `TunnelDefenderMissileLocomotor` values (`locomotor.ini`: `Speed = 225`, `Acceleration = 675`,
    /// `MaxThrustAngle = 25`, per second, converted to frames) for synthetic tests; real missiles get
    /// their locomotor from the adapter.
    fn default() -> Self {
        MissileParams {
            try_to_follow_target: true,
            fuel_lifetime: 0,
            ignition_delay: 0,
            initial_velocity: 0.0,
            initial_dist: 0.0,
            dive_distance: 0.0,
            lock_distance: 75.0,
            use_weapon_speed: false,
            detonate_on_no_fuel: false,
            kill_self_delay: 3,
            max_speed: 7.5,
            max_accel: 0.75,
            max_thrust_angle: 25.0_f32.to_radians(),
            preferred_height: 0.0,
            preferred_height_damping: 1.0,
            min_speed: 0.0,
            braking: 0.0,
            close_enough_dist: 1.0,
            friction: PHYSICS_DEFAULT_FRICTION,
        }
    }
}

/// The physics and AI state of one missile in flight (`MissileAIUpdate` + its `PhysicsBehavior`).
#[derive(Clone, Debug)]
pub struct MissileFlight {
    pub params: MissileParams,
    pub state: MissileState,
    /// `m_stateTimestamp`.
    pub state_frame: u32,
    /// `m_fuelExpirationDate` (`u32::MAX` = infinite).
    pub fuel_end: u32,
    /// `m_isArmed`, set at ignition.
    pub armed: bool,
    /// `m_noTurnDistLeft`.
    pub no_turn_left: f32,
    /// `m_prevPos` for the no-turn count: updated only once the missile has ignited.
    pub no_turn_prev: crate::math::Coord3D,
    /// `m_framesTillDecoyed` (`MissileAIUpdate::setFramesTillCountermeasureDiversionOccurs`).
    pub decoy_at: u32,
    /// `PhysicsBehavior::m_vel` (per frame).
    pub vel: crate::math::Coord3D,
    /// Forces the locomotor and friction added since the last physics step (`m_accel`).
    pub accel: crate::math::Coord3D,
    /// The orientation (`getTransformMatrix`'s X vector), a unit vector.
    pub heading: crate::math::Coord3D,
    /// `WAS_AIRBORNE_LAST_FRAME`: a landing is the airborne -> grounded edge.
    pub airborne: bool,
    /// `m_motiveForceExpires`: `isMotive` while the frame is below it.
    pub motive_until: u32,
    /// The `OBJECT_STATUS_BRAKING` status (`doKillState`, MissileAIUpdate.cpp:~600). It is set on the first `KILL`
    /// update and never cleared for a projectile (`Projectiles never stop braking once they start`), so a
    /// `KILL_SELF` missile keeps its braking cheat. While set the physics does not integrate the XY position.
    pub braking: bool,
    /// The locomotor's `maxAccel` and whether its turn rate is `BIGNUM`, as the last state set them
    /// (`setMaxAcceleration` / `setMaxTurnRate`). The drive uses these until the next state changes them, so a
    /// `KILL_SELF` missile keeps thrusting (`doKillSelfState` sets nothing).
    pub accel_max: f32,
    pub turn_on: bool,
    /// The AI's goal for a tracked missile: the victim's position, kept after the victim dies (the AI keeps
    /// its last goal position, `setGoalPosition`).
    pub goal_last: Coord3D,
    /// `PRECISE_Z_POS` (`Locomotor::setUsePreciseZPos`): set by `doAttackState` once the missile is inside
    /// `DistanceToTargetBeforeDiving` (MissileAIUpdate.cpp:568-571). Missiles never clear it, so the dive is
    /// final. While set, the thrust aims at the goal's own height instead of the cruise height.
    pub precise_z: bool,
    /// The attached exhaust particle system is live: set at ignition (`doIgnitionState`'s
    /// `createAttachedParticleSystemID`), cleared when fuel runs out without `DetonateOnNoFuel`
    /// (`doAttackState`'s `tossExhaust`). Client data only (not in the CRC, never read by the sim).
    pub exhaust_lit: bool,
}

/// `PhysicsBehavior::applyFrictionalForces` (`PhysicsUpdate.cpp:446-500`) for a projectile at `height` above the
/// terrain. Above `-(3*3)*gravity` (`Thing::isSignificantlyAboveTerrain`, Thing.cpp) only aerodynamic friction acts:
/// `-aero * vel` on every axis. Below it the lateral velocity (across `heading`) loses `lateral` a frame, and the
/// forward velocity loses `forward` unless the locomotor drives the object (`isMotive`). Mass cancels (the force is
/// `mass * friction`, and `applyForce` divides by the mass). `friction` is `[forward, lateral, aerodynamic]`.
pub fn physics_friction_accel(vel: Coord3D, heading: Coord3D, height: f32, gravity: f32, motive: bool, friction: [f32; 3]) -> Coord3D {
    let [forward_f, lateral_f, aero] = friction;
    if height > -(3.0 * 3.0) * gravity {
        return Coord3D::new(-aero * vel.x, -aero * vel.y, -aero * vel.z);
    }
    if vel.x == 0.0 && vel.y == 0.0 {
        return Coord3D::ZERO;
    }
    let hl = (heading.x * heading.x + heading.y * heading.y).sqrt();
    let (dx, dy) = if hl > 0.0 { (heading.x / hl, heading.y / hl) } else { (1.0, 0.0) };
    let lateral_dot = vel.x * -dy + vel.y * dx;
    let mut ax = -(lateral_f * (lateral_dot * -dy));
    let mut ay = -(lateral_f * (lateral_dot * dx));
    if !motive {
        let forward_dot = vel.x * dx + vel.y * dy;
        ax += -(forward_f * (forward_dot * dx));
        ay += -(forward_f * (forward_dot * dy));
    }
    Coord3D::new(ax, ay, 0.0)
}

/// `PhysicsBehavior::getForwardSpeed3D` (PhysicsUpdate.cpp:978-1000): each axis of the velocity is multiplied by the
/// matching component of the direction, the speed is the root of the sum of the squares, and the sign is that of the
/// sum (`dot`). This is not `|v| cos(theta)`: a velocity off the axis of `dir` reads slower than its true forward part.
pub fn forward_speed_3d(vel: Coord3D, dir: Coord3D) -> f32 {
    let (vx, vy, vz) = (vel.x * dir.x, vel.y * dir.y, vel.z * dir.z);
    let speed = (vx * vx + vy * vy + vz * vz).sqrt();
    if vx + vy + vz >= 0.0 {
        speed
    } else {
        -speed
    }
}

/// `Locomotor::moveTowardsPositionThrust`'s force gate (Locomotor.cpp:2003): thrust, and the damping with it, is applied
/// only when the forward speed differs from the desired one or the thrust leans off the heading.
pub fn thrust_is_applied(forward_delta: f32, thrust_dir: Coord3D, heading: Coord3D) -> bool {
    forward_delta != 0.0 || thrust_dir.x != heading.x || thrust_dir.y != heading.y || thrust_dir.z != heading.z
}

/// `PartitionManager`'s `FROM_BOUNDINGSPHERE_3D` (PartitionManager.cpp:913-935): the centre distance less both bounding
/// sphere radii, never below zero.
pub fn bounding_sphere_gap(a: Coord3D, b: Coord3D, radius_a: f32, radius_b: f32) -> f32 {
    let d = Coord3D::new(b.x - a.x, b.y - a.y, b.z - a.z);
    ((d.x * d.x + d.y * d.y + d.z * d.z).sqrt() - radius_a - radius_b).max(0.0)
}

/// `getLocomotorDistanceToGoal` for a projectile (AIUpdate.cpp:2468-2472): the 3D centre distance to the goal, which
/// `AIInternalMoveToState::update` compares with `CloseEnoughDist` (AIStates.cpp:1910).
pub fn arrived_at_goal(pos: Coord3D, goal: Coord3D, close_enough: f32) -> bool {
    let d = Coord3D::new(goal.x - pos.x, goal.y - pos.y, goal.z - pos.z);
    (d.x * d.x + d.y * d.y + d.z * d.z).sqrt() < close_enough
}

/// `calcDirectionToApplyThrust` (`Locomotor.cpp:194-264`): the thrust direction that points the missile's
/// velocity, gravity included, at `goal` when the thrust is `max_accel`. It solves for the smallest
/// positive time to reach the goal, and aims the thrust at `D / t - velocity`; with no solution it aims
/// at the goal. `None` when the missile is already at the goal (the caller keeps its heading).
pub fn missile_thrust_direction(pos: crate::math::Coord3D, goal: crate::math::Coord3D, vel: crate::math::Coord3D, gravity: f32, max_accel: f32) -> Option<crate::math::Coord3D> {
    use crate::math::Coord3D;
    const NEARLY_ZERO: f32 = 1e-4;
    let d = Coord3D::new(goal.x - pos.x, goal.y - pos.y, goal.z - pos.z);
    let dist_sqr = d.x * d.x + d.y * d.y + d.z * d.z;
    if dist_sqr < NEARLY_ZERO * NEARLY_ZERO {
        return None;
    }
    let dist = dist_sqr.sqrt();
    let normalise = |v: Coord3D| {
        let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
        if l > 0.0 { Coord3D::new(v.x / l, v.y / l, v.z / l) } else { v }
    };
    // The velocity with gravity added for the next frame.
    let cv = Coord3D::new(vel.x, vel.y, vel.z + gravity);
    let cv_mag = (cv.x * cv.x + cv.y * cv.y + cv.z * cv.z).sqrt();
    let denom = cv_mag * cv_mag - max_accel * max_accel;
    if denom.abs() > NEARLY_ZERO {
        let t = dist * (cv_mag + max_accel) / denom;
        let t2 = dist * (cv_mag - max_accel) / denom;
        if t >= 0.0 || t2 >= 0.0 {
            let t = if t < 0.0 || (t2 >= 0.0 && t2 < t) { t2 } else { t };
            if t.abs() > NEARLY_ZERO {
                return Some(normalise(Coord3D::new(d.x / t - cv.x, d.y / t - cv.y, d.z / t - cv.z)));
            }
        }
    }
    Some(normalise(d))
}

#[cfg(test)]
mod thrust_tests {
    use super::*;
    use crate::math::Coord3D;

    #[test]
    fn thrust_leans_up_to_hold_altitude_against_gravity() {
        // At rest, a thrust of 0.5 cannot cover gravity 1 straight at a point 100 away on the same level:
        // the solution tilts the thrust upward so the velocity points at the goal.
        let d = missile_thrust_direction(Coord3D::ZERO, Coord3D::new(100.0, 0.0, 0.0), Coord3D::ZERO, -1.0, 0.75).unwrap();
        assert!(d.z > 0.3 && d.x > 0.0, "{d:?}");
    }

    #[test]
    fn thrust_points_at_the_goal_when_there_is_no_gravity_to_fight() {
        let d = missile_thrust_direction(Coord3D::ZERO, Coord3D::new(0.0, 100.0, 0.0), Coord3D::ZERO, 0.0, 0.75).unwrap();
        assert!(d.x.abs() < 1e-4 && (d.y - 1.0).abs() < 1e-4 && d.z.abs() < 1e-4, "{d:?}");
    }

    #[test]
    fn thrust_at_the_goal_keeps_the_heading() {
        assert_eq!(missile_thrust_direction(Coord3D::ZERO, Coord3D::ZERO, Coord3D::ZERO, -1.0, 0.75), None);
    }
}

#[cfg(test)]
mod friction_tests {
    use super::*;
    use crate::math::Coord3D;

    #[test]
    fn ground_friction_slows_forward_motion_only_when_not_driven() {
        let v = Coord3D::new(4.0, 0.0, 0.0);
        let h = Coord3D::new(1.0, 0.0, 0.0);
        // Driven (isMotive): no forward friction, the velocity is along the heading.
        assert_eq!(physics_friction_accel(v, h, 0.0, -1.0, true, PHYSICS_DEFAULT_FRICTION), Coord3D::ZERO);
        // Not driven: forward friction 0.15 * 4 opposes the motion.
        let a = physics_friction_accel(v, h, 0.0, -1.0, false, PHYSICS_DEFAULT_FRICTION);
        assert!((a.x + 0.6).abs() < 1e-6 && a.y == 0.0, "{a:?}");
    }

    #[test]
    fn lateral_friction_acts_across_the_heading_even_when_driven() {
        let v = Coord3D::new(0.0, 4.0, 0.0);
        let h = Coord3D::new(1.0, 0.0, 0.0);
        let a = physics_friction_accel(v, h, 0.0, -1.0, true, PHYSICS_DEFAULT_FRICTION);
        assert!((a.y + 0.6).abs() < 1e-6 && a.x == 0.0, "{a:?}");
    }

    #[test]
    fn no_ground_friction_when_significantly_above_the_terrain() {
        let v = Coord3D::new(4.0, 0.0, 0.0);
        let h = Coord3D::new(1.0, 0.0, 0.0);
        assert_eq!(physics_friction_accel(v, h, 9.5, -1.0, false, PHYSICS_DEFAULT_FRICTION), Coord3D::ZERO);
        assert_ne!(physics_friction_accel(v, h, 8.5, -1.0, false, PHYSICS_DEFAULT_FRICTION), Coord3D::ZERO);
    }

    #[test]
    fn the_shell_friction_values_are_the_ini_ones() {
        // ForwardFriction 0.5 and LateralFriction 0.25 from the PhysicsBehavior INI replace the 0.15 defaults.
        let v = Coord3D::new(4.0, 0.0, 0.0);
        let h = Coord3D::new(1.0, 0.0, 0.0);
        let a = physics_friction_accel(v, h, 0.0, -1.0, false, [0.5, 0.25, 0.0]);
        assert!((a.x + 2.0).abs() < 1e-6, "{a:?}");
        let b = physics_friction_accel(Coord3D::new(0.0, 4.0, 0.0), h, 0.0, -1.0, false, [0.5, 0.25, 0.0]);
        assert!((b.y + 1.0).abs() < 1e-6, "{b:?}");
    }

    #[test]
    fn aerodynamic_friction_applies_above_the_terrain_on_every_axis() {
        let v = Coord3D::new(4.0, 0.0, -2.0);
        let h = Coord3D::new(1.0, 0.0, 0.0);
        let a = physics_friction_accel(v, h, 9.5, -1.0, false, [0.15, 0.15, 0.1]);
        assert!((a.x + 0.4).abs() < 1e-6 && (a.z - 0.2).abs() < 1e-6, "{a:?}");
    }

    #[test]
    fn forward_speed_is_the_per_axis_getforwardspeed3d() {
        // PhysicsUpdate.cpp:988-1000: (2*0.6, 2*0.8, 0) -> sqrt(1.2^2 + 1.6^2) = 2.0, not the dot product 2.8.
        let s = forward_speed_3d(Coord3D::new(2.0, 2.0, 0.0), Coord3D::new(0.6, 0.8, 0.0));
        assert!((s - 2.0).abs() < 1e-6, "{s}");
        // Backwards: the sign follows the sum of the components.
        let b = forward_speed_3d(Coord3D::new(-3.0, 0.0, 0.0), Coord3D::new(1.0, 0.0, 0.0));
        assert!((b + 3.0).abs() < 1e-6, "{b}");
    }

    #[test]
    fn thrust_is_only_applied_when_it_changes_the_speed_or_the_heading() {
        let h = Coord3D::new(1.0, 0.0, 0.0);
        assert!(!thrust_is_applied(0.0, h, h));
        assert!(thrust_is_applied(0.5, h, h));
        assert!(thrust_is_applied(0.0, Coord3D::new(0.0, 1.0, 0.0), h));
    }

    #[test]
    fn arrival_is_the_3d_distance_to_the_goal() {
        // 2D-close but 50 above: not arrived (AIUpdate.cpp:2468-2472 is FROM_CENTER_3D for a projectile).
        assert!(!arrived_at_goal(Coord3D::new(0.0, 0.0, 50.0), Coord3D::ZERO, 1.0));
        assert!(arrived_at_goal(Coord3D::new(0.5, 0.0, 0.0), Coord3D::ZERO, 1.0));
    }

    #[test]
    fn the_kill_gate_subtracts_both_bounding_spheres() {
        // FROM_BOUNDINGSPHERE_3D (PartitionManager.cpp:913-935): centre distance 10, radii 3 and 4 -> 3, never below 0.
        let g = bounding_sphere_gap(Coord3D::ZERO, Coord3D::new(10.0, 0.0, 0.0), 3.0, 4.0);
        assert!((g - 3.0).abs() < 1e-6, "{g}");
        assert_eq!(bounding_sphere_gap(Coord3D::ZERO, Coord3D::new(2.0, 0.0, 0.0), 3.0, 4.0), 0.0);
    }
}

/// `NeutronMissileUpdateModuleData` plus the object's `HeightDieUpdate` dive height (sim copy).
#[derive(Clone, Debug, PartialEq)]
pub struct NeutronParams {
    pub template: NeutronMissileTemplate,
    /// `HeightDieUpdate::TargetHeight` on the same object: the missile detonates this high above
    /// the ground while diving.
    pub dive_height: f32,
}

/// The projectile object template's behavior (`DumbProjectileBehavior`, `MissileAIUpdate` or
/// `NeutronMissileUpdate`).
#[derive(Clone, Debug, PartialEq)]
pub enum ProjectileBehavior {
    Dumb(DumbProjectileParams),
    Missile(MissileParams),
    Neutron(NeutronParams),
}

/// `GarrisonHitKill*` of `DumbProjectileBehaviorModuleData` / `MissileAIUpdateModuleData`
/// (DumbProjectileBehavior.cpp:485-515, MissileAIUpdate.cpp:333-364). Both behaviors share the block.
#[derive(Clone, Debug, PartialEq)]
pub struct GarrisonHitKill {
    /// `GarrisonHitKillCount`: occupants cleared by one hit. 0 is off (the adapter omits the whole block then).
    pub count: i32,
    /// `GarrisonHitKillRequiredKindOf` / `GarrisonHitKillForbiddenKindOf` as sim kind-of bits
    /// (`ThingTemplate::isKindOfMulti`: every required bit set, no forbidden bit).
    pub kind_of: u64,
    pub kind_of_not: u64,
    /// `GarrisonHitKillFX`: played on the building when its garrison is cleared (instead of the explosion).
    pub fx: Option<String>,
}

/// What the weapon's `ProjectileObject` template contributes to a fired projectile.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectileTemplate {
    pub behavior: ProjectileBehavior,
    pub bounding_radius: f32,
    /// `KINDOF_SMALL_MISSILE`: only these are reported to a victim's countermeasures.
    pub small_missile: bool,
    /// A `NeutronMissileUpdate` projectile: on detonation the sim spawns this warhead object (by
    /// template name) and kills it so its `NeutronMissileSlowDeathBehavior` runs the ground blasts.
    pub neutron_warhead: Option<String>,
    /// A `NeutronBlastBehavior` projectile (`NeutronCannonShell`): the on-death blast.
    pub neutron_blast: Option<crate::superweapon::NeutronBlastTemplate>,
    /// `GarrisonHitKillCount > 0`: the projectile clears a garrison building instead of exploding.
    pub garrison_hit_kill: Option<GarrisonHitKill>,
}

/// `MissileAIUpdate::MissileStateType` (the states the sim uses).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissileState {
    Launch,
    Ignition,
    AttackNoTurn,
    Attack,
    /// `KILL`: inside the lock distance the missile brakes straight at its goal and detonates on arrival
    /// (`doKillState`).
    Kill,
    /// `KILL_SELF` (`airborneTargetGone`): no warhead; removed `KillSelfDelay` frames after entry.
    KillSelf,
}

fn sub(a: Coord3D, b: Coord3D) -> Coord3D {
    Coord3D::new(a.x - b.x, a.y - b.y, a.z - b.z)
}
fn add(a: Coord3D, b: Coord3D) -> Coord3D {
    Coord3D::new(a.x + b.x, a.y + b.y, a.z + b.z)
}
fn scale(a: Coord3D, s: f32) -> Coord3D {
    Coord3D::new(a.x * s, a.y * s, a.z * s)
}
fn length(a: Coord3D) -> f32 {
    sqrt(a.x * a.x + a.y * a.y + a.z * a.z)
}

/// `BezierSegment::evaluateBezSegmentAtT`: the standard cubic Bernstein form.
pub fn bezier(cp: &[Coord3D; 4], t: f32) -> Coord3D {
    let u = 1.0 - t;
    let (b0, b1, b2, b3) = (u * u * u, 3.0 * t * u * u, 3.0 * t * t * u, t * t * t);
    Coord3D::new(
        cp[0].x * b0 + cp[1].x * b1 + cp[2].x * b2 + cp[3].x * b3,
        cp[0].y * b0 + cp[1].y * b1 + cp[2].y * b2 + cp[3].y * b3,
        cp[0].z * b0 + cp[1].z * b1 + cp[2].z * b2 + cp[3].z * b3,
    )
}

/// `BezierSegment::getSegmentPoints` + `BezFwdIterator`: `n` samples from `t=0` to `t=1`
/// inclusive (`n <= 1` gives the start point).
pub fn segment_points(cp: [Coord3D; 4], n: usize) -> Vec<Coord3D> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![cp[0]];
    }
    (0..n).map(|i| bezier(&cp, i as f32 / (n - 1) as f32)).collect()
}

/// `BezierSegment::splitSegmentAtT`.
fn split_segment_at_t(cp: &[Coord3D; 4], t: f32) -> ([Coord3D; 4], [Coord3D; 4]) {
    let p0p1 = add(scale(sub(cp[1], cp[0]), t), cp[0]);
    let p1p2 = add(scale(sub(cp[2], cp[1]), t), cp[1]);
    let p2p3 = add(scale(sub(cp[3], cp[2]), t), cp[2]);
    let tri_left = add(scale(sub(p1p2, p0p1), t), p0p1);
    let tri_right = add(scale(sub(p2p3, p1p2), t), p1p2);
    let mid = bezier(cp, t);
    ([cp[0], p0p1, tri_left, mid], [mid, tri_right, p2p3, cp[3]])
}

/// `BezierSegment::getApproximateLength`: the `(chord + polygon)/2` estimate, subdividing while the
/// error exceeds `tolerance`.
pub fn approximate_length(cp: &[Coord3D; 4], tolerance: f32) -> f32 {
    let chord = length(sub(cp[3], cp[0]));
    let polygon = length(sub(cp[1], cp[0])) + length(sub(cp[2], cp[1])) + length(sub(cp[3], cp[2]));
    if polygon - chord > tolerance {
        let (s1, s2) = split_segment_at_t(cp, 0.5);
        return approximate_length(&s1, tolerance) + approximate_length(&s2, tolerance);
    }
    (chord + polygon) * 0.5
}

/// `DumbProjectileBehavior::calcFlightPath`: build the four control points (inner two this high
/// above the highest intervening terrain, this far along the firer→victim line) and sample the
/// curve at `speed` units per frame.
pub fn compute_flight_path(
    start: &Coord3D,
    end: &Coord3D,
    p: &DumbProjectileParams,
    highest_intervening_terrain: f32,
    speed: f32,
) -> Vec<Coord3D> {
    let mut cp = [*start, Coord3D::ZERO, Coord3D::ZERO, *end];
    let d = sub(*end, *start);
    let dist = length(d);
    let (ux, uy) = if dist > 0.0 { (d.x / dist, d.y / dist) } else { (0.0, 0.0) };
    let f1 = dist * p.first_percent_indent;
    let f2 = dist * p.second_percent_indent;
    cp[1] = Coord3D::new(start.x + ux * f1, start.y + uy * f1, 0.0);
    cp[2] = Coord3D::new(start.x + ux * f2, start.y + uy * f2, 0.0);
    let high = highest_intervening_terrain.max(start.z).max(end.z);
    cp[1].z = high + p.first_height;
    cp[2].z = high + p.second_height;
    let segments = if speed > 0.0 { ceil(approximate_length(&cp, 1.0) / speed) as usize } else { 0 };
    segment_points(cp, segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_endpoints_and_symmetry() {
        let cp = [
            Coord3D::new(0.0, 0.0, 0.0),
            Coord3D::new(0.0, 0.0, 100.0),
            Coord3D::new(100.0, 0.0, 100.0),
            Coord3D::new(100.0, 0.0, 0.0),
        ];
        assert_eq!(bezier(&cp, 0.0), cp[0]);
        assert_eq!(bezier(&cp, 1.0), cp[3]);
        // Symmetric control points put the apex at t=0.5, 75 units up.
        assert!((bezier(&cp, 0.5).z - 75.0).abs() < 1e-3);
    }

    #[test]
    fn arc_clears_the_requested_height() {
        let p = DumbProjectileParams { first_height: 20.0, second_height: 40.0, ..DumbProjectileParams::default() };
        let path = compute_flight_path(&Coord3D::new(0.0, 0.0, 0.0), &Coord3D::new(200.0, 0.0, 0.0), &p, 5.0, 10.0);
        assert!(path.len() > 2);
        let apex = path.iter().fold(0.0f32, |m, q| m.max(q.z));
        // The curve stays below its control points, but well above the terrain it clears.
        assert!(apex > 5.0 + 10.0, "apex {apex} should clear the terrain");
        assert!(apex < 5.0 + 40.0 + 1e-3, "apex {apex} stays under the control point");
        assert!((path[0].z - 0.0).abs() < 1e-4);
        assert!((path.last().unwrap().z - 0.0).abs() < 1e-4);
        // A taller second control point lifts the arc.
        let p2 = DumbProjectileParams { second_height: 80.0, ..p };
        let path2 = compute_flight_path(&Coord3D::new(0.0, 0.0, 0.0), &Coord3D::new(200.0, 0.0, 0.0), &p2, 5.0, 10.0);
        let apex2 = path2.iter().fold(0.0f32, |m, q| m.max(q.z));
        assert!(apex2 > apex, "apex {apex2} should exceed {apex}");
    }
}
