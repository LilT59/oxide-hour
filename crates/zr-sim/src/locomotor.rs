//! `Locomotor` (`GameLogic/Object/Locomotor.cpp`): the appearance-specific movement kernels.
//!
//! `zr-sim` does not depend on the INI crate: the adapter converts `zr_templates::LocomotorTemplate`
//! into [`LocomotorParams`](crate::template::LocomotorParams) (already per-frame units).
//!
//! Ported: `locoUpdate_moveTowardsPosition`'s dispatch, `moveTowardsPositionLegs`/`Wheels`/`Treads`
//! (the same ground kernel), `moveTowardsPositionHover`, `moveTowardsPositionThrust`,
//! `moveTowardsPositionWings` (min speed + circling) and `moveTowardsPositionClimb`,
//! `handleBehaviorZ` (`SURFACE_RELATIVE_HEIGHT` / `ABSOLUTE_HEIGHT` / `NO_Z_MOTIVE_FORCE`),
//! `locoUpdate_moveTowardsAngle` and `locoUpdate_maintainCurrentPosition`, the wheels donut timer and
//! the persistent `m_brakingFactor` (per object in `crate::physics::Physics`).
//!
//! Movement is force-driven for every appearance (`moveTowardsPositionLegs`/`Wheels`/`Treads`/
//! `Hover`/`Wings`/`Thrust` apply motive forces to [`crate::physics`]); gravity, friction and the
//! ground collision live there. Hover and wings keep their `SURFACE_RELATIVE_HEIGHT` through
//! `handleBehaviorZ`'s lift force. Reduced: slope tilt and the client-side pitch/roll/wobble are not
//! simulated.

use crate::condition::BodyDamageType;
use crate::logic::GameLogic;
use crate::math::{acos, angle_diff, asin, atan2, cos, normalize_angle, sin, sqrt, Coord3D, PI};
use crate::object::{status, Object, ObjectId};
use crate::template::{LocomotorKind, LocomotorParams, LocomotorSet, ZBehavior};
use crate::terrain::TerrainLogic;
use crate::transform::Matrix3D;

/// `Locomotor::getMaxSpeedForCondition` / `getMaxTurnRate` / `getMaxAcceleration` / `getMaxLift`
/// (`Locomotor.cpp:789-860`) with the body's `BodyDamageType`: at `IS_CONDITION_WORSE` than the movement
/// penalty state `penalty` (`GlobalData::m_movementPenaltyDamageState`, `!(condition < penalty)`,
/// `BodyModule.h:101-103`) the `*Damaged` values are used, otherwise the pristine ones. Every kernel starts from
/// these (`Locomotor.cpp:1618-1625`), so the result replaces the pristine fields in place; the `m_maxSpeed` /
/// `m_maxTurnRate` / `m_maxAccel` / `m_maxLift` clamps were already applied by [`Object::locomotor`].
/// `getBraking` takes no condition.
pub fn apply_body_damage_state(loco: &mut LocomotorParams, condition: BodyDamageType, penalty: BodyDamageType) {
    if condition < penalty {
        return;
    }
    loco.max_speed = loco.max_speed_damaged;
    loco.max_turn_rate = loco.max_turn_rate_damaged;
    loco.acceleration = loco.acceleration_damaged;
    loco.lift = loco.lift_damaged;
}

/// `PATHFIND_CELL_SIZE_F` (`AIPathfind.h`), used by the treads/wheels slow-down tests.
const CELL_F: f32 = crate::pathfind::PATHFIND_CELL_SIZE_F;
/// `MIN_VEL` in `locoUpdate_moveTowardsPosition`.
const MIN_VEL: f32 = CELL_F / crate::math::LOGICFRAMES_PER_SECOND as f32;
/// `DONUT_DISTANCE` / `DONUT_TIME_DELAY_SECONDS` (`Locomotor.cpp:50-51`).
const DONUT_DISTANCE: f32 = 4.0 * CELL_F;
pub(crate) const DONUT_TIME_DELAY_FRAMES: u32 = (2.5 * crate::math::LOGICFRAMES_PER_SECOND as f32) as u32;
/// `MAX_BRAKING_FACTOR` (`Locomotor.cpp:54`): the cap on the treads kernel's squared slow-down ratio.
const MAX_BRAKING_FACTOR: f32 = 5.0;

/// `Locomotor::calcSlowDownDist` (`Locomotor.cpp:81`).
fn calc_slow_down_dist(cur_speed: f32, desired_speed: f32, max_braking: f32) -> f32 {
    let delta = cur_speed - desired_speed;
    if delta <= 0.0 {
        return 0.0;
    }
    let dist = (delta * delta / max_braking.abs()) * 0.5;
    dist * 1.05
}

/// `Pathfinder::validLocomotorSurfacesForCellType` (`AIPathfind.cpp:4892-4916`).
fn surfaces_for_cell_type(t: crate::pathfind::CellType) -> u32 {
    use crate::pathfind::{surface, CellType};
    match t {
        CellType::Clear => surface::GROUND | surface::AIR,
        CellType::Water => surface::WATER | surface::AIR,
        CellType::Cliff => surface::CLIFF | surface::AIR,
        CellType::Rubble => surface::RUBBLE | surface::AIR,
        CellType::Obstacle | CellType::BridgeImpassable | CellType::Impassable => surface::AIR,
    }
}

/// `Pathfinder::validMovementTerrain` (`AIPathfind.cpp:4921-4942`): only the *terrain* counts, not
/// obstacles (an obstacle or impassable cell is accepted), and the cell must offer one of the
/// locomotor's surfaces. The two early returns are only `CELL_OBSTACLE` and `CELL_IMPASSABLE`
/// (`:4928-4929`); a `CELL_BRIDGE_IMPASSABLE` cell falls through to
/// `validLocomotorSurfacesForCellType`, which offers `AIR` only (`:4904-4906`), so a ground
/// locomotor may not enter it. The port's cell grid is the ground layer (`Object::getLayer` is not
/// stored per object here), which is the layer every wheels locomotor in `locomotor.ini` moves on.
pub(crate) fn valid_movement_terrain(pathfinder: &crate::pathfind::Pathfinder, surfaces: u32, pos: &Coord3D) -> bool {
    let Some(ty) = pathfinder.cell_type_at(pos) else { return false };
    valid_movement_terrain_cell(ty, surfaces)
}

/// The body of `Pathfinder::validMovementTerrain` (`AIPathfind.cpp:4924-4941`) for one cell type,
/// split out so the cell types the port's grid cannot produce (a `CELL_BRIDGE_IMPASSABLE` cell,
/// which the engine creates on a bridge over water or a cliff) stay testable.
fn valid_movement_terrain_cell(ty: crate::pathfind::CellType, surfaces: u32) -> bool {
    use crate::pathfind::CellType;
    // "Only do terrain, not obstacle cells": `CELL_OBSTACLE` and `CELL_IMPASSABLE` return true
    // (`:4928-4929`). `CELL_BRIDGE_IMPASSABLE` is *not* in that list (`:4904-4906`, AIR only).
    if matches!(ty, CellType::Obstacle | CellType::Impassable) {
        return true;
    }
    surfaces_for_cell_type(ty) & surfaces != 0
}

/// `PhysicsBehavior::getMass()` (`PhysicsUpdate.cpp:1089-1096`) as the free kernels read it: the
/// template mass plus `contain->getContainedItemsMass()` (`OpenContain.cpp:835-845`), which the port
/// caches in [`crate::object::Object::contained_mass`]. `1.0` for an object without a physics module
/// keeps the old fallback of the `unwrap_or`.
fn obj_mass(obj: &Object) -> f32 {
    obj.get_mass().unwrap_or(1.0).max(0.0001)
}

/// `PhysicsBehavior::applyMotiveForce` inlined for the free kernels (the object owns its physics).
fn apply_motive_force_obj(obj: &mut Object, force: Coord3D, frame: u32) {
    // `Real massInv = 1.0f / mass; m_accel += modForce * massInv` (`PhysicsUpdate.cpp:335-338`),
    // unclamped (as in `applyForce`).
    let mass_inv = 1.0 / obj.get_mass().unwrap_or(1.0);
    if let Some(p) = obj.physics.as_mut() {
        p.motive_force_expires = 0;
        p.accel.x += force.x * mass_inv;
        p.accel.y += force.y * mass_inv;
        p.accel.z += force.z * mass_inv;
        p.motive_force_expires = frame + crate::physics::MOTIVE_FRAMES;
        // `applyForce` from an external module wakes the module with `UPDATE_SLEEP_NONE` (PhysicsUpdate.cpp:401-402):
        // its update runs next frame, not this one. The kernel has no `GameLogic`, so the flag is flushed at the end of
        // the frame (`flush_physics_wake_requests`).
        p.wake_requested = true;
    }
}

/// `Locomotor::rotateObjAroundLocoPivot` (Locomotor.cpp:2134-2208): turn toward `goal` by at most
/// `max_turn_rate` and return the signed remaining angle before clamping (`relAngle`, `stdAngleDiff`).
/// With a `TurnPivotOffset` the object turns about a point `offset * bounding radius` ahead of its centre:
/// the transform rotates about that point, so the position moves too (:2141-2166). Braking turns about the
/// centre (`offset = 0`, :2141).
fn rotate_towards_position(obj: &mut Object, goal: &Coord3D, max_turn_rate: f32, pivot_offset: f32) -> f32 {
    let (amount, turning) = rotate_about_loco_pivot(obj, goal, max_turn_rate, pivot_offset);
    // The callers do `physics->setTurning(rotating)` (`Locomotor.cpp:927`, `:1472-1474`).
    if let Some(p) = obj.physics.as_mut() {
        p.turning = turning;
    }
    amount
}

/// `Locomotor::rotateObjAroundLocoPivot` (`Locomotor.cpp:2134`) without its `setTurning`: returns the signed
/// remaining angle and the `PhysicsTurningType` it returns (`-1` negative, `0` none, `1` positive). The
/// blocked branch of the kernel uses the turn (`*blocked = TURN_NONE != ...`, `Locomotor.cpp:1056`) and does not
/// write `m_turning`, so it calls this directly.
fn rotate_about_loco_pivot(obj: &mut Object, goal: &Coord3D, max_turn_rate: f32, pivot_offset: f32) -> (f32, i8) {
    let angle = obj.angle;
    let braking = obj.physics.as_ref().is_some_and(|p| p.braking);
    let offset = if braking { 0.0 } else { pivot_offset };
    let (desired, turn_pos) = if offset != 0.0 {
        let turn_point_offset = offset * obj.template.geometry.bounding_circle_radius();
        let turn = Coord3D::new(
            obj.position.x + cos(angle) * turn_point_offset,
            obj.position.y + sin(angle) * turn_point_offset,
            obj.position.z,
        );
        // Very close to the goal the pivot twitches on rounding error; the engine returns `TURN_NONE` (:2153-2154).
        if (goal.x - turn.x).abs() < 0.1 && (goal.y - turn.y).abs() < 0.1 {
            return (0.0, 0);
        }
        (atan2(goal.y - turn.y, goal.x - turn.x), Some(turn))
    } else {
        (atan2(goal.y - obj.position.y, goal.x - obj.position.x), None)
    };
    let amount = angle_diff(desired, angle);
    let (clamped, turning) = if amount > max_turn_rate {
        (max_turn_rate, 1i8)
    } else if amount < -max_turn_rate {
        (-max_turn_rate, -1i8)
    } else {
        (amount, 0i8)
    };
    if let Some(turn) = turn_pos {
        // `Matrix3D tmp(1); tmp.Translate(turnPos.x, turnPos.y, 0); tmp.In_Place_Pre_Rotate_Z(amount);
        // tmp.Translate(-turnPos.x, -turnPos.y, 0); mtx.mul(tmp, *getTransformMatrix()); setTransformMatrix(&mtx)`
        // (Locomotor.cpp:2171-2180, Matrix3D from matrix3d.h). The heading and position both come from the
        // resulting matrix (`Thing::setTransformMatrix`, Thing.cpp:263-283).
        let mut tmp = crate::transform::Matrix3D::identity();
        tmp.translate(turn.x, turn.y, 0.0);
        tmp.in_place_pre_rotate_z(clamped);
        tmp.translate(-turn.x, -turn.y, 0.0);
        let mtx = crate::transform::Matrix3D::mul(&tmp, &obj.transform);
        obj.set_transform_matrix(mtx);
    } else {
        // `obj->setOrientation(normalizeAngle(angle + amount))` (Locomotor.cpp:2208-2209).
        obj.set_orientation(normalize_angle(angle + clamped));
    }
    (amount, turning)
}

/// `moveTowardsPositionLegs` / `moveTowardsPositionTreads` share the forward-force pattern:
/// compute a goal speed, then apply `mass * acceleration` along the facing vector.
fn maintain_goal_speed(obj: &mut Object, loco: &LocomotorParams, goal_speed: f32, actual_speed: f32, braking_factor: f32, frame: u32) {
    let speed_delta = goal_speed - actual_speed;
    if speed_delta == 0.0 {
        return;
    }
    let mass = obj_mass(obj);
    let acceleration = if speed_delta > 0.0 { loco.acceleration } else { -braking_factor * loco.braking };
    let mut accel_force = mass * acceleration;
    let max_force_needed = mass * speed_delta;
    if accel_force.abs() > max_force_needed.abs() {
        accel_force = max_force_needed;
    }
    let (dx, dy) = (cos(obj.angle), sin(obj.angle));
    apply_motive_force_obj(obj, Coord3D::new(accel_force * dx, accel_force * dy, 0.0), frame);
}

/// `moveTowardsPositionLegs` (`Locomotor.cpp:1615`).
#[allow(clippy::too_many_arguments)]
fn move_towards_position_legs(
    obj: &mut Object,
    loco: &LocomotorParams,
    goal: &Coord3D,
    on_path_dist: f32,
    desired_speed: f32,
    terrain: &TerrainLogic,
    pathfinder: &crate::pathfind::Pathfinder,
    frame: u32,
    gravity: f32,
) {
    let actual = obj.physics.as_ref().map(|p| p.forward_speed_2d(obj.angle)).unwrap_or(0.0);
    let desired_angle = atan2(goal.y - obj.position.y, goal.x - obj.position.x);
    let rel_angle = angle_diff(desired_angle, obj.angle);
    // `locoUpdate_moveTowardsAngle(obj, desiredAngle)` (`Locomotor.cpp:1681`), not a turn toward the
    // goal: with `MinSpeed == 0` it turns toward a point 1000 units out along that bearing, through
    // the locomotor's turn pivot (`:904-917`).
    move_towards_angle_force(obj, loco, desired_angle, terrain, pathfinder, frame, gravity);
    let angle_coeff = (rel_angle.abs() / (PI / 4.0)).min(1.0);
    let mut goal_speed = (1.0 - angle_coeff) * desired_speed;
    // `if (onPathDistToGoal < slowDownDist && !getFlag(NO_SLOW_DOWN_AS_APPROACHING_DEST)) goalSpeed =
    // m_template->m_minSpeed;` (`Locomotor.cpp:1671-1674`).
    let slow = calc_slow_down_dist(actual, loco.min_speed, loco.braking);
    if on_path_dist < slow && !obj.loco_rt.no_slow_down_as_approaching_dest {
        goal_speed = loco.min_speed;
    }
    maintain_goal_speed(obj, loco, goal_speed, actual, 1.0, frame);
}

/// `moveTowardsPositionTreads` (`Locomotor.cpp:1165`).
fn move_towards_position_treads(obj: &mut Object, loco: &LocomotorParams, goal: &Coord3D, on_path_dist: f32, desired_speed: f32, frame: u32) {
    // `rotateTowardsPosition` (`Locomotor.cpp:1185`) turns the body first, and `actualSpeed =
    // physics->getForwardSpeed2D()` (`:1207`) then projects the velocity on the *turned* heading
    // (`getUnitDirectionVector2D` is rebuilt by `setTransformMatrix`), so the speed is read after the turn.
    let rel_angle = rotate_towards_position(obj, goal, loco.max_turn_rate, loco.turn_pivot_offset);
    let actual = obj.physics.as_ref().map(|p| p.forward_speed_2d(obj.angle)).unwrap_or(0.0);
    let angle_coeff = (rel_angle.abs() / (PI / 4.0)).min(1.0);
    let dx = obj.position.x - goal.x;
    let dy = obj.position.y - goal.y;
    let mut goal_speed = (1.0 - angle_coeff) * desired_speed;
    let slow_down_dist = (actual / 1.50) * (actual / loco.braking);
    if dx * dx + dy * dy < (2.0 * CELL_F) * (2.0 * CELL_F) && angle_coeff > 0.05 {
        goal_speed = actual * 0.6;
    }
    let start_braking = on_path_dist < slow_down_dist && !obj.physics.as_ref().is_some_and(|p| p.braking) && !obj.loco_rt.no_slow_down_as_approaching_dest;
    // `Locomotor.cpp:1215-1232`: the start test sets `IS_BRAKING` *and* `m_brakingFactor = 1.1`; the far
    // test clears the flag only. `m_brakingFactor` is a persistent `Locomotor` member (`:445`), so a
    // non-braking frame keeps the value the last braking frame left.
    if start_braking {
        if let Some(p) = obj.physics.as_mut() {
            p.braking = true;
            p.braking_factor = 1.1;
        }
    }
    if on_path_dist > CELL_F && on_path_dist > 2.0 * slow_down_dist {
        if let Some(p) = obj.physics.as_mut() {
            p.braking = false;
        }
    }
    if obj.physics.as_ref().is_some_and(|p| p.braking) {
        // `m_brakingFactor = slowDownDist/onPathDistToGoal`, squared, capped at `MAX_BRAKING_FACTOR`
        // (`:1228-1231`). A plain division, as in the engine: `inf > 5.0f` clamps too.
        let mut factor = slow_down_dist / on_path_dist;
        factor *= factor;
        if factor > MAX_BRAKING_FACTOR {
            factor = MAX_BRAKING_FACTOR;
        }
        if let Some(p) = obj.physics.as_mut() {
            p.braking_factor = factor;
        }
        if slow_down_dist > on_path_dist {
            goal_speed = actual - loco.braking;
            if goal_speed < 0.0 {
                goal_speed = 0.0;
            }
        } else if slow_down_dist > on_path_dist * 0.75 {
            goal_speed = actual - loco.braking / 2.0;
            if goal_speed < 0.0 {
                goal_speed = 0.0;
            }
        } else {
            goal_speed = actual;
        }
    }
    let braking_factor = obj.physics.as_ref().map_or(1.0, |p| p.braking_factor);
    maintain_goal_speed(obj, loco, goal_speed, actual, braking_factor, frame);
}

