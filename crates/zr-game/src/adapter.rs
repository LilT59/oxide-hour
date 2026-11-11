//! `ThingTemplate` -> [`zr_sim::ObjectTemplate`] adapter (the seam described in `zr_sim::template`).
//!
//! `zr-templates` already holds engine units (per-frame velocities, radians per frame, frames),
//! so nothing is converted here: values are copied across, and the locomotor goes through
//! [`LocomotorParams::from_engine_units`], never `from_ini`.
//!
//! Decisions:
//! * `KindOf` bit layouts differ between the crates, so bits are mapped **by name**
//!   ([`kind_of_bits`]).
//! * Locomotor: the first resolvable locomotor of `SET_NORMAL` (the set the unit spawns with).
//!   A unit with several (e.g. amphibious) uses the first, like the original's first-in-set
//!   default until the pathfinder picks by surface.
//! * Body: `ActiveBody` family `MaxHealth`/`InitialHealth`; no body or `InactiveBody` gives
//!   `max_health = 0` (the sim's "no body").
//! * Geometry/vision copied as is.
//! * Combat: the **normal** (no-upgrade) `WeaponSet` and `ArmorSet` (empty condition mask);
//!   damage/death types map by table index (checked by a test); the first turret's
//!   `TurretTurnRate`; `AutoAcquireEnemiesWhenIdle` `YES` / `ATTACK_BUILDINGS`.
//!   Weapons and armors are cached per id so units share one `Arc`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use zr_sim::armor::ArmorTemplate as SimArmor;
use zr_sim::chinook::ChinookTemplate;
use zr_sim::contain::{ContainKind as SimContainKind, ContainTemplate, GarrisonFirePoints};
use zr_sim::damage::{DamageType as SimDamageType, DeathType as SimDeathType};
use zr_sim::die::{DieMux, DieModuleTemplate, DieTemplate, SlowDeathTemplate};
use zr_sim::ocl::{ApplyRandomForceTemplate, AttackTemplate, CreateObjectTemplate, DeliverPayloadTemplate, OclNugget, OclTemplate};
use zr_sim::physics::PhysicsTemplate;
use zr_sim::production::{Prerequisite, ProductionExit, ProductionTemplate, QuantityModifier, SpawnPointBone};
use zr_sim::projectile::{DumbProjectileParams, MissileParams, ProjectileBehavior, ProjectileTemplate, PHYSICS_DEFAULT_FRICTION};
use zr_sim::rider::{RiderChangeTemplate, RiderSlot};
use zr_sim::supply::{DockKind, DockTemplate, SupplyTruckTemplate};
use zr_sim::template::{kind_of, Geometry, GeometryShape, LocomotorKind, LocomotorParams, LocomotorSet, SleepyModuleKind, ZBehavior};
use zr_sim::weapon::{affects, anti, CombatTemplate, LaunchBones, Prefire, WeaponSetTemplate, WeaponTemplate as SimWeapon};
use zr_sim::upgrade::{SetFlagBits, SimUpgradeModule, UpgradeEffect};
use zr_sim::special_power::{SimSpecialPower, SimSpecialPowerTemplate, SpecialAbilityTemplate, SpecialPowerEffect};
use zr_sim::{ObjectTemplate, TemplateSource};
use zr_templates::{
    ArmorId, ArmorSetFlags, ArmorSetType, AutoAcquire, BodyClass, ContainKind as TemplateContainKind, DebrisDisposition, DeathType,
    DieData, DieKind, DockKind as TemplateDockKind, DumbProjectileData, GenericNugget, GeometryType, KindOf, KindOfMask,
    LocomotorAppearance, LogicModule, LocomotorBehaviorZ, ModuleKind, LocomotorSetType, LocomotorSurface, LocomotorTemplate, Mask, MaxHealthChangeType as TemplateMaxHealth,
    MissileAiData, ModuleData, NameEnum, Nugget, ObjectCreationList, ObjectStatus, OclId, Prerequisite as TemplatePrerequisite,
    ProductionExitData, ProductionExitKind, RawItem, SlowDeathData, SlowDeathPhase, Templates, ThingTemplate, UpgradeKind,
    WeaponAffects, WeaponAnti, WeaponId, WeaponPrefire, WeaponReload, WeaponSetFlags,
    WeaponSetType, WeaponTemplate,
};

/// Map the template layer's `KindOf` set onto the sim's bit constants, by name.
pub fn kind_of_bits(t: &ThingTemplate) -> u64 {
    kind_of_mask_bits(&t.kind_of)
}

/// The sim's `kind_of` subset for a full `KindOf` mask (`kind_of_bits`' mapping). The sim mirrors
/// the engine's `s_bitNameList` order, so each kind maps by name (and, equivalently, by index).
pub fn kind_of_mask_bits(m: &KindOfMask) -> u64 {
    m.iter().filter_map(|k| kind_of::bit_for_name(k.name())).fold(0, |bits, b| bits | b)
}

/// Is a `KindOf` name present? The sim's 64-bit kind set is full, so the few extra kinds the
/// pathfinder tests (`DEFENSIVE_WALL`, `ATTACK_NEEDS_LINE_OF_SIGHT`) map to dedicated template
/// flags instead of bits.
pub fn kind_of_has(m: &KindOfMask, name: &str) -> bool {
    m.iter().any(|k| k.name().eq_ignore_ascii_case(name))
}

/// The secondary [`zr_sim::template::collide_kind`] bits for a full `KindOf` mask: the kinds the
/// `Collide` modules test that do not fit in the full 64-bit `kind_of` set.
pub fn collide_kind_mask_bits(m: &KindOfMask) -> u32 {
    m.iter().filter_map(|k| zr_sim::template::collide_kind::bit_for_name(k.name())).fold(0, |bits, b| bits | b)
}

/// `LocomotorTemplate` (engine units) -> sim parameters, no unit conversion.
fn loco_kind(appearance: LocomotorAppearance) -> LocomotorKind {
    match appearance {
        LocomotorAppearance::TwoLegs => LocomotorKind::Legs,
        LocomotorAppearance::Climber => LocomotorKind::Climber,
        LocomotorAppearance::FourWheels | LocomotorAppearance::Motorcycle => LocomotorKind::Wheels,
        LocomotorAppearance::Treads => LocomotorKind::Treads,
        LocomotorAppearance::Hover => LocomotorKind::Hover,
        LocomotorAppearance::Thrust => LocomotorKind::Thrust,
        LocomotorAppearance::Wings => LocomotorKind::Wings,
        // `OTHER` (missiles, boats, ...): no sim flavour yet; treat as legs and follow the INI flag.
        LocomotorAppearance::Other => LocomotorKind::Legs,
    }
}

fn z_behavior(z: LocomotorBehaviorZ) -> ZBehavior {
    match z {
        LocomotorBehaviorZ::NoZMotiveForce => ZBehavior::NoZMotiveForce,
        LocomotorBehaviorZ::SeaLevel => ZBehavior::AbsoluteHeight,
        LocomotorBehaviorZ::SurfaceRelativeHeight => ZBehavior::SurfaceRelativeHeight,
        LocomotorBehaviorZ::AbsoluteHeight => ZBehavior::AbsoluteHeight,
        LocomotorBehaviorZ::FixedSurfaceRelativeHeight => ZBehavior::FixedSurfaceRelativeHeight,
        LocomotorBehaviorZ::FixedAbsoluteHeight => ZBehavior::FixedAbsoluteHeight,
        LocomotorBehaviorZ::FixedRelativeToGroundAndBuildings => ZBehavior::FixedRelativeToGroundAndBuildings,
        LocomotorBehaviorZ::RelativeToHighestLayer => ZBehavior::RelativeToHighestLayer,
    }
}

fn sim_locomotor_set(s: LocomotorSetType) -> LocomotorSet {
    match s {
        LocomotorSetType::SetNormal => LocomotorSet::Normal,
        LocomotorSetType::SetNormalUpgraded => LocomotorSet::NormalUpgraded,
        LocomotorSetType::SetFreefall => LocomotorSet::Freefall,
        LocomotorSetType::SetWander => LocomotorSet::Wander,
        LocomotorSetType::SetPanic => LocomotorSet::Panic,
        LocomotorSetType::SetTaxiing => LocomotorSet::Taxiing,
        LocomotorSetType::SetSupersonic => LocomotorSet::Supersonic,
        LocomotorSetType::SetSluggish => LocomotorSet::Sluggish,
    }
}

/// `LocomotorTemplate` (already per-frame) -> [`LocomotorParams`].
pub fn convert_locomotor(l: &LocomotorTemplate) -> LocomotorParams {
    let airborne = l.surfaces.contains(LocomotorSurface::Air);
    LocomotorParams {
        kind: loco_kind(l.appearance),
        max_speed: l.max_speed,
        max_speed_damaged: l.max_speed_damaged,
        min_speed: l.min_speed,
        max_turn_rate: l.max_turn_rate,
        max_turn_rate_damaged: l.max_turn_rate_damaged,
        min_turn_speed: l.min_turn_speed,
        turn_pivot_offset: l.turn_pivot_offset,
        can_move_backward: l.can_move_backward,
        extra_2d_friction: l.extra_2d_friction,
        acceleration: l.acceleration,
        acceleration_damaged: l.acceleration_damaged,
        braking: l.braking,
        lift: l.lift,
        lift_damaged: l.lift_damaged,
        airborne_targeting_height: l.airborne_targeting_height as f32,
        preferred_height: l.preferred_height,
        preferred_height_damping: l.preferred_height_damping,
        max_thrust_angle: l.max_thrust_angle,
        circling_radius: l.circling_radius,
        behavior_z: z_behavior(l.behavior_z),
        close_enough_dist: l.close_enough_dist,
        close_enough_dist_3d: l.is_close_enough_dist_3d,
        // `ULTRA_ACCURATE` / `PRECISE_Z_POS` are runtime flags (`JetAIUpdate`); only the template's
        // slide-into-place factor is data.
        ultra_accurate: false,
        slide_into_place_factor: l.ultra_accurate_slide_into_place_factor,
        precise_z: false,
        // `Locomotor::setPhysicsOptions` (`Locomotor.cpp:946`) pushes `getStickToGround()`, which is the INI value
        // only (`Locomotor.h:286`). No appearance forces the glue.
        stick_to_ground: l.stick_to_ground,
        surfaces_ground: l.surfaces.contains(LocomotorSurface::Ground),
        surfaces_air: airborne,
        surfaces_water: l.surfaces.contains(LocomotorSurface::Water),
        surfaces_cliff: l.surfaces.contains(LocomotorSurface::Cliff),
        surfaces_rubble: l.surfaces.contains(LocomotorSurface::Rubble),
        allow_motive_force_while_airborne: l.allow_motive_force_while_airborne,
        apply_2d_friction_when_airborne: l.apply_2d_friction_when_airborne,
        speed_limit_z: l.speed_limit_z,
    }
}

/// Every `Locomotor = SET_x name` entry of a template, in set order.
pub fn convert_locomotor_sets(all: &Templates, t: &ThingTemplate) -> BTreeMap<LocomotorSet, LocomotorParams> {
    let mut out = BTreeMap::new();
    if let Some(ai) = t.ai() {
        for (set, refs) in &ai.locomotor_sets {
            if let Some(id) = refs.iter().find_map(|r| r.id) {
                out.insert(sim_locomotor_set(*set), convert_locomotor(all.locomotor_by_id(id)));
            }
        }
    }
    out
}

/// `WeaponTemplate` -> [`SimWeapon`] (values are already frames / per-frame / radians).
pub fn convert_weapon(w: &WeaponTemplate) -> SimWeapon {
    let mut a = 0u8;
    for (e, bit) in [
        (WeaponAffects::SelfTarget, affects::SELF),
        (WeaponAffects::Allies, affects::ALLIES),
        (WeaponAffects::Enemies, affects::ENEMIES),
        (WeaponAffects::Neutrals, affects::NEUTRALS),
        (WeaponAffects::Suicide, affects::KILLS_SELF),
        (WeaponAffects::NotSimilar, affects::DOESNT_AFFECT_SIMILAR),
        (WeaponAffects::NotAirborne, affects::DOESNT_AFFECT_AIRBORNE),
    ] {
        if w.radius_damage_affects.contains(e) {
            a |= bit;
        }
    }
    let mut m = 0u8;
    for (e, bit) in [
        (WeaponAnti::AirborneVehicle, anti::AIRBORNE_VEHICLE),
        (WeaponAnti::Ground, anti::GROUND),
        (WeaponAnti::Projectile, anti::PROJECTILE),
        (WeaponAnti::AirborneInfantry, anti::AIRBORNE_INFANTRY),
        (WeaponAnti::SmallMissile, anti::SMALL_MISSILE),
        (WeaponAnti::Mine, anti::MINE),
        (WeaponAnti::BallisticMissile, anti::BALLISTIC_MISSILE),
        (WeaponAnti::Parachute, anti::PARACHUTE),
    ] {
        if w.anti_mask.contains(e) {
            m |= bit;
        }
    }
    SimWeapon {
        name: w.name.clone(),
        primary_damage: w.primary_damage,
        primary_damage_radius: w.primary_damage_radius,
        secondary_damage: w.secondary_damage,
        secondary_damage_radius: w.secondary_damage_radius,
        attack_range: w.attack_range,
        minimum_attack_range: w.minimum_attack_range,
        damage_type: SimDamageType::from_index(w.damage_type.index()),
        death_type: SimDeathType::from_index(w.death_type.index()),
        min_delay_between_shots: w.min_delay_between_shots,
        max_delay_between_shots: w.max_delay_between_shots,
        clip_size: w.clip_size,
        clip_reload_time: w.clip_reload_time,
        shots_per_barrel: w.shots_per_barrel.max(1),
        pre_attack_delay: w.pre_attack_delay,
        prefire: match w.pre_attack_type {
            WeaponPrefire::PerShot => Prefire::PerShot,
            WeaponPrefire::PerAttack => Prefire::PerAttack,
            WeaponPrefire::PerClip => Prefire::PerClip,
        },
        weapon_speed: w.weapon_speed,
        has_projectile: w.projectile_object.is_some(),
        projectile: None,
        scatter_radius: w.scatter_radius,
        scatter_radius_vs_infantry: w.scatter_radius_vs_infantry,
        damage_dealt_at_self_position: w.damage_dealt_at_self_position,
        auto_reloads_clip: w.auto_reloads_clip == WeaponReload::Yes,
        returns_to_base_to_reload: w.auto_reloads_clip == WeaponReload::ReturnToBase,
        acceptable_aim_delta: w.acceptable_aim_delta,
        affects: a,
        anti_mask: m,
        fire_fx: w.fire_fx[0].clone(),
        projectile_detonation_fx: w.projectile_detonation_fx[0].clone(),
        projectile_exhaust: w.projectile_exhaust.clone(),
        projectile_name: w.projectile_object.as_ref().map(|r| r.name.clone()).unwrap_or_default(),
        projectile_stream_name: w.projectile_stream_name.clone(),
        request_assist_range: w.request_assist_range,
        laser: None,
        die_on_detonate: w.missile_calls_on_die,
        special_power_completion: None,
        weapon_bonus: zr_sim::bonus::WeaponBonusSet {
            entries: w
                .weapon_bonus
                .iter()
                .map(|((cond, field), value)| (cond.index() as u8, field.index() as u8, *value))
                .collect(),
        },
        // `ContinuousFireOne/Two/Coast`, `AutoReloadWhenIdle`, `FireSoundLoopTime` (Weapon.cpp field parsers).
        continuous_fire_one_shots_needed: w.continuous_fire_one_shots_needed,
        continuous_fire_two_shots_needed: w.continuous_fire_two_shots_needed,
        continuous_fire_coast_frames: w.continuous_fire_coast_frames,
        auto_reload_when_idle_frames: w.auto_reload_when_idle_frames,
        fire_sound_loop_time: w.fire_sound_loop_time,
        // `DamageStatusType` (`ObjectStatusMaskType::parseSingleBitFromINI`): the status's bit index is the
        // zr-templates `ObjectStatus` index, which is the C++ `ObjectStatusTypes` order (`names.rs`).
        damage_status_mask: match w.damage_status_type {
            zr_templates::ObjectStatus::None => 0,
            s => 1u64 << s.index(),
        },
        allow_attack_garrisoned_bldgs: w.allow_attack_garrisoned_bldgs,
    }
}

/// `ArmorTemplate` -> [`SimArmor`].
pub fn convert_armor(a: &zr_templates::ArmorTemplate) -> SimArmor {
    let mut out = SimArmor::new(&a.name);
    out.coefficients.copy_from_slice(&a.coefficients);
    out
}

/// Shared sim-side values, one `Arc` per template id / name. The `things` / `ocls` memos also break
/// reference cycles (an OCL creating an object whose die modules reference that OCL again).
#[derive(Default)]
pub struct CombatCache {
    weapons: BTreeMap<WeaponId, Arc<SimWeapon>>,
    armors: BTreeMap<ArmorId, Arc<SimArmor>>,
    things: BTreeMap<String, Arc<ObjectTemplate>>,
    ocls: BTreeMap<OclId, Arc<OclTemplate>>,
    /// Garrison `FIREPOINT` bones per template (see [`crate::firepoints`]); installed before any
    /// template is converted.
    fire_points: BTreeMap<String, GarrisonFirePoints>,
    /// `WeaponLaunchBone` launch positions per template (see [`crate::launch_bones`]); installed
    /// before any template is converted.
    launch_bones: BTreeMap<String, LaunchBones>,
    /// `DockStart`/`DockAction`/`DockEnd`/`DockWaiting` bones per template (see
    /// [`crate::dock_bones`]); installed before any template is converted.
    dock_bones: BTreeMap<String, zr_sim::dock::DockBones>,
    /// `Runway*Parking*` / `Runway*Park*Han` / `Runway*Prep*` / `RunwayStart*` / `RunwayEnd*` /
    /// `HeliPark01` bones per airfield template (see [`crate::parking_bones`]).
    parking_bones: BTreeMap<String, zr_sim::jet_ai::ParkingBones>,
    /// `SpawnPointBoneName` bones per template (see [`crate::spawn_bones`]).
    spawn_bones: BTreeMap<String, Vec<SpawnPointBone>>,
    /// Names currently being converted, to detect a reference cycle.
    visiting: BTreeSet<String>,
    /// OCLs currently being converted (an object created by an OCL may reference that OCL again).
    visiting_ocls: BTreeSet<OclId>,
}

impl CombatCache {
    fn weapon(&mut self, all: &Templates, id: WeaponId) -> Arc<SimWeapon> {
        self.weapons
            .entry(id)
            .or_insert_with(|| {
                let mut s = convert_weapon(all.weapon_by_id(id));
                s.projectile = resolve_projectile(all, all.weapon_by_id(id));
                s.special_power_completion = projectile_power_completion(all, all.weapon_by_id(id));
                s.laser = laser_spec(all, all.weapon_by_id(id));
                Arc::new(s)
            })
            .clone()
    }
    fn armor(&mut self, all: &Templates, id: ArmorId) -> Arc<SimArmor> {
        self.armors.entry(id).or_insert_with(|| Arc::new(convert_armor(all.armor_by_id(id)))).clone()
    }

