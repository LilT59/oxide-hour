//! Possession: direct control of one unit per player (docs/POSSESSION.md, "Simulation side").
//!
//! Possession is just more logic messages. `Possess` / `Release` change [`GameLogic::possessed`];
//! `PossessInput` stores the unit's input for this frame; [`GameLogic::update_possession`] then
//! drives the unit once per frame, after the command list and before the combat / sleepy-update
//! passes. While possessed the unit's [`AiState::suspended`](crate::ai::AiState) is set, so the
//! `AiUpdate` module leaves it alone, `update_object_combat` skips it (no attack orders, no idle
//! auto-acquire) and `command_attack` / `MoveTo` / `Stop` ignore it. Other units are untouched.
//!
//! # Input convention
//!
//! The move vector is interpreted in **world XY** (+x east, +y north), not relative to the unit
//! or the camera. The client converts camera-relative WASD into a world vector before sending.
//! Its length is clamped to 1 and scales the locomotor's top speed (analog stick). `aim` is a
//! world-space point (the client ray-casts its crosshair against terrain/objects). A frame without
//! a `PossessInput` counts as "no move, no buttons" and the last aim point is kept.
//!
//! # Movement
//!
//! Through the unit's own locomotor, exactly as the AI would: with input,
//! `Locomotor::locoUpdate_moveTowardsPosition(obj, pos + move * lookahead)`
//! (Object/Locomotor.cpp; [`move_towards_position`]); without input,
//! `Locomotor::locoUpdate_maintainCurrentPosition` ([`maintain_current_position`]), which brakes
//! at the locomotor's braking rate. Top speed, acceleration, braking and turn rate all come from
//! the unit's [`LocomotorParams`]; nothing is added. The lookahead is far enough that the
//! locomotor's "brake to reach the goal" limit never slows a unit that is simply driving.
//!
//! # Aiming
//!
//! * Turreted units (`TurretTurnRate`): the turret slews toward the aim point at that rate
//!   (`TurretAI::friend_turnTowardsAngle`, the same step as the attack state), independent of the
//!   hull, so it works while driving.
//! * Non-turreted units: the body turns toward the aim point at the locomotor turn rate
//!   **only while there is no move input**. With move input the locomotor steers the body toward
//!   the move direction (locomotors cannot strafe). Immobile weapon owners snap.
//!
//! # Firing
//!
//! `FIRE` held: slot 0 (else the first present weapon), only when the aim is within the
//! acceptable aim delta of the turret / body heading (as `AIAttackAimAtTargetState`), the aim
//! point is inside `[min range, range]` (boundary distance, like `fireWeaponTemplate`) and the
//! weapon status is `ReadyToFire`. A pre-attack delay is honoured: the first ready frame calls
//! `Weapon::preFireWeapon`, the shot follows when the delay has elapsed
//! (`AIAttackFireWeaponState`). Releasing the button cancels a pending pre-attack. The shot goes
//! through the same `fire_weapon` as AI attacks, so bookkeeping, scatter, projectiles, delayed
//! damage, armor and veterancy are shared.
//!
//! * Projectile weapons fire at the **aim point** (`AttackTarget::Ground`, the fire-at-position
//!   path of `Weapon::fireWeapon(source, &pos)`); the projectile and its radius damage do the rest.
//! * Instant weapons (no projectile object: infantry guns and the like) are **hitscan**: a
//!   deterministic ray is cast from the unit (at half its geometry height) toward the aim point
//!   and continued out to weapon range. Candidates come from the partition manager and are taken
//!   in (distance along the ray, id) order; the first *enemy* object with a live body whose
//!   bounding circle the ray enters, at a height inside the object's geometry, becomes the victim.
//!   Allies and neutrals do not block. Terrain is sampled every [`HITSCAN_STEP`] units
//!   (then bisected) and wins if it is reached first; the shot then lands on the terrain point.
//!   TODO: pitch limits and real collision geometry (the sim only has bounding circles).
//!
//! `ALT_FIRE` is a documented hook ([`GameLogic::possession_alt_fire`]): special powers and unit
//! abilities are not ported yet, so it does nothing.
//!
//! # Exit
//!
//! `Release`, the unit dying / being destroyed, or its owner changing releases automatically
//! (checked at the start of every possession pass). The AI then resumes idle: path and attack
//! order cleared, the AI woken so it brakes. Invalid messages (not the owner, dead, structure,
//! wrong player, no possession) are ignored, and a unit that becomes contained (transport /
//! garrison) auto-releases: [`GameLogic::can_possess`] rejects contained units and the pass
//! treats `Object::contained_by` as invalid.

