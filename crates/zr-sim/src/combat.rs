//! Attack orders, weapon firing, radius damage, projectiles and idle auto-acquire.
//!
//! This is the sim's reduction of three original pieces:
//!
//! * `AIAttackState` and its sub-machine (AI/AIStates.cpp: `AttackStateMachine` with the
//!   `CHASE_TARGET` / `AIM_AT_TARGET` / `FIRE_WEAPON` states). One state-machine transition costs
//!   one frame, as in the original, so with a short `DelayBetweenShots` the shot cadence is
//!   limited to one shot every 2 frames (aim success, then fire).
//! * `Weapon::privateFireWeapon` / `WeaponTemplate::fireWeaponTemplate` /
//!   `WeaponTemplate::dealDamageInternal` (Object/Weapon.cpp), including the delayed-damage list of
//!   `WeaponStore::update` and the scatter RNG draws.
//! * `WeaponSet::chooseBestWeaponForTarget` (Object/WeaponSet.cpp), `PREFER_MOST_DAMAGE`.
//!
//! Where it differs (documented, all TODO):
//! * Attack processing is a per-frame pass over objects that own weapons (in id order), run after
//!   the command list and before the sleepy updates, instead of an `AIUpdate` state machine on the
//!   sleepy schedule. Movement still goes through the normal `MoveTo` locomotion.
//! * Turrets: a single turret angle in world space that slews at `TurretTurnRate`; units without a
//!   turret rotate their body at the locomotor turn rate (immobile ones snap). The turret
//!   recentres instantly when the unit is not attacking. Pitch is not simulated.
//! * Projectiles are lightweight entities (not `Object`s; `projectile.rs`): `DumbProjectileBehavior`'s
//!   Bezier arc with its physics/friction and `MissileAIUpdate`'s launch/ignition/attack/flight states
//!   are ported there, but a projectile has no `ActiveBody`, no armed-object collision and no
//!   `projectileNowJammed` path.
//! * Distances are 2D (`ATTACK_RANGE_IS_2D`); radius damage uses the 2D partition (the original's
//!   `DAMAGE_RANGE_CALC_TYPE` is 3D bounding sphere).
//! * Relationships come from a per-player table (`Player::getRelationship` / `Object::getRelationship`):
//!   a directed override map plus an explicit neutral player, defaulting to `Neutral` when unset.
//! * The chase uses the ground pathfinder and re-paths with the original's tests: the
//!   `MIN_RECOMPUTE_TIME` gate and `isSamePosition`'s distance-scaled tolerance
//!   (`AIAttackApproachTargetState::computePath` / `AIInternalMoveToState::update`).

use crate::ai::FAST_AS_POSSIBLE;
use crate::armor::ArmorTemplate;
use crate::body::DamageInfo;
use crate::crc::Crc;
use crate::damage::{DamageType, DeathType, Relationship, HUGE_DAMAGE_AMOUNT};
use crate::logic::GameLogic;
use crate::math::{acos, angle_diff, atan2, ceil, cos, normalize_angle, sin, sqrt, Coord3D, TWO_PI};
use crate::object::{Object, ObjectId, PlayerIndex};
use crate::partition::DistanceMode;
use crate::pathfind::PATHFIND_CELL_SIZE_F;
use crate::projectile::{
    arrived_at_goal, bounding_sphere_gap, compute_flight_path, forward_speed_3d, missile_thrust_direction, physics_friction_accel, thrust_is_applied,
    DumbProjectileParams,
    MissileFlight, MissileState, NeutronParams, ProjectileBehavior, MOTIVE_FRAMES, PHYSICS_DEFAULT_FRICTION,
};
use crate::shroud::ObjectShroudStatus;
use crate::template::{kind_of, ObjectTemplate};
use crate::weapon::{affects, anti, WeaponSet, WeaponStatus, WeaponTemplate, WEAPON_SLOT_COUNT};
use std::collections::BTreeMap;
use std::sync::Arc;

/// `REL_THRESH` in `AIAttackAimAtTargetState` / `TurretAIAimTurretState` (about 2 degrees).
pub(crate) const AIM_REL_THRESH: f32 = 0.035;
/// `AIStateMachine.h`: `MIN_REPATH_TIME` / `AIAttackApproachTargetState::MIN_RECOMPUTE_TIME`, the
/// minimum number of frames between chase re-paths.
const MIN_RECOMPUTE_TIME: u32 = 10;
impl GameLogic {
    /// `Thing::isSignificantlyAboveTerrain` (`Thing.cpp:338-343`): the height above the terrain is more than
    /// `-(3*3) * gravity`, the distance a falling object covers in three frames. The C++ height is
    /// `Object::calculateHeightAboveTerrain` (`Object.cpp:2820-2825`): above the surface of the object's layer
    /// (`getLayerHeight(x, y, m_layer)`). The port has no per-object layer, so the nearest surface stands in for it:
    /// a unit standing on a bridge deck measures from the deck (its layer is the bridge's in C++), a flier over open
    /// ground from the ground. Callers: `Weapon.cpp:1456` (radius damage), `VeterancyCrateCollide.cpp:83` (the
    /// collector, through `collide.rs`).
    pub(crate) fn significantly_above_terrain(&self, id: ObjectId) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        let surface = self.terrain.surface_height(o.position.x, o.position.y, o.position.z);
        o.position.z - surface > -(3.0 * 3.0) * self.rules.gravity
    }
}

/// What an attack order targets (`aiAttackObject` / `aiAttackPosition`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AttackTarget {
    Object(ObjectId),
    Ground(Coord3D),
}

/// The attack sub-machine's states (`AttackStateMachine` in `AIStates.cpp:265`): `CHASE_TARGET` is
/// `AIAttackPursueTargetState` when the attacker can catch a fleeing victim and
/// `AIAttackApproachTargetState` otherwise; `APPROACH_TARGET` is the approach state; `AIM_AT_TARGET`
/// is the default state (`AttackStateMachine` defines it first); `FIRE_WEAPON` fires once and drops
/// back to aim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttackPhase {
    /// `CHASE_TARGET` -> `AIAttackPursueTargetState`: turreted, matching a fleeing victim's speed.
    Pursue,
    /// `CHASE_TARGET`/`APPROACH_TARGET` -> `AIAttackApproachTargetState`: path to a firing position.
    Approach,
    /// `AIM_AT_TARGET` (the default state).
    Aim,
    /// `FIRE_WEAPON`: pre-attack started, waiting for the weapon to be ready.
    Fire,
}

/// `AttackExitConditionsInterface` / `ExitConditions` (`AIGuard.h:97`, `AIGuard.cpp:114`): the
/// optional conditions under which an `AIAttackState` reports success. `shouldExit` is checked at
/// the top of every update (`AIStates.cpp:5604`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct AttackExitConditions {
    /// `ATTACK_ExitIfNoUnitFound`: exit when there is no goal object.
    pub exit_if_no_unit_found: bool,
    /// `ATTACK_ExitIfExpiredDuration`: exit once the frame reaches `give_up_frame`.
    pub exit_if_expired_duration: bool,
    /// `ATTACK_ExitIfOutsideRadius`: exit when the goal object leaves the guarded area.
    pub exit_if_outside_radius: bool,
    pub center: Coord3D,
    pub radius_sqr: f32,
    pub give_up_frame: u32,
}

impl AttackExitConditions {
    /// `ExitConditions::shouldExit` (`AIGuard.cpp:114`).
    pub fn should_exit(&self, goal_object: Option<Coord3D>, frame: u32) -> bool {
        let Some(goal) = goal_object else {
            return self.exit_if_no_unit_found;
        };
        if self.exit_if_expired_duration && frame >= self.give_up_frame {
            return true;
        }
        if self.exit_if_outside_radius {
            let dx = goal.x - self.center.x;
            let dy = goal.y - self.center.y;
            if dx * dx + dy * dy > self.radius_sqr {
                return true;
            }
        }
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttackOrder {
    pub target: AttackTarget,
    pub phase: AttackPhase,
    /// `getNumConsecutiveShotsFiredAtTarget`.
    pub consecutive_shots: u32,
    /// Victim position the current chase path was computed for (`m_prevVictimPos`).
    chase_ref: Option<Coord3D>,
    /// `m_approachTimestamp` + `MIN_RECOMPUTE_TIME`: the frame the next chase re-path may run.
    repath_allowed_at: u32,
    /// `AIAttackApproachTargetState` / `AIAttackPursueTargetState::m_waitingForPath`
    /// (`AIStates.cpp:2427,2862`): the attack states own this flag. `computePath` sets it when it
    /// requests a path and only the state consumes it (`AIStates.cpp:2480,2504,2904`), so the
    /// pathfinder clearing the `AIUpdateInterface` flag in `doPathfind` (`AIUpdate.cpp:402`) does
    /// not end the wait. Distinct from `AiState::waiting_for_path`.
    pub(crate) waiting_for_path: bool,
    /// Weapon slot used for the most recent frame (`m_curWeapon`).
    pub slot: usize,
    /// `Weapon::getMaxShotCount` (`AttackNugget` sets it; `None` = unlimited).
    pub max_shots: Option<u32>,
    /// The slot this order was started with (`AttackOrder::with_limit`). It is a copy for the
    /// `m_lockedWeaponOnEnter` read (`ai_states.rs`), not the lock: the temporary lock is
    /// `WeaponSet::temporary_lock`, set by [`GameLogic::set_weapon_lock`] and released by the rules in
    /// `WeaponSet::releaseWeaponLock`. Nothing here releases or reads it for weapon choice.
    pub locked_slot: Option<usize>,
    /// `MSG_DO_FORCE_ATTACK_OBJECT`: ignore the ally relationship check.
    pub force: bool,
    /// `AIAttackState::m_attackParameters`.
    pub exit: AttackExitConditions,
    /// `AIAttackState::m_follow`: keep pursuing a live mobile victim even after it leaves range.
    pub follow: bool,
}

impl AttackOrder {
    pub fn new(target: AttackTarget) -> Self {
        Self {
            target,
            phase: AttackPhase::Aim,
            consecutive_shots: 0,
            chase_ref: None,
            repath_allowed_at: 0,
            waiting_for_path: false,
            slot: 0,
            max_shots: None,
            locked_slot: None,
            force: false,
            exit: AttackExitConditions::default(),
            follow: false,
        }
    }
    /// `MSG_DO_FORCE_ATTACK_OBJECT`: attack even an ally / neutral target.
    pub fn new_force(target: AttackTarget) -> Self {
        Self { force: true, ..Self::new(target) }
    }
    /// `AttackNugget`: a weapon-locked attack with a shot limit (`number_of_shots < 0` = unlimited).
    pub fn with_limit(target: AttackTarget, number_of_shots: i32, weapon_slot: usize) -> Self {
        Self {
            max_shots: if number_of_shots < 0 { None } else { Some(number_of_shots as u32) },
            locked_slot: Some(weapon_slot),
            ..Self::new(target)
        }
    }
    /// `AIGuardInnerState`/`AIGuardOuterState`/`AIGuardAttackAggressorState`: an attack with guard
    /// exit conditions (`AIGuard.cpp:431/560/861`).
    pub fn with_exit(mut self, exit: AttackExitConditions, follow: bool) -> Self {
        self.exit = exit;
        self.follow = follow;
        self
    }
}

/// Per-object combat state: weapons, current attack order, turret, idle scan timer, death cause.
#[derive(Clone, Debug, Default)]
pub struct ObjectCombat {
    pub weapons: WeaponSet,
    pub attack: Option<AttackOrder>,
    /// `AIAttackState::m_victimTeam` (AIStates.cpp:5459, :5559): the victim and its team when the attack began. The
    /// update checks a changed team against it (AIStates.cpp:5633-5669). Keyed by the victim, so a new attack restarts it.
    pub attack_victim_team: Option<(ObjectId, Option<String>)>,
    /// World-space turret heading (radians).
    pub turret_angle: f32,
    /// World-space turret pitch (radians, `TurretAI::m_pitch`).
    pub turret_pitch: f32,
    /// The `TurretAI` state machine, if the template has a `Turret` block.
    pub turret: Option<crate::turret_ai::TurretMachine>,
    /// `WeaponSet::m_curWeapon` (`WeaponSet.cpp:178`, `PRIMARY_WEAPON` at construction): the slot the
    /// turret's `AIM`/`FIRE` and `isOwnersCurWeaponOnTurret` read. The attack pass and the turret's own
    /// idle-mood choice keep it current.
    pub current_weapon_slot: usize,
    /// `AIUpdateInterface::m_nextMoodCheckTime` (AIUpdate.cpp:256): the frame of the next mood (auto-acquire)
    /// check. Set by `resetNextMoodCheckTime` (idle entry), `wakeUpAndAttemptToTarget`, `setNextMoodCheckTime`
    /// and by each check that runs (`getNextMoodTarget`, AIUpdate.cpp:4608).
    pub next_mood_check: u32,
    /// `AIUpdateInterface::m_randomlyOffsetMoodCheck` (AIUpdate.cpp:278): the next due check is offset by
    /// `GameLogicRandomValue(-rate/2, rate/2)` (AIUpdate.cpp:4612), then the flag is cleared.
    pub randomly_offset_mood_check: bool,
    /// Set when the object is killed; consumed by the (not yet ported) die modules.
    pub death: Option<DeathType>,
    /// `AIAttackMoveState`: keep auto-acquiring while moving.
    pub attack_move: bool,
    /// `AIGuardState`: hold this point/object and engage nearby enemies.
    pub guard: Option<crate::orders::GuardTarget>,
    pub guard_mode: crate::orders::GuardMode,
    /// `MSG_DO_FORCE_ATTACK_OBJECT`: the current attack order ignores the ally check.
    pub force_attack: bool,
    /// `AIHuntState`: keep acquiring the closest enemy anywhere (ignoring fog) and attack it.
    pub hunt: bool,
    /// `AIAttackAreaState`: the guarded area (centre, radius) to hunt within.
    pub hunt_area: Option<(Coord3D, f32)>,
    /// `Object::m_weaponBonusCondition` (`WeaponBonusConditionType` bits, `bonus::condition`), except the
    /// `PLAYER_UPGRADE` bit (on `Object`) and the veterancy bits (derived). Set and cleared by the
    /// `FiringTracker` (`CONTINUOUS_FIRE_*`, `TARGET_FAERIE_FIRE`) and the `TempWeaponBonusHelper`.
    pub weapon_bonus_conditions: u32,
    /// The state of the object's helper modules (`object_helpers.rs`).
    pub helpers: crate::object_helpers::HelperState,
}

impl ObjectCombat {
    /// Build the initial combat state from the template's normal weapon set. `turret_next_idle_scan`
    /// is the creation-time `TurretAIIdleState::resetIdleScan` frame (drawn by the caller at the
    /// `TurretAI` constructor's position in the module-construction order).
    pub fn new(t: &ObjectTemplate, angle: f32, now: u32, turret_next_idle_scan: Option<u32>) -> Self {
        let weapons = t.combat.weapon_set.as_ref().map_or_else(WeaponSet::default, |s| WeaponSet::from_template(s, now));
        let turret = turret_next_idle_scan.map(|scan| crate::turret_ai::TurretMachine::new(&t.combat, angle, scan));
        Self {
            weapons,
            attack: None,
            attack_victim_team: None,
            turret_angle: crate::math::normalize_angle(angle + t.combat.turret_natural_angle),
            turret_pitch: t.combat.turret_natural_pitch,
            turret,
            current_weapon_slot: 0,
            // `AIUpdateInterface` constructor (AIUpdate.cpp:256, :278).
            next_mood_check: 0,
            randomly_offset_mood_check: false,
            death: None,
            attack_move: false,
            guard: None,
            guard_mode: crate::orders::GuardMode::Normal,
            force_attack: false,
            hunt: false,
            hunt_area: None,
            weapon_bonus_conditions: 0,
            helpers: Default::default(),
        }
    }

    /// `WeaponSet::updateWeaponSet` for a set change (`WeaponSet.cpp:320-357`): rebuild the weapons
    /// from the newly selected set and reset `m_curWeapon` to `PRIMARY_WEAPON`. The reset is
    /// deliberate even when the primary slot is empty ("always start with the cur weapon being
    /// primary, even if there is no primary weapon", `:349-357`). `WeaponLockSharedAcrossSets` is not
    /// modelled; every shipped weapon set takes the reset. The caller releases the locks (the
    /// `releaseWeaponLock(LOCKED_PERMANENTLY)` branch, `:330-335`).
    pub fn rebuild_weapon_set(&mut self, set: &crate::weapon::WeaponSetTemplate, now: u32) {
        self.weapons = WeaponSet::from_template(set, now);
        self.current_weapon_slot = 0;
    }

    pub(crate) fn crc(&self, c: &mut Crc) {
        self.weapons.crc(c);        match &self.attack {
            Some(a) => {
                c.add_bool(true);
                match a.target {
                    AttackTarget::Object(id) => {
                        c.add_u32(1);
                        c.add_u32(id.0);
                    }
                    AttackTarget::Ground(p) => {
                        c.add_u32(2);
                        c.add_coord(&p);
                    }
                }
                c.add_u32(a.phase as u32);
                c.add_u32(a.consecutive_shots);
                c.add_u32(a.slot as u32);
                match &a.chase_ref {
                    Some(p) => {
                        c.add_bool(true);
                        c.add_coord(p);
                    }
                    None => c.add_bool(false),
                }
                c.add_u32(a.repath_allowed_at);
                c.add_bool(a.waiting_for_path);
                c.add_u32(a.max_shots.map_or(u32::MAX, |m| m));
                c.add_u32(a.locked_slot.map_or(u32::MAX, |s| s as u32));
                c.add_bool(a.force);
                c.add_bool(a.follow);
                c.add_bool(a.exit.exit_if_no_unit_found);
                c.add_bool(a.exit.exit_if_expired_duration);
                c.add_bool(a.exit.exit_if_outside_radius);
                c.add_coord(&a.exit.center);
                c.add_f32(a.exit.radius_sqr);
                c.add_u32(a.exit.give_up_frame);
            }
            None => c.add_bool(false),
        }
        c.add_f32(self.turret_angle);
        c.add_f32(self.turret_pitch);
        c.add_u32(self.current_weapon_slot as u32);
        match &self.turret {
            Some(t) => {
                c.add_bool(true);
                c.add_u32(t.state as u32);
                c.add_bool(t.enabled);
                match t.target {
                    crate::turret_ai::TurretTarget::None => c.add_u32(0),
                    crate::turret_ai::TurretTarget::Object(id) => {
                        c.add_u32(1);
                        c.add_u32(id.0);
                    }
                    crate::turret_ai::TurretTarget::Position(p) => {
                        c.add_u32(2);
                        c.add_coord(&p);
                    }
                }
                c.add_bool(t.force_attacking);
                c.add_bool(t.target_was_set_by_idle_mood);
                c.add_u32(t.victim_initial_player.map_or(u32::MAX, |p| p as u32));
                c.add_u32(t.sleep_until);
                c.add_u32(t.state_sleep_until);
                c.add_u32(t.next_idle_scan);
                c.add_f32(t.desired_angle);
                c.add_u32(t.hold_timestamp);
                c.add_bool(t.did_fire);
                c.add_u32(t.continuous_fire_expiration);
                c.add_u32(t.enable_sweep_until);
                c.add_bool(t.positive_sweep);
                c.add_bool(t.fires_while_turning);
                c.add_f32(t.last_angle);
                c.add_bool(t.rotating);
            }
            None => c.add_bool(false),
        }
        c.add_u32(self.next_mood_check);
        c.add_bool(self.randomly_offset_mood_check);
        c.add_u32(self.death.map_or(0xffff, |d| d as u32));
        c.add_bool(self.attack_move);
        match self.guard {
            Some(crate::orders::GuardTarget::Point(p)) => {
                c.add_u32(1);
                c.add_coord(&p);
            }
            Some(crate::orders::GuardTarget::Object(id)) => {
                c.add_u32(2);
                c.add_u32(id.0);
            }
            None => c.add_u32(0),
        }
        c.add_u32(self.guard_mode as u32);
        c.add_bool(self.force_attack);
        c.add_bool(self.hunt);
        match &self.hunt_area {
            Some((p, r)) => {
                c.add_bool(true);
                c.add_coord(p);
                c.add_f32(*r);
            }
            None => c.add_bool(false),
        }
        // Hashed only when set, so an object that never fires a continuous-fire or temporary bonus keeps its CRC.
        if self.weapon_bonus_conditions != 0 {
            c.add_u32(self.weapon_bonus_conditions);
        }
        self.helpers.crc(c);
    }
}

/// A flying projectile of a projectile weapon. `flight` is the real behavior when the weapon's
/// `ProjectileObject` template was resolved, otherwise the lightweight straight/homing fallback.
#[derive(Clone, Debug)]
struct Projectile {
    /// Monotonic id, for the client to match entities across frames.
    id: u32,
    source: ObjectId,
    /// Tracked victim, `INVALID` for position shots and scattered shots.
    victim: ObjectId,
    pos: Coord3D,
    prev_pos: Coord3D,
    dest: Coord3D,
    /// The weapon whose detonation is applied (`detWeap`).
    template: Arc<WeaponTemplate>,
    flight: ProjectileFlight,
    /// `m_lifespanFrame` (0 = none).
    lifespan_frame: u32,
    /// `m_noDamage`: a countermeasure-diverted missile deals no warhead damage.
    no_damage: bool,
    /// `MissileAIUpdate::m_exhaustSysTmpl`: the exhaust particle system name for the firer's veterancy
    /// level. Client data only; `None` for every other projectile behavior.
    exhaust: Option<String>,
}

/// The per-projectile behavior (`DumbProjectileBehavior` / `MissileAIUpdate`), or the fallback.
#[derive(Clone, Debug)]
enum ProjectileFlight {
    /// Straight homing at `speed` per frame: `has_projectile` with no resolved template.
    Straight { speed: f32 },
    /// `DumbProjectileBehavior`: follow the precomputed Bezier arc. With `params.physics` the shell's
    /// `PhysicsBehavior` state rides along: `vel` (`m_vel`, the launcher's velocity plus gravity and friction),
    /// `airborne` (`WAS_AIRBORNE_LAST_FRAME`), `axes` (the orientation's X, Y and Z axes) and `rates` (pitch, yaw and
    /// roll, from `TumbleRandomly`, applied each physics frame).
    Dumb { params: DumbProjectileParams, path: Vec<Coord3D>, step: usize, end: Coord3D, vel: Coord3D, airborne: bool, axes: [Coord3D; 3], rates: [f32; 3] },
    /// `MissileAIUpdate` + its `PhysicsBehavior`: the launch, ignition, no-turn, homing, kill states.
    Missile(MissileFlight),
    /// `NeutronMissileUpdate`: the nuclear missile's own launch → attack flight.
    Neutron {
        params: NeutronParams,
        target: Coord3D,
        /// `m_intermedPos` (`TargetFromDirectlyAbove` above the target).
        intermed: Coord3D,
        /// The missile's current forward direction (`getTransformMatrix`'s X vector).
        dir: Coord3D,
        vel: Coord3D,
        launched: bool,
        /// `m_reachedIntermediatePos`.
        reached_intermediate: bool,
        /// `m_noTurnDistLeft`.
        no_turn_left: f32,
        /// `m_frameAtLaunch` / `m_heightAtLaunch` (`SpecialSpeedHeight`).
        frame_at_launch: u32,
        height_at_launch: f32,
    },
}

/// `WeaponDelayedDamageInfo`.
#[derive(Clone, Debug)]
struct DelayedDamage {
    when: u32,
    source: ObjectId,
    victim: ObjectId,
    pos: Coord3D,
    template: Arc<WeaponTemplate>,
}

/// What `MissileAIUpdate`'s goal object contributes to the AI's move goal and the KILL detonation gate
/// (`AIMoveToState::update` and `doKillState`, AIStates.cpp:2085-2125 / MissileAIUpdate.cpp:600-630).
#[derive(Clone, Copy, Debug)]
struct GoalObject {
    /// `Object::getPosition()`.
    pos: Coord3D,
    /// `GeometryInfo::getCenterPosition` (`pos` plus `getZDeltaToCenterPosition`, Geometry.cpp:245-255): the victim's
    /// geometry centre, which is where a `DumbProjectileBehavior` shell aims (`:355-360`) and where
    /// `FROM_BOUNDINGSPHERE_*` distance queries measure from (PartitionManager.cpp:893-935).
    centre: Coord3D,
    /// `GeometryInfo::getBoundingCircleRadius` (`FROM_CENTER_2D` checks).
    radius: f32,
    /// `GeometryInfo::getBoundingSphereRadius` (`FROM_BOUNDINGSPHERE_3D`).
    sphere_radius: f32,
    /// `GeometryInfo::getMaxHeightAbovePosition() / 2` (`AIMoveToState`'s half height).
    half_height: f32,
    /// `PhysicsBehavior::m_vel` (per frame).
    velocity: Coord3D,
    /// `Thing::getUnitDirectionVector3D`, flattened to XY: the sim's transform carries no pitch.
    facing: Coord3D,
    has_physics: bool,
    immobile: bool,
}

/// Read-only view of a live projectile, for client rendering (`position`, direction and the
/// projectile object template whose model is drawn).
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectileSnapshot {
    pub id: u32,
    pub source: ObjectId,
    pub prev_pos: Coord3D,
    pub pos: Coord3D,
    /// Movement this frame (`pos - prev_pos`); zero on the creation frame.
    pub velocity: Coord3D,
    /// The projectile object template name (`ProjectileObject`), for `template -> model`.
    pub template_name: String,
    /// The exhaust particle system (`ProjectileExhaust` for the firer's veterancy level), for a missile.
    pub exhaust: Option<String>,
    /// The exhaust system is attached and emitting: from ignition until `tossExhaust` (fuel out) or the
    /// projectile's removal. The client stops emitting when this goes false or the snapshot disappears.
    pub exhaust_lit: bool,
    /// The object's orientation (its X axis, a unit vector): a missile's `MissileFlight::heading`, which
    /// includes pitch, so an attached exhaust follows the full transform. Other projectiles use their
    /// movement direction (or +X when they are not moving).
    pub heading: Coord3D,
}

/// World-level combat state: projectiles, delayed damage and the alliance table.
#[derive(Default)]
pub struct CombatState {
    projectiles: Vec<Projectile>,
    delayed: Vec<DelayedDamage>,
    /// Directed relationship overrides (`Player::m_playerRelations`): from-player -> to-player.
    /// An absent pair defaults to [`Relationship::Neutral`].
    relationships: BTreeMap<(PlayerIndex, PlayerIndex), Relationship>,
    /// The neutral/civilian player (`PlayerList::getNeutralPlayer`), neutral to and from everyone.
    neutral: Option<PlayerIndex>,
    /// Projectile streams (`projectile_stream.rs`).
    pub(crate) streams: crate::projectile_stream::StreamState,
    /// Laser beams (`laser_beam.rs`).
    pub(crate) lasers: crate::laser_beam::LaserState,
    /// The engine's global time of day / weather as the `NIGHT`/`SNOW` model-condition bits (see
    /// [`crate::weapon::LaunchEnvironment`]). `Object::friend_bindToDrawable` (Object.cpp:3038-3048) folds them into
    /// every drawable's condition state, and `getProjectileLaunchOffset` matches that state, so they can pick a
    /// different launch-bone state at fire time. The engine defaults are afternoon / normal (GlobalData.cpp:725-730).
    launch_environment: crate::weapon::LaunchEnvironment,
}

impl CombatState {
    pub fn projectile_count(&self) -> usize {
        self.projectiles.len()
    }

    /// The live projectile `id`'s position (`ProjectileStreamUpdate::getAllPoints` reads
    /// `projectile->getPosition()`); `None` once it has detonated.
    pub(crate) fn projectile_position(&self, id: u32) -> Option<Coord3D> {
        self.projectiles.iter().find(|p| p.id == id).map(|p| p.pos)
    }

    /// Read-only snapshots of the live projectiles (client rendering).
    pub fn projectile_snapshots(&self) -> Vec<ProjectileSnapshot> {
        self.projectiles
            .iter()
            .map(|p| {
                let velocity = Coord3D::new(p.pos.x - p.prev_pos.x, p.pos.y - p.prev_pos.y, p.pos.z - p.prev_pos.z);
                let heading = match &p.flight {
                    ProjectileFlight::Missile(m) => m.heading,
                    // A shell's orientation is its X axis (`getTransformMatrix`), set by the flight step or by tumbling.
                    ProjectileFlight::Dumb { axes, .. } => axes[0],
                    _ => {
                        let l = sqrt(velocity.x * velocity.x + velocity.y * velocity.y + velocity.z * velocity.z);
                        if l > 0.0 { Coord3D::new(velocity.x / l, velocity.y / l, velocity.z / l) } else { Coord3D::new(1.0, 0.0, 0.0) }
                    }
                };
                ProjectileSnapshot {
                    id: p.id,
                    source: p.source,
                    prev_pos: p.prev_pos,
                    pos: p.pos,
                    velocity,
                    template_name: p.template.projectile_name.clone(),
                    exhaust: p.exhaust.clone(),
                    exhaust_lit: matches!(&p.flight, ProjectileFlight::Missile(m) if m.exhaust_lit),
                    heading,
                }
            })
            .collect()
    }

    pub(crate) fn crc(&self, c: &mut Crc) {
        c.add_u32(self.projectiles.len() as u32);
        for p in &self.projectiles {
            c.add_u32(p.source.0);
            c.add_u32(p.victim.0);
            c.add_coord(&p.pos);
            c.add_coord(&p.prev_pos);
            c.add_coord(&p.dest);
        }
        c.add_u32(self.delayed.len() as u32);
        for d in &self.delayed {
            c.add_u32(d.when);
            c.add_u32(d.source.0);
            c.add_u32(d.victim.0);
            c.add_coord(&d.pos);
        }
        // BTreeMap iteration is sorted, so the hash is deterministic.
        c.add_u32(self.relationships.len() as u32);
        for ((a, b), r) in &self.relationships {
            c.add_u32(u32::from(*a));
            c.add_u32(u32::from(*b));
            c.add_u32(*r as u32);
        }
        c.add_u32(self.neutral.map_or(0xffff, u32::from));
        // Streams and beams stand in for objects the original creates, so they are hashed, but only while
        // they exist: other games keep their CRC.
        if !self.streams.streams.is_empty() || !self.lasers.beams.is_empty() {
            self.streams.crc(c);
            self.lasers.crc(c);
        }
    }
}

/// A resolved attack target.
pub(crate) struct TargetInfo {
    pos: Coord3D,
    radius: f32,
    kind_of: u64,
    armor: Option<Arc<ArmorTemplate>>,
    id: ObjectId,
    airborne: bool,
    /// `KINDOF_BALLISTIC_MISSILE` / `KINDOF_DEMOTRAP` (template flags) and `KINDOF_PARACHUTE` (`collide_kind`): the
    /// kinds that are not in the 64-bit `kind_of` set.
    ballistic_missile: bool,
    demotrap: bool,
    parachute: bool,
    /// The victim holds a clearable garrison (`isGarrisonable() && contain count > 0 &&
    /// !isImmuneToClearBuildingAttacks()`): the `DAMAGE_KILL_GARRISONED` / `AllowAttackGarrisonedBldgs` damage
    /// estimates (`ActiveBody::estimateDamage`, ActiveBody.cpp:294-302; `Weapon.cpp:632-641`).
    clearable_garrison: bool,
}

fn dist_2d(a: &Coord3D, b: &Coord3D) -> f32 {
    sqrt((a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y))
}

/// `isSamePosition` (AIStates.cpp:203): true when the target has moved no further than a tenth of
/// the attacker's distance to it since the path was computed. Only the 2D position matters for
/// pathfinding.
fn is_same_position(our_pos: &Coord3D, prev_target: &Coord3D, cur_target: &Coord3D) -> bool {
    let (dx, dy) = (cur_target.x - prev_target.x, cur_target.y - prev_target.y);
    let (tx, ty) = (cur_target.x - our_pos.x, cur_target.y - our_pos.y);
    // Tolerance is (dist/10)^2.
    let tolerance_sqr = (tx * tx + ty * ty) * (1.0 / 100.0);
    dx * dx + dy * dy <= tolerance_sqr
}

fn dist_3d(a: &Coord3D, b: &Coord3D) -> f32 {
    sqrt((a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y) + (a.z - b.z) * (a.z - b.z))
}

/// Rotate `p` about the vertical (Generals Z) axis through `pivot` by `a` radians.
pub(crate) fn rotate_z_about(p: [f32; 3], pivot: [f32; 3], a: f32) -> [f32; 3] {
    let (s, c) = (crate::math::sin(a), crate::math::cos(a));
    let (dx, dy) = (p[0] - pivot[0], p[1] - pivot[1]);
    [pivot[0] + dx * c - dy * s, pivot[1] + dx * s + dy * c, p[2]]
}

/// Rotate `p` about the Generals Y axis through `pivot` by `a` radians (turret pitch,
/// `W3DModelDraw`'s `Pre_Rotate_Y`).
/// Rotates `v` by the unit quaternion `q = (x, y, z, w)` (the bone rest rotation of a `WeaponLaunchBone`).
fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [qx, qy, qz, qw] = q;
    let t = [2.0 * (qy * v[2] - qz * v[1]), 2.0 * (qz * v[0] - qx * v[2]), 2.0 * (qx * v[1] - qy * v[0])];
    [
        v[0] + qw * t[0] + (qy * t[2] - qz * t[1]),
        v[1] + qw * t[1] + (qz * t[0] - qx * t[2]),
        v[2] + qw * t[2] + (qx * t[1] - qy * t[0]),
    ]
}

fn rotate_y_about(p: [f32; 3], pivot: [f32; 3], a: f32) -> [f32; 3] {
    let (s, c) = (crate::math::sin(a), crate::math::cos(a));
    let (dx, dz) = (p[0] - pivot[0], p[2] - pivot[2]);
    [pivot[0] + dx * c + dz * s, p[1], pivot[2] - dx * s + dz * c]
}

/// `Vector3::Normalized_Cross_Product`: `a x b` normalised (a parallel pair gives zero, as in C++).
fn normalized_cross(a: Coord3D, b: Coord3D) -> Coord3D {
    // `Vector3::Normalized_Cross_Product` through `Vector3::Normalize` (`Inv_Sqrt`): vector_math.rs.
    let c = crate::vector_math::normalized_cross((a.x, a.y, a.z), (b.x, b.y, b.z));
    Coord3D::new(c.0, c.1, c.2)
}

/// `Matrix3D(axis, angle).Rotate_Vector` (Rodrigues): rotate `v` about unit `axis` by `angle`.
fn rotate_about_axis(v: Coord3D, axis: Coord3D, angle: f32) -> Coord3D {
    let (s, c) = (sin(angle), cos(angle));
    let cross = Coord3D::new(axis.y * v.z - axis.z * v.y, axis.z * v.x - axis.x * v.z, axis.x * v.y - axis.y * v.x);
    let dot = axis.x * v.x + axis.y * v.y + axis.z * v.z;
    Coord3D::new(
        v.x * c + cross.x * s + axis.x * dot * (1.0 - c),
        v.y * c + cross.y * s + axis.y * dot * (1.0 - c),
        v.z * c + cross.z * s + axis.z * dot * (1.0 - c),
    )
}

/// `distCalcProc_BoundaryAndBoundary_2D` squared: gap between the two bounding circles (0 if overlapping).
pub(crate) fn boundary_dist_sqr(a: &Coord3D, ra: f32, b: &Coord3D, rb: f32) -> f32 {
    let d = dist_2d(a, b) - ra - rb;
    if d <= 0.0 {
        0.0
    } else {
        d * d
    }
}

impl GameLogic {
    // ---- relationships ---------------------------------------------------------------------