    /// Memoized [`convert_thing_with`] by name; `None` when the template is missing or on a cycle.
    fn thing(&mut self, all: &Templates, name: &str) -> Option<Arc<ObjectTemplate>> {
        if let Some(a) = self.things.get(name) {
            return Some(a.clone());
        }
        let t = all.thing(name)?;
        if !self.visiting.insert(name.to_owned()) {
            return None; // reference cycle: drop the back-edge
        }
        let ot = convert_thing_with(all, t, self);
        self.visiting.remove(name);
        let a = Arc::new(ot);
        self.things.insert(name.to_owned(), a.clone());
        Some(a)
    }

    /// Memoized OCL conversion by id; `None` on a reference cycle.
    fn ocl(&mut self, all: &Templates, id: OclId) -> Option<Arc<OclTemplate>> {
        if let Some(a) = self.ocls.get(&id) {
            return Some(a.clone());
        }
        if !self.visiting_ocls.insert(id) {
            return None; // reference cycle: drop the back-edge
        }
        let o = all.ocl_by_id(id);
        let a = Arc::new(convert_ocl(all, o, self));
        self.visiting_ocls.remove(&id);
        self.ocls.insert(id, a.clone());
        Some(a)
    }
}

/// `LaserName` / `LaserBoneName` (`Weapon::createLaser`): the laser thing's `LifetimeUpdate` and
/// `DeletionUpdate` lifetimes (`LaserBeam` rolls them like the object does). A laser name with no
/// template is no laser.
fn laser_spec(all: &Templates, w: &WeaponTemplate) -> Option<zr_sim::LaserSpec> {
    thing_laser_spec(all, &w.laser_name, &w.laser_bone_name)
}

/// The laser thing `name` with its timed modules (the same rolls as a weapon's `LaserName`), starting at `bone`.
/// The assisted-targeting feedback lasers use it with an empty bone (`AssistedTargetingUpdate.cpp:118-141`).
fn thing_laser_spec(all: &Templates, name: &str, bone: &str) -> Option<zr_sim::LaserSpec> {
    if name.is_empty() {
        return None;
    }
    let t = all.thing(name)?;
    // Every timed module, in template order: each constructor rolls its lifetime then (`Object::Object`
    // builds the modules in definition order). `LifetimeUpdate` kills, `DeletionUpdate` destroys.
    let rolls = t
        .modules
        .iter()
        .filter_map(|m| match &m.data {
            ModuleData::Lifetime(d) => Some((zr_sim::LaserRollKind::Lifetime, d.min_lifetime, d.max_lifetime)),
            ModuleData::Logic(l) => match &**l {
                LogicModule::Deletion { min_lifetime, max_lifetime } => Some((zr_sim::LaserRollKind::Deletion, *min_lifetime, *max_lifetime)),
                _ => None,
            },
            _ => None,
        })
        .collect();
    Some(zr_sim::LaserSpec { template_name: name.to_string(), bone_name: bone.to_string(), rolls })
}

/// Resolve a weapon's `ProjectileObject` thing into the sim's projectile behavior, if that
/// template carries a `DumbProjectileBehavior` or `MissileAIUpdate`.
fn resolve_projectile(all: &Templates, w: &WeaponTemplate) -> Option<Arc<ProjectileTemplate>> {
    let id = w.projectile_object.as_ref().and_then(|r| r.id)?;
    let t = all.thing_by_id(id);
    // A projectile's `PhysicsBehavior` (every shipped one has it): its friction values. The engine's update order is by
    // sleepy phase, not by module order, so the module list only says whether the physics is there at all.
    let phys = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Physics(p) => Some(p),
        _ => None,
    });
    let friction = phys.map_or(PHYSICS_DEFAULT_FRICTION, |p| [p.forward_friction, p.lateral_friction, p.aerodynamic_friction]);
    let dumb = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::DumbProjectile(d) => Some(DumbProjectileParams { physics: phys.is_some(), friction, ..dumb_params(d) }),
        _ => None,
    });
    let neutron = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Logic(l) => match l.as_ref() {
            LogicModule::NeutronMissile {
                distance_to_travel_before_turning,
                max_turn_rate,
                forward_damping,
                relative_speed,
                target_from_directly_above,
                launch_fx,
                special_speed_time,
                special_speed_height,
                special_accel_factor,
                special_jitter_distance,
                ignition_fx,
                ..
            } => Some(zr_sim::superweapon::NeutronMissileTemplate {
                distance_to_travel_before_turning: *distance_to_travel_before_turning,
                max_turn_rate: *max_turn_rate,
                forward_damping: *forward_damping,
                relative_speed: *relative_speed,
                target_from_directly_above: *target_from_directly_above,
                special_speed_time: *special_speed_time,
                special_speed_height: *special_speed_height,
                special_accel_factor: *special_accel_factor,
                special_jitter_distance: *special_jitter_distance,
                launch_fx: launch_fx.clone(),
                ignition_fx: ignition_fx.clone(),
            }),
            _ => None,
        },
        _ => None,
    });
    let dive_height = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Logic(l) => match l.as_ref() {
            LogicModule::HeightDie { target_height, .. } => Some(*target_height),
            _ => None,
        },
        _ => None,
    });
    let neutron_blast = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Logic(l) => match l.as_ref() {
            LogicModule::NeutronBlast { blast_radius, affect_airborne, affect_allies } => Some(zr_sim::superweapon::NeutronBlastTemplate {
                blast_radius: *blast_radius,
                affect_airborne: *affect_airborne,
                affect_allies: *affect_allies,
            }),
            _ => None,
        },
        _ => None,
    });
    let behavior = if let Some(d) = dumb {
        ProjectileBehavior::Dumb(d)
    } else if let Some(template) = &neutron {
        // The real `NeutronMissileUpdate` launch/attack flight (its own motion states).
        ProjectileBehavior::Neutron(zr_sim::projectile::NeutronParams {
            template: template.clone(),
            dive_height: dive_height.unwrap_or(100.0),
        })
    } else {
        let m = t.ai().and_then(|ai| ai.missile.as_deref())?;
        // The missile's own `SET_NORMAL` locomotor: its speed, acceleration and thrust angle.
        let loco = convert_locomotor_sets(all, t).get(&LocomotorSet::Normal).copied();
        ProjectileBehavior::Missile(MissileParams { friction, ..missile_params(m, loco.as_ref()) })
    };
    let garrison = t
        .modules
        .iter()
        .find_map(|m| match &m.data {
            ModuleData::DumbProjectile(d) => {
                garrison_hit_kill(d.garrison_hit_kill_count, &d.garrison_hit_kill_kindof, &d.garrison_hit_kill_kindof_not, d.garrison_hit_kill_fx.as_ref())
            }
            _ => None,
        })
        .or_else(|| {
            let m = t.ai().and_then(|ai| ai.missile.as_deref())?;
            garrison_hit_kill(m.garrison_hit_kill_count, &m.garrison_hit_kill_kindof, &m.garrison_hit_kill_kindof_not, m.garrison_hit_kill_fx.as_ref())
        });
    Some(Arc::new(ProjectileTemplate {
        behavior,
        bounding_radius: geometry_circle_radius(t),
        small_missile: t.is_kind_of(KindOf::SmallMissile),
        neutron_warhead: neutron.is_some().then(|| t.name.clone()),
        neutron_blast,
        garrison_hit_kill: garrison,
    }))
}

/// The power named by a weapon's projectile object's `SpecialPowerCompletionDie`, if any.
fn projectile_power_completion(all: &Templates, w: &zr_templates::WeaponTemplate) -> Option<String> {
    let t = all.thing_by_id(w.projectile_object.as_ref().and_then(|r| r.id)?);
    t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Logic(l) => match l.as_ref() {
            LogicModule::SpecialPowerCompletionDie { special_power, .. } => special_power.clone(),
            _ => None,
        },
        _ => None,
    })
}

/// `GeometryInfo::getBoundingCircleRadius` for a `zr-templates` geometry.
fn geometry_circle_radius(t: &ThingTemplate) -> f32 {
    let g = &t.geometry;
    match g.geometry_type {
        GeometryType::Box => (g.major_radius * g.major_radius + g.minor_radius * g.minor_radius).sqrt(),
        _ => g.major_radius,
    }
}

fn dumb_params(d: &DumbProjectileData) -> DumbProjectileParams {
    DumbProjectileParams {
        first_height: d.first_height,
        second_height: d.second_height,
        first_percent_indent: d.first_percent_indent,
        second_percent_indent: d.second_percent_indent,
        max_lifespan: d.max_lifespan,
        orient_to_flight_path: d.orient_to_flight_path,
        flight_path_adjust_dist_per_frame: d.flight_path_adjust_dist_per_frame,
        // Set by `resolve_projectile` from the thing's `PhysicsBehavior`.
        physics: false,
        tumble: d.tumble_randomly,
        friction: PHYSICS_DEFAULT_FRICTION,
    }
}

/// `GarrisonHitKillCount` / `RequiredKindOf` / `ForbiddenKindOf` / `FX` of a projectile's
/// `DumbProjectileBehavior` or `MissileAIUpdate` (both behaviors run the same block).
fn garrison_hit_kill(count: i32, required: &KindOfMask, forbidden: &KindOfMask, fx: Option<&String>) -> Option<zr_sim::projectile::GarrisonHitKill> {
    (count > 0).then(|| zr_sim::projectile::GarrisonHitKill {
        count,
        kind_of: kind_of_mask_bits(required),
        kind_of_not: kind_of_mask_bits(forbidden),
        fx: fx.cloned(),
    })
}

fn missile_params(m: &MissileAiData, loco: Option<&LocomotorParams>) -> MissileParams {
    MissileParams {
        try_to_follow_target: m.try_to_follow_target,
        fuel_lifetime: m.fuel_lifetime,
        ignition_delay: m.ignition_delay,
        initial_velocity: m.initial_velocity,
        initial_dist: m.initial_dist,
        dive_distance: m.dive_distance,
        lock_distance: m.lock_distance,
        use_weapon_speed: m.use_weapon_speed,
        detonate_on_no_fuel: m.detonate_on_no_fuel,
        kill_self_delay: m.kill_self_delay,
        max_speed: loco.map_or(0.0, |l| l.max_speed),
        max_accel: loco.map_or(0.0, |l| l.acceleration),
        max_thrust_angle: loco.map_or(0.0, |l| l.max_thrust_angle),
        preferred_height: loco.map_or(0.0, |l| l.preferred_height),
        preferred_height_damping: loco.map_or(1.0, |l| l.preferred_height_damping),
        min_speed: loco.map_or(0.0, |l| l.min_speed),
        braking: loco.map_or(0.0, |l| l.braking),
        close_enough_dist: loco.map_or(1.0, |l| l.close_enough_dist),
        friction: PHYSICS_DEFAULT_FRICTION,
    }
}

/// The normal-set armor, weapons, turret and auto-acquire settings of a thing.
pub fn convert_combat(all: &Templates, t: &ThingTemplate, cache: &mut CombatCache) -> CombatTemplate {
    let armor = t.armor_set_for(ArmorSetFlags::EMPTY).and_then(|s| s.armor.as_ref()).and_then(|r| r.id).map(|id| cache.armor(all, id));
    let weapon_set = t.weapon_set_for(WeaponSetFlags::EMPTY).map(|s| {
        let mut out = WeaponSetTemplate::default();
        for (i, w) in s.weapons.iter().enumerate() {
            out.weapons[i] = w.as_ref().and_then(|r| r.id).map(|id| cache.weapon(all, id));
        }
        out
    });
    let mut c = CombatTemplate { armor, weapon_set, ..CombatTemplate::default() };
    if let Some(lb) = cache.launch_bones.get(&t.name) {
        c.launch_bones = lb.clone();
    }
    c.armor_flag_bits = SetFlagBits {
        player_upgrade: 1 << ArmorSetType::PlayerUpgrade.index(),
        veteran: 1 << ArmorSetType::Veteran.index(),
        elite: 1 << ArmorSetType::Elite.index(),
        hero: 1 << ArmorSetType::Hero.index(),
    };
    c.weapon_flag_bits = SetFlagBits {
        player_upgrade: 1 << WeaponSetType::PlayerUpgrade.index(),
        veteran: 1 << WeaponSetType::Veteran.index(),
        elite: 1 << WeaponSetType::Elite.index(),
        hero: 1 << WeaponSetType::Hero.index(),
    };
    if let Some(ai) = t.ai() {
        if let Some(tu) = ai.turret.as_ref() {
            c.turret_turn_rate = Some(tu.turn_rate);
            c.turret_pitch_rate = Some(tu.pitch_rate);
            c.turret_natural_angle = tu.natural_turret_angle;
            c.turret_allows_pitch = tu.allows_pitch;
            c.turret_fire_pitch = tu.fire_pitch;
            c.turret_min_pitch = tu.min_physical_pitch;
            c.turret_ground_unit_pitch = tu.ground_unit_pitch;
            c.turret_natural_pitch = tu.natural_turret_pitch;
            c.turret_max_height = t.geometry.height;
            c.turret_controlled_slots = tu
                .controlled_weapon_slots
                .iter()
                .fold(0u8, |m, s| m | (1u8 << s.index()));
            c.turret_fire_angle_sweep = tu.turret_fire_angle_sweep;
            c.turret_sweep_speed_modifier = tu.turret_sweep_speed_modifier;
            c.turret_min_idle_scan_angle = tu.min_idle_scan_angle;
            c.turret_max_idle_scan_angle = tu.max_idle_scan_angle;
            c.turret_min_idle_scan_interval = tu.min_idle_scan_interval;
            c.turret_max_idle_scan_interval = tu.max_idle_scan_interval;
            c.turret_recenter_time = tu.recenter_time;
            c.turret_initially_disabled = tu.initially_disabled;
            c.turret_fires_while_turning = tu.fires_while_turning;
        }
        c.turrets_linked = ai.turrets_linked;
        c.auto_acquire_enemies = ai.auto_acquire_enemies_when_idle.contains(AutoAcquire::Yes);
        c.auto_acquire_buildings = ai.auto_acquire_enemies_when_idle.contains(AutoAcquire::AttackBuildings);
        c.auto_acquire_not_while_attacking = ai.auto_acquire_enemies_when_idle.contains(AutoAcquire::Notwhileattacking);
        c.auto_acquire_while_stealthed = ai.auto_acquire_enemies_when_idle.contains(AutoAcquire::Stealthed);
        c.mood_attack_check_rate = ai.mood_attack_check_rate;
    }
    c
}

/// `DeathTypes` mask -> a bit set by `DeathType` index (the sim's `DieMux`).
fn death_bits(m: &Mask<DeathType>) -> u32 {
    m.iter().fold(0u32, |b, d| b | (1u32 << d.index()))
}

/// `Disposition` mask -> a bit set by `DebrisDisposition` index.
fn disposition_bits(m: &Mask<DebrisDisposition>) -> u32 {
    m.iter().fold(0u32, |b, d| b | (1u32 << d.index()))
}

fn phase_index(p: SlowDeathPhase) -> usize {
    match p {
        SlowDeathPhase::Initial => zr_sim::die::PHASE_INITIAL,
        SlowDeathPhase::Midpoint => zr_sim::die::PHASE_MIDPOINT,
        SlowDeathPhase::Final => zr_sim::die::PHASE_FINAL,
    }
}

/// `DieData` -> [`DieModuleTemplate`].
fn convert_die(all: &Templates, die: &DieData, cache: &mut CombatCache) -> DieModuleTemplate {
    let mux = DieMux {
        death_types: death_bits(&die.mux.death_types),
        veterancy_levels: die.mux.veterancy_levels,
        exempt_status: status_bits(die.mux.exempt_status),
        required_status: status_bits(die.mux.required_status),
    };
    let kind = match &die.kind {
        DieKind::Destroy => DieTemplate::Destroy,
        DieKind::InstantDeath { fx, ocl, weapons } => DieTemplate::InstantDeath {
            fx: fx.iter().flatten().cloned().collect(),
            ocl: ocl.iter().filter_map(|r| r.id).filter_map(|id| cache.ocl(all, id)).collect(),
            weapons: weapons.iter().filter_map(|r| r.id).map(|id| cache.weapon(all, id)).collect(),
        },
        DieKind::CreateObject { ocl, transfer_previous_health, transfer_selection } => DieTemplate::CreateObject {
            ocl: ocl.as_ref().and_then(|r| r.id).and_then(|id| cache.ocl(all, id)),
            transfer_previous_health: *transfer_previous_health,
            transfer_selection: *transfer_selection,
        },
        DieKind::FxList { default_death_fx, orient_to_object, initially_active, .. } => DieTemplate::FxList {
            default_death_fx: default_death_fx.clone(),
            orient_to_object: *orient_to_object,
            initially_active: *initially_active,
        },
        DieKind::SlowDeath(sd) => DieTemplate::SlowDeath(Arc::new(convert_slow_death(all, sd, cache))),
        // `CrushDie` / `CreateCrateDie` / `EjectPilotDie`: the death itself destroys the object
        // (the crate/pilot spawn is handled by `GameLogic::die_spawns`).
        DieKind::Crush(_) | DieKind::CreateCrate(_) | DieKind::EjectPilot(_) | DieKind::Keep => DieTemplate::FxList {
            default_death_fx: None,
            orient_to_object: false,
            initially_active: false,
        },
    };
    DieModuleTemplate { mux, kind }
}