use crate::ai::{maintain_current_position, move_towards_position};
use crate::combat::{AttackTarget, AIM_REL_THRESH};
use crate::command::{buttons, PossessInput};
use crate::crc::Crc;
use crate::locomotor::handle_behavior_z;
use crate::logic::GameLogic;
use crate::math::{angle_diff, atan2, cos, normalize_angle, sin, sqrt, Coord3D, MAP_XY_FACTOR, TWO_PI};
use crate::object::{ObjectId, PlayerIndex};
use crate::partition::DistanceMode;
use crate::damage::Relationship;
use crate::template::{kind_of, LocomotorKind, ZBehavior};
use crate::weapon::WeaponStatus;

/// Terrain sampling interval of the hitscan ray, in world units.
pub const HITSCAN_STEP: f32 = 5.0;
/// Move vectors shorter than this (in -1..1 units) count as no input.
const MOVE_DEADZONE: f32 = 0.01;

/// Per-player possession state. Hashed into the CRC (only while someone is possessing).
#[derive(Clone, Debug, PartialEq)]
pub struct PossessState {
    pub unit: ObjectId,
    /// Input received since the last pass.
    input: Option<PossessInput>,
    /// Last aim point sent; the turret keeps pointing there when frames carry no input.
    aim: Option<Coord3D>,
    /// A pre-attack was started and the shot is pending.
    pre_firing: bool,
    /// Target altitude for air units (`ASCEND`/`DESCEND`), world Z. `None` until the first frame.
    goal_z: Option<f32>,
}

impl PossessState {
    fn new(unit: ObjectId) -> Self {
        Self { unit, input: None, aim: None, pre_firing: false, goal_z: None }
    }

    pub(crate) fn crc(&self, c: &mut Crc) {
        c.add_u32(self.unit.0);
        match &self.aim {
            Some(a) => {
                c.add_bool(true);
                c.add_coord(a);
            }
            None => c.add_bool(false),
        }
        c.add_bool(self.pre_firing);
        match self.goal_z {
            Some(z) => {
                c.add_bool(true);
                c.add_f32(z);
            }
            None => c.add_bool(false),
        }
    }
}

impl GameLogic {
    /// The unit `player` currently possesses.
    pub fn possessed_unit(&self, player: PlayerIndex) -> Option<ObjectId> {
        self.possessed.get(&player).map(|s| s.unit)
    }

    /// Whether any player possesses `id`.
    pub fn is_possessed(&self, id: ObjectId) -> bool {
        self.possessed.values().any(|s| s.unit == id)
    }

    /// Validation shared by `Possess`: owned by `player`, alive, not a structure, not already
    /// possessed by someone else, not contained.
    pub fn can_possess(&self, player: PlayerIndex, id: ObjectId) -> bool {
        let Some(o) = self.find_object(id) else { return false };
        o.player == player
            && o.body.as_ref().is_some_and(|b| !b.is_dead())
            && o.template.kind_of & kind_of::STRUCTURE == 0
            && o.contained_by.is_none()
            && !self.possessed.iter().any(|(p, s)| s.unit == id && *p != player)
    }

    /// `MSG_POSSESS`.
    pub(crate) fn command_possess(&mut self, player: PlayerIndex, id: ObjectId) {
        if !self.can_possess(player, id) {
            return;
        }
        if self.possessed.get(&player).is_some_and(|s| s.unit == id) {
            return; // already possessing it
        }
        self.release_player(player); // one unit per player: possessing another switches
        // aiIdle: drop the path and any attack order (cancels a pending pre-attack), then suspend.
        self.end_attack(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.stop();
            o.ai.suspended = true;
        }
        // A parked / taxiing jet is flown by hand: make it airborne-capable (normal locomotor).
        self.jet_begin_possession(id);
        let mut st = PossessState::new(id);
        // ... and one that sits on the ground climbs to its cruise height (ASCEND/DESCEND move the goal).
        if let Some(o) = self.objects.get(&id) {
            if o.template.jet.is_some() {
                if let Some(l) = o.locomotor() {
                    let surface = self.terrain.ground_height(o.position.x, o.position.y);
                    if l.is_airborne() && o.position.z - surface < l.preferred_height {
                        st.goal_z = Some(surface + l.preferred_height);
                    }
                }
            }
        }
        self.possessed.insert(player, st);
    }