/// `moveTowardsPositionWheels` (`Locomotor.cpp:1279`), including the persistent
/// `MOVING_BACKWARDS` / `DOING_THREE_POINT_TURN` state used by wheeled vehicles.
#[allow(clippy::too_many_arguments)]
fn move_towards_position_wheels(
    obj: &mut Object,
    loco: &LocomotorParams,
    goal: &Coord3D,
    on_path_dist: f32,
    mut desired_speed: f32,
    pathfinder: &crate::pathfind::Pathfinder,
    frame: u32,
) {
    let actual = obj.physics.as_ref().map(|p| p.forward_speed_2d(obj.angle)).unwrap_or(0.0);
    let max_turn_rate = loco.max_turn_rate;
    let mut turn_speed = loco.min_turn_speed;
    if turn_speed < loco.max_speed / 4.0 {
        turn_speed = loco.max_speed / 4.0;
    }
    let angle = obj.angle;
    let mut desired_angle = atan2(goal.y - obj.position.y, goal.x - obj.position.x);
    let mut rel_angle = angle_diff(desired_angle, angle);
    let major_radius = obj.template.geometry.major_radius;
    let (mut moving_backwards, mut doing_3pt) = obj
        .physics
        .as_ref()
        .map(|p| (p.moving_backwards, p.doing_three_point_turn))
        .unwrap_or((false, false));

    let mut move_backwards = false;
    if actual == 0.0 {
        moving_backwards = false;
        if loco.can_move_backward && rel_angle.abs() > PI / 2.0 {
            moving_backwards = true;
            doing_3pt = on_path_dist > 5.0 * major_radius;
        }
    }
    if moving_backwards {
        if rel_angle.abs() < PI / 2.0 {
            move_backwards = false;
            moving_backwards = false;
        } else {
            move_backwards = true;
            doing_3pt = on_path_dist > 5.0 * major_radius;
            let do3pt = doing_3pt;
            if !do3pt {
                desired_angle = angle_diff(desired_angle, PI);
                rel_angle = angle_diff(desired_angle, angle);
            }
        }
    }
    if let Some(p) = obj.physics.as_mut() {
        p.moving_backwards = moving_backwards;
        p.doing_three_point_turn = doing_3pt;
    }

    const SMALL_TURN: f32 = PI / 20.0;
    if rel_angle.abs() > SMALL_TURN && desired_speed > turn_speed {
        desired_speed = turn_speed;
    }

    let mut goal_speed = desired_speed;
    // The engine negates `actualSpeed` while reversing before the slow-down maths.
    let actual_speed = if move_backwards { -actual } else { actual };
    let slow_down_time = actual_speed / loco.braking + 1.0;
    let slow_down_dist = (actual_speed / 1.5) * slow_down_time + actual_speed;
    let effective = slow_down_dist.max(CELL_F);

    // `Locomotor.cpp:1370-1432`: turning more than 15 degrees, project half a second ahead along the
    // intended turn. If the terrain at the halfway point or at the end is not a surface this
    // locomotor can move on, turn in place toward the goal (no drive) and stay "driven" instead of
    // walking into the impassable patch.
    const FIFTEEN_DEGREES: f32 = PI / 12.0;
    let project_frames = crate::math::LOGICFRAMES_PER_SECOND as f32 / 2.0;
    if rel_angle.abs() > FIFTEEN_DEGREES {
        let average_speed = (goal_speed + actual_speed) / 2.0;
        let distance = project_frames * average_speed;
        let mut turn_factor = average_speed / turn_speed;
        if turn_factor > 1.0 {
            turn_factor = 1.0;
        }
        let turn_amount = project_frames * turn_factor * max_turn_rate / 4.0;
        let target_angle = if rel_angle < 0.0 { angle - turn_amount } else { angle + turn_amount };
        let offset = (cos(target_angle) * distance, sin(target_angle) * distance);
        let half = Coord3D::new(obj.position.x + offset.0 / 2.0, obj.position.y + offset.1 / 2.0, obj.position.z);
        let next = Coord3D::new(obj.position.x + offset.0, obj.position.y + offset.1, obj.position.z);
        let surfaces = crate::pathfind::PathObject::surfaces_for(loco);
        if !valid_movement_terrain(pathfinder, surfaces, &half) || !valid_movement_terrain(pathfinder, surfaces, &next) {
            rotate_towards_position(obj, goal, max_turn_rate, loco.turn_pivot_offset);
            // `physics->applyMotiveForce(&force)` with a zero force (`:1425-1428`): the top-level
            // zero motive force of `move_towards_position_force` already marked the object driven
            // this frame, so only the drive is skipped here.
            return;
        }
    }

    // `Locomotor.cpp:1414-1451`. The start test sets `m_brakingFactor = 1.1` (`:1417`); the far test
    // clears only the flag; the donut timer can set it again; then, while braking, the factor is
    // recomputed, squared, capped at `MAX_BRAKING_FACTOR` and finally forced back to 1.0 (`:1436-1441`),
    // so a wheeled unit brakes with the plain rate. The field is still written in the engine's order.
    let no_slow_down = obj.loco_rt.no_slow_down_as_approaching_dest;
    if let Some(p) = obj.physics.as_mut() {
        if on_path_dist < effective && !p.braking && !no_slow_down {
            p.braking = true;
            p.braking_factor = 1.1;
        }
        if on_path_dist > CELL_F && on_path_dist > 2.0 * slow_down_dist {
            p.braking = false;
        }
        // `Locomotor.cpp:1426-1432`: circling a nearby goal for longer than `DONUT_TIME_DELAY_SECONDS`
        // starts braking, so the `wasBraking` step can walk the unit in.
        if on_path_dist > DONUT_DISTANCE {
            p.donut_timer = frame + DONUT_TIME_DELAY_FRAMES;
        } else if p.donut_timer < frame {
            p.braking = true;
        }
        if p.braking {
            let mut factor = slow_down_dist / on_path_dist;
            factor *= factor;
            if factor > MAX_BRAKING_FACTOR {
                factor = MAX_BRAKING_FACTOR;
            }
            // `m_brakingFactor = slowDownDist/onPathDistToGoal`, squared, capped (`:1436-1440`) and then
            // overwritten with 1.0 (`:1441`): the engine's sequence, kept as written.
            p.braking_factor = factor;
            p.braking_factor = 1.0;
            if slow_down_dist > on_path_dist {
                goal_speed = actual_speed - loco.braking;
                if goal_speed < 0.0 {
                    goal_speed = 0.0;
                }
            } else if slow_down_dist > on_path_dist * 0.75 {
                goal_speed = actual_speed - loco.braking / 2.0;
                if goal_speed < 0.0 {
                    goal_speed = 0.0;
                }
            } else {
                goal_speed = actual_speed;
            }
        }
    }
    let braking_factor = obj.physics.as_ref().map_or(1.0, |p| p.braking_factor);

    // Wheeled vehicles can only turn while moving.
    let turn_factor = (actual / turn_speed).abs().min(1.0);
    let turn_amount = turn_factor * max_turn_rate;
    let target = if move_backwards && !doing_3pt {
        Coord3D::new(obj.position.x - (goal.x - obj.position.x), obj.position.y - (goal.y - obj.position.y), goal.z)
    } else {
        *goal
    };
    // `rotateObjAroundLocoPivot(obj, goalPos, turnAmount)` (Locomotor.cpp wheels, the pivot applies here too).
    rotate_towards_position(obj, &target, turn_amount, loco.turn_pivot_offset);

    let mut speed_delta = goal_speed - actual_speed;
    if move_backwards {
        speed_delta = -goal_speed + actual_speed;
    }
    if speed_delta != 0.0 {
        let mass = obj_mass(obj);
        let acceleration = if move_backwards {
            if speed_delta < 0.0 { -loco.acceleration } else { braking_factor * loco.braking }
        } else if speed_delta > 0.0 {
            loco.acceleration
        } else {
            -braking_factor * loco.braking
        };
        let mut accel_force = mass * acceleration;
        let max_force_needed = mass * speed_delta;
        if accel_force.abs() > max_force_needed.abs() {
            accel_force = max_force_needed;
        }
        let (dx, dy) = (cos(obj.angle), sin(obj.angle));
        apply_motive_force_obj(obj, Coord3D::new(accel_force * dx, accel_force * dy, 0.0), frame);
    }
}

/// `moveTowardsPositionClimb` (`Locomotor.cpp:1711-1846`): infantry climbing. The `CLIMBING` flag is a
/// `Locomotor` member that persists between frames — set when the goal is more than one pathfind cell
/// above or below (`dz*dz > sqr(PATHFIND_CELL_SIZE_F)`, `:1732-1734`), cleared when it is less than a
/// unit away in height (`:1735-1737`). Only while climbing does the kernel look one unit ahead along
/// the goal direction to the terrain height there: a step down of more than 0.1 unit makes it back up
/// (`moveBackwards`, `:1743-1753`), and a slope steeper than 1:1 slows the desired speed by
/// `slope*4` (`:1756-1761`). The desired angle is reversed while backing up (`:1773-1776`) and the
/// actual speed is negated (`:1792-1794`), so the acceleration picks the backward branch
/// (`:1814-1816`).
#[allow(clippy::too_many_arguments)]
fn move_towards_position_climb(
    obj: &mut Object,
    loco: &LocomotorParams,
    goal: &Coord3D,
    on_path_dist: f32,
    desired_speed: f32,
    terrain: &TerrainLogic,
    pathfinder: &crate::pathfind::Pathfinder,
    frame: u32,
    gravity: f32,
) {
    // `getMaxSpeedForCondition(damageState)` (`:1716-1718`): `loco` already carries the body's damage
    // state ([`apply_body_damage_state`]), so `max_speed` is the damage-state maximum.
    let mut desired_speed = desired_speed.min(loco.max_speed);
    let pos = obj.position;
    let dz = pos.z - goal.z;
    let mut climbing = obj.loco_rt.climbing;
    if dz * dz > CELL_F * CELL_F {
        climbing = true;
    }
    if dz.abs() < 1.0 {
        climbing = false;
    }
    let mut move_backwards = false;
    if climbing {
        // `Coord3D delta = goalPos; delta.x -= pos.x; delta.y -= pos.y; delta.z = 0;
        // delta.normalize(); delta.x += pos.x; delta.y += pos.y;` (`:1743-1749`): the point one unit
        // from the object toward the goal. `delta.z = getGroundHeight(delta.x, delta.y)` (`:1750`).
        let (mut dx, mut dy) = (goal.x - pos.x, goal.y - pos.y);
        let len = sqrt(dx * dx + dy * dy);
        if len > 0.0 {
            dx /= len;
            dy /= len;
        }
        let ahead_z = terrain.ground_height(pos.x + dx, pos.y + dy);
        // `if (delta.z < pos.z-0.1) moveBackwards = true;` (`:1752-1754`).
        if ahead_z < pos.z - 0.1 {
            move_backwards = true;
        }
        // `Real groundSlope = fabs(delta.z - pos.z); if (groundSlope<1.0f) groundSlope = 1.0f;
        // if (groundSlope>1.0f) desiredSpeed /= groundSlope*4;` (`:1756-1761`).
        let ground_slope = (ahead_z - pos.z).abs().max(1.0);
        if ground_slope > 1.0 {
            desired_speed /= ground_slope * 4.0;
        }
    }
    obj.loco_rt.climbing = climbing;
    // `setFlag(MOVING_BACKWARDS, moveBackwards)` (`:1762`): written every frame, unlike the wheels
    // kernel's persistent state.
    if let Some(p) = obj.physics.as_mut() {
        p.moving_backwards = move_backwards;
    }
    // `desiredAngle = atan2(goalPos.y - pos.y, goalPos.x - pos.x)` and `stdAngleDiff(desiredAngle, PI)`
    // while backing up (`:1768-1776`).
    let mut desired_angle = atan2(goal.y - obj.position.y, goal.x - obj.position.x);
    if move_backwards {
        desired_angle = angle_diff(desired_angle, PI);
    }
    let rel_angle = angle_diff(desired_angle, obj.angle);
    // `locoUpdate_moveTowardsAngle(obj, desiredAngle)` (`:1778`), as for legs.
    move_towards_angle_force(obj, loco, desired_angle, terrain, pathfinder, frame, gravity);
    let angle_coeff = (rel_angle.abs() / (PI / 4.0)).min(1.0);
    let mut goal_speed = (1.0 - angle_coeff) * desired_speed;
    let actual = obj.physics.as_ref().map(|p| p.forward_speed_2d(obj.angle)).unwrap_or(0.0);
    // `if (moveBackwards) actualSpeed = -actualSpeed;` (`:1792-1794`).
    let actual_speed = if move_backwards { -actual } else { actual };
    // `Real slowDownDist = calcSlowDownDist(actualSpeed, m_template->m_minSpeed, getBraking()); if
    // (onPathDistToGoal < slowDownDist && !getFlag(NO_SLOW_DOWN_AS_APPROACHING_DEST)) goalSpeed =
    // m_template->m_minSpeed;` (`:1797-1801`).
    let slow = calc_slow_down_dist(actual_speed, loco.min_speed, loco.braking);
    if on_path_dist < slow && !obj.loco_rt.no_slow_down_as_approaching_dest {
        goal_speed = loco.min_speed;
    }
    // The speed-delta tail (`:1806-1846`) is inline in the engine: forward acceleration is
    // `maxAcceleration`, backward acceleration is `-maxAcceleration`, and braking is `getBraking()`
    // (no `m_brakingFactor`, which the legs/treads tail applies).
    let mut speed_delta = goal_speed - actual_speed;
    if move_backwards {
        speed_delta = -goal_speed + actual_speed;
    }
    if speed_delta != 0.0 {
        let mass = obj_mass(obj);
        let acceleration = if move_backwards {
            if speed_delta < 0.0 {
                -loco.acceleration
            } else {
                loco.braking
            }
        } else if speed_delta > 0.0 {
            loco.acceleration
        } else {
            -loco.braking
        };
        let mut accel_force = mass * acceleration;
        let max_force_needed = mass * speed_delta;
        if accel_force.abs() > max_force_needed.abs() {
            accel_force = max_force_needed;
        }
        let (dx, dy) = (cos(obj.angle), sin(obj.angle));
        apply_motive_force_obj(obj, Coord3D::new(accel_force * dx, accel_force * dy, 0.0), frame);
    }
}

/// `moveTowardsPositionOther` (`Locomotor.cpp:2347`).
fn move_towards_position_other(obj: &mut Object, loco: &LocomotorParams, goal: &Coord3D, on_path_dist: f32, desired_speed: f32, frame: u32) {
    let actual = obj.physics.as_ref().map(|p| p.forward_speed_2d(obj.angle)).unwrap_or(0.0);
    let mut goal_speed = desired_speed;
    // `Coord3D dirToApplyForce = *obj->getUnitDirectionVector2D();` (`Locomotor.cpp:2372`): the direction
    // is read into a local *before* `rotateTowardsPosition` (`:2389`) turns the body, and the motive
    // force at the end of the kernel uses that copy (`:2422-2426`), so this frame's acceleration still
    // points along the old heading. Every other kernel reads `getUnitDirectionVector2D()` after its
    // turn (legs `:1698`, treads/wheels after `rotateTowardsPosition`), which is why only the
    // hover/wings/other appearance (the Chinook) sees the one-frame lag.
    let dir;
    if loco.ultra_accurate
        && (goal.y - obj.position.y).abs() <= goal_speed * loco.slide_into_place_factor
        && (goal.x - obj.position.x).abs() <= goal_speed * loco.slide_into_place_factor
    {
        // "don't turn, just slide in the right direction" (`:2375-2385`): the force points at the goal.
        if let Some(p) = obj.physics.as_mut() {
            p.turning = 0;
        }
        let (dx, dy) = (goal.x - obj.position.x, goal.y - obj.position.y);
        let len = sqrt(dx * dx + dy * dy);
        dir = if len > 0.0 { (dx / len, dy / len) } else { (0.0, 0.0) };
    } else {
        dir = (cos(obj.angle), sin(obj.angle));
        rotate_towards_position(obj, goal, loco.max_turn_rate, loco.turn_pivot_offset);
    }
    let slow = calc_slow_down_dist(actual, loco.min_speed, loco.braking);
    if on_path_dist < slow && !obj.loco_rt.no_slow_down_as_approaching_dest {
        goal_speed = loco.min_speed;
    }
    let speed_delta = goal_speed - actual;
    if speed_delta != 0.0 {
        let mass = obj_mass(obj);
        let acceleration = if speed_delta > 0.0 { loco.acceleration } else { -loco.braking };
        let mut accel_force = mass * acceleration;
        let max_force_needed = mass * speed_delta;
        if accel_force.abs() > max_force_needed.abs() {
            accel_force = max_force_needed;
        }
        apply_motive_force_obj(obj, Coord3D::new(accel_force * dir.0, accel_force * dir.1, 0.0), frame);
    }
}

/// Is this locomotor kind driven by motive forces in the sim? The engine applies a motive force
/// for every appearance (`moveTowardsPositionLegs`/`Wheels`/`Treads`/`Hover`/`Wings`/`Thrust` all
/// call `PhysicsBehavior::applyMotiveForce`), so all of them are.
pub fn is_force_driven(loco: &LocomotorParams) -> bool {
    matches!(
        loco.kind,
        LocomotorKind::Legs
            | LocomotorKind::Wheels
            | LocomotorKind::Treads
            | LocomotorKind::Climber
            | LocomotorKind::Hover
            | LocomotorKind::Wings
            | LocomotorKind::Thrust
    )
}

/// `Locomotor::calcLiftToUseAtPt` (`Locomotor.cpp:2043`): the vertical acceleration needed to
/// reach `preferred_height` at rest, given the current z velocity. `SurfaceRelativeHeight` and
/// `AbsoluteHeight` apply it as `mass * lift` (`handleBehaviorZ`).
fn calc_lift_to_use_at_pt(obj: &Object, loco: &LocomotorParams, cur_z: f32, preferred_height: f32, gravity: f32) -> f32 {
    let max_gross_lift = loco.lift;
    let max_net_lift = (max_gross_lift + gravity).max(0.0);
    let cur_vel_z = obj.physics.as_ref().map_or(0.0, |p| p.velocity.z);
    // Going down, braking is limited by net lift; going up, by gravity.
    let max_accel = if loco.ultra_accurate {
        if cur_vel_z < 0.0 { 2.0 * max_net_lift } else { -2.0 * max_net_lift }
    } else if cur_vel_z < 0.0 {
        max_net_lift
    } else {
        gravity
    };
    const TINY_ACCEL: f32 = 0.001;
    let desired_accel;
    if max_accel.abs() > TINY_ACCEL {
        let delta_z = preferred_height - cur_z;
        let brake_dist = cur_vel_z * cur_vel_z / max_accel.abs();
        if brake_dist.abs() > delta_z.abs() {
            desired_accel = max_accel;
        } else if cur_vel_z.abs() > loco.speed_limit_z {
            desired_accel = loco.speed_limit_z - cur_vel_z;
        } else {
            desired_accel = 2.0 * (delta_z - cur_vel_z);
        }
    } else {
        desired_accel = 0.0;
    }
    let mut lift = desired_accel - gravity;
    if loco.ultra_accurate {
        const UP_FACTOR: f32 = 3.0;
        if lift > UP_FACTOR * max_gross_lift {
            lift = UP_FACTOR * max_gross_lift;
        } else if lift < -max_gross_lift {
            lift = -max_gross_lift;
        }
    } else {
        lift = lift.clamp(0.0, max_gross_lift);
    }
    lift
}

/// `Locomotor::handleBehaviorZ` (`Locomotor.cpp:2217`) for a force-driven object. The
/// fixed-height flavours set z directly; `SURFACE_RELATIVE_HEIGHT`/`ABSOLUTE_HEIGHT` apply a lift
/// force (the reduced sim has no separate `RELATIVE_TO_HIGHEST_LAYER`/building layers, so those
/// fall back to the surface). Returns `requiresConstantCalling` (`Locomotor.cpp:2220-2239`): FALSE
/// only for `Z_NO_Z_MOTIVE_FORCE`; every other flavour must be called every frame.
fn handle_behavior_z_force(
    obj: &mut Object,
    loco: &LocomotorParams,
    terrain: &TerrainLogic,
    frame: u32,
    gravity: f32,
    goal_z: f32,
) -> bool {
    let pos = obj.position;
    let surface = surface_height(terrain, pos.x, pos.y, pos.z);
    // `PRECISE_Z_POS` (`setUsePreciseZPos`): fly to the goal's own z instead of the preferred height.
    let precise = loco.precise_z;
    let apply_lift = |obj: &mut Object, preferred: f32| {
        let preferred = if precise { goal_z } else { preferred };
        let delta = (preferred - obj.position.z) * loco.preferred_height_damping;
        let preferred = obj.position.z + delta;
        let lift = calc_lift_to_use_at_pt(obj, loco, obj.position.z, preferred, gravity);
        if lift != 0.0 {
            let mass = obj_mass(obj);
            apply_motive_force_obj(obj, Coord3D::new(0.0, 0.0, lift * mass), frame);
        }
    };
    let has_z_goal = loco.preferred_height != 0.0 || precise;
    match loco.behavior_z {
        ZBehavior::NoZMotiveForce => return false,
        ZBehavior::AbsoluteHeight => {
            if has_z_goal {
                apply_lift(obj, loco.preferred_height);
            }
        }
        ZBehavior::SurfaceRelativeHeight | ZBehavior::RelativeToHighestLayer => {
            if has_z_goal {
                apply_lift(obj, loco.preferred_height + surface);
            }
        }
        ZBehavior::FixedSurfaceRelativeHeight | ZBehavior::FixedRelativeToGroundAndBuildings => {
            let mut p = obj.position;
            p.z = loco.preferred_height + surface;
            obj.set_position(p);
        }
        ZBehavior::FixedAbsoluteHeight => {
            let mut p = obj.position;
            p.z = loco.preferred_height;
            obj.set_position(p);
        }
    }
    true
}