/// `SlowDeathData` -> [`SlowDeathTemplate`] (phase lists flattened into `[_; 3]`).
fn convert_slow_death(all: &Templates, sd: &SlowDeathData, cache: &mut CombatCache) -> SlowDeathTemplate {
    let mut fx: [Vec<String>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut ocl: [Vec<Arc<OclTemplate>>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut weapons: [Vec<Arc<SimWeapon>>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for (phase, names) in &sd.fx {
        fx[phase_index(*phase)].extend(names.iter().flatten().cloned());
    }
    for (phase, refs) in &sd.ocl {
        ocl[phase_index(*phase)].extend(refs.iter().filter_map(|r| r.id).filter_map(|id| cache.ocl(all, id)));
    }
    for (phase, refs) in &sd.weapons {
        weapons[phase_index(*phase)].extend(refs.iter().filter_map(|r| r.id).map(|id| cache.weapon(all, id)));
    }
    SlowDeathTemplate {
        probability_modifier: sd.probability_modifier,
        modifier_bonus_per_overkill_percent: sd.modifier_bonus_per_overkill_percent,
        sink_rate: sd.sink_rate,
        sink_delay: sd.sink_delay,
        sink_delay_variance: sd.sink_delay_variance,
        destruction_delay: sd.destruction_delay,
        destruction_delay_variance: sd.destruction_delay_variance,
        destruction_altitude: sd.destruction_altitude,
        fx,
        ocl,
        weapons,
        fling_force: sd.fling_force,
        fling_force_variance: sd.fling_force_variance,
        fling_pitch: sd.fling_pitch,
        fling_pitch_variance: sd.fling_pitch_variance,
        special: None,
    }
}

/// A template `ObjectStatus` mask as the sim's status bits. Template bit `i` is `ObjectStatusTypes.h`
/// value `i`, the same bit as `zr_sim::object::status`. The enum has 45 values, so the template's
/// `u128` fits the sim's `u64` (`zr-game/tests/set_flag_bits.rs` pins the order).
fn status_bits(m: Mask<ObjectStatus>) -> u64 {
    debug_assert!(m.bits() >> 64 == 0, "ObjectStatus mask wider than the sim's status word");
    m.bits() as u64
}

/// A die module's mux from the template layer's `DieMuxData`.
fn convert_mux(mux: &zr_templates::DieMuxData) -> DieMux {
    DieMux {
        death_types: death_bits(&mux.death_types),
        veterancy_levels: mux.veterancy_levels,
        exempt_status: status_bits(mux.exempt_status),
        required_status: status_bits(mux.required_status),
    }
}

/// A mux that applies to every death (for die modules whose `DieMuxData` the parser does not keep).
fn all_deaths_mux() -> DieMux {
    DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, exempt_status: 0, required_status: 0 }
}

/// A single FX name (`None`/`None` are dropped).
fn fx_one(name: &Option<String>) -> Vec<String> {
    name.as_ref().filter(|n| !n.eq_ignore_ascii_case("None")).cloned().into_iter().collect()
}

/// A single OCL name to its sim template, through the cache.
fn ocl_one(all: &Templates, cache: &mut CombatCache, name: &Option<String>) -> Option<Arc<OclTemplate>> {
    let n = name.as_ref()?;
    if n.eq_ignore_ascii_case("None") {
        return None;
    }
    let id = all.ocl_id(n)?;
    cache.ocl(all, id)
}

/// `JetSlowDeath` logic module -> [`zr_sim::die::JetSlowDeathTemplate`].
fn convert_jet_slow_death(all: &Templates, sd: &SlowDeathData, cache: &mut CombatCache, l: &LogicModule) -> zr_sim::die::JetSlowDeathTemplate {
    let LogicModule::JetSlowDeath {
        fx_on_ground_death,
        ocl_on_ground_death,
        fx_initial_death,
        ocl_initial_death,
        delay_secondary_from_initial_death,
        fx_secondary,
        ocl_secondary,
        fx_hit_ground,
        ocl_hit_ground,
        delay_final_blow_up_from_hit_ground,
        fx_final_blow_up,
        ocl_final_blow_up,
        roll_rate,
        roll_rate_delta,
        pitch_rate,
        fall_how_fast,
        ..
    } = l
    else {
        unreachable!("convert_jet_slow_death called with a non-jet module");
    };
    zr_sim::die::JetSlowDeathTemplate {
        slow: Arc::new(convert_slow_death(all, sd, cache)),
        fx_on_ground_death: fx_one(fx_on_ground_death),
        ocl_on_ground_death: ocl_one(all, cache, ocl_on_ground_death).into_iter().collect(),
        fx_initial_death: fx_one(fx_initial_death),
        ocl_initial_death: ocl_one(all, cache, ocl_initial_death).into_iter().collect(),
        delay_secondary_from_initial_death: *delay_secondary_from_initial_death,
        fx_secondary: fx_one(fx_secondary),
        ocl_secondary: ocl_one(all, cache, ocl_secondary).into_iter().collect(),
        fx_hit_ground: fx_one(fx_hit_ground),
        ocl_hit_ground: ocl_one(all, cache, ocl_hit_ground).into_iter().collect(),
        delay_final_blow_up_from_hit_ground: *delay_final_blow_up_from_hit_ground,
        fx_final_blow_up: fx_one(fx_final_blow_up),
        ocl_final_blow_up: ocl_one(all, cache, ocl_final_blow_up).into_iter().collect(),
        roll_rate: *roll_rate,
        roll_rate_delta: *roll_rate_delta,
        pitch_rate: *pitch_rate,
        fall_how_fast: *fall_how_fast,
    }
}

/// `HelicopterSlowDeathBehavior` logic module -> a [`zr_sim::die::SlowDeathTemplate`] with the
/// helicopter spiral/blade extras.
fn convert_helicopter_slow_death(all: &Templates, sd: &SlowDeathData, cache: &mut CombatCache, l: &LogicModule) -> zr_sim::die::SlowDeathTemplate {
    let LogicModule::HelicopterSlowDeath {
        spiral_orbit_turn_rate,
        spiral_orbit_forward_speed,
        spiral_orbit_forward_speed_damping,
        min_self_spin,
        max_self_spin,
        self_spin_update_delay,
        self_spin_update_amount,
        fall_how_fast,
        min_blade_fly_off_delay,
        max_blade_fly_off_delay,
        attach_particle,
        blade_object_name,
        ocl_eject_pilot,
        fx_blade,
        ocl_blade,
        fx_hit_ground,
        ocl_hit_ground,
        fx_final_blow_up,
        ocl_final_blow_up,
        delay_from_ground_to_final_death,
        final_rubble_object,
        sound_death_loop,
        max_braking,
        ..
    } = l
    else {
        unreachable!("convert_helicopter_slow_death called with a non-helicopter module");
    };
    let mut slow = convert_slow_death(all, sd, cache);
    slow.special = Some(zr_sim::die::SpecialDeathExtras::Helicopter(zr_sim::die::HelicopterExtras {
        spiral_orbit_turn_rate: *spiral_orbit_turn_rate,
        spiral_orbit_forward_speed: *spiral_orbit_forward_speed,
        spiral_orbit_forward_speed_damping: *spiral_orbit_forward_speed_damping,
        min_self_spin: *min_self_spin,
        max_self_spin: *max_self_spin,
        self_spin_update_delay: *self_spin_update_delay,
        self_spin_update_amount: *self_spin_update_amount,
        fall_how_fast: *fall_how_fast,
        min_blade_fly_off_delay: *min_blade_fly_off_delay,
        max_blade_fly_off_delay: *max_blade_fly_off_delay,
        attach_particle: attach_particle.clone(),
        blade_object_name: blade_object_name.clone(),
        ocl_eject_pilot: ocl_one(all, cache, ocl_eject_pilot),
        fx_blade: fx_blade.clone(),
        ocl_blade: ocl_one(all, cache, ocl_blade),
        fx_hit_ground: fx_hit_ground.clone(),
        ocl_hit_ground: ocl_one(all, cache, ocl_hit_ground),
        fx_final_blow_up: fx_final_blow_up.clone(),
        ocl_final_blow_up: ocl_one(all, cache, ocl_final_blow_up),
        delay_from_ground_to_final_death: *delay_from_ground_to_final_death,
        final_rubble_object: final_rubble_object.clone(),
        max_braking: *max_braking,
        death_loop_sound: sound_death_loop.clone(),
    }));
    slow
}

/// `NeutronMissileSlowDeathBehavior` logic module -> a [`zr_sim::die::SlowDeathTemplate`] with the
/// falling-warhead blasts.
fn convert_neutron_slow_death(all: &Templates, sd: &SlowDeathData, cache: &mut CombatCache, l: &LogicModule) -> zr_sim::die::SlowDeathTemplate {
    let LogicModule::NeutronMissileSlowDeath { scorch_size, fx_list, blasts, .. } = l else {
        unreachable!("convert_neutron_slow_death called with a non-neutron module");
    };
    let mut slow = convert_slow_death(all, sd, cache);
    slow.special = Some(zr_sim::die::SpecialDeathExtras::Neutron(zr_sim::die::NeutronExtras {
        scorch_size: *scorch_size,
        fx_list: fx_list.clone(),
        blasts: blasts
            .iter()
            .map(|b| zr_sim::die::NeutronBlast {
                enabled: b.enabled,
                delay: b.delay,
                scorch_delay: b.scorch_delay,
                inner_radius: b.inner_radius,
                outer_radius: b.outer_radius,
                max_damage: b.max_damage,
                min_damage: b.min_damage,
                topple_speed: b.topple_speed,
                push_force: b.push_force,
            })
            .collect(),
    }));
    slow
}

/// Whether `ModuleData` is a die-class module (`DieModuleInterface`) the sim runs on death.
/// The behaviour modules of `t` that are sleepy-heap `UpdateModule`s in C++, in INI order: `Object::Object`'s behaviour
/// loop (Object.cpp:405-410) and the `registerObject` push after it. `die_of_module` maps a module position to its
/// index in the sim's `die` list (for the `SlowDeath` kinds). Draw and client-update modules are not in the behaviour
/// array, so they are skipped. A behaviour module with no [`SleepyModuleKind`] is skipped too.
fn sleepy_module_order(t: &ThingTemplate, die_of_module: &[(usize, usize)]) -> Vec<SleepyModuleKind> {
    t.modules
        .iter()
        .enumerate()
        .filter(|(_, m)| m.kind == ModuleKind::Behavior)
        .filter_map(|(module_index, m)| {
            let die = || die_of_module.iter().find(|(i, _)| *i == module_index).map(|&(_, d)| d);
            Some(match &m.data {
                ModuleData::Ai(_) => SleepyModuleKind::Ai,
                ModuleData::AutoHeal(_) => SleepyModuleKind::AutoHeal,
                ModuleData::AutoDeposit(_) => SleepyModuleKind::AutoDeposit,
                ModuleData::BaseRegenerate(_) => SleepyModuleKind::BaseRegenerate,
                ModuleData::Dock(_) => SleepyModuleKind::Dock,
                ModuleData::Flammable(_) => SleepyModuleKind::Flammable,
                ModuleData::Physics(_) => SleepyModuleKind::Physics,
                ModuleData::Poisoned(_) => SleepyModuleKind::Poisoned,
                ModuleData::Production(_) => SleepyModuleKind::Production,
                ModuleData::ProductionExit(_) => SleepyModuleKind::ProductionExit,
                ModuleData::Propaganda(_) => SleepyModuleKind::Propaganda,
                ModuleData::RepairDock(_) => SleepyModuleKind::RepairDock,
                ModuleData::Spawn(_) => SleepyModuleKind::Spawn,
                ModuleData::Stealth(_) => SleepyModuleKind::Stealth,
                ModuleData::StealthDetector(_) => SleepyModuleKind::StealthDetector,
                ModuleData::Topple(_) => SleepyModuleKind::Topple,
                ModuleData::Lifetime(_) => SleepyModuleKind::Lifetime,
                // `SpecialAbility` is a `SpecialPowerModule`, not an `UpdateModule` (`SpecialPowerModule.h`); only
                // the `SpecialAbilityUpdate` half of the pair is a heap entry (`SpecialAbilityUpdate.cpp:86`).
                ModuleData::SpecialAbility(d) if !d.power_module => SleepyModuleKind::SpecialAbility,
                ModuleData::SpecialAbility(_) => return None,
                ModuleData::Slaved(_) => SleepyModuleKind::Slaved,
                ModuleData::Horde(_) => SleepyModuleKind::Horde,
                ModuleData::FireWeapon(_) => SleepyModuleKind::FireWeapon,
                ModuleData::ParkingPlace(_) => SleepyModuleKind::ParkingPlace,
                ModuleData::DumbProjectile(_) => SleepyModuleKind::DumbProjectile,
                ModuleData::Countermeasures(_) => SleepyModuleKind::Countermeasures,
                ModuleData::Minefield(_) => SleepyModuleKind::Minefield,
                ModuleData::GenerateMinefield(_) => SleepyModuleKind::GenerateMinefield,
                // The `ModuleData::SimpleUpdate` catch-all also holds classes that are not update modules; only the
                // four typed `UpdateModule` classes are heap entries (`ProneUpdate.cpp`, `DemoTrapUpdate.cpp`,
                // `CleanupHazardUpdate.cpp`, `CommandButtonHuntUpdate.cpp`).
                ModuleData::SimpleUpdate(d) => match d.class.as_str() {
                    "ProneUpdate" => SleepyModuleKind::Prone,
                    "DemoTrapUpdate" => SleepyModuleKind::DemoTrap,
                    "CleanupHazardUpdate" => SleepyModuleKind::CleanupHazard,
                    "CommandButtonHuntUpdate" => SleepyModuleKind::CommandButtonHunt,
                    _ => return None,
                },
                // `PowerPlantUpdate` and `OverchargeBehavior` are separate modules (and heap entries); both parse into
                // `ModuleData::Overcharge`, so the class name decides which one this is.
                ModuleData::Overcharge(_) => match m.name.as_str() {
                    "OverchargeBehavior" => SleepyModuleKind::Overcharge,
                    _ => SleepyModuleKind::PowerPlant,
                },
                ModuleData::Contain(c) => match c.kind {
                    TemplateContainKind::Garrison => SleepyModuleKind::GarrisonContain,
                    TemplateContainKind::Heal => SleepyModuleKind::HealContain,
                    TemplateContainKind::Open => SleepyModuleKind::OpenContain,
                    TemplateContainKind::Transport => SleepyModuleKind::TransportContain,
                    TemplateContainKind::Tunnel => SleepyModuleKind::TunnelContain,
                    TemplateContainKind::Overlord => SleepyModuleKind::OverlordContain,
                    TemplateContainKind::Helix => SleepyModuleKind::HelixContain,
                    TemplateContainKind::Parachute => SleepyModuleKind::ParachuteContain,
                    TemplateContainKind::InternetHack => SleepyModuleKind::InternetHackContain,
                    TemplateContainKind::RailedTransport => SleepyModuleKind::RailedTransportContain,
                    TemplateContainKind::RiderChange => SleepyModuleKind::RiderChangeContain,
                    TemplateContainKind::MobNexus => SleepyModuleKind::MobNexusContain,
                    TemplateContainKind::Cave => SleepyModuleKind::CaveContain,
                },
                ModuleData::Die(d) if matches!(d.kind, DieKind::SlowDeath(_)) => SleepyModuleKind::SlowDeath { die: die()? },
                ModuleData::Logic(l) => match l.as_ref() {
                    LogicModule::JetSlowDeath { .. }
                    | LogicModule::HelicopterSlowDeath { .. }
                    | LogicModule::BattleBusSlowDeath { .. }
                    | LogicModule::NeutronMissileSlowDeath { .. } => SleepyModuleKind::SlowDeath { die: die()? },
                    LogicModule::Railroad { .. } => SleepyModuleKind::Railroad,
                    LogicModule::RadarUpdate { .. } => SleepyModuleKind::Radar,
                    LogicModule::SpectreGunshipDeployment { .. } => SleepyModuleKind::Spectre,
                    LogicModule::TechBuilding { .. } => SleepyModuleKind::TechBuilding,
                    // `AutoFindHealingUpdate` (`AutoFindHealingUpdate.cpp:102-147`): a plain `UpdateModule`.
                    LogicModule::AutoFindHealing { .. } => SleepyModuleKind::AutoFindHealing,
                    // `AssistedTargetingUpdate` is a plain `UpdateModule` (AssistedTargetingUpdate.cpp:66-76): one heap entry.
                    LogicModule::AssistedTargeting { .. } => SleepyModuleKind::AssistedTargeting,
                    LogicModule::Bridge { .. } => SleepyModuleKind::Bridge,
                    LogicModule::Deletion { .. } => SleepyModuleKind::Deletion,
                    LogicModule::DynamicShroudClearingRange { .. } => SleepyModuleKind::DynamicShroud,
                    _ => return None,
                },
                _ => return None,
            })
        })
        .collect()
}

fn is_die_module(m: &ModuleData) -> bool {
    match m {
        ModuleData::Die(_) | ModuleData::RebuildHole(_) | ModuleData::RebuildHoleExpose(_) | ModuleData::GenerateMinefield(_) => true,
        ModuleData::Logic(l) => matches!(
            l.as_ref(),
            LogicModule::JetSlowDeath { .. }
                | LogicModule::UpgradeDie { .. }
                | LogicModule::SpecialPowerCompletionDie { .. }
                | LogicModule::DamDie { .. }
                | LogicModule::HelicopterSlowDeath { .. }
                | LogicModule::BattleBusSlowDeath { .. }
                | LogicModule::NeutronMissileSlowDeath { .. }
                | LogicModule::NeutronBlast { .. }
        ),
        _ => false,
    }
}

/// Convert one die-class `ModuleData` into a [`DieModuleTemplate`].
fn convert_module_die(all: &Templates, m: &ModuleData, cache: &mut CombatCache) -> Option<DieModuleTemplate> {
    match m {
        ModuleData::Die(d) => Some(convert_die(all, d, cache)),
        ModuleData::RebuildHole(_) => Some(DieModuleTemplate { mux: all_deaths_mux(), kind: DieTemplate::RebuildHole }),
        ModuleData::RebuildHoleExpose(d) => Some(DieModuleTemplate {
            mux: all_deaths_mux(),
            kind: DieTemplate::RebuildHoleExpose(zr_sim::modules4::RebuildHoleExposeTemplate {
                hole_name: d.hole_name.clone(),
                hole_max_health: d.hole_max_health,
                transfer_attackers: d.transfer_attackers,
            }),
        }),
        ModuleData::GenerateMinefield(_) => Some(DieModuleTemplate { mux: all_deaths_mux(), kind: DieTemplate::GenerateMinefield }),
        ModuleData::Logic(l) => match l.as_ref() {
            LogicModule::JetSlowDeath { slow, mux, .. } => {
                let jet = convert_jet_slow_death(all, slow, cache, l);
                Some(DieModuleTemplate { mux: convert_mux(mux), kind: DieTemplate::JetSlowDeath(Arc::new(jet)) })
            }
            LogicModule::UpgradeDie { upgrade_to_remove, mux } => Some(DieModuleTemplate {
                mux: convert_mux(mux),
                kind: DieTemplate::Upgrade {
                    upgrade: (!upgrade_to_remove.is_empty()).then(|| upgrade_to_remove.clone()),
                },
            }),
            LogicModule::SpecialPowerCompletionDie { special_power, mux } => Some(DieModuleTemplate {
                mux: convert_mux(mux),
                kind: DieTemplate::SpecialPowerCompletion { power: special_power.clone().unwrap_or_default() },
            }),
            LogicModule::DamDie { mux } => Some(DieModuleTemplate { mux: convert_mux(mux), kind: DieTemplate::Dam }),
            LogicModule::HelicopterSlowDeath { slow, mux, .. } => Some(DieModuleTemplate {
                mux: convert_mux(mux),
                kind: DieTemplate::SlowDeath(Arc::new(convert_helicopter_slow_death(all, slow, cache, l))),
            }),
            LogicModule::BattleBusSlowDeath { slow, mux, .. } => Some(DieModuleTemplate {
                mux: convert_mux(mux),
                // The real-death path is the base slow death (the alternate "first death" throw is
                // not modelled; see the module note).
                kind: DieTemplate::SlowDeath(Arc::new(convert_slow_death(all, slow, cache))),
            }),
            LogicModule::NeutronMissileSlowDeath { slow, mux, .. } => Some(DieModuleTemplate {
                mux: convert_mux(mux),
                kind: DieTemplate::SlowDeath(Arc::new(convert_neutron_slow_death(all, slow, cache, l))),
            }),
            LogicModule::NeutronBlast { blast_radius, affect_airborne, affect_allies } => Some(DieModuleTemplate {
                mux: all_deaths_mux(),
                kind: DieTemplate::NeutronBlast(zr_sim::superweapon::NeutronBlastTemplate {
                    blast_radius: *blast_radius,
                    affect_airborne: *affect_airborne,
                    affect_allies: *affect_allies,
                }),
            }),
            _ => None,
        },
        _ => None,
    }
}

/// `ObjectCreationList` -> [`OclTemplate`].
fn convert_ocl(all: &Templates, ocl: &ObjectCreationList, cache: &mut CombatCache) -> OclTemplate {
    let mut nuggets = Vec::with_capacity(ocl.nuggets.len());
    for n in &ocl.nuggets {
        match n {
            Nugget::CreateObject(g) => nuggets.push(OclNugget::CreateObject(convert_create_object(all, g, true, cache))),
            Nugget::CreateDebris(g) => nuggets.push(OclNugget::CreateDebris(convert_create_object(all, g, false, cache))),
            Nugget::FireWeapon(fw) => {
                if let Some(id) = fw.weapon.id {
                    nuggets.push(OclNugget::FireWeapon(cache.weapon(all, id)));
                } else {
                    nuggets.push(OclNugget::Unsupported);
                }
            }
            Nugget::ApplyRandomForce(f) => nuggets.push(OclNugget::ApplyRandomForce(ApplyRandomForceTemplate {
                spin_rate: f.spin_rate,
                min_force_magnitude: f.min_force_magnitude,
                max_force_magnitude: f.max_force_magnitude,
                min_force_pitch: f.min_force_pitch,
                max_force_pitch: f.max_force_pitch,
            })),
            Nugget::Attack(a) => nuggets.push(OclNugget::Attack(AttackTemplate {
                number_of_shots: a.number_of_shots,
                weapon_slot: a.weapon_slot.index(),
            })),
            Nugget::DeliverPayload(dp) => {
                let transport = if dp.transport.is_empty() || dp.transport.eq_ignore_ascii_case("None") {
                    None
                } else {
                    cache.thing(all, &dp.transport)
                };
                let payload = dp
                    .payload
                    .iter()
                    .filter_map(|p| cache.thing(all, &p.name).map(|t| (t, p.count.max(0) as u32)))
                    .collect();
                nuggets.push(OclNugget::DeliverPayload(std::sync::Arc::new(DeliverPayloadTemplate {
                    transport,
                    payload,
                    delivery_distance: dp.data.delivery_distance,
                    drop_delay: dp.data.drop_delay,
                    delay_delivery_frames_max: dp.delay_delivery_frames_max,
                    drop_offset: dp.data.drop_offset,
                    drop_variance: dp.data.drop_variance,
                    is_parachute_directly: dp.data.is_parachute_directly,
                    self_destruct_object: dp.data.self_destruct_object,
                    fire_weapon: dp.data.fire_weapon,
                    strafing_weapon_slot: dp.data.strafing_weapon_slot.map_or(0, |s| s.index() as u32),
                    strafe_length: dp.data.strafe_length,
                })));
            }
            _ => nuggets.push(OclNugget::Unsupported),
        }
    }
    OclTemplate { name: ocl.name.clone(), nuggets }
}

/// `GenericObjectCreationNugget` -> [`CreateObjectTemplate`].
fn convert_create_object(all: &Templates, g: &GenericNugget, is_object: bool, cache: &mut CombatCache) -> CreateObjectTemplate {
    let objects = if is_object {
        g.names.iter().map(|n| cache.thing(all, n)).collect()
    } else {
        // `CreateDebris` always uses the engine's `GenericDebris` template (when it exists).
        vec![cache.thing(all, "GenericDebris")]
    };
    CreateObjectTemplate {
        is_object,
        names_len: g.names.len(),
        objects,
        count: g.count,
        offset: g.offset,
        disposition: disposition_bits(&g.disposition),
        disposition_intensity: g.disposition_intensity,
        spin_rate: g.spin_rate,
        yaw_rate: g.yaw_rate,
        roll_rate: g.roll_rate,
        pitch_rate: g.pitch_rate,
        min_force_pitch: g.min_force_pitch,
        max_force_pitch: g.max_force_pitch,
        min_force_magnitude: g.min_force_magnitude,
        max_force_magnitude: g.max_force_magnitude,
        orient_in_force_direction: g.orient_in_force_direction,
        spread_formation: g.spread_formation,
        min_distance_a_formation: g.min_distance_a_formation,
        min_distance_b_formation: g.min_distance_b_formation,
        max_distance_formation: g.max_distance_formation,
        min_health: g.min_health,
        max_health: g.max_health,
        requires_live_player: g.requires_live_player,
    }
}

/// `Prerequisites` -> the sim's [`Prerequisite`] list.
fn convert_prerequisite(p: &TemplatePrerequisite) -> Prerequisite {
    match p {
        TemplatePrerequisite::AnyOfObjects(names) => Prerequisite::AnyOfObjects(names.clone()),
        TemplatePrerequisite::Science(s) => Prerequisite::Science(s.clone()),
    }
}

/// `ProductionUpdate` + a production exit update -> [`ProductionTemplate`].
fn convert_production(t: &ThingTemplate, spawn_bones: &[SpawnPointBone]) -> Option<ProductionTemplate> {
    let prod = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Production(p) => Some(p),
        _ => None,
    })?;
    let exit = t
        .modules
        .iter()
        .find_map(|m| match &m.data {
            ModuleData::ProductionExit(e) => Some(convert_exit(e, spawn_bones)),
            _ => None,
        })
        .unwrap_or_default();
    Some(ProductionTemplate {
        max_queue_entries: prod.max_queue_entries,
        quantity_modifiers: prod
            .quantity_modifiers
            .iter()
            .map(|q| QuantityModifier { template_name: q.template_name.clone(), quantity: q.quantity })
            .collect(),
        exit,
        // `DisabledType`'s `repr(u8)` order matches the sim's `disabled::DisabledType`.
        disabled_types_to_process: prod.disabled_types_to_process.iter().fold(0u16, |m, d| m | (1u16 << (d as u8))),
        num_door_animations: prod.num_door_animations,
        door_opening_time: prod.door_opening_time,
        door_wait_open_time: prod.door_wait_open_time,
        door_close_time: prod.door_close_time,
    })
}