    /// `MSG_POSSESS_INPUT`: remembered until this frame's pass; ignored if `player` possesses nothing.
    pub(crate) fn command_possess_input(&mut self, player: PlayerIndex, input: PossessInput) {
        if let Some(s) = self.possessed.get_mut(&player) {
            s.input = Some(input);
        }
    }

    /// `MSG_RELEASE_POSSESSION`.
    pub(crate) fn command_release(&mut self, player: PlayerIndex) {
        self.release_player(player);
    }

    /// Clear `player`'s possession and hand the unit back to the (idle) AI.
    fn release_player(&mut self, player: PlayerIndex) {
        let Some(st) = self.possessed.remove(&player) else { return };
        let now = self.frame;
        let mut wake = false;
        if let Some(o) = self.objects.get_mut(&st.unit) {
            for w in o.combat.weapons.weapons.iter_mut().flatten() {
                w.cancel_pre_attack(now);
            }
            o.ai.suspended = false;
            o.ai.stop();
            o.combat.attack = None;
            wake = o.ai_handle.is_some();
        }
        if wake {
            // The AI wakes next frame and brakes the unit to a halt. Port-only (no C++ equivalent): the wake is the
            // ordinary forward-only pull, so an AI already due sooner keeps its slot.
            self.wake_up_now(st.unit);
        }
    }

    /// Hook for `ALT_FIRE` (special power / ability buttons with the aim point as target).
    /// TODO: map to the unit's command buttons once special powers are ported.
    pub fn possession_alt_fire(&mut self, unit: ObjectId, aim: Coord3D) {
        // ALT_FIRE triggers the possessed unit's first special ability (Black Lotus capture,
        // Jarmen Kell snipe, Burton charges) or special power at the aim point
        // (`SpecialAbilityUpdate` / `SpecialPowerStore`).
        let power = self.find_object(unit).and_then(|o| {
            o.template
                .special_abilities
                .first()
                .map(|a| a.power.clone())
                .or_else(|| o.template.special_powers.first().map(|p| p.power.clone()))
        });
        let player = self.possessed.iter().find(|(_, s)| s.unit == unit).map(|(p, _)| *p);
        if let (Some(power), Some(player)) = (power, player) {
            self.command_do_special_power(player, unit, &power, crate::special_power::SpecialPowerTarget::Position(aim));
        }
    }

    /// One possession pass: auto-release, then move, aim and fire every possessed unit
    /// (players in index order).
    pub(crate) fn update_possession(&mut self) {
        if self.possessed.is_empty() {
            return;
        }
        let players: Vec<PlayerIndex> = self.possessed.keys().copied().collect();
        for p in players {
            let Some(mut st) = self.possessed.get(&p).cloned() else { continue };
            let valid = self
                .objects
                .get(&st.unit)
                .is_some_and(|o| !o.destroyed && o.player == p && o.contained_by.is_none() && o.body.as_ref().is_none_or(|b| !b.is_dead()));
            if !valid {
                self.release_player(p);
                continue;
            }
            let had_input = st.input.is_some();
            let input = st.input.take().unwrap_or(PossessInput { move_x: 0, move_y: 0, aim: Coord3D::ZERO, buttons: 0 });
            if had_input && input.aim.x.is_finite() && input.aim.y.is_finite() && input.aim.z.is_finite() {
                st.aim = Some(input.aim);
            }
            let pressed = if had_input { input.buttons } else { 0 };

            self.possess_move_and_aim(&mut st, &input, had_input, pressed);
            if pressed & buttons::ALT_FIRE != 0 {
                if let Some(a) = st.aim {
                    self.possession_alt_fire(st.unit, a);
                }
            }
            self.possess_fire(&mut st, pressed & buttons::FIRE != 0);
            if let Some(slot) = self.possessed.get_mut(&p) {
                *slot = st;
            }
        }
    }