/// `Locomotor::calcMinTurnRadius` (`Locomotor.cpp`): `minSpeed / maxTurnRate`, `BIGNUM` when the
/// unit cannot turn.
fn min_turn_radius(loco: &LocomotorParams) -> f32 {
    if loco.max_turn_rate > 0.0 {
        loco.min_speed / loco.max_turn_rate
    } else {
        999999.0
    }
}

/// `Locomotor::maintainCurrentPositionWings` (`Locomotor.cpp:2509`): fixed-wing aircraft cannot
/// stop, so they circle a point projected `CirclingRadius` from their maintain position.
fn maintain_current_position_wings(
    obj: &mut Object,
    loco: &LocomotorParams,
    terrain: &TerrainLogic,
    frame: u32,
    gravity: f32,
) {
    if let Some(p) = obj.physics.as_mut() {
        p.turning = 0;
    }
    let motive = obj.physics.as_ref().is_some_and(|p| p.is_motive(frame));
    let ground = terrain.surface_height(obj.position.x, obj.position.y, obj.position.z);
    let above_terrain = obj.position.z > ground;
    if !motive || !above_terrain {
        return;
    }
    let maintain = obj.ai.maintain_pos.get_or_insert(obj.position);
    let maintain = *maintain;
    let mut turn_radius = loco.circling_radius;
    if turn_radius == 0.0 {
        turn_radius = min_turn_radius(loco);
    }
    let dx = maintain.x - obj.position.x;
    let dy = maintain.y - obj.position.y;
    let angle_toward = if dx.abs() < 0.001 && dy.abs() < 0.001 { obj.angle } else { atan2(dy, dx) };
    let mut aim = PI - PI / 8.0;
    let radius = if turn_radius < 0.0 {
        aim = -aim;
        -turn_radius
    } else {
        turn_radius
    };
    let angle = angle_toward + aim;
    let desired = Coord3D::new(maintain.x + cos(angle) * radius, maintain.y + sin(angle) * radius, maintain.z);
    move_towards_position_other(obj, loco, &desired, 0.0, loco.min_speed, frame);
    handle_behavior_z_force(obj, loco, terrain, frame, gravity, maintain.z);
}

/// `Locomotor::locoUpdate_moveTowardsPosition` (`Locomotor.cpp:950`): returns `true`
/// once the object has reached `goal`. This is the engine's flow: apply the motive force, then (when
/// braking) directly step the position toward the goal and record the `OBJECT_STATUS_BRAKING` flag.
#[allow(clippy::too_many_arguments)]
pub fn move_towards_position_force(
    obj: &mut Object,
    loco: &LocomotorParams,
    goal: &Coord3D,
    terrain: &TerrainLogic,
    pathfinder: &crate::pathfind::Pathfinder,
    frame: u32,
    desired_speed: f32,
    gravity: f32,
    on_path_dist: Option<f32>,
) -> bool {
    move_towards_position_force_blocked(obj, loco, goal, terrain, pathfinder, frame, desired_speed, gravity, on_path_dist, false)
}

/// [`move_towards_position_force`] with the caller's `blocked` flag, for callers that do not read the flag
/// back (`locoUpdate_moveTowardsPosition(..., &blocked)`, `Locomotor.cpp:1040-1058`).
#[allow(clippy::too_many_arguments)]
pub fn move_towards_position_force_blocked(
    obj: &mut Object,
    loco: &LocomotorParams,
    goal: &Coord3D,
    terrain: &TerrainLogic,
    pathfinder: &crate::pathfind::Pathfinder,
    frame: u32,
    desired_speed: f32,
    gravity: f32,
    on_path_dist: Option<f32>,
    blocked: bool,
) -> bool {
    let mut blocked = blocked;
    move_towards_position_force_blocked_out(obj, loco, goal, terrain, pathfinder, frame, desired_speed, gravity, on_path_dist, &mut blocked)
}

/// `Locomotor::locoUpdate_moveTowardsPosition(obj, goalPos, onPathDistToGoal, desiredSpeed, Bool *blocked)`
/// (`Locomotor.cpp:950`) with the caller's `blocked` flag read and written back (`AIUpdate.cpp:2147`, `:2240`,
/// `:2257`). On return `*blocked` is the kernel's output: the blocked branch sets it to `TURN_NONE != turn`
/// (`Locomotor.cpp:1056`), the other paths leave it FALSE, and an early return (no physics) leaves it as it was.
/// The caller reads it for the blocked-frame clamp (`AIUpdate.cpp:2290`).
#[allow(clippy::too_many_arguments)]
pub fn move_towards_position_force_blocked_out(
    obj: &mut Object,
    loco: &LocomotorParams,
    goal: &Coord3D,
    terrain: &TerrainLogic,
    pathfinder: &crate::pathfind::Pathfinder,
    frame: u32,
    desired_speed: f32,
    gravity: f32,
    on_path_dist: Option<f32>,
    blocked: &mut bool,
) -> bool {
    if obj.physics.is_none() {
        return false;
    }
    // `locoUpdate_moveTowardsPosition` clears `MAINTAIN_POS_IS_VALID` (`Locomotor.cpp:953`).
    obj.ai.maintain_pos = None;
    let max_speed = loco.max_speed;
    let desired_speed = desired_speed.min(max_speed);
    let dx = goal.x - obj.position.x;
    let dy = goal.y - obj.position.y;
    let dist = sqrt(dx * dx + dy * dy);
    // `onPathDistToGoal` (`AIUpdate.cpp:2214`): the distance along the path, when the AI computed a
    // look-ahead goal; otherwise the straight distance to the goal.
    let mut path_dist = on_path_dist.unwrap_or(dist);
    // `Locomotor.cpp:962-967`: far enough out that stopping from max speed would not overshoot, the
    // unit is not braking (the flag, not the `OBJECT_STATUS_BRAKING` bit the physics reads) and the
    // persistent braking factor is reset to 1.0 (`m_brakingFactor = 1.0f`, `:966`).
    let dist_to_stop_at_max_speed = (max_speed / loco.braking) * max_speed / 2.0;
    if path_dist > CELL_F && path_dist > dist_to_stop_at_max_speed {
        if let Some(p) = obj.physics.as_mut() {
            p.braking = false;
            p.braking_factor = 1.0;
        }
    }
    // `if(physics->getIsStunned()) return;` (`Locomotor.cpp:976-978`), after the far-goal braking reset above and before
    // the actual distance is taken. Lane 165: the port had no such return. The blocked flag is not written here.
    if obj.physics.as_ref().is_some_and(|p| p.is_stunned) {
        return false;
    }
    // `locoUpdate_moveTowardsPosition` (`Locomotor.cpp:1006-1013`): "If the actual distance is farther,
    // then use the actual distance so we get there" (and a goal far past the path distance, e.g. an
    // explicit goal with `onPathDistToGoal = 0`, switches braking on).
    if dist > path_dist {
        if !obj.is_kind_of(crate::template::kind_of::PROJECTILE) && dist > 2.0 * path_dist {
            if let Some(p) = obj.physics.as_mut() {
                p.braking = true;
            }
        }
        path_dist = dist;
    }
    // `if (appearance == LOCO_WINGS) setFlag(IS_BRAKING, false)` (`Locomotor.cpp:1064-1071`): wings
    // cannot stop, so they never brake.
    if loco.kind == LocomotorKind::Wings {
        if let Some(p) = obj.physics.as_mut() {
            p.braking = false;
        }
    }
    // `Bool wasBraking = obj->getStatusBits().test(OBJECT_STATUS_BRAKING)` (`Locomotor.cpp:1073`): the
    // *status* bit, which `locoUpdate_moveTowardsPosition` sets at its end (`:1111`), so the top-level
    // clear above does not change this frame's `wasBraking` step.
    let was_braking = obj.status_bits & status::IS_BRAKING != 0;

    // The engine's donut timer lives in the wheels kernel only (`Locomotor.cpp:1426-1432`, ported
    // there); the other kernels have no donut brake.

    // `locoUpdate_moveTowardsPosition` (`Locomotor.cpp:1034-1035`): `applyMotiveForce(&nullAccel)`
    // marks the object as driven for `MOTIVE_FRAMES` whatever the kernel does. The kernel only runs
    // when `AllowAirborneMotiveForce` is set or the object is not treated as airborne
    // (`heightAboveSurface > -(3*3)*gravity`, `Locomotor.cpp:1026`; gate at `:1076`).
    apply_motive_force_obj(obj, Coord3D::ZERO, frame);
    // `getLayerHeight(obj->getLayer())` in the engine: the layer the object is on (a bridge deck
    // counts), so the ground-only `getSurfaceHtAtPt` is not used here.
    let layer_height = terrain.surface_height(obj.position.x, obj.position.y, obj.position.z);
    let height_above_surface = obj.position.z - layer_height;
    let treat_as_airborne = height_above_surface > -(3.0 * 3.0) * gravity;
    // `if (*blocked)` (`Locomotor.cpp:1040-1058`): a blocked unit scrubs its velocity to the speed the blocker
    // allows and turns in place, with no motive force this frame. A desired speed above the unit's own speed,
    // or a flier over a blocker, first clears the flag (`:1040-1049`).
    if *blocked {
        if obj.physics.as_ref().is_some_and(|p| desired_speed > p.velocity_magnitude()) {
            *blocked = false;
        }
        if treat_as_airborne && loco.surfaces_air {
            *blocked = false;
        }
    }
    if *blocked {
        if let Some(p) = obj.physics.as_mut() {
            p.scrub_velocity_2d(desired_speed); // "stop if we are about to run into the blocking object." (`:1052`)
        }
        // `*blocked = (TURN_NONE != rotateObjAroundLocoPivot(obj, goalPos, turnRate))` (`:1054-1056`). The wander
        // case (`m_wanderWidthFactor != 0`, which skips the turn and keeps `*blocked` TRUE) is not ported: the
        // locomotor has no wander field.
        let (_, turn) = rotate_about_loco_pivot(obj, goal, loco.max_turn_rate, loco.turn_pivot_offset);
        *blocked = turn != 0;
        // `handleBehaviorZ(obj, physics, goalPos)` runs on every path of the blocked branch (`:1059-1061`:
        // "it is very important to be sure to call this in all situations"), whatever the locomotor's kind (R18-12).
        handle_behavior_z_force(obj, loco, terrain, frame, gravity, goal.z);
        return false;
    }
    let mut thrust_arrived = false;
    if loco.allow_motive_force_while_airborne || !treat_as_airborne {
        match loco.kind {
            LocomotorKind::Thrust => {
                thrust_arrived = move_towards_position_thrust(obj, loco, goal, dist, desired_speed, gravity, terrain, frame);
            }
            LocomotorKind::Treads => move_towards_position_treads(obj, loco, goal, path_dist, desired_speed, frame),
            LocomotorKind::Wheels => move_towards_position_wheels(obj, loco, goal, path_dist, desired_speed, pathfinder, frame),
            LocomotorKind::Climber => move_towards_position_climb(obj, loco, goal, path_dist, desired_speed, terrain, pathfinder, frame, gravity),
            LocomotorKind::Legs => move_towards_position_legs(obj, loco, goal, path_dist, desired_speed, terrain, pathfinder, frame, gravity),
            _ => move_towards_position_other(obj, loco, goal, path_dist, desired_speed, frame),
        }
    }

    // `handleBehaviorZ(obj, physics, goalPos)` (`Locomotor.cpp:1109`) runs after the kernel of every kind, before the
    // braking status and the `wasBraking` block (`:1111-1115`). R18-12: it was gated on hover and wings only.
    handle_behavior_z_force(obj, loco, terrain, frame, gravity, goal.z);
    // `obj->setStatus(MAKE_OBJECT_STATUS_MASK(OBJECT_STATUS_BRAKING), getFlag(IS_BRAKING))`
    // (`Locomotor.cpp:1111`): the physics (`PhysicsUpdate.cpp:664`) and the next frame's `wasBraking`
    // read the status bit; the kernels read the locomotor flag.
    if obj.physics.as_ref().is_some_and(|p| p.braking) {
        obj.status_bits |= status::IS_BRAKING;
    } else {
        obj.status_bits &= !status::IS_BRAKING;
    }

    // `locoUpdate_moveTowardsPosition`'s `wasBraking` branch: when braking, physics only adjusts z,
    // so the locomotor moves x/y directly toward the goal at the forward speed.
    if was_braking && dist > 0.001 {
        let forward = obj.physics.as_ref().map(|p| p.forward_speed_2d(obj.angle)).unwrap_or(0.0).abs();
        let vel = forward.max(MIN_VEL).min(dist);
        let inv = 1.0 / dist;
        let mut pos = obj.position;
        pos.x += dx * inv * vel;
        pos.y += dy * inv * vel;
        obj.set_position(pos);
    }

    // THRUST reports its own arrival (within the close-enough radius); the engine's AI owns the snap.
    if loco.kind == LocomotorKind::Thrust {
        return thrust_arrived;
    }
    // Arrived: within a small radius and slow.
    let dx = goal.x - obj.position.x;
    let dy = goal.y - obj.position.y;
    let dist2 = sqrt(dx * dx + dy * dy);
    let speed = obj.physics.as_ref().map(|p| p.speed_2d()).unwrap_or(0.0);
    // Wings "arrive" when they pass close enough; they never slow to a stop.
    if loco.kind == LocomotorKind::Wings {
        return dist2 <= loco.close_enough_dist.max(2.0);
    }
    let arrive = loco.close_enough_dist.max(1.0);
    // Only the port's path bookkeeping reads this. `locoUpdate_moveTowardsPosition` has no arrival step: it never snaps
    // the unit, scrubs its velocity or clears `IS_BRAKING` here (that flag clears on a far goal, `Locomotor.cpp:963-967`,
    // or in `locoUpdate_maintainCurrentPosition`, `:2442`), so a braking unit stays pinned on its goal.
    dist2 <= arrive && speed <= loco.braking.max(0.05)
}

/// `Locomotor::locoUpdate_maintainCurrentPosition` for a force-driven ground object. Legs, climbers,
/// wheels and treads stop dead (`scrubVelocity2D(0)`); hover brakes with a motive force; thrust and
/// wings cannot stop and keep moving. Returns `requiresConstantCalling` (`Locomotor.cpp:2445`): true
/// for the appearances that must be called every frame.
pub fn maintain_current_position_force(
    obj: &mut Object,
    loco: &LocomotorParams,
    frame: u32,
    gravity: f32,
    terrain: &TerrainLogic,
) -> bool {
    // `locoUpdate_maintainCurrentPosition` (`Locomotor.cpp:2433-2442`): latch `m_maintainPos` the first
    // time (until a move clears it, `:953`), reset the donut timer (`:2441`, before the physics null
    // check and before the thrust/wings returns) and clear `IS_BRAKING` (`:2442`), for every appearance.
    if obj.ai.maintain_pos.is_none() {
        obj.ai.maintain_pos = Some(obj.position);
    }
    if let Some(p) = obj.physics.as_mut() {
        p.donut_timer = frame + DONUT_TIME_DELAY_FRAMES;
        p.braking = false;
    }
    let maintain = obj.ai.maintain_pos.unwrap_or(obj.position);
    if obj.physics.is_none() {
        // `if (physics == nullptr) { DEBUG_CRASH(...); return TRUE; }` (`Locomotor.cpp:2443-2447`).
        return true;
    }
    // `Bool requiresConstantCalling = TRUE;` (`Locomotor.cpp:2452`), then the switch
    // (`:2453-2503`): THRUST/HOVER/WINGS/OTHER report TRUE, two legs / climber / wheels /
    // motorcycle / treads take `maintainCurrentPositionOther` and report FALSE.
    let mut requires = true;
    match loco.kind {
        // `maintainCurrentPositionThrust` (`Locomotor.cpp:2525`): thrust cannot stop; it keeps
        // thrusting toward the latched maintain position at `getMinSpeed()`. The goal is the latched
        // point, not the current one: a THRUST locomotor has no lift and no Z behaviour
        // (`Locomotor.cpp:400-410` throws for any other Z behaviour), so the altitude is held only
        // while the goal is above the object.
        LocomotorKind::Thrust => {
            move_towards_position_thrust(obj, loco, &maintain, 0.0, loco.min_speed, gravity, terrain, frame);
        }
        // `maintainCurrentPositionWings` (`Locomotor.cpp:2533`): fixed-wing aircraft circle instead
        // of stopping.
        LocomotorKind::Wings => maintain_current_position_wings(obj, loco, terrain, frame, gravity),
        LocomotorKind::Hover => {
            // `maintainCurrentPositionHover` (`Locomotor.cpp:2572-2611`).
            let motive = obj.physics.as_ref().is_some_and(|p| p.is_motive(frame));
            if let Some(p) = obj.physics.as_mut() {
                p.turning = 0; // `physics->setTurning(TURN_NONE)` (`:2574`).
            }
            if motive {
                let actual = obj.physics.as_ref().map_or(0.0, |p| p.forward_speed_2d(obj.angle));
                let mass = obj_mass(obj);
                // `Real minSpeed = max(1.0E-10f, m_template->m_minSpeed)` (`:2585`).
                let min_speed = loco.min_speed.max(1.0e-10);
                let speed_delta = min_speed - actual;
                if speed_delta.abs() > min_speed {
                    // The engine's `maxAcceleration` is `getMaxAcceleration(bdt)` (`:2580`), the
                    // damage-state variant of `m_acceleration`; `loco.acceleration` carries that state.
                    let acceleration = if speed_delta > 0.0 { loco.acceleration } else { -loco.braking };
                    let mut accel_force = mass * acceleration;
                    let max_force_needed = mass * speed_delta;
                    if accel_force.abs() > max_force_needed.abs() {
                        accel_force = max_force_needed;
                    }
                    let (dx, dy) = (cos(obj.angle), sin(obj.angle));
                    apply_motive_force_obj(obj, Coord3D::new(accel_force * dx, accel_force * dy, 0.0), frame);
                }
            }
        }
        // Two legs, climber, wheels, motorcycle and treads all take `maintainCurrentPositionOther`
        // (`Locomotor.h:382-384`): `physics->setTurning(TURN_NONE); if (physics->isMotive())
        // physics->scrubVelocity2D(0)` (`Locomotor.cpp:2616-2626`). The unit stops dead on the frame
        // its move ends; it does not coast.
        _ => {
            requires = false;
            let motive = obj.physics.as_ref().is_some_and(|p| p.is_motive(frame));
            if let Some(p) = obj.physics.as_mut() {
                p.turning = 0; // `physics->setTurning(TURN_NONE)` (`:2619`).
                if motive {
                    p.scrub_velocity_2d(0.0);
                }
            }
        }
    }
    // "but we do need to do this even if not moving, for hovering/Thrusting things":
    // `if (handleBehaviorZ(obj, physics, m_maintainPos)) requiresConstantCalling = TRUE;`
    // (`Locomotor.cpp:2506-2508`) runs for **every** appearance, after the switch.
    let latched_z = obj.ai.maintain_pos.map_or(obj.position.z, |p| p.z);
    if handle_behavior_z_force(obj, loco, terrain, frame, gravity, latched_z)
    {
        requires = true;
    }
    requires
}