/// The production exit update -> the sim's [`ProductionExit`]. `spawn_bones` are the model's
/// `SpawnPointBoneName` bones (see [`crate::spawn_bones`]), used by a spawn point exit only.
fn convert_exit(e: &ProductionExitData, spawn_bones: &[SpawnPointBone]) -> ProductionExit {
    match e.kind {
        ProductionExitKind::Queue => ProductionExit::Queue {
            unit_create_point: e.unit_create_point,
            natural_rally_point: e.natural_rally_point,
            exit_delay_frames: e.exit_delay,
            initial_burst: e.initial_burst,
            allow_airborne_creation: e.allow_airborne_creation,
        },
        ProductionExitKind::SupplyCenter => ProductionExit::SupplyCenter {
            unit_create_point: e.unit_create_point,
            natural_rally_point: e.natural_rally_point,
            grant_temporary_stealth: e.grant_temporary_stealth,
        },
        ProductionExitKind::Default => {
            ProductionExit::Default { unit_create_point: e.unit_create_point, natural_rally_point: e.natural_rally_point }
        }
        ProductionExitKind::SpawnPoint => ProductionExit::SpawnPoint { bones: spawn_bones.to_vec() },
    }
}

/// The contain module data -> sim [`ContainTemplate`] (first contain module wins). `fire_points`
/// are the garrison `FIREPOINT` bones resolved from the install's W3D art ([`crate::firepoints`]).
fn convert_contain(t: &ThingTemplate, fire_points: Option<&GarrisonFirePoints>) -> Option<ContainTemplate> {
    let d = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Contain(c) => Some(c.as_ref()),
        _ => None,
    })?;
    let kind = match d.kind {
        zr_templates::ContainKind::Open => SimContainKind::Open,
        zr_templates::ContainKind::Transport => SimContainKind::Transport,
        zr_templates::ContainKind::Garrison => SimContainKind::Garrison,
        zr_templates::ContainKind::Tunnel => SimContainKind::Tunnel,
        zr_templates::ContainKind::Heal => SimContainKind::Heal,
        zr_templates::ContainKind::Overlord => SimContainKind::Overlord,
        zr_templates::ContainKind::Helix => SimContainKind::Helix,
        zr_templates::ContainKind::Parachute => SimContainKind::Parachute,
        zr_templates::ContainKind::InternetHack => SimContainKind::InternetHack,
        zr_templates::ContainKind::RailedTransport => SimContainKind::RailedTransport,
        zr_templates::ContainKind::RiderChange => SimContainKind::RiderChange,
        zr_templates::ContainKind::MobNexus => SimContainKind::MobNexus,
        zr_templates::ContainKind::Cave => SimContainKind::Cave,
    };
    let allow_all = (0..KindOf::count()).all(|i| d.allow_inside_kind_of.contains(KindOf::from_index(i)));
    let mut initial_payload: Vec<(String, u32)> = Vec::new();
    if let Some((name, count)) = &d.initial_payload {
        initial_payload.push((name.clone(), (*count).max(0) as u32));
    }
    if let Some((name, count)) = &d.initial_roster {
        initial_payload.push((name.clone(), (*count).max(0) as u32));
    }
    // `OverlordContain`/`HelixContain` use a name list (`PayloadTemplateName`), one object per name
    // (OverlordContain.cpp:126-147, HelixContain.cpp:148-189). `createPayload` feeds the same list.
    for name in &d.payload_template_names {
        initial_payload.push((name.clone(), 1));
    }
    Some(ContainTemplate {
        kind,
        capacity: d.capacity(),
        allow_inside_kind_of: if allow_all { None } else { Some(kind_of_mask_bits(&d.allow_inside_kind_of)) },
        forbid_inside_kind_of: kind_of_mask_bits(&d.forbid_inside_kind_of),
        allow_allies_inside: d.allow_allies_inside,
        allow_enemies_inside: d.allow_enemies_inside,
        allow_neutral_inside: d.allow_neutral_inside,
        passengers_allowed_to_fire: d.passengers_allowed_to_fire,
        damage_percentage_to_units: d.damage_percentage_to_units,
        number_of_exit_paths: d.number_of_exit_paths,
        exit_delay: d.exit_delay,
        door_open_time: d.door_open_time,
        scatter_nearby_on_exit: d.scatter_nearby_on_exit,
        orient_like_container_on_exit: d.orient_like_container_on_exit,
        // `TransportContain::onRemoving`'s mood wake: the transport family only (the flag is parsed for all containers).
        reset_mood_check_on_exit: kind.is_transport() && d.reset_mood_check_time_on_exit,
        // `TransportContainModuleData::m_healthRegen` (`HealthRegen%PerSec`), read by
        // `TransportContain::update` and copied by `MobNexusContain::update` (MobNexusContain.cpp:335-380).
        // Zero for every other class (they have no such field).
        health_regen: if kind.is_transport() || kind == SimContainKind::MobNexus { d.health_regen } else { 0.0 },
        // `GarrisonContainModuleData::m_doIHealObjects` (`HealObjects`, GarrisonContain.h:69).
        do_i_heal_objects: d.do_i_heal_objects,
        // `TimeForFullHeal`: `HealContainModuleData::m_framesForFullHeal` is whole frames
        // (HealContain.cpp:62), `GarrisonContainModuleData::m_framesForFullHeal` a Real
        // (GarrisonContain.h:70).
        heal_frames: match kind {
            SimContainKind::Heal => d.heal_frames as f32,
            SimContainKind::Garrison => d.frames_for_full_heal,
            _ => 0.0,
        },
        initial_payload,
        tunnel_frames_for_full_heal: d.tunnel_frames_for_full_heal,
        is_enclosing_container: d.is_enclosing_container,
        fire_points: fire_points.cloned().unwrap_or_default(),
        passengers_in_turret: d.passengers_in_turret,
        weapon_bonus_passed_to_passengers: d.weapon_bonus_passed_to_passengers,
        // `GarrisonContainModuleData::m_immuneToClearBuildingAttacks` (`GarrisonContain.h:72`).
        immune_to_clear_building_attacks: d.immune_to_clear_building_attacks,
    })
}

/// The `RiderChangeContain` rider table (`RiderChangeContainModuleData`) in sim units. Unfilled slots
/// and `NONE` names drop out. Condition names resolve through the sim's `ModelConditionFlags` table;
/// the weapon set and object status masks keep their template bit order (the sim's `WEAPONSET_*` and
/// `status` constants, see `zr-game/tests/set_flag_bits.rs`).
fn convert_rider_change(t: &ThingTemplate) -> Option<Arc<RiderChangeTemplate>> {
    let d = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Contain(c) if c.kind == zr_templates::ContainKind::RiderChange => Some(c.as_ref()),
        _ => None,
    })?;
    let condition = |name: &str| if name.eq_ignore_ascii_case("NONE") { None } else { zr_sim::condition::bit_for_name(name) };
    Some(Arc::new(RiderChangeTemplate {
        riders: d
            .riders
            .iter()
            .filter(|r| !r.template_name.is_empty())
            .map(|r| RiderSlot {
                template_name: r.template_name.clone(),
                model_condition: condition(&r.model_condition),
                weapon_set_flag: mask_bits(&r.weapon_set),
                object_status: status_bits(r.object_status),
                locomotor_set: sim_locomotor_set(r.locomotor_set),
                command_set: r.command_set.clone(),
            })
            .collect(),
        scuttle_frames: d.scuttle_frames,
        scuttle_state: condition(&d.scuttle_state),
    }))
}

/// A supply dock module -> the sim's [`DockTemplate`].
fn convert_dock(t: &ThingTemplate) -> Option<DockTemplate> {
    let d = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Dock(d) => Some(d),
        _ => None,
    })?;
    Some(DockTemplate {
        kind: match d.kind {
            TemplateDockKind::SupplyCenter => DockKind::Center,
            TemplateDockKind::SupplyWarehouse => DockKind::Warehouse,
            TemplateDockKind::RailedTransport => DockKind::Warehouse,
        },
        starting_boxes: d.starting_boxes,
        delete_when_empty: d.delete_when_empty,
        number_approach_positions: d.number_approach_positions,
        allows_passthrough: d.allows_passthrough,
    })
}

/// A `SupplyTruckAIUpdate` module's raw fields (it derives `AIUpdateModuleData`, so its own fields
/// sit in `AiUpdateData::extra`) -> the sim's [`SupplyTruckTemplate`]. `ChinookAIUpdate` derives
/// `SupplyTruckAIUpdate`; `WorkerAIUpdate` implements `SupplyTruckAIInterface` with the same field
/// names (WorkerAIUpdate.h:66), so both are treated as supply trucks.
fn convert_supply_truck(t: &ThingTemplate) -> Option<SupplyTruckTemplate> {
    let ai = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Ai(a) if matches!(a.class_name.as_str(), "SupplyTruckAIUpdate" | "ChinookAIUpdate" | "WorkerAIUpdate") => {
            Some(a)
        }
        _ => None,
    })?;
    let mut st = SupplyTruckTemplate { max_boxes: 1, center_action_delay: 0, warehouse_action_delay: 0, scan_distance: 0.0 };
    for item in &ai.extra {
        let RawItem::Field(f) = item else { continue };
        match f.key.as_str() {
            "MaxBoxes" => st.max_boxes = f.value.trim().parse().unwrap_or(1),
            "SupplyCenterActionDelay" => {
                st.center_action_delay = f.value.trim().parse().map(zr_templates::value::duration_ms_to_frames_ceil).unwrap_or(0)
            }
            "SupplyWarehouseActionDelay" => {
                st.warehouse_action_delay = f.value.trim().parse().map(zr_templates::value::duration_ms_to_frames_ceil).unwrap_or(0)
            }
            "SupplyWarehouseScanDistance" => st.scan_distance = f.value.trim().parse().unwrap_or(0.0),
            _ => {}
        }
    }
    Some(st)
}

/// `ChinookAIUpdate`'s combat-drop fields (`AiUpdateData::extra`) -> the sim's
/// [`ChinookTemplate`]. Velocities become per frame, durations become frames.
fn convert_chinook(t: &ThingTemplate) -> Option<ChinookTemplate> {
    let ai = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Ai(a) if a.class_name == "ChinookAIUpdate" => Some(a),
        _ => None,
    })?;
    let mut ch = ChinookTemplate::default();
    for item in &ai.extra {
        let RawItem::Field(f) = item else { continue };
        match f.key.as_str() {
            "NumRopes" => ch.num_ropes = f.value.trim().parse().unwrap_or(ch.num_ropes),
            "RopeDropSpeed" => {
                ch.rope_drop_speed = f.value.trim().parse().map(zr_templates::value::velocity_per_sec_to_per_frame).unwrap_or(ch.rope_drop_speed)
            }
            "RappelSpeed" => {
                ch.rappel_speed = f.value.trim().parse().map(zr_templates::value::velocity_per_sec_to_per_frame).unwrap_or(ch.rappel_speed)
            }
            "PerRopeDelayMin" => {
                ch.per_rope_delay_min = f.value.trim().parse().map(zr_templates::value::duration_ms_to_frames_ceil).unwrap_or(ch.per_rope_delay_min)
            }
            "PerRopeDelayMax" => {
                ch.per_rope_delay_max = f.value.trim().parse().map(zr_templates::value::duration_ms_to_frames_ceil).unwrap_or(ch.per_rope_delay_max)
            }
            "MinDropHeight" => ch.min_drop_height = f.value.trim().parse().unwrap_or(ch.min_drop_height),
            "WaitForRopesToDrop" => ch.wait_for_ropes_to_drop = f.value.trim().eq_ignore_ascii_case("yes"),
            "RopeFinalHeight" => ch.rope_final_height = f.value.trim().parse().unwrap_or(ch.rope_final_height),
            _ => {}
        }
    }
    Some(ch)
}

/// `AssaultTransportAIUpdate`'s module fields (`AiUpdateData::extra`) -> the sim's [`AssaultTransportTemplate`]
/// (AssaultTransportAIUpdate.h:44-52). Only `MembersGetHealedAtLifeRatio` is read by the sim; the default is 0.
fn convert_assault_transport(t: &ThingTemplate) -> Option<zr_sim::assault_transport::AssaultTransportTemplate> {
    let ai = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Ai(a) if a.class_name == "AssaultTransportAIUpdate" => Some(a),
        _ => None,
    })?;
    let mut at = zr_sim::assault_transport::AssaultTransportTemplate::default();
    for item in &ai.extra {
        let RawItem::Field(f) = item else { continue };
        if f.key == "MembersGetHealedAtLifeRatio" {
            at.members_get_healed_at_life_ratio = f.value.trim().parse().unwrap_or(at.members_get_healed_at_life_ratio);
        }
    }
    Some(at)
}

