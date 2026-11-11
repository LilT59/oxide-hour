//! Superweapon / death-adjacent update modules (`GameLogic/Object/Update/*`).
//!
//! Ported here: `HeightDieUpdate` (`HeightDieUpdate.cpp`), `NeutronMissileUpdate`
//! (`NeutronMissileUpdate.cpp`), the neutron warhead blasts
//! (`NeutronMissileSlowDeathBehavior`, runtime lives in [`crate::die`]), `SpyVisionUpdate`
//! (`SpyVisionUpdate.cpp`), the particle uplink cannon (`ParticleUplinkCannonUpdate.cpp`), the
//! Spectre gunship (`SpectreGunshipDeploymentUpdate.cpp` / `SpectreGunshipUpdate.cpp`) and the
//! missile launcher building door (`MissileLauncherBuildingUpdate.cpp`).
//!
//! Client-only FX/sounds/beams/decals are recorded as FX events; the sim owns the gameplay state
//! (damage, kills, money, reveals, model conditions). Reductions are documented on each function.

use std::sync::Arc;

use crate::logic::GameLogic;
use crate::math::{acos, atan2, cos, sin, sqrt, Coord3D, LOGICFRAMES_PER_SECOND, PI, TWO_PI};
use crate::object::ObjectId;
use crate::weapon::WeaponTemplate;

/// `HeightDieUpdateModuleData` (sim units).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeightDieTemplate {
    pub target_height: f32,
    pub target_height_includes_structures: bool,
    pub only_when_moving_down: bool,
    pub destroy_attached_particles_at_height: f32,
    pub snap_to_ground_on_death: bool,
    pub initial_delay: u32,
}

/// `NeutronMissileUpdateModuleData` (sim units).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NeutronMissileTemplate {
    pub distance_to_travel_before_turning: f32,
    pub max_turn_rate: f32,
    pub forward_damping: f32,
    pub relative_speed: f32,
    pub target_from_directly_above: f32,
    pub special_speed_time: u32,
    pub special_speed_height: f32,
    pub special_accel_factor: f32,
    pub special_jitter_distance: f32,
    pub launch_fx: Option<String>,
    pub ignition_fx: Option<String>,
}

/// `SpyVisionUpdateModuleData` (sim units).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpyVisionTemplate {
    pub needs_upgrade: bool,
    pub self_powered: bool,
    pub self_powered_duration: u32,
    pub self_powered_interval: u32,
    /// `SpyOnKindof`: which enemy units this reveal spies the vision of.
    pub spy_on_kind_of: u64,
}

/// `ParticleUplinkCannonUpdateModuleData` (sim units).
#[derive(Clone, Debug)]
pub struct ParticleUplinkTemplate {
    pub special_power: Option<String>,
    pub begin_charge_frames: u32,
    pub raise_antenna_frames: u32,
    pub ready_delay_frames: u32,
    pub width_grow_frames: u32,
    pub beam_travel_frames: u32,
    pub total_firing_frames: u32,
    pub reveal_range: f32,
    pub swath_of_death_distance: f32,
    pub swath_of_death_amplitude: f32,
    pub total_scorch_marks: u32,
    pub scorch_mark_scalar: f32,
    /// The laser drawable's base width (`getLaserTemplateWidth`,
    /// `ParticleUplinkCannon_OrbitalLaser`'s `OuterBeamWidth 26.0 * 0.5`).
    pub beam_radius: f32,
    pub total_damage_pulses: u32,
    pub damage_per_second: f32,
    pub damage_radius_scalar: f32,
    pub damage_type: crate::damage::DamageType,
    pub death_type: crate::damage::DeathType,
    pub remnant_object: Option<String>,
    pub manual_driving_speed: f32,
    pub manual_fast_driving_speed: f32,
    pub double_click_to_fast_drive_delay: u32,
    pub ground_hit_fx: Option<String>,
    pub beam_launch_fx: Option<String>,
    pub delay_between_launch_fx: u32,
}

impl Default for ParticleUplinkTemplate {
    fn default() -> Self {
        ParticleUplinkTemplate {
            special_power: None,
            begin_charge_frames: 0,
            raise_antenna_frames: 0,
            ready_delay_frames: 0,
            width_grow_frames: 0,
            beam_travel_frames: 0,
            total_firing_frames: 0,
            reveal_range: 0.0,
            swath_of_death_distance: 0.0,
            swath_of_death_amplitude: 0.0,
            total_scorch_marks: 0,
            scorch_mark_scalar: 1.0,
            beam_radius: 13.0,
            total_damage_pulses: 0,
            damage_per_second: 0.0,
            damage_radius_scalar: 1.0,
            damage_type: crate::damage::DamageType::Explosion,
            death_type: crate::damage::DeathType::Normal,
            remnant_object: None,
            manual_driving_speed: 0.0,
            manual_fast_driving_speed: 0.0,
            double_click_to_fast_drive_delay: 0,
            ground_hit_fx: None,
            beam_launch_fx: None,
            delay_between_launch_fx: 30,
        }
    }
}

/// `SpectreGunshipDeploymentUpdateModuleData` (sim units).
#[derive(Clone, Debug, Default)]
pub struct SpectreDeploymentTemplate {
    pub gunship: Option<Arc<crate::template::ObjectTemplate>>,
    pub attack_area_radius: f32,
    /// `CreateLocation`: 0 near source, 1 farthest from source, 2 near target, 3 farthest from target.
    pub create_location: u8,
    pub gunship_orbit_radius: f32,
    pub special_power: Option<String>,
}

/// `SpectreGunshipUpdateModuleData` (sim units).
#[derive(Clone, Debug, Default)]
pub struct SpectreGunshipTemplate {
    pub special_power: Option<String>,
    pub howitzer_firing_rate: u32,
    pub howitzer_follow_lag: u32,
    pub orbit_time: u32,
    pub attack_area_radius: f32,
    pub targeting_reticle_radius: f32,
    pub orbit_radius: f32,
    pub strafing_increment: f32,
    pub orbit_insertion_slope: f32,
    pub random_offset_for_howitzer: f32,
    pub howitzer_weapon: Option<Arc<WeaponTemplate>>,
    pub gattling_template_name: Option<String>,
}

/// `MissileLauncherBuildingUpdateModuleData` (sim units).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MissileLauncherBuildingTemplate {
    pub special_power: Option<String>,
    pub door_open_frames: u32,
    pub door_wait_open_frames: u32,
    pub door_closing_frames: u32,
}