    /// `Player::setPlayerRelationship`: set the directed relationship from `a` to `b`. Self is
    /// always [`Relationship::Allies`] and the neutral player is always neutral, so those are
    /// ignored here.
    pub fn set_relationship(&mut self, a: PlayerIndex, b: PlayerIndex, r: Relationship) {
        if a == b {
            return;
        }
        self.combat.relationships.insert((a, b), r);
    }

    /// Set both directions at once, as `PlayerList::newGame` does for the sides list's
    /// `playerEnemies` / `playerAllies` entries.
    pub fn set_relationship_symmetric(&mut self, a: PlayerIndex, b: PlayerIndex, r: Relationship) {
        if a == b {
            return;
        }
        self.combat.relationships.insert((a, b), r);
        self.combat.relationships.insert((b, a), r);
    }

    /// Declare two players allied (symmetric).
    pub fn set_allied(&mut self, a: PlayerIndex, b: PlayerIndex) {
        self.set_relationship_symmetric(a, b, Relationship::Allies);
    }

    /// Declare two players enemies (symmetric).
    pub fn set_enemies(&mut self, a: PlayerIndex, b: PlayerIndex) {
        self.set_relationship_symmetric(a, b, Relationship::Enemies);
    }

    /// Objects of this player are neutral to and from everyone (`PlayerList::getNeutralPlayer`,
    /// whose relationships `newGame` forces to `NEUTRAL`).
    pub fn set_neutral_player(&mut self, p: Option<PlayerIndex>) {
        self.combat.neutral = p;
    }

    /// The neutral player index, if one is set.
    pub fn neutral_player(&self) -> Option<PlayerIndex> {
        self.combat.neutral
    }

    /// `Object::getRelationship` -> `Team::getRelationship` -> `Player::getRelationship`, reduced
    /// to owner indices. Defaults to [`Relationship::Neutral`] when no override is set.
    pub fn relationship(&self, a: PlayerIndex, b: PlayerIndex) -> Relationship {
        if a == b {
            Relationship::Allies
        } else if self.combat.neutral == Some(a) || self.combat.neutral == Some(b) {
            Relationship::Neutral
        } else {
            self.combat.relationships.get(&(a, b)).copied().unwrap_or(Relationship::Neutral)
        }
    }

    pub fn combat_state(&self) -> &CombatState {
        &self.combat
    }

    /// `TheGlobalData->m_timeOfDay == TIME_OF_DAY_NIGHT` / `m_weather == WEATHER_SNOWY` as the `NIGHT`/`SNOW`
    /// model-condition bits every object's drawable carries (`Object::friend_bindToDrawable`, Object.cpp:3038-3048).
    /// The map sets the globals at load (`WorldHeightMap::ParseLightingDataChunk`, WorldHeightMap.cpp:742-744, and
    /// `ParseWorldDictDataChunk`, :723-733); the engine default is afternoon / normal (GlobalData.cpp:725-730), so
    /// callers that do not set it get the retail default. Used by the fire-time launch-bone query
    /// ([`crate::weapon::LaunchBones::select_slot`]).
    pub fn set_launch_environment(&mut self, night: bool, snow: bool) {
        self.combat.launch_environment = crate::weapon::LaunchEnvironment { night, snow };
    }

    /// The launch-bone environment (`NIGHT`/`SNOW`) currently in force.
    pub fn launch_environment(&self) -> crate::weapon::LaunchEnvironment {
        self.combat.launch_environment
    }

    // ---- commands --------------------------------------------------------------------------

    /// `MSG_DO_ATTACK_OBJECT` / attack-position for a group, with the engine's command source. The
    /// player-facing pointer/replay path is `CMD_FROM_PLAYER`; the dispatcher passes the message's
    /// source so a script or AI order keeps its own (`AIGroup::groupAttackObject` -> `aiAttackObject(...,
    /// cmdSource)`).
    pub(crate) fn command_attack_from(&mut self, player: PlayerIndex, ids: &[ObjectId], target: AttackTarget, source: crate::ai_states::CommandSource) {
        self.command_attack_impl(player, ids, target, false, source);
    }

    /// [`Self::command_attack_from`] for the player-only ground force attack (`orders.rs`), which the
    /// engine sends as a player command.
    pub(crate) fn command_attack(&mut self, player: PlayerIndex, ids: &[ObjectId], target: AttackTarget) {
        self.command_attack_impl(player, ids, target, false, crate::ai_states::CommandSource::Player);
    }

    /// `MSG_DO_FORCE_ATTACK_OBJECT`: attack even an ally / neutral object.
    pub(crate) fn command_force_attack_from(&mut self, player: PlayerIndex, ids: &[ObjectId], target: AttackTarget, source: crate::ai_states::CommandSource) {
        self.command_attack_impl(player, ids, target, true, source);
    }

    fn command_attack_impl(&mut self, player: PlayerIndex, ids: &[ObjectId], target: AttackTarget, force: bool, source: crate::ai_states::CommandSource) {
        for &id in ids {
            let (has_weapons, passengers) = match self.objects.get(&id) {
                Some(o) if !o.destroyed && o.player == player && !o.ai.suspended => {
                    let has = !o.combat.weapons.is_empty();
                    // `AIGroup::groupAttackObjectPrivate`: order every garrisoned member that can
                    // fire to attack the same target.
                    let fires = self.is_passenger_allowed_to_fire(id);
                    (has, if fires { self.contained_units(id).to_vec() } else { Vec::new() })
                }
                _ => continue,
            };
            if !has_weapons && passengers.is_empty() {
                continue;
            }
            if let AttackTarget::Object(t) = target {
                if t == id || self.target_info(&target, player, force).is_none() {
                    continue;
                }
            }
            if has_weapons {
                self.end_attack(id); // cancels a pending pre-attack
                match target {
                    AttackTarget::Object(t) => self.ai_attack_object_from(id, t, force, false, None, source),
                    AttackTarget::Ground(p) => self.ai_attack_position_order_from(id, p, false, source),
                }
            }
            for p in passengers {
                if let AttackTarget::Object(t) = target {
                    if t == p {
                        continue;
                    }
                }
                let ok = self.objects.get(&p).is_some_and(|o| !o.destroyed && !o.ai.suspended && !o.combat.weapons.is_empty());
                if !ok {
                    continue;
                }
                self.end_attack(p);
                match target {
                    AttackTarget::Object(t) => self.ai_attack_object_from(p, t, force, false, None, source),
                    AttackTarget::Ground(pos) => self.ai_attack_position_order_from(p, pos, false, source),
                }
            }
        }
    }

    /// `AttackNugget::create` (`ObjectCreationList.cpp:182`): lock the weapon slot
    /// (`setWeaponLock(..., LOCKED_TEMPORARILY)`) and `aiAttackPosition` at the target for at most
    /// `number_of_shots` shots. The `RadiusDecalUpdate` is client-only and not simulated.
    pub(crate) fn ocl_attack_position(&mut self, source: ObjectId, target: Coord3D, number_of_shots: i32, weapon_slot: usize) {
        let ok = self.objects.get(&source).is_some_and(|o| !o.destroyed && !o.ai.suspended && !o.combat.weapons.is_empty());
        if !ok {
            return;
        }
        // `AttackNugget::create` (`ObjectCreationList.cpp:204`): `setWeaponLock(slot, LOCKED_TEMPORARILY)`.
        self.set_weapon_lock(source, weapon_slot, crate::weapon::WeaponLockType::Temporarily);
        self.end_attack(source);
        self.ai_start_attack(source, AttackTarget::Ground(target), false, false, Some((number_of_shots, weapon_slot)), crate::ai_states::AiStateType::Idle);
    }

    /// `aiIdle`: drop the attack order (and any chase path). `privateIdle` (AIUpdate.cpp:3092-3098) runs
    /// `getStateMachine()->clear()`, which notifies the AI (`AIStateMachine::clear`, AIStates.cpp:1017-1027 ->
    /// `friend_notifyStateMachineChanged` -> `wakeUpNow`), so the command path wakes the module.
    pub(crate) fn end_attack(&mut self, id: ObjectId) {
        self.end_attack_with(id);
        self.wake_up_now(id);
    }

    /// The attack's own exit, reached through the attack state machine's internal transitions (a failing aim or
    /// approach, an immobile unit out of range, the exit conditions, the shot limit, a guard's attack sub-state). The
    /// transition is an internal `setState` on a plain `StateMachine` (`AttackStateMachine`, AIStateMachine.h:188), so
    /// no `wakeUpNow` follows (lane 192).
    pub(crate) fn end_attack_internal(&mut self, id: ObjectId) {
        self.end_attack_with(id);
    }

    /// `AIAttackState::onExit` (`AIStates.cpp:5714-5745`): clears the attack statuses, the order, the current victim and
    /// the turret targets. `onExit` deletes the attack's sub-machine (`AIStates.cpp:5723`), so the sub-state that is
    /// current runs its own `onExit`. Only the approach and pursue sub-states are moves: their `onExit` runs
    /// `AIInternalMoveToState::onExit`, whose `friend_endingMove` sets `m_movementComplete` (AIStates.cpp:2804, :3134;
    /// AIUpdate.cpp:2046-2050). The next update's completed-move block then drops the path, the goal and the queued
    /// request (AIUpdate.cpp:1035-1041, :2018-2028), unless the next state starts a move first (`friend_startingMove`,
    /// AIUpdate.cpp:2034-2036). Aim and fire are not moves and leave the move alone. This exit does not stop the unit
    /// itself.
    fn end_attack_with(&mut self, id: ObjectId) {
        let now = self.frame;
        // `AIAttackState::onExit` (`AIStates.cpp:5727`): clear the attack statuses whether or not an
        // order is still set (the fire state's `onExit` may already have run).
        self.clear_firing_status(id);
        let mut move_exit = false;
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.force_attack = false;
            // `AIAttackState::onExit` (AIStates.cpp:5739): `ai->setCurrentVictim(nullptr)`.
            o.ai.machine.current_victim = ObjectId::INVALID;
            if let Some(a) = o.combat.attack.take() {
                if let Some(Some(w)) = o.combat.weapons.weapons.get_mut(a.slot) {
                    w.cancel_pre_attack(now);
                }
                move_exit = matches!(a.phase, AttackPhase::Approach | AttackPhase::Pursue);
            }
            if move_exit {
                // The sub-state's `m_goalPosition` goes with the sub-state (`AIInternalMoveToState`, AIStates.cpp:1736-1750).
                o.ai.move_goal = None;
            }
        }
        if move_exit {
            // `AIInternalMoveToState::onExit` of the approach or pursue sub-state (AIStates.cpp:2804, :3134).
            self.ai_internal_move_exit(id);
        }
        // `AIAttackState::onExit` (`AIStates.cpp:5740-5741`): clear every turret's target.
        self.clear_turret_target(id);
    }

    // ---- target resolution -----------------------------------------------------------------

    /// Resolve an attack target for `attacker_player`; `None` if it is gone, dead, unattackable
    /// or an ally. `force` (`MSG_DO_FORCE_ATTACK_OBJECT`) skips the ally check.
    pub(crate) fn target_info(&self, target: &AttackTarget, attacker_player: PlayerIndex, force: bool) -> Option<TargetInfo> {
        match target {
            AttackTarget::Ground(p) => Some(TargetInfo {
                pos: *p,
                radius: 0.0,
                kind_of: 0,
                armor: None,
                id: ObjectId::INVALID,
                airborne: false,
                ballistic_missile: false,
                demotrap: false,
                parachute: false,
                clearable_garrison: false,
            }),
            AttackTarget::Object(id) => {
                let v = self.find_object(*id)?;
                // Contained units are hidden from targeting (`Object::getContainedBy`).
                if v.contained_by.is_some() {
                    return None;
                }
                let body = v.body.as_ref()?;
                // `WeaponSet::getAttackResult` (`WeaponSet.cpp:490-496`): an `OBJECT_STATUS_MASKED`
                // object, and anything with `KINDOF_UNATTACKABLE` (pings, ambient props), is never a
                // legal target - a forced attack does not bypass either check.
                if v.status_mask(self.frame) & crate::object::status::MASKED != 0
                    || v.is_kind_of(kind_of::UNATTACKABLE)
                {
                    return None;
                }
                // `Object::getRelationship(victim)` (Object.cpp:1603-1604): an undetected defector reads as an ally to
                // every other object, so it is not a legal target for a non-forced attack (`WeaponSet.cpp:572`,
                // `getRelationship`; the defector window of defect_l193.rs). The attacker's own flag is not read here.
                let rel = if v.undetected_defector { Relationship::Allies } else { self.relationship(attacker_player, v.player) };
                if body.is_dead() || (!force && rel == Relationship::Allies) {
                    return None;
                }
                // `victim->isAirborneTarget()` (`WeaponSet.cpp:408`, the anti mask; `Weapon.cpp:2559`, the laser lift).
                let airborne = v.status_bits & crate::object::status::AIRBORNE_TARGET != 0;
                let clearable_garrison = self.is_clearable_garrison(*id);
                Some(TargetInfo {
                    pos: v.position,
                    radius: v.template.geometry.bounding_circle_radius(),
                    kind_of: v.template.kind_of,
                    armor: v.template.combat.armor.clone(),
                    id: v.id,
                    airborne,
                    ballistic_missile: v.template.ballistic_missile,
                    demotrap: v.template.demotrap,
                    parachute: v.template.collide_kinds & crate::template::collide_kind::PARACHUTE != 0,
                    clearable_garrison,
                })
            }
        }
    }

    /// `getVictimAntiMask` (`WeaponSet.cpp:389-433`): the class a victim is targeted as. Missiles, projectiles and
    /// mines come first (a missile is also a projectile, so `SMALL_MISSILE` wins); then the airborne classes; the
    /// airborne kinds that are none of those (not unattackable either) give 0; everything else is `GROUND`.
    fn victim_anti_mask(t: &TargetInfo) -> u8 {
        if t.kind_of & kind_of::SMALL_MISSILE != 0 {
            anti::SMALL_MISSILE
        } else if t.ballistic_missile {
            anti::BALLISTIC_MISSILE
        } else if t.kind_of & kind_of::PROJECTILE != 0 {
            anti::PROJECTILE
        } else if t.kind_of & kind_of::MINE != 0 || t.demotrap {
            anti::MINE | anti::GROUND
        } else if t.airborne {
            if t.kind_of & kind_of::VEHICLE != 0 {
                anti::AIRBORNE_VEHICLE
            } else if t.kind_of & kind_of::INFANTRY != 0 {
                anti::AIRBORNE_INFANTRY
            } else if t.parachute {
                anti::PARACHUTE
            } else {
                0
            }
        } else {
            anti::GROUND
        }
    }

    /// `WeaponSet::chooseBestWeaponForTarget`, `PREFER_MOST_DAMAGE`. Ready weapons trump
    /// reloading ones; ties go to the lower slot.
    pub(crate) fn choose_weapon(attacker: &Object, victim: Option<&TargetInfo>, now: u32, locked_slot: Option<usize>) -> Option<usize> {
        let set = &attacker.combat.weapons;
        // `WeaponSet::isCurWeaponLocked()`: a held lock (`MSG_SWITCH_WEAPONS` or a special attack's temporary
        // lock) forces the slot until it is released. The permanent one wins (`setWeaponLock`, `WeaponSet.cpp:1096`).
        // The caller's `locked_slot` is a fallback for callers outside the weapon set; the attack path passes none.
        if let Some(slot) = set.locked_slot().or(locked_slot) {
            if set.weapon(slot).is_some() {
                return Some(slot);
            }
        }
        let Some(v) = victim.filter(|v| v.id != ObjectId::INVALID) else {
            // attacking the ground: only the primary weapon
            return set.weapon(0).map(|_| 0);
        };
        let anti_mask = Self::victim_anti_mask(v);
        let (mut best, mut best_dmg, mut found) = (0usize, 0.0f32, false);
        let (mut backup, mut backup_dmg, mut found_backup) = (0usize, 0.0f32, false);
        for i in (0..WEAPON_SLOT_COUNT).rev() {
            let Some(w) = set.weapon(i) else { continue };
            let status = w.status(now);
            if status == WeaponStatus::OutOfAmmo && !w.template.auto_reloads_clip {
                continue;
            }
            if w.template.anti_mask & anti_mask == 0 {
                continue;
            }
            let damage = w.template.estimate_damage(v.armor.as_deref(), v.clearable_garrison);
            if damage <= 0.0 && w.template.damage_type != crate::damage::DamageType::Unresistable {
                continue;
            }
            // `WeaponSet.cpp:891-894`: a weapon whose turret is aiming at the victim is not ready,
            // so another weapon can fire while this one turns.
            let ready = status == WeaponStatus::ReadyToFire && !crate::turret_ai::turret_aiming_at(attacker, i, v.id);
            if !ready {
                if damage >= backup_dmg {
                    backup_dmg = damage;
                    backup = i;
                    found_backup = true;
                }
            } else if damage >= best_dmg {
                best_dmg = damage;
                best = i;
                found = true;
            }
        }
        if found {
            Some(best)
        } else if found_backup {
            Some(backup)
        } else {
            None
        }
    }

    // ---- the per-frame pass ----------------------------------------------------------------