/// A `DozerAIUpdate` / `WorkerAIUpdate` module's raw fields -> the sim's
/// [`zr_sim::dozer::DozerTemplate`]. `WorkerAIUpdate` derives from `SupplyTruckAIUpdate` but
/// implements `DozerAIInterface` with the same field names (`WorkerAIUpdate.h:61-110`).
fn convert_dozer(t: &ThingTemplate) -> Option<zr_sim::dozer::DozerTemplate> {
    let ai = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Ai(a) if matches!(a.class_name.as_str(), "DozerAIUpdate" | "WorkerAIUpdate") => Some(a),
        _ => None,
    })?;
    let mut d = zr_sim::dozer::DozerTemplate::default();
    for item in &ai.extra {
        let RawItem::Field(f) = item else { continue };
        match f.key.as_str() {
            "RepairHealthPercentPerSecond" => {
                d.repair_health_percent_per_second = zr_templates::value::scan_percent(f.value.trim()).unwrap_or(0.0)
            }
            // `BoredTime` (`INI::parseDurationReal`: ms -> frames, real).
            "BoredTime" => {
                d.bored_time_frames = f.value.trim().parse().map(zr_templates::value::duration_ms_to_frames).unwrap_or(0.0)
            }
            "BoredRange" => d.bored_range = f.value.trim().parse().unwrap_or(0.0),
            _ => {}
        }
    }
    Some(d)
}

/// A `HackInternetAIUpdate` module's raw fields -> the sim's
/// [`zr_sim::hack_internet::HackInternetTemplate`] (`HackInternetAIUpdateModuleData`). The durations
/// are `INI::parseDurationUnsignedInt` (ms rounded up to frames); the cash amounts and XP are plain
/// integers; `PackUnpackVariationFactor` is an `AIUpdateModuleData` real.
fn convert_hack_internet(t: &ThingTemplate) -> Option<zr_sim::hack_internet::HackInternetTemplate> {
    let ai = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Ai(a) if a.class_name == "HackInternetAIUpdate" => Some(a),
        _ => None,
    })?;
    let mut h = zr_sim::hack_internet::HackInternetTemplate::default();
    let ms = |v: &str| v.trim().parse::<u32>().map(zr_templates::value::duration_ms_to_frames_ceil).unwrap_or(0);
    let int = |v: &str| v.trim().parse::<u32>().unwrap_or(0);
    for item in &ai.extra {
        let RawItem::Field(f) = item else { continue };
        match f.key.as_str() {
            "UnpackTime" => h.unpack_frames = ms(&f.value),
            "PackTime" => h.pack_frames = ms(&f.value),
            "CashUpdateDelay" => h.cash_update_delay = ms(&f.value),
            "CashUpdateDelayFast" => h.cash_update_delay_fast = ms(&f.value),
            "RegularCashAmount" => h.regular_cash = int(&f.value),
            "VeteranCashAmount" => h.veteran_cash = int(&f.value),
            "EliteCashAmount" => h.elite_cash = int(&f.value),
            "HeroicCashAmount" => h.heroic_cash = int(&f.value),
            "XpPerCashUpdate" => h.xp_per_cash_update = int(&f.value),
            "PackUnpackVariationFactor" => h.pack_unpack_variation = f.value.trim().parse().unwrap_or(0.0),
            _ => {}
        }
    }
    Some(h)
}

/// `Mask<T>` -> a raw condition bitmask (`ArmorSetFlags` / `WeaponSetFlags`).
fn mask_bits<T: NameEnum>(m: &Mask<T>) -> u32 {
    m.iter().fold(0u32, |b, e| b | 1 << e.index())
}

/// Armor sets with their condition masks (`ArmorTemplateSet`).
fn convert_armor_sets(all: &Templates, t: &ThingTemplate, cache: &mut CombatCache) -> Vec<(u32, Option<Arc<SimArmor>>)> {
    t.armor_sets
        .iter()
        .map(|s| {
            let armor = s.armor.as_ref().and_then(|r| r.id).map(|id| cache.armor(all, id));
            (mask_bits(&s.conditions), armor)
        })
        .collect()
}

/// Weapon sets with their condition masks (`WeaponTemplateSet`).
fn convert_weapon_sets(all: &Templates, t: &ThingTemplate, cache: &mut CombatCache) -> Vec<(u32, WeaponSetTemplate)> {
    t.weapon_sets
        .iter()
        .map(|s| {
            let mut out = WeaponSetTemplate::default();
            for (i, w) in s.weapons.iter().enumerate() {
                out.weapons[i] = w.as_ref().and_then(|r| r.id).map(|id| cache.weapon(all, id));
            }
            (mask_bits(&s.conditions), out)
        })
        .collect()
}