/// `NeutronBlastBehaviorModuleData` (sim units): the neutron shell's on-death blast.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NeutronBlastTemplate {
    pub blast_radius: f32,
    pub affect_airborne: bool,
    pub affect_allies: bool,
}

/// `ParticleUplinkCannonUpdate` firing state.
#[derive(Clone, Debug, Default)]
pub struct PucState {
    pub status: u8,
    pub laser_status: u8,
    pub start_attack_frame: u32,
    pub start_decay_frame: u32,
    pub initial_target: Coord3D,
    pub current_target: Coord3D,
    pub override_target: Coord3D,
    /// `m_manualTargetMode`: a human drives the beam; otherwise the automatic S-curve runs.
    pub manual: bool,
    pub scorch_marks_made: u32,
    pub next_scorch_frame: u32,
    pub damage_pulses_made: u32,
    pub next_damage_frame: u32,
    pub next_launch_fx_frame: u32,
    pub widening: bool,
    pub decaying: bool,
    pub widen_start: u32,
    pub widen_finish: u32,
    pub decay_start: u32,
    pub decay_finish: u32,
    pub width_scalar: f32,
}

/// `SpectreGunshipUpdate` runtime (on the gunship).
#[derive(Clone, Debug, Default)]
pub struct SpectreState {
    pub status: u8,
    pub initial_target: Coord3D,
    pub override_target: Coord3D,
    /// `m_satellitePosition`: the current move-to point of the gunship.
    pub satellite: Coord3D,
    pub orbit_escape_frame: u32,
    pub gattling: Option<ObjectId>,
    pub gattling_target: Coord3D,
    pub position_to_shoot: Coord3D,
    pub howitzer_counter: u32,
}

/// `MissileLauncherBuildingUpdate` door state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MissileLauncherState {
    pub door_state: u8,
    pub timeout_state: u8,
    pub timeout_frame: u32,
}

/// Per-object superweapon runtime, stored beside the other periodic behavior state.
#[derive(Clone, Debug, Default)]
pub struct SuperweaponState {
    /// `HeightDieUpdate`.
    pub height_die_started: bool,
    pub height_die_earliest: u32,
    pub height_die_last_z: f32,
    pub height_die_has_last: bool,
    pub height_die_dead: bool,
    pub height_die_particles: bool,
    /// `NeutronMissileUpdate`: target, launch/attack state and velocity.
    pub neutron_target: Option<Coord3D>,
    pub neutron_launched: bool,
    pub neutron_armed: bool,
    pub neutron_vel: Coord3D,
    pub neutron_frame: u32,
    /// `SpyVisionUpdate`: active / deactivate frame.
    pub spy_vision_active: bool,
    pub spy_vision_deactivate: u32,
    /// `SpyVisionUpdate::setDisabledUntilFrame` (internet-center sabotage).
    pub spy_vision_disabled_until: u32,
    /// `ParticleUplinkCannonUpdate`.
    pub puc: Option<Box<PucState>>,
    /// `SpectreGunshipUpdate` (on the gunship).
    pub spectre: Option<Box<SpectreState>>,
    /// `SpectreGunshipDeploymentUpdate`: the gunship spawned by this building.
    pub spectre_gunship_id: Option<ObjectId>,
    /// `MissileLauncherBuildingUpdate` door.
    pub missile_launcher: Option<Box<MissileLauncherState>>,
}

/// `ParticleUplinkCannonUpdate::PUCStatus`.
pub mod puc_status {
    pub const IDLE: u8 = 0;
    pub const CHARGING: u8 = 1;
    pub const PREPARING: u8 = 2;
    pub const ALMOST_READY: u8 = 3;
    pub const READY_TO_FIRE: u8 = 4;
    pub const PREFIRE: u8 = 5;
    pub const FIRING: u8 = 6;
    pub const POSTFIRE: u8 = 7;
    pub const PACKING: u8 = 8;
}

/// `ParticleUplinkCannonUpdate::LaserStatus`.
pub mod laser_status {
    pub const NONE: u8 = 0;
    pub const BORN: u8 = 1;
    pub const DECAYING: u8 = 2;
    pub const DEAD: u8 = 3;
}

/// `SpectreGunshipUpdate::GunshipStatus`.
pub mod gunship_status {
    pub const IDLE: u8 = 0;
    pub const INSERTING: u8 = 1;
    pub const ORBITING: u8 = 2;
    pub const DEPARTING: u8 = 3;
}

/// `MissileLauncherBuildingUpdate::DoorStateType`.
pub mod door_state {
    pub const CLOSED: u8 = 0;
    pub const OPENING: u8 = 1;
    pub const OPEN: u8 = 2;
    pub const WAITING_TO_CLOSE: u8 = 3;
    pub const CLOSING: u8 = 4;
}

impl SuperweaponState {
    /// `true` when no superweapon/height-die/neutron/spy state is set (keeps unrelated CRCs).
    pub(crate) fn is_default(&self) -> bool {
        !self.height_die_started
            && self.height_die_earliest == 0
            && !self.height_die_has_last
            && self.height_die_last_z == 0.0
            && !self.height_die_dead
            && !self.height_die_particles
            && self.neutron_target.is_none()
            && !self.neutron_launched
            && !self.neutron_armed
            && self.neutron_vel == Coord3D::ZERO
            && self.neutron_frame == 0
            && !self.spy_vision_active
            && self.spy_vision_deactivate == 0
            && self.spy_vision_disabled_until == 0
            && self.puc.is_none()
            && self.spectre.is_none()
            && self.spectre_gunship_id.is_none()
            && self.missile_launcher.is_none()
    }