/// `Locomotor::locoUpdate_moveTowardsAngle` (`Locomotor.cpp:834`) for a force-driven object: turn
/// toward `goal_angle`; a locomotor that cannot stand still (`MinSpeed > 0`) keeps flying toward a
/// point two steps ahead in that direction instead.
///
/// The legs and climb kernels call this (`Locomotor.cpp:1681`, `:1802`); every `TWO_LEGS`/`CLIMBER`
/// locomotor in `locomotor.ini` has `MinSpeed = 0`, so they take the rotation branch. (With
/// `MinSpeed > 0` the engine re-enters `locoUpdate_moveTowardsPosition`, which dispatches back into
/// the same kernel forever; the port follows the same call graph, and the data never reaches it.)
pub fn move_towards_angle_force(
    obj: &mut Object,
    loco: &LocomotorParams,
    goal_angle: f32,
    terrain: &TerrainLogic,
    pathfinder: &crate::pathfind::Pathfinder,
    frame: u32,
    gravity: f32,
) {
    obj.ai.maintain_pos = None;
    if obj.physics.is_none() {
        return;
    }
    if loco.min_speed > 0.0 {
        let desired = Coord3D::new(
            obj.position.x + cos(goal_angle) * loco.min_speed * 2.0,
            obj.position.y + sin(goal_angle) * loco.min_speed * 2.0,
            obj.position.z,
        );
        // "pass a huge num for "dist to goal", so that we don't think we're nearing our destination"
        move_towards_position_force(obj, loco, &desired, terrain, pathfinder, frame, loco.min_speed, gravity, Some(99999.0));
    } else {
        let desired = Coord3D::new(obj.position.x + cos(goal_angle) * 1000.0, obj.position.y + sin(goal_angle) * 1000.0, obj.position.z);
        // `rotateTowardsPosition(obj, desiredPos)` (`Locomotor.cpp:914-917`): through
        // `rotateObjAroundLocoPivot`, i.e. about `getTurnPivotOffset()` (`:2162-2166`).
        rotate_towards_position(obj, &desired, loco.max_turn_rate, loco.turn_pivot_offset);
        handle_behavior_z_force(obj, loco, terrain, frame, gravity, obj.position.z);
    }
}

// ---- THRUST 3D kernel (`moveTowardsPositionThrust`, Locomotor.cpp:1912) ------------------------

/// `isNearlyZero` (`Locomotor.cpp:95`).
fn nearly_zero(a: f32) -> bool {
    a.abs() < 0.001
}

/// The object's forward direction, `Matrix3D::Get_X_Vector` (`Locomotor.cpp:220`, `:1957`). The engine reads the
/// matrix, so pitch and roll come from the transform's own rotation rows, not from reconstructed Euler angles.
fn forward_vector(obj: &Object) -> Coord3D {
    obj.transform.x_vector()
}

fn v3_dot(a: Coord3D, b: Coord3D) -> f32 {
    a.x * b.x + a.y * b.y + a.z * b.z
}

fn v3_cross(a: Coord3D, b: Coord3D) -> Coord3D {
    Coord3D::new(a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x)
}

fn v3_normalize(v: Coord3D) -> Coord3D {
    let l = sqrt(v3_dot(v, v));
    if l > 0.0 {
        Coord3D::new(v.x / l, v.y / l, v.z / l)
    } else {
        v
    }
}

/// `tryToRotateVector3D` (`Locomotor.cpp:110`): rotate `cur_dir` toward `goal_dir` by at most
/// `max_angle` (negative = a fraction of the angle between them). Returns the actual direction and
/// the angle turned.
fn try_rotate_vector3d(max_angle: f32, cur_dir: Coord3D, goal_dir: Coord3D) -> (Coord3D, f32) {
    if nearly_zero(max_angle) {
        return (cur_dir, 0.0);
    }
    let cur = v3_normalize(cur_dir);
    let goal = v3_normalize(goal_dir);
    let cosine = v3_dot(cur, goal).clamp(-1.0, 1.0);
    let mut angle_between = acos(cosine);
    let mut max_angle = max_angle;
    if max_angle < 0.0 {
        max_angle = -max_angle * angle_between;
        if nearly_zero(max_angle) {
            return (cur_dir, 0.0);
        }
    }
    if angle_between.abs() <= max_angle {
        return (goal, angle_between);
    }
    // `nah, try as much as we can in the right dir`: rotate around the axis perpendicular to both,
    // `Matrix3D rotMtx(objCrossGoal, angleBetween); actualDir = rotMtx.Rotate_Vector(curDir);` (Locomotor.cpp:154-164).
    // The result is not renormalised, as in the engine.
    let axis = v3_normalize(v3_cross(cur, goal));
    angle_between = max_angle;
    let rot = Matrix3D::axis_angle(axis, angle_between);
    (rot.rotate_vector(cur), angle_between)
}

/// `tryToOrientInThisDirection3D` (`Locomotor.cpp:171`): yaw/pitch the object toward `dir` by at
/// most `max_turn_rate`. The sim keeps roll in `Physics::roll`.
fn try_orient_in_this_direction_3d(obj: &mut Object, max_turn_rate: f32, dir: Coord3D) -> f32 {
    // `tryToRotateVector3D(maxTurnRate, obj->getTransformMatrix()->Get_X_Vector(), desiredDir, actualDir)` (Locomotor.cpp:174).
    let current = obj.transform.x_vector();
    let (actual, rel) = try_rotate_vector3d(max_turn_rate, current, dir);
    if rel != 0.0 {
        // `newXform.buildTransformMatrix(objPos, actualDir); obj->setTransformMatrix(&newXform)` (Locomotor.cpp:177-182).
        let pos = obj.position;
        obj.set_transform_matrix(Matrix3D::build_transform(pos, actual));
        // The port keeps the pitch for its own readers; it is the matrix's pitch (`asin` of the direction's z).
        if let Some(p) = obj.physics.as_mut() {
            p.pitch = asin(actual.z.clamp(-1.0, 1.0));
        }
    }
    rel
}

/// `calcDirectionToApplyThrust` (`Locomotor.cpp:194`): the direction to thrust in to minimise the
/// angle between velocity and the direction to the goal, accounting for current velocity and
/// gravity.
fn calc_direction_to_apply_thrust(obj: &Object, goal: Coord3D, max_accel: f32, gravity: f32) -> Coord3D {
    let obj_pos = obj.position;
    let vec_to_goal = Coord3D::new(goal.x - obj_pos.x, goal.y - obj_pos.y, goal.z - obj_pos.z);
    if nearly_zero(v3_dot(vec_to_goal, vec_to_goal)) {
        return forward_vector(obj);
    }
    let mut cur_vel = obj.physics.as_ref().map_or(Coord3D::ZERO, |p| p.velocity);
    cur_vel.z += gravity;
    let dist_to_goal_sqr = v3_dot(vec_to_goal, vec_to_goal);
    let dist_to_goal = sqrt(dist_to_goal_sqr);
    let cur_vel_mag_sqr = v3_dot(cur_vel, cur_vel);
    let cur_vel_mag = sqrt(cur_vel_mag_sqr);
    let max_accel_sqr = max_accel * max_accel;
    let denom = cur_vel_mag_sqr - max_accel_sqr;
    if !nearly_zero(denom) {
        let mut t = (dist_to_goal * (cur_vel_mag + max_accel)) / denom;
        let t2 = (dist_to_goal * (cur_vel_mag - max_accel)) / denom;
        if t >= 0.0 || t2 >= 0.0 {
            if t < 0.0 || (t2 >= 0.0 && t2 < t) {
                t = t2;
            }
            if !nearly_zero(t) {
                return v3_normalize(Coord3D::new(
                    vec_to_goal.x / t - cur_vel.x,
                    vec_to_goal.y / t - cur_vel.y,
                    vec_to_goal.z / t - cur_vel.z,
                ));
            }
        }
    }
    v3_normalize(vec_to_goal)
}

/// `moveTowardsPositionThrust` (`Locomotor.cpp:1912`). Returns `true` when within the arrival
/// radius (the engine's AI state machine owns the final snap; the reduced sim AI needs the flag).
#[allow(clippy::too_many_arguments)]
fn move_towards_position_thrust(
    obj: &mut Object,
    loco: &LocomotorParams,
    goal: &Coord3D,
    on_path_dist: f32,
    desired_speed: f32,
    gravity: f32,
    terrain: &TerrainLogic,
    frame: u32,
) -> bool {
    let max_forward_speed = loco.max_speed;
    let mut desired_speed = desired_speed.clamp(loco.min_speed, max_forward_speed.max(loco.min_speed));
    let actual_forward_speed = obj.physics.as_ref().map_or(0.0, |p| p.forward_speed_3d(obj.transform.x_vector()));
    if loco.braking > 0.0 {
        let slow = calc_slow_down_dist(actual_forward_speed, loco.min_speed, loco.braking);
        if on_path_dist < slow && !obj.loco_rt.no_slow_down_as_approaching_dest {
            desired_speed = loco.min_speed;
        }
    }
    let pos = obj.position;
    let mut local_goal = *goal;
    // `if( m_preferredHeight != 0.0f && !getFlag(PRECISE_Z_POS) )` (`Locomotor.cpp:1932`).
    if loco.preferred_height != 0.0 && !loco.precise_z {
        let surface = surface_height(terrain, pos.x, pos.y, pos.z);
        local_goal.z = loco.preferred_height + surface;
        let mut delta = local_goal.z - pos.z;
        delta *= loco.preferred_height_damping;
        local_goal.z = pos.z + delta;
    }
    let forward_dir = forward_vector(obj);
    let forward_speed_delta = desired_speed - actual_forward_speed;
    let max_accel = if forward_speed_delta > 0.0 || loco.braking == 0.0 { loco.acceleration } else { -loco.braking };
    let max_turn_rate = loco.max_turn_rate;
    let desired_thrust_dir = calc_direction_to_apply_thrust(obj, local_goal, max_accel, gravity);
    let max_thrust_angle = if max_turn_rate > 0.0 { loco.max_thrust_angle } else { 0.0 };
    let (thrust_dir, thrust_angle) = try_rotate_vector3d(max_thrust_angle, forward_dir, desired_thrust_dir);

    // Orient along the velocity (not the thrust direction).
    // `obj->getStatusBits().test(OBJECT_STATUS_BRAKING)` (`Locomotor.cpp:1979`): the status bit the last
    // `locoUpdate_moveTowardsPosition` set, not the live locomotor flag.
    let vel = obj.physics.as_ref().map_or(Coord3D::ZERO, |p| p.velocity);
    let braking = obj.status_bits & status::IS_BRAKING != 0;
    if !nearly_zero(obj.physics.as_ref().map_or(0.0, |p| p.velocity_magnitude())) {
        let mut vel = vel;
        let mut adjust = true;
        let mut turn = max_turn_rate;
        if braking {
            vel = Coord3D::new(goal.x - pos.x, goal.y - pos.y, goal.z - pos.z);
            if nearly_zero(v3_dot(vel, vel)) {
                adjust = false;
            }
            turn *= 3.0;
        }
        if adjust {
            try_orient_in_this_direction_3d(obj, turn, vel);
        }
    }

    if forward_speed_delta != 0.0 || thrust_angle != 0.0 {
        let max_forward_speed = max_forward_speed.max(0.01);
        let damping = (max_accel / max_forward_speed).clamp(0.0, 1.0);
        let cur_vel = obj.physics.as_ref().map_or(Coord3D::ZERO, |p| p.velocity);
        let accel_vec = Coord3D::new(
            thrust_dir.x * max_accel - cur_vel.x * damping,
            thrust_dir.y * max_accel - cur_vel.y * damping,
            thrust_dir.z * max_accel - cur_vel.z * damping,
        );
        // `Real mass = physics->getMass();` (`Locomotor.cpp:2015`).
        let mass = obj.get_mass().unwrap_or(1.0);
        apply_motive_force_obj(obj, Coord3D::new(mass * accel_vec.x, mass * accel_vec.y, mass * accel_vec.z), frame);
    }

    let dx = goal.x - obj.position.x;
    let dy = goal.y - obj.position.y;
    let dz = goal.z - obj.position.z;
    sqrt(dx * dx + dy * dy + dz * dz) <= loco.close_enough_dist.max(2.0)
}

/// `Locomotor::getSurfaceHtAtPt` (`Locomotor.cpp:2028`): the water surface where the ground is
/// under water, otherwise the ground. Bridge decks are not part of it, so a preferred height is
/// measured from the ground beneath a bridge, as in the engine.
fn surface_height(terrain: &TerrainLogic, x: f32, y: f32, _current_z: f32) -> f32 {
    let (underwater, water_z, ground_z) = terrain.underwater_info(x, y);
    if underwater {
        water_z.unwrap_or(ground_z)
    } else {
        ground_z
    }
}

/// Move `(x, y)` toward `goal` at the locomotor's rates. Returns `true` on arrival.
///
/// This is the shared ground kernel (`moveTowardsPositionLegs`/`Wheels`/`Treads`); the air flavours
/// add `handleBehaviorZ` and, for wings, a minimum speed.
pub fn move_towards_position(
    loco: &LocomotorParams,
    pos: &mut Coord3D,
    angle: &mut f32,
    speed: &mut f32,
    goal: &Coord3D,
    terrain: &TerrainLogic,
) -> bool {
    let (dx, dy) = (goal.x - pos.x, goal.y - pos.y);
    let dist = sqrt(dx * dx + dy * dy);
    if dist <= 0.001 {
        *speed = if loco.kind == LocomotorKind::Wings { loco.min_speed } else { 0.0 };
        handle_behavior_z(loco, pos, terrain, goal);
        return true;
    }
    let desired = atan2(dy, dx);
    let diff = angle_diff(desired, *angle);
    let turn = diff.clamp(-loco.max_turn_rate, loco.max_turn_rate);
    *angle = normalize_angle(*angle + turn);
    let remaining = angle_diff(desired, *angle);

    // Target speed: full speed when aligned, crawl (or stop) when still facing away.
    let mut target = loco.max_speed;
    let align = cos(remaining);
    if align < 0.0 {
        target = 0.0;
    } else {
        target *= align;
    }
    // Speed from which we can still brake to a halt over `dist`.
    if loco.braking > 0.0 {
        let brake_limit = sqrt(2.0 * loco.braking * dist);
        if brake_limit < target {
            target = brake_limit;
        }
    }
    // Wings and THRUST never drop below `MinSpeed` (they would fall out of the sky); other
    // locomotors crawl. `moveTowardsPositionThrust` clamps `desiredSpeed` to `[MinSpeed, MaxSpeed]`.
    let keeps_min_speed = matches!(loco.kind, LocomotorKind::Wings | LocomotorKind::Thrust);
    if target < loco.min_speed && (keeps_min_speed || target > 0.0) {
        target = loco.min_speed;
    }
    let delta = target - *speed;
    *speed += if delta > 0.0 { delta.min(loco.acceleration) } else { delta.max(-loco.braking) };
    if *speed < 0.0 {
        *speed = 0.0;
    }
    // Fixed-wing aircraft and THRUST cannot fly slower than `MinSpeed`.
    if keeps_min_speed && *speed < loco.min_speed {
        *speed = loco.min_speed;
    }

    if *speed >= dist && loco.kind != LocomotorKind::Wings && loco.kind != LocomotorKind::Thrust {
        pos.x = goal.x;
        pos.y = goal.y;
        *speed = 0.0;
        handle_behavior_z(loco, pos, terrain, goal);
        return true;
    }
    pos.x += cos(*angle) * *speed;
    pos.y += sin(*angle) * *speed;
    handle_behavior_z(loco, pos, terrain, goal);
    // Wings "arrive" when they pass close enough to the goal.
    loco.kind == LocomotorKind::Wings && dist <= loco.close_enough_dist.max(2.0)
}

/// `Locomotor::handleBehaviorZ`: keep `z` at the surface, the preferred height, or leave it alone.
pub fn handle_behavior_z(loco: &LocomotorParams, pos: &mut Coord3D, terrain: &TerrainLogic, _goal: &Coord3D) {
    let surface = surface_height(terrain, pos.x, pos.y, pos.z);
    let target_z = match loco.behavior_z {
        ZBehavior::NoZMotiveForce => {
            if loco.stick_to_ground {
                // Ground locomotors glue to their layer immediately (`STICK_TO_GROUND`): the layer
                // height (`TerrainLogic::getLayerHeight`, a bridge deck when on one), not
                // `getSurfaceHtAtPt`, which only the preferred-height flavours use (Locomotor.cpp:2238).
                pos.z = terrain.surface_height(pos.x, pos.y, pos.z);
                return;
            }
            return;
        }
        ZBehavior::AbsoluteHeight => loco.preferred_height,
        // SURFACE_RELATIVE_HEIGHT and the fixed/other flavours: above the surface.
        _ => surface + loco.preferred_height,
    };
    // Climb/descend at a fixed rate (the engine integrates `Lift` as an acceleration; this is the
    // reduced form, fast enough to reach `PreferredHeight` in a couple of seconds).
    let rate = (loco.max_speed * 0.5).max(loco.lift).max(0.5);
    let dz = target_z - pos.z;
    if dz.abs() <= rate {
        pos.z = target_z;
    } else {
        pos.z += dz.signum() * rate;
    }
}

/// `Locomotor::locoUpdate_moveTowardsAngle`: turn the body toward `goal_angle` at the turn rate.
/// Returns `true` once aligned.
pub fn move_towards_angle(loco: &LocomotorParams, angle: &mut f32, goal_angle: f32) -> bool {
    let diff = angle_diff(goal_angle, *angle);
    if diff.abs() <= loco.max_turn_rate {
        *angle = normalize_angle(goal_angle);
        true
    } else {
        *angle = normalize_angle(*angle + diff.signum() * loco.max_turn_rate);
        false
    }
}

/// `Locomotor::locoUpdate_maintainCurrentPosition` for an object without physics (the reduced
/// position-based model): `maintainCurrentPositionOther` (`Locomotor.cpp:2601-2608`) stops a ground
/// unit dead, so this only zeroes the speed; thrust and wings never stop.
pub fn maintain_current_position(loco: &LocomotorParams, pos: &mut Coord3D, angle: f32, speed: &mut f32, terrain: &TerrainLogic) {
    if matches!(loco.kind, LocomotorKind::Wings | LocomotorKind::Thrust) {
        // Fixed-wing aircraft and THRUST cannot stop; keep flying straight at min speed
        // (`maintainCurrentPositionThrust` -> `moveTowardsPositionThrust(..., getMinSpeed())`).
        if *speed < loco.min_speed {
            *speed = loco.min_speed;
        }
        pos.x += cos(angle) * *speed;
        pos.y += sin(angle) * *speed;
        let goal = Coord3D::new(pos.x, pos.y, pos.z);
        handle_behavior_z(loco, pos, terrain, &goal);
        return;
    }
    *speed = 0.0;
    // Keep the preferred height even while stationary (hover / thrust), or glue to the ground.
    let goal = Coord3D::new(pos.x, pos.y, pos.z);
    handle_behavior_z(loco, pos, terrain, &goal);
}

impl GameLogic {
    /// `AIUpdateInterface::setLocomotorSet` (`LocomotorSetUpgrade` calls this): switch `id` to the
    /// `Locomotor = SET_x` entry named by `set`. Returns `false` when the template has no such set.
    ///
    /// This is the stable seam the upgrades agent uses; the AI reads the set every frame, so the new
    /// rates apply immediately.
    pub fn set_locomotor_set(&mut self, id: ObjectId, set: LocomotorSet) -> bool {
        let rebuilt = {
            let Some(o) = self.objects.get_mut(&id) else { return false };
            if o.destroyed || o.template.locomotor_for(set).is_none() {
                return false;
            }
            // `chooseLocomotorSet` returns early for the set already in use (`AIUpdate.cpp:815`), so only a
            // real switch rebuilds the `Locomotor`s and draws their ctor RNG.
            let rebuilt = o.locomotor_set != set;
            o.locomotor_set = set;
            rebuilt
        };
        if rebuilt {
            self.rebuild_locomotors(id, set);
        }
        self.apply_locomotor_physics_options(id);
        // No wake (R28 F1): `setLocomotorUpgrade`, `chooseLocomotorSet` and `chooseGoodLocomotorFromCurrentSet`
        // (AIUpdate.cpp:801-872) never wake the module; it picks the new locomotor at its next scheduled update.
        true
    }