/// `GrantUpgradeCreate` modules of a template (`GrantUpgradeCreateModuleData`,
/// `GrantUpgradeCreate.cpp:48-62`). The upgrade's *scope* is not part of the module data: it is
/// `UpgradeTemplate::getUpgradeType()`, which `onCreate`/`onBuildComplete` read to pick
/// `Player::addUpgrade` or `Object::giveUpgrade` (`:104-112`, `:129-137`).
fn convert_grant_upgrade_create(all: &Templates, t: &ThingTemplate) -> Vec<zr_sim::GrantUpgradeCreateTemplate> {
    t.modules
        .iter()
        .filter_map(|m| match &m.data {
            ModuleData::Logic(l) => match l.as_ref() {
                LogicModule::GrantUpgradeCreate { upgrade_to_grant, exempt_status } => {
                    Some(zr_sim::GrantUpgradeCreateTemplate {
                        upgrade: upgrade_to_grant.clone(),
                        // A missing upgrade template is a `DEBUG_CRASH` in the engine; the sim's
                        // default upgrade scope is `UPGRADE_TYPE_PLAYER` (`UpgradeTemplate`'s ctor).
                        player_upgrade: all
                            .upgrade(upgrade_to_grant)
                            .is_none_or(|u| u.upgrade_type == zr_templates::UpgradeType::Player),
                        exempt_under_construction: exempt_status.contains(ObjectStatus::UnderConstruction),
                    })
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Typed upgrade modules of a template (`ArmorUpgrade`, `WeaponSetUpgrade`, ...).
fn convert_upgrade_modules(all: &Templates, t: &ThingTemplate, cache: &mut CombatCache) -> Vec<Arc<SimUpgradeModule>> {
    t.modules
        .iter()
        .filter_map(|m| match &m.data {
            ModuleData::Upgrade(u) => Some(Arc::new(SimUpgradeModule {
                triggered_by: u.mux.triggered_by.clone(),
                conflicts_with: u.mux.conflicts_with.clone(),
                removes_upgrades: u.mux.removes_upgrades.clone(),
                requires_all_triggers: u.mux.requires_all_triggers,
                effect: match &u.kind {
                    UpgradeKind::Armor => UpgradeEffect::Armor { flag_bit: 1 << ArmorSetType::PlayerUpgrade.index() },
                    UpgradeKind::WeaponSet => UpgradeEffect::WeaponSet { flag_bit: 1 << WeaponSetType::PlayerUpgrade.index() },
                    UpgradeKind::LocomotorSet => UpgradeEffect::Locomotor { flag_bit: 0 },
                    UpgradeKind::MaxHealth { add_max_health, change_type } => UpgradeEffect::MaxHealth {
                        add_max_health: *add_max_health,
                        change_type: match change_type {
                            TemplateMaxHealth::SameCurrentHealth => zr_sim::body::MaxHealthChangeType::SameCurrentHealth,
                            TemplateMaxHealth::PreserveRatio => zr_sim::body::MaxHealthChangeType::PreserveRatio,
                            TemplateMaxHealth::AddCurrentHealthToo | TemplateMaxHealth::FullyHeal => {
                                zr_sim::body::MaxHealthChangeType::AddCurrentHealthToo
                            }
                        },
                    },
                    UpgradeKind::ObjectCreation { ocl } => {
                        UpgradeEffect::ObjectCreation { ocl: ocl.as_ref().and_then(|r| r.id).and_then(|id| cache.ocl(all, id)) }
                    }
                    UpgradeKind::CommandSet { command_set, command_set_alt, trigger_alt } => UpgradeEffect::CommandSet {
                        command_set: command_set.clone(),
                        command_set_alt: command_set_alt.clone(),
                        trigger_alt: trigger_alt.clone(),
                    },
                    UpgradeKind::ExperienceScalar { add_xp_scalar } => UpgradeEffect::ExperienceScalar { add_xp_scalar: *add_xp_scalar },
                    UpgradeKind::WeaponBonus => UpgradeEffect::WeaponBonus,
                    UpgradeKind::ModelCondition { flag } => UpgradeEffect::ModelCondition { flag: flag.clone() },
                },
            })),
            // `PowerPlantUpgrade` is a GameLogic upgrade module (not `ModuleData::Upgrade`): while
            // active it adds the template's `EnergyBonus` to the object's power.
            ModuleData::Logic(l) => match l.as_ref() {
                LogicModule::PowerPlantUpgrade { mux } => Some(Arc::new(SimUpgradeModule {
                    triggered_by: mux.triggered_by.clone(),
                    conflicts_with: mux.conflicts_with.clone(),
                    removes_upgrades: mux.removes_upgrades.clone(),
                    requires_all_triggers: mux.requires_all_triggers,
                    effect: UpgradeEffect::PowerPlant,
                })),
                // `UnpauseSpecialPowerUpgrade` (UnpauseSpecialPowerUpgrade.cpp:84).
                LogicModule::UnpauseSpecialPower { special_power: Some(power), mux } => Some(Arc::new(SimUpgradeModule {
                    triggered_by: mux.triggered_by.clone(),
                    conflicts_with: mux.conflicts_with.clone(),
                    removes_upgrades: mux.removes_upgrades.clone(),
                    requires_all_triggers: mux.requires_all_triggers,
                    effect: UpgradeEffect::UnpauseSpecialPower { power: power.clone() },
                })),
                // `StealthUpgrade` (StealthUpgrade.cpp:51-70): `upgradeImplementation` sets
                // `OBJECT_STATUS_CAN_STEALTH` (and gives it to a spawner's slaves).
                LogicModule::StealthUpgrade { mux } => Some(Arc::new(SimUpgradeModule {
                    triggered_by: mux.triggered_by.clone(),
                    conflicts_with: mux.conflicts_with.clone(),
                    removes_upgrades: mux.removes_upgrades.clone(),
                    requires_all_triggers: mux.requires_all_triggers,
                    effect: UpgradeEffect::Stealth,
                })),
                // `RadarUpgrade` (RadarUpgrade.cpp:128-143): the player's radar count and this
                // object's `RadarUpdate::extendRadar`.
                LogicModule::RadarUpgrade { disable_proof, mux } => Some(Arc::new(SimUpgradeModule {
                    triggered_by: mux.triggered_by.clone(),
                    conflicts_with: mux.conflicts_with.clone(),
                    removes_upgrades: mux.removes_upgrades.clone(),
                    requires_all_triggers: mux.requires_all_triggers,
                    effect: UpgradeEffect::Radar { disable_proof: *disable_proof },
                })),
                // `GrantScienceUpgrade` (GrantScienceUpgrade.cpp:85-99).
                LogicModule::GrantScience { grant_science, mux } => Some(Arc::new(SimUpgradeModule {
                    triggered_by: mux.triggered_by.clone(),
                    conflicts_with: mux.conflicts_with.clone(),
                    removes_upgrades: mux.removes_upgrades.clone(),
                    requires_all_triggers: mux.requires_all_triggers,
                    effect: UpgradeEffect::GrantScience { science: grant_science.clone() },
                })),
                // `CostModifierUpgrade` (CostModifierUpgrade.cpp:166-173): the entry
                // `ThingTemplate::calcCostToBuild` multiplies by kind-of.
                LogicModule::CostModifier { effect_kind_of, percentage, mux } => Some(Arc::new(SimUpgradeModule {
                    triggered_by: mux.triggered_by.clone(),
                    conflicts_with: mux.conflicts_with.clone(),
                    removes_upgrades: mux.removes_upgrades.clone(),
                    requires_all_triggers: mux.requires_all_triggers,
                    effect: UpgradeEffect::CostModifier { kind_of: kind_of_mask_bits(effect_kind_of), percentage: *percentage },
                })),
                // `PassengersFireUpgrade` (PassengersFireUpgrade.cpp:52-66).
                LogicModule::PassengersFireUpgrade { mux } => Some(Arc::new(SimUpgradeModule {
                    triggered_by: mux.triggered_by.clone(),
                    conflicts_with: mux.conflicts_with.clone(),
                    removes_upgrades: mux.removes_upgrades.clone(),
                    requires_all_triggers: mux.requires_all_triggers,
                    effect: UpgradeEffect::PassengersFire,
                })),
                // `ReplaceObjectUpgrade` (ReplaceObjectUpgrade.cpp:71-140): the replacement template
                // is resolved here, like an `ObjectCreationUpgrade`'s OCL.
                LogicModule::ReplaceObject { replace_object, mux } => Some(Arc::new(SimUpgradeModule {
                    triggered_by: mux.triggered_by.clone(),
                    conflicts_with: mux.conflicts_with.clone(),
                    removes_upgrades: mux.removes_upgrades.clone(),
                    requires_all_triggers: mux.requires_all_triggers,
                    effect: UpgradeEffect::ReplaceObject {
                        template: if replace_object.is_empty() { None } else { cache.thing(all, replace_object) },
                    },
                })),
                // `SubObjectsUpgrade` (SubObjectsUpgrade.cpp:85-130): client-only show/hide; the sim
                // carries the module so `affectedByUpgrade` sees the upgrade.
                LogicModule::SubObjects { mux, .. } => Some(Arc::new(SimUpgradeModule {
                    triggered_by: mux.triggered_by.clone(),
                    conflicts_with: mux.conflicts_with.clone(),
                    removes_upgrades: mux.removes_upgrades.clone(),
                    requires_all_triggers: mux.requires_all_triggers,
                    effect: UpgradeEffect::SubObjects,
                })),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// `PhysicsBehavior` -> the sim's [`PhysicsTemplate`] (`PhysicsBehaviorModuleData`).
fn convert_physics(t: &ThingTemplate) -> Option<PhysicsTemplate> {
    let p = t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Physics(p) => Some(p),
        _ => None,
    })?;
    Some(convert_physics_data(p))
}

/// `PhysicsBehaviorModuleData` -> [`PhysicsTemplate`].
fn convert_physics_data(p: &zr_templates::module::PhysicsData) -> PhysicsTemplate {
    PhysicsTemplate {
        mass: p.mass,
        shock_resistance: p.shock_resistance,
        shock_max_yaw: p.shock_max_yaw,
        shock_max_pitch: p.shock_max_pitch,
        shock_max_roll: p.shock_max_roll,
        forward_friction: p.forward_friction,
        lateral_friction: p.lateral_friction,
        z_friction: p.z_friction,
        aerodynamic_friction: p.aerodynamic_friction,
        center_of_mass_offset: p.center_of_mass_offset,
        allow_bouncing: p.allow_bouncing,
        allow_collide_force: p.allow_collide_force,
        kill_when_resting_on_ground: p.kill_when_resting_on_ground,
        min_fall_speed_for_damage: p.min_fall_speed_for_damage,
        fall_height_damage_factor: p.fall_height_damage_factor,
        pitch_roll_yaw_factor: p.pitch_roll_yaw_factor,
    }
}

/// `RailroadBehavior` -> the sim's train data: `IsLocomotive`, the `CarriageTemplateName` list, the
/// conductor's speed parameters, the collision constants, and the `PhysicsBehaviorModuleData` half the
/// block also carries (`RailroadGuideAIUpdate.h:43-51`).
fn convert_railroad(t: &ThingTemplate) -> Option<zr_sim::template::Railroad> {
    t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Logic(l) => match l.as_ref() {
            LogicModule::Railroad {
                physics,
                is_locomotive,
                carriage_template_names,
                speed_max,
                acceleration,
                braking,
                friction,
                wait_at_station_time,
                running_garrison_speed_max,
                kill_speed_min,
                ..
            } => Some(zr_sim::template::Railroad {
                is_locomotive: *is_locomotive,
                carriage_templates: carriage_template_names.clone(),
                speed_max: *speed_max,
                acceleration: *acceleration,
                braking: *braking,
                friction: *friction,
                wait_at_station_time: *wait_at_station_time,
                running_garrison_speed_max: *running_garrison_speed_max,
                kill_speed_min: *kill_speed_min,
                physics: convert_physics_data(physics),
            }),
            _ => None,
        },
        _ => None,
    })
}

/// `HijackerUpdate` -> the sim's hijacker template, resolving the parachute container.
fn convert_hijacker(all: &Templates, t: &ThingTemplate, cache: &mut CombatCache) -> Option<zr_sim::template::HijackerTemplate> {
    t.modules.iter().find_map(|m| match &m.data {
        ModuleData::Logic(l) => match l.as_ref() {
            LogicModule::Hijacker { attach_to_target_bone, parachute_name } => Some(zr_sim::template::HijackerTemplate {
                attach_to_bone: attach_to_target_bone.clone(),
                parachute: if parachute_name.is_empty() || parachute_name.eq_ignore_ascii_case("None") {
                    None
                } else {
                    cache.thing(all, parachute_name)
                },
            }),
            _ => None,
        },
        _ => None,
    })
}

/// `SpecialPower` templates -> the sim store (`SpecialPowerStore`).
pub fn special_power_store(t: &Templates) -> BTreeMap<String, Arc<SimSpecialPowerTemplate>> {
    t.special_powers
        .iter()
        .map(|p| {
            (
                p.name.clone(),
                Arc::new(SimSpecialPowerTemplate {
                    name: p.name.clone(),
                    kind: p.kind.clone(),
                    reload_time: p.reload_time,
                    required_science: p.required_science.clone(),
                    shared_synced_timer: p.shared_synced_timer,
                    public_timer: p.public_timer,
                    radius_cursor_radius: p.radius_cursor_radius,
                }),
            )
        })
        .collect()
}

/// `Science` / `Rank` tables -> the sim copies.
pub fn science_tables(t: &Templates) -> (BTreeMap<String, zr_sim::science::ScienceInfo>, Vec<zr_sim::science::RankInfo>) {
    let sciences = t
        .sciences
        .iter()
        .map(|s| {
            (
                s.name.clone(),
                zr_sim::science::ScienceInfo {
                    science_purchase_point_cost: s.science_purchase_point_cost,
                    prerequisite_sciences: s.prerequisite_sciences.clone(),
                    grantable: s.grantable,
                },
            )
        })
        .collect();
    let ranks = t
        .ranks
        .iter()
        .map(|r| zr_sim::science::RankInfo {
            skill_points_needed: r.skill_points_needed,
            sciences_granted: r.sciences_granted.clone(),
            science_purchase_points_granted: r.science_purchase_points_granted,
        })
        .collect();
    (sciences, ranks)
}

/// `SpecialAbilityUpdate` modules of a template -> sim [`SpecialAbilityTemplate`]s.
fn convert_special_abilities(all: &Templates, t: &ThingTemplate, cache: &mut CombatCache) -> Vec<Arc<SpecialAbilityTemplate>> {
    t.modules
        .iter()
        .filter_map(|m| match &m.data {
            ModuleData::SpecialAbility(d) => Some(Arc::new(SpecialAbilityTemplate {
                power: d.special_power.clone(),
                start_ability_range: d.start_ability_range,
                ability_abort_range: d.ability_abort_range,
                approach_requires_los: d.approach_requires_los,
                preparation_frames: d.preparation_frames,
                pack_time: d.pack_time,
                unpack_time: d.unpack_time,
                pack_unpack_variation: d.pack_unpack_variation,
                skip_packing_with_no_target: d.skip_packing_with_no_target,
                award_xp_for_triggering: d.award_xp_for_triggering,
                skill_points_for_triggering: d.skill_points_for_triggering,
                effect_duration: d.effect_duration,
                effect_value: d.effect_value,
                special_object: d.special_object.as_deref().and_then(|n| cache.thing(all, n)),
                max_special_objects: d.max_special_objects,
                need_to_face: d.need_to_face,
                flee_range_after_completion: d.flee_range_after_completion,
                starts_paused: d.flags.starts_paused,
                power_module: d.power_module,
            })),
            _ => None,
        })
        .collect()
}

/// Special-power modules of a template -> sim [`SimSpecialPower`]s.
fn convert_special_powers(all: &Templates, t: &ThingTemplate, cache: &mut CombatCache) -> Vec<Arc<SimSpecialPower>> {
    let mut out: Vec<Arc<SimSpecialPower>> = t
        .modules
        .iter()
        .filter_map(|m| match &m.data {
            ModuleData::SpecialPower(d) => {
                let effect = match d.module_class.as_str() {
                    "OCLSpecialPower" => SpecialPowerEffect::Ocl {
                        ocl: d.ocl.as_deref().and_then(|n| all.ocl_id(n)).and_then(|id| cache.ocl(all, id)),
                        create_at_target: d.create_at_target,
                        // `OCLSpecialPower` `CreateLocation` (OCLSpecialPower.cpp:50-58); absent means CREATE_AT_EDGE_NEAR_SOURCE.
                        create_location: match d.create_location.to_ascii_uppercase().as_str() {
                            "CREATE_AT_EDGE_NEAR_TARGET" => zr_sim::ocl::CreateLocation::EdgeNearTarget,
                            "CREATE_AT_EDGE_FARTHEST_FROM_TARGET" => zr_sim::ocl::CreateLocation::EdgeFarthestFromTarget,
                            "CREATE_AT_LOCATION" | "CREATE_AT_TARGET" => zr_sim::ocl::CreateLocation::AtLocation,
                            "USE_OWNER_OBJECT" => zr_sim::ocl::CreateLocation::UseOwnerObject,
                            "CREATE_ABOVE_LOCATION" => zr_sim::ocl::CreateLocation::AboveLocation,
                            _ => zr_sim::ocl::CreateLocation::EdgeNearSource,
                        },
                    },
                    "FireWeaponPower" => SpecialPowerEffect::FireWeapon,
                    "CashHackSpecialPower" => SpecialPowerEffect::CashHack { amount: d.money_amount },
                    "CleanupAreaPower" => SpecialPowerEffect::CleanupArea { range: d.cleanup_move_range },
                    "BaikonurLaunchPower" => SpecialPowerEffect::Baikonur {
                        detonation_object: d.detonation_object.as_deref().and_then(|n| cache.thing(all, n)),
                    },
                    "SpyVisionSpecialPower" => SpecialPowerEffect::SpyVision {
                        base: d.spy_vision_base_duration,
                        bonus_per_captured: d.spy_vision_bonus_per_captured,
                        max: d.spy_vision_max_duration,
                    },
                    "CashBountyPower" => SpecialPowerEffect::CashBounty,
                    _ => return None,
                };
                Some(Arc::new(SimSpecialPower {
                    power: d.special_power.clone(),
                    effect,
                    starts_paused: d.flags.starts_paused,
                    power_module: true,
                }))
            }
            // The superweapon update modules are `SpecialPowerUpdateModule`s: their `special_power`
            // field names the power the command button triggers.
            ModuleData::Logic(l) => match l.as_ref() {
                // `SpecialPowerUpdateModule`s, not `SpecialPowerModule`s: no pause count.
                LogicModule::ParticleUplinkCannon { special_power, .. } => special_power.as_ref().map(|p| {
                    Arc::new(SimSpecialPower { power: p.clone(), effect: SpecialPowerEffect::ParticleUplink, starts_paused: false, power_module: false })
                }),
                LogicModule::SpectreGunshipDeployment { special_power, .. } => special_power.as_ref().map(|p| {
                    Arc::new(SimSpecialPower { power: p.clone(), effect: SpecialPowerEffect::SpectreDeployment, starts_paused: false, power_module: false })
                }),
                _ => None,
            },
            _ => None,
        })
        .collect();
    // `SpecialAbilityUpdate`-driven powers already have a `SimSpecialPower` when their module is
    // present; the superweapon modules above are the only extra entries.
    out.dedup_by(|a, b| a.power == b.power);
    out
}

/// `GameData` `WeaponBonus = CONDITION FIELD percent` -> the sim's global bonus set.
pub fn global_weapon_bonus(t: &Templates) -> zr_sim::bonus::WeaponBonusSet {
    zr_sim::bonus::WeaponBonusSet {
        entries: t.weapon_bonus.iter().map(|((cond, field), value)| (cond.index() as u8, field.index() as u8, *value)).collect(),
    }
}

/// `UpgradeTemplate::calcCostToBuild` / `calcTimeToBuild` per upgrade name.
pub fn upgrade_info(t: &Templates) -> BTreeMap<String, zr_sim::upgrade::UpgradeInfo> {
    t.upgrades()
        .iter()
        .map(|u| {
            (
                u.name.clone(),
                zr_sim::upgrade::UpgradeInfo { cost: u.build_cost.max(0) as u32, build_time_frames: (u.build_time * 30.0) as u32 },
            )
        })
        .collect()
}

pub fn convert_thing(all: &Templates, t: &ThingTemplate) -> ObjectTemplate {
    convert_thing_with(all, t, &mut CombatCache::default())
}

/// As [`convert_thing`], sharing weapon / armor `Arc`s through `cache`.
pub fn convert_thing_with(all: &Templates, t: &ThingTemplate, cache: &mut CombatCache) -> ObjectTemplate {
    // `ThingFactory::newObject` (`ThingFactory.cpp:315`): a `BuildVariations` blueprint is replaced
    // by one of its variation templates before the object is created (picked at random). Every
    // shipped blueprint (the GLA Technical chassis) is art-only, so the sim resolves the blueprint
    // to its first variation's gameplay while keeping the blueprint's own build metadata (cost /
    // time / prerequisites), which the sim reads from the same `ObjectTemplate` when queueing.
    if let Some(first) = t.build_variations.first() {
        if first != &t.name {
            if let Some(v) = cache.thing(all, first) {
                let mut ot = (*v).clone();
                ot.name = t.name.clone();
                ot.build_cost = i32::from(t.build_cost);
                ot.build_time_frames = (t.build_time * 30.0) as u32;
                ot.prerequisites = t.prerequisites.iter().map(convert_prerequisite).collect();
                return ot;
            }
        }
    }
    let g = &t.geometry;
    let (max_health, initial_health) = match t.body() {
        Some(b) if b.class != BodyClass::Inactive => (b.max_health, b.initial_health),
        _ => (0.0, 0.0),
    };
    // The sim's `die` list is the converted die modules in definition order. Each converted one keeps its module
    // position, so the `SlowDeath` entries of `update_module_order` can name their die index.
    let mut die_list = Vec::new();
    let mut die_of_module: Vec<(usize, usize)> = Vec::new();
    for (module_index, m) in t.modules.iter().enumerate() {
        if !is_die_module(&m.data) {
            continue;
        }
        if let Some(d) = convert_module_die(all, &m.data, cache) {
            die_of_module.push((module_index, die_list.len()));
            die_list.push(d);
        }
    }
    // `ActiveBodyModuleData` subdual fields (an inactive body has none).
    let (subdual_damage_cap, subdual_heal_rate, subdual_heal_amount) = match t.body() {
        Some(b) if b.class != BodyClass::Inactive => (b.subdual_damage_cap, b.subdual_damage_heal_rate, b.subdual_damage_heal_amount),
        _ => (0.0, 0, 0.0),
    };
    let locomotor_sets = convert_locomotor_sets(all, t);
    let locomotor = locomotor_sets.get(&LocomotorSet::Normal).copied();
    // `AIUpdateInterface` construction: one `Locomotor` per resolved `SET_NORMAL` entry (`LocomotorSet::addLocomotor`).
    let resolved = |ai: &zr_templates::AiUpdateData, set: LocomotorSetType| {
        ai.locomotor_sets.get(&set).map_or(0, |refs| refs.iter().filter(|r| r.id.is_some()).count()) as u8
    };
    let ai_update = t.ai().map(|ai| resolved(ai, LocomotorSetType::SetNormal));
    // The resolved entries of every set (one `Locomotor` each on a switch, AIUpdate.cpp:830-841).
    let locomotor_entries: BTreeMap<LocomotorSet, u8> = t
        .ai()
        .map(|ai| {
            ai.locomotor_sets
                .iter()
                .map(|(set, refs)| (sim_locomotor_set(*set), refs.iter().filter(|r| r.id.is_some()).count() as u8))
                .filter(|(_, n)| *n > 0)
                .collect()
        })
        .unwrap_or_default();
    // `Object::Object` builds the modules in definition order: the AI's locomotor draws come before the stealth detector's
    // unless the stealth module is listed first.
    let ai_pos = t.modules.iter().position(|m| matches!(m.data, ModuleData::Ai(_)));
    let stealth_pos = t.modules.iter().position(|m| matches!(m.data, ModuleData::StealthDetector(_)));
    let ai_before_stealth = match (ai_pos, stealth_pos) {
        (Some(a), Some(s)) => a < s,
        _ => true,
    };
    ObjectTemplate {
        name: t.name.clone(),
        kind_of: kind_of_bits(t),
        auto_rally_point: kind_of_has(&t.kind_of, "AUTO_RALLYPOINT"),
        geometry: Geometry {
            shape: match g.geometry_type {
                GeometryType::Sphere => GeometryShape::Sphere,
                GeometryType::Cylinder => GeometryShape::Cylinder,
                GeometryType::Box => GeometryShape::Box,
            },
            major_radius: g.major_radius,
            minor_radius: g.minor_radius,
            height: g.height,
            is_small: g.is_small,
        },
        max_health,
        initial_health,
        subdual_damage_cap,
        subdual_heal_rate,
        subdual_heal_amount,
        locomotor,
        locomotor_sets,
        locomotor_entries,
        ai_update,
        ai_before_stealth,
        vision_range: t.vision_range,
        shroud_clearing_range: t.shroud_clearing_range,
        shroud_reveal_to_all_range: t.shroud_reveal_to_all_range,
        combat: convert_combat(all, t, cache),
        die: die_list,
        update_module_order: sleepy_module_order(t, &die_of_module),
        radar_extend_time: t
            .modules
            .iter()
            .find_map(|m| match &m.data {
                ModuleData::Logic(l) => match l.as_ref() {
                    LogicModule::RadarUpdate { radar_extend_time } => Some(*radar_extend_time),
                    _ => None,
                },
                _ => None,
            })
            .unwrap_or(0.0),
        has_special_power_completion: t.modules.iter().any(|m| {
            matches!(&m.data, ModuleData::Logic(l) if matches!(l.as_ref(), LogicModule::SpecialPowerCompletionDie { .. }))
        }),
        build_cost: i32::from(t.build_cost),
        placement_view_angle: t.placement_view_angle,
        // `ThingTemplate::calcTimeToBuild`: `BuildTime` (seconds) * LOGICFRAMES_PER_SECOND, truncated.
        build_time_frames: (t.build_time * 30.0) as u32,
        prerequisites: t.prerequisites.iter().map(convert_prerequisite).collect(),
        production: convert_production(t, cache.spawn_bones.get(&t.name).map_or(&[], |b| b.as_slice())),
        contain: convert_contain(t, cache.fire_points.get(&t.name)).map(std::sync::Arc::new),
        rider_change: convert_rider_change(t),
        transport_slot_count: u32::from(t.transport_slot_count),
        energy_production: t.energy_production,
        energy_bonus: t.energy_bonus,
        dock: convert_dock(t),
        dock_bones: cache.dock_bones.get(&t.name).cloned().unwrap_or_default(),
        supply_truck: convert_supply_truck(t),
        chinook: convert_chinook(t),
        assault_transport: convert_assault_transport(t),
        deploy_style_ai: t.modules.iter().any(|m| m.name == "DeployStyleAIUpdate"),
        dozer: convert_dozer(t),
        hack_internet: convert_hack_internet(t),
        skill_point_value: t.skill_point_value,
        experience_value: t.experience_value,
        experience_required: t.experience_required,
        is_trainable: t.is_trainable,
        armor_sets: convert_armor_sets(all, t, cache),
        weapon_sets: convert_weapon_sets(all, t, cache),
        upgrade_modules: convert_upgrade_modules(all, t, cache),
        special_powers: convert_special_powers(all, t, cache),
        special_abilities: convert_special_abilities(all, t, cache),
        special_power_create: t
            .modules
            .iter()
            .any(|m| matches!(&m.data, ModuleData::Logic(l) if matches!(l.as_ref(), LogicModule::SpecialPowerCreate))),
        grant_upgrade_create: convert_grant_upgrade_create(all, t),
        physics: convert_physics(t),
        hijacker: convert_hijacker(all, t, cache),
        railroad: convert_railroad(t),
        is_bridge: t.is_bridge,
        fence_width: t.fence_width,
        fence_x_offset: t.fence_x_offset,
        defensive_wall: kind_of_has(&t.kind_of, "DEFENSIVE_WALL"),
        ballistic_missile: kind_of_has(&t.kind_of, "BALLISTIC_MISSILE"),
        demotrap: kind_of_has(&t.kind_of, "DEMOTRAP"),
        attack_needs_line_of_sight: kind_of_has(&t.kind_of, "ATTACK_NEEDS_LINE_OF_SIGHT"),
        aircraft_path_around: kind_of_has(&t.kind_of, "AIRCRAFT_PATH_AROUND"),
        ignored_in_gui: kind_of_has(&t.kind_of, "IGNORED_IN_GUI"),
        factory_exit_width: t.factory_exit_width,
        factory_extra_bib_width: t.factory_extra_bib_width,
        cleared_by_build: kind_of_has(&t.kind_of, "CLEARED_BY_BUILD"),
        always_selectable: kind_of_has(&t.kind_of, "ALWAYS_SELECTABLE"),
        collide_kinds: collide_kind_mask_bits(&t.kind_of),
        // Every AutoHeal module, in INI order: `Object::Object` builds one per module (lane 178).
        auto_heal: t.modules.iter().filter_map(|m| match &m.data {
            ModuleData::AutoHeal(d) => Some(zr_sim::behaviors::AutoHealTemplate {
                healing_amount: d.healing_amount,
                healing_delay: d.healing_delay,
                start_healing_delay: d.start_healing_delay,
                radius: d.radius,
                affects_whole_player: d.affects_whole_player,
                skip_self_for_healing: d.skip_self_for_healing,
                single_burst: d.single_burst,
                kind_of: kind_of_mask_bits(&d.kind_of),
                forbidden_kind_of: kind_of_mask_bits(&d.forbidden_kind_of),
                starts_active: d.starts_active,
                triggered_by: d.upgrade.triggered_by.clone(),
                conflicts_with: d.upgrade.conflicts_with.clone(),
            }),
            _ => None,
        }).collect(),
        flammable: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Flammable(d) => Some(zr_sim::behaviors::FlammableTemplate {
                burned_delay: d.burned_delay,
                aflame_duration: d.aflame_duration,
                aflame_damage_delay: d.aflame_damage_delay,
                aflame_damage_amount: d.aflame_damage_amount,
                flame_damage_limit: d.flame_damage_limit,
                flame_damage_expiration: d.flame_damage_expiration,
            }),
            _ => None,
        }),
        poisoned: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Poisoned(d) => Some(zr_sim::behaviors::PoisonedTemplate {
                poison_damage_interval: d.poison_damage_interval,
                poison_duration: d.poison_duration,
            }),
            _ => None,
        }),
        fire_weapon_when_dead: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::FireWeaponWhenDead(d) => d.death_weapon.as_ref().and_then(|r| r.id).map(|id| {
                zr_sim::behaviors::DeathWeaponTemplate {
                    weapon: std::sync::Arc::new(convert_weapon(all.weapon_by_id(id))),
                    death_types: d.die.death_types.bits() as u32,
                    starts_active: d.starts_active,
                    triggered_by: d.upgrade.triggered_by.clone(),
                    conflicts_with: d.upgrade.conflicts_with.clone(),
                }
            }),
            _ => None,
        }),
        lifetime: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Lifetime(d) => Some((d.min_lifetime, d.max_lifetime)),
            _ => None,
        }),
        // `DeletionUpdate`: a lifetime that *destroys* the object (no death pipeline).
        deletion: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Logic(l) => match &**l {
                LogicModule::Deletion { min_lifetime, max_lifetime } => Some((*min_lifetime, *max_lifetime)),
                _ => None,
            },
            _ => None,
        }),
        dynamic_shroud: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Logic(l) => match &**l {
                LogicModule::DynamicShroudClearingRange {
                    change_interval,
                    grow_interval,
                    shrink_delay,
                    shrink_time,
                    grow_delay,
                    grow_time,
                    final_vision,
                    ..
                } => Some(zr_sim::template::DynamicShroudTemplate {
                    change_interval: *change_interval,
                    grow_interval: *grow_interval,
                    shrink_delay: *shrink_delay,
                    shrink_time: *shrink_time,
                    grow_delay: *grow_delay,
                    grow_time: *grow_time,
                    final_vision: *final_vision,
                }),
                _ => None,
            },
            _ => None,
        }),
        stealth: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Stealth(d) => Some(zr_sim::behaviors::StealthTemplate {
                stealth_delay: d.stealth_delay,
                move_threshold_speed: d.move_threshold_speed,
                forbidden_conditions: d.forbidden_conditions,
                required_status: status_bits(d.required_status),
                forbidden_status: status_bits(d.forbidden_status),
                innate_stealth: d.innate_stealth,
                granted_by_special_power: d.granted_by_special_power,
                disguises_as_team: d.disguises_as_team,
                reveal_distance_from_target: d.reveal_distance_from_target,
                order_idle_enemies_to_attack_on_reveal: d.order_idle_enemies_to_attack_on_reveal,
                use_rider_stealth: d.use_rider_stealth,
            }),
            _ => None,
        }),
        stealth_detector: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::StealthDetector(d) => Some(zr_sim::behaviors::StealthDetectorTemplate {
                detection_rate: d.detection_rate,
                detection_range: d.detection_range,
                initially_disabled: d.initially_disabled,
                extra_required_kind_of: kind_of_mask_bits(&d.extra_required_kind_of),
                extra_forbidden_kind_of: kind_of_mask_bits(&d.extra_forbidden_kind_of),
                can_detect_while_garrisoned: d.can_detect_while_garrisoned,
                can_detect_while_contained: d.can_detect_while_contained,
            }),
            _ => None,
        }),
        crush: zr_sim::modules2::CrushInfo {
            crusher_level: t.crusher_level,
            crushable_level: t.crushable_level,
            squish_collide: t.modules.iter().any(|m| matches!(m.data, ModuleData::SquishCollide(_))),
        },
        topple: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Topple(d) => Some(zr_sim::modules2::ToppleTemplate {
                kill_when_start_toppling: d.kill_when_start_toppling,
                kill_when_finished_toppling: d.kill_when_finished_toppling,
                topple_left_or_right_only: d.topple_left_or_right_only,
            }),
            _ => None,
        }),
        auto_deposit: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::AutoDeposit(d) => Some(zr_sim::modules2::AutoDepositTemplate {
                deposit_timing: d.deposit_timing,
                deposit_amount: d.deposit_amount,
                initial_capture_bonus: d.initial_capture_bonus,
                upgraded_boost: d.upgraded_boost.clone(),
            }),
            _ => None,
        }),
        fire_weapon: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::FireWeapon(d) => d.weapon.as_ref().and_then(|r| r.id).map(|id| zr_sim::modules2::FireWeaponTemplate {
                weapon: std::sync::Arc::new(convert_weapon(all.weapon_by_id(id))),
                initial_delay: d.initial_delay,
                exclusive_weapon_delay: d.exclusive_weapon_delay,
            }),
            _ => None,
        }),
        horde: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Horde(d) => Some(zr_sim::modules2::HordeTemplate {
                update_rate: d.update_rate,
                kind_of: kind_of_mask_bits(&d.kind_of),
                count: d.count,
                radius: d.radius,
                rub_off_radius: d.rub_off_radius,
                allies_only: d.allies_only,
                exact_match: d.exact_match,
                allowed_nationalism: d.allowed_nationalism,
            }),
            _ => None,
        }),
        base_regenerate: t.modules.iter().any(|m| matches!(m.data, ModuleData::BaseRegenerate(_))),
        spawn: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Spawn(d) => Some(zr_sim::modules2::SpawnTemplate {
                spawn_number: d.spawn_number,
                spawn_replace_delay: d.spawn_replace_delay,
                one_shot: d.one_shot,
                spawn_template_names: d.spawn_template_names.clone(),
                spawned_require_spawner: d.spawned_require_spawner,
                initial_burst: d.initial_burst,
                aggregate_health: d.aggregate_health,
            }),
            _ => None,
        }),
        spawn_die_mux: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Spawn(d) => Some(convert_mux(&d.die)),
            _ => None,
        }),
        slaved: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Slaved(d) => Some(zr_sim::modules2::SlavedTemplate {
                guard_max_range: d.guard_max_range,
                guard_wander_range: d.guard_wander_range,
                attack_range: d.attack_range,
                repair_rate_per_second: d.repair_rate_per_second,
                stay_on_same_layer_as_master: d.stay_on_same_layer_as_master,
            }),
            _ => None,
        }),
        crate_collides: t
            .modules
            .iter()
            .filter_map(|m| match &m.data {
                ModuleData::CrateCollide(d) => Some(zr_sim::modules2::CrateCollideTemplate {
                    required_kind_of: kind_of_mask_bits(&d.required_kind_of),
                    forbidden_kind_of: kind_of_mask_bits(&d.forbidden_kind_of),
                    forbid_owner_player: d.forbid_owner_player,
                    building_pickup: d.building_pickup,
                    human_only: d.human_only,
                    allow_multi_pickup: d.allow_multi_pickup,
                    pickup_science: d.pickup_science.clone(),
                    effect: convert_crate_effect(&d.kind),
                }),
                _ => None,
            })
            .collect(),
        fire_weapon_collide: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::FireWeaponCollide(d) => d.weapon.as_ref().and_then(|r| r.id).map(|id| zr_sim::modules2::FireWeaponCollideTemplate {
                weapon: std::sync::Arc::new(convert_weapon(all.weapon_by_id(id))),
                fire_once: d.fire_once,
                required_status: status_bits(d.required_status),
                forbidden_status: status_bits(d.forbidden_status),
            }),
            _ => None,
        }),
        create_crate: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Die(d) => match &d.kind {
                DieKind::CreateCrate(c) => Some(c.crate_names.clone()),
                _ => None,
            },
            _ => None,
        }),
        eject_pilot: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Die(d) => match &d.kind {
                DieKind::EjectPilot(e) => Some(zr_sim::modules2::EjectPilotTemplate {
                    air_ocl: e.air_creation_list.as_ref().and_then(|r| r.id).and_then(|id| cache.ocl(all, id)),
                    ground_ocl: e.ground_creation_list.as_ref().and_then(|r| r.id).and_then(|id| cache.ocl(all, id)),
                }),
                _ => None,
            },
            _ => None,
        }),
        veterancy_gain: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::VeterancyGainCreate(d) => Some(zr_sim::modules2::VeterancyGainTemplate {
                starting_level: d.starting_level.index() as u8,
                science_required: d.science_required.clone(),
            }),
            _ => None,
        }),
        propaganda: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Propaganda(d) => Some(zr_sim::modules2::PropagandaTemplate {
                radius: d.radius,
                delay_between_updates: d.delay_between_updates,
                heal_percent_each_second: d.heal_percent_each_second,
                upgraded_heal_percent_each_second: d.upgraded_heal_percent_each_second,
                upgrade_required: d.upgrade_required.clone(),
                affects_self: d.affects_self,
            }),
            _ => None,
        }),
        repair_dock: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::RepairDock(d) => Some(zr_sim::modules2::RepairDockTemplate {
                frames_for_full_heal: d.frames_for_full_heal,
                number_approach_positions: d.number_approach_positions,
                allows_passthrough: d.allows_passthrough,
            }),
            _ => None,
        }),
        // `OverchargeBehavior` is the toggle/health-drain module; `PowerPlantUpdate` (the rods) is a separate module
        // and heap entry. `OverchargeBehavior::enable` drives the rods through the `PowerPlantUpdateInterface`, so its
        // data carries the object's rod timer too (`PowerPlantUpdateModuleData::m_rodsExtendTime`).
        overcharge: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Overcharge(d) if m.name == "OverchargeBehavior" => Some(zr_sim::modules2::OverchargeTemplate {
                health_percent_to_drain_per_second: d.health_percent_to_drain_per_second,
                not_allowed_when_health_below_percent: d.not_allowed_when_health_below_percent,
                rods_extend_time: t
                    .modules
                    .iter()
                    .find(|p| p.name == "PowerPlantUpdate")
                    .and_then(|p| match &p.data {
                        ModuleData::Overcharge(p) => Some(p.rods_extend_time),
                        _ => None,
                    })
                    .unwrap_or(0),
            }),
            _ => None,
        }),
        power_plant: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Overcharge(d) if m.name == "PowerPlantUpdate" => {
                Some(zr_sim::modules2::PowerPlantTemplate { rods_extend_time: d.rods_extend_time })
            }
            _ => None,
        }),
        tech_building: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Logic(l) => match &**l {
                LogicModule::TechBuilding { pulse_fx, pulse_fx_rate } => Some(zr_sim::template::TechBuildingTemplate {
                    pulse_fx: pulse_fx.clone(),
                    pulse_fx_rate: *pulse_fx_rate,
                }),
                _ => None,
            },
            _ => None,
        }),
        auto_find_healing: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Logic(l) => match &**l {
                LogicModule::AutoFindHealing { scan_rate, scan_range, never_heal, always_heal } => {
                    Some(zr_sim::template::AutoFindHealingTemplate {
                        scan_frames: *scan_rate,
                        scan_range: *scan_range,
                        never_heal: *never_heal,
                        always_heal: *always_heal,
                    })
                }
                _ => None,
            },
            _ => None,
        }),
        prone: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::SimpleUpdate(d) if d.class == "ProneUpdate" => {
                Some(zr_sim::modules2::ProneTemplate { damage_to_frames_ratio: d.damage_to_frames_ratio })
            }
            _ => None,
        }),
        // `AssistedTargetingUpdate` (AssistedTargetingUpdate.cpp:50-76): the first module, as `findUpdateModule` returns.
        assisted_targeting: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Logic(l) => match &**l {
                LogicModule::AssistedTargeting { assisting_clip_size, assisting_weapon_slot, laser_from_assisted, laser_to_target } => {
                    Some(zr_sim::assist::AssistedTargetingTemplate {
                        clip_size: *assisting_clip_size,
                        weapon_slot: assisting_weapon_slot.index(),
                        laser_from_assisted: thing_laser_spec(all, laser_from_assisted, ""),
                        laser_to_target: thing_laser_spec(all, laser_to_target, ""),
                    })
                }
                _ => None,
            },
            _ => None,
        }),
        countermeasures: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Countermeasures(d) => Some(zr_sim::modules2::CountermeasuresTemplate {
                flare_template_name: d.flare_template_name.clone(),
                evasion_rate: d.evasion_rate,
                volley_size: d.volley_size,
                volley_arc_angle: d.volley_arc_angle,
                volley_velocity_factor: d.volley_velocity_factor,
                number_of_volleys: d.number_of_volleys,
                delay_between_volleys: d.delay_between_volleys,
                reload_time: d.reload_time,
                must_reload_at_airfield: d.must_reload_at_airfield,
                missile_decoy_delay: d.missile_decoy_delay,
                countermeasure_reaction_frames: d.countermeasure_reaction_frames,
                starts_active: d.starts_active,
                triggered_by: d.triggered_by.clone(),
                conflicts_with: d.conflicts_with.clone(),
                // The flare template's `LifetimeUpdate` (frames).
                flare_lifetime_frames: d
                    .flare_template_name
                    .as_deref()
                    .and_then(|n| all.thing(n))
                    .and_then(|f| {
                        f.modules.iter().find_map(|m| match &m.data {
                            ModuleData::Lifetime(l) => Some(l.max_lifetime),
                            _ => None,
                        })
                    })
                    .unwrap_or(90),
            }),
            _ => None,
        }),
        jet: t.ai().and_then(|ai| ai.jet.as_ref()).map(|j| zr_sim::modules2::JetAiTemplate {
            needs_runway: j.needs_runway,
            keeps_parking_space_when_airborne: j.keeps_parking_space_when_airborne,
            takeoff_pause: j.takeoff_pause,
            return_to_base_idle_time: j.return_to_base_idle_time,
            out_of_ammo_damage_per_second: j.out_of_ammo_damage_per_second,
            takeoff_dist_for_max_lift: j.takeoff_dist_for_max_lift,
            min_height: j.min_height,
            parking_offset: j.parking_offset,
            sneaky_offset_when_attacking: j.sneaky_offset_when_attacking,
            attacking_loco: sim_locomotor_set(j.attacking_loco),
            returning_loco: sim_locomotor_set(j.returning_loco),
            attack_loco_persist_time: j.attack_loco_persist_time,
            attackers_miss_persist_time: j.attackers_miss_persist_time,
        }),
        parking_place: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::ParkingPlace(d) => Some(zr_sim::modules2::ParkingPlaceTemplate {
                num_rows: d.num_rows,
                num_cols: d.num_cols,
                has_runways: d.has_runways,
                heal_amount_per_second: d.heal_amount_per_second,
                approach_height: d.approach_height,
                landing_deck_height_offset: d.landing_deck_height_offset,
                park_in_hangars: d.park_in_hangars,
            }),
            _ => None,
        }),
        parking_bones: cache.parking_bones.get(&t.name).cloned().unwrap_or_default(),
        transition_damage: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::TransitionDamage(d) => Some(zr_sim::modules2::TransitionDamageTemplate {
                damaged_fx: d.damaged_fx.clone(),
                really_damaged_fx: d.really_damaged_fx.clone(),
                rubble_fx: d.rubble_fx.clone(),
            }),
            _ => None,
        }),
        keep_object_die: t.modules.iter().any(|m| matches!(&m.data, ModuleData::Die(d) if matches!(d.kind, DieKind::Keep))),
        minefield: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Minefield(d) => Some(zr_sim::modules2::MinefieldTemplate {
                detonation_weapon: d.detonation_weapon.as_ref().and_then(|r| r.id).map(|id| std::sync::Arc::new(convert_weapon(all.weapon_by_id(id)))),
                regenerates: d.regenerates,
                workers_detonate: d.workers_detonate,
                num_virtual_mines: d.num_virtual_mines,
            }),
            _ => None,
        }),
        rebuild_hole: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::RebuildHole(d) => Some(zr_sim::modules4::RebuildHoleTemplate {
                worker_object_name: d.worker_object_name.clone(),
                worker_respawn_delay: d.worker_respawn_delay,
                hole_health_regen_percent_per_second: d.hole_health_regen_percent_per_second,
            }),
            _ => None,
        }),
        demo_trap: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::SimpleUpdate(d) if d.class == "DemoTrapUpdate" => Some(zr_sim::modules4::DemoTrapTemplate {
                detonation_weapon: d.detonation_weapon.as_ref().and_then(|r| r.id).map(|id| std::sync::Arc::new(convert_weapon(all.weapon_by_id(id)))),
                trigger_detonation_range: d.trigger_detonation_range,
                scan_frames: d.scan_rate,
                ignore_kind_of: kind_of_mask_bits(&d.ignore_kind_of),
                friendly_detonation: d.friendly_detonation,
                detonate_when_killed: d.detonate_when_killed,
                default_proximity_mode: d.default_proximity_mode,
            }),
            _ => None,
        }),
        cleanup_hazard: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::SimpleUpdate(d) if d.class == "CleanupHazardUpdate" => {
                Some(zr_sim::modules4::CleanupHazardTemplate { scan_frames: d.scan_rate, scan_range: d.scan_range })
            }
            _ => None,
        }),
        command_button_hunt: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::SimpleUpdate(d) if d.class == "CommandButtonHuntUpdate" => {
                Some(zr_sim::modules4::CommandButtonHuntTemplate { scan_frames: d.scan_rate, scan_range: d.scan_range })
            }
            _ => None,
        }),
        generate_minefield: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::GenerateMinefield(d) => Some(zr_sim::modules4::GenerateMinefieldTemplate {
                mine_name: d.mine_name.clone(),
                upgraded_mine_name: d.upgraded_mine_name.clone(),
                upgraded_trigger: d.upgraded_triggered_by.clone(),
                distance_around_object: d.distance_around_object,
                mines_per_square_foot: d.mines_per_square_foot,
                generate_only_on_death: d.generate_only_on_death,
                border_only: d.border_only,
                smart_border: d.smart_border,
                smart_border_skip_interior: d.smart_border_skip_interior,
                always_circular: d.always_circular,
                upgradable: d.upgradable,
                random_jitter: d.random_jitter,
                // `zr-templates` does not type `SkipIfThisMuchUnderStructure`; no shipped INI sets
                // it, so the engine's 0.33 default applies (`GenerateMinefieldBehaviorModuleData`).
                skip_if_this_much_under_structure: 0.33,
                starts_active: false,
                triggered_by: d.triggered_by.clone(),
                conflicts_with: d.conflicts_with.clone(),
            }),
            _ => None,
        }),
        height_die: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Logic(l) => match l.as_ref() {
                LogicModule::HeightDie {
                    target_height,
                    target_height_includes_structures,
                    only_when_moving_down,
                    destroy_attached_particles_at_height,
                    snap_to_ground_on_death,
                    initial_delay,
                } => Some(zr_sim::superweapon::HeightDieTemplate {
                    target_height: *target_height,
                    target_height_includes_structures: *target_height_includes_structures,
                    only_when_moving_down: *only_when_moving_down,
                    destroy_attached_particles_at_height: *destroy_attached_particles_at_height,
                    snap_to_ground_on_death: *snap_to_ground_on_death,
                    initial_delay: *initial_delay,
                }),
                _ => None,
            },
            _ => None,
        }),
        neutron_missile: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Logic(l) => match l.as_ref() {
                LogicModule::NeutronMissile {
                    distance_to_travel_before_turning,
                    max_turn_rate,
                    forward_damping,
                    relative_speed,
                    target_from_directly_above,
                    launch_fx,
                    special_speed_time,
                    special_speed_height,
                    special_accel_factor,
                    special_jitter_distance,
                    ignition_fx,
                    ..
                } => Some(zr_sim::superweapon::NeutronMissileTemplate {
                    distance_to_travel_before_turning: *distance_to_travel_before_turning,
                    max_turn_rate: *max_turn_rate,
                    forward_damping: *forward_damping,
                    relative_speed: *relative_speed,
                    target_from_directly_above: *target_from_directly_above,
                    special_speed_time: *special_speed_time,
                    special_speed_height: *special_speed_height,
                    special_accel_factor: *special_accel_factor,
                    special_jitter_distance: *special_jitter_distance,
                    launch_fx: launch_fx.clone(),
                    ignition_fx: ignition_fx.clone(),
                }),
                _ => None,
            },
            _ => None,
        }),
        spy_vision: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Logic(l) => match l.as_ref() {
                LogicModule::SpyVisionUpdate { needs_upgrade, self_powered, self_powered_duration, self_powered_interval, spy_on_kind_of, .. } => {
                    Some(zr_sim::superweapon::SpyVisionTemplate {
                        needs_upgrade: *needs_upgrade,
                        self_powered: *self_powered,
                        self_powered_duration: *self_powered_duration,
                        self_powered_interval: *self_powered_interval,
                        spy_on_kind_of: kind_of_mask_bits(spy_on_kind_of),
                    })
                }
                _ => None,
            },
            _ => None,
        }),
        particle_uplink: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Logic(l) => match l.as_ref() {
                LogicModule::ParticleUplinkCannon {
                    special_power,
                    begin_charge_frames,
                    raise_antenna_frames,
                    ready_delay_frames,
                    width_grow_frames,
                    beam_travel_frames,
                    total_firing_frames,
                    reveal_range,
                    swath_of_death_distance,
                    swath_of_death_amplitude,
                    total_scorch_marks,
                    scorch_mark_scalar,
                    total_damage_pulses,
                    damage_per_second,
                    damage_radius_scalar,
                    damage_type,
                    death_type,
                    damage_pulse_remnant_object_name,
                    manual_driving_speed,
                    manual_fast_driving_speed,
                    double_click_to_fast_drive_delay,
                    ground_hit_fx,
                    beam_launch_fx,
                    delay_between_launch_fx,
                    ..
                } => Some(zr_sim::superweapon::ParticleUplinkTemplate {
                    special_power: special_power.clone(),
                    begin_charge_frames: *begin_charge_frames,
                    raise_antenna_frames: *raise_antenna_frames,
                    ready_delay_frames: *ready_delay_frames,
                    width_grow_frames: *width_grow_frames,
                    beam_travel_frames: *beam_travel_frames,
                    total_firing_frames: *total_firing_frames,
                    reveal_range: *reveal_range,
                    swath_of_death_distance: *swath_of_death_distance,
                    swath_of_death_amplitude: *swath_of_death_amplitude,
                    total_scorch_marks: *total_scorch_marks,
                    scorch_mark_scalar: *scorch_mark_scalar,
                    // `getLaserTemplateWidth()`: `ParticleUplinkCannon_OrbitalLaser`'s
                    // `OuterBeamWidth 26.0 * 0.5`; the module's own fallback is 13.0.
                    beam_radius: 13.0,
                    total_damage_pulses: *total_damage_pulses,
                    damage_per_second: *damage_per_second,
                    damage_radius_scalar: *damage_radius_scalar,
                    damage_type: damage_type
                        .iter()
                        .next()
                        .map(|d| SimDamageType::from_index(d.index()))
                        .unwrap_or(SimDamageType::Explosion),
                    death_type: zr_templates::names::DeathType::from_name(death_type)
                        .map(|d| SimDeathType::from_index(d.index()))
                        .unwrap_or(SimDeathType::Normal),
                    remnant_object: (!damage_pulse_remnant_object_name.is_empty()).then(|| damage_pulse_remnant_object_name.clone()),
                    manual_driving_speed: *manual_driving_speed,
                    manual_fast_driving_speed: *manual_fast_driving_speed,
                    double_click_to_fast_drive_delay: *double_click_to_fast_drive_delay,
                    ground_hit_fx: ground_hit_fx.clone(),
                    beam_launch_fx: beam_launch_fx.clone(),
                    delay_between_launch_fx: *delay_between_launch_fx,
                }),
                _ => None,
            },
            _ => None,
        }),
        spectre_deployment: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Logic(l) => match l.as_ref() {
                LogicModule::SpectreGunshipDeployment { gunship_template_name, special_power, attack_area_radius, create_location, .. } => {
                    Some(zr_sim::superweapon::SpectreDeploymentTemplate {
                        gunship: (!gunship_template_name.is_empty())
                            .then(|| cache.thing(all, gunship_template_name))
                            .flatten(),
                        attack_area_radius: *attack_area_radius,
                        create_location: match create_location.as_str() {
                            "CREATE_AT_EDGE_NEAR_SOURCE" => 0,
                            "CREATE_AT_EDGE_FARTHEST_FROM_SOURCE" => 1,
                            "CREATE_AT_EDGE_NEAR_TARGET" => 2,
                            _ => 3,
                        },
                        // The shipped deployment INI never sets `GunshipOrbitRadius` (the module's
                        // `buildFieldParse` has no entry for it), so the spawn point sits on the map
                        // edge; the gunship's own `GunshipOrbitRadius` drives the real orbit.
                        gunship_orbit_radius: 0.0,
                        special_power: special_power.clone(),
                    })
                }
                _ => None,
            },
            _ => None,
        }),
        spectre_gunship: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Logic(l) => match l.as_ref() {
                LogicModule::SpectreGunship {
                    special_power,
                    gattling_template_name,
                    howitzer_firing_rate,
                    orbit_time,
                    howitzer_follow_lag,
                    attack_area_radius,
                    strafing_increment,
                    orbit_insertion_slope,
                    random_offset_for_howitzer,
                    targeting_reticle_radius,
                    gunship_orbit_radius,
                    howitzer_weapon_template,
                    ..
                } => Some(zr_sim::superweapon::SpectreGunshipTemplate {
                    special_power: special_power.clone(),
                    howitzer_firing_rate: *howitzer_firing_rate,
                    howitzer_follow_lag: *howitzer_follow_lag,
                    orbit_time: *orbit_time,
                    attack_area_radius: *attack_area_radius,
                    targeting_reticle_radius: *targeting_reticle_radius,
                    orbit_radius: *gunship_orbit_radius,
                    strafing_increment: *strafing_increment,
                    orbit_insertion_slope: *orbit_insertion_slope,
                    random_offset_for_howitzer: *random_offset_for_howitzer,
                    howitzer_weapon: howitzer_weapon_template.as_ref().and_then(|n| all.weapon_id(n)).map(|id| cache.weapon(all, id)),
                    gattling_template_name: (!gattling_template_name.is_empty()).then(|| gattling_template_name.clone()),
                }),
                _ => None,
            },
            _ => None,
        }),
        missile_launcher: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::Logic(l) => match l.as_ref() {
                LogicModule::MissileLauncherBuilding { special_power, door_open_time, door_wait_open_time, door_close_time, .. } => {
                    Some(zr_sim::superweapon::MissileLauncherBuildingTemplate {
                        special_power: special_power.clone(),
                        door_open_frames: *door_open_time,
                        door_wait_open_frames: *door_wait_open_time,
                        door_closing_frames: *door_close_time,
                    })
                }
                _ => None,
            },
            _ => None,
        }),
        cash_bounty: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::SpecialPower(d) if d.module_class == "CashBountyPower" => Some(d.bounty),
            _ => None,
        }),
        cash_bounty_power: t.modules.iter().find_map(|m| match &m.data {
            ModuleData::SpecialPower(d) if d.module_class == "CashBountyPower" => Some(d.special_power.clone()),
            _ => None,
        }),
    }
}