    /// Hash the state deterministically.
    pub(crate) fn crc(&self, c: &mut crate::crc::Crc) {
        c.add_bool(self.height_die_started);
        c.add_u32(self.height_die_earliest);
        c.add_bool(self.height_die_has_last);
        c.add_f32(self.height_die_last_z);
        c.add_bool(self.height_die_dead);
        c.add_bool(self.height_die_particles);
        match self.neutron_target {
            Some(p) => {
                c.add_bool(true);
                c.add_coord(&p);
            }
            None => c.add_bool(false),
        }
        c.add_bool(self.neutron_launched);
        c.add_bool(self.neutron_armed);
        c.add_coord(&self.neutron_vel);
        c.add_u32(self.neutron_frame);
        c.add_bool(self.spy_vision_active);
        c.add_u32(self.spy_vision_deactivate);
        c.add_u32(self.spy_vision_disabled_until);
        match &self.puc {
            Some(p) => {
                c.add_bool(true);
                c.add_u32(p.status as u32);
                c.add_u32(p.laser_status as u32);
                c.add_u32(p.start_attack_frame);
                c.add_u32(p.start_decay_frame);
                c.add_coord(&p.current_target);
                c.add_bool(p.manual);
                c.add_u32(p.damage_pulses_made);
                c.add_u32(p.scorch_marks_made);
                c.add_f32(p.width_scalar);
            }
            None => c.add_bool(false),
        }
        match &self.spectre {
            Some(s) => {
                c.add_bool(true);
                c.add_u32(s.status as u32);
                c.add_coord(&s.initial_target);
                c.add_coord(&s.override_target);
                c.add_u32(s.orbit_escape_frame);
                c.add_u32(s.gattling.map_or(u32::MAX, |x| x.0));
                c.add_coord(&s.gattling_target);
                c.add_coord(&s.position_to_shoot);
                c.add_u32(s.howitzer_counter);
            }
            None => c.add_bool(false),
        }
        c.add_u32(self.spectre_gunship_id.map_or(u32::MAX, |x| x.0));
        match &self.missile_launcher {
            Some(m) => {
                c.add_bool(true);
                c.add_u32(m.door_state as u32);
                c.add_u32(m.timeout_state as u32);
                c.add_u32(m.timeout_frame);
            }
            None => c.add_bool(false),
        }
    }
}

impl GameLogic {
    // ---- HeightDieUpdate -------------------------------------------------------------------