    /// The draws of one `Locomotor::Locomotor` (`Locomotor.cpp:673-675`), in ctor order: the wander angle offset
    /// `GameLogicRandomValueReal(-PI/6, PI/6)`, the increment `GameLogicRandomValueReal(0.8, 1.2)` and the
    /// direction `GameLogicRandomValue(0, 1)`. The wander itself is not ported.
    pub(crate) fn draw_locomotor_ctor_rng(&mut self) {
        let _angle_offset = self.rng.value_real(-PI / 6.0, PI / 6.0);
        let _offset_increment = self.rng.value_real(0.8, 1.2);
        let _offset_increasing = self.rng.value(0, 1);
    }

    /// `chooseLocomotorSetExplicit` (`AIUpdate.cpp:830-841`) for a real switch to `set`: it builds fresh `Locomotor`s,
    /// one per resolved entry of the set (`addLocomotor`, `Locomotor.cpp:2765-2767`). Each ctor resets the runtime
    /// flags, `m_brakingFactor = 1.0f` and `m_donutTimer = now + 75` (`Locomotor.cpp:658`, `:676`), its preferred
    /// height and close-enough distance back to the template (`:659-670`, so a runtime `setPreferredHeight` is
    /// dropped), and draws its RNG ([`draw_locomotor_ctor_rng`](Self::draw_locomotor_ctor_rng), `:673-675`), so the
    /// switch draws one triple per entry. `m_flags = 0` (`:658`) also clears `IS_BRAKING`, `MOVING_BACKWARDS` and
    /// `DOING_THREE_POINT_TURN` (`Locomotor.h:433-436`), which the port keeps in the physics state; the
    /// `OBJECT_STATUS_BRAKING` bit follows the flag on the next `locoUpdate` (`Locomotor.cpp:1111`), so it is not
    /// touched here. The caller decides whether this is a real switch.
    pub(crate) fn rebuild_locomotors(&mut self, id: ObjectId, set: LocomotorSet) {
        let frame = self.frame;
        let entries = self.objects.get(&id).map_or(0, |o| o.template.locomotor_entry_count(set));
        if let Some(o) = self.objects.get_mut(&id) {
            o.loco_rt = crate::object::LocoRuntime::default();
            o.preferred_height_override = None;
            if let Some(p) = o.physics.as_mut() {
                p.braking_factor = 1.0;
                p.donut_timer = frame + DONUT_TIME_DELAY_FRAMES;
                p.braking = false;
                p.moving_backwards = false;
                p.doing_three_point_turn = false;
            }
        }
        for _ in 0..entries {
            self.draw_locomotor_ctor_rng();
        }
    }

    /// The locomotor `id` drives with right now: [`Object::locomotor`] with the body's damage state applied
    /// ([`apply_body_damage_state`]). `getBodyModule()->getDamageState()` (`Locomotor.cpp:1618-1625`,
    /// `AIUpdate.cpp:2166`, `:2213`); the body state comes from the health ratio and the `GameData` thresholds
    /// ([`GameLogic::body_damage_state`]). An object without a body is `BODY_PRISTINE`.
    pub(crate) fn locomotor_now(&self, id: ObjectId) -> Option<LocomotorParams> {
        let mut loco = self.objects.get(&id)?.locomotor()?;
        let condition = self.body_damage_state(id).unwrap_or(BodyDamageType::Pristine);
        apply_body_damage_state(&mut loco, condition, self.rules.movement_penalty_damage_state);
        Some(loco)
    }

    /// The locomotor set `id` is currently using.
    pub fn locomotor_set(&self, id: ObjectId) -> Option<LocomotorSet> {
        self.objects.get(&id).map(|o| o.locomotor_set)
    }

    /// `Locomotor::startMove` (`Locomotor.cpp:782-786`): "Reset the donut timer",
    /// `m_donutTimer = getFrame() + DONUT_TIME_DELAY_SECONDS*LOGICFRAMES_PER_SECOND`. The engine calls it
    /// from `AIInternalMoveToState::onEnter` (`AIStates.cpp:1627-1628`), the only caller, so the port's
    /// AI move-state entry ([`GameLogic::ai_internal_move_enter`]) is the one call site.
    pub(crate) fn loco_start_move(&mut self, id: ObjectId) {
        let frame = self.frame;
        let Some(o) = self.objects.get_mut(&id) else { return };
        // `if (ai->getCurLocomotor())` (`AIStates.cpp:1627`): only an object with a current locomotor
        // instance has a donut timer to reset.
        if o.locomotor().is_none() {
            return;
        }
        if let Some(p) = o.physics.as_mut() {
            p.donut_timer = frame + DONUT_TIME_DELAY_FRAMES;
        }
    }

    /// `Locomotor::setPhysicsOptions` (`Locomotor.cpp:932`): push `StickToGround`,
    /// `Apply2DFrictionWhenAirborne` and `Extra2DFriction` onto the object's `PhysicsBehavior`.
    pub fn apply_locomotor_physics_options(&mut self, id: ObjectId) {
        let (stick, airborne_friction, extra) = match self.objects.get(&id).and_then(|o| o.locomotor()) {
            Some(l) => (l.stick_to_ground, l.apply_2d_friction_when_airborne, l.extra_2d_friction),
            None => return,
        };
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.physics.as_mut()) {
            p.stick_to_ground = stick;
            p.apply_friction_2d_when_airborne = airborne_friction;
            p.extra_friction = extra;
        }
    }
}

#[cfg(test)]
#[path = "locomotor_l176.rs"]
mod l176_tests;

#[cfg(test)]
mod braking_flag_tests {
    //! The top-level `IS_BRAKING` clear and the `OBJECT_STATUS_BRAKING` bit (`Locomotor.cpp:950-1162`).
    use super::*;
    use crate::physics::PhysicsTemplate;
    use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
    use std::sync::Arc;

    /// A wheeled unit at `dist` from its goal along +x, with the setup of the AN3 dozer's final
    /// approach (`AmericaVehicleDozer`): speed 1.0, braking 0.0556, forward speed 0.9549.
    fn wheels_at(dist: f32) -> (GameLogic, ObjectId, LocomotorParams, Coord3D, TerrainLogic) {
        let mut t = ObjectTemplate::new("Wheeler");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 5.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
        t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Wheels, 1.0, 0.0, 0.2, 0.0556, 0.1, true));
        let t = Arc::new(t);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let loco = g.objects.get(&id).unwrap().locomotor().unwrap();
        let goal = Coord3D::new(50.0 + dist, 50.0, 0.0);
        (g, id, loco, goal, TerrainLogic::flat(400, 400, 0))
    }

    /// `Locomotor.cpp:962-967`: `distToStopAtMaxSpeed = (maxSpeed/getBraking())*maxSpeed/2.0`;
    /// beyond it the locomotor flag is cleared *before* the kernel runs, so a braking unit moving at
    /// `0.9549`/frame at 12.7 units stops braking (the kernel's slow-down distance there is 12.53)
    /// and the physics integrates x/y that frame. Without the clear the flag would stay set and
    /// `PhysicsUpdate.cpp:664` would freeze the unit in place (the port's dozer lost ~1.2 units at
    /// frame 404 this way).
    #[test]
    fn the_toplevel_braking_clear_keeps_the_wheels_flag_off_at_its_slowdown_edge() {
        let (mut g, id, loco, goal, terrain) = wheels_at(12.7);
        {
            let o = g.objects.get_mut(&id).unwrap();
            let p = o.physics.as_mut().unwrap();
            p.braking = true;
            p.velocity = Coord3D::new(0.9549, 0.0, 0.0);
            // Not the donut brake's business (12.7 < DONUT_DISTANCE 40, so an unset timer would set it).
            p.donut_timer = 100;
        }
        {
            let o = g.objects.get_mut(&id).unwrap();
            let arrived = move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 10, 1.0, -1.0, Some(12.7));
            assert!(!arrived);
            assert!(!o.physics.as_ref().unwrap().braking, "the top-level distToStopAtMaxSpeed clear");
            assert_eq!(o.status_bits & status::IS_BRAKING, 0, "the status bit follows the flag at the end of the call");
        }
    }

    /// `Bool wasBraking = obj->getStatusBits().test(OBJECT_STATUS_BRAKING)` (`Locomotor.cpp:1073`):
    /// the status bit set by the previous call (`:1111`), not the current flag, so the `wasBraking`
    /// step still moves the unit on a frame whose top-level clear just dropped the flag. This is
    /// C++'s frame 404 for the dozer: the flag was cleared and the unit still stepped 0.95 toward
    /// the goal, then the physics also integrated its velocity (the port froze it).
    #[test]
    fn the_was_braking_step_reads_the_status_bit_not_the_flag() {
        let (mut g, id, loco, goal, terrain) = wheels_at(12.7);
        {
            let o = g.objects.get_mut(&id).unwrap();
            let p = o.physics.as_mut().unwrap();
            p.braking = false;
            p.velocity = Coord3D::new(0.9549, 0.0, 0.0);
            p.donut_timer = 100;
            let start = o.position.x;
            o.status_bits |= status::IS_BRAKING; // what the previous frame's locoUpdate set
            let arrived = move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 10, 1.0, -1.0, Some(12.7));
            assert!(!arrived);
            let moved = o.position.x - start;
            assert!((moved - 0.9549).abs() < 0.01, "the wasBraking step moved {moved} (needs the status bit)");
            assert!(!o.physics.as_ref().unwrap().braking, "the kernel does not start braking past its slow-down distance");
        }
    }

    /// `PhysicsUpdate.cpp:664`: the physics freezes x/y on `OBJECT_STATUS_BRAKING`. With the flag and
    /// the status bit split, a locomotor flag alone does not stop the integration.
    #[test]
    fn the_physics_gate_reads_the_status_bit_not_the_locomotor_flag() {
        let (mut g, id, _loco, _goal, _terrain) = wheels_at(20.0);
        {
            let o = g.objects.get_mut(&id).unwrap();
            let p = o.physics.as_mut().unwrap();
            p.velocity = Coord3D::new(1.0, 0.0, 0.0);
            p.braking = true; // the locomotor flag alone
        }
        g.physics_due_now(id);
        let start = g.objects.get(&id).unwrap().position.x;
        g.update();
        let moved_flag_only = g.objects.get(&id).unwrap().position.x - start;
        assert!(moved_flag_only > 0.0, "a locomotor flag alone does not freeze the physics");
        {
            let o = g.objects.get_mut(&id).unwrap();
            let p = o.physics.as_mut().unwrap();
            p.velocity = Coord3D::new(1.0, 0.0, 0.0);
            o.status_bits |= status::IS_BRAKING;
        }
        g.physics_due_now(id);
        let start = g.objects.get(&id).unwrap().position.x;
        g.update();
        assert_eq!(g.objects.get(&id).unwrap().position.x, start, "the status bit freezes x/y");
    }
}

#[cfg(test)]
mod kernel_tests {
    //! The turn pivot (`Locomotor.cpp:2134-2208`): `locoUpdate_moveTowardsAngle` uses it
    //! (`:904-917`), the legs/climb kernels go through `locoUpdate_moveTowardsAngle` (`:1681`,
    //! `:1802`), the pivot move is a transform change that dirties the partition (Object.cpp:1768-1858)
    //! and the early return reports `TURN_NONE` (`:2153-2154`). Also `applyMotiveForce`'s reciprocal
    //! mass (`PhysicsUpdate.cpp:335-338`).
    use super::*;
    use crate::physics::PhysicsTemplate;
    use crate::template::{kind_of, ObjectTemplate};
    use std::sync::Arc;

    /// A legs locomotor with `TurnPivotOffset = 0.5` on a body of bounding radius 10, so the pivot is
    /// five units ahead of the centre.
    fn pivot_legs() -> (GameLogic, ObjectId, LocomotorParams, TerrainLogic) {
        let mut t = ObjectTemplate::new("PivotLegs");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 10.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.physics = Some(PhysicsTemplate { mass: 3.0, ..PhysicsTemplate::default() });
        let mut loco = LocomotorParams::from_engine_units(LocomotorKind::Legs, 1.0, 0.0, 0.2, 0.1, 3.0, true);
        loco.turn_pivot_offset = 0.5;
        t.locomotor = Some(loco);
        let t = Arc::new(t);
        let terrain = TerrainLogic::flat(100, 100, 0);
        let mut g = GameLogic::new(terrain.clone(), 1);
        let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        (g, id, loco, terrain)
    }

    /// `locoUpdate_moveTowardsAngle`'s `MinSpeed == 0` branch calls `rotateTowardsPosition`
    /// (`Locomotor.cpp:914-917`), which turns about `getTurnPivotOffset()` (`:2162-2166`); the rotation
    /// about the pivot moves the object's centre.
    #[test]
    fn move_towards_angle_turns_about_the_turn_pivot() {
        let (mut g, id, loco, terrain) = pivot_legs();
        let start = g.objects.get(&id).unwrap().position;
        {
            let o = g.objects.get_mut(&id).unwrap();
            move_towards_angle_force(o, &loco, PI / 2.0, &terrain, &g.pathfinder, 0, -1.0);
        }
        let o = g.objects.get(&id).unwrap();
        // The far point is 1000 units out at 90°, seen from the pivot (55, 50), so the bearing is
        // slightly off 90° (`atan2(1000, -5)`).
        let far = Coord3D::new(50.0, 50.0 + 1000.0, 0.0);
        let expected = atan2(far.y - 50.0, far.x - 55.0);
        assert!((o.angle - expected).abs() < 1e-4, "turned to the far point's bearing: expected {expected}, got {}", o.angle);
        // The centre, five units behind the pivot, swings around it by that amount.
        let want = Coord3D::new(55.0 - 5.0 * cos(expected), 50.0 - 5.0 * sin(expected), 0.0);
        assert!((o.position.x - want.x).abs() < 0.01 && (o.position.y - want.y).abs() < 0.01, "the pivot moved the centre: {:?}", o.position);
        assert!(o.position != start);
    }

    /// `obj->setTransformMatrix(&mtx)` in the pivot branch (`Locomotor.cpp:2180`) runs
    /// `Object::reactToTransformChange`, which dirties the partition when the object moved
    /// (Object.cpp:1808-1826).
    #[test]
    fn the_pivot_move_dirties_the_partition() {
        let (mut g, id, loco, terrain) = pivot_legs();
        g.update_partition(); // places the object and clears the creation-time dirty flag
        assert!(!g.objects.get(&id).unwrap().partition_dirty, "clean before the turn");
        {
            let o = g.objects.get_mut(&id).unwrap();
            move_towards_angle_force(o, &loco, PI / 2.0, &terrain, &g.pathfinder, 0, -1.0);
        }
        assert!(g.objects.get(&id).unwrap().partition_dirty, "the pivot move dirties the partition cell");
        g.update_partition();
        let pos = g.objects.get(&id).unwrap().position;
        assert!(g.partition().objects_in_range(&pos, 1.0, crate::partition::DistanceMode::FromCenter2D).contains(&id), "the partition follows the pivot move");
    }