    /// Locomotor movement plus turret / body aiming for one unit.
    ///
    /// Ground and hover/thrust units steer with the world-space move vector. Fixed-wing aircraft
    /// (`Wings`) read it as throttle (`move_y`) and turn (`move_x`) because they cannot stop or
    /// strafe. Air units also change altitude toward `st.goal_z` from `ASCEND`/`DESCEND`, clamped to
    /// the locomotor's envelope (surface + 2 up to twice its cruise height).
    fn possess_move_and_aim(&mut self, st: &mut PossessState, input: &PossessInput, had_input: bool, pressed: u8) {
        // The unit's own locomotor with its body damage state (`MovementPenaltyDamageState`): a possessed unit
        // moves by its own rules, so the damaged speed, turn rate, acceleration and lift apply as for the AI.
        let damage_loco = self.locomotor_now(st.unit);
        let terrain = &self.terrain;
        let Some(o) = self.objects.get_mut(&st.unit) else { return };
        let (mx, my) = if had_input {
            // -32768 clamps to -32767 so the range is symmetric.
            (f32::from(input.move_x.max(-32767)) / 32767.0, f32::from(input.move_y.max(-32767)) / 32767.0)
        } else {
            (0.0, 0.0)
        };
        let len = sqrt(mx * mx + my * my);
        let moving = len > MOVE_DEADZONE;
        let loco = damage_loco;

        if let Some(base) = loco {
            let mut pos = o.position;
            let mut angle = o.angle;
            let mut speed = o.ai.speed;
            let mut l = base;
            if base.is_airborne() || base.kind == LocomotorKind::Hover {
                // Altitude goal, moved by ASCEND/DESCEND at the locomotor's climb rate.
                let surface = terrain.ground_height(pos.x, pos.y);
                let goal = st.goal_z.get_or_insert(pos.z);
                let step = (base.max_speed * 0.5).max(base.lift).max(1.0);
                let (up, down) = (pressed & buttons::ASCEND != 0, pressed & buttons::DESCEND != 0);
                if up != down {
                    *goal += if up { step } else { -step };
                }
                let max_z = surface + base.preferred_height.max(60.0) * 2.0;
                *goal = goal.clamp(surface + 2.0, max_z);
                l.preferred_height = match base.behavior_z {
                    ZBehavior::AbsoluteHeight => *goal,
                    _ => (*goal - surface).max(0.0),
                };
            }
            match base.kind {
                LocomotorKind::Wings => {
                    // Fixed wing: W/S throttle (never below MinSpeed), A/D turn, always flying.
                    angle = normalize_angle(angle + base.max_turn_rate * mx.clamp(-1.0, 1.0));
                    let target = (base.max_speed * my.max(0.0)).max(base.min_speed);
                    speed = if speed < target {
                        (speed + base.acceleration).min(target)
                    } else {
                        (speed - base.braking).max(target)
                    }
                    .max(base.min_speed);
                    pos.x += cos(angle) * speed;
                    pos.y += sin(angle) * speed;
                    let goal = Coord3D::new(pos.x, pos.y, pos.z);
                    handle_behavior_z(&l, &mut pos, terrain, &goal);
                }
                _ if moving => {
                    let mag = len.min(1.0);
                    // Analog stick: scale top speed, never exceed the unit's own.
                    l.max_speed = base.max_speed * mag;
                    // Far enough that the "brake to reach the goal" limit is never the binding one.
                    let lookahead = (MAP_XY_FACTOR * 3.0).max(if base.braking > 0.0 { 2.0 * base.max_speed * base.max_speed / base.braking } else { 0.0 });
                    let goal = Coord3D::new(pos.x + mx / len * lookahead, pos.y + my / len * lookahead, pos.z);
                    move_towards_position(&l, &mut pos, &mut angle, &mut speed, &goal, terrain);
                }
                _ => {
                    maintain_current_position(&l, &mut pos, angle, &mut speed, terrain);
                }
            }
            o.set_position(pos);
            o.set_orientation(angle);
            o.ai.speed = speed;
        }

        // Aiming
        let Some(aim) = st.aim else {
            o.combat.turret_angle = if o.template.combat.turret_turn_rate.is_some() { o.combat.turret_angle } else { o.angle };
            return;
        };
        let (dx, dy) = (aim.x - o.position.x, aim.y - o.position.y);
        if dx * dx + dy * dy <= 1e-6 {
            return;
        }
        let desired = atan2(dy, dx);
        if let Some(rate) = o.template.combat.turret_turn_rate {
            // TurretAI::friend_turnTowardsAngle: snap when closer than one step.
            let diff = angle_diff(desired, o.combat.turret_angle);
            let a = if diff.abs() < rate {
                desired
            } else if diff > 0.0 {
                o.combat.turret_angle + rate
            } else {
                o.combat.turret_angle - rate
            };
            o.combat.turret_angle = normalize_angle(a);
        } else {
            // Fixed-wing aircraft hold their flight heading; everything else turns toward the aim
            // when not moving (locomotors cannot strafe).
            if !moving && loco.is_none_or(|l| l.kind != LocomotorKind::Wings) {
                let rel = angle_diff(desired, o.angle);
                let step = loco.map_or(TWO_PI, |l| l.max_turn_rate);
                let a = if rel.abs() < step || step <= 0.0 {
                    desired
                } else if rel > 0.0 {
                    normalize_angle(o.angle + step)
                } else {
                    normalize_angle(o.angle - step)
                };
                o.set_orientation(a);
            }
            o.combat.turret_angle = o.angle;
        }
    }