    /// `AIAttackState::update` (`AIStates.cpp:5600`): run one frame of the attack sub-machine for
    /// the object's current `ObjectCombat::attack`. Called by the `AI_ATTACK_*` states in
    /// [`crate::ai_states`]; an object without an attack order does nothing here.
    ///
    /// The sub-machine (`AttackStateMachine`, `AIStates.cpp:265`) is reduced to its states' bodies
    /// run inline: `CHASE_TARGET` is [`Self::can_pursue`] ? `AIAttackPursueTargetState`
    /// (`AIStates.cpp:2953`) : `AIAttackApproachTargetState` (`AIStates.cpp:2554`),
    /// `AIM_AT_TARGET` (`AIStates.cpp:4923`) and `FIRE_WEAPON` (`AIStates.cpp:5181`). The pursue
    /// state's in-range speed-matching is reduced to stopping to aim.
    pub(crate) fn ai_attack_update_object(&mut self, id: ObjectId) {
        self.ai_attack_pass(id, false);
    }

    /// The attack's sub-machine entry, run by the enclosing state's `onEnter` in the same frame
    /// (`AIAttackState::onEnter`, AIStates.cpp:5516-5592). The default AIM state's `onEnter` returns CONTINUE and its
    /// `outOfWeaponRangeObject` condition moves the machine to CHASE_TARGET at once (`StateMachine.cpp:647-651`), so an
    /// out-of-range victim's pursue or approach `computePath` runs during the enter. Nothing else runs: an in-range
    /// victim waits for the next update (AIAttackAimAtTargetState::onEnter only returns CONTINUE), and the chase's
    /// incidental turret scan is an update action (AIStates.cpp:2783, :3108), not an entry one.
    pub(crate) fn ai_attack_enter_pass(&mut self, id: ObjectId) {
        self.ai_attack_pass(id, true);
    }

    /// One pass of the attack's combat machine. `entering` selects the entry chain of
    /// [`ai_attack_enter_pass`](Self::ai_attack_enter_pass); otherwise it is `AIAttackState::update`.
    fn ai_attack_pass(&mut self, id: ObjectId, entering: bool) {
        self.ai_attack_pass_inner(id, entering, false);
    }

    /// The sub-machine's transition into AIM or FIRE, run in the same update as the transition
    /// (`StateMachine::internalSetState`, StateMachine.cpp:562-630). A state whose `onEnter` returns
    /// CONTINUE checks its transition conditions at once (`friend_checkForTransitions`, :124-194). For
    /// the attack that is `cannotPossiblyAttackObject` (a dead or unattackable victim fails the attack,
    /// AIStates.cpp:1242) and `outOfWeaponRangeObject` (the chase starts), so the attack can end in the
    /// frame that fired its last shot. Only the state's own conditions run here: `shouldExit` and the
    /// shot limit are the next update's (`AIAttackState::update`, AIStates.cpp:5604-5632, :5699), and so are
    /// `chooseWeapon` and the team-target clears (F9, AIStates.cpp:5651-5669, :5686-5688). Not modelled: the
    /// `wantToSquishTarget` -> CHASE_TARGET condition of the AIM and FIRE lists (AIStates.cpp:271-282), which is
    /// pre-existing and not part of this transition (ROADMAP: the crusher's chase on a squish).
    fn ai_attack_transition_pass(&mut self, id: ObjectId) {
        self.ai_attack_pass_inner(id, true, true);
    }