/// `CrateCollide` subclass -> the sim's pickup effect.
fn convert_crate_effect(kind: &zr_templates::modules2::CrateKind) -> zr_sim::modules2::CrateEffect {
    use zr_sim::modules2::CrateEffect;
    match kind {
        zr_templates::modules2::CrateKind::Money { money_provided, upgrade_boost } => {
            CrateEffect::Money { money_provided: *money_provided, upgrade_boost: upgrade_boost.clone() }
        }
        zr_templates::modules2::CrateKind::Heal => CrateEffect::Heal,
        zr_templates::modules2::CrateKind::Veterancy { effect_range, adds_owner_veterancy, is_pilot } => {
            CrateEffect::Veterancy { effect_range: *effect_range, adds_owner_veterancy: *adds_owner_veterancy, is_pilot: *is_pilot }
        }
        zr_templates::modules2::CrateKind::Unit { unit_count, unit_name } => {
            CrateEffect::Unit { unit_count: *unit_count, unit_name: unit_name.clone() }
        }
        zr_templates::modules2::CrateKind::Salvage { weapon_chance, level_chance, money_chance, min_money, max_money } => {
            CrateEffect::Salvage {
                weapon_chance: *weapon_chance,
                level_chance: *level_chance,
                money_chance: *money_chance,
                min_money: *min_money,
                max_money: *max_money,
            }
        }
        zr_templates::modules2::CrateKind::Shroud => CrateEffect::Shroud,
        zr_templates::modules2::CrateKind::ConvertToCarBomb => CrateEffect::ConvertToCarBomb,
        zr_templates::modules2::CrateKind::ConvertToHijackedVehicle => CrateEffect::ConvertToHijackedVehicle,
        zr_templates::modules2::CrateKind::SabotageCommandCenter => CrateEffect::SabotageCommandCenter,
        zr_templates::modules2::CrateKind::SabotageFakeBuilding => CrateEffect::SabotageFakeBuilding,
        zr_templates::modules2::CrateKind::SabotageInternetCenter { sabotage_frames } => {
            CrateEffect::SabotageInternetCenter { sabotage_frames: *sabotage_frames }
        }
        zr_templates::modules2::CrateKind::SabotageMilitaryFactory { sabotage_frames } => {
            CrateEffect::SabotageMilitaryFactory { sabotage_frames: *sabotage_frames }
        }
        zr_templates::modules2::CrateKind::SabotagePowerPlant { power_sabotage_frames } => {
            CrateEffect::SabotagePowerPlant { power_sabotage_frames: *power_sabotage_frames }
        }
        zr_templates::modules2::CrateKind::SabotageSuperweapon => CrateEffect::SabotageSuperweapon,
        zr_templates::modules2::CrateKind::SabotageSupplyCenter { steal_cash_amount } => {
            CrateEffect::SabotageSupplyCenter { steal_cash_amount: *steal_cash_amount }
        }
        zr_templates::modules2::CrateKind::SabotageSupplyDropzone { steal_cash_amount } => {
            CrateEffect::SabotageSupplyDropzone { steal_cash_amount: *steal_cash_amount }
        }
        zr_templates::modules2::CrateKind::Other => CrateEffect::Other,
    }
}