    /// The legs kernel calls `locoUpdate_moveTowardsAngle(obj, desiredAngle)` (`Locomotor.cpp:1681`),
    /// which turns toward a point 1000 units out along the bearing rather than toward the goal, so
    /// with a pivot offset the desired bearing is the one from the pivot point to that far point.
    #[test]
    fn legs_turn_toward_the_far_point_not_the_goal() {
        let (mut g, id, loco, terrain) = pivot_legs();
        let goal = Coord3D::new(60.0, 60.0, 0.0);
        {
            let o = g.objects.get_mut(&id).unwrap();
            move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 0, 1.0, -1.0, Some(1000.0));
        }
        let angle = g.objects.get(&id).unwrap().angle;
        // `desiredPos = pos + 1000 * (cos, sin)` of the goal's bearing; the pivot is at (55, 50).
        let far = Coord3D::new(50.0 + cos(PI / 4.0) * 1000.0, 50.0 + sin(PI / 4.0) * 1000.0, 0.0);
        let expected = atan2(far.y - 50.0, far.x - 55.0);
        assert!((angle - expected).abs() < 1e-3, "turned to the far point from the pivot: expected {expected}, got {angle}");
        let toward_goal = atan2(goal.y - 50.0, goal.x - 55.0);
        assert!((angle - toward_goal).abs() > 0.1, "not the goal bearing ({toward_goal})");
    }

    /// The pivot turn is `mtx.mul(tmp, *getTransformMatrix())` then `setTransformMatrix(&mtx)` (`Locomotor.cpp:2171-2180`),
    /// so the heading is `Get_Z_Rotation()` of the new matrix and the centre is its translation
    /// (`Thing.cpp:263-283`). Over several turns the matrix accumulates the rotations; the heading is its readback,
    /// not `normalizeAngle(angle + amount)` (which differs from it by ULPs).
    #[test]
    fn the_pivot_heading_is_the_transform_matrix_readback() {
        let (mut g, id, loco, terrain) = pivot_legs();
        let far = Coord3D::new(50.0, 50.0 + 1000.0, 0.0);
        for frame in 0..12 {
            let o = g.objects.get_mut(&id).unwrap();
            move_towards_angle_force(o, &loco, PI / 2.0, &terrain, &g.pathfinder, frame, -1.0);
            assert_eq!(o.angle.to_bits(), o.transform.z_rotation().to_bits(), "frame {frame}: heading is the matrix readback");
            assert_eq!(o.position, o.transform.translation(), "frame {frame}: position is the matrix translation");
        }
        let o = g.objects.get(&id).unwrap();
        assert!(o.angle > 0.0, "the unit turned toward the far point {far:?}: {}", o.angle);
    }

    /// `if (fabs(dx)<0.1f && fabs(dy)<0.1f) return TURN_NONE` (`Locomotor.cpp:2153-2154`): the
    /// callers do `physics->setTurning(rotating)`, so the stale `m_turning` is cleared.
    #[test]
    fn the_pivot_early_return_clears_turning() {
        let (mut g, id, _loco, _terrain) = pivot_legs();
        let o = g.objects.get_mut(&id).unwrap();
        o.physics.as_mut().unwrap().turning = 1;
        // The turn point is (55, 50); this goal is within 0.1 on both axes.
        let rel = rotate_towards_position(o, &Coord3D::new(55.05, 50.02, 0.0), 3.0, 0.5);
        assert_eq!(rel, 0.0);
        assert_eq!(o.physics.as_ref().unwrap().turning, 0, "TURN_NONE");
        assert_eq!(o.angle, 0.0, "no rotation");
        assert_eq!(o.position, Coord3D::new(50.0, 50.0, 0.0), "no pivot move");
    }

    /// `massInv = 1.0f / mass; accel += force * massInv` (`PhysicsUpdate.cpp:335-338`), also in the
    /// kernels' inlined `applyMotiveForce`: `2.5 * (1/3)` differs from `2.5 / 3` in f32.
    #[test]
    fn the_motive_force_uses_the_reciprocal_mass() {
        let (mut g, id, _loco, _terrain) = pivot_legs();
        let mass = 3.0f32;
        let force = 2.5f32;
        assert_ne!(force * (1.0 / mass), force / mass, "the two spellings differ in f32");
        {
            let o = g.objects.get_mut(&id).unwrap();
            apply_motive_force_obj(o, Coord3D::new(force, 0.0, 0.0), 0);
        }
        let accel = g.objects.get(&id).unwrap().physics.as_ref().unwrap().accel.x;
        assert_eq!(accel, force * (1.0 / mass), "the reciprocal multiply");
        assert_ne!(accel, force / mass, "not a division");
    }

    /// `Locomotor.cpp:1368-1432` (wheels): turning more than 15 degrees, half a second of travel is
    /// projected along the intended turn; when the terrain there is not one the locomotor can move on
    /// (`!validMovementTerrain`), the kernel only rotates toward the goal — no drive force at all.
    #[test]
    fn wheels_turn_in_place_instead_of_driving_into_water() {
        // 62x62 samples = 310 world units; a water strip at world x 300..340 fills the last cells.
        let mut terrain = TerrainLogic::from_samples(62, 62, 0, vec![0u8; 62 * 62]);
        terrain.add_water_area(zr_map::PolygonTrigger {
            name: "strip".into(),
            layer: String::new(),
            id: 7,
            is_water: true,
            is_river: false,
            river_start: 0,
            points: vec![[300, -10, 10], [340, -10, 10], [340, 700, 10], [300, 700, 10]],
        });
        let pathfinder = crate::pathfind::Pathfinder::new(&terrain);
        assert_eq!(pathfinder.cell_type_at(&Coord3D::new(302.0, 200.0, 0.0)), Some(crate::pathfind::CellType::Water));
        assert_eq!(pathfinder.cell_type_at(&Coord3D::new(295.0, 200.0, 0.0)), Some(crate::pathfind::CellType::Clear));

        let mut wheels = ObjectTemplate::new("Wheels");
        wheels.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        wheels.geometry.major_radius = 3.0;
        wheels.max_health = 100.0;
        wheels.initial_health = 100.0;
        wheels.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
        wheels.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Wheels, 1.0, 0.0, 0.2, 0.1, 0.1, true));
        let wheels = Arc::new(wheels);
        let mut g = GameLogic::new(terrain.clone(), 1);
        // Facing +x toward the water, goal 90 degrees to the side: |relAngle| > 15°, and the
        // half-second projection (goalSpeed 1, actualSpeed 0 -> 7.5 units) lands in the water.
        let blocked = g.create_object(&wheels, Coord3D::new(295.0, 200.0, 0.0), 0.0, 0);
        let clear = g.create_object(&wheels, Coord3D::new(100.0, 200.0, 0.0), 0.0, 0);
        let loco = g.objects.get(&blocked).unwrap().locomotor().unwrap();
        let goal = Coord3D::new(0.0, 250.0, 0.0);
        for id in [blocked, clear] {
            g.objects.get_mut(&id).unwrap().physics.as_mut().unwrap().velocity = Coord3D::new(0.9, 0.0, 0.0);
        }
        {
            let o = g.objects.get_mut(&blocked).unwrap();
            move_towards_position_force(o, &loco, &goal, &terrain, &pathfinder, 10, 1.0, -1.0, Some(1000.0));
        }
        {
            let o = g.objects.get(&blocked).unwrap();
            assert_eq!(o.physics.as_ref().unwrap().accel, Coord3D::ZERO, "no drive force into the water");
            assert!((o.angle - 0.1).abs() < 1e-5, "it turns toward the goal in place (maxTurnRate 0.1): {}", o.angle);
        }
        // The same setup on clear ground does drive (the projection is a no-op there).
        {
            let o = g.objects.get_mut(&clear).unwrap();
            move_towards_position_force(o, &loco, &goal, &terrain, &pathfinder, 10, 1.0, -1.0, Some(1000.0));
            assert!(o.physics.as_ref().unwrap().accel != Coord3D::ZERO, "clear terrain: the drive runs");
        }
    }

    /// `moveTowardsPositionClimb` (`Locomotor.cpp:1711-1846`): the `CLIMBING` flag is a `Locomotor`
    /// member, set only when the goal is more than one pathfind cell away in height and cleared under one
    /// unit (`:1732-1737`); while climbing, the slope comes from the terrain height one unit ahead along
    /// the goal (`:1743-1761`), not from the goal's own z. A step down of more than 0.1 unit backs the
    /// climber up: the desired angle reverses and the actual speed is negated (`:1752-1754`,
    /// `:1773-1776`, `:1792-1794`). Revert check: the old goal-z slope divides the 2-unit rise by 8 (accel
    /// 0.125, not 1.0) and never reverses the angle.
    #[test]
    fn the_climb_kernel_slows_only_while_climbing_and_backs_up_down_a_step() {
        let mut t = ObjectTemplate::new("Climber");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 5.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
        t.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Climber, 1.0, 0.0, 2.0, 0.1, 0.1, true));
        let t = Arc::new(t);
        let terrain = TerrainLogic::flat(400, 400, 0);
        let mut g = GameLogic::new(terrain.clone(), 1);
        let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let loco = g.objects.get(&id).unwrap().locomotor().unwrap();
        // A rise of 2 units is inside the hysteresis band: not climbing, so no slope slow-down. Mass 10
        // and acceleration 2 give the full-speed force 10 (accel 1.0); the old goal-z slope divided the
        // desired speed by 8 and gave 0.125.
        let rise = Coord3D::new(150.0, 50.0, 2.0);
        {
            let o = g.objects.get_mut(&id).unwrap();
            move_towards_position_force(o, &loco, &rise, &terrain, &g.pathfinder, 10, 1.0, -1.0, Some(200.0));
        }
        assert!(!g.objects[&id].loco_rt.climbing, "|dz| 2 is not > PATHFIND_CELL_SIZE 10");
        let accel = g.objects[&id].physics.as_ref().unwrap().accel.x;
        assert!((accel - 1.0).abs() < 1e-6, "full speed on a 2-unit rise, got {accel}");

        // Climbing over flat ground: the flag latches on a 20-unit rise and stays through a 5-unit one,
        // and the terrain ahead (flat) is what the slope reads, so there is no slow-down either frame.
        for z in [20.0f32, 5.0] {
            g.objects.get_mut(&id).unwrap().physics.as_mut().unwrap().accel = Coord3D::ZERO;
            let goal = Coord3D::new(150.0, 50.0, z);
            let o = g.objects.get_mut(&id).unwrap();
            move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 10, 1.0, -1.0, Some(200.0));
            let accel = g.objects[&id].physics.as_ref().unwrap().accel.x;
            assert!((accel - 1.0).abs() < 1e-6, "goal z {z}: the ground ahead is level, got {accel}");
        }
        assert!(g.objects[&id].loco_rt.climbing, "the flag stays set between the 10-unit thresholds");

        // Standing 5 units above flat ground with a goal 30 up: the ground one unit ahead is lower than
        // pos.z - 0.1, so the climber backs up: the desired angle reverses (the object turns away from +x)
        // and MOVING_BACKWARDS is set.
        {
            let o = g.objects.get_mut(&id).unwrap();
            o.position.z = 5.0;
            o.angle = 0.0;
            o.physics.as_mut().unwrap().accel = Coord3D::ZERO;
        }
        let up = Coord3D::new(150.0, 50.0, 30.0);
        {
            let o = g.objects.get_mut(&id).unwrap();
            move_towards_position_force(o, &loco, &up, &terrain, &g.pathfinder, 10, 1.0, -1.0, Some(200.0));
        }
        let o = g.objects.get(&id).unwrap();
        assert!(o.physics.as_ref().unwrap().moving_backwards, "the step down backs the climber up");
        assert!(o.angle.abs() > 0.09, "turned toward the reversed bearing, angle {}", o.angle);
    }

    /// `Pathfinder::validMovementTerrain` (`AIPathfind.cpp:4924-4941`): only `CELL_OBSTACLE` and
    /// `CELL_IMPASSABLE` are accepted regardless of surface; every other type is checked against
    /// `validLocomotorSurfacesForCellType`, where `CELL_BRIDGE_IMPASSABLE` offers AIR only (`:4904-4906`).
    /// Revert check: add `BridgeImpassable` back to the early return and the bridge asserts fail.
    #[test]
    fn bridge_impassable_terrain_is_air_only() {
        use crate::pathfind::{surface, CellType};
        assert!(valid_movement_terrain_cell(CellType::Obstacle, surface::GROUND), "an obstacle cell is accepted");
        assert!(valid_movement_terrain_cell(CellType::Impassable, surface::GROUND), "an impassable map cell is accepted");
        assert!(!valid_movement_terrain_cell(CellType::BridgeImpassable, surface::GROUND), "a ground locomotor may not enter it");
        assert!(!valid_movement_terrain_cell(CellType::BridgeImpassable, surface::WATER), "nor a boat");
        assert!(valid_movement_terrain_cell(CellType::BridgeImpassable, surface::AIR), "an air locomotor may");
        assert!(valid_movement_terrain_cell(CellType::Clear, surface::GROUND));
    }
}

#[cfg(test)]
mod donut_and_braking_factor_tests {
    //! `Locomotor::m_donutTimer` (`Locomotor.cpp:676`, `:785`, `:1426-1432`, `:2441`) and the persistent
    //! `Locomotor::m_brakingFactor` (`:445`, `:658`, `:966`, `:1218-1231`, `:1255`, `:1417-1441`,
    //! `:1491-1493`).
    use super::*;
    use crate::physics::PhysicsTemplate;
    use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
    use std::sync::Arc;