    fn ai_attack_pass_inner(&mut self, id: ObjectId, entering: bool, transition: bool) {
        let now = self.frame;
        let Some(obj) = self.objects.get(&id) else { return };
        if obj.destroyed || obj.is_dying() || obj.ai.suspended {
            return;
        }
        let Some(mut order) = obj.combat.attack else {
            return;
        };
        // `AIAttackState::update` (`AIStates.cpp:5604`): the exit conditions end the attack.
        let goal_pos = match order.target {
            AttackTarget::Object(v) => self.objects.get(&v).map(|o| o.position),
            AttackTarget::Ground(p) => Some(p),
        };
        // `ExitConditions::shouldExit` only tests `getGoalObject` (object attacks); a position
        // attack has no goal object, so `exit_if_no_unit_found` does not fire for it.
        let exit_goal = match order.target {
            AttackTarget::Object(_) => goal_pos,
            AttackTarget::Ground(_) => Some(Coord3D::ZERO),
        };
        // Both exits are `STATE_SUCCESS` from `AIAttackState::update`: the attack's `onExit` runs, and the sub-state's move
        // (approach or pursue) ends with it (AIStates.cpp:5714-5745; see `end_attack_with`).
        if !transition && order.exit.should_exit(exit_goal, now) {
            self.end_attack_internal(id);
            return;
        }
        // `AIAttackState::update` (`AIStates.cpp:5699`): the attack ends once the weapon's shot
        // limit is used up (`AttackNugget`'s `NumberOfShots`). A transition check does not test it.
        if !transition && order.max_shots.is_some_and(|m| order.consecutive_shots >= m) {
            self.end_attack_internal(id);
            return;
        }
        // `AIAttackState::update` (AIStates.cpp:5633-5669): the victim's team changed since the attack began (`m_victimTeam`,
        // recorded at the state's entry, :5459, :5559), and the attacker no longer considers it an enemy, so the attack
        // fails. A changed team that is still an enemy keeps the attack. The relation is the object-level one (a defector
        // reads as an ally for its defection window).
        if !transition {
            if let AttackTarget::Object(v) = order.target {
                let team_now = self.objects.get(&v).and_then(|o| o.team.clone());
                let recorded = self.objects.get(&id).and_then(|o| o.combat.attack_victim_team.clone());
                match recorded {
                    Some((rv, rt)) if rv == v => {
                        if team_now != rt && self.object_relationship(id, v) != Relationship::Enemies {
                            self.clear_team_target_if(id, v);
                            self.end_attack_internal(id);
                            return;
                        }
                    }
                    _ => {
                        if let Some(o) = self.objects.get_mut(&id) {
                            o.combat.attack_victim_team = Some((v, team_now));
                        }
                    }
                }
            }
        }
        let Some(obj) = self.objects.get(&id) else { return };
        let (pos, angle, src_r, player) = (obj.position, obj.angle, obj.template.geometry.bounding_circle_radius(), obj.player);
        let contained = obj.contained_by;
        let loco = obj.template.locomotor;
        let turret_rate = obj.template.combat.turret_turn_rate;

        // AIAttackState::update: victim gone / no longer attackable ends the attack; the weapon is
        // re-chosen every frame.
        let target = self.target_info(&order.target, player, order.force);
        // A transition check tests only the victim (`cannotPossiblyAttackObject`, AIStates.cpp:1242): the weapon is
        // not re-chosen there (`chooseWeapon` is in `AIAttackState::update`, AIStates.cpp:5686-5688), so the current
        // slot stands until the next update.
        let slot = if transition { target.as_ref().map(|_| order.slot) } else { target.as_ref().and_then(|t| Self::choose_weapon(obj, Some(t), now, None)) };
        let (Some(t), Some(slot)) = (target, slot) else {
            // `AIAttackState::update` (`AIStates.cpp:5651,5667`): a victim that is no longer an enemy
            // stops being the team target. The update-only step: a transition check does not clear it.
            if let (false, AttackTarget::Object(v)) = (transition, order.target) {
                if self.objects.get(&v).is_some_and(|vo| self.relationship(player, vo.player) != Relationship::Enemies) {
                    self.clear_team_target_if(id, v);
                }
            }
            // `AIAttackState::update` returns STATE_FAILURE (AIStates.cpp:5664-5667): an internal transition, no notify.
            self.end_attack_internal(id);
            return;
        };
        let wt = obj.combat.weapons.weapon(slot).map(|w| w.template.clone()).expect("chosen slot has a weapon");
        // The attack's chase/approach/aim states order the turret that carries the current weapon
        // (`getWhichTurretForCurWeapon`, `AIStates.cpp:2637-2649`, `:3083-3084`, `:4988-5014`).
        let turret_controls = obj.combat.turret.as_ref().is_some_and(|m| m.slot_on_turret(&obj.template.combat, slot));
        let turret_drives = turret_controls && turret_rate.is_some_and(|r| r != 0.0);
        // `setCurrentVictim` is called by `AIAttackState::update` (`AIStates.cpp:5630`), the approach's `updateInternal`
        // (:2704) and the pursue's `updateInternal` (:3054); `AIIdleState` clears it (:1385). The live victim becomes the AI's
        // current victim (`atk`) on those updates. The entry does not (the state's `onEnter` never calls `setCurrentVictim`).
        if !entering {
            if let AttackTarget::Object(v) = order.target {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.current_victim = v;
                }
            }
        }
        if slot != order.slot {
            // changing weapon abandons a pending pre-attack on the old one
            if let Some(o) = self.objects.get_mut(&id) {
                if let Some(Some(w)) = o.combat.weapons.weapons.get_mut(order.slot) {
                    w.cancel_pre_attack(now);
                }
            }
            order.slot = slot;
        }
        // `AIAttackState::chooseWeapon` sets the object's current weapon (`WeaponSet::m_curWeapon`).
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.current_weapon_slot = slot;
        }
        if turret_controls {
            match order.target {
                AttackTarget::Object(v) => self.set_turret_target_object(id, Some(v), order.force),
                AttackTarget::Ground(p) => self.set_turret_target_position(id, Some(p)),
            }
        }

        // A firing passenger shoots from its assigned fire point (`GarrisonContain`), which is
        // what makes its range effectively start at the building's edge. Move to the best point
        // *before* the range check (`attemptBestFirePointPosition`), like the C++ aim state.
        let mut pos = pos;
        if let Some(container) = contained {
            if self.is_enclosing_garrison(container) {
                self.assign_passenger_fire_point(container, id, t.pos);
            }
            pos = self.passenger_fire_position(container, id);
            if let Some(o) = self.objects.get_mut(&id) {
                o.set_position(pos);
            }
        }

        let d2 = boundary_dist_sqr(&pos, src_r, &t.pos, t.radius);
        let wb = self.combined_weapon_bonus(id, &wt);
        let range = wt.attack_range() * wb.get(crate::bonus::WeaponBonusField::Range);
        let min_range = wt.min_attack_range();
        let in_range = d2 <= range * range && d2 >= min_range * min_range;
        let (dx, dy) = (t.pos.x - pos.x, t.pos.y - pos.y);
        // The entry stops at the default AIM state: an in-range victim is aimed at on the next update.
        if entering && in_range {
            return;
        }
        // `AIAttackApproachTargetState::updateInternal` (AIStates.cpp:2673): `m_stopIfInRange = !ai->isAttackPath()`. An approach
        // on an attack path does not stop at the first in-range frame (AIStates.cpp:2710); it keeps moving along the path, and
        // the move's arrival ends the approach (AIStates.cpp:1908-1940, :2722). The approach's in-range frames take the move path.
        // A unit with no locomotor cannot move, so its in-range frame keeps the success below (as before this rule).
        // A contact weapon in range on an object victim succeeds before the stop test (AIStates.cpp:2706-2709), so it never drives on.
        let contact_in_range = in_range && wt.is_contact_weapon() && matches!(order.target, AttackTarget::Object(_));
        // The stop test's view (AIStates.cpp:2710-2721, and the position branch :2740-2750): a ground mover whose line to the
        // target is blocked does not stop in range; it falls through to `computePath` and the move. `isAttackViewBlockedByObstacle`
        // applies only when the unit is on the ground (`isDoingGroundMovement`); the victim test skips flyers (`los.rs`).
        let view_blocked = in_range
            && order.phase == AttackPhase::Approach
            && self.is_doing_ground_movement(id)
            && match order.target {
                AttackTarget::Object(v) => self.is_view_blocked_by_obstacle(id, v),
                AttackTarget::Ground(p) => self.attack_view(id, None).is_some_and(|view| self.pathfinder.attack_view_blocked(&self.terrain, &view, &pos, &p)),
            };
        // `m_stopIfInRange` is `!isAttackPath()` (AIStates.cpp:2673): an attack path does not stop in range (:2710). The stop
        // success needs the view to be clear, so a blocked view also keeps the approach moving (:2710-2721).
        let approach_moving = in_range
            && !contact_in_range
            && loco.is_some()
            && order.phase == AttackPhase::Approach
            && (view_blocked || self.objects.get(&id).is_some_and(|o| o.ai.attack_path_active));

        if !in_range || approach_moving {
            if let Some(container) = contained.filter(|_| !approach_moving) {
                // A contained unit cannot move to close the gap; the aim state fails.
                // `attemptBestFirePointPosition` also frees the fire point on failure.
                self.remove_object_from_garrison_point(container, id);
                self.end_attack_internal(id);
                return;
            }
            // outOfWeaponRange -> CHASE_TARGET. `AIAttackPursueTargetState::onEnter`
            // (`AIStates.cpp:2953`) succeeds out to the approach state when the attacker cannot
            // catch a fleeing victim; `canPursue` (`AIStates.cpp:2356`).
            if order.phase == AttackPhase::Fire {
                if let Some(Some(w)) = self.objects.get_mut(&id).map(|o| &mut o.combat.weapons.weapons[slot]) {
                    w.cancel_pre_attack(now);
                }
                // `AIAttackFireWeaponState::onExit` (`AIStates.cpp:5347`): leaving the fire state clears the bit.
                self.clear_firing_status(id);
            }
            if loco.is_none() {
                // `AIAttackPursueTargetState::updateInternal` returns success for an attacker that cannot
                // move, but `update()` still runs its incidental scan (`AIStates.cpp:3108-3123`).
                if !entering {
                    self.ai_attack_incidental_turret_scan(id, order.force, turret_controls);
                }
                self.end_attack_internal(id); // immobile and out of range: cannot possibly attack
                return;
            }
            // `AIAttackPursueTargetState::computePath` (AIStates.cpp:2855-2857): a stuck unit fails the pursue, and the
            // attack goes on to the approach state (`onEnter` SUCCESS at :2935-2940, `updateInternal` FAILURE at :3046-3052).
            let blocked_and_stuck = self.objects.get(&id).is_some_and(|o| o.ai.is_blocked_and_stuck);
            let was_pursuing = order.phase == AttackPhase::Pursue;
            if !approach_moving && !blocked_and_stuck && self.can_pursue(id, &wt, &t, turret_controls) {
                // `AIAttackPursueTargetState::computePath` (`AIStates.cpp:2848`): a regular path to
                // the victim, re-pathed at most every `MIN_RECOMPUTE_TIME` once it has moved.
                order.phase = AttackPhase::Pursue;
                // `updateInternal` (`AIStates.cpp:3087`): match the victim's speed (or close the
                // last stretch at full speed once already in firing range).
                let victim_speed = self.pursue_victim_speed(&t);
                // `AIAttackPursueTargetState::computePath` (`AIStates.cpp:2862,2904`): the state owns
                // `m_waitingForPath`; consume it once `doPathfind` has finished the request.
                if let Some(o) = self.objects.get(&id) {
                    if order.waiting_for_path && !o.ai.waiting_for_path {
                        order.waiting_for_path = false;
                    }
                }
                let initial = order.chase_ref.is_none();
                let waiting = order.waiting_for_path;
                let moved = order.chase_ref.is_some_and(|r| !is_same_position(&pos, &r, &t.pos));
                // `forceRepath` (AIStates.cpp:2859-2860): no path and not waiting for one. A forced repath skips the
                // minimum-time test (:2867) and the same-victim-position test (:2894).
                let forced = self.objects.get(&id).is_some_and(|o| !o.ai.has_path() && !o.ai.waiting_for_path);
                let requested = !waiting && (initial || forced || (now >= order.repath_allowed_at && moved));
                if requested {
                    // `AIAttackPursueTargetState::computePath` (AIStates.cpp:2901-2905): `requestPath(&m_goalPosition, false)`,
                    // a non-final goal at the victim's position. The request takes that position as given: the adjust is the
                    // enter's (`onEnter` below), not the compute's, so `ai_request_path` carries no second adjust.
                    self.ai_request_path(id, t.pos, true);
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.ai.is_final_goal = false;
                    }
                    order.chase_ref = Some(t.pos);
                    order.repath_allowed_at = now + MIN_RECOMPUTE_TIME;
                    order.waiting_for_path = true;
                }
                if entering {
                    // The base `AIInternalMoveToState::onEnter` after the pursue's `computePath` (AIStates.cpp:2967-2968).
                    self.pursue_enter_move(id, t.pos, requested);
                }
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.desired_speed = if victim_speed > 0.0 { victim_speed } else { FAST_AS_POSSIBLE };
                    o.combat.attack = Some(order);
                }
                // `AIAttackPursueTargetState::update` (`AIStates.cpp:3112-3123`).
                if !entering {
                    self.ai_attack_incidental_turret_scan(id, order.force, turret_controls);
                }
                return;
            }
            // A pursue that failed (blocked and stuck, or refused) hands over to the approach state, whose `onEnter` runs
            // (AIStates.cpp:2935-2940, :3046-3052), so the approach's entry tests apply to it as well.
            let approach_entering = entering || was_pursuing;
            // `AIAttackApproachTargetState`'s refusals (AIStates.cpp:2609-2621 at entry, :2680-2688 on update).
            if self.approach_refused(id, order.target, approach_entering, wt.is_contact_weapon()) {
                self.end_attack_internal(id);
                return;
            }
            // Approach: `AIAttackApproachTargetState::computePath` (`AIStates.cpp:2409`).
            order.phase = AttackPhase::Approach;
            let o = self.objects.get_mut(&id).expect("exists");
            // A missing path forces a re-path; otherwise re-path only once `MIN_RECOMPUTE_TIME`
            // frames have passed and the victim has moved past `isSamePosition`'s tolerance.
            // `AIAttackApproachTargetState::computePath` (`AIStates.cpp:2427,2480,2504`): the state
            // owns `m_waitingForPath`; consume it once `doPathfind` has finished the request.
            // `AIInternalMoveToState::update`'s `m_waitingForPath` block (AIStates.cpp:1786-1801): the served request is consumed
            // in the update that finds the pathfinder done. `computePath` ran first and saw the flag, so it queued nothing.
            let served = order.waiting_for_path && !o.ai.waiting_for_path;
            if served {
                order.waiting_for_path = false;
            }
            let initial = order.chase_ref.is_none();
            // `AIAttackApproachTargetState::computePath` (AIStates.cpp:2421-2429): `forceRepath` is set by a blocked and
            // stuck unit, or by a unit with no path that is not waiting for one (`getPath()==nullptr &&
            // !isWaitingForPath()`). A forced repath skips the minimum-time test (AIStates.cpp:2436) and the
            // same-victim-position test (:2452). The predicate does not read `isMoving` (lane 189, R25-14).
            let forced = o.ai.is_blocked_and_stuck || (o.ai.path.is_empty() && !o.ai.waiting_for_path);
            let waiting = order.waiting_for_path || served;
            let moved = order.chase_ref.is_some_and(|r| !is_same_position(&pos, &r, &t.pos));
            if !waiting && (initial || forced || (now >= order.repath_allowed_at && moved)) {
                // `AIAttackApproachTargetState::computePath` -> `requestAttackPath` ->
                // `computeAttackPath` -> `findAttackPath`: queued on the pathfinder
                // (`AIStates.cpp:2486`), serviced by `processPathfindQueue`.
                let goal = approach_point(&pos, src_r, &t.pos, t.radius, range, min_range);
                let victim_id = match order.target {
                    AttackTarget::Object(v) => v,
                    AttackTarget::Ground(_) => ObjectId::INVALID,
                };
                let mut queued = None;
                if crate::pathfind::PathObject::for_object(o).is_some() {
                    queued = o.ai.request_attack_path(&mut self.pathfinder, id, victim_id, t.pos, now);
                } else {
                    let terrain = &self.terrain;
                    o.ai.move_to(&pos, &goal, terrain);
                }
                // Lane 188: the entry runs `AIInternalMoveToState::onEnter` after `computePath` (AIStates.cpp:2665-2678,
                // :1635), so `friend_startingMove` clears `m_movementComplete` (AIUpdate.cpp:2036). The guard's return
                // move exits in the same update (`friend_endingMove`), and `AIUpdateInterface::update`'s completed-move
                // block (AIUpdate.cpp:1035-1060) would otherwise `destroyPath` the request queued here.
                if approach_entering {
                    o.ai.friend_starting_move();
                }
                order.chase_ref = Some(t.pos);
                order.repath_allowed_at = now + MIN_RECOMPUTE_TIME;
                order.waiting_for_path = true;
                o.combat.attack = Some(order);
                // `requestAttackPath`'s `setQueueForPathTime` (AIUpdate.cpp:532-539) wakes a module asleep past the queue time.
                if let Some(frames) = queued {
                    self.wake_up_for_queue_time(id, frames);
                }
            } else {
                o.combat.attack = Some(order);
            }
            // `AIInternalMoveToState::update`'s arrival (AIStates.cpp:1908-1940) runs after the repath in the same update:
            // a move that has reached the end of its path returns SUCCESS, and `updateInternal` returns it (AIStates.cpp:2722),
            // so the approach hands over to aim. Otherwise the move continues.
            let goal = self.objects.get(&id).and_then(|o| o.ai.move_goal).unwrap_or(t.pos);
            // `AIInternalMoveToState::update` returns CONTINUE while the state itself waits for its path (AIStates.cpp:1786-1793),
            // before the arrival test. The approach's own request (`computePath` -> `requestAttackPath`) sets the state's flag in
            // this update, so a request queued from aim does not arrive on the old path (the recursion this gate prevents). The
            // AI's `isWaitingForPath` is not tested by C++ at this point, so it is not tested here.
            // The served request left no path (`computeAttackPath`'s in-range return, AIUpdate.cpp:1830-1845): `thePath == nullptr`
            // makes `AIInternalMoveToState::update` FAILURE (AIStates.cpp:1797-1801), so `updateInternal` returns SUCCESS.
            let served_without_path = served && !self.objects.get(&id).is_some_and(|o| o.ai.has_path());
            let arrived = served_without_path
                || (!order.waiting_for_path
                    && self.objects.get(&id).is_some_and(|o| o.ai.has_path())
                    && self.ai_at_goal(id, &goal));
            if !arrived {
                // The update's incidental scan (`update`, AIStates.cpp:2783-2794). On arrival the success block runs it once.
                if !entering {
                    self.ai_attack_incidental_turret_scan(id, order.force, turret_controls);
                }
                return;
            }
        }

        if matches!(order.phase, AttackPhase::Pursue | AttackPhase::Approach) {
            // `AIAttackApproachTargetState` succeeds once in range: stop and go to AIM next frame. Its
            // `update` still runs the incidental scan on this frame (`AIStates.cpp:2783-2794`); the
            // pursue state clears `m_isInitialApproach` before its own scan (`:3129`), so it does not.
            if order.phase == AttackPhase::Approach {
                self.ai_attack_incidental_turret_scan(id, order.force, turret_controls);
            }
            order.phase = AttackPhase::Aim;
            order.chase_ref = None;
            let o = self.objects.get_mut(&id).expect("exists");
            // `AIAttackApproachTargetState::onExit` / `AIAttackPursueTargetState::onExit` (`AIStates.cpp:2811`,
            // `:3140`): `m_isInitialApproach = false`, so the incidental scan stops for this attack.
            o.ai.machine.attack_initial_approach = false;
            o.combat.attack = Some(order);
            // The approach or pursue sub-state exits into aim, and its `AIInternalMoveToState::onExit` ends the move
            // (AIStates.cpp:2804, :3134). The completed-move block of this same update drops the path and the goal
            // (AIUpdate.cpp:1035-1041), where the port used to stop the unit here (lane 191 R27 F3).
            self.ai_internal_move_exit(id);
            // AIM's `onEnter` returns CONTINUE, so its conditions run in this update (see `ai_attack_transition_pass`).
            self.ai_attack_transition_pass(id);
            return;
        }

        let desired = atan2(dy, dx);
        let mut fire_entered = false;
        // The fire state left for AIM (a shot, or a failed fire): AIM's entry runs in this update.
        let mut left_fire = false;
        let thresh = wt.acceptable_aim_delta.max(AIM_REL_THRESH);
        match order.phase {
            AttackPhase::Aim => {
                // `AIAttackAimAtTargetState::update` (`AIStates.cpp:5042-5065`): a turret that can
                // turn aims and fires through its own state machine, and the AI aim state stays in
                // AIM. The turret target was ordered above.
                if turret_drives {
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.combat.attack = Some(order);
                    }
                    return;
                }
                // Body aim: `AIAttackAimAtTargetState`'s non-turret branch (`AIStates.cpp:5070-5149`).
                // This covers a hull weapon and a turret whose turn rate is 0 (a Comanche's "fake"
                // turret); in both cases the engine does no pitch here.
                // `setLocomotorGoalOrientation` turns at the current locomotor's `getMaxTurnRate(bdt)` (`AIStates.cpp:5077`).
                let turn_rate = self.locomotor_now(id).map(|l| l.max_turn_rate);
                let o = self.objects.get_mut(&id).expect("exists");
                let rel = angle_diff(desired, angle);
                let angle_aligned = rel.abs() < thresh;
                if !angle_aligned {
                    let step = turn_rate.unwrap_or(TWO_PI);
                    let a = if rel.abs() < step || step <= 0.0 { desired } else if rel > 0.0 { normalize_angle(angle + step) } else { normalize_angle(angle - step) };
                    o.set_orientation(a);
                }
                // A body-only weapon keeps the world-space turret angle with the body (a turreted
                // object's angle is carried by `update_turrets`).
                if o.combat.turret.is_none() {
                    o.combat.turret_angle = o.angle;
                }
                if angle_aligned {
                    // AIAttackFireWeaponState::onEnter; its preFireCurrentWeapon runs after this borrow ends.
                    order.phase = AttackPhase::Fire;
                    fire_entered = true;
                    // `obj->setStatus(OBJECT_STATUS_IS_FIRING_WEAPON)` (`AIStates.cpp:5203`).
                    o.status_bits |= crate::object::status::IS_FIRING_WEAPON;
                }
                o.combat.attack = Some(order);
            }
            AttackPhase::Fire => {
                // AIAttackFireWeaponState::update
                let status = self.objects[&id].combat.weapons.weapon(slot).map(|w| w.status(now));
                match status {
                    Some(WeaponStatus::PreAttack) => {}
                    Some(WeaponStatus::ReadyToFire) => {
                        // `AIAttackFireWeaponState` -> `Object::fireCurrentWeapon` (`Object.cpp:1534`): a spent clip
                        // releases the temporary lock.
                        if self.fire_weapon(id, slot, order.target) {
                            self.release_weapon_lock(id, crate::weapon::WeaponLockType::Temporarily);
                        }
                        order.consecutive_shots += 1;
                        order.phase = AttackPhase::Aim;
                        left_fire = true;
                        // `AIAttackFireWeaponState::onExit` (`AIStates.cpp:5347`) clears the firing bit.
                        self.clear_firing_status(id);
                        if let Some(o) = self.objects.get_mut(&id) {
                            o.combat.attack = Some(order);
                        }
                    }
                    _ => {
                        // not ready (between shots / reloading): failure -> back to AIM
                        order.phase = AttackPhase::Aim;
                        left_fire = true;
                        if let Some(Some(w)) = self.objects.get_mut(&id).map(|o| &mut o.combat.weapons.weapons[slot]) {
                            w.cancel_pre_attack(now);
                        }
                        self.clear_firing_status(id);
                        if let Some(o) = self.objects.get_mut(&id) {
                            o.combat.attack = Some(order);
                        }
                    }
                }
                if let Some(o) = self.objects.get_mut(&id) {
                    if let Some(a) = o.combat.attack.as_mut() {
                        a.slot = order.slot;
                    }
                }
            }
            AttackPhase::Pursue | AttackPhase::Approach => unreachable!("handled above"),
        }
        // `AIAttackFireWeaponState::onEnter` (`AIStates.cpp:5197`): the first victim a team fires on
        // becomes its common target.
        if fire_entered {
            if let AttackTarget::Object(v) = order.target {
                if self.team_attacks_common_target(id) && self.get_team_target_object(id).is_none() {
                    self.set_team_target_object(id, Some(v));
                }
            }
            // `Object::preFireCurrentWeapon(getMachineGoalObject())` (AIStates.cpp:5204): a ground attack has no victim.
            let victim = match order.target {
                AttackTarget::Object(v) => v,
                AttackTarget::Ground(_) => ObjectId::INVALID,
            };
            self.pre_fire_current_weapon(id, slot, victim);
        }
        // The entered state's conditions run in this update (see `ai_attack_transition_pass`): FIRE after the
        // aim's success (`AIAttackAimAtTargetState::update` returns SUCCESS when aligned, AIStates.cpp:5102-5110),
        // and AIM after the fire state (SUCCESS on a shot, FAILURE when not ready, AIStates.cpp:5226-5240).
        if fire_entered || left_fire {
            self.ai_attack_transition_pass(id);
        }
    }

    /// `AIAttackApproachTargetState::update` / `AIAttackPursueTargetState::update` (`AIStates.cpp:2783-2794`,
    /// `:3108-3123`): while the attack is in its initial approach and the current weapon sits on a turret, the
    /// turret looks for an incidental target of its own with `getNextMoodTarget(true, false)` and aims at it,
    /// so a turreted unit can shoot something closer while it closes on the ordered victim. The mood timer
    /// applies (`calledByAI`), so this runs at the mood rate, not every frame. `force` is `m_isForceAttacking`.
    fn ai_attack_incidental_turret_scan(&mut self, id: ObjectId, force: bool, turret_controls: bool) {
        if !turret_controls || !self.objects.get(&id).is_some_and(|o| o.ai.machine.attack_initial_approach) {
            return;
        }
        if let Some(t) = self.ai_idle_scan(id, false, None, crate::orders::GuardMode::Normal, true) {
            self.set_turret_target_object(id, Some(t), force);
        }
    }

    /// `obj->clearStatus(OBJECT_STATUS_IS_FIRING_WEAPON)` (`AIAttackFireWeaponState::onExit`,
    /// AIStates.cpp:5347; `AIAttackState::onExit`, `:5727`).
    fn clear_firing_status(&mut self, id: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.status_bits &= !crate::object::status::IS_FIRING_WEAPON;
        }
    }

    /// `canPursue` (`AIStates.cpp:2356`): the attacker can chase a victim only with a turret, when
    /// the victim has physics and is moving away but slower than the attacker, and is not already
    /// too close. The `m_aiCrushesInfantry`/`canCrushOrSquish` branch is not modelled (the sim's
    /// global AI-data flag is not ported); crushers fall through to the normal test.
    /// The base `AIInternalMoveToState::onEnter` (AIStates.cpp:1615-1667) for the pursue's entry, run after its `computePath`:
    /// `startMove` (the donut timer, AIStates.cpp:1627), `friend_startingMove` (:1635), then, when the pursue's request was made
    /// and the move adjusts (not ultra-accurate, not a `RIDER8`), `adjustDestination` (or `snapClosestGoalPosition`) of the
    /// goal and `updateGoal` (:1661-1667). The request itself was queued with the raw victim position (`requestPath`).
    fn pursue_enter_move(&mut self, id: ObjectId, goal: Coord3D, requested: bool) {
        self.loco_start_move(id);
        let (ultra, rider8) = match self.objects.get(&id) {
            Some(o) => (o.loco_rt.ultra_accurate, o.status_bits & crate::object::status::RIDER8 != 0),
            None => return,
        };
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.friend_starting_move();
        }
        if requested && !ultra && !rider8 {
            let adjusted = self.ai_adjust_move_goal(id, goal);
            self.pathfind_update_goal(id, &adjusted);
        }
    }

    /// `AIAttackApproachTargetState`'s refusals of a victim: `onEnter` (AIStates.cpp:2609-2621, `entering`) and `updateInternal`
    /// (:2680-2688). A computer player does not chase an airborne aircraft unless it is hunting. At entry a human player's
    /// auto-acquired attack (last command from the AI) is refused unless `isAllowedToChase` or the weapon is a contact weapon.
    /// `isAllowedToChase` is set on an assault transport's member (AssaultTransportAIUpdate.cpp:231, lane 194:
    /// `AiState::allowed_to_chase`). A retaliating guard is exempt at entry (:2606).
    pub(crate) fn approach_refused(&self, id: ObjectId, target: AttackTarget, entering: bool, contact: bool) -> bool {
        let AttackTarget::Object(victim) = target else { return false };
        let (Some(o), Some(v)) = (self.objects.get(&id), self.objects.get(&victim)) else { return false };
        let state = o.ai.machine.current;
        if entering && state == crate::ai_states::AiStateType::GuardRetaliate {
            return false;
        }
        if self.is_ai_player(o.player) {
            let airborne = v.status_bits & crate::object::status::AIRBORNE_TARGET != 0;
            return state != crate::ai_states::AiStateType::Hunt && v.is_kind_of(kind_of::AIRCRAFT) && airborne;
        }
        entering && o.ai.machine.last_command_source == crate::ai_states::CommandSource::Ai && !o.ai.allowed_to_chase && !contact
    }

    fn can_pursue(&self, id: ObjectId, wt: &WeaponTemplate, t: &TargetInfo, turret_controls: bool) -> bool {
        // `canPursue`'s "have to have a turret to pursue" (AIStates.cpp:2366) asks for the turret of the current weapon,
        // `getWhichTurretForCurWeapon`, not whether the unit has any turret.
        if t.id == ObjectId::INVALID || !turret_controls {
            return false;
        }
        let Some(src) = self.objects.get(&id) else { return false };
        let Some(victim) = self.objects.get(&t.id) else { return false };
        let Some(vp) = victim.physics.as_ref() else { return false };
        // `ourMaxSpeed = getCurLocomotorSpeed()` (`AIStates.cpp:2385`) = `getMaxSpeedForCondition(bdt)` (`AIUpdate.cpp:794`).
        let Some(loco) = self.locomotor_now(id) else { return false };
        let d2 = boundary_dist_sqr(&src.position, src.template.geometry.bounding_circle_radius(), &t.pos, t.radius);
        let min = wt.min_attack_range();
        if d2 < min * min {
            return false; // `weapon->isTooClose`
        }
        let our_max = loco.max_speed;
        let victim_speed = vp.speed_2d();
        if victim_speed >= our_max || victim_speed < our_max / 10.0 {
            return false;
        }
        // They are moving towards us? (`getUnitDirectionVector2D`.)
        let dx = t.pos.x - src.position.x;
        let dy = t.pos.y - src.position.y;
        if dx * cos(victim.angle) + dy * sin(victim.angle) < 0.0 {
            return false;
        }
        true
    }

    /// `victim->getPhysics()->getForwardSpeed2D()` (`AIStates.cpp:3087`), falling back to the
    /// object's locomotor speed when it has no physics module.
    fn pursue_victim_speed(&self, t: &TargetInfo) -> f32 {
        self.objects
            .get(&t.id)
            .map(|v| v.physics.as_ref().map_or(v.ai.speed, |p| p.speed_2d()))
            .unwrap_or(0.0)
    }

    /// `AIIdleState`'s mood targeting / `AIHuntState` / `AIAttackAreaState` /
    /// `AIGuardMachine::lookForInnerTarget` scan (`AIStates.cpp:1443`, `7230`, `7400`,
    /// `AIGuard.cpp:212`): recentre the turret and return the closest valid enemy, if any. The
    /// caller ([`crate::ai_states`]) decides which state to enter. `mobile_acquire` (guard / hunt /
    /// attack-move) keeps scanning while moving.
    pub(crate) fn ai_idle_scan(
        &mut self,
        id: ObjectId,
        hunt: bool,
        hunt_area: Option<(Coord3D, f32)>,
        guard_mode: crate::orders::GuardMode,
        mobile_acquire: bool,
    ) -> Option<ObjectId> {
        self.ai_scan(id, hunt, hunt_area, guard_mode, mobile_acquire, false, true)
    }

    /// `getNextMoodTarget(false, false)`: the attack-move / attack-follow scan on the frame their attack
    /// sub-machine became idle (`forceRetargetThisFrame`, `AIStates.cpp:3629, 3651, 4470, 4495`). `calledByAI` is
    /// false there, so the team victim, the mood timer and the human-only `WITHIN_ATTACK_RANGE` / `UNFOGGED`
    /// flags are skipped (`AIUpdate.cpp:4577-4609, 4659-4672`).
    pub(crate) fn ai_force_retarget_scan(&mut self, id: ObjectId) -> Option<ObjectId> {
        self.ai_scan(id, false, None, crate::orders::GuardMode::Normal, true, false, false)
    }

    /// [`ai_idle_scan`](Self::ai_idle_scan) with `guard` set for the guard machine's scan, whose own
    /// range factors (`AI_VISIONFACTOR_GUARDINNER`) are not modelled: it keeps the plain vision range.
    #[allow(clippy::too_many_arguments)] // the scan's C++ parameters (getNextMoodTarget's flags)
    pub(crate) fn ai_scan(
        &mut self,
        id: ObjectId,
        hunt: bool,
        hunt_area: Option<(Coord3D, f32)>,
        guard_mode: crate::orders::GuardMode,
        mobile_acquire: bool,
        guard: bool,
        called_by_ai: bool,
    ) -> Option<ObjectId> {
        let now = self.frame;
        // `Object::isAbleToAttack` (Object.cpp:3245): a passenger whose container does not allow firing cannot
        // attack. Its team victim is rejected below and `findClosestEnemy`'s `CAN_ATTACK` filter rejects every
        // candidate, but the timer still runs (AIUpdate.cpp:4604), so the scan returns nothing after it.
        let container_blocks = self.objects.get(&id).and_then(|o| o.contained_by).is_some_and(|c| !self.is_passenger_allowed_to_fire(c));
        // `getNextMoodTarget`'s early returns, before the team victim and the timer (AIUpdate.cpp:4531-4572). The
        // hunt and guard scans do not go through it. `calledDuringIdle` is the idle state's call, the only caller
        // with neither `mobile_acquire` nor `hunt`/`guard` (AIStates.cpp:1444).
        if !hunt && !guard && self.mood_scan_blocked_before_timer(id, !mobile_acquire) {
            return None;
        }
        // The last attacker for the passive retaliation (AIUpdate.cpp:4626-4639), read before the object is borrowed
        // mutably below: it is returned only if it still exists.
        let last_attacker = self.objects.get(&id).map(|o| o.last_damage_info.source).and_then(|s| self.objects.get(&s).filter(|x| !x.destroyed).map(|x| x.id));
        // `getNextMoodTarget` (`AIUpdate.cpp:4575`): "Check if team auto targets same victim". The team
        // victim is returned when this unit can attack it (mixed teams, no aircraft for toxin
        // tractors) and its attitude is at least NORMAL. (`AIHuntState` has its own version.) `calledByAI`
        // false skips it (`:4577`).
        if called_by_ai && !hunt && !container_blocks && self.team_attacks_common_target(id) {
            if let Some(tv) = self.get_team_target_object(id) {
                let ok = {
                    let o = &self.objects[&id];
                    o.ai.attitude >= 0
                        && self
                            .target_info(&AttackTarget::Object(tv), o.player, false)
                            .is_some_and(|ti| Self::choose_weapon(o, Some(&ti), now, None).is_some())
                };
                if ok {
                    return Some(tv);
                }
            }
        }
        // `getNextMoodTarget`: a contained unit adds its container's bounding radius to its scan
        // range, so a garrison can acquire anything within weapon range of the building's edge.
        let contained_radius = self
            .objects
            .get(&id)
            .and_then(|o| o.contained_by)
            .and_then(|c| self.objects.get(&c))
            .map_or(0.0, |c| c.template.geometry.bounding_circle_radius());
        // `AIGuardMachine::lookForInnerTarget` (`AIGuard.cpp:212`): the guard scan is centred on the
        // guarded object/position with `getStdGuardRange` (inner circle), distance `FROM_CENTER_2D`.
        let guard_center = if guard { self.ai_guard_centre(id) } else { None };
        // `AIGuard.cpp:273-282`: an area guard scans the polygon's radius around its centre, and only inside the polygon.
        let guard_area = if guard { self.objects.get(&id).and_then(|o| o.ai.machine.guard.area.clone()) } else { None };
        let guard_range = match (&guard_area, guard) {
            (Some(area), true) => crate::guard_area::polygon_radius(area),
            (None, true) => self.ai_guard_range(id),
            _ => 0.0,
        };
        let o = self.objects.get_mut(&id).expect("exists");
        let c = &o.template.combat;
        // `AI::findClosestEnemy` (AIHuntState / AIAttackAreaState) has no mood gate: neither the
        // template's `AutoAcquireEnemiesWhenIdle` flag nor the mood scan timer applies to a hunt, and
        // the guard machine's scan has its own timer (`AIGuardMachine`'s `next_scan_time`).
        // `AIUpdateInterface::getMoodMatrixActionAdjustment` (AIUpdate.cpp:4370): a sleeping
        // (`ATTITUDE_SLEEP`) unit's idle mood sets `MAA_Affect_Range_IgnoreAll`, so `AIIdleState`
        // skips mood targeting entirely (AIStates.cpp:1440). Guard/attack-move/hunt are separate
        // states and keep scanning.
        // Angry-mob members (`INFANTRY` + `IGNORED_IN_GUI`) are exempt from the mood matrix.
        let mood_exempt = o.template.ignored_in_gui && o.is_kind_of(kind_of::INFANTRY);
        if !mobile_acquire && o.ai.attitude <= -2 && !mood_exempt {
            return None;
        }
        // `getNextMoodTarget`'s timer (AIUpdate.cpp:4603-4614): not yet due -> no scan. Otherwise the next check is
        // `MoodAttackCheckRate` frames on, and a pending random offset (set by a reset or a wake-up) moves it by
        // `GameLogicRandomValue(-rate/2, rate/2)` once. `self.rng` is a field disjoint from `self.objects`.
        // `calledByAI` false skips the whole block (`:4595`).
        if called_by_ai && !hunt && !guard {
            if now < o.combat.next_mood_check {
                return None;
            }
            let rate = c.mood_attack_check_rate;
            o.combat.next_mood_check = now + rate;
            if o.combat.randomly_offset_mood_check {
                let half = (rate >> 1) as i32;
                let offset = self.rng.value(-half, half);
                o.combat.next_mood_check = (o.combat.next_mood_check as i32 + offset) as u32;
                o.combat.randomly_offset_mood_check = false;
            }
        }
        // `getMoodMatrixValue`'s passive AI unit (AIUpdate.cpp:4626-4639) answers its last attacker, unless that
        // attacker healed it, and it has no answer without a body (`!bmi`). This is before the range search.
        if o.ai.attitude == -1 && self.ai_players.contains_key(&o.player) {
            o.body.as_ref()?;
            if o.last_damage_info.kind != crate::damage::DamageType::Healing {
                return last_attacker;
            }
        }
        // A unit in a container that does not let its passengers fire finds nothing: the timer above has run, and
        // `findClosestEnemy` rejects every candidate (see `container_blocks`).
        if container_blocks {
            return None;
        }
        let (pos, player, buildings) = (o.position, o.player, c.auto_acquire_buildings);
        let mobile = o.template.locomotor.is_some() && o.template.kind_of & kind_of::IMMOBILE == 0 && o.contained_by.is_none();
        let max_range = o.combat.weapons.weapons.iter().flatten().fold(0.0f32, |m, w| m.max(w.template.attack_range()));
        let ai_player = self.ai_players.contains_key(&player);
        let base_range = if mobile && o.template.vision_range > 0.0 { o.template.vision_range } else { max_range };
        // `AI::getAdjustedVisionRangeForObject(OWNERTYPE | MOOD)` (`AI.cpp:827-877`) as
        // `getNextMoodTarget` calls it: the owner-type guard-outer factor, then (computer-controlled
        // units only) the Alert / Aggressive factor. A unit inside a container uses its largest weapon
        // range instead and skips both. The guard machine's own scan keeps the plain vision range.
        let range = if mobile && !guard && o.template.vision_range > 0.0 {
            let t = &self.ai_tunables;
            let mut r = base_range * if ai_player { t.guard_outer_modifier_ai } else { t.guard_outer_modifier_human };
            if ai_player && !hunt {
                match o.ai.attitude {
                    1 => r *= t.alert_range_modifier,
                    2 => r *= t.aggressive_range_modifier,
                    _ => {}
                }
            }
            r
        } else {
            base_range
        } + contained_radius;
        let range = if guard { guard_range } else { range };
        let scan_pos = guard_center.unwrap_or(pos);
        // `UnsignedInt flags = AI::CAN_ATTACK; if (m_attackUsesLineOfSight) { if (obj->isKindOf(
        // KINDOF_ATTACK_NEEDS_LINE_OF_SIGHT)) flags |= AI::CAN_SEE; }` (`AIUpdate.cpp:4640-4645`). Only
        // `getNextMoodTarget` sets it: the hunt / attack-area scans pass `AI::CAN_ATTACK` alone
        // (`AIStates.cpp:7229-7232`, `:7400`) and the guard machine builds its own filter list
        // (`AIGuard.cpp:234-250`), neither of which has a line-of-sight filter.
        let check_los = !hunt && !guard && self.ai_tunables.attack_uses_line_of_sight && o.template.attack_needs_line_of_sight;
        let attack_info = o.ai.attack_info.clone();
        // `AI::findClosestEnemy` with an `AttackPriorityInfo` that is not the default set picks by
        // priority; the default set (or none) returns the closest. The guard machine's own scan
        // (`AIGuardMachine::lookForInnerTarget`, `AIGuard.cpp:212`) never passes the info: plain closest.
        let priority_set = attack_info.filter(|n| !guard && self.scripts.attack_priorities.contains_key(n));
        // `AIHuntState` scans the whole map ("do NOT do line of sight check"); other scans are local.
        let candidates: Vec<ObjectId> = if hunt {
            self.objects.keys().copied().collect()
        } else {
            self.partition.objects_in_range(&scan_pos, range, if guard { DistanceMode::FromCenter2D } else { DistanceMode::FromBoundingSphere2D })
        };
        let mut best: Option<(f32, ObjectId)> = None;
        let mut ranked: Vec<(f32, ObjectId)> = Vec::new();
        for cid in candidates {
            if cid == id {
                continue;
            }
            let Some(v) = self.objects.get(&cid) else { continue };
            if v.destroyed || self.relationship(player, v.player) != Relationship::Enemies {
                continue;
            }
            // `PartitionFilterStealthedAndUndetected`: a stealthed, undetected enemy is not targetable.
            if self.is_stealthed(cid) {
                continue;
            }
            // `AI::CAN_SEE` -> `PartitionFilterLineOfSight` (`AIUpdate.cpp:4648-4655`, `AI.cpp:640-642`):
            // a unit with `KINDOF_ATTACK_NEEDS_LINE_OF_SIGHT` only targets what it can see when `AIData`'s
            // `AttackUsesLineOfSight` is on. The terrain march first, then the obstacle cells
            // (`PartitionManager.cpp:5426-5437`).
            if check_los
                && (!self.is_clear_line_of_sight_terrain(Some(id), pos, Some(cid), v.position)
                    || self.is_view_blocked_by_obstacle(id, cid))
            {
                continue;
            }
            // `AI::findClosestEnemy` adds `PartitionFilterRejectBuildings` unless the qualifiers carry
            // `ATTACK_BUILDINGS` (the template's `AutoAcquireEnemiesWhenIdle` flag, `getNextMoodTarget`
            // only; hunts never set it). The filter (`PartitionManager.cpp:5020`) lets a structure
            // through for a computer-controlled unit (`m_acquireEnemies`), for `FS_BASE_DEFENSE`
            // structures and for containers that can attack (garrisoned buildings). The guard
            // machine's scan has no such filter at all.
            if !guard && !(buildings && !hunt) && v.template.kind_of & kind_of::STRUCTURE != 0 {
                let accepted = ai_player
                    || v.template.kind_of & kind_of::FS_BASE_DEFENSE != 0
                    || (v.template.contain.is_some() && self.is_able_to_attack(cid));
                if !accepted {
                    continue;
                }
            }
            // `PartitionFilterPolygonTrigger` (AIGuard.cpp:239, PartitionManager.cpp:5159-5166): an area guard's
            // candidates are inside its polygon, by the integer position.
            if let Some(area) = &guard_area {
                if !area.contains(v.position.x as i32, v.position.y as i32) {
                    continue;
                }
            }
            // `AIAttackAreaState` only targets enemies inside the guarded area.
            if let Some((area, radius)) = &hunt_area {
                let (dx, dy) = (v.position.x - area.x, v.position.y - area.y);
                if dx * dx + dy * dy > radius * radius {
                    continue;
                }
            }
            // `PartitionFilterIsFlying::allow` (`AIGuard.cpp:286-289`, `PartitionManager.cpp:5247`): `isUsingAirborneLocomotor`.
            if guard_mode == crate::orders::GuardMode::FlyingUnitsOnly && !self.is_airborne_locomotor(cid) {
                continue;
            }
            // `AI::UNFOGGED` is only set for a human-controlled unit when `calledByAI` (`AIUpdate.cpp:4666`), so AI
            // players ignore fog when acquiring targets, and so does the attack-move's post-attack scan.
            if called_by_ai && !ai_player && !guard && self.object_shroud_status(player, cid) != ObjectShroudStatus::Clear {
                continue;
            }
            let target = AttackTarget::Object(cid);
            let Some(info) = self.target_info(&target, player, false) else { continue };
            let attacker = &self.objects[&id];
            if Self::choose_weapon(attacker, Some(&info), now, None).is_none() {
                continue;
            }
            if !mobile {
                // Immobile units only take targets already within weapon range. A garrisoned unit
                // is checked from the best fire point (`calcBestGarrisonPosition`), so it can
                // acquire targets only the building's edge can reach.
                let slot = Self::choose_weapon(attacker, Some(&info), now, None).unwrap_or(0);
                let wt = &attacker.combat.weapons.weapons[slot].as_ref().expect("weapon").template;
                let origin = attacker
                    .contained_by
                    .and_then(|c| self.best_garrison_position(c, info.pos))
                    .unwrap_or(pos);
                let d2 = boundary_dist_sqr(&origin, attacker.template.geometry.bounding_circle_radius(), &info.pos, info.radius);
                let (r, m) = (wt.attack_range(), wt.min_attack_range());
                if d2 > r * r || d2 < m * m {
                    continue;
                }
            }
            // `AIUpdateInterface::getNextMoodTarget` (AIUpdate.cpp:4666): a human-controlled unit's scan
            // sets `WITHIN_ATTACK_RANGE` (`PartitionFilterWithinAttackRange`: some weapon reaches it).
            if called_by_ai && mobile && !hunt && !guard && !ai_player {
                let a = &self.objects[&id];
                let d2 = boundary_dist_sqr(&pos, a.template.geometry.bounding_circle_radius(), &info.pos, info.radius);
                let reach = a.combat.weapons.weapons.iter().flatten().any(|w| {
                    let (r, m) = (w.template.attack_range(), w.template.min_attack_range());
                    d2 <= r * r && d2 >= m * m
                });
                if !reach {
                    continue;
                }
            }
            let d = scan_pos.dist_2d(&v.position);
            if priority_set.is_some() {
                let my_r = self.objects[&id].template.geometry.bounding_circle_radius();
                ranked.push((crate::math::sqrt(boundary_dist_sqr(&pos, my_r, &info.pos, info.radius)), cid));
            }
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, cid));
            }
        }
        let Some(set) = priority_set else { return best.map(|(_, cid)| cid) };
        // `AI::findClosestEnemy` (AI.cpp:672): walk the candidates near to far; a priority-0 target is
        // skipped, a container counts its best occupant, and each priority point is worth
        // `AttackPriorityDistanceModifier` of distance.
        ranked.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let modifier = self.ai_tunables.attack_priority_distance_modifier;
        let (mut best_enemy, mut effective, mut actual) = (None, 0i32, 0i32);
        for (dist, cid) in ranked {
            let mut cur = self.attack_priority_of(&set, cid);
            if cur == 0 {
                continue;
            }
            for &occ in self.contained_units(cid) {
                cur = cur.max(self.attack_priority_of(&set, occ));
            }
            let distance_penalty = if modifier > 0.0 { (dist / modifier) as i32 } else { 0 };
            let mod_priority = (cur - distance_penalty).max(1);
            if mod_priority > effective || (mod_priority == effective && cur > actual) {
                effective = mod_priority;
                actual = cur;
                best_enemy = Some(cid);
            }
        }
        best_enemy
    }

    // ---- firing ----------------------------------------------------------------------------

    /// `Weapon::privateFireWeapon`: effects first (scatter RNG, damage / projectile), then the
    /// ammo and delay bookkeeping (delay RNG), in the original's order. Returns `true` when the
    /// clip auto-reloaded (`Weapon::fireWeapon`'s `reloaded`, `Weapon.cpp:2571`), which is what
    /// `Object::fireCurrentWeapon` releases the temporary lock on (`Object.cpp:1534`).
    pub(crate) fn fire_weapon(&mut self, shooter: ObjectId, slot: usize, target: AttackTarget) -> bool {
        let now = self.frame;
        let Some(obj) = self.objects.get(&shooter) else { return false };
        let (spos, src_r, player) = (obj.position, obj.template.geometry.bounding_circle_radius(), obj.player);
        let Some(wt) = obj.combat.weapons.weapon(slot).map(|w| w.template.clone()) else { return false };
        // `Weapon::privateFireWeapon` uses `drawable->getBarrelCount(wslot)`; the sim owns the
        // resolved `WeaponLaunchBone` list instead of the drawable (`Drawable::getBarrelCount`,
        // Drawable.cpp:4217-4227: the first draw module with a non-empty barrel list for the slot).
        let barrel_count = obj
            .template
            .combat
            .launch_bones
            .select_slot(slot, launch_conditions(obj, self.combat.launch_environment))
            .barrels
            .len();
        // `Weapon::privateFireWeapon` (Weapon.cpp:2593): a networked weapon asks its equivalent neighbours to assist a
        // shot at an object before any target or range check (assist.rs). The ground-target callers
        // (`fire_temp_weapon`, `fire_temp_weapon_at`) never reach this with an object, so only this entry asks.
        if let AttackTarget::Object(victim) = target {
            if wt.request_assist_range > 0.0 {
                self.process_request_assistance(shooter, victim, wt.request_assist_range);
            }
        }
        // `Weapon::privateFireWeapon`'s DAMAGE_DEPLOY case (Weapon.cpp:2611-2620, before the readiness and range work): the
        // shooter's assault transport designates the victim (`beginAssault`, AssaultTransportAIUpdate.cpp:122-128).
        if wt.damage_type == crate::body::DamageType::Deploy {
            if let AttackTarget::Object(victim) = target {
                self.assault_begin(shooter, victim);
            }
        }
        let Some(t) = self.target_info(&target, player, self.objects.get(&shooter).is_some_and(|o| o.combat.force_attack)) else { return false };
        let barrel = {
            let w = self.objects.get_mut(&shooter).and_then(|o| o.combat.weapons.weapons[slot].as_mut());
            match w {
                Some(w) => w.current_barrel(barrel_count),
                None => return false,
            }
        };
        let launch = self.launch_position(shooter, slot, barrel);
        let barrel_dir = self.launch_heading(shooter, slot, barrel);
        self.fire_weapon_template(shooter, Some(slot), spos, launch, barrel_dir, src_r, &wt, &target, &t);

        let rate_of_fire = self.combined_weapon_bonus(shooter, &wt).get(crate::bonus::WeaponBonusField::RateOfFire);
        let rng = &mut self.rng;
        let mut reloaded = false;
        if let Some(o) = self.objects.get_mut(&shooter) {
            if let Some(w) = &mut o.combat.weapons.weapons[slot] {
                reloaded = w.consume_shot_with(now, rng, rate_of_fire);
                w.advance_barrel(barrel_count);
            }
        }
        // `Object::fireCurrentWeapon` (Object.cpp:1510; the position form at :1532) calls `FiringTracker::shotFired` after `fireWeapon` and
        // before the caller releases the temporary lock. Every caller of this function is a C++ `fireCurrentWeapon`
        // path: the AI attack and turret fire states (`AIAttackFireWeaponState`, AIStates.cpp:5265/:5329; the turret's
        // FIRE state is the same class, TurretAI.cpp:96), `DeliverPayloadAIUpdate`'s strafe (a ground shot, its caller
        // checks readiness first) and `FireWeaponPower`/`MissileDefender` through `aiAttackObject`. Possession is the
        // port's own mode; it fires the unit's own weapon exactly as a player's `fireCurrentWeapon` does
        // (docs/POSSESSION.md). The `Weapon::fireWeapon` callers do NOT track and do not come through here:
        // `FireWeaponCollide` (FireWeaponCollide.cpp:89), `PointDefenseLaserUpdate` (:218) and
        // `WeaponStore::createAndFireTempWeapon` (Weapon.cpp:1624/:1636, the port's `fire_temp_weapon`/
        // `fire_temp_weapon_at`). A ground shot has no victim (`INVALID_ID`).
        let params = self.objects.get(&shooter).and_then(|o| o.combat.weapons.weapons[slot].as_ref()).map(crate::object_helpers::FiringParams::of);
        if let Some(p) = params {
            let victim = match target {
                AttackTarget::Object(v) => v,
                AttackTarget::Ground(_) => ObjectId::INVALID,
            };
            self.firing_shot_fired(shooter, victim, p);
        }
        reloaded
    }

    /// `Object::preFireCurrentWeapon` (Object.cpp:1549-1561), called by `AIAttackFireWeaponState::onEnter`
    /// (AIStates.cpp:5181-5211) for a body weapon and for a turret's fire state (TurretAI.cpp:96): the weapon starts its
    /// pre-attack against `victim` only when it can fire next frame (`frame + 1 >= possibleNextShotFrame`). The
    /// pre-attack then starts only if `Weapon::preFireWeapon`'s delay is positive (Weapon.cpp:2795-2807), with the
    /// shooter's `FiringTracker` count (`getPreAttackDelay`, Weapon.cpp:2949) and the PreAttack bonus.
    pub(crate) fn pre_fire_current_weapon(&mut self, id: ObjectId, slot: usize, victim: ObjectId) {
        let now = self.frame;
        let Some(w) = self.objects.get(&id).and_then(|o| o.combat.weapons.weapon(slot)) else { return };
        if now + 1 < w.when_we_can_fire_again() {
            return;
        }
        let wt = w.template.clone();
        let shots = self.firing_consecutive_shots(id, victim).max(0) as u32;
        let pre_attack = self.combined_weapon_bonus(id, &wt).get(crate::bonus::WeaponBonusField::PreAttack);
        if let Some(o) = self.objects.get_mut(&id) {
            if let Some(Some(w)) = o.combat.weapons.weapons.get_mut(slot) {
                w.pre_fire_with(now, shots, pre_attack);
            }
        }
    }

    /// `Object::setWeaponLock` (`WeaponSet::setWeaponLock`, `WeaponSet.cpp:1078`): lock `slot` on `id`.
    /// Returns `false` when `id` is gone or has no weapon in that slot.
    pub(crate) fn set_weapon_lock(&mut self, id: ObjectId, slot: usize, kind: crate::weapon::WeaponLockType) -> bool {
        match self.objects.get_mut(&id) {
            Some(o) if !o.destroyed => {
                let ok = o.combat.weapons.set_weapon_lock(slot, kind);
                // `WeaponSet::setWeaponLock` makes the locked slot `m_curWeapon` (WeaponSet.cpp:1088-1100), for a permanent
                // lock and for a temporary one when no permanent lock is held. `current_weapon_slot` is the port's `m_curWeapon`.
                if ok && o.combat.weapons.locked_slot() == Some(slot) {
                    o.combat.current_weapon_slot = slot;
                }
                ok
            }
            _ => false,
        }
    }

    /// `Object::releaseWeaponLock` (`WeaponSet::releaseWeaponLock`, `WeaponSet.cpp:1113`).
    pub(crate) fn release_weapon_lock(&mut self, id: ObjectId, kind: crate::weapon::WeaponLockType) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.weapons.release_weapon_lock(kind);
        }
    }

    /// `Weapon::calcProjectileLaunchPosition` reduced to what the sim needs: the firing barrel's
    /// `WeaponLaunchBone` rest position, rotated by the object's yaw and, for a turreted slot, by
    /// the turret's yaw/pitch about the turret bone's pivot (`W3DModelDraw::getProjectileLaunchOffset` over the
    /// object's draw modules and `convertBonePosToWorldPos`). The launch matrix comes from the first draw module
    /// whose state `findBestInfo` picks for the object's current model conditions has a barrel for the slot
    /// (`LaunchBones::select_slot`). Model space is Generals (x east, y north, z up).
    fn launch_position(&self, shooter: ObjectId, slot: usize, barrel: usize) -> Coord3D {
        let Some(o) = self.objects.get(&shooter) else { return Coord3D::new(0.0, 0.0, 0.0) };
        let lb = &o.template.combat.launch_bones;
        let sel = lb.select_slot(slot, launch_conditions(o, self.combat.launch_environment));
        let Some(pos) = sel.barrels.get(barrel).map(|b| b.pos) else { return o.position };
        let mut p = pos;
        if let Some(tur) = sel.turret {
            if tur.slots & (1u8 << slot) != 0 {
                if let Some(pp) = tur.pitch_pivot {
                    p = rotate_y_about(p, pp, -o.combat.turret_pitch);
                }
                let rel = crate::math::angle_diff(o.combat.turret_angle, o.angle);
                p = rotate_z_about(p, tur.yaw_pivot, rel);
            }
        }
        let (s, c) = (crate::math::sin(o.angle), crate::math::cos(o.angle));
        Coord3D::new(o.position.x + p[0] * c - p[1] * s, o.position.y + p[0] * s + p[1] * c, o.position.z + p[2])
    }

    /// The launch transform's X vector (`Weapon::calcProjectileLaunchPosition` -> `getTransformMatrix`
    /// `.Get_X_Vector()`, Weapon.cpp:3031-3148): the barrel bone's rest rotation (`R_bone * X`, the bone matrix
    /// of `W3DModelDraw::getProjectileLaunchOffset`), then the turret's pitch (`Ry(-pitch)`) and yaw (`Rz`) for a
    /// turreted slot, or the object's yaw. The turret's yaw is absolute here, as in `launch_position`.
    fn launch_heading(&self, shooter: ObjectId, slot: usize, barrel: usize) -> Coord3D {
        let Some(o) = self.objects.get(&shooter) else { return Coord3D::new(1.0, 0.0, 0.0) };
        let lb = &o.template.combat.launch_bones;
        let sel = lb.select_slot(slot, launch_conditions(o, self.combat.launch_environment));
        let rest = sel.barrels.get(barrel).map_or([0.0, 0.0, 0.0, 1.0], |b| b.rot);
        let x = quat_rotate(rest, [1.0, 0.0, 0.0]);
        let turret = sel.turret.filter(|t| t.slots & (1u8 << slot) != 0);
        let (yaw, pitch) = match turret {
            Some(t) => (o.combat.turret_angle, if t.pitch_pivot.is_some() { o.combat.turret_pitch } else { 0.0 }),
            None => (o.angle, 0.0),
        };
        // `Pre_Rotate_Y(-pitch)` lifts the barrel for a positive pitch: Ry(-p) (x, y, z) = (cp x - sp z, y, sp x + cp z).
        let (sp, cp) = (sin(pitch), cos(pitch));
        let (px, py, pz) = (cp * x[0] - sp * x[2], x[1], sp * x[0] + cp * x[2]);
        // `Pre_Rotate_Z(yaw)`.
        let (sy, cy) = (sin(yaw), cos(yaw));
        Coord3D::new(cy * px - sy * py, sy * px + cy * py, pz)
    }

    /// `WeaponStore::createAndFireTempWeapon(wt, source, pos)`: a fresh weapon is loaded and fired
    /// at a position. Used by OCL `FireWeapon` and the death weapons.
    pub(crate) fn fire_temp_weapon(&mut self, source: ObjectId, wt: Arc<WeaponTemplate>) {
        let Some(obj) = self.objects.get(&source) else { return };
        let (spos, src_r) = (obj.position, obj.template.geometry.bounding_circle_radius());
        let target = AttackTarget::Ground(spos);
        let t = TargetInfo { pos: spos, radius: 0.0, kind_of: 0, armor: None, id: ObjectId::INVALID, airborne: false, ballistic_missile: false, demotrap: false, parachute: false, clearable_garrison: false };
        let facing = Coord3D::new(cos(obj.angle), sin(obj.angle), 0.0);
        self.fire_weapon_template(source, None, spos, spos, facing, src_r, &wt, &target, &t);
    }

    /// As [`fire_temp_weapon`](Self::fire_temp_weapon) but aimed at a world position (an OCL
    /// `FireWeapon` nugget fired at the special power's target).
    pub(crate) fn fire_temp_weapon_at(&mut self, source: ObjectId, wt: Arc<WeaponTemplate>, pos: Coord3D) {
        let Some(obj) = self.objects.get(&source) else { return };
        let (spos, src_r) = (obj.position, obj.template.geometry.bounding_circle_radius());
        let target = AttackTarget::Ground(pos);
        let t = TargetInfo { pos, radius: 0.0, kind_of: 0, armor: None, id: ObjectId::INVALID, airborne: false, ballistic_missile: false, demotrap: false, parachute: false, clearable_garrison: false };
        let facing = Coord3D::new(cos(obj.angle), sin(obj.angle), 0.0);
        self.fire_weapon_template(source, None, spos, spos, facing, src_r, &wt, &target, &t);
    }

    /// `Object::reloadAllAmmo(TRUE)` (`Object.cpp:1250-1253` -> `WeaponSet::reloadAllAmmo(obj, now=TRUE)`,
    /// WeaponSet.cpp:995-1008): every slot's clip is loaded instantly (`Weapon::loadAmmoNow`). Used by
    /// `FireWeaponPower` (`FireWeaponPower.cpp:106, :132, :161`); the `FiringTracker`'s idle auto-reload
    /// (`object_helpers.rs`) does the same per weapon inline.
    pub(crate) fn reload_all_ammo(&mut self, id: ObjectId) {
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&id) {
            for w in o.combat.weapons.weapons.iter_mut().flatten() {
                w.load_ammo_now(now);
            }
        }
    }

    /// The secondary point of a weapon FX (`Weapon::fireWeaponTemplate`'s `targetPos`, Weapon.cpp:918-926): the
    /// victim object's centre (`GeometryInfo::getCenterPosition`: a sphere's centre is its position, a box or
    /// cylinder's is half its height up, Geometry.cpp:244-255), else the ground point.
    pub(crate) fn fx_target_point(&self, victim: ObjectId, pos: Coord3D) -> Coord3D {
        match self.objects.get(&victim).filter(|o| !o.destroyed) {
            Some(o) => {
                let g = &o.template.geometry;
                let dz = if g.shape == crate::template::GeometryShape::Sphere { 0.0 } else { g.height * 0.5 };
                Coord3D::new(o.position.x, o.position.y, o.position.z + dz)
            }
            None => pos,
        }
    }

    /// `WeaponTemplate::fireWeaponTemplate` (FX/OCL/sound omitted). `launch` is the firing barrel's
    /// world position (`positionProjectileForLaunch`); range checks and the muzzle FX still use the
    /// shooter's origin `spos`.
    #[allow(clippy::too_many_arguments)]
    #[track_caller]
    fn fire_weapon_template(&mut self, shooter: ObjectId, slot: Option<usize>, spos: Coord3D, launch: Coord3D, barrel_dir: Coord3D, src_r: f32, wt: &Arc<WeaponTemplate>, target: &AttackTarget, t: &TargetInfo) {
        let now = self.frame;
        // `Weapon::positionProjectileForLaunch`: the launcher's physics velocity transfers to the projectile.
        let launcher_vel = self.objects.get(&shooter).and_then(|o| o.physics.as_ref()).map_or(Coord3D::ZERO, |p| p.velocity);
        let victim_id = match target {
            AttackTarget::Object(id) => *id,
            AttackTarget::Ground(_) => ObjectId::INVALID,
        };
        let victim_pos = t.pos;
        let d2 = boundary_dist_sqr(&spos, src_r, &victim_pos, t.radius);
        let range = wt.attack_range() * self.combined_weapon_bonus(shooter, wt).get(crate::bonus::WeaponBonusField::Range);
        if d2 > range * range {
            return;
        }
        let min = wt.min_attack_range();
        if d2 < min * min {
            return;
        }

        // `WeaponTemplate::fireWeaponTemplate`: play the muzzle `FireFX`. Without a skeleton the
        // muzzle is approximated at the object position plus half its height (the original uses
        // the `WeaponFireFXBone`).
        if let Some(fx) = &wt.fire_fx {
            let muzzle_z = self.objects.get(&shooter).map_or(spos.z, |o| o.position.z + o.template.geometry.height * 0.5);
            let target = self.fx_target_point(victim_id, victim_pos);
            self.record_fx_toward(shooter, fx.clone(), Coord3D::new(spos.x, spos.y, muzzle_z), target, wt.weapon_speed);
        }

        // scatter
        let is_infantry = victim_id != ObjectId::INVALID && t.kind_of & kind_of::INFANTRY != 0;
        let mut dest = victim_pos;
        let mut scatter = 0.0f32;
        if wt.scatter_radius > 0.0 || (wt.scatter_radius_vs_infantry > 0.0 && is_infantry) {
            scatter = wt.scatter_radius;
            if is_infantry && wt.scatter_radius_vs_infantry > 0.0 {
                scatter += wt.scatter_radius_vs_infantry;
            }
            scatter = self.rng.value_real(0.0, scatter);
            let ang = self.rng.value_real(0.0, TWO_PI);
            dest.x += scatter * cos(ang);
            dest.y += scatter * sin(ang);
            dest.z = self.terrain.ground_height(dest.x, dest.y);
        }

        if !wt.has_projectile {
            // `Weapon::fireWeaponTemplate`'s laser branch (Weapon.cpp:1035-1051): the beam runs to the
            // victim when the scatter lies inside a damage radius (a victim object is lifted 10 above the
            // ground unless it is a projectile or airborne), else to the ground point.
            if let Some(spec) = &wt.laser {
                // Weapon.cpp:1035-1057: the beam is created, the damage is dealt at once (the victim on a hit,
                // `INVALID` on a miss at the scattered point), and the function returns with no DelayedDamage.
                let radius_bonus = self.combined_weapon_bonus(shooter, wt).get(crate::bonus::WeaponBonusField::Radius);
                let on_victim = victim_id != ObjectId::INVALID
                    && (scatter <= wt.primary_damage_radius * radius_bonus || scatter <= wt.secondary_damage_radius * radius_bonus);
                let (end, at, damage_id) = if on_victim {
                    let lift = if t.kind_of & kind_of::PROJECTILE == 0 && !t.airborne { 10.0 } else { 0.0 };
                    let damage_id = if wt.damage_dealt_at_self_position { ObjectId::INVALID } else { victim_id };
                    (Coord3D::new(victim_pos.x, victim_pos.y, victim_pos.z + lift), victim_pos, damage_id)
                } else {
                    (dest, dest, ObjectId::INVALID)
                };
                self.create_laser(shooter, spec, end);
                self.deal_damage_internal(shooter, damage_id, at, wt, false);
                return;
            }
            let delay = dist_3d(&victim_pos, &spos) / wt.weapon_speed;
            let damage_id = if wt.damage_dealt_at_self_position { ObjectId::INVALID } else { victim_id };
            let damage_pos = if wt.damage_dealt_at_self_position { spos } else { victim_pos };
            if delay < 1.0 {
                self.deal_damage_internal(shooter, damage_id, damage_pos, wt, false);
            } else {
                // `Weapon.cpp:1085`: `UnsignedInt delayInWholeFrames = REAL_TO_INT_CEIL(delay)`. A
                // `WeaponSpeed = 0` weapon (`TroopCrawlerAssault`) gives an infinite delay, which x86's
                // float-to-int conversion turns into `INT_MIN`; the frame sum wraps.
                let whole = if delay.is_finite() { ceil(delay) as i32 } else { i32::MIN };
                let when = now.wrapping_add(whole as u32);
                self.combat.delayed.push(DelayedDamage { when, source: shooter, victim: damage_id, pos: damage_pos, template: wt.clone() });
            }
        } else {
            let victim = if scatter > 0.0 { ObjectId::INVALID } else { victim_id };
            // `TryToFollowTarget = No`: the missile is a coordinate shot (`MissileAIUpdate::projectileFire...`),
            // so it does not track the victim, and no countermeasure can divert it.
            let victim = if matches!(wt.projectile.as_deref().map(|pt| &pt.behavior), Some(ProjectileBehavior::Missile(mp)) if !mp.try_to_follow_target) {
                ObjectId::INVALID
            } else {
                victim
            };
            let flight = match wt.projectile.as_deref().map(|pt| &pt.behavior) {
                Some(ProjectileBehavior::Dumb(params)) => {
                    let params = *params;
                    // `DumbProjectileBehavior::projectileFireAtObjectOrPosition` (DumbProjectileBehavior.cpp:355-360): with
                    // a victim object the flight path ends at its geometry centre (`GeometryInfo::getCenterPosition`),
                    // "aim at the center, not the ground part"; a position shot uses the point as given.
                    let end = if victim != ObjectId::INVALID { self.fx_target_point(victim, dest) } else { dest };
                    let highest = highest_terrain_along(&launch, &end, &self.terrain);
                    let path = compute_flight_path(&launch, &end, &params, highest, wt.weapon_speed);
                    // `Weapon::positionProjectileForLaunch` -> `transferVelocityTo`: a shell with a `PhysicsBehavior` takes
                    // the launcher's velocity (DumbProjectileBehavior.cpp:345). Its `m_vel` is then integrated each frame.
                    let vel = if params.physics { launcher_vel } else { Coord3D::ZERO };
                    let airborne = launch.z > self.terrain.ground_height(launch.x, launch.y);
                    // `DumbProjectileBehavior::projectileFireAtObjectOrPosition` (DumbProjectileBehavior.cpp:381-387):
                    // with `TumbleRandomly` and a physics body, the pitch, yaw and roll rates are drawn in that order.
                    let rates = if params.tumble && params.physics {
                        let (lo, hi) = (-1.0 / std::f32::consts::PI, 1.0 / std::f32::consts::PI);
                        let pitch = self.rng.value_real(lo, hi);
                        let yaw = self.rng.value_real(lo, hi);
                        let roll = self.rng.value_real(lo, hi);
                        [pitch, yaw, roll]
                    } else {
                        [0.0; 3]
                    };
                    ProjectileFlight::Dumb { params, path, step: 0, end, vel, airborne, axes: basis_from_dir(barrel_dir), rates }
                }
                Some(ProjectileBehavior::Missile(params)) => {
                    let params = *params;
                    // `Weapon::privateFireWeapon`: a `KINDOF_SMALL_MISSILE` fired at an aircraft
                    // with active countermeasures may be diverted to a flare.
                    let decoy_at = if victim != ObjectId::INVALID
                        && wt.projectile.as_ref().is_some_and(|pt| pt.small_missile)
                        && self.objects.get(&victim).is_some_and(|v| v.locomotor_set != crate::template::LocomotorSet::Supersonic)
                    {
                        self.countermeasure_decoy_report(victim).map_or(0, |delay| now + delay)
                    } else {
                        0
                    };
                    // `WeaponTemplate`'s `UseWeaponSpeed`: the weapon speed is the initial velocity, the
                    // max speed and the acceleration (`projectileFireAtObjectOrPosition`).
                    let mut params = params;
                    if params.use_weapon_speed {
                        params.max_speed = wt.weapon_speed;
                        params.max_accel = wt.weapon_speed;
                    }
                    let initial_speed = if params.use_weapon_speed { wt.weapon_speed } else { params.initial_velocity };
                    // `MissileAIUpdate::projectileFireAtObjectOrPosition`: `dir` is the launch transform's X
                    // vector with `dir.Z += 2*zFactor` toward a higher victim, normalised.
                    let xy = dist_2d(&dest, &launch).max(1.0);
                    let dz = dest.z - launch.z;
                    let z_factor = if dz > 0.0 { dz / xy } else { 0.0 };
                    let (hx, hy, hz) = (barrel_dir.x, barrel_dir.y, barrel_dir.z + 2.0 * z_factor);
                    let hl = sqrt(hx * hx + hy * hy + hz * hz);
                    let heading = if hl > 0.0 { Coord3D::new(hx / hl, hy / hl, hz / hl) } else { barrel_dir };
                    // `applyMotiveForce(mass * initialVel * dir)` only when `initialVelToUse > 0`: the
                    // impulse is the first frame's acceleration and makes the locomotor motive.
                    let (accel, motive_until) = if initial_speed > 0.0 {
                        (Coord3D::new(heading.x * initial_speed, heading.y * initial_speed, heading.z * initial_speed), now + MOTIVE_FRAMES)
                    } else {
                        (Coord3D::ZERO, 0)
                    };
                    ProjectileFlight::Missile(MissileFlight {
                        params,
                        state: MissileState::Launch,
                        state_frame: now,
                        fuel_end: u32::MAX,
                        armed: false,
                        no_turn_left: params.initial_dist,
                        no_turn_prev: launch,
                        decoy_at,
                        // `Weapon::positionProjectileForLaunch`: the launcher's velocity transfers to the missile.
                        vel: launcher_vel,
                        accel,
                        heading,
                        airborne: launch.z > self.terrain.ground_height(launch.x, launch.y),
                        motive_until,
                        precise_z: false,
                        braking: false,
                        accel_max: 0.0,
                        turn_on: false,
                        goal_last: dest,
                        exhaust_lit: false,
                    })
                }
                Some(ProjectileBehavior::Neutron(params)) => {
                    // `NeutronMissileUpdate::projectileFireAtObjectOrPosition`: the target is the
                    // ground point and the intermediate point is `TargetFromDirectlyAbove` above it.
                    let params = params.clone();
                    let intermed = Coord3D::new(dest.x, dest.y, dest.z + params.template.target_from_directly_above);
                    let mut dir = Coord3D::new(dest.x - launch.x, dest.y - launch.y, dest.z - launch.z);
                    let dl = sqrt(dir.x * dir.x + dir.y * dir.y + dir.z * dir.z);
                    if dl > 0.0 {
                        dir.x /= dl;
                        dir.y /= dl;
                        dir.z /= dl;
                    }
                    ProjectileFlight::Neutron {
                        target: dest,
                        intermed,
                        dir,
                        vel: Coord3D::ZERO,
                        launched: false,
                        reached_intermediate: params.template.target_from_directly_above == 0.0,
                        no_turn_left: params.template.distance_to_travel_before_turning,
                        frame_at_launch: now,
                        height_at_launch: launch.z,
                        params,
                    }
                }
                None => ProjectileFlight::Straight { speed: wt.weapon_speed },
            };
            let lifespan_frame = match &flight {
                ProjectileFlight::Dumb { params, .. } => now + params.max_lifespan,
                _ => 0,
            };
            // `Weapon::fireWeaponTemplate` passes `m_projectileExhausts[sourceObj->getVeterancyLevel()]` to the
            // projectile. Only `MissileAIUpdate` stores it: `DumbProjectileBehavior` takes the parameter and drops it.
            let exhaust = match &flight {
                ProjectileFlight::Missile(_) => {
                    let level = self.objects.get(&shooter).map_or(0, |o| o.experience.level.index());
                    wt.projectile_exhaust.get(level).cloned().flatten()
                }
                _ => None,
            };
            // `Weapon::privateFireWeapon`'s `TheThingFactory->newObject(getProjectileTemplate())` (Weapon.cpp:1105)
            // takes the next object id.
            let id = self.allocate_object_id(&wt.projectile_name, std::panic::Location::caller()).0;
            self.combat.projectiles.push(Projectile {
                id,
                source: shooter,
                victim,
                pos: launch,
                prev_pos: launch,
                dest,
                template: wt.clone(),
                flight,
                lifespan_frame,
                no_damage: false,
                exhaust,
            });
            // `Weapon::newProjectileFired`: a stream weapon adds the projectile to its stream.
            if !wt.projectile_stream_name.is_empty() {
                self.new_projectile_fired(shooter, slot, &wt.projectile_stream_name, id, victim_id, victim_pos);
            }
        }
    }

    // ---- damage ----------------------------------------------------------------------------

    /// `Object::attemptDamage` + `Object::onDie`; the death type is carried into the die modules.
    pub(crate) fn apply_damage(&mut self, id: ObjectId, info: DamageInfo, death: DeathType) -> f32 {
        self.damage_object_with_death(id, info, death)
    }

    /// `WeaponTemplate::dealDamageInternal`. Victims are visited in the partition's
    /// (distance, id) order, which is deterministic.
    pub(crate) fn deal_damage_internal(&mut self, source: ObjectId, victim: ObjectId, pos: Coord3D, wt: &WeaponTemplate, projectile_detonation: bool) {
        if source == ObjectId::INVALID {
            return;
        }
        if wt.has_projectile && !projectile_detonation {
            return; // projectile weapons never deal damage directly
        }
        // The source may already be pending destruction; the original still finds it by id.
        let src = self.objects.get(&source).map(|o| (o.player, o.position, o.template.clone()));
        // `Weapon::computeBonus`: the source's condition flags scale damage and radius.
        let wb = self.combined_weapon_bonus(source, wt);
        let dmg_bonus = wb.get(crate::bonus::WeaponBonusField::Damage);
        let radius_bonus = wb.get(crate::bonus::WeaponBonusField::Radius);

        let primary = if victim != ObjectId::INVALID { self.objects.get(&victim).map(|o| (o.id, o.position)) } else { None };
        let pos = primary.map_or(pos, |(_, p)| p);

        let primary_radius = wt.primary_damage_radius * radius_bonus;
        let radius = primary_radius.max(wt.secondary_damage_radius * radius_bonus);
        let primary_r_sqr = primary_radius * primary_radius;

        // (victim id, squared boundary distance)
        let mut victims: Vec<(ObjectId, f32)> = Vec::new();
        if radius > 0.0 {
            for cid in self.partition.objects_in_range(&pos, radius, DistanceMode::FromBoundingSphere2D) {
                if let Some(o) = self.objects.get(&cid) {
                    let d2 = boundary_dist_sqr(&pos, 0.0, &o.position, o.template.geometry.bounding_circle_radius());
                    victims.push((cid, d2));
                }
            }
        } else {
            if wt.affects & affects::KILLS_SELF != 0 {
                // `WeaponTemplate::dealDamageInternal`'s no-radius `WEAPON_KILLS_SELF` branch (Weapon.cpp:1401-1412):
                // the self-hit carries the weapon's damage *and* status type, so `attemptDamage` treats a
                // `DAMAGE_STATUS` self-hit as status damage (`ActiveBody.cpp:483-490`), not as health damage.
                if wt.damage_type == DamageType::Status {
                    self.apply_status_damage(source, source, wt.damage_status_mask, HUGE_DAMAGE_AMOUNT);
                } else {
                    self.apply_damage(source, DamageInfo { source, amount: HUGE_DAMAGE_AMOUNT, kind: wt.damage_type }, wt.death_type);
                }
                return;
            }
            if let Some((id, _)) = primary {
                victims.push((id, 0.0));
            }
        }

        for (cid, d2) in victims {
            let Some(cur) = self.objects.get(&cid) else { continue };
            let (cur_player, cur_template) = (cur.player, cur.template.clone());
            let is_primary = primary.is_some_and(|(id, _)| id == cid);
            let mut kill_self = false;
            if let (Some((src_player, src_pos, src_template)), false) = (&src, is_primary) {
                if wt.affects & affects::KILLS_SELF != 0 && source == cid {
                    kill_self = true;
                } else {
                    if wt.affects & affects::SELF == 0 && source == cid {
                        continue;
                    }
                    let rel = self.relationship(cur_player, *src_player);
                    if wt.affects & affects::DOESNT_AFFECT_SIMILAR != 0 && src_template.name == cur_template.name && rel == Relationship::Allies {
                        continue;
                    }
                    // `if ((affects & WEAPON_DOESNT_AFFECT_AIRBORNE) && curVictim->isSignificantlyAboveTerrain())` (Weapon.cpp:1456).
                    if wt.affects & affects::DOESNT_AFFECT_AIRBORNE != 0 && self.significantly_above_terrain(cid) {
                        continue;
                    }
                    let required = match rel {
                        Relationship::Allies => affects::ALLIES,
                        Relationship::Enemies => affects::ENEMIES,
                        Relationship::Neutral => affects::NEUTRALS,
                    };
                    if wt.affects & required == 0 {
                        continue;
                    }
                }
                let _ = src_pos;
            }
            let amount = if kill_self {
                HUGE_DAMAGE_AMOUNT
            } else if d2 <= primary_r_sqr {
                wt.primary_damage * dmg_bonus
            } else {
                wt.secondary_damage * dmg_bonus
            };
            if wt.damage_type == DamageType::Status {
                // `ActiveBody::attemptDamage`'s DAMAGE_STATUS case (ActiveBody.cpp:483-490): the amount is a duration
                // in milliseconds for the status, not health damage.
                self.apply_status_damage(cid, source, wt.damage_status_mask, amount);
            } else {
                self.apply_damage(cid, DamageInfo { source, amount, kind: wt.damage_type }, wt.death_type);
            }
        }
    }

    /// `ActiveBody::attemptDamage`'s DAMAGE_STATUS case (ActiveBody.cpp:483-490): `Object::doStatusDamage(status,
    /// ConvertDurationFromMsecsToFrames(amount))` with the frames rounded up (`REAL_TO_INT_CEIL`). The amount is the
    /// armour-adjusted damage (`m_curArmor.adjustDamage`, before the switch), and an effectively dead object takes
    /// nothing (`attemptDamage`'s first check, ActiveBody.cpp:357). There is no zero check: a hit whose duration
    /// rounds to zero still calls `doStatusDamage`, which sets the status and clears a different status the helper
    /// holds (`StatusDamageHelper.cpp:80-92`). The hit then falls through `attemptDamage`'s `amount > 0` block like
    /// any other damage (`:527-600`): it records the last attacker (no health change, so no damage-module callback).
    pub(crate) fn apply_status_damage(&mut self, id: ObjectId, source: ObjectId, status_mask: u64, msec: f32) {
        let Some(o) = self.objects.get(&id) else { return };
        if o.effectively_dead || status_mask == 0 {
            return;
        }
        let Some(body) = o.body.as_ref() else { return };
        let amount = body.adjusted_damage(DamageType::Status, msec);
        // `LOGICFRAMES_PER_MSEC_REAL` = 30 / 1000 (GameCommon.h:70).
        let frames = (amount * crate::math::LOGICFRAMES_PER_SECOND as f32 / 1000.0).ceil();
        self.do_status_damage(id, status_mask, if frames > 0.0 { frames as u32 } else { 0 });
        if source != ObjectId::INVALID && msec > 0.0 {
            self.record_last_damage(id, &DamageInfo { source, amount: msec, kind: DamageType::Status });
        }
    }

    // ---- projectiles and delayed damage ----------------------------------------------------

    /// Move every projectile one frame; detonate on arrival (`projectileHandleCollision` ->
    /// `handleProjectileDetonation`).
    pub(crate) fn update_projectiles(&mut self) {
        let now = self.frame;
        let list = std::mem::take(&mut self.combat.projectiles);
        let mut keep = Vec::with_capacity(list.len());
        for mut p in list {
            if !self.step_projectile(&mut p, now) {
                keep.push(p);
            }
        }
        keep.append(&mut self.combat.projectiles);
        self.combat.projectiles = keep;
    }

    /// Advance one projectile; returns `true` when it detonated (and must be removed).
    fn step_projectile(&mut self, p: &mut Projectile, now: u32) -> bool {
        let victim = if p.victim != ObjectId::INVALID {
            self.find_object(p.victim).map(|v| {
                let g = &v.template.geometry;
                let facing = Coord3D::new(cos(v.angle), sin(v.angle), 0.0);
                let dz = if g.shape == crate::template::GeometryShape::Sphere { 0.0 } else { g.height * 0.5 };
                GoalObject {
                    pos: v.position,
                    centre: Coord3D::new(v.position.x, v.position.y, v.position.z + dz),
                    radius: g.bounding_circle_radius(),
                    sphere_radius: geometry_bounding_sphere_radius(g),
                    half_height: geometry_max_height_above_position(g) * 0.5,
                    velocity: v.physics.as_ref().map_or(Coord3D::ZERO, |ph| ph.velocity),
                    facing,
                    has_physics: v.physics.is_some(),
                    immobile: v.is_kind_of(kind_of::IMMOBILE),
                }
            })
        } else {
            None
        };
        // Take the flight out so we can call `&mut self` helpers while mutating it.
        let mut flight = std::mem::replace(&mut p.flight, ProjectileFlight::Straight { speed: 0.0 });
        // `KILL_SELF` done: removed with no warhead.
        let mut vanish = false;
        let detonate_at = match &mut flight {
            ProjectileFlight::Straight { speed } => {
                let speed = *speed;
                let (dest, victim_r) = victim.map_or((p.dest, 0.0), |v| (v.pos, v.radius));
                let dist = dist_3d(&p.pos, &dest);
                if dist - victim_r <= speed {
                    // The port's fallback for a projectile weapon whose template did not resolve: there is no
                    // `DumbProjectileBehavior` detonation to fire, so the victim is the primary target at its own
                    // position (a radius-0 weapon still damages it). The real behaviors are positional (below).
                    Some((p.victim, dest))
                } else {
                    let f = if dist > 0.0 { speed / dist } else { 0.0 };
                    p.prev_pos = p.pos;
                    p.pos = Coord3D::new(p.pos.x + (dest.x - p.pos.x) * f, p.pos.y + (dest.y - p.pos.y) * f, p.pos.z + (dest.z - p.pos.z) * f);
                    None
                }
            }
            ProjectileFlight::Dumb { params, path, step, end, vel, airborne, axes, rates } => {
                // The frame's movement is measured from the position at its start (the snapshot's velocity).
                p.prev_pos = p.pos;
                // PHASE_PHYSICS runs before the shell's own NORMAL-phase update (UpdateModule.h:80-87): the physics moves
                // the object, and a landing detonates the shell (`onCollide(nullptr)`, PhysicsUpdate.cpp:836).
                if params.physics && self.shell_physics(&mut p.pos, vel, airborne, axes, rates, params.friction) {
                    Some((ObjectId::INVALID, p.pos))
                } else if p.lifespan_frame != 0 && now >= p.lifespan_frame {
                    // `DumbProjectileBehavior::update`: MaxLifespan detonates where the object is (`detonate()`, no victim).
                    Some((ObjectId::INVALID, p.pos))
                } else if *step >= path.len() {
                    Some((ObjectId::INVALID, p.pos))
                } else {
                    // `FlightPathAdjustDistPerSecond`: creep the end point toward a moving victim's geometry centre
                    // (`victim->getGeometryInfo().getCenterPosition`, DumbProjectileBehavior.cpp:585-590).
                    if p.victim != ObjectId::INVALID && params.flight_path_adjust_dist_per_frame > 0.0 {
                        if let Some(v) = victim {
                            let vp = v.centre;
                            let delta = Coord3D::new(vp.x - end.x, vp.y - end.y, vp.z - end.z);
                            let d = sqrt(delta.x * delta.x + delta.y * delta.y + delta.z * delta.z);
                            if d > 0.1 {
                                let m = d.min(params.flight_path_adjust_dist_per_frame);
                                *end = Coord3D::new(end.x + delta.x / d * m, end.y + delta.y / d * m, end.z + delta.z / d * m);
                                let highest = highest_terrain_along(&p.pos, end, &self.terrain);
                                *path = compute_flight_path(&p.pos, end, params, highest, p.template.weapon_speed);
                                *step = 0;
                            }
                        }
                    }
                    if *step < path.len() {
                        let s = *step;
                        // The orientation (DumbProjectileBehavior.cpp:632-683): not with TumbleRandomly, which turns the
                        // shell by its rates instead. The step's direction is from the path point before it; step 0
                        // uses path[1] - path[0].
                        if params.orient_to_flight_path && !params.tumble {
                            let pair = if s > 0 {
                                Some((path[s], path[s - 1]))
                            } else if path.len() >= 2 {
                                Some((path[1], path[0]))
                            } else {
                                None
                            };
                            if let Some((a, b)) = pair {
                                let d = Coord3D::new(a.x - b.x, a.y - b.y, a.z - b.z);
                                let l = sqrt(d.x * d.x + d.y * d.y + d.z * d.z);
                                if l > 0.0 {
                                    *axes = basis_from_dir(Coord3D::new(d.x / l, d.y / l, d.z / l));
                                }
                            }
                        }
                        let next = path[s];
                        *step += 1;
                        p.pos = next;
                        // `projectileHandleCollision` -> `detonate()` at the shell's own position (the detonation
                        // weapon is positional, DumbProjectileBehavior.cpp:531-539). A hit on a garrison building is
                        // handled by the garrison block first (`:485-515`).
                        match self.projectile_collision(p) {
                            Some(hit) => {
                                if self.projectile_garrison_kill(p, hit) {
                                    vanish = true;
                                    None
                                } else {
                                    Some((ObjectId::INVALID, p.pos))
                                }
                            }
                            None => None,
                        }
                    } else {
                        Some((ObjectId::INVALID, p.pos))
                    }
                }
            }
            ProjectileFlight::Missile(m) => {
                // `MissileAIUpdate::update` with its physics; `true` when the missile is gone.
                if self.step_missile(p, m, now, victim) {
                    vanish = true;
                }
                None
            }
            ProjectileFlight::Neutron {
                params,
                target,
                intermed,
                dir,
                vel,
                launched,
                reached_intermediate,
                no_turn_left,
                frame_at_launch,
                height_at_launch,
            } => {
                // `NeutronMissileUpdate::update`: reach the intermediate (above-target) point, then
                // switch to a straight-down dive (`STRAIGHT_DOWN_SLOW_FACTOR`).
                if !*reached_intermediate {
                    let dx = p.pos.x - intermed.x;
                    let dy = p.pos.y - intermed.y;
                    let dz = p.pos.z - intermed.z;
                    let bound = p.template.projectile.as_ref().map_or(7.0, |pt| pt.bounding_radius).max(0.1);
                    if dx * dx + dy * dy + dz * dz <= bound * bound {
                        *reached_intermediate = true;
                        p.pos = *intermed;
                        let vlen = sqrt(vel.x * vel.x + vel.y * vel.y + vel.z * vel.z);
                        vel.x = 0.0;
                        vel.y = 0.0;
                        vel.z = -vlen * 0.5;
                    }
                }
                if !*launched {
                    // `doLaunch`: the projectile sits at the launcher's launch point; run the launch
                    // and ignition FX, arm the warhead and start `m_frameAtLaunch`.
                    *launched = true;
                    if let Some(fx) = &params.template.launch_fx {
                        self.record_fx_at(p.source, fx.clone(), p.pos);
                    }
                    if let Some(fx) = &params.template.ignition_fx {
                        self.record_fx_at(p.source, fx.clone(), p.pos);
                    }
                    *height_at_launch = p.pos.z;
                    *frame_at_launch = now;
                    p.prev_pos = p.pos;
                    p.pos = Coord3D::new(p.pos.x + vel.x, p.pos.y + vel.y, p.pos.z + vel.z);
                    None
                } else {
                    // `doAttack`: turn `dir` toward the goal and accelerate along it.
                    let mut speed = params.template.relative_speed;
                    if params.template.target_from_directly_above != 0.0 && *reached_intermediate {
                        speed *= 0.5;
                    }
                    let goal = if *reached_intermediate { *target } else { *intermed };
                    if *no_turn_left <= 0.0 {
                        let mut other = Coord3D::new(goal.x - p.pos.x, goal.y - p.pos.y, goal.z - p.pos.z);
                        let ol = sqrt(other.x * other.x + other.y * other.y + other.z * other.z);
                        if ol > 0.0 {
                            other.x /= ol;
                            other.y /= ol;
                            other.z /= ol;
                        }
                        let c = (dir.x * other.x + dir.y * other.y + dir.z * other.z).clamp(-1.0, 1.0);
                        let angle = acos(c);
                        if angle.abs() < params.template.max_turn_rate {
                            *dir = other;
                        } else {
                            // `NeutronMissileUpdate.cpp:317` takes the cross unguarded: a parallel pair turns by zero axis.
                            let axis = normalized_cross(*dir, other);
                            *dir = rotate_about_axis(*dir, axis, params.template.max_turn_rate);
                        }
                    }
                    let damping = params.template.forward_damping;
                    vel.x += speed * dir.x - damping * vel.x;
                    vel.y += speed * dir.y - damping * vel.y;
                    vel.z += speed * dir.z - damping * vel.z;
                    // `SpecialSpeedHeight`: a scripted height for the first `SpecialSpeedTime` frames.
                    if params.template.special_speed_time > 0 && now <= frame_at_launch.saturating_add(params.template.special_speed_time) {
                        let elapsed = now - *frame_at_launch;
                        if elapsed < params.template.special_speed_time {
                            let time_frac = elapsed as f32 / params.template.special_speed_time as f32;
                            let af = params.template.special_accel_factor.max(0.01);
                            let new_z = *height_at_launch + (af * time_frac) * (af * time_frac) / af * params.template.special_speed_height;
                            vel.z = new_z - p.pos.z;
                        }
                    }
                    let old = p.pos;
                    p.prev_pos = p.pos;
                    p.pos = Coord3D::new(p.pos.x + vel.x, p.pos.y + vel.y, p.pos.z + vel.z);
                    if *no_turn_left > 0.0 {
                        *no_turn_left -= dist_3d(&old, &p.pos);
                    }
                    let ground = self.terrain.ground_height(p.pos.x, p.pos.y);
                    // `HeightDieUpdate`: detonate the dive at `TargetHeight` above the ground (or on
                    // hitting it).
                    if (*reached_intermediate && p.pos.z <= ground + params.dive_height) || p.pos.z < ground {
                        Some((ObjectId::INVALID, p.pos))
                    } else {
                        self.projectile_collision(p).map(|_| (ObjectId::INVALID, p.pos))
                    }
                }
            }
        };
        if vanish {
            return true;
        }
        if let Some((hit, pos)) = detonate_at {
            self.detonate_projectile(p, hit, pos);
            return true;
        }
        p.flight = flight;
        false
    }

    /// `WeaponStore::handleProjectileDetonation` (Weapon.cpp:1609-1615) -> `fireProjectileDetonationWeapon(source, pos)`:
    /// the detonation weapon is fired *at the projectile's position* (`obj->getPosition()`, DumbProjectileBehavior.cpp:539 /
    /// MissileAIUpdate.cpp:390), with no victim object, so `dealDamageInternal` takes its positional branch
    /// (Weapon.cpp:1361-1365, 1386-1398): the blast is centred on `pos` and every object inside the radius is a
    /// non-primary victim (the `affects` flags apply to all of them). `hit` is `INVALID` for every real projectile
    /// behavior; only the port's unresolved-template fallback (the `Straight` flight) still passes its victim.
    fn detonate_projectile(&mut self, p: &Projectile, hit: ObjectId, pos: Coord3D) {
        // `Weapon::fireWeaponTemplate`'s `targetPos` for a positional shot is the position itself, so the
        // `ProjectileDetonationFX`'s secondary is the blast point (`:918-926`).
        if let Some(fx) = &p.template.projectile_detonation_fx {
            let target = if hit == ObjectId::INVALID { pos } else { self.fx_target_point(hit, pos) };
            self.record_fx_toward(p.source, fx.clone(), pos, target, p.template.weapon_speed);
        }
        // The projectile object's `SpecialPowerCompletionDie`: the firing source's power completes
        // now (`Weapon.cpp`).
        if let Some(power) = p.template.special_power_completion.clone() {
            if let Some(player) = self.objects.get(&p.source).map(|o| o.player) {
                self.record_completed_special_power(player, &power, p.source);
            }
        }
        // A neutron warhead (`NeutronMissileUpdate`): spawn the warhead object at the detonation
        // point and kill it, so its `NeutronMissileSlowDeathBehavior` runs the ground blasts.
        if let Some(name) = p.template.projectile.as_ref().and_then(|pt| pt.neutron_warhead.clone()) {
            if let Some(tmpl) = self.find_template(&name) {
                let player = self.objects.get(&p.source).map_or(0, |o| o.player);
                let id = self.create_object(&tmpl, pos, 0.0, player);
                if let Some(o) = self.objects.get_mut(&id) {
                    o.producer_id = p.source;
                    o.sp_creator = p.source;
                }
                self.kill_object(id);
            }
        }
        // `NeutronBlastBehavior::onDie` on the projectile object (`NeutronCannonShell`): wipe
        // infantry / unman vehicles around the detonation point.
        if let Some(blast) = p.template.projectile.as_ref().and_then(|pt| pt.neutron_blast.clone()) {
            let src_player = self.objects.get(&p.source).map_or(0, |o| o.player);
            self.neutron_blast_at(pos, src_player, ObjectId::INVALID, &blast);
        }
        // `m_noDamage` (countermeasure-diverted missile): the warhead does not go off.
        if p.no_damage {
            return;
        }
        self.deal_damage_internal(p.source, hit, pos, p.template.as_ref(), true);
    }

    /// The garrison block of `DumbProjectileBehavior::projectileHandleCollision`
    /// (DumbProjectileBehavior.cpp:485-515) / `MissileAIUpdate::projectileHandleCollision`
    /// (MissileAIUpdate.cpp:333-364): a projectile marked `GarrisonHitKillCount > 0` that hits a garrisonable
    /// building clears that many matching occupants, plays `GarrisonHitKillFX` on the building (the centre,
    /// not the shell), and is destroyed with no explosion. Returns `true` when it cleared the building.
    fn projectile_garrison_kill(&mut self, p: &Projectile, other: ObjectId) -> bool {
        let Some(g) = p.template.projectile.as_ref().and_then(|pt| pt.garrison_hit_kill.as_ref()) else { return false };
        // `projectileGetLauncherID()`: the score goes to the launcher, not the shell.
        let killed = self.kill_garrisoned(other, g.count, p.source, g.kind_of, g.kind_of_not);
        if killed == 0 {
            return false;
        }
        // `FXList::doFXObj(d->m_garrisonHitKillFX, other, nullptr)`: at the building.
        if let (Some(name), Some(o)) = (g.fx.as_deref(), self.find_object(other)) {
            let pos = o.position;
            self.record_fx_at(p.source, name.to_string(), pos);
        }
        true
    }

    /// `PhysicsBehavior::update`'s integration (PhysicsUpdate.cpp:618-700): the forces the AI left in `pending` this
    /// frame (`applyMotiveForce`), gravity (`applyGravitationalForces`) and `applyFrictionalForces` go into `vel`
    /// (`m_vel` integrates `m_accel`), and `vel` moves `pos` unless the object is braking (`OBJECT_STATUS_BRAKING` skips
    /// a projectile's integration, :650-657). `heading` is the object's X axis, which the friction is taken across.
    #[allow(clippy::too_many_arguments)]
    fn projectile_integrate(&self, pos: &mut Coord3D, vel: &mut Coord3D, heading: Coord3D, pending: Coord3D, motive: bool, friction: [f32; 3], braking: bool) {
        let gravity = self.rules.gravity;
        let ground = self.terrain.ground_height(pos.x, pos.y);
        let fr = physics_friction_accel(*vel, heading, pos.z - ground, gravity, motive, friction);
        for (v, a) in [(&mut vel.x, pending.x + fr.x), (&mut vel.y, pending.y + fr.y), (&mut vel.z, pending.z + fr.z + gravity)] {
            *v += a;
            if v.abs() < 0.001 {
                *v = 0.0;
            }
        }
        if !braking {
            pos.x += vel.x;
            pos.y += vel.y;
            pos.z += vel.z;
        }
    }

    /// The ground clamp of `PhysicsBehavior::update` (PhysicsUpdate.cpp:762-780): the excess fall is removed from
    /// `vel.z`. Returns `true` on the airborne -> grounded edge (`WAS_AIRBORNE_LAST_FRAME`, :836), which calls
    /// `onCollide(nullptr)` on the object.
    fn projectile_clamp(&self, pos: &mut Coord3D, vel: &mut Coord3D, airborne: &mut bool) -> bool {
        let ground = self.terrain.ground_height(pos.x, pos.y);
        if pos.z <= ground {
            vel.z += ground - pos.z;
            if vel.z > 0.0 {
                vel.z = 0.0;
            }
            pos.z = ground;
        }
        let now_airborne = pos.z > ground;
        let landed = *airborne && !now_airborne;
        *airborne = now_airborne;
        landed
    }

    /// One PHASE_PHYSICS update of a shell (see `projectile_integrate`). A `TumbleRandomly` shell first damps its rates
    /// (`applyYPRDamping`: the friction branch uses `1 - DEFAULT_LATERAL_FRICTION`, the aerodynamic one `1 - aero`,
    /// PhysicsUpdate.cpp:455, 490, 587), then turns by them after the integration: `Rotate_X(roll)`, `Rotate_Y(pitch)`,
    /// `Rotate_Z(yaw)` at `PitchRollYawFactor` (2.0, PhysicsUpdate.cpp:137), before the ground clamp (`:~730-750`).
    /// Returns `true` when the shell lands.
    fn shell_physics(&self, pos: &mut Coord3D, vel: &mut Coord3D, airborne: &mut bool, axes: &mut [Coord3D; 3], rates: &mut [f32; 3], friction: [f32; 3]) -> bool {
        let gravity = self.rules.gravity;
        let ground = self.terrain.ground_height(pos.x, pos.y);
        let damp = if pos.z - ground > -(3.0 * 3.0) * gravity { 1.0 - friction[2] } else { 1.0 - PHYSICS_DEFAULT_FRICTION[1] };
        for r in rates.iter_mut() {
            *r *= damp;
        }
        self.projectile_integrate(pos, vel, axes[0], Coord3D::ZERO, false, friction, false);
        const PITCH_ROLL_YAW_FACTOR: f32 = 2.0;
        rotate_axes(axes, 0, rates[2] * PITCH_ROLL_YAW_FACTOR);
        rotate_axes(axes, 1, rates[0] * PITCH_ROLL_YAW_FACTOR);
        rotate_axes(axes, 2, rates[1] * PITCH_ROLL_YAW_FACTOR);
        self.projectile_clamp(pos, vel, airborne)
    }

    /// `Locomotor`'s braking cheat for a projectile (the `wasBraking` block, Locomotor.cpp:1100-1150): the position moves
    /// straight at `goal` by its speed, at least `MIN_VEL` and at most the distance. A projectile never stops braking.
    fn braking_cheat(p: &mut Projectile, m: &MissileFlight, goal: Coord3D) {
        let to = Coord3D::new(goal.x - p.pos.x, goal.y - p.pos.y, goal.z - p.pos.z);
        let dl = sqrt(to.x * to.x + to.y * to.y + to.z * to.z);
        if dl > 0.001 {
            let vmag = sqrt(m.vel.x * m.vel.x + m.vel.y * m.vel.y + m.vel.z * m.vel.z);
            let speed = vmag.max(PATHFIND_CELL_SIZE_F / crate::math::LOGICFRAMES_PER_SECOND as f32).min(dl);
            p.pos = Coord3D::new(p.pos.x + to.x / dl * speed, p.pos.y + to.y / dl * speed, p.pos.z + to.z / dl * speed);
        }
    }

    /// One frame of a missile in sleepy-phase order (UpdateModule.h:80-87): `MissileAIUpdate::update` first (PHASE_INITIAL:
    /// the state switch, the state machine and the locomotor drive, AIUpdate.cpp:1002-1114), then the missile's
    /// `PhysicsBehavior::update` (PHASE_PHYSICS). Returns `true` when the missile is gone: detonated (the warhead applied)
    /// or KILL_SELF finished.
    fn step_missile(&mut self, p: &mut Projectile, m: &mut MissileFlight, now: u32, victim: Option<GoalObject>) -> bool {
        let start = p.pos;
        // `MissileAIUpdate::update`: the no-turn distance counts the displacement once ignited.
        if m.no_turn_left > 0.0 && m.state != MissileState::Launch {
            m.no_turn_left -= dist_3d(&m.no_turn_prev, &p.pos);
            m.no_turn_prev = p.pos;
        }
        // A marked missile diverts to the latest flare and continues with no warhead damage.
        if m.decoy_at != 0 && now >= m.decoy_at {
            m.decoy_at = 0;
            p.no_damage = true;
            if let Some(flare) = self.calculate_countermeasure_to_divert_to(p.victim) {
                p.victim = flare;
                if let Some(f) = self.find_object(flare) {
                    p.dest = f.position;
                }
            }
        }
        // `m_isTrackingTarget`: the goal is the victim's position while it lives (the AI keeps its last goal after it dies).
        let tracking = p.victim != ObjectId::INVALID;
        if let (true, Some(v)) = (tracking, victim) {
            m.goal_last = v.pos;
        }
        let goal = if tracking { m.goal_last } else { p.dest };
        let victim_alive = victim.is_some();

        let gravity = self.rules.gravity;
        match self.missile_ai(p, m, now, tracking, goal, victim_alive, victim) {
            MissileAi::Vanish => return true,
            MissileAi::Detonate(at) => {
                self.detonate_projectile(p, ObjectId::INVALID, at);
                return true;
            }
            MissileAi::Drive(drive_goal) => {
                // `moveTowardsPositionThrust` (the thrust and the orientation), then the braking cheat of `locoUpdate`.
                let thrust_goal = self.missile_thrust_goal(p.pos, drive_goal, m);
                missile_drive(p, m, now, drive_goal, thrust_goal, gravity);
                if m.braking {
                    Self::braking_cheat(p, m, drive_goal);
                }
            }
        }

        // PHASE_PHYSICS (PhysicsUpdate::update): the forces the AI added this frame, gravity and friction, then the position.
        p.prev_pos = start;
        let pending = m.accel;
        m.accel = Coord3D::ZERO;
        let motive = now < m.motive_until;
        let braking = m.braking;
        self.projectile_integrate(&mut p.pos, &mut m.vel, m.heading, pending, motive, m.params.friction, braking);
        let landed = self.projectile_clamp(&mut p.pos, &mut m.vel, &mut m.airborne);
        // `onCollide(nullptr)` -> `projectileHandleCollision(nullptr)` (MissileAIUpdate.cpp:298): an unarmed missile is inert;
        // a landing "somewhere else" does not detonate.
        if landed && m.armed && !missile_landing_is_inert(p.pos, goal) {
            self.detonate_projectile(p, ObjectId::INVALID, p.pos);
            return true;
        }
        // `projectileHandleCollision(other)` ignores everything until the warhead is armed.
        if m.armed && !braking {
            if let Some(hit) = self.projectile_collision(p) {
                if self.projectile_garrison_kill(p, hit) {
                    return true;
                }
                self.detonate_projectile(p, ObjectId::INVALID, p.pos);
                return true;
            }
        }
        false
    }

    /// `MissileAIUpdate::update`'s state switch and the AI move of this frame (AIUpdate.cpp:1002-1114). The state's
    /// handler sets the locomotor's `maxAccel` and turn rate (`m.accel_max`, `m.turn_on`); a transition happens inside
    /// it, and the drive of the same frame still runs with the settings it made (MissileAIUpdate.cpp:496-586). Returns
    /// what happens to the missile, and the goal the drive steers to.
    #[allow(clippy::too_many_arguments)]
    fn missile_ai(&mut self, p: &mut Projectile, m: &mut MissileFlight, now: u32, tracking: bool, goal: Coord3D, victim_alive: bool, victim: Option<GoalObject>) -> MissileAi {
        // A coordinate shot aims `APPROACH_HEIGHT` above its spot while it is not locked (`aiMoveToPosition`, :275-290).
        // `AIMoveToState::update` then lifts the state's goal position (AIStates.cpp:2085-2125); the machine's own
        // goal position (used by the 2D lock check) keeps the un-lifted spot.
        let aim = if !tracking && m.params.lock_distance > 0.0 { Coord3D::new(goal.x, goal.y, goal.z + APPROACH_HEIGHT) } else { goal };
        let drive_goal = missile_ai_goal(p.pos, m.vel, aim, if tracking { victim.as_ref() } else { None });
        // The KILL state's move goal is the same lift on the original target position (a coordinate shot re-aims at
        // `m_originalTargetPos` when it locks, MissileAIUpdate.cpp:567-572).
        let kill_goal = missile_ai_goal(p.pos, m.vel, goal, if tracking { victim.as_ref() } else { None });
        match m.state {
            MissileState::Launch => {
                // `doLaunchState`: no thrust until the ignition delay has passed.
                if now.saturating_sub(m.state_frame) >= m.params.ignition_delay {
                    // `doIgnitionState`, which the update falls through to in the same frame: armed, exhaust, fuel,
                    // `setMaxAcceleration(m_maxAccel)` and no turn.
                    m.armed = true;
                    // `doIgnitionState`: `m_exhaustID = createAttachedParticleSystemID(m_exhaustSysTmpl, obj)`.
                    m.exhaust_lit = true;
                    m.fuel_end = if m.params.fuel_lifetime != 0 { now + m.params.fuel_lifetime } else { u32::MAX };
                    m.accel_max = m.params.max_accel;
                    m.turn_on = false;
                    m.state = MissileState::AttackNoTurn;
                    m.state_frame = now;
                } else {
                    // `doLaunchState`: `setMaxAcceleration(0)`, `setMaxTurnRate(0)`.
                    m.accel_max = 0.0;
                    m.turn_on = false;
                }
                MissileAi::Drive(drive_goal)
            }
            MissileState::Ignition => MissileAi::Drive(drive_goal),
            MissileState::KillSelf => {
                // `doKillSelfState`: hold KillSelfDelay frames, then destroy (no warhead). It sets no locomotor state, so the
                // drive keeps the last settings and the goal.
                if now.saturating_sub(m.state_frame) >= m.params.kill_self_delay {
                    MissileAi::Vanish
                } else {
                    MissileAi::Drive(drive_goal)
                }
            }
            MissileState::AttackNoTurn | MissileState::Attack => {
                let entry = m.state;
                let fuel_out = now >= m.fuel_end;
                if fuel_out && m.params.detonate_on_no_fuel {
                    // `doAttackState`: `detonate()` when out of fuel.
                    return MissileAi::Detonate(p.pos);
                }
                // `doAttackState`: out of fuel `setMaxAcceleration(0)`, `setMaxTurnRate(0)` and `tossExhaust()`; otherwise
                // `setMaxAcceleration(m_maxAccel)` and the turn is `BIGNUM` only in `ATTACK` (`doAttackState(turnOK)`).
                if fuel_out {
                    m.accel_max = 0.0;
                    m.turn_on = false;
                    m.exhaust_lit = false;
                } else {
                    m.accel_max = m.params.max_accel;
                    m.turn_on = entry == MissileState::Attack;
                }
                // The lock check (`FROM_CENTER_2D`; halved when not tracking): `switchToState(KILL)` and return. For a
                // coordinate shot the goal becomes the spot itself (`aiMoveToPosition(&m_originalTargetPos)`), and the drive
                // of this frame still runs with the attack settings.
                let mut lock = m.params.lock_distance;
                if lock > 0.0 {
                    if !tracking {
                        lock *= 0.5;
                    }
                    if dist_2d(&p.pos, &goal) < lock {
                        m.state = MissileState::Kill;
                        m.state_frame = now;
                        return MissileAi::Drive(drive_goal);
                    }
                }
                // `doAttackState` inside `DistanceToTargetBeforeDiving` (MissileAIUpdate.cpp:551-574): the locomotor
                // goes `PRECISE_Z_POS`, so the missile dives onto the goal's height. The retail branch measures to the
                // goal position (RETAIL_COMPATIBLE_CRC is 1, GameDefines.h:103), 2D from centre to centre.
                if m.params.preferred_height > 0.0 && dist_2d(&p.pos, &goal) < m.params.dive_distance {
                    m.precise_z = true;
                }
                if entry == MissileState::AttackNoTurn && m.no_turn_left <= 0.0 {
                    m.state = MissileState::Attack;
                    m.state_frame = now;
                }
                // `airborneTargetGone` (`m_isTrackingTarget && goal == nullptr`, MissileAIUpdate.cpp:600): KILL_SELF, no warhead.
                if tracking && !victim_alive {
                    m.fuel_end = now;
                    m.state = MissileState::KillSelf;
                    m.state_frame = now;
                }
                MissileAi::Drive(drive_goal)
            }
            MissileState::Kill => {
                // `doKillState`: out of fuel -> `detonate()` with DetonateOnNoFuel, else `airborneTargetGone` (KILL_SELF) and
                // return before the BRAKING status is set, so the drive keeps the previous settings.
                if now >= m.fuel_end {
                    if m.params.detonate_on_no_fuel {
                        return MissileAi::Detonate(p.pos);
                    }
                    m.fuel_end = now;
                    m.state = MissileState::KillSelf;
                    m.state_frame = now;
                    return MissileAi::Drive(kill_goal);
                }
                // `doKillState`: the BRAKING status (the locomotor then cheats, and the physics skips the XY integration),
                // `setMaxAcceleration(m_maxAccel)` and a BIGNUM turn. The status is set in this frame, so the first KILL
                // frame already brakes.
                m.braking = true;
                m.accel_max = m.params.max_accel;
                m.turn_on = true;
                // `isIdle()`: the AI's move has arrived. `getLocomotorDistanceToGoal` returns the 3D distance to the AI
                // path's last node for a projectile (`KINDOF_PROJECTILE`, AIUpdate.cpp:2468-2472), and the quick path's last
                // node is the state's lifted goal (`computeQuickPath`, AIUpdate.cpp:1643-1646); `AIInternalMoveToState::update`
                // compares it with `CloseEnoughDist` (AIStates.cpp:1910).
                if arrived_at_goal(p.pos, kill_goal, m.params.close_enough_dist) {
                    if tracking && victim_alive {
                        // `getGoalObject()`: the bounding-sphere distance (`FROM_BOUNDINGSPHERE_3D` subtracts both spheres,
                        // PartitionManager.cpp:893-935) under `closeEnough` (`getMaxSpeedForCondition`): the missile is set on
                        // the goal object's position and detonates there (MissileAIUpdate.cpp:620-630). Otherwise the AI
                        // re-issues its move.
                        let centre = victim.map_or(goal, |v| v.centre);
                        let missile_r = p.template.projectile.as_ref().map_or(0.0, |pt| pt.bounding_radius);
                        let goal_r = victim.map_or(0.0, |v| v.sphere_radius);
                        let bs = bounding_sphere_gap(p.pos, centre, missile_r, goal_r);
                        let close_enough = if m.params.max_speed > 0.0 { m.params.max_speed } else { 1.0 };
                        if bs < close_enough {
                            p.pos = goal;
                            return MissileAi::Detonate(goal);
                        }
                    } else {
                        // No goal object (a coordinate shot, or the victim died): `isIdle()` detonates in place.
                        return MissileAi::Detonate(p.pos);
                    }
                }
                // `airborneTargetGone` once the victim is gone and the AI is not idle (`if (m_isTrackingTarget && goal == nullptr)`).
                if tracking && !victim_alive {
                    m.fuel_end = now;
                    m.state = MissileState::KillSelf;
                    m.state_frame = now;
                }
                MissileAi::Drive(kill_goal)
            }
        }
    }

    /// `Locomotor::moveTowardsPositionThrust`'s thrust goal (Locomotor.cpp:1935-1943): with a `PreferredHeight` and
    /// without `PRECISE_Z_POS` the missile aims at the cruise height above the ground under it, the height error
    /// damped by `PreferredHeightDamping`. Otherwise it aims at the goal's own position.
    fn missile_thrust_goal(&self, pos: Coord3D, goal: Coord3D, m: &MissileFlight) -> Coord3D {
        if m.params.preferred_height == 0.0 || m.precise_z {
            return goal;
        }
        let surface = self.surface_height(pos.x, pos.y);
        let delta = (m.params.preferred_height + surface - pos.z) * m.params.preferred_height_damping;
        Coord3D::new(goal.x, goal.y, pos.z + delta)
    }

    /// `Locomotor::getSurfaceHtAtPt` (Locomotor.cpp:2028-2040): the water surface where the point is underwater, else the
    /// terrain (no bridge decks).
    fn surface_height(&self, x: f32, y: f32) -> f32 {
        let (underwater, water_z, ground) = self.terrain.underwater_info(x, y);
        if underwater {
            water_z.unwrap_or(ground)
        } else {
            ground
        }
    }

    /// `ProjectileCollide` / `shouldProjectileCollideWith` basics: the projectile hits the first
    /// non-ally object whose bounding circle it overlaps. The detonation itself is positional (at the
    /// projectile's own position), so only the object id is returned.
    fn projectile_collision(&self, p: &Projectile) -> Option<ObjectId> {
        let r = p.template.projectile.as_ref().map_or(5.0, |pt| pt.bounding_radius).max(1.0);
        let src_player = self.objects.get(&p.source).map(|s| s.player);
        for cid in self.partition.objects_in_range(&p.pos, r + 15.0, DistanceMode::FromBoundingSphere2D) {
            if cid == p.source {
                continue;
            }
            let Some(o) = self.objects.get(&cid) else { continue };
            if o.destroyed {
                continue;
            }
            if let Some(sp) = src_player {
                if self.relationship(o.player, sp) == Relationship::Allies {
                    continue;
                }
            }
            let rad = o.template.geometry.bounding_circle_radius();
            // 3D bounding-sphere test: an arc that clears the object's height passes over it.
            let dz = (p.pos.z - o.position.z).abs();
            if p.pos.dist_2d(&o.position) <= r + rad && dz <= r + o.template.geometry.height {
                return Some(cid);
            }
        }
        None
    }

    /// `WeaponStore::update`: deal due delayed damage in the order it was scheduled.
    pub(crate) fn update_delayed_damage(&mut self) {
        let now = self.frame;
        let list = std::mem::take(&mut self.combat.delayed);
        let mut keep = Vec::with_capacity(list.len());
        for d in list {
            if now >= d.when {
                self.deal_damage_internal(d.source, d.victim, d.pos, &d.template, false);
            } else {
                keep.push(d);
            }
        }
        keep.append(&mut self.combat.delayed);
        self.combat.delayed = keep;
    }
}