/// [`TemplateSource`] over the typed template database, caching one `Arc` per template.
/// Lookup is case-sensitive, like `ThingFactory::findTemplate`.
pub struct TemplateStore {
    templates: Templates,
    cache: Mutex<BTreeMap<String, Arc<ObjectTemplate>>>,
    combat_cache: Mutex<CombatCache>,
    /// `CommandSet` name -> the things its `UNIT_BUILD` / `DOZER_CONSTRUCT` buttons make.
    unit_builds: Mutex<BTreeMap<String, std::collections::BTreeSet<String>>>,
    /// `CommandSet` name -> the lowercase upgrade names its `PLAYER_UPGRADE` / `OBJECT_UPGRADE` buttons carry.
    upgrade_builds: Mutex<BTreeMap<String, std::collections::BTreeSet<String>>>,
}

impl TemplateStore {
    pub fn new(templates: Templates) -> Self {
        Self {
            templates,
            cache: Mutex::new(BTreeMap::new()),
            combat_cache: Mutex::new(CombatCache::default()),
            unit_builds: Mutex::new(BTreeMap::new()),
            upgrade_builds: Mutex::new(BTreeMap::new()),
        }
    }
    /// Install the command-set build tables (see [`TemplateSource::can_make_unit`]).
    pub fn set_unit_builds(&self, sets: BTreeMap<String, std::collections::BTreeSet<String>>) {
        *self.unit_builds.lock().unwrap_or_else(|e| e.into_inner()) = sets;
    }
    /// Install the command-set upgrade tables (see [`TemplateSource::can_build_upgrade`]).
    pub fn set_upgrade_builds(&self, sets: BTreeMap<String, std::collections::BTreeSet<String>>) {
        *self.upgrade_builds.lock().unwrap_or_else(|e| e.into_inner()) = sets;
    }
    /// The underlying typed templates.
    pub fn templates(&self) -> &Templates {
        &self.templates
    }

    /// Install the garrison `FIREPOINT` bones resolved from the install's W3D art
    /// ([`crate::firepoints::garrison_fire_points`]). Called once, before any template is looked
    /// up; it clears the memo so no template converted earlier keeps empty points.
    pub fn set_fire_points(&self, fire_points: BTreeMap<String, GarrisonFirePoints>) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let mut cc = self.combat_cache.lock().unwrap_or_else(|e| e.into_inner());
        *cc = CombatCache { fire_points, ..CombatCache::default() };
    }

    /// Install the `WeaponLaunchBone` launch positions resolved from the install's W3D art
    /// ([`crate::launch_bones::launch_bones`]). Called once, right after
    /// [`set_fire_points`](Self::set_fire_points), before any template is looked up.
    pub fn set_launch_bones(&self, launch_bones: BTreeMap<String, LaunchBones>) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let mut cc = self.combat_cache.lock().unwrap_or_else(|e| e.into_inner());
        cc.launch_bones = launch_bones;
    }

    /// Install the dock bones resolved from the install's W3D art ([`crate::dock_bones::dock_bones`]).
    /// Called once, right after [`set_launch_bones`](Self::set_launch_bones).
    pub fn set_dock_bones(&self, dock_bones: BTreeMap<String, zr_sim::dock::DockBones>) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let mut cc = self.combat_cache.lock().unwrap_or_else(|e| e.into_inner());
        cc.dock_bones = dock_bones;
    }

    /// Install the `SpawnPointBoneName` bones resolved from the install's W3D art
    /// ([`crate::spawn_bones::spawn_bones`]).
    pub fn set_spawn_bones(&self, spawn_bones: BTreeMap<String, Vec<SpawnPointBone>>) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let mut cc = self.combat_cache.lock().unwrap_or_else(|e| e.into_inner());
        cc.spawn_bones = spawn_bones;
    }

    /// Install the airfield parking/runway bones resolved from the install's W3D art
    /// ([`crate::parking_bones::parking_bones`]).
    pub fn set_parking_bones(&self, parking_bones: BTreeMap<String, zr_sim::jet_ai::ParkingBones>) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let mut cc = self.combat_cache.lock().unwrap_or_else(|e| e.into_inner());
        cc.parking_bones = parking_bones;
    }
}

impl TemplateSource for TemplateStore {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(a) = cache.get(name) {
            return Some(a.clone());
        }
        let mut cc = self.combat_cache.lock().unwrap_or_else(|e| e.into_inner());
        let a = cc.thing(&self.templates, name)?;
        cache.insert(name.to_owned(), a.clone());
        Some(a)
    }

    /// `ThingFactory::firstTemplate` order: templates are prepended, so newest first.
    fn harvester_templates(&self) -> Vec<String> {
        self.templates
            .things()
            .iter()
            .rev()
            .filter(|t| t.is_kind_of(KindOf::Harvester))
            .map(|t| t.name.clone())
            .collect()
    }

    /// `AIPlayer::queueDozer` (AIPlayer.cpp:3169-3236): every `KINDOF_DOZER` template, newest first (`firstTemplate`).
    fn dozer_templates(&self) -> Vec<String> {
        self.templates
            .things()
            .iter()
            .rev()
            .filter(|t| t.is_kind_of(KindOf::Dozer))
            .map(|t| t.name.clone())
            .collect()
    }

    /// `BuildAssistant::isPossibleToMakeUnit`: the builder's command set has a build button for `unit`.
    fn can_make_unit(&self, builder: &str, unit: &str) -> Option<bool> {
        let set = &self.templates.thing(builder)?.command_set;
        let sets = self.unit_builds.lock().unwrap_or_else(|e| e.into_inner());
        Some(sets.get(set).is_some_and(|s| s.contains(unit)))
    }

    /// `AIPlayer::buildUpgrade`'s `canUpgradeHere` (`AIPlayer.cpp:1822-1836`): the builder's command set
    /// holds a `PLAYER_UPGRADE` / `OBJECT_UPGRADE` button for `upgrade`.
    fn can_build_upgrade(&self, builder: &str, upgrade: &str) -> Option<bool> {
        let set = &self.templates.thing(builder)?.command_set;
        let sets = self.upgrade_builds.lock().unwrap_or_else(|e| e.into_inner());
        Some(sets.get(set).is_some_and(|s| s.contains(&upgrade.to_ascii_lowercase())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zr_templates::{DamageType, DeathType};

    fn squash(s: &str) -> String {
        s.replace('_', "").to_ascii_lowercase()
    }

    #[test]
    fn damage_and_death_tables_line_up_with_the_sim() {
        assert_eq!(DamageType::count(), SimDamageType::COUNT);
        for i in 0..DamageType::count() {
            let name = squash(DamageType::from_index(i).name());
            assert_eq!(name, squash(&format!("{:?}", SimDamageType::from_index(i))), "damage type {i}");
        }
        assert_eq!(DeathType::count(), SimDeathType::COUNT);
        for i in 0..DeathType::count() {
            let name = squash(DeathType::from_index(i).name());
            assert_eq!(name, squash(&format!("{:?}", SimDeathType::from_index(i))), "death type {i}");
        }
    }

    #[test]
    fn kind_of_names_map_to_the_sim_bits() {
        // The script/AI lane needs MONEY_HACKER / NO_GARRISON / FS_INTERNET_CENTER (and the
        // supply-source kinds); every modelled name maps by name, unmodelled kinds are dropped.
        let m: KindOfMask = [
            KindOf::Structure,
            KindOf::MoneyHacker,
            KindOf::NoGarrison,
            KindOf::FsInternetCenter,
            KindOf::SupplySource,
        ]
        .into_iter()
        .collect();
        let bits = kind_of_mask_bits(&m);
        for k in [kind_of::STRUCTURE, kind_of::MONEY_HACKER, kind_of::NO_GARRISON, kind_of::FS_INTERNET_CENTER, kind_of::SUPPLY_SOURCE] {
            assert_ne!(bits & k, 0, "missing bit {k:#x}");
        }
        let unknown: KindOfMask = [KindOf::IgnoreDockingBones].into_iter().collect();
        assert_eq!(kind_of_mask_bits(&unknown), 0);
    }

    #[test]
    fn weapon_conversion_copies_engine_units() {
        let mut w = WeaponTemplate::new("W");
        w.primary_damage = 60.0;
        w.attack_range = 150.0;
        w.min_delay_between_shots = 30;
        w.max_delay_between_shots = 45;
        w.clip_size = 3;
        w.damage_type = DamageType::ArmorPiercing;
        let s = convert_weapon(&w);
        assert_eq!((s.primary_damage, s.attack_range, s.min_delay_between_shots, s.max_delay_between_shots, s.clip_size), (60.0, 150.0, 30, 45, 3));
        assert_eq!(s.damage_type, SimDamageType::ArmorPiercing);
        assert_eq!(s.affects, affects::ALLIES | affects::ENEMIES | affects::NEUTRALS);
        assert_eq!(s.anti_mask, anti::GROUND);
        assert!(s.auto_reloads_clip && !s.has_projectile);
    }

    /// `AntiSmallMissile` / `AntiMine` / `AntiBallisticMissile` / `AntiParachute` (`Weapon.cpp:218`, the `WEAPON_ANTI_*`
    /// bits of `WeaponAntiFlags`, `Weapon.h:100-107`) each keep their own bit: a weapon with `AntiMine` must not also
    /// be an anti-missile weapon. Reverting the four mappings to the shared bit (or to `GROUND`) makes the assert fail.
    #[test]
    fn anti_flags_convert_to_their_own_bits() {
        use zr_templates::WeaponAnti;
        let mut w = WeaponTemplate::new("W");
        w.anti_mask = [WeaponAnti::SmallMissile, WeaponAnti::Mine, WeaponAnti::BallisticMissile, WeaponAnti::Parachute].into_iter().collect();
        assert_eq!(convert_weapon(&w).anti_mask, anti::SMALL_MISSILE | anti::MINE | anti::BALLISTIC_MISSILE | anti::PARACHUTE);
    }
}