    /// `HeightDieUpdate::update`: kill the object once it descends below the target height.
    pub(crate) fn update_height_die(&mut self, id: ObjectId, now: u32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.height_die.clone()) else { return };
        if self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return;
        }
        let (pos, contained, has_last, last_z, dead, particles) = {
            let o = self.objects.get(&id).unwrap();
            (
                o.position,
                o.contained_by.is_some(),
                o.superweapon.height_die_has_last,
                o.superweapon.height_die_last_z,
                o.superweapon.height_die_dead,
                o.superweapon.height_die_particles,
            )
        };
        // `m_earliestDeathFrame`.
        let started = self.objects.get(&id).is_some_and(|o| o.superweapon.height_die_started);
        if !started {
            if let Some(o) = self.objects.get_mut(&id) {
                o.superweapon.height_die_started = true;
                o.superweapon.height_die_earliest = now + t.initial_delay;
            }
            return;
        }
        let earliest = self.objects.get(&id).map_or(0, |o| o.superweapon.height_die_earliest);
        if earliest > now {
            return;
        }
        // Contained objects only track their position.
        if contained {
            if let Some(o) = self.objects.get_mut(&id) {
                o.superweapon.height_die_last_z = pos.z;
                o.superweapon.height_die_has_last = true;
            }
            return;
        }
        let terrain_height = self.terrain.ground_height(pos.x, pos.y);
        if !dead {
            let direction_ok = if t.only_when_moving_down && has_last { pos.z < last_z } else { true };
            let target_height = if t.target_height_includes_structures {
                self.tallest_structure_height(&pos).max(t.target_height) + terrain_height
            } else {
                terrain_height + t.target_height
            };
            if pos.z < target_height && direction_ok {
                if t.snap_to_ground_on_death || pos.z < terrain_height {
                    let mut p = pos;
                    p.z = terrain_height;
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.set_position(p);
                    }
                }
                // `Object::kill()`: unresistable lethal damage.
                self.kill_object(id);
                if let Some(o) = self.objects.get_mut(&id) {
                    o.superweapon.height_die_dead = true;
                }
            }
        }
        let _ = particles;
        if let Some(o) = self.objects.get_mut(&id) {
            o.superweapon.height_die_last_z = pos.z;
            o.superweapon.height_die_has_last = true;
        }
    }

    /// The tallest `KINDOF_STRUCTURE` height overlapping `pos` (`TargetHeightIncludesStructures`).
    fn tallest_structure_height(&self, pos: &Coord3D) -> f32 {
        let mut tallest = 0.0f32;
        for o in self.objects.values() {
            if !o.is_kind_of(crate::template::kind_of::STRUCTURE) {
                continue;
            }
            let r = o.template.geometry.bounding_circle_radius();
            if o.position.dist_2d(pos) <= r {
                tallest = tallest.max(o.template.geometry.height);
            }
        }
        tallest
    }

    // ---- NeutronMissileUpdate (object-carried path) -----------------------------------------

    /// `NeutronMissileUpdate::update` for an object that carries the module (a spawned warhead);
    /// inert unless a target was set. The projectile path uses the shared motion in `combat.rs`.
    pub(crate) fn update_neutron_missile(&mut self, id: ObjectId, now: u32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.neutron_missile.clone()) else { return };
        let (target, mut vel, launched, frame) = {
            let Some(o) = self.objects.get(&id) else { return };
            (o.superweapon.neutron_target, o.superweapon.neutron_vel, o.superweapon.neutron_launched, o.superweapon.neutron_frame)
        };
        let Some(target) = target else { return };
        if !launched {
            if frame != now {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.superweapon.neutron_frame = now;
                }
            }
            return;
        }
        let pos = self.objects.get(&id).map_or(Coord3D::ZERO, |o| o.position);
        let above = Coord3D::new(target.x, target.y, target.z + t.target_from_directly_above);
        let dist_above = pos.dist_2d(&above);
        let goal = if dist_above <= t.distance_to_travel_before_turning.max(1.0) { target } else { above };
        let speed = t.relative_speed.max(1.0);
        let dx = goal.x - pos.x;
        let dy = goal.y - pos.y;
        let dz = goal.z - pos.z;
        let len = (dx * dx + dy * dy + dz * dz).sqrt().max(1e-3);
        let (vx, vy, mut vz) = (dx / len * speed, dy / len * speed, dz / len * speed);
        let _ = &mut vel;
        if now.saturating_sub(frame) >= t.special_speed_time && pos.z > target.z + t.special_speed_height {
            vz *= t.special_accel_factor.max(1.0);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            let new_pos = Coord3D::new(pos.x + vx, pos.y + vy, pos.z + vz);
            o.set_position(new_pos);
            // The heading is the matrix whose X axis points along the velocity: `buildTransformMatrix` takes a unit
            // direction (NeutronMissileUpdate.cpp:325) and `setTransformMatrix` takes the result (:423). A stationary
            // missile has no direction, so its matrix stays as it was.
            let speed_len = (vx * vx + vy * vy + vz * vz).sqrt();
            if speed_len > 0.0 {
                let dir = Coord3D::new(vx / speed_len, vy / speed_len, vz / speed_len);
                o.set_transform_matrix(crate::transform::Matrix3D::build_transform(new_pos, dir));
            }
            o.superweapon.neutron_vel = Coord3D::new(vx, vy, vz);
        }
        let (armed, dist) = self.objects.get(&id).map_or((false, 0.0), |o| (o.superweapon.neutron_armed, o.position.dist_2d(&target)));
        if !armed && dist <= t.distance_to_travel_before_turning.max(1.0) {
            if let Some(fx) = t.ignition_fx.clone() {
                self.record_fx(id, fx);
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.superweapon.neutron_armed = true;
            }
        }
    }

    /// `NeutronBlastBehavior::onDie` / `neutronBlastToObject`: kill infantry (and everything they
    /// are carrying) and unman vehicles within `BlastRadius`. `AffectAirborne` chooses whether
    /// aircraft are caught. (The retail `KINDOF_CLIFF_JUMPER` "kill the whole bike" case is not
    /// modelled: the sim does not carry that kind-of bit, so combat bikes are unmanned like other
    /// vehicles.)
    pub(crate) fn apply_neutron_blast(&mut self, id: ObjectId, t: &NeutronBlastTemplate) {
        let (pos, src_player) = match self.objects.get(&id) {
            Some(o) => (o.position, o.player),
            None => return,
        };
        self.neutron_blast_at(pos, src_player, id, t);
    }

    /// The neutron blast at an explicit world point and source player (the projectile
    /// `NeutronBlastBehavior` path has no dying object of its own).
    pub(crate) fn neutron_blast_at(&mut self, pos: Coord3D, src_player: crate::object::PlayerIndex, exclude: ObjectId, t: &NeutronBlastTemplate) {
        if t.blast_radius <= 0.0 {
            return;
        }
        let victims = self.partition.objects_in_range(&pos, t.blast_radius, crate::partition::DistanceMode::FromCenter2D);
        for cid in victims {
            if cid == exclude {
                continue;
            }
            let (infantry, vehicle, drone, aircraft, player, has_contain) = match self.objects.get(&cid) {
                Some(o) if !o.destroyed => (
                    o.is_kind_of(crate::template::kind_of::INFANTRY),
                    o.is_kind_of(crate::template::kind_of::VEHICLE),
                    o.is_kind_of(crate::template::kind_of::DRONE),
                    o.is_kind_of(crate::template::kind_of::AIRCRAFT),
                    o.player,
                    !o.contained.is_empty() || self.is_tunnel_container(cid),
                ),
                _ => continue,
            };
            if !t.affect_airborne && aircraft {
                continue;
            }
            if !t.affect_allies && self.relationship(player, src_player) == crate::damage::Relationship::Allies {
                continue;
            }
            if infantry {
                self.kill_object(cid);
            }
            if has_contain {
                self.kill_all_contained(cid);
            }
            if vehicle && !drone {
                self.set_disabled(cid, crate::disabled::DisabledType::Unmanned);
                if let Some(o) = self.objects.get_mut(&cid) {
                    o.ai.stop();
                    o.combat.attack = None;
                }
                if let Some(n) = self.neutral_player() {
                    if let Some(o) = self.objects.get_mut(&cid) {
                        o.player = n;
                    }
                }
            }
        }
    }

    // ---- SpyVisionUpdate -------------------------------------------------------------------

    /// `SpyVisionUpdate::activateSpyVision`.
    pub fn activate_spy_vision(&mut self, id: ObjectId, duration: u32) {
        if self.objects.get(&id).is_none_or(|o| o.template.spy_vision.is_none()) {
            return;
        }
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&id) {
            o.superweapon.spy_vision_active = true;
            o.superweapon.spy_vision_deactivate = if duration == 0 { u32::MAX } else { now + duration };
        }
    }

    /// `SpyVisionUpdate::update`: turn the reveal off when the duration expires, on again when
    /// self-powered. The shroud reveal itself is applied by [`GameLogic::spy_vision_sources`] from
    /// `update_shroud`.
    pub(crate) fn update_spy_vision(&mut self, id: ObjectId, now: u32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.spy_vision.clone()) else { return };
        let (active, deactivate, disabled_until) = match self.objects.get(&id) {
            Some(o) => (o.superweapon.spy_vision_active, o.superweapon.spy_vision_deactivate, o.superweapon.spy_vision_disabled_until),
            None => return,
        };
        // `SpyVisionUpdate::setDisabledUntilFrame`: the sabotage shuts the vision down.
        if disabled_until > now {
            if active {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.superweapon.spy_vision_active = false;
                }
            }
            return;
        }
        if active && deactivate <= now {
            if let Some(o) = self.objects.get_mut(&id) {
                o.superweapon.spy_vision_active = false;
            }
        } else if !active && t.self_powered {
            // Self-powered (Strategy Center): turn on for the configured duration / interval.
            self.activate_spy_vision(id, t.self_powered_duration);
        }
    }

    /// The active spy-vision sources: `(owner, spy_on_kind_of)`. `update_shroud` ORs each owner's
    /// mask into a matching enemy unit's look (`Object::setVisionSpied` / `m_visionSpiedMask`).
    pub(crate) fn spy_vision_sources(&self) -> Vec<(crate::object::PlayerIndex, u64)> {
        self.objects
            .values()
            .filter(|o| !o.is_destroyed() && o.superweapon.spy_vision_active)
            .filter_map(|o| o.template.spy_vision.as_ref().map(|t| (o.player, t.spy_on_kind_of)))
            .collect()
    }

    /// Whether the object's `SpyVisionUpdate` is currently revealing (`m_currentlyActive`).
    pub fn spy_vision_active(&self, id: ObjectId) -> bool {
        self.find_object(id).is_some_and(|o| o.superweapon.spy_vision_active)
    }

    // ---- ParticleUplinkCannonUpdate --------------------------------------------------------

    /// `ParticleUplinkCannonUpdate::initiateIntentToDoSpecialPower`: begin the firing timeline. The
    /// sim runs the automatic "S" swath (the original's computer-controlled path); a human's manual
    /// drive is available through [`GameLogic::drive_particle_uplink`].
    pub(crate) fn start_particle_uplink(&mut self, id: ObjectId, target: Coord3D) -> bool {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.particle_uplink.clone()) else { return false };
        if self.objects.get(&id).is_none_or(|o| o.superweapon.puc.as_ref().is_some_and(|p| p.start_attack_frame != 0)) {
            return false;
        }
        let now = self.frame.max(1);
        if let Some(o) = self.objects.get_mut(&id) {
            o.superweapon.puc = Some(Box::new(PucState {
                status: puc_status::READY_TO_FIRE,
                laser_status: laser_status::NONE,
                start_attack_frame: now,
                start_decay_frame: now + t.total_firing_frames,
                initial_target: target,
                current_target: target,
                override_target: target,
                manual: false,
                width_scalar: 0.0,
                ..PucState::default()
            }));
        }
        true
    }

    /// `ParticleUplinkCannonUpdate::setSpecialPowerOverridableDestination`: a manual drive click.
    pub fn drive_particle_uplink(&mut self, id: ObjectId, target: Coord3D) {
        if let Some(p) = self.objects.get_mut(&id).and_then(|o| o.superweapon.puc.as_deref_mut()) {
            p.override_target = target;
            p.manual = true;
        }
    }

    /// `ParticleUplinkCannonUpdate::update`: the charge status while idle, then the firing timeline
    /// (laser birth/decay, the moving swath, scorch marks, damage pulses and launch FX).
    pub(crate) fn update_particle_uplink(&mut self, id: ObjectId, now: u32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.particle_uplink.clone()) else { return };
        let Some(mut st) = self.objects.get(&id).and_then(|o| o.superweapon.puc.clone()) else { return };
        let (me_pos, me_player) = match self.objects.get(&id) {
            Some(o) => (o.position, o.player),
            None => return,
        };
        if st.start_attack_frame == 0 {
            return;
        }
        let birth = st.start_attack_frame + t.beam_travel_frames;
        let decay_start = st.start_decay_frame + t.beam_travel_frames;
        let death = decay_start + t.width_grow_frames;

        // `LASERSTATUS_NONE`: become eligible to fire.
        if st.laser_status == laser_status::NONE && birth <= now {
            st.laser_status = laser_status::BORN;
            st.scorch_marks_made = 0;
            st.next_scorch_frame = now;
            st.damage_pulses_made = 0;
            st.next_damage_frame = now;
            st.widen_start = now;
            st.widen_finish = now + t.width_grow_frames;
            st.widening = t.width_grow_frames > 0;
            st.width_scalar = 0.0;
        }
        // `LASERSTATUS_BORN`: start the width decay at `orbitalDecayStart`.
        if st.laser_status == laser_status::BORN && decay_start <= now {
            st.laser_status = laser_status::DECAYING;
            st.decay_start = now;
            st.decay_finish = now + t.width_grow_frames;
            st.decaying = t.width_grow_frames > 0;
            st.widening = false;
            st.width_scalar = 1.0;
        }
        // `LASERSTATUS_DECAYING`: the beam is gone at `orbitalDeathFrame`.
        if st.laser_status == laser_status::DECAYING && death <= now {
            if let Some(o) = self.objects.get_mut(&id) {
                o.superweapon.puc = None;
            }
            return;
        }

        let firing = birth <= now && now < death;
        if firing {
            let cur = if !st.manual {
                // `ParticleUplinkCannonUpdate::update`'s "S curve" swath centred on the target.
                let factor = (now - birth) as f32 / (death - birth).max(1) as f32;
                let radians = factor * TWO_PI - PI;
                let cx_distance = factor * t.swath_of_death_distance - t.swath_of_death_distance * 0.5;
                let cx_height = sin(radians) * t.swath_of_death_amplitude;
                let b2i = Coord3D::new(st.initial_target.x - me_pos.x, st.initial_target.y - me_pos.y, st.initial_target.z - me_pos.z);
                let target_distance = sqrt(b2i.x * b2i.x + b2i.y * b2i.y + b2i.z * b2i.z);
                let mut cart = (cx_distance + target_distance, cx_height);
                let cart_len = sqrt(cart.0 * cart.0 + cart.1 * cart.1);
                if cart_len > 0.0 {
                    cart.0 /= cart_len;
                    cart.1 /= cart_len;
                }
                let mut b2t = (st.initial_target.x - me_pos.x, st.initial_target.y - me_pos.y);
                let bl = sqrt(b2t.0 * b2t.0 + b2t.1 * b2t.1);
                if bl > 0.0 {
                    b2t.0 /= bl;
                    b2t.1 /= bl;
                }
                let mut dot = b2t.0 * cart.0 + b2t.1 * cart.1;
                dot = dot.clamp(-0.99999, 0.99999);
                let mut angle = acos(dot);
                if b2t.1 >= 0.0 {
                    angle = -angle;
                }
                // `Matrix3D::Scale(targetDistance); Rotate_Z(-angle); Get_X_Vector`.
                let td = sqrt(cart.0 * cart.0 + cart.1 * cart.1) * target_distance;
                Coord3D::new(me_pos.x + cos(-angle) * td, me_pos.y + sin(-angle) * td, 0.0)
            } else {
                // Manual drive: move toward the override destination at `ManualDrivingSpeed`.
                let speed = t.manual_driving_speed / LOGICFRAMES_PER_SECOND as f32;
                let v = Coord3D::new(st.override_target.x - st.current_target.x, st.override_target.y - st.current_target.y, 0.0);
                let d = sqrt(v.x * v.x + v.y * v.y);
                if d > 0.0 {
                    let step = speed.min(d);
                    st.current_target.x += v.x / d * step;
                    st.current_target.y += v.y / d * step;
                }
                st.current_target
            };
            let cur = Coord3D::new(cur.x, cur.y, self.terrain.ground_height(cur.x, cur.y));
            st.current_target = cur;

            // `LaserRadiusUpdate`: widen over `WidthGrowTime`, then decay over `WidthGrowTime`.
            if st.widening {
                if now >= st.widen_finish {
                    st.width_scalar = 1.0;
                    st.widening = false;
                } else {
                    st.width_scalar = (now - st.widen_start) as f32 / (st.widen_finish - st.widen_start).max(1) as f32;
                }
            } else if st.decaying {
                st.width_scalar = 1.0 - (now - st.decay_start) as f32 / (st.decay_finish - st.decay_start).max(1) as f32;
                if st.width_scalar < 0.0 {
                    st.width_scalar = 0.0;
                }
            }
            let logical_radius = t.beam_radius * st.width_scalar;
            let damage_radius = logical_radius * t.damage_radius_scalar;

            // Scorch marks (`m_nextScorchMarkFrame`) + the owner's shroud reveal.
            if st.next_scorch_frame <= now {
                st.scorch_marks_made += 1;
                if let Some(fx) = &t.ground_hit_fx {
                    self.record_fx_at(id, fx.clone(), cur);
                }
                // `doShroudReveal` then `undoShroudReveal`: mark the cells as seen for the owner.
                let mask = 1u32 << me_player;
                self.shroud.add_looker(&cur, t.reveal_range, mask);
                self.shroud.remove_looker(&cur, t.reveal_range, mask);
                if t.total_scorch_marks > 0 {
                    let nf = st.scorch_marks_made as f32 / t.total_scorch_marks as f32;
                    st.next_scorch_frame = birth + (nf * (death - birth) as f32) as u32;
                }
            }

            // Damage pulses: `damagePerPulse = (TotalFiringTime seconds * DamagePerSecond) /
            // TotalDamagePulses`, applied to every object within `damageRadius` (allies included,
            // exactly as `attemptDamage`).
            if st.next_damage_frame <= now {
                st.damage_pulses_made += 1;
                let total_seconds = t.total_firing_frames as f32 / LOGICFRAMES_PER_SECOND as f32;
                let per_pulse = total_seconds * t.damage_per_second / t.total_damage_pulses.max(1) as f32;
                let victims = self.partition.objects_in_range(&cur, damage_radius, crate::partition::DistanceMode::FromCenter2D);
                for cid in victims {
                    if cid == id {
                        continue;
                    }
                    self.damage_object_with_death(
                        cid,
                        crate::body::DamageInfo { source: id, amount: per_pulse, kind: t.damage_type },
                        t.death_type,
                    );
                }
                if let Some(name) = &t.remnant_object {
                    if let Some(rt) = self.find_template(name) {
                        self.create_object(&rt, cur, 0.0, me_player);
                    }
                }
                if t.total_damage_pulses > 0 {
                    let nf = st.damage_pulses_made as f32 / t.total_damage_pulses as f32;
                    st.next_damage_frame = birth + (nf * (death - birth) as f32) as u32;
                }
            }
        }

        // Status and the periodic beam-launch FX.
        let end_decay = st.start_decay_frame + t.width_grow_frames;
        st.status = if end_decay <= now {
            puc_status::PACKING
        } else if st.start_decay_frame <= now {
            puc_status::POSTFIRE
        } else {
            puc_status::FIRING
        };
        if st.status == puc_status::FIRING && st.next_launch_fx_frame <= now {
            if let Some(fx) = &t.beam_launch_fx {
                self.record_fx_at(id, fx.clone(), me_pos);
            }
            st.next_launch_fx_frame = now + t.delay_between_launch_fx;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.superweapon.puc = Some(st);
        }
    }

    // ---- SpectreGunshipDeploymentUpdate / SpectreGunshipUpdate -----------------------------

    /// `TerrainLogic::findFarthestEdgePoint`: the map corner opposite `farthest_from`.
    pub(crate) fn find_farthest_edge(&self, farthest_from: &Coord3D) -> Coord3D {
        let (lo, hi) = self.terrain.world_extent();
        let x = if farthest_from.x < (hi.x - lo.x) * 0.5 { hi.x } else { lo.x };
        let y = if farthest_from.y < (hi.y - lo.y) * 0.5 { hi.y } else { lo.y };
        Coord3D::new(x, y, self.terrain.ground_height(x, y))
    }

    /// `TerrainLogic::findClosestEdgePoint`: the nearest of the four map edges.
    pub(crate) fn find_closest_edge(&self, p: &Coord3D) -> Coord3D {
        let (lo, hi) = self.terrain.world_extent();
        let d = [(p.y - lo.y).abs(), (p.x - hi.x).abs(), (p.y - hi.y).abs(), (p.x - lo.x).abs()];
        let mut best = 0;
        for i in 1..4 {
            if d[i] < d[best] {
                best = i;
            }
        }
        let (x, y) = match best {
            0 => (p.x, lo.y),
            1 => (hi.x, p.y),
            2 => (p.x, hi.y),
            _ => (lo.x, p.y),
        };
        Coord3D::new(x, y, self.terrain.ground_height(x, y))
    }

    /// `SpectreGunshipDeploymentUpdate::initiateIntentToDoSpecialPower`: spawn the gunship at the
    /// map edge and trigger its own `SpectreGunshipUpdate` special power at the target.
    pub(crate) fn start_spectre_deployment(&mut self, id: ObjectId, target: Coord3D) -> bool {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.spectre_deployment.clone()) else { return false };
        let Some(gunship) = t.gunship.clone() else { return false };
        let player = match self.objects.get(&id) {
            Some(o) => o.player,
            None => return false,
        };
        let creation = match t.create_location {
            0 => self.find_closest_edge(&self.objects[&id].position),
            1 => self.find_farthest_edge(&self.objects[&id].position),
            2 => self.find_closest_edge(&target),
            _ => self.find_farthest_edge(&target),
        };
        // Push the creation point `distanceToTarget + GunshipOrbitRadius` directly away from the
        // target, so the transit and hover are off-map.
        let mut delta = Coord3D::new(target.x - creation.x, target.y - creation.y, 0.0);
        let dist = sqrt(delta.x * delta.x + delta.y * delta.y);
        if dist > 0.0 {
            delta.x /= dist;
            delta.y /= dist;
        }
        let spawn = Coord3D::new(target.x - delta.x * (dist + t.gunship_orbit_radius), target.y - delta.y * (dist + t.gunship_orbit_radius), 0.0);
        let spawn = Coord3D::new(spawn.x, spawn.y, self.terrain.ground_height(spawn.x, spawn.y) + 200.0);
        let gid = self.create_object(&gunship, spawn, atan2(target.y - spawn.y, target.x - spawn.x), player);
        self.set_ocl_producer(gid, id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.superweapon.spectre_gunship_id = Some(gid);
        }
        self.start_spectre_gunship(gid, target)
    }

    /// `SpectreGunshipUpdate::initiateIntentToDoSpecialPower`: begin inserting toward the orbit.
    fn start_spectre_gunship(&mut self, id: ObjectId, target: Coord3D) -> bool {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.spectre_gunship.clone()) else { return false };
        let player = match self.objects.get(&id) {
            Some(o) => o.player,
            None => return false,
        };
        // Create the gattling gunner and put it in the gunship's `HelixContain`.
        let gattling = t.gattling_template_name.clone().and_then(|n| self.find_template(&n)).map(|gt| self.create_object(&gt, target, 0.0, player));
        if let Some(g) = gattling {
            self.set_ocl_producer(g, id);
            self.force_enter_contain(id, g);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.superweapon.spectre = Some(Box::new(SpectreState {
                status: gunship_status::INSERTING,
                initial_target: target,
                override_target: target,
                gattling,
                gattling_target: target,
                position_to_shoot: target,
                ..SpectreState::default()
            }));
        }
        true
    }

    /// `SpectreGunshipUpdate::update`: fly to the orbit point (insertion), sweep the target area
    /// with the gattling and howitzer for `OrbitTime`, then leave the map.
    pub(crate) fn update_spectre(&mut self, id: ObjectId, now: u32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.spectre_gunship.clone()) else { return };
        let Some(mut s) = self.objects.get(&id).and_then(|o| o.superweapon.spectre.clone()) else { return };
        let gunship_pos = match self.objects.get(&id) {
            Some(o) => o.position,
            None => return,
        };

        if s.status == gunship_status::INSERTING || s.status == gunship_status::ORBITING {
            // `perigee` (gun -> target, flattened/normalised) and `apogee` (perpendicular).
            let mut perigee = Coord3D::new(gunship_pos.x - s.initial_target.x, gunship_pos.y - s.initial_target.y, 0.0);
            let distance_to_target = sqrt(perigee.x * perigee.x + perigee.y * perigee.y);
            if distance_to_target > 0.0 {
                perigee.x /= distance_to_target;
                perigee.y /= distance_to_target;
            }
            let apogee = (-perigee.y, perigee.x);
            let n1 = t.orbit_insertion_slope.clamp(0.5, 0.8);
            let n2 = 1.0 - n1;
            let dx = (perigee.x * n1 + apogee.0 * n2) * t.orbit_radius;
            let dy = (perigee.y * n1 + apogee.1 * n2) * t.orbit_radius;
            s.satellite = Coord3D::new(s.initial_target.x + dx, s.initial_target.y + dy, gunship_pos.z);
            // `shipAI->aiMoveToPosition(&m_satellitePosition, CMD_FROM_AI)` (SpectreGunshipUpdate.cpp:438).
            self.ai_move_to_position_cmd(id, s.satellite, crate::ai_states::CommandSource::Ai);
            // Constrain the manual reticle to `AttackAreaRadius - TargetingReticleRadius`.
            let constraint = (t.attack_area_radius - t.targeting_reticle_radius).max(0.0);
            let mut d = Coord3D::new(s.initial_target.x - s.override_target.x, s.initial_target.y - s.override_target.y, 0.0);
            let dl = sqrt(d.x * d.x + d.y * d.y);
            if dl > constraint {
                d.x = d.x / dl * constraint;
                d.y = d.y / dl * constraint;
                s.override_target = Coord3D::new(s.initial_target.x - d.x, s.initial_target.y - d.y, s.override_target.z);
            }
            if s.status == gunship_status::INSERTING && distance_to_target <= t.orbit_radius * 1.1 {
                s.status = gunship_status::ORBITING;
                s.orbit_escape_frame = now + t.orbit_time;
            }
        } else if s.status == gunship_status::DEPARTING {
            // Off-map: destroy the gunship (`isPointOffMap`).
            let (lo, hi) = self.terrain.world_extent();
            if gunship_pos.x < lo.x || gunship_pos.y < lo.y || gunship_pos.x > hi.x || gunship_pos.y > hi.y {
                if let Some(g) = s.gattling {
                    self.destroy_object(g);
                }
                self.destroy_object(id);
                if let Some(o) = self.objects.get_mut(&id) {
                    o.superweapon.spectre = None;
                }
                return;
            }
        }

        if s.status == gunship_status::ORBITING {
            if now >= s.orbit_escape_frame {
                if let Some(g) = s.gattling {
                    self.destroy_object(g);
                }
                s.gattling = None;
                s.status = gunship_status::DEPARTING;
                // `disengageAndDepartAO`: fly off the map in the facing direction.
                let edge = Coord3D::new(gunship_pos.x + cos(self.objects[&id].angle) * 99999.0, gunship_pos.y + sin(self.objects[&id].angle) * 99999.0, gunship_pos.z);
                // `shipAI->aiMoveToPosition(&exitPoint, CMD_FROM_AI)` (SpectreGunshipUpdate.cpp:803).
                self.ai_move_to_position_cmd(id, edge, crate::ai_states::CommandSource::Ai);
            } else if now % t.howitzer_firing_rate.max(1) == 0 {
                // Re-evaluate the target every `HowitzerFiringRate` frames.
                s.position_to_shoot = s.override_target;
                let mut valid = self.spectre_find_target(&s, t.targeting_reticle_radius, &s.override_target, id);
                if valid.is_none() {
                    // Non-human players fall back to the whole `AttackAreaRadius` circle.
                    if let Some(v) = self.spectre_find_target(&s, t.attack_area_radius, &s.initial_target, id) {
                        valid = Some(v);
                        s.position_to_shoot = self.objects[&v].position;
                    }
                }
                // The gattling keeps a barrage on `positionToShootAt`.
                let gattling_target = valid.map(|v| self.objects[&v].position).unwrap_or(s.gattling_target);
                if let Some(g) = s.gattling {
                    if self.objects.contains_key(&g) {
                        self.ocl_attack_position(g, gattling_target, 9999, 0);
                    }
                }
                // Follow with a howitzer volley once the gattling has held the target.
                if s.howitzer_counter > t.howitzer_follow_lag {
                    if let Some(weapon) = t.howitzer_weapon.clone() {
                        // `SpectreGunshipUpdate.cpp:599-600`: `GameLogicRandomValue( -offs, offs )` takes `int`
                        // arguments (the Real offset truncates) and draws both axes unconditionally.
                        let offs = t.random_offset_for_howitzer as i32;
                        let ox = self.rng.value(-offs, offs) as f32;
                        let oy = self.rng.value(-offs, offs) as f32;
                        let shot = Coord3D::new(s.gattling_target.x + ox, s.gattling_target.y + oy, s.gattling_target.z);
                        self.fire_temp_weapon_at(id, weapon, shot);
                    }
                }
            }
            // Gattling targeting: wind the aim toward `positionToShootAt` while the gattling is
            // firing (`OBJECT_STATUS_IS_FIRING_WEAPON`; the sim notes the last fire frame).
            let gattling_firing = s.gattling.is_some_and(|g| {
                self.objects.get(&g).is_some_and(|o| {
                    (0..crate::weapon::WEAPON_SLOT_COUNT).any(|i| {
                        o.combat.weapons.weapon(i).is_some_and(|w| w.last_fire_frame() != 0 && now.saturating_sub(w.last_fire_frame()) <= 1)
                    })
                })
            });
            if gattling_firing {
                let mut delta = Coord3D::new(s.position_to_shoot.x - s.gattling_target.x, s.position_to_shoot.y - s.gattling_target.y, s.position_to_shoot.z - s.gattling_target.z);
                let dist = sqrt(delta.x * delta.x + delta.y * delta.y + delta.z * delta.z);
                if dist < t.strafing_increment {
                    s.gattling_target = s.position_to_shoot;
                    s.howitzer_counter += 1;
                } else {
                    s.howitzer_counter = 0;
                    delta.x = delta.x / dist * t.strafing_increment;
                    delta.y = delta.y / dist * t.strafing_increment;
                    delta.z = delta.z / dist * t.strafing_increment;
                    s.gattling_target = Coord3D::new(s.gattling_target.x + delta.x, s.gattling_target.y + delta.y, s.gattling_target.z + delta.z);
                }
            }
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.superweapon.spectre = Some(s);
        }
        let _ = t.special_power;
    }

    /// The closest live, in-map enemy matching the Spectre filters, within `radius` of `center`,
    /// and at a fair distance from the ship (`isFairDistanceFromShip`).
    fn spectre_find_target(&self, s: &SpectreState, radius: f32, center: &Coord3D, id: ObjectId) -> Option<ObjectId> {
        let ship = self.objects.get(&id)?;
        let t = ship.template.spectre_gunship.as_ref()?;
        let mut best: Option<(ObjectId, f32)> = None;
        for o in self.objects.values() {
            if o.id == id || o.destroyed || o.is_dying() || o.contained_by.is_some() {
                continue;
            }
            if self.relationship(o.player, ship.player) != crate::damage::Relationship::Enemies {
                continue;
            }
            if o.position.dist_2d(center) > radius {
                continue;
            }
            let ship_delta = Coord3D::new(ship.position.x - o.position.x, ship.position.y - o.position.y, 0.0);
            let ship_dist = sqrt(ship_delta.x * ship_delta.x + ship_delta.y * ship_delta.y);
            if ship_dist <= t.orbit_radius * 0.75 {
                continue;
            }
            let d = o.position.dist_2d(center);
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((o.id, d));
            }
        }
        let _ = s;
        best.map(|(id, _)| id)
    }

    // ---- MissileLauncherBuildingUpdate -----------------------------------------------------

    /// `MissileLauncherBuildingUpdate::initiateIntentToDoSpecialPower`: the missile launcher closes
    /// its door after firing its own power.
    pub(crate) fn notify_missile_launcher_fired(&mut self, id: ObjectId, power: &str, now: u32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.missile_launcher.clone()) else { return };
        if t.special_power.as_deref() != Some(power) {
            return;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            let state = o.superweapon.missile_launcher.get_or_insert_with(|| Box::new(MissileLauncherState::default()));
            state.door_state = door_state::WAITING_TO_CLOSE;
            state.timeout_state = door_state::CLOSING;
            state.timeout_frame = now + t.door_wait_open_frames;
        }
    }

    /// `MissileLauncherBuildingUpdate::update`: open the door `DoorOpenTime` before the power is
    /// ready, pop it open when ready, and close it after firing. Pure model-condition state; the
    /// special power itself has its own reload timer.
    pub(crate) fn update_missile_launcher(&mut self, id: ObjectId, now: u32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.missile_launcher.clone()) else { return };
        if self.objects.get(&id).is_none_or(|o| o.under_construction) {
            return;
        }
        let ready = t.special_power.as_deref().map(|p| self.special_power_ready_at(id, p)).unwrap_or(u32::MAX);
        let mut state = self.objects.get(&id).and_then(|o| o.superweapon.missile_launcher.clone()).unwrap_or_default();
        // `m_timeoutFrame` elapsed -> enter the queued state.
        if state.timeout_frame != 0 && now > state.timeout_frame {
            match state.timeout_state {
                door_state::OPEN => {
                    state.door_state = door_state::OPEN;
                    state.timeout_frame = 0;
                }
                door_state::CLOSING => {
                    state.door_state = door_state::CLOSING;
                    // `m_timeoutFrame = now + DoorCloseTime`, capped by half the time left to ready.
                    let delta = ready.saturating_sub(now);
                    state.timeout_frame = now + t.door_closing_frames.min(delta / 2);
                    state.timeout_state = door_state::CLOSED;
                }
                door_state::CLOSED => {
                    state.door_state = door_state::CLOSED;
                    state.timeout_frame = 0;
                }
                _ => {}
            }
        }
        if state.door_state != door_state::OPEN && ready <= now {
            // `m_specialPowerModule->isReady()`: pop the door open.
            state.door_state = door_state::OPEN;
            state.timeout_state = door_state::OPEN;
            state.timeout_frame = 0;
        } else if state.door_state == door_state::CLOSED && now >= ready.saturating_sub(t.door_open_frames) {
            state.door_state = door_state::OPENING;
            state.timeout_state = door_state::OPEN;
            state.timeout_frame = ready.saturating_sub(1);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.superweapon.missile_launcher = Some(state);
        }
    }
}