/// `MissileAIUpdate.cpp`'s `APPROACH_HEIGHT`: a coordinate shot aims this far above its spot.
const APPROACH_HEIGHT: f32 = 10.0;

/// `Locomotor::moveTowardsPositionThrust` for the missile's current settings (`m.accel_max`, `m.turn_on`): the thrust heads
/// for `thrust_goal`, leaning at most `MaxThrustAngle` off the heading (not at all without a turn), and the velocity is
/// damped by `maxAccel / maxSpeed`. The thrust is applied only when it changes the speed or the heading (Locomotor.cpp:2003).
/// The orientation turns onto the velocity, or onto `goal` while braking (`:1980-2000`).
fn missile_drive(p: &Projectile, m: &mut MissileFlight, now: u32, goal: Coord3D, thrust_goal: Coord3D, gravity: f32) {
    let max_speed = if m.params.max_speed > 0.0 { m.params.max_speed } else { 0.01 };
    // `desired = clamp(MinSpeed, getMaxSpeedForCondition, max)` (`FAST_AS_POSSIBLE` asks for the max, so the clamp leaves it).
    // A braking locomotor (`Braking`; the three AuroraBomb weapons in the shipped data, the template default is BIGNUM,
    // Locomotor.cpp:289 and the zr-templates default) drops to `MinSpeed` inside `calcSlowDownDist` of its goal, with
    // the flight distance as `onPathDistToGoal`. `AIUpdate::doLocomotor` passes `computeFlightDistToGoal` for an airborne
    // locomotor (AIUpdate.cpp:2194-2197), whose projection is 2D (shipped AIPathfind.cpp:1046-1107), so the port's
    // `dist_2d` is the engine value; every shipped `MissileAIUpdate` locomotor has `Surfaces = AIR`
    // (`isDoingGroundMovement` false). The brake itself is `-Braking` while faster (Locomotor.cpp:1961).
    let forward = forward_speed_3d(m.vel, m.heading);
    let braking = m.params.braking;
    let mut desired_speed = m.params.max_speed;
    if braking > 0.0 && dist_2d(&p.pos, &goal) < calc_slow_down_dist(forward, m.params.min_speed, braking) {
        desired_speed = m.params.min_speed;
    }
    let delta = desired_speed - forward;
    let accel = if delta > 0.0 || braking == 0.0 { m.accel_max } else { -braking };
    // `calcDirectionToApplyThrust`: the thrust that steers the velocity (gravity included) at the thrust goal.
    let want = missile_thrust_direction(p.pos, thrust_goal, m.vel, gravity, accel).unwrap_or(m.heading);
    let max_angle = if m.turn_on { m.params.max_thrust_angle } else { 0.0 };
    let thrust_dir = rotate_toward(m.heading, want, max_angle);
    if thrust_is_applied(delta, thrust_dir, m.heading) {
        let damping = (accel / max_speed).clamp(0.0, 1.0);
        m.accel = Coord3D::new(
            m.accel.x + thrust_dir.x * accel - m.vel.x * damping,
            m.accel.y + thrust_dir.y * accel - m.vel.y * damping,
            m.accel.z + thrust_dir.z * accel - m.vel.z * damping,
        );
        // `applyMotiveForce` sets `isMotive` for `MOTIVE_FRAMES` (PhysicsUpdate.cpp:419-431).
        m.motive_until = now + MOTIVE_FRAMES;
    }
    // `tryToOrientInThisDirection3D` at the turn rate, after the thrust was worked out from the old heading. Nothing
    // moving means no orientation (`isNearlyZero(velocity magnitude)`).
    let speed = sqrt(m.vel.x * m.vel.x + m.vel.y * m.vel.y + m.vel.z * m.vel.z);
    if m.turn_on && speed != 0.0 {
        let dir = if m.braking { Coord3D::new(goal.x - p.pos.x, goal.y - p.pos.y, goal.z - p.pos.z) } else { m.vel };
        let dl = sqrt(dir.x * dir.x + dir.y * dir.y + dir.z * dir.z);
        if dl > 0.0 {
            m.heading = Coord3D::new(dir.x / dl, dir.y / dl, dir.z / dl);
        }
    }
}