    fn unit(name: &str, loco: LocomotorParams, mass: f32, radius: f32) -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = radius;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.physics = Some(PhysicsTemplate { mass, ..PhysicsTemplate::default() });
        t.locomotor = Some(loco);
        Arc::new(t)
    }

    /// A wheeled unit at `dist` from its goal along +x, with the AN3 dozer's numbers (speed 1.0,
    /// braking 0.0556, forward speed 0.9549). Its 2D slow-down distance at that speed is 12.52.
    fn wheels_at(dist: f32) -> (GameLogic, ObjectId, LocomotorParams, Coord3D, TerrainLogic) {
        let t = unit("Wheeler", LocomotorParams::from_engine_units(LocomotorKind::Wheels, 1.0, 0.0, 0.2, 0.0556, 0.1, true), 10.0, 5.0);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let loco = g.objects.get(&id).unwrap().locomotor().unwrap();
        let goal = Coord3D::new(50.0 + dist, 50.0, 0.0);
        (g, id, loco, goal, TerrainLogic::flat(400, 400, 0))
    }

    /// A treaded unit (mass 10, `MaxSpeed` 1.0, `Braking` 0.1): at a full 1.0 forward speed its
    /// `slowDownDist` is `(1.0/1.5)*(1.0/0.1)` = 6.667 and `distToStopAtMaxSpeed` is 5.
    fn treads_at(dist: f32) -> (GameLogic, ObjectId, LocomotorParams, Coord3D, TerrainLogic) {
        let t = unit("Tread", LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.2, 0.1, 0.1, true), 10.0, 5.0);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let loco = g.objects.get(&id).unwrap().locomotor().unwrap();
        let goal = Coord3D::new(50.0 + dist, 50.0, 0.0);
        (g, id, loco, goal, TerrainLogic::flat(400, 400, 0))
    }

    fn physics(g: &mut GameLogic, id: ObjectId) -> &mut crate::physics::Physics {
        g.objects.get_mut(&id).unwrap().physics.as_mut().unwrap()
    }

    /// `Locomotor::Locomotor` (`Locomotor.cpp:676`): the ctor arms the donut timer
    /// (`m_donutTimer = getFrame()+DONUT_TIME_DELAY_SECONDS*LOGICFRAMES_PER_SECOND`).
    #[test]
    fn the_donut_timer_starts_at_creation_plus_the_delay() {
        let (g, id, _loco, _goal, _terrain) = wheels_at(20.0);
        assert_eq!(g.objects[&id].physics.as_ref().unwrap().donut_timer, DONUT_TIME_DELAY_FRAMES);
    }

    /// `m_donutTimer` and `m_brakingFactor` are `Locomotor` members that the engine saves and CRCs
    /// (`Locomotor::xfer`, `Locomotor.cpp:750-755`), and both change later braking, so they are part of
    /// the port's simulation hash: two otherwise identical worlds that differ only in them must hash
    /// differently.
    #[test]
    fn the_donut_timer_and_braking_factor_are_in_the_crc() {
        for (timer, factor) in [(DONUT_TIME_DELAY_FRAMES + 1, 1.0f32), (DONUT_TIME_DELAY_FRAMES, 2.0f32)] {
            let (g, id, _loco, _goal, _terrain) = wheels_at(20.0);
            let base = g.crc();
            let (mut other, other_id, _l, _go, _te) = wheels_at(20.0);
            assert_eq!(other_id, id, "identical worlds");
            {
                let p = other.objects.get_mut(&id).unwrap().physics.as_mut().unwrap();
                p.donut_timer = timer;
                p.braking_factor = factor;
            }
            assert_ne!(other.crc(), base, "donut_timer {timer}, braking_factor {factor} must be hashed");
        }
    }

    /// `Locomotor::startMove` (`Locomotor.cpp:782-786`), called from `AIInternalMoveToState::onEnter`
    /// (`AIStates.cpp:1627-1628`): a 20-unit move (inside `DONUT_DISTANCE` 40, outside the 12.52-unit
    /// 2D slow-down distance) with a donut timer that expired 200 frames ago does not brake on its
    /// first frame, because entering the move state re-arms the timer.
    #[test]
    fn a_short_wheeled_move_does_not_brake_on_its_first_frame() {
        let (mut g, id, loco, goal, terrain) = wheels_at(20.0);
        g.frame = 200;
        {
            let p = physics(&mut g, id);
            p.velocity = Coord3D::new(0.9549, 0.0, 0.0);
            p.donut_timer = 0;
        }
        g.ai_internal_move_enter_pub(id, goal, true);
        assert_eq!(
            g.objects[&id].physics.as_ref().unwrap().donut_timer,
            200 + DONUT_TIME_DELAY_FRAMES,
            "AIInternalMoveToState::onEnter -> Locomotor::startMove re-arms the timer"
        );
        let o = g.objects.get_mut(&id).unwrap();
        assert!(!move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 200, 1.0, -1.0, Some(20.0)));
        let o = &g.objects[&id];
        assert!(!o.physics.as_ref().unwrap().braking, "a fresh move 20 units out must not brake");
        assert_eq!(o.status_bits & status::IS_BRAKING, 0, "so the physics still integrates x/y");
    }

    /// The control for the test above: with the timer still expired the wheels donut test
    /// (`Locomotor.cpp:1429`) brakes the unit on that same frame.
    #[test]
    fn an_expired_donut_timer_brakes_a_wheels_unit_that_starts_inside_donut_distance() {
        let (mut g, id, loco, goal, terrain) = wheels_at(20.0);
        {
            let p = physics(&mut g, id);
            p.velocity = Coord3D::new(0.9549, 0.0, 0.0);
            p.donut_timer = 0;
        }
        let o = g.objects.get_mut(&id).unwrap();
        assert!(!move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 200, 1.0, -1.0, Some(20.0)));
        assert!(g.objects[&id].physics.as_ref().unwrap().braking, "20 < DONUT_DISTANCE and the timer expired");
    }

    /// `locoUpdate_maintainCurrentPosition` (`Locomotor.cpp:2441`) re-arms the donut timer every idle
    /// frame, before the physics null check and before the thrust/wings returns.
    #[test]
    fn maintain_current_position_rearms_the_donut_timer() {
        let (mut g, id, loco, _goal, terrain) = wheels_at(20.0);
        g.frame = 200;
        physics(&mut g, id).donut_timer = 0;
        {
            let o = g.objects.get_mut(&id).unwrap();
            // Not motive, so this returns before the brake force; the timer reset is above that.
            assert!(!maintain_current_position_force(o, &loco, 200, -1.0, &terrain));
        }
        assert_eq!(g.objects[&id].physics.as_ref().unwrap().donut_timer, 200 + DONUT_TIME_DELAY_FRAMES);
    }

    /// `AIUpdateInterface::chooseLocomotorSetExplicit` (`AIUpdate.cpp:830-847`) clears the set and adds
    /// fresh `Locomotor` instances for it, so their ctor state applies again: `m_flags = 0`
    /// (`Locomotor.cpp:658`) clears `IS_BRAKING`, `MOVING_BACKWARDS` and `DOING_THREE_POINT_TURN`
    /// (`Locomotor.h:433-436`), and `m_brakingFactor = 1.0` / `m_donutTimer` (`:656-676`) are reset.
    /// Revert check: drop the three flag clears and the asserts fail.
    #[test]
    fn a_locomotor_set_change_clears_the_runtime_flags() {
        let loco = LocomotorParams::from_engine_units(LocomotorKind::Wheels, 1.0, 0.0, 0.2, 0.1, 0.1, true);
        let mut t = ObjectTemplate::new("Switcher");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 5.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.physics = Some(PhysicsTemplate { mass: 10.0, ..PhysicsTemplate::default() });
        t.locomotor = Some(loco);
        t.locomotor_sets.insert(crate::template::LocomotorSet::Normal, loco);
        t.locomotor_sets.insert(crate::template::LocomotorSet::Sluggish, loco);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        {
            let p = physics(&mut g, id);
            p.braking = true;
            p.braking_factor = MAX_BRAKING_FACTOR;
            p.moving_backwards = true;
            p.doing_three_point_turn = true;
            p.donut_timer = 0;
        }
        assert!(g.set_locomotor_set(id, crate::template::LocomotorSet::Sluggish), "the template has that set");
        let p = g.objects[&id].physics.as_ref().unwrap();
        assert!(!p.braking, "IS_BRAKING is a fresh Locomotor's m_flags bit");
        assert!(!p.moving_backwards, "so is MOVING_BACKWARDS");
        assert!(!p.doing_three_point_turn, "and DOING_THREE_POINT_TURN");
        assert_eq!(p.braking_factor, 1.0, "the ctor's m_brakingFactor");
        assert_eq!(p.donut_timer, DONUT_TIME_DELAY_FRAMES, "the ctor's m_donutTimer");
    }

    /// `moveTowardsPositionTreads` (`Locomotor.cpp:1208-1209`) and `moveTowardsPositionWheels`
    /// (`:1353-1354`) divide by the raw `getBraking()`: `slowDownTime = actualSpeed / getBraking()`.
    /// The port clamped the divisor at 0.0001, which shrinks the slow-down distance and makes a unit
    /// with a small braking rate drive on instead of braking. Revert check: restore `.max(0.0001)` at
    /// either site and that site's assert fails.
    #[test]
    fn the_slow_down_distance_uses_the_raw_braking_rate() {
        // Braking 1e-5 per frame, 1 unit/frame: the treads slow-down distance is
        // (1/1.5)*(1/1e-5) = 66666.7 (the old clamp gave 6666.7), so 20000 units is inside it.
        let t = unit("Tread", LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.2, 1e-5, 0.1, true), 10.0, 5.0);
        let terrain = TerrainLogic::flat(400, 400, 0);
        let mut g = GameLogic::new(terrain.clone(), 1);
        let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let loco = g.objects.get(&id).unwrap().locomotor().unwrap();
        physics(&mut g, id).velocity = Coord3D::new(1.0, 0.0, 0.0);
        let goal = Coord3D::new(50.0 + 20000.0, 50.0, 0.0);
        let o = g.objects.get_mut(&id).unwrap();
        assert!(!move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 10, 1.0, -1.0, Some(20000.0)));
        assert!(g.objects[&id].physics.as_ref().unwrap().braking, "20000 < 66666.7 starts the treads brake");

        // Wheels: braking 1e-5, 0.9 units/frame -> slowDownDist (0.9/1.5)*(0.9/1e-5 + 1) + 0.9 =
        // 54001.5 (the old clamp gave 5401.5), so 20000 units is inside the effective distance.
        let w = unit("Wheel", LocomotorParams::from_engine_units(LocomotorKind::Wheels, 1.0, 0.0, 0.2, 1e-5, 0.1, true), 10.0, 5.0);
        let mut g2 = GameLogic::new(terrain.clone(), 1);
        let id2 = g2.create_object(&w, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let loco2 = g2.objects.get(&id2).unwrap().locomotor().unwrap();
        physics(&mut g2, id2).velocity = Coord3D::new(0.9, 0.0, 0.0);
        let o = g2.objects.get_mut(&id2).unwrap();
        assert!(!move_towards_position_force(o, &loco2, &goal, &terrain, &g2.pathfinder, 10, 1.0, -1.0, Some(20000.0)));
        assert!(g2.objects[&id2].physics.as_ref().unwrap().braking, "20000 < 54001.5 starts the wheels brake");
    }

    /// `moveTowardsPositionTreads` (`Locomotor.cpp:1228-1231`): while braking, `m_brakingFactor` is
    /// `(slowDownDist/onPathDistToGoal)^2` capped at `MAX_BRAKING_FACTOR` 5.0 (the port capped at 2.0).
    #[test]
    fn the_treads_braking_factor_is_the_squared_ratio_capped_at_five() {
        let (mut g, id, loco, goal, terrain) = treads_at(2.0);
        physics(&mut g, id).velocity = Coord3D::new(1.0, 0.0, 0.0);
        let o = g.objects.get_mut(&id).unwrap();
        assert!(!move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 10, 1.0, -1.0, Some(2.0)));
        let p = g.objects[&id].physics.as_ref().unwrap();
        assert!(p.braking, "2 < slowDownDist 6.667 starts the brake");
        // (6.667/2)^2 = 11.1 -> 5.0.
        assert_eq!(p.braking_factor, MAX_BRAKING_FACTOR);
    }

    /// `m_brakingFactor` is a persistent member (`Locomotor.h:445`): a frame that no longer brakes keeps
    /// the value the last braking frame left and uses it at `:1255`. The port rebuilt it from 1.0 every
    /// frame. Here 8 units out the treads kernel neither starts nor clears braking (8 > 6.667, 8 <
    /// `PATHFIND_CELL_SIZE_F`), so the stored 5.0 is what scales `-braking`.
    #[test]
    fn the_treads_braking_factor_persists_into_a_non_braking_frame() {
        let (mut g, id, loco, goal, terrain) = treads_at(8.0);
        {
            let p = physics(&mut g, id);
            p.velocity = Coord3D::new(1.0, 0.0, 0.0);
            p.braking = false;
            p.braking_factor = MAX_BRAKING_FACTOR;
            p.accel = Coord3D::ZERO;
        }
        let o = g.objects.get_mut(&id).unwrap();
        // Desired speed 0.5 while doing 1.0: `acceleration = -m_brakingFactor*getBraking()`.
        assert!(!move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 10, 0.5, -1.0, Some(8.0)));
        let p = g.objects[&id].physics.as_ref().unwrap();
        assert!(!p.braking);
        assert!((p.accel.x + 5.0 * 0.1).abs() < 1e-6, "accel {} is -5 * 0.1, not -1 * 0.1", p.accel.x);
    }

    /// `moveTowardsPositionWheels` (`Locomotor.cpp:1436-1441`) recomputes the squared ratio, clamps it
    /// and then forces `m_brakingFactor = 1.0f`, so wheels always brake with the plain rate.
    #[test]
    fn the_wheels_kernel_forces_the_braking_factor_to_one_while_braking() {
        let (mut g, id, loco, goal, terrain) = wheels_at(12.0);
        {
            let p = physics(&mut g, id);
            p.velocity = Coord3D::new(0.9549, 0.0, 0.0);
            p.braking = false;
            p.braking_factor = MAX_BRAKING_FACTOR;
        }
        let o = g.objects.get_mut(&id).unwrap();
        assert!(!move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 10, 1.0, -1.0, Some(12.0)));
        let p = g.objects[&id].physics.as_ref().unwrap();
        assert!(p.braking, "12 < the 12.52 2D slow-down distance");
        assert_eq!(p.braking_factor, 1.0);
    }

    /// The treads kernel has no donut brake: `m_donutTimer` is read in `moveTowardsPositionWheels` only
    /// (`Locomotor.cpp:1426-1432`). A stationary treads unit 8 units out (inside `DONUT_DISTANCE` 40 and
    /// under `PATHFIND_CELL_SIZE_F`, where the kernel's own far test cannot clear the flag) with an
    /// expired timer keeps driving instead of freezing on the `wasBraking` step.
    #[test]
    fn a_treads_unit_inside_donut_distance_does_not_brake_on_an_expired_timer() {
        let (mut g, id, loco, goal, terrain) = treads_at(8.0);
        {
            let p = physics(&mut g, id);
            p.donut_timer = 0;
            p.accel = Coord3D::ZERO;
        }
        let o = g.objects.get_mut(&id).unwrap();
        assert!(!move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 200, 1.0, -1.0, Some(8.0)));
        let o = &g.objects[&id];
        assert!(!o.physics.as_ref().unwrap().braking, "no donut brake outside the wheels kernel");
        assert_eq!(o.status_bits & status::IS_BRAKING, 0);
        assert!(o.physics.as_ref().unwrap().accel.x > 0.0, "it drives toward the goal");
    }

    /// `doLocomotor` (`AIUpdate.cpp:2214-2228`): `POSITION_ON_PATH` with no path does nothing — the
    /// module sleeps while waiting for the path — and only the `NONE` goal reaches
    /// `locoUpdate_maintainCurrentPosition`. The port maintained position in both cases, which clears
    /// `IS_BRAKING` and re-arms the donut timer on frames the engine leaves the locomotor alone.
    #[test]
    fn a_unit_waiting_for_a_path_is_not_maintained() {
        let (mut g, id, _loco, _goal, _terrain) = wheels_at(20.0);
        g.frame = 200;
        {
            let o = g.objects.get_mut(&id).unwrap();
            // A move in progress that is waiting for its path: goal type POSITION_ON_PATH, no path.
            o.ai.locomotor_goal_type = crate::ai::LocoGoalType::PositionOnPath;
            o.ai.waiting_for_path = true;
            let p = o.physics.as_mut().unwrap();
            p.donut_timer = 0;
            p.braking = true;
        }
        g.ai_do_locomotor(id);
        {
            let p = g.objects[&id].physics.as_ref().unwrap();
            assert_eq!(p.donut_timer, 0, "the waiting frame does not re-arm the donut timer");
            assert!(p.braking, "nor does it clear IS_BRAKING");
        }
        // The idle case does: `NONE` -> `locoUpdate_maintainCurrentPosition`.
        {
            let o = g.objects.get_mut(&id).unwrap();
            o.ai.locomotor_goal_type = crate::ai::LocoGoalType::None;
            o.ai.waiting_for_path = false;
        }
        g.ai_do_locomotor(id);
        {
            let p = g.objects[&id].physics.as_ref().unwrap();
            assert_eq!(p.donut_timer, 200 + DONUT_TIME_DELAY_FRAMES, "the idle frame re-arms it");
            assert!(!p.braking);
        }
    }

    /// `moveTowardsPositionThrust` reads `OBJECT_STATUS_BRAKING` (`Locomotor.cpp:1979`), the status bit
    /// set by the last `locoUpdate_moveTowardsPosition` (`:1111`), not the live `IS_BRAKING` flag: with
    /// the bit set it aims at the goal instead of along its velocity.
    #[test]
    fn the_thrust_kernel_reads_the_braking_status_bit() {
        fn thrust_unit() -> (GameLogic, ObjectId, LocomotorParams, TerrainLogic) {
            let mut loco = LocomotorParams::from_engine_units(LocomotorKind::Thrust, 2.0, 0.2, 0.2, 0.1, 0.1, false);
            loco.max_thrust_angle = 0.3;
            let t = unit("Missile", loco, 5.0, 4.0);
            let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
            // On the ground (`treatAsAirborne` false), so the kernel runs without
            // `AllowAirborneMotiveForce`; the kernel's z handling is not part of this test.
            let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
            let loco = g.objects.get(&id).unwrap().locomotor().unwrap();
            (g, id, loco, TerrainLogic::flat(300, 300, 0))
        }
        // The missile sits at (50,50) facing +x while moving +y; its goal is at (+10,-10) (14.14 units,
        // under `distToStopAtMaxSpeed` 20, so the top-level clear does not touch the flag). Aligned to
        // its velocity it turns toward +90 degrees; aligned to the goal it turns toward -45.
        let goal = Coord3D::new(60.0, 40.0, 0.0);
        let setup = |g: &mut GameLogic, id: ObjectId, flag: bool, bit: bool| {
            let o = g.objects.get_mut(&id).unwrap();
            o.angle = 0.0;
            if bit {
                o.status_bits |= status::IS_BRAKING;
            } else {
                o.status_bits &= !status::IS_BRAKING;
            }
            let p = o.physics.as_mut().unwrap();
            p.velocity = Coord3D::new(0.0, 1.0, 0.0);
            p.braking = flag;
        };
        // Status bit set, flag clear: aim at the goal (down-right from +y), turn 3*maxTurnRate = 0.3.
        let (mut g, id, loco, terrain) = thrust_unit();
        setup(&mut g, id, false, true);
        {
            let o = g.objects.get_mut(&id).unwrap();
            move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 10, 2.0, -1.0, Some(14.142));
        }
        let toward_goal = g.objects[&id].angle;
        assert!((toward_goal + 0.3).abs() < 1e-4, "aimed at the goal (status bit): {toward_goal}");
        // Flag set, status bit clear (what the port read): aim along the velocity instead.
        let (mut g, id, loco, terrain) = thrust_unit();
        setup(&mut g, id, true, false);
        {
            let o = g.objects.get_mut(&id).unwrap();
            move_towards_position_force(o, &loco, &goal, &terrain, &g.pathfinder, 10, 2.0, -1.0, Some(14.142));
        }
        let toward_velocity = g.objects[&id].angle;
        assert!((toward_velocity - 0.1).abs() < 1e-4, "the flag alone does not aim at the goal: {toward_velocity}");
    }

    /// A two-legs unit at `dist` from its goal along +x (the GLA worker's `FastHumanLocomotor` shape:
    /// speed 0.95, braking 0.1).
    fn legs_at(dist: f32) -> (GameLogic, ObjectId, LocomotorParams, Coord3D, TerrainLogic) {
        let t = unit("Legger", LocomotorParams::from_engine_units(LocomotorKind::Legs, 0.95, 0.0, 0.2, 0.1, 0.1, true), 10.0, 3.0);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let loco = g.objects.get(&id).unwrap().locomotor().unwrap();
        let goal = Coord3D::new(50.0 + dist, 50.0, 0.0);
        (g, id, loco, goal, TerrainLogic::flat(400, 400, 0))
    }

    /// `locoUpdate_maintainCurrentPosition` dispatches TWO_LEGS to `maintainCurrentPositionLegs`
    /// (`Locomotor.cpp:2486`), which is `maintainCurrentPositionOther` (`Locomotor.h:382`):
    /// `physics->scrubVelocity2D(0)` (`Locomotor.cpp:2601-2608`). The worker therefore stops dead when
    /// its move ends; the port braked with a motive force, which let it coast for frames after arrival
    /// (AN3 s1 worker 229 drifted 2.7 units past the C++ position, the first snapshot divergence at
    /// frame 240).
    #[test]
    fn a_legs_unit_scrubs_its_velocity_instead_of_coasting_at_the_end_of_a_move() {
        let (mut g, id, loco, _goal, terrain) = legs_at(20.0);
        g.frame = 200;
        // The last frame of a move: the kernel applied a motive force, so the object is driven.
        g.apply_motive_force(id, Coord3D::new(1.0, 0.0, 0.0));
        physics(&mut g, id).velocity = Coord3D::new(0.6722, 0.6722, 0.0);
        let before = g.objects[&id].position;
        let keep_calling = {
            let o = g.objects.get_mut(&id).unwrap();
            maintain_current_position_force(o, &loco, 200, -1.0, &terrain)
        };
        let o = &g.objects[&id];
        let p = o.physics.as_ref().unwrap();
        assert_eq!(p.velocity.x, 0.0, "scrubVelocity2D(0) clears the x velocity");
        assert_eq!(p.velocity.y, 0.0, "scrubVelocity2D(0) clears the y velocity");
        assert_eq!(o.position, before, "the unit stops in place, it does not move on");
        assert!(!keep_calling, "maintainCurrentPositionOther reports requiresConstantCalling = FALSE");
    }

    /// The control: HOVER keeps `maintainCurrentPositionHover` (`Locomotor.cpp:2567-2597`), which
    /// brakes with a motive force instead of scrubbing the velocity, and reports
    /// `requiresConstantCalling = TRUE`.
    #[test]
    fn a_hover_unit_brakes_with_a_motive_force() {
        let mut hover = LocomotorParams::from_engine_units(LocomotorKind::Hover, 0.95, 0.0, 0.2, 0.1, 0.1, true);
        hover.preferred_height = 10.0;
        let t = unit("Hoverer", hover, 10.0, 5.0);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 10.0), 0.0, 0);
        let loco = g.objects.get(&id).unwrap().locomotor().unwrap();
        g.frame = 200;
        g.apply_motive_force(id, Coord3D::new(1.0, 0.0, 0.0));
        physics(&mut g, id).velocity = Coord3D::new(0.6722, 0.6722, 0.0);
        let keep_calling = {
            let o = g.objects.get_mut(&id).unwrap();
            maintain_current_position_force(o, &loco, 200, -1.0, &TerrainLogic::flat(400, 400, 0))
        };
        let p = g.objects[&id].physics.as_ref().unwrap();
        assert!(p.velocity.x.abs() > 0.0, "hover brakes with a force, the velocity stays");
        assert!(keep_calling, "maintainCurrentPositionHover reports requiresConstantCalling = TRUE");
    }

    /// `moveTowardsPositionOther` reads `Coord3D dirToApplyForce = *obj->getUnitDirectionVector2D()`
    /// (`Locomotor.cpp:2372`) *before* `rotateTowardsPosition` (`:2389`) turns the body, and the motive
    /// force at the tail uses that copy (`:2422-2426`): the acceleration of a frame points along the
    /// heading the frame started with. Every other kernel fetches the direction after its turn.
    ///
    /// A hover unit at rest facing +x, ordered to a goal at 45°: the kernel turns it by `MaxTurnRate`
    /// but the force is still pure +x, so `accel.y` stays zero. Revert check: read the direction after
    /// the turn and the force picks up `sin(turn)` (AN3 s1 f1620: the Chinook's dock approach).
    #[test]
    fn a_hover_force_points_along_the_heading_the_frame_started_with() {
        // No z behaviour and no preferred height, so the z lift adds nothing to the x/y force.
        let mut hover = LocomotorParams::from_engine_units(LocomotorKind::Hover, 1.0, 0.0, 0.2, 0.1, 0.1, true);
        hover.behavior_z = ZBehavior::NoZMotiveForce;
        let t = unit("Hoverer", hover, 10.0, 5.0);
        let terrain = TerrainLogic::flat(400, 400, 0);
        let mut g = GameLogic::new(terrain.clone(), 1);
        let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let goal = Coord3D::new(150.0, 150.0, 0.0);
        {
            let o = g.objects.get_mut(&id).unwrap();
            assert_eq!(o.angle, 0.0, "facing +x");
            assert_eq!(o.physics.as_ref().unwrap().velocity, Coord3D::ZERO, "at rest: no friction term");
            move_towards_position_force(o, &hover, &goal, &terrain, &g.pathfinder, 0, 1.0, -1.0, Some(1000.0));
        }
        let o = g.objects.get(&id).unwrap();
        assert!(o.angle > 0.0, "the body turned toward the 45 degree goal");
        let p = o.physics.as_ref().unwrap();
        assert!(p.accel.x > 0.0, "the motive force is along the old +x heading");
        assert_eq!(p.accel.y, 0.0, "no force component along the new heading");
    }

    /// `maintainCurrentPositionOther` (`Locomotor.cpp:2616-2626`) and `maintainCurrentPositionHover`
    /// (`:2572-2576`) both start with `physics->setTurning(TURN_NONE)`: a unit whose move ends stops
    /// turning. (`maintainCurrentPositionWings` does the same, `:2536`.)
    #[test]
    fn an_idle_unit_stops_turning() {
        for kind in [LocomotorKind::Treads, LocomotorKind::Wheels, LocomotorKind::Hover] {
            let t = unit("Turner", LocomotorParams::from_engine_units(kind, 1.0, 0.0, 0.2, 0.1, 0.1, true), 10.0, 5.0);
            let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
            let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
            let loco = g.objects.get(&id).unwrap().locomotor().unwrap();
            g.objects.get_mut(&id).unwrap().physics.as_mut().unwrap().turning = 1;
            let o = g.objects.get_mut(&id).unwrap();
            maintain_current_position_force(o, &loco, 10, -1.0, &TerrainLogic::flat(400, 400, 0));
            assert_eq!(g.objects[&id].physics.as_ref().unwrap().turning, 0, "{kind:?} stops turning when idle");
        }
    }

    /// `NO_SLOW_DOWN_AS_APPROACHING_DEST` (`Locomotor.h:433-436`): every kernel wraps its slow-down in
    /// `!getFlag(NO_SLOW_DOWN_AS_APPROACHING_DEST)` (`Locomotor.cpp:2389-2396` for
    /// `moveTowardsPositionOther`, `:1215` treads, `:1414` wheels, `:1671` legs, `:1798` climb, `:1924`
    /// thrust). `computeAttackPath`'s contact-weapon arm sets it (`AIUpdate.cpp:1869-1872`) so a car bomb
    /// runs at full speed into its target instead of braking short of it. Revert check: drop the guard and
    /// the flagged unit brakes too.
    #[test]
    fn the_contact_approach_flag_disables_the_slow_down() {
        let mut hover = LocomotorParams::from_engine_units(LocomotorKind::Hover, 5.0, 1.0, 0.2, 1.0, 0.1, true);
        hover.behavior_z = ZBehavior::NoZMotiveForce;
        let t = unit("Hoverer", hover, 10.0, 5.0);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let goal = Coord3D::new(200.0, 50.0, 0.0);
        // Full speed 5 toward a goal 3 units along the path: `calcSlowDownDist(5, 1, 1)` is 8.4, so the
        // slow-down would clamp `goalSpeed` to `MinSpeed` 1 and brake. With the flag it does not.
        for (flag, brakes) in [(false, true), (true, false)] {
            let o = g.objects.get_mut(&id).unwrap();
            o.loco_rt.no_slow_down_as_approaching_dest = flag;
            o.angle = 0.0;
            let p = o.physics.as_mut().unwrap();
            p.velocity = Coord3D::new(5.0, 0.0, 0.0);
            p.accel = Coord3D::ZERO;
            move_towards_position_other(o, &hover, &goal, 3.0, 5.0, 0);
            let p = o.physics.as_ref().unwrap();
            assert_eq!(p.accel.x < 0.0, brakes, "flag {flag}: accel {:?}", p.accel);
        }
    }

    /// `AIUpdateInterface::chooseLocomotor` clears `NO_SLOW_DOWN_AS_APPROACHING_DEST` when the current
    /// locomotor changes (`AIUpdate.cpp:884-887`), and a set switch builds fresh `Locomotor`s
    /// (`chooseLocomotorSetExplicit`, `:830-841`). `rebuild_locomotors` is that switch:
    /// `LocoRuntime::default` is the fresh `m_flags = 0`.
    #[test]
    fn a_locomotor_set_change_clears_the_contact_approach_flag() {
        let t = unit("Bomber", LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.2, 0.1, 0.1, true), 10.0, 5.0);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        g.objects.get_mut(&id).unwrap().loco_rt.no_slow_down_as_approaching_dest = true;
        g.rebuild_locomotors(id, LocomotorSet::Normal);
        assert!(!g.objects[&id].loco_rt.no_slow_down_as_approaching_dest, "a fresh `Locomotor` has `m_flags = 0`");
    }

    /// `locoUpdate_maintainCurrentPosition` calls `handleBehaviorZ(obj, physics, m_maintainPos)`
    /// *after* the appearance switch, for every appearance ("but we do need to do this even if not
    /// moving", `Locomotor.cpp:2506-2508`), and its TRUE result forces `requiresConstantCalling`
    /// (`:2235-2238`). A TREADS locomotor with `Z_FIXED_ABSOLUTE_HEIGHT` therefore still snaps its z.
    #[test]
    fn a_non_flying_appearance_still_gets_handle_behavior_z() {
        let mut loco_params = LocomotorParams::from_engine_units(LocomotorKind::Treads, 1.0, 0.0, 0.2, 0.1, 0.1, true);
        loco_params.behavior_z = ZBehavior::FixedAbsoluteHeight;
        loco_params.preferred_height = 50.0;
        let t = unit("Climber", loco_params, 10.0, 5.0);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let loco = g.objects.get(&id).unwrap().locomotor().unwrap();
        let keep_calling = {
            let o = g.objects.get_mut(&id).unwrap();
            maintain_current_position_force(o, &loco, 10, -1.0, &TerrainLogic::flat(400, 400, 0))
        };
        assert_eq!(g.objects[&id].position.z, 50.0, "handleBehaviorZ runs for a treads appearance");
        assert!(keep_calling, "and its TRUE forces requiresConstantCalling");
    }

    /// `maintainCurrentPositionHover`'s `Real minSpeed = max(1.0E-10f, m_template->m_minSpeed)`
    /// (`Locomotor.cpp:2585`): with the template's `MinSpeed` above 1e-10 the brake aims for that
    /// speed instead of zero, so a slower unit is *accelerated*. (Shipped HOVER locomotors have
    /// `MinSpeed = 0`, which is why the engine only DEBUG_CRASHes on this data.)
    #[test]
    fn the_hover_brake_aims_for_the_templates_min_speed() {
        let mut hover = LocomotorParams::from_engine_units(LocomotorKind::Hover, 1.0, 0.5, 0.2, 0.1, 0.1, true);
        hover.preferred_height = 10.0;
        let t = unit("Hoverer", hover, 10.0, 5.0);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&t, Coord3D::new(50.0, 50.0, 10.0), 0.0, 0);
        let loco = g.objects.get(&id).unwrap().locomotor().unwrap();
        g.frame = 200;
        g.apply_motive_force(id, Coord3D::new(1.0, 0.0, 0.0));
        {
            let p = physics(&mut g, id);
            p.velocity = Coord3D::new(0.1, 0.0, 0.0);
            p.accel = Coord3D::ZERO;
        }
        let o = g.objects.get_mut(&id).unwrap();
        maintain_current_position_force(o, &loco, 200, -1.0, &TerrainLogic::flat(400, 400, 0));
        let p = g.objects[&id].physics.as_ref().unwrap();
        assert_eq!(
            p.accel.x, 0.0,
            "|MinSpeed 0.5 - 0.1| = 0.4 is not > MinSpeed, so no brake force: a hardcoded 1e-10 would brake here"
        );
    }
}