    /// Fire handling for one unit (see the module docs).
    fn possess_fire(&mut self, st: &mut PossessState, fire_held: bool) {
        let now = self.frame;
        let unit = st.unit;
        let cancel = |this: &mut GameLogic, st: &mut PossessState, slot: Option<usize>| {
            if st.pre_firing {
                if let (Some(slot), Some(o)) = (slot, this.objects.get_mut(&unit)) {
                    if let Some(Some(w)) = o.combat.weapons.weapons.get_mut(slot) {
                        w.cancel_pre_attack(now);
                    }
                }
            }
            st.pre_firing = false;
        };
        let Some(o) = self.objects.get(&unit) else { return };
        // A held weapon lock wins (`WeaponSet::isCurWeaponLocked`, as `chooseBestWeaponForTarget`), else the first weapon.
        let weapons = &o.combat.weapons;
        let Some(slot) = weapons.locked_slot().filter(|&s| weapons.weapon(s).is_some()).or_else(|| weapons.weapons.iter().position(Option::is_some)) else {
            return;
        };
        if !fire_held {
            cancel(self, st, Some(slot));
            return;
        }
        let Some(aim) = st.aim else { return };
        let w = o.combat.weapons.weapon(slot).expect("slot has a weapon");
        let wt = w.template.clone();
        let (pos, src_r) = (o.position, o.template.geometry.bounding_circle_radius());

        // Range: boundary distance to the aim point, as fireWeaponTemplate.
        let gap = (pos.dist_2d(&aim) - src_r).max(0.0);
        let (range, min_range) = (wt.attack_range(), wt.min_attack_range());
        if gap > range || gap < min_range {
            cancel(self, st, Some(slot));
            return;
        }
        // Alignment (AIAttackAimAtTargetState).
        let heading = if o.template.combat.turret_turn_rate.is_some() { o.combat.turret_angle } else { o.angle };
        let desired = atan2(aim.y - pos.y, aim.x - pos.x);
        if angle_diff(desired, heading).abs() > wt.acceptable_aim_delta.max(AIM_REL_THRESH) {
            return;
        }
        match w.status(now) {
            WeaponStatus::PreAttack => {}
            WeaponStatus::ReadyToFire => {
                // The shot's victim decides the pre-attack count: `Weapon::getPreAttackDelay` reads the shooter's
                // `FiringTracker` (`getNumConsecutiveShotsFiredAtTarget`, Weapon.cpp:2949), not a count kept here.
                let target = if wt.has_projectile { AttackTarget::Ground(aim) } else { self.hitscan_target(unit, &aim, range) };
                let victim = match target {
                    AttackTarget::Object(v) => v,
                    AttackTarget::Ground(_) => ObjectId::INVALID,
                };
                let shots = self.firing_consecutive_shots(unit, victim).max(0) as u32;
                if !st.pre_firing {
                    let delay = w.pre_attack_delay(shots);
                    if delay > 0 {
                        if let Some(Some(w)) = self.objects.get_mut(&unit).map(|o| &mut o.combat.weapons.weapons[slot]) {
                            w.pre_fire(now, shots);
                        }
                        st.pre_firing = true;
                        return;
                    }
                }
                st.pre_firing = false;
                // A spent clip releases the temporary lock (`Object::fireCurrentWeapon`, `Object.cpp:1534`). Possession is
                // a port-only addition, but its shot is the unit's own `fireCurrentWeapon`: readiness is checked above, so
                // the firing tracker (`FiringTracker::shotFired`) sees it exactly as it sees a player's normal AI shot.
                if self.fire_weapon(unit, slot, target) {
                    self.release_weapon_lock(unit, crate::weapon::WeaponLockType::Temporarily);
                }
            }
            _ => st.pre_firing = false, // between shots / reloading / out of ammo
        }
    }