/// `MissileAIUpdate::update`'s outcome for a frame: the missile keeps flying with the drive towards `Drive`'s goal, or it
/// detonates at a point (the detonation weapon is positional, so no victim object), or it is removed with no warhead.
enum MissileAi {
    Drive(Coord3D),
    Detonate(Coord3D),
    Vanish,
}

/// The landing rule of `MissileAIUpdate::projectileHandleCollision(nullptr)` (MissileAIUpdate.cpp:298): a landing "somewhere
/// else" (more than one cell above the goal and more than three cells from it) does not detonate.
fn missile_landing_is_inert(pos: Coord3D, goal: Coord3D) -> bool {
    let delta_z = pos.z - goal.z;
    delta_z > PATHFIND_CELL_SIZE_F && dist_3d(&pos, &goal) > 3.0 * PATHFIND_CELL_SIZE_F
}

/// The `ModelConditionFlags` a draw module sees: the sim's derived bits ([`crate::condition::derive`]) plus the
/// weapon-set conditions `Object::setWeaponSetFlag` mirrors into the drawable (`Object.cpp:3101-3120`,
/// `TheWeaponSetTypeToModelConditionTypeMap` WeaponSet.h:86-105), the armor crate upgrades the salvage crate sets
/// (`SalvageCrateCollide.cpp:216-225`), and the game-wide `NIGHT`/`SNOW` bits (`Object::friend_bindToDrawable`,
/// Object.cpp:3038-3048, from [`LaunchEnvironment`](crate::weapon::LaunchEnvironment)). `derive` sets none of them,
/// so without this the GLA technical's `WEAPONSET_CRATEUPGRADE_*` / `REALLYDAMAGED WEAPONSET_CRATEUPGRADE_*` launch
/// states or a night-only model could never be picked.
fn launch_conditions(o: &crate::object::Object, env: crate::weapon::LaunchEnvironment) -> u128 {
    let mut bits = o.model_conditions.bits() | env.bits();
    for (set, name) in [
        (crate::weapon::WEAPONSET_VETERAN, "WEAPONSET_VETERAN"),
        (crate::weapon::WEAPONSET_ELITE, "WEAPONSET_ELITE"),
        (crate::weapon::WEAPONSET_HERO, "WEAPONSET_HERO"),
        (crate::weapon::WEAPONSET_PLAYER_UPGRADE, "WEAPONSET_PLAYER_UPGRADE"),
        (crate::weapon::WEAPONSET_CRATEUPGRADE_ONE, "WEAPONSET_CRATEUPGRADE_ONE"),
        (crate::weapon::WEAPONSET_CRATEUPGRADE_TWO, "WEAPONSET_CRATEUPGRADE_TWO"),
    ] {
        if o.weapon_set_flags & set != 0 {
            if let Some(bit) = crate::condition::bit_for_name(name) {
                bits |= 1u128 << bit;
            }
        }
    }
    for (set, name) in [
        (crate::armor::ARMORSET_CRATE_UPGRADE_ONE, "ARMORSET_CRATEUPGRADE_ONE"),
        (crate::armor::ARMORSET_CRATE_UPGRADE_TWO, "ARMORSET_CRATEUPGRADE_TWO"),
    ] {
        if o.armor_set_flags & set != 0 {
            if let Some(bit) = crate::condition::bit_for_name(name) {
                bits |= 1u128 << bit;
            }
        }
    }
    bits
}