#[cfg(test)]
#[path = "locomotor_l154.rs"]
mod l154_tests;

/// Lane 168 (the R18 review, R18-7 and R18-12): the 3D turn's matrix and `handleBehaviorZ` per kind (`locomotor_l168.rs`).
#[cfg(test)]
#[path = "locomotor_l168.rs"]
mod l168_tests;

/// Wave 19 wrap-up: no arrival step in the move (`locomotor_w19.rs`).
#[cfg(test)]
#[path = "locomotor_w19.rs"]
mod w19_tests;

#[cfg(test)]
#[path = "locomotor_l163.rs"]
mod l163_tests;

/// Lane 165: the stunned return of the kernel (`locomotor_l165.rs`).
#[cfg(test)]
#[path = "locomotor_l165.rs"]
mod l165_tests;

#[cfg(test)]
mod damage_and_set_tests {
    //! The body damage state's movement penalty (`Locomotor::getMaxSpeedForCondition` and friends,
    //! `Locomotor.cpp:789-860`), the `OBJECT_STATUS_AIRBORNE_TARGET` update (`AIUpdate.cpp:2294-2300`) and the
    //! ctor draws of a locomotor-set switch (`Locomotor.cpp:673-675`).
    use super::*;
    use crate::body::{DamageInfo, DamageType};
    use crate::object::{status, ObjectId};
    use crate::template::{kind_of, LocomotorKind, LocomotorParams, LocomotorSet, ObjectTemplate, ZBehavior};
    use std::sync::Arc;

    /// A wheeled unit with distinct damaged values (speed 1.0 -> 0.5, turn 0.1 -> 0.05, accel 0.2 -> 0.1,
    /// lift 2.0 -> 1.0) and 100 health.
    fn damaged_unit() -> (GameLogic, ObjectId) {
        let mut l = LocomotorParams::from_engine_units(LocomotorKind::Wheels, 1.0, 0.0, 0.2, 0.05, 0.1, true);
        l.max_speed_damaged = 0.5;
        l.max_turn_rate_damaged = 0.05;
        l.acceleration_damaged = 0.1;
        l.lift = 2.0;
        l.lift_damaged = 1.0;
        let mut t = ObjectTemplate::new("Damageable");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 5.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.locomotor = Some(l);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        (g, id)
    }

    /// Damage `id` by `amount` (no armor, so the amount lands as health loss).
    fn hurt(g: &mut GameLogic, id: ObjectId, amount: f32) {
        let info = DamageInfo { source: ObjectId::INVALID, amount, kind: DamageType::SmallArms };
        g.objects.get_mut(&id).unwrap().body.as_mut().unwrap().attempt_damage(&info);
    }

    /// `IS_CONDITION_BETTER` (`BodyModule.h:103`) against `MOVEMENT_PENALTY_DAMAGE_STATE` (REALLYDAMAGED): the
    /// pristine and DAMAGED bodies keep the pristine values, REALLYDAMAGED and RUBBLE take the `*Damaged` ones.
    #[test]
    fn the_movement_penalty_starts_at_really_damaged() {
        let base = LocomotorParams::from_engine_units(LocomotorKind::Wheels, 1.0, 0.0, 0.2, 0.05, 0.1, true);
        let mut damaged = base;
        damaged.max_speed_damaged = 0.5;
        damaged.max_turn_rate_damaged = 0.05;
        damaged.acceleration_damaged = 0.1;
        damaged.lift_damaged = 1.0;
        for (state, want_damaged) in [
            (BodyDamageType::Pristine, false),
            (BodyDamageType::Damaged, false),
            (BodyDamageType::ReallyDamaged, true),
            (BodyDamageType::Rubble, true),
        ] {
            let mut l = damaged;
            apply_body_damage_state(&mut l, state, BodyDamageType::ReallyDamaged);
            if want_damaged {
                assert_eq!((l.max_speed, l.max_turn_rate, l.acceleration), (0.5, 0.05, 0.1), "{state:?} takes the damaged values");
            } else {
                assert_eq!((l.max_speed, l.max_turn_rate, l.acceleration), (1.0, 0.1, 0.2), "{state:?} keeps the pristine values");
            }
        }
        // The penalty state is data (`MovementPenaltyDamageState`): DAMAGED moves the line up one state.
        let mut l = damaged;
        apply_body_damage_state(&mut l, BodyDamageType::Damaged, BodyDamageType::Damaged);
        assert_eq!(l.max_speed, 0.5, "DAMAGED takes the damaged values when the penalty state is DAMAGED");
        let mut l = damaged;
        apply_body_damage_state(&mut l, BodyDamageType::Pristine, BodyDamageType::Damaged);
        assert_eq!(l.max_speed, 1.0, "PRISTINE stays better than a DAMAGED penalty state");
    }

    /// The object's locomotor follows its body: at 40% health (`BODY_DAMAGED`) the pristine speed still applies,
    /// at 5% (`BODY_REALLYDAMAGED`) the damaged speed, turn rate, acceleration and lift apply.
    /// Reverting the `apply_body_damage_state` call in `locomotor_now` makes the last asserts fail.
    #[test]
    fn the_locomotor_now_uses_the_damaged_values_at_really_damaged() {
        let (mut g, id) = damaged_unit();
        let full = g.locomotor_now(id).unwrap();
        assert_eq!((full.max_speed, full.max_turn_rate, full.acceleration, full.lift), (1.0, 0.1, 0.2, 2.0));

        hurt(&mut g, id, 60.0); // health 40 of 100: ratio 0.4, BODY_DAMAGED
        let damaged = g.locomotor_now(id).unwrap();
        assert_eq!(damaged.max_speed, 1.0, "BODY_DAMAGED is still better than the penalty state");

        hurt(&mut g, id, 35.0); // health 5 of 100: ratio 0.05, BODY_REALLYDAMAGED
        let really = g.locomotor_now(id).unwrap();
        assert_eq!(
            (really.max_speed, really.max_turn_rate, really.acceleration, really.lift),
            (0.5, 0.05, 0.1, 1.0),
            "BODY_REALLYDAMAGED applies the *Damaged values"
        );
        // The raw template fields are untouched: the damage state is applied per query.
        assert_eq!(g.objects[&id].locomotor().unwrap().max_speed, 1.0);
    }

    /// `MovementPenaltyDamageState` is read from the sim's rules (the `GameData` value): with `DAMAGED` a 40% body
    /// (`BODY_DAMAGED`) already takes the damaged values, while the default REALLYDAMAGED leaves it pristine.
    #[test]
    fn the_penalty_state_comes_from_the_rules() {
        let (mut g, id) = damaged_unit();
        hurt(&mut g, id, 60.0); // health 40 of 100: BODY_DAMAGED
        assert_eq!(g.locomotor_now(id).unwrap().max_speed, 1.0, "the default penalty state is REALLYDAMAGED");
        let rules = crate::energy::GameRules { movement_penalty_damage_state: BodyDamageType::Damaged, ..crate::energy::GameRules::default() };
        g.set_rules(rules);
        assert_eq!(g.locomotor_now(id).unwrap().max_speed, 0.5, "DAMAGED is the penalty state from the rules");
    }

    /// `Object::locomotor` clamps the damaged lift to `m_maxLift` as the pristine one (`Locomotor.cpp:852-861`).
    #[test]
    fn the_damaged_lift_is_clamped_to_the_runtime_max_lift() {
        let (mut g, id) = damaged_unit();
        g.objects.get_mut(&id).unwrap().loco_rt.max_lift = 0.25;
        let l = g.objects[&id].locomotor().unwrap();
        assert_eq!((l.lift, l.lift_damaged), (0.25, 0.25));
    }

    /// `OBJECT_STATUS_AIRBORNE_TARGET` (`AIUpdate.cpp:2296`): after the frame's movement the bit is set while the
    /// object is more than `AirborneTargetingHeight` above the terrain, and cleared once it is not. A
    /// no-physics flier with `AbsoluteHeight` 100 stays up at z 50 (height 50 > 30); one held at z 5 is not.
    /// Reverting the `update_airborne_target_status` call in `ai_do_locomotor` makes the first asserts fail.
    #[test]
    fn the_airborne_target_bit_follows_the_height_above_terrain() {
        // A no-physics flier holding `preferred` above the ground (`AbsoluteHeight`), with `AirborneTargetingHeight` 30.
        fn flier(preferred: f32) -> Arc<ObjectTemplate> {
            let mut l = LocomotorParams::from_engine_units(LocomotorKind::Wings, 1.0, 0.0, 0.2, 0.05, 0.1, false);
            l.behavior_z = ZBehavior::AbsoluteHeight;
            l.preferred_height = preferred;
            l.airborne_targeting_height = 30.0;
            let mut t = ObjectTemplate::new("Flier");
            t.kind_of = kind_of::AIRCRAFT | kind_of::SELECTABLE;
            t.geometry.major_radius = 5.0;
            t.max_health = 100.0;
            t.initial_health = 100.0;
            t.locomotor = Some(l);
            Arc::new(t)
        }
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);

        // 50 above the ground, climbing toward 100: still above 30 after the frame.
        let up = g.create_object(&flier(100.0), Coord3D::new(50.0, 50.0, 50.0), 0.0, 0);
        g.ai_do_locomotor(up);
        assert_ne!(g.objects[&up].status_bits & status::AIRBORNE_TARGET, 0, "50 above the ground is airborne");

        // 5 above the ground, holding 5: the bit is cleared even though it starts set.
        let low = g.create_object(&flier(5.0), Coord3D::new(150.0, 50.0, 5.0), 0.0, 0);
        g.objects.get_mut(&low).unwrap().status_bits |= status::AIRBORNE_TARGET;
        g.ai_do_locomotor(low);
        assert_eq!(g.objects[&low].status_bits & status::AIRBORNE_TARGET, 0, "5 above the ground clears the bit");
    }

    /// `chooseLocomotorSet` (`AIUpdate.cpp:809-841`): a real switch builds the new `Locomotor`s and their ctor draws
    /// `GameLogicRandomValueReal(-PI/6, PI/6)`, `GameLogicRandomValueReal(0.8, 1.2)`, `GameLogicRandomValue(0, 1)`
    /// (`Locomotor.cpp:673-675`), in that order. Choosing the set already in use draws nothing.
    /// Reverting the `draw_locomotor_ctor_rng` call in `set_locomotor_set` makes the first assert fail.
    #[test]
    fn a_locomotor_set_switch_redraws_the_ctor_rng_once() {
        let normal = LocomotorParams::from_engine_units(LocomotorKind::Wheels, 1.0, 0.0, 0.2, 0.05, 0.1, true);
        let panic = LocomotorParams::from_engine_units(LocomotorKind::Wheels, 2.0, 0.0, 0.2, 0.05, 0.1, true);
        let mut t = ObjectTemplate::new("Runner");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 5.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.locomotor = Some(normal);
        t.locomotor_sets.insert(LocomotorSet::Panic, panic);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);

        let mut expect = g.rng.clone();
        expect.value_real(-PI / 6.0, PI / 6.0);
        expect.value_real(0.8, 1.2);
        expect.value(0, 1);
        assert!(g.set_locomotor_set(id, LocomotorSet::Panic));
        assert_eq!(g.rng, expect, "one switch draws the ctor values in order");

        let before = g.rng.clone();
        assert!(g.set_locomotor_set(id, LocomotorSet::Panic));
        assert_eq!(g.rng, before, "the set already in use is not rebuilt");
    }

    /// The AI's set chooser (`AIUpdateInterface::chooseLocomotorSet`, `AIUpdate.cpp:809-841`, ported as
    /// `jet_states::ai_choose_locomotor_set`) rebuilds the `Locomotor`s on a real switch exactly like
    /// `set_locomotor_set`: the same three ctor draws, and none for the set already in use.
    /// Reverting the `rebuild_locomotors` call in `ai_choose_locomotor_set` makes the first assert fail.
    #[test]
    fn the_ai_set_chooser_redraws_the_ctor_rng_on_a_real_switch_only() {
        let normal = LocomotorParams::from_engine_units(LocomotorKind::Wheels, 1.0, 0.0, 0.2, 0.05, 0.1, true);
        let panic = LocomotorParams::from_engine_units(LocomotorKind::Wheels, 2.0, 0.0, 0.2, 0.05, 0.1, true);
        let mut t = ObjectTemplate::new("Chooser");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 5.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.locomotor = Some(normal);
        t.locomotor_sets.insert(LocomotorSet::Panic, panic);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);

        let mut expect = g.rng.clone();
        expect.value_real(-PI / 6.0, PI / 6.0);
        expect.value_real(0.8, 1.2);
        expect.value(0, 1);
        assert!(g.ai_choose_locomotor_set(id, LocomotorSet::Panic));
        assert_eq!(g.rng, expect, "a real switch draws the ctor values once");
        assert_eq!(g.objects[&id].locomotor_set, LocomotorSet::Panic);

        let before = g.rng.clone();
        assert!(g.ai_choose_locomotor_set(id, LocomotorSet::Panic));
        assert_eq!(g.rng, before, "the set already in use is not rebuilt");
    }

    /// `chooseLocomotorSetExplicit` (`AIUpdate.cpp:830-841`) builds one `Locomotor` per resolved entry of the set, and
    /// each ctor draws its three values (`Locomotor.cpp:673-675`). A switch to a two-entry set (the GLA combat bike's
    /// `SET_SLUGGISH`, Ground then Cliff) draws two triples.
    /// Reverting the per-entry loop in `rebuild_locomotors` (one triple) makes the assert fail.
    #[test]
    fn a_set_switch_draws_one_ctor_triple_per_entry() {
        let normal = LocomotorParams::from_engine_units(LocomotorKind::Wheels, 1.0, 0.0, 0.2, 0.05, 0.1, true);
        let panic = LocomotorParams::from_engine_units(LocomotorKind::Wheels, 2.0, 0.0, 0.2, 0.05, 0.1, true);
        let mut t = ObjectTemplate::new("TwoEntries");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 5.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.locomotor = Some(normal);
        t.locomotor_sets.insert(LocomotorSet::Normal, normal);
        t.locomotor_sets.insert(LocomotorSet::Panic, panic);
        t.locomotor_entries.insert(LocomotorSet::Normal, 1);
        t.locomotor_entries.insert(LocomotorSet::Panic, 2);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);

        let mut expect = g.rng.clone();
        for _ in 0..2 {
            expect.value_real(-PI / 6.0, PI / 6.0);
            expect.value_real(0.8, 1.2);
            expect.value(0, 1);
        }
        assert!(g.set_locomotor_set(id, LocomotorSet::Panic));
        assert_eq!(g.rng, expect, "one triple per entry of the new set");
    }

    /// `Locomotor::Locomotor` resets `m_preferredHeight` to the template value (`Locomotor.cpp:659-670`), so a set
    /// switch drops a runtime preferred height (`setPreferredHeight`, here the chinook's building height,
    /// `ChinookAIUpdate.cpp:146`).
    /// Reverting the `preferred_height_override = None` in `rebuild_locomotors` makes the assert fail.
    #[test]
    fn a_set_switch_resets_the_runtime_preferred_height() {
        let normal = LocomotorParams::from_engine_units(LocomotorKind::Wheels, 1.0, 0.0, 0.2, 0.05, 0.1, true);
        let panic = LocomotorParams::from_engine_units(LocomotorKind::Wheels, 2.0, 0.0, 0.2, 0.05, 0.1, true);
        let mut t = ObjectTemplate::new("Hoverer");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 5.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.locomotor = Some(normal);
        t.locomotor_sets.insert(LocomotorSet::Panic, panic);
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        g.objects.get_mut(&id).unwrap().preferred_height_override = Some(40.0);

        assert!(g.set_locomotor_set(id, LocomotorSet::Panic));
        assert_eq!(g.objects[&id].preferred_height_override, None, "the new locomotors start at the template height");
    }

    /// `JetAIUpdate::onObjectCreated` (JetAIUpdate.cpp:1939-1944): the base constructor chose NORMAL (its draws come
    /// before the object is inserted, logic.rs), then `chooseLocomotorSet(TAXIING)` builds one `Locomotor` per taxi
    /// entry and draws each once (AIUpdate.cpp:830-841, Locomotor.cpp:673-675). With two taxi entries the choice draws
    /// six values, no more. Reverting the `rebuild_locomotors` draws, or adding the old `ai_creation_locomotor_draws`
    /// call back to `jet_on_created`, makes the assert fail.
    #[test]
    fn a_jet_taxi_choice_draws_one_ctor_triple_per_taxi_entry() {
        let f = 30.0f32;
        let wings = LocomotorParams::from_engine_units(LocomotorKind::Wings, 175.0 / f, 60.0 / f, 120.0 / (f * f), 10.0 / (f * f), 2.0, false);
        let taxi = LocomotorParams::from_engine_units(LocomotorKind::Treads, 50.0 / f, 0.0, 100.0 / (f * f), 999_999.0 / (f * f), 3.0, true);
        let mut t = ObjectTemplate::new("TaxiingJet");
        t.kind_of = kind_of::AIRCRAFT | kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 14.0;
        t.max_health = 160.0;
        t.initial_health = 160.0;
        t.locomotor = Some(wings);
        t.locomotor_sets.insert(LocomotorSet::Normal, wings);
        t.locomotor_sets.insert(LocomotorSet::Taxiing, taxi);
        t.locomotor_entries.insert(LocomotorSet::Normal, 1);
        t.locomotor_entries.insert(LocomotorSet::Taxiing, 2);
        t.jet = Some(crate::modules2::JetAiTemplate {
            needs_runway: false,
            keeps_parking_space_when_airborne: false,
            takeoff_pause: 0,
            return_to_base_idle_time: 0,
            out_of_ammo_damage_per_second: 0.0,
            takeoff_dist_for_max_lift: 0.0,
            min_height: 0.0,
            parking_offset: 0.0,
            sneaky_offset_when_attacking: 0.0,
            attacking_loco: LocomotorSet::Normal,
            returning_loco: LocomotorSet::Normal,
            attack_loco_persist_time: 0,
            attackers_miss_persist_time: 0,
        });
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let id = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        assert_eq!(g.objects[&id].locomotor_set, LocomotorSet::Taxiing, "the creation choice parks the jet on taxi");

        // Put the jet back on NORMAL and run the creation choice again, with the RNG cloned right before it.
        g.objects.get_mut(&id).unwrap().locomotor_set = LocomotorSet::Normal;
        let mut expect = g.rng.clone();
        for _ in 0..2 {
            expect.value_real(-PI / 6.0, PI / 6.0);
            expect.value_real(0.8, 1.2);
            expect.value(0, 1);
        }
        g.jet_on_created(id);
        assert_eq!(g.rng, expect, "the taxi choice draws one triple per taxi entry");
    }
}