    /// Deterministic hitscan ray (see the module docs): the first enemy object along the ray from
    /// `shooter` through `aim`, else the terrain point the ray reaches (or its end at `range`).
    pub fn hitscan_target(&self, shooter: ObjectId, aim: &Coord3D, range: f32) -> AttackTarget {
        let Some(s) = self.find_object(shooter) else { return AttackTarget::Ground(*aim) };
        let origin = Coord3D::new(s.position.x, s.position.y, s.position.z + s.template.geometry.height * 0.5);
        let (dx, dy, dz) = (aim.x - origin.x, aim.y - origin.y, aim.z - origin.z);
        let len = sqrt(dx * dx + dy * dy + dz * dz);
        if len <= 0.001 || range <= 0.0 {
            return AttackTarget::Ground(*aim);
        }
        let d = Coord3D::new(dx / len, dy / len, dz / len);
        let at = |t: f32| Coord3D::new(origin.x + d.x * t, origin.y + d.y * t, origin.z + d.z * t);
        let on_ground = |p: Coord3D| Coord3D::new(p.x, p.y, self.terrain.ground_height(p.x, p.y));

        // Terrain: first sample below the surface, then bisect between it and the previous one.
        let mut t_terrain = range;
        let mut terrain_hit = false;
        let mut prev = 0.0f32;
        let mut t = HITSCAN_STEP;
        loop {
            let tt = t.min(range);
            let p = at(tt);
            if p.z < self.terrain.ground_height(p.x, p.y) {
                let (mut lo, mut hi) = (prev, tt);
                for _ in 0..8 {
                    let mid = (lo + hi) * 0.5;
                    let m = at(mid);
                    if m.z < self.terrain.ground_height(m.x, m.y) {
                        hi = mid;
                    } else {
                        lo = mid;
                    }
                }
                t_terrain = hi;
                terrain_hit = true;
                break;
            }
            if tt >= range {
                break;
            }
            prev = tt;
            t += HITSCAN_STEP;
        }

        // Objects: horizontal projection of the ray.
        let h = sqrt(d.x * d.x + d.y * d.y);
        let mut best: Option<(f32, ObjectId)> = None;
        if h > 1e-4 {
            let (ux, uy) = (d.x / h, d.y / h);
            let mid = at(range * 0.5);
            let mut hits: Vec<(u32, ObjectId)> = Vec::new();
            for cid in self.partition.objects_in_range(&mid, range * 0.5 * h, DistanceMode::FromBoundingSphere2D) {
                if cid == shooter {
                    continue;
                }
                let Some(v) = self.find_object(cid) else { continue };
                if v.body.as_ref().is_none_or(|b| b.is_dead()) || self.relationship(s.player, v.player) != Relationship::Enemies {
                    continue;
                }
                let (cx, cy) = (v.position.x - origin.x, v.position.y - origin.y);
                let r = v.template.geometry.bounding_circle_radius();
                let along = cx * ux + cy * uy; // distance along the horizontal ray
                let perp2 = cx * cx + cy * cy - along * along;
                if perp2 > r * r {
                    continue;
                }
                let half = sqrt(r * r - perp2);
                let (s_in, s_out) = (along - half, along + half);
                if s_out < 0.0 {
                    continue;
                }
                let s_in = s_in.max(0.0);
                let s_mid = along.clamp(s_in, s_out);
                let z_ray = origin.z + d.z / h * s_mid;
                if z_ray < v.position.z - 1.0 || z_ray > v.position.z + v.template.geometry.height.max(1.0) {
                    continue;
                }
                let t_in = s_in / h;
                if t_in <= range {
                    hits.push((t_in.to_bits(), cid)); // t_in >= 0, so bit order is value order
                }
            }
            hits.sort_unstable();
            best = hits.first().map(|&(bits, id)| (f32::from_bits(bits), id));
        }
        match best {
            Some((t_obj, id)) if !terrain_hit || t_obj <= t_terrain => AttackTarget::Object(id),
            _ => AttackTarget::Ground(on_ground(at(t_terrain))),
        }
    }
}