/// `AIStates.cpp:2085-2125` (`AIMoveToState::update`): the goal a missile's move state steers to and measures its arrival
/// from. A goal object's position is lifted by half its `MaxHeightAbovePosition`, then again by the same height difference
/// when the missile is below it (so a missile under a tall building climbs to clear it), and a non-immobile, physical
/// goal is led by `0.5 * distance * goalSpeed / mySpeed` along its facing. A coordinate shot's spot is lifted the same
/// way when the missile is below it. `GetGoalObject()` is null once the victim dies, so the lift applies without the lead.
fn missile_ai_goal(pos: Coord3D, my_vel: Coord3D, raw_goal: Coord3D, victim: Option<&GoalObject>) -> Coord3D {
    let mut goal = raw_goal;
    let half_height = victim.map_or(0.0, |v| v.half_height);
    goal.z += half_height;
    let z_delta = goal.z - pos.z;
    if z_delta > 0.0 {
        goal.z += z_delta;
    }
    if let Some(v) = victim {
        if v.has_physics && !v.immobile {
            let delta = Coord3D::new(goal.x - pos.x, goal.y - pos.y, goal.z - pos.z);
            let dist = sqrt(delta.x * delta.x + delta.y * delta.y + delta.z * delta.z);
            let mut my_speed = sqrt(my_vel.x * my_vel.x + my_vel.y * my_vel.y + my_vel.z * my_vel.z);
            if my_speed < 5.0 {
                my_speed = 5.0; // "avoid divide by 0."
            }
            let goal_speed = sqrt(v.velocity.x * v.velocity.x + v.velocity.y * v.velocity.y + v.velocity.z * v.velocity.z);
            let lead = 0.5 * dist * goal_speed / my_speed;
            goal.x += v.facing.x * lead;
            goal.y += v.facing.y * lead;
            goal.z += v.facing.z * lead;
        }
    }
    goal
}

/// `GeometryInfo::getMaxHeightAbovePosition` (Geometry.cpp:207-221): a sphere's major radius, else the height.
fn geometry_max_height_above_position(g: &crate::template::Geometry) -> f32 {
    match g.shape {
        crate::template::GeometryShape::Sphere => g.major_radius,
        _ => g.height,
    }
}

/// `GeometryInfo::getBoundingSphereRadius` (Geometry.cpp:487-514): the sphere's radius, a cylinder's half height or its
/// major radius, a box's corner distance. (`Geometry::bounding_sphere_radius` in `template.rs` is the sim's airborne
/// collision approximation; this is the partition manager's value for `FROM_BOUNDINGSPHERE_*` distance queries.)
pub(crate) fn geometry_bounding_sphere_radius(g: &crate::template::Geometry) -> f32 {
    match g.shape {
        crate::template::GeometryShape::Sphere => g.major_radius,
        crate::template::GeometryShape::Cylinder => (g.height * 0.5).max(g.major_radius),
        crate::template::GeometryShape::Box => {
            sqrt(g.major_radius * g.major_radius + g.minor_radius * g.minor_radius + (g.height * 0.5) * (g.height * 0.5))
        }
    }
}

/// The orientation whose X axis is `dir` with the Z axis up, as `Matrix3D::buildTransformMatrix` (roll 0). The Y and Z axes
/// are returned as well, since a tumbling shell turns about them (`[X, Y, Z]`).
fn basis_from_dir(dir: Coord3D) -> [Coord3D; 3] {
    let up = Coord3D::new(0.0, 0.0, 1.0);
    let x = dir;
    // Y = up x X; a vertical X has no roll reference, so Y falls back to the world Y axis.
    let mut y = Coord3D::new(up.y * x.z - up.z * x.y, up.z * x.x - up.x * x.z, up.x * x.y - up.y * x.x);
    let yl = sqrt(y.x * y.x + y.y * y.y + y.z * y.z);
    if yl > 1e-6 {
        y = Coord3D::new(y.x / yl, y.y / yl, y.z / yl);
    } else {
        y = Coord3D::new(0.0, 1.0, 0.0);
    }
    let z = Coord3D::new(x.y * y.z - x.z * y.y, x.z * y.x - x.x * y.z, x.x * y.y - x.y * y.x);
    [x, y, z]
}

/// `Matrix3D::Rotate_X/Y/Z` post-multiplied onto an orientation given by its local axes (`[X, Y, Z]`): the rotation by `a`
/// turns the two axes other than `idx`, in the object's frame (roll about X, pitch about Y, yaw about Z).
fn rotate_axes(axes: &mut [Coord3D; 3], idx: usize, a: f32) {
    if a == 0.0 {
        return;
    }
    let (s, c) = (sin(a), cos(a));
    let [x, y, z] = *axes;
    let lin = |p: Coord3D, cp: f32, q: Coord3D, cq: f32| Coord3D::new(cp * p.x + cq * q.x, cp * p.y + cq * q.y, cp * p.z + cq * q.z);
    match idx {
        0 => {
            axes[1] = lin(y, c, z, s);
            axes[2] = lin(y, -s, z, c);
        }
        1 => {
            axes[0] = lin(x, c, z, -s);
            axes[2] = lin(x, s, z, c);
        }
        _ => {
            axes[0] = lin(x, c, y, s);
            axes[1] = lin(x, -s, y, c);
        }
    }
}

/// `tryToRotateVector3D` (Locomotor.cpp:110-167) for a positive `max_angle`. A zero angle (`isNearlyZero`, :117-120) returns
/// `from`. Both directions are normalised first (:123-127). The normalised goal is returned when it lies within `max_angle`
/// (:146-147); otherwise the normalised current direction turns by `max_angle` about the normalised cross product (:150-164).
/// The percent form (a negative `maxAngle`, :129-141) is not ported: the callers pass a positive turn rate.
fn rotate_toward(from: Coord3D, to: Coord3D, max_angle: f32) -> Coord3D {
    if max_angle.abs() < 0.001 {
        return from;
    }
    let normalize = |v: Coord3D| {
        let n = crate::vector_math::vec3_normalize((v.x, v.y, v.z));
        Coord3D::new(n.0, n.1, n.2)
    };
    let cur = normalize(from);
    let goal = normalize(to);
    let c = (cur.x * goal.x + cur.y * goal.y + cur.z * goal.z).clamp(-1.0, 1.0);
    if acos(c).abs() <= max_angle {
        return goal;
    }
    let axis = normalized_cross(cur, goal);
    rotate_about_axis(cur, axis, max_angle)
}

/// `calcSlowDownDist` (Locomotor.cpp:81-90): the distance in which a speed `cur` brakes to `desired` at `braking`,
/// with the engine's 5% fudge.
fn calc_slow_down_dist(cur: f32, desired: f32, braking: f32) -> f32 {
    let delta = cur - desired;
    if delta <= 0.0 {
        return 0.0;
    }
    (delta * delta / braking.abs()) * 0.5 * 1.05
}

/// Where to drive to get a victim into weapon range (a simplified `Weapon::computeApproachTarget`):
/// a point on the line to the victim at 90% of the attack range (or midway between min and max
/// range for weapons with a minimum range).
pub(crate) fn approach_point(from: &Coord3D, src_r: f32, victim: &Coord3D, victim_r: f32, range: f32, min_range: f32) -> Coord3D {
    let mut gap = range * 0.9;
    if min_range > gap {
        gap = (min_range + range) * 0.5;
    }
    let want = gap + src_r + victim_r;
    let d = dist_2d(from, victim);
    if d <= 0.001 {
        return *victim;
    }
    let f = want / d;
    Coord3D::new(victim.x + (from.x - victim.x) * f, victim.y + (from.y - victim.y) * f, victim.z)
}

/// `PartitionManager::estimateTerrainExtremesAlongLine` (highest point): the highest terrain under
/// the straight line from `a` to `b`, sampled once per pathfind cell.
fn highest_terrain_along(a: &Coord3D, b: &Coord3D, terrain: &crate::terrain::TerrainLogic) -> f32 {
    let d = dist_2d(a, b);
    let steps = ceil(d / PATHFIND_CELL_SIZE_F).max(1.0) as u32;
    let mut high = a.z.max(b.z);
    for i in 0..=steps {
        let f = i as f32 / steps as f32;
        high = high.max(terrain.ground_height(a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f));
    }
    high
}

#[cfg(test)]
mod weapon_set_rebuild_tests {
    //! `WeaponSet::updateWeaponSet` for a set change (`WeaponSet.cpp:320-357`), through the port's
    //! flag path (`Object::setWeaponSetFlag` -> `replace_weapon_set_flags`). The current slot must go
    //! back to `PRIMARY_WEAPON` ("always start with the cur weapon being primary, even if there is no
    //! primary weapon", `:349-357`).
    use super::*;
    use crate::template::ObjectTemplate;
    use crate::terrain::TerrainLogic;
    use crate::weapon::{WeaponSetTemplate, WeaponTemplate, WEAPONSET_CRATEUPGRADE_ONE};
    use std::sync::Arc;

    fn gun(name: &str) -> Arc<WeaponTemplate> {
        let mut w = WeaponTemplate::new(name);
        w.primary_damage = 10.0;
        w.attack_range = 100.0;
        Arc::new(w)
    }

    /// `AIAttackState::chooseWeapon` left the object on the secondary slot; a crate upgrade switches
    /// the weapon set, and `updateWeaponSet` must reset the slot to PRIMARY.
    /// Revert check: replacing `CombatState::rebuild_weapon_set` with a plain `WeaponSet::from_template`
    /// keeps slot 1 and fails the first assert.
    #[test]
    fn a_weapon_set_change_resets_the_current_slot_to_primary() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 0);
        let mut t = ObjectTemplate::new("TwoSets");
        t.kind_of = crate::template::kind_of::VEHICLE | crate::template::kind_of::SELECTABLE;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.geometry.major_radius = 5.0;
        let mut normal = WeaponSetTemplate::default();
        normal.weapons[0] = Some(gun("Normal"));
        let mut upgraded = WeaponSetTemplate::default();
        upgraded.weapons[1] = Some(gun("Upgraded"));
        t.weapon_sets = vec![(0, normal), (WEAPONSET_CRATEUPGRADE_ONE, upgraded)];
        let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        g.objects.get_mut(&id).unwrap().combat.current_weapon_slot = 1;

        g.set_weapon_set_flag(id, WEAPONSET_CRATEUPGRADE_ONE);

        let o = g.find_object(id).expect("unit");
        assert_eq!(o.combat.current_weapon_slot, 0, "updateWeaponSet resets m_curWeapon to PRIMARY");
        assert!(o.combat.weapons.weapons[0].is_none(), "the upgraded set has no primary");
        assert!(o.combat.weapons.weapons[1].is_some(), "the upgraded set's secondary weapon");
    }
}

#[cfg(test)]
mod airborne_filter_tests {
    //! The airborne filters of the AI scan and the radius damage (`AIGuard.cpp:286-289`, `Weapon.cpp:1456`).
    use super::*;
    use crate::orders::GuardMode;
    use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
    use crate::terrain::TerrainLogic;
    use crate::weapon::{affects, WeaponSetTemplate, WeaponTemplate};
    use std::sync::Arc;

    fn unit(name: &str, kind: u64) -> ObjectTemplate {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind | kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
        t.geometry.major_radius = 5.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t
    }

    /// A guard with a 100-unit weapon that reaches the whole test field.
    fn gunner() -> ObjectTemplate {
        let mut t = unit("Gunner", 0);
        t.combat.auto_acquire_enemies = true;
        let mut w = WeaponTemplate::new("Hit");
        w.primary_damage = 1.0;
        w.attack_range = 100.0;
        let mut set = WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(w));
        t.combat.weapon_set = Some(set);
        t
    }

    /// A ground tank (no airborne locomotor) and a helicopter whose locomotor is `Wings`-style (air surface) but which
    /// is sitting on the ground: `isUsingAirborneLocomotor` is true for it, so `FlyingUnitsOnly` takes it.
    /// Reverting the `is_airborne_locomotor` guard (the old `AIRCRAFT` kind and 9-unit height test) makes the assert fail.
    #[test]
    fn flying_units_only_guard_takes_an_airborne_locomotor_on_the_ground() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let guard = g.create_object(&Arc::new(gunner()), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let tank = g.create_object(&Arc::new(unit("Tank", 0)), Coord3D::new(110.0, 100.0, 0.0), 0.0, 1);
        let mut heli = unit("Heli", kind_of::AIRCRAFT);
        heli.locomotor = Some(LocomotorParams::from_engine_units(LocomotorKind::Wings, 1.0, 0.0, 0.2, 0.05, 0.1, false));
        let heli = g.create_object(&Arc::new(heli), Coord3D::new(120.0, 100.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        g.reveal_map_permanently_for_player(0);
        g.objects.get_mut(&guard).unwrap().ai.suspended = true;
        g.objects.get_mut(&guard).unwrap().combat.guard_mode = GuardMode::FlyingUnitsOnly;

        let found = g.ai_idle_scan(guard, true, None, GuardMode::FlyingUnitsOnly, true);
        assert_eq!(found, Some(heli), "the airborne-locomotor helicopter is the only flying target (the tank is not)");
        assert_ne!(found, Some(tank));
    }

    /// `Weapon::fireWeapon`'s radius loop (`Weapon.cpp:1456`): a `DOESNT_AFFECT_AIRBORNE` weapon skips every victim
    /// `isSignificantlyAboveTerrain` (more than three frames of fall, `Thing.cpp:338-343`), not only aircraft. A
    /// vehicle hovering 20 units up is spared; the same vehicle on the ground is hit.
    /// Reverting the `significantly_above_terrain` check (back to the aircraft-kind test) makes the first assert fail.
    #[test]
    fn a_doesnt_affect_airborne_radius_skips_any_victim_above_the_terrain() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let mut shooter = gunner();
        let mut w = WeaponTemplate::new("Blast");
        w.primary_damage = 10.0;
        w.primary_damage_radius = 30.0;
        w.secondary_damage_radius = 30.0;
        w.affects = affects::ENEMIES | affects::DOESNT_AFFECT_AIRBORNE;
        let mut set = WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(w.clone()));
        shooter.combat.weapon_set = Some(set);
        let shooter = g.create_object(&Arc::new(shooter), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let hover = g.create_object(&Arc::new(unit("Hover", 0)), Coord3D::new(105.0, 100.0, 0.0), 0.0, 1);
        let ground = g.create_object(&Arc::new(unit("Ground", 0)), Coord3D::new(95.0, 100.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        g.objects.get_mut(&hover).unwrap().position.z = 20.0;

        g.deal_damage_internal(shooter, ObjectId::INVALID, Coord3D::new(100.0, 100.0, 0.0), &w, true);
        assert_eq!(g.objects[&hover].health(), 100.0, "a victim 20 units up is spared, whatever its kind");
        assert!(g.objects[&ground].health() < 100.0, "the ground victim is hit");
    }

    fn target(kind: u64, airborne: bool) -> TargetInfo {
        TargetInfo {
            pos: Coord3D::new(0.0, 0.0, 0.0),
            radius: 0.0,
            kind_of: kind,
            armor: None,
            id: ObjectId::INVALID,
            airborne,
            ballistic_missile: false,
            demotrap: false,
            parachute: false,
            clearable_garrison: false,
        }
    }

    /// `WeaponSet::getVictimAntiMask` (`WeaponSet.cpp:389-433`), branch by branch: a small missile is also a projectile
    /// and gets `SMALL_MISSILE` only; mines and demotraps are `MINE | GROUND`; an airborne vehicle, infantry or
    /// parachute gets its own air class; an airborne kind of none of those gets 0; anything else is `GROUND`.
    /// Reverting `victim_anti_mask` to the old airborne-only mapping makes the first assert fail (a missile was GROUND).
    #[test]
    fn the_victim_anti_mask_follows_get_victim_anti_mask() {
        use kind_of as k;
        let mut ballistic = target(k::STRUCTURE, false);
        ballistic.ballistic_missile = true;
        let mut demotrap = target(k::STRUCTURE, false);
        demotrap.demotrap = true;
        let mut parachute = target(k::AIRCRAFT, true);
        parachute.parachute = true;
        assert_eq!(GameLogic::victim_anti_mask(&target(k::SMALL_MISSILE | k::PROJECTILE, false)), anti::SMALL_MISSILE);
        assert_eq!(GameLogic::victim_anti_mask(&ballistic), anti::BALLISTIC_MISSILE);
        assert_eq!(GameLogic::victim_anti_mask(&target(k::PROJECTILE, false)), anti::PROJECTILE);
        assert_eq!(GameLogic::victim_anti_mask(&target(k::MINE, false)), anti::MINE | anti::GROUND);
        assert_eq!(GameLogic::victim_anti_mask(&demotrap), anti::MINE | anti::GROUND);
        assert_eq!(GameLogic::victim_anti_mask(&target(k::VEHICLE, true)), anti::AIRBORNE_VEHICLE);
        assert_eq!(GameLogic::victim_anti_mask(&target(k::INFANTRY, true)), anti::AIRBORNE_INFANTRY);
        assert_eq!(GameLogic::victim_anti_mask(&parachute), anti::PARACHUTE);
        assert_eq!(GameLogic::victim_anti_mask(&target(k::AIRCRAFT, true)), 0, "an airborne aircraft is no class of its own");
        assert_eq!(GameLogic::victim_anti_mask(&target(k::VEHICLE, false)), anti::GROUND);
    }

    /// `Object::isSignificantlyAboveTerrain` measures from the surface of the object's layer (`Object.cpp:2820-2825`).
    /// A unit standing on a bridge deck (its layer is the bridge's in C++) is not significantly above the terrain: the
    /// deck is the surface under it. A flier 12 above the deck is. Reverting the helper to the ground height makes the
    /// first assert fail (the deck, 8 up, reads as airborne).
    #[test]
    fn a_unit_on_a_bridge_deck_is_not_significantly_above_the_terrain() {
        use crate::bridge::Bridge;
        let mut terrain = TerrainLogic::flat(400, 400, 0);
        terrain.add_bridge(Bridge::new(Coord3D::new(0.0, 100.0, 8.0), Coord3D::new(400.0, 100.0, 8.0), 20.0, 2));
        let mut g = GameLogic::new(terrain, 1);
        let deck = g.create_object(&Arc::new(unit("Deck", 0)), Coord3D::new(200.0, 100.0, 0.0), 0.0, 0);
        let flier = g.create_object(&Arc::new(unit("Flier", 0)), Coord3D::new(200.0, 100.0, 0.0), 0.0, 0);
        g.objects.get_mut(&deck).unwrap().position.z = 8.0;
        g.objects.get_mut(&flier).unwrap().position.z = 20.0;
        assert!(!g.significantly_above_terrain(deck), "on the deck: the surface under it is the deck");
        assert!(g.significantly_above_terrain(flier), "20 up is 12 above the deck");
    }
}

#[cfg(test)]
mod r9_status_duration_tests {
    //! `DAMAGE_STATUS`'s duration and the zero-duration case (`ActiveBody.cpp:483-490`, `StatusDamageHelper.cpp:80-92`).
    use super::*;
    use crate::object::status;
    use crate::template::{kind_of, ObjectTemplate};
    use crate::terrain::TerrainLogic;
    use std::sync::Arc;

    /// `ActiveBody::attemptDamage` calls `doStatusDamage(type, REAL_TO_INT_CEIL(frames))` with no zero check, and
    /// `StatusDamageHelper::doStatusDamage` sets the status and its wake for `duration` (0 included), which also clears
    /// a *different* status the helper holds. The port returned before the call when the duration rounded to zero, so
    /// a zero-millisecond status hit did nothing at all.
    #[test]
    fn a_zero_length_status_hit_still_sets_the_status() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        let mut t = ObjectTemplate::new("Target");
        t.kind_of = kind_of::VEHICLE;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        let id = g.create_object(&Arc::new(t), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
        g.apply_status_damage(id, ObjectId::INVALID, status::MASKED, 0.0);
        assert!(g.objects[&id].status_mask(g.frame()) & status::MASKED != 0, "the zero-frame status is set immediately");
        assert_eq!(g.objects[&id].combat.helpers.status_damage.map(|(mask, _)| mask), Some(status::MASKED));
        // The helper is woken at `now`; its update clears the status (`clearStatusCondition`).
        g.update();
        assert!(g.objects[&id].status_mask(g.frame()) & status::MASKED == 0, "the zero-frame status clears on the helper's update");
    }
}

#[cfg(test)]
mod missile_goal_tests {
    //! `AIMoveToState::update`'s goal for a projectile (`AIStates.cpp:2085-2125`) and the geometry values the
    //! missile's KILL gate takes from it (`FROM_BOUNDINGSPHERE_3D`, PartitionManager.cpp:893-935).
    use super::*;
    use crate::template::{Geometry, GeometryShape};

    fn goal_object(pos: Coord3D, half_height: f32) -> GoalObject {
        GoalObject { pos, centre: pos, radius: 10.0, sphere_radius: 10.0, half_height, velocity: Coord3D::ZERO, facing: Coord3D::new(1.0, 0.0, 0.0), has_physics: false, immobile: false }
    }

    /// The state's goal is the victim's half height above its position, doubled while the missile is below that
    /// height (`m_goalPosition.z += halfHeight`, then `+= zDelta` when positive). The missile converges to the
    /// victim's geometry centre. Reverting the lift (the raw goal) makes this fail.
    #[test]
    fn the_missile_goal_is_lifted_to_the_victims_centre() {
        let victim = goal_object(Coord3D::new(400.0, 0.0, 0.0), 20.0);
        // From below the centre: 20 + (20 - 0) = 40.
        let from_below = missile_ai_goal(Coord3D::new(0.0, 0.0, 0.0), Coord3D::ZERO, victim.pos, Some(&victim));
        assert!((from_below.z - 40.0).abs() < 1e-4, "{from_below:?}");
        // Above the centre: only the half height.
        let from_above = missile_ai_goal(Coord3D::new(0.0, 0.0, 60.0), Coord3D::ZERO, victim.pos, Some(&victim));
        assert!((from_above.z - 20.0).abs() < 1e-4, "{from_above:?}");
        // A coordinate shot (no goal object) is not lifted unless the missile is below it.
        let shot = missile_ai_goal(Coord3D::new(0.0, 0.0, 0.0), Coord3D::ZERO, Coord3D::new(400.0, 0.0, 0.0), None);
        assert_eq!(shot.z, 0.0, "a level coordinate shot stays level");
        let uphill = missile_ai_goal(Coord3D::new(0.0, 0.0, 0.0), Coord3D::ZERO, Coord3D::new(400.0, 0.0, 10.0), None);
        assert!((uphill.z - 20.0).abs() < 1e-4, "an uphill shot aims twice the height difference up: {uphill:?}");
    }

    /// A goal object with physics that is not immobile is led by `0.5 * distance * goalSpeed / mySpeed` along its
    /// facing (`AIMoveToState::update`, AIStates.cpp:2100-2113). Reverting the lead makes this fail.
    #[test]
    fn a_moving_goal_with_physics_is_led() {
        let mut victim = goal_object(Coord3D::new(400.0, 0.0, 0.0), 0.0);
        victim.has_physics = true;
        victim.velocity = Coord3D::new(0.2, 0.0, 0.0);
        let own = Coord3D::new(7.5, 0.0, 0.0);
        // 0.5 * 400 * 0.2 / 7.5 = 5.333...
        let lead = missile_ai_goal(Coord3D::ZERO, own, victim.pos, Some(&victim));
        assert!((lead.x - 405.3333).abs() < 1e-2, "the goal is ahead of the victim: {lead:?}");
        assert_eq!(lead.y, 0.0);
        // The lead follows the victim's facing.
        victim.facing = Coord3D::new(0.0, 1.0, 0.0);
        let north = missile_ai_goal(Coord3D::ZERO, own, victim.pos, Some(&victim));
        assert!((north.y - 5.3333).abs() < 1e-2 && north.x == 400.0, "{north:?}");
        // An immobile goal, or one without physics, is not led.
        victim.immobile = true;
        assert_eq!(missile_ai_goal(Coord3D::ZERO, own, victim.pos, Some(&victim)).x, 400.0);
        victim.immobile = false;
        victim.has_physics = false;
        assert_eq!(missile_ai_goal(Coord3D::ZERO, own, victim.pos, Some(&victim)).x, 400.0);
        // A slow missile uses the 5-unit floor ("avoid divide by 0", AIStates.cpp:2101).
        let mut slow_victim = goal_object(Coord3D::new(400.0, 0.0, 0.0), 0.0);
        slow_victim.has_physics = true;
        slow_victim.velocity = Coord3D::new(0.2, 0.0, 0.0);
        let slow = missile_ai_goal(Coord3D::ZERO, Coord3D::ZERO, slow_victim.pos, Some(&slow_victim));
        assert!((slow.x - 408.0).abs() < 1e-2, "0.5 * 400 * 0.2 / 5 = 8: {slow:?}");
    }

    /// `GeometryInfo::getBoundingSphereRadius` (Geometry.cpp:487-514) and `getMaxHeightAbovePosition`
    /// (`:207-221`): the KILL gate's `FROM_BOUNDINGSPHERE_3D` uses the sphere radius, not the 2D footprint.
    #[test]
    fn the_geometry_values_are_the_engine_ones() {
        let sphere = Geometry { shape: GeometryShape::Sphere, major_radius: 10.0, minor_radius: 10.0, height: 20.0, is_small: true };
        assert_eq!(geometry_bounding_sphere_radius(&sphere), 10.0);
        assert_eq!(geometry_max_height_above_position(&sphere), 10.0);
        let cylinder = Geometry { shape: GeometryShape::Cylinder, major_radius: 5.0, minor_radius: 5.0, height: 40.0, is_small: false };
        assert_eq!(geometry_bounding_sphere_radius(&cylinder), 20.0);
        assert_eq!(geometry_max_height_above_position(&cylinder), 40.0);
        let tall_cylinder = Geometry { shape: GeometryShape::Cylinder, major_radius: 15.0, minor_radius: 15.0, height: 10.0, is_small: false };
        assert_eq!(geometry_bounding_sphere_radius(&tall_cylinder), 15.0);
        let boxx = Geometry { shape: GeometryShape::Box, major_radius: 10.0, minor_radius: 10.0, height: 20.0, is_small: false };
        assert!((geometry_bounding_sphere_radius(&boxx) - 300.0f32.sqrt()).abs() < 1e-4);
    }

    /// `W3DModelDraw::getProjectileLaunchOffset` uses `findBestInfo(condition)` with the drawable's conditions, and
    /// `Object::setWeaponSetFlag` mirrors the weapon-set flag into them; so a crate upgrade selects that state's barrel
    /// at fire time. Reverting `launch_position`/`barrel_count` to the default `slots` (or dropping the weapon-set bits
    /// from `launch_conditions`) leaves the shot at the origin and fails this.
    #[test]
    fn the_launch_state_follows_the_weapon_set_flags() {
        use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
        use crate::terrain::TerrainLogic;
        use crate::weapon::{CombatTemplate, LaunchBarrel, LaunchState, WeaponSetTemplate, WeaponTemplate};

        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        let mut w = WeaponTemplate::new("Gun");
        w.primary_damage = 10.0;
        w.primary_damage_radius = 2.0;
        w.attack_range = 500.0;
        w.has_projectile = true;
        w.weapon_speed = 5.0;
        let mut set = WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(w));
        let mut t = ObjectTemplate::new("Tank");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
        t.geometry.major_radius = 8.0;
        t.max_health = 200.0;
        t.initial_health = 200.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
        t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
        let barrel = |z: f32| LaunchBarrel { pos: [0.0, 0.0, z], rot: [0.0, 0.0, 0.0, 1.0] };
        t.combat.launch_bones.states = vec![
            LaunchState { conditions: vec![0], slots: [Vec::new(), Vec::new(), Vec::new()], turret: None },
            LaunchState {
                conditions: vec![1u128 << crate::condition::bit_for_name("WEAPONSET_CRATEUPGRADE_ONE").unwrap()],
                slots: [vec![barrel(12.0)], Vec::new(), Vec::new()],
                turret: None,
            },
        ];
        let a = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let b = g.create_object(&Arc::new(ObjectTemplate::new("Target")), Coord3D::new(150.0, 50.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        g.fire_weapon(a, 0, AttackTarget::Object(b));
        let plain = g.combat.projectiles.last().unwrap().pos.z;
        assert_eq!(plain, 0.0, "no upgrade: the default state has no launch bone");
        g.objects.get_mut(&a).unwrap().weapon_set_flags = crate::weapon::WEAPONSET_CRATEUPGRADE_ONE;
        g.objects.get_mut(&a).unwrap().combat.weapons.weapons[0].as_mut().unwrap().load_ammo_now(g.frame);
        g.fire_weapon(a, 0, AttackTarget::Object(b));
        let up = g.combat.projectiles.last().unwrap().pos.z;
        assert!((up - 12.0).abs() < 1e-4, "the crate-upgrade state's barrel is used (z {up})");
    }
}