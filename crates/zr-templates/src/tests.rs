//! Unit tests with synthetic INI snippets (no game data needed).

use std::f32::consts::PI as STD_PI;

use zr_ini::IniDb;

use crate::value::*;
use crate::weapon::parse_shot_delay;
use crate::*;

fn build(text: &str) -> (Templates, Vec<Warning>) {
    let blocks = zr_ini::parse("test.ini", text).expect("synthetic INI parses");
    let mut db = IniDb::new();
    db.extend(blocks);
    Templates::from_ini(&db)
}

fn build_clean(text: &str) -> Templates {
    let (t, w) = build(text);
    assert!(w.is_empty(), "unexpected warnings: {w:#?}");
    t
}

/// `DefaultThingTemplate` as in the shipped data (trimmed).
const DEFAULT: &str = "
Object DefaultThingTemplate
  KindOf = NONE
  IsTrainable = No
  WeaponSet
    Conditions = None
    Weapon = PRIMARY None
  End
  ArmorSet
    Conditions = None
    Armor = None
    DamageFX = None
  End
  Body = InactiveBody ModuleTag_DefaultInactiveBody
  End
  Behavior = DestroyDie ModuleTag_DefaultDestroyDie
  End
  Draw = W3DDefaultDraw ModuleTag_DefaultW3DDefaultDraw
  End
  InheritableModule
    Behavior = AutoHealBehavior ModuleTag_DefaultAutoHealBehavior
      HealingAmount = 10
    End
  End
  OverrideableByLikeKind
    Behavior = StealthUpdate ModuleTag_DefaultStealthUpdate
      StealthDelay = 2000
    End
  End
  Geometry = SPHERE
  GeometryMajorRadius = 1.0
  GeometryMinorRadius = 1.0
  GeometryHeight = 1.0
  GeometryIsSmall = Yes
End
";

fn tags(t: &ThingTemplate) -> Vec<&str> {
    t.modules.iter().map(|m| m.tag.as_str()).collect()
}

// ---------------------------------------------------------------------------------------------
// conversions (INI.cpp / GameCommon.h)

#[test]
fn durations_round_up_to_frames() {
    assert_eq!(duration_ms_to_frames_ceil(0), 0);
    assert_eq!(duration_ms_to_frames_ceil(1), 1);
    assert_eq!(duration_ms_to_frames_ceil(33), 1);
    assert_eq!(duration_ms_to_frames_ceil(34), 2);
    assert_eq!(duration_ms_to_frames_ceil(500), 15);
    assert_eq!(duration_ms_to_frames_ceil(1000), 30);
    assert_eq!(duration_ms_to_frames_ceil(2000), 60);
    // the real-valued form keeps the fraction
    assert_eq!(duration_ms_to_frames(1000.0), 1000.0f32 * (30.0f32 / 1000.0f32));
    assert!((duration_ms_to_frames(50.0) - 1.5).abs() < 1e-6);
}

#[test]
#[allow(clippy::approx_constant, clippy::excessive_precision)]
fn pi_is_the_engines_literal() {
    // `PI` in BaseMacros.h is `3.14159265359f`; it rounds to the same f32
    assert_eq!(PI.to_bits(), 3.14159265359f32.to_bits());
}

#[test]
fn velocity_acceleration_angle_conversions() {
    let spf = 1.0f32 / 30.0f32;
    assert_eq!(velocity_per_sec_to_per_frame(30.0), 30.0 * spf);
    assert_eq!(accel_per_sec2_to_per_frame2(1000.0), 1000.0 * (spf * spf));
    assert_eq!(degrees_to_radians(180.0), 180.0 * (PI / 180.0f32));
    assert!((degrees_to_radians(180.0) - STD_PI).abs() < 1e-6);
    assert!((degrees_to_radians(90.0) - STD_PI / 2.0).abs() < 1e-6);
    // 180 deg/s is pi radians per second, pi/30 per frame
    assert_eq!(deg_per_sec_to_rad_per_frame(180.0), 180.0 * (spf * (PI / 180.0f32)));
    assert!((deg_per_sec_to_rad_per_frame(180.0) - STD_PI / 30.0).abs() < 1e-6);
    assert_eq!(friction_per_sec_to_per_frame(6.0), 6.0 * spf);
}

#[test]
fn scalar_scanners_follow_sscanf() {
    assert_eq!(scan_real("1.5").unwrap(), 1.5);
    assert_eq!(scan_real("+2").unwrap(), 2.0);
    assert_eq!(scan_real("-0.25").unwrap(), -0.25);
    assert_eq!(scan_real(".5").unwrap(), 0.5);
    assert_eq!(scan_real("50%").unwrap(), 50.0); // trailing junk ignored
    assert_eq!(scan_real("1e2").unwrap(), 100.0);
    assert!(scan_real("abc").is_err());
    assert!(scan_real("-").is_err());
    assert_eq!(scan_int("12").unwrap(), 12);
    assert_eq!(scan_int("-7").unwrap(), -7);
    assert_eq!(scan_int("3.9").unwrap(), 3);
    assert!(scan_int("x").is_err());
    assert!(scan_uint("-1").is_err());
    assert_eq!(scan_percent("23%").unwrap(), 23.0f32 / 100.0f32);
    assert_eq!(scan_percent("100").unwrap(), 1.0);
    assert!(scan_bool("Yes").unwrap() && !scan_bool("no").unwrap());
    assert!(scan_bool("true").is_err());
    assert_eq!(parse_color("R:100 G:114 B:245").unwrap(), [100, 114, 245, 255]);
    assert_eq!(parse_color("R:1 G:2 B:3 A:4").unwrap(), [1, 2, 3, 4]);
    assert!(parse_color("R:1 G:2").is_err());
    assert!(parse_color("R:300 G:2 B:3").is_err());
}

#[test]
fn bit_flag_parsing() {
    let m: KindOfMask = Mask::EMPTY.parse(&["VEHICLE", "SELECTABLE"]).unwrap();
    assert!(m.contains(KindOf::Vehicle) && m.contains(KindOf::Selectable));
    // a plain list replaces; +/- edit; NONE clears
    let m2 = m.parse(&["INFANTRY"]).unwrap();
    assert!(m2.contains(KindOf::Infantry) && !m2.contains(KindOf::Vehicle));
    let m3 = m.parse(&["+INFANTRY", "-SELECTABLE"]).unwrap();
    assert!(m3.contains(KindOf::Vehicle) && m3.contains(KindOf::Infantry) && !m3.contains(KindOf::Selectable));
    assert!(m.parse(&["NONE"]).unwrap().is_empty());
    assert!(m.parse(&["VEHICLE", "+INFANTRY"]).is_err(), "mixing forms is an error");
    assert!(m.parse(&["NOT_A_KIND"]).is_err());
    assert!(m.parse(&["vehicle"]).unwrap().contains(KindOf::Vehicle), "case-insensitive");
}

#[test]
fn name_tables_match_the_engine() {
    assert_eq!(DamageType::ALL.len(), 38);
    assert_eq!(DamageType::Explosion.index(), 0);
    assert_eq!(DamageType::Status.index(), 37);
    assert_eq!(DamageType::from_name("armor_piercing"), Some(DamageType::ArmorPiercing));
    assert_eq!(DeathType::ALL.len(), 21);
    assert_eq!(KindOf::ALL.len(), 117);
    assert_eq!(KindOf::Obstacle.index(), 0);
    assert_eq!(WeaponSlot::from_name("TERTIARY"), Some(WeaponSlot::Tertiary));
    assert_eq!(LocomotorSetType::from_name("SET_NORMAL_UPGRADED"), Some(LocomotorSetType::SetNormalUpgraded));
}

// ---------------------------------------------------------------------------------------------
// armor

#[test]
fn armor_default_and_overrides_and_redefinition() {
    let t = build_clean(
        "
Armor TankArmor
  Armor = Default 100%
  Armor = SMALL_ARMS 25%
  Armor = FLAME 12.5%
End
Armor TankArmor
  Armor = CRUSH 50%
End
Armor Plain
  Armor = Default 0%
  Armor = UNRESISTABLE 100%
End
",
    );
    // Armor definitions replace each other (`clear()` on every parse): SMALL_ARMS is back to 100%
    let a = t.armor("TankArmor").unwrap();
    assert_eq!(a.coefficient(DamageType::Crush), 0.5);
    assert_eq!(a.coefficient(DamageType::SmallArms), 1.0);
    assert_eq!(a.coefficient(DamageType::Flame), 1.0);
    assert_eq!(t.armors().len(), 2);
    let p = t.armor("Plain").unwrap();
    assert_eq!(p.coefficient(DamageType::Explosion), 0.0);
    assert_eq!(p.coefficient(DamageType::Unresistable), 1.0);
    // adjustDamage: unresistable bypasses, others scaled, never negative
    assert_eq!(p.adjust_damage(DamageType::Explosion, 100.0), 0.0);
    assert_eq!(p.adjust_damage(DamageType::SubdualUnresistable, 100.0), 100.0);
    assert_eq!(a.adjust_damage(DamageType::Crush, 80.0), 40.0);
}

#[test]
fn armor_unknown_damage_type_warns() {
    let (_, w) = build("Armor A\n  Armor = BOGUS 50%\n  Bogus = 1\nEnd\n");
    assert_eq!(w.len(), 2);
    assert_eq!(w[0].kind, WarningKind::BadValue);
    assert_eq!(w[1].kind, WarningKind::UnknownField);
    assert!(w[1].scope.contains("Armor A"));
}

// ---------------------------------------------------------------------------------------------
// weapon

const WEAPON: &str = "
Weapon TankGun
  PrimaryDamage = 60.0
  PrimaryDamageRadius = 5.0
  AttackRange = 150.0
  MinimumAttackRange = 10
  AcceptableAimDelta = 90
  DamageType = ARMOR_PIERCING
  DeathType = EXPLODED
  WeaponSpeed = 300
  MinWeaponSpeed = 30
  WeaponRecoil = 5
  MinTargetPitch = -15
  MaxTargetPitch = 15
  ProjectileObject = Shell
  FireFX = FX_Fire
  VeterancyFireFX = HEROIC FX_FireHero
  FireSound = TankSound
  DelayBetweenShots = 2000
  ClipSize = 3
  ClipReloadTime = 1000
  PreAttackDelay = 500
  PreAttackType = PER_ATTACK
  AutoReloadsClip = RETURN_TO_BASE
  RadiusDamageAffects = ALLIES ENEMIES
  ProjectileCollidesWith = STRUCTURES WALLS
  AntiAirborneVehicle = Yes
  AntiGround = No
  WeaponBonus = VETERAN RATE_OF_FIRE 125%
  WeaponBonus = ELITE DAMAGE 150%
  ScatterTarget = X:1.5 Y:-2
  ScatterTarget = X:0 Y:3
  ContinuousFireCoast = 100
  HistoricBonusTime = 3000
End
Object Shell
  KindOf = PROJECTILE
End
";

#[test]
fn weapon_fields_are_converted() {
    let t = build_clean(WEAPON);
    let w = t.weapon("TankGun").unwrap();
    assert_eq!(w.primary_damage, 60.0);
    assert_eq!(w.primary_damage_radius, 5.0);
    assert_eq!(w.attack_range, 150.0);
    assert_eq!(w.minimum_attack_range, 10.0);
    assert!((w.acceptable_aim_delta - STD_PI / 2.0).abs() < 1e-6); // degrees to radians
    assert_eq!(w.damage_type, DamageType::ArmorPiercing);
    assert_eq!(w.death_type, DeathType::Exploded);
    assert_eq!(w.weapon_speed, velocity_per_sec_to_per_frame(300.0)); // per frame
    assert_eq!(w.min_weapon_speed, velocity_per_sec_to_per_frame(30.0));
    assert!((w.weapon_recoil - degrees_to_radians(5.0)).abs() < 1e-9);
    assert!((w.min_target_pitch + degrees_to_radians(15.0)).abs() < 1e-9);
    assert_eq!(w.min_delay_between_shots, 60); // 2000 ms
    assert_eq!(w.max_delay_between_shots, 60);
    assert_eq!(w.clip_size, 3);
    assert_eq!(w.clip_reload_time, 30);
    assert_eq!(w.pre_attack_delay, 15);
    assert_eq!(w.pre_attack_type, WeaponPrefire::PerAttack);
    assert_eq!(w.auto_reloads_clip, WeaponReload::ReturnToBase);
    assert_eq!(w.continuous_fire_coast_frames, 3);
    assert_eq!(w.historic_bonus_time, 90);
    assert!(w.radius_damage_affects.contains(WeaponAffects::Allies));
    assert!(!w.radius_damage_affects.contains(WeaponAffects::Neutrals));
    assert!(w.projectile_collides_with.contains(WeaponCollide::Walls));
    assert!(!w.projectile_collides_with.contains(WeaponCollide::Allies));
    assert!(w.anti_mask.contains(WeaponAnti::AirborneVehicle));
    assert!(!w.anti_mask.contains(WeaponAnti::Ground), "AntiGround = No clears the default bit");
    assert_eq!(w.weapon_bonus[&(WeaponBonusCondition::Veteran, WeaponBonusField::RateOfFire)], 1.25);
    assert_eq!(w.weapon_bonus[&(WeaponBonusCondition::Elite, WeaponBonusField::Damage)], 1.5);
    assert_eq!(w.scatter_targets, vec![(1.5, -2.0), (0.0, 3.0)]);
    // FireFX sets every level; VeterancyFireFX overrides one
    assert_eq!(w.fire_fx[0].as_deref(), Some("FX_Fire"));
    assert_eq!(w.fire_fx[3].as_deref(), Some("FX_FireHero"));
    assert_eq!(w.fire_sound.as_deref(), Some("TankSound"));
    // reference resolved
    let p = w.projectile_object.as_ref().unwrap();
    assert_eq!(t.thing_by_id(p.id.unwrap()).name, "Shell");
}

#[test]
fn weapon_defaults_match_the_constructor() {
    let t = build_clean("Weapon W\nEnd\n");
    let w = t.weapon("W").unwrap();
    assert_eq!(w.damage_type, DamageType::Explosion);
    assert_eq!(w.weapon_speed, 999999.0);
    assert_eq!(w.min_target_pitch, -PI);
    assert_eq!(w.max_target_pitch, PI);
    assert_eq!(w.radius_damage_angle, PI);
    assert_eq!(w.shots_per_barrel, 1);
    assert_eq!(w.continuous_fire_one_shots_needed, i32::MAX);
    assert!(w.anti_mask.contains(WeaponAnti::Ground) && !w.anti_mask.contains(WeaponAnti::AirborneVehicle));
    assert!(w.radius_damage_affects.contains(WeaponAffects::Neutrals));
    assert_eq!(w.projectile_collides_with, Mask::EMPTY.with(WeaponCollide::Structures));
    assert_eq!(w.auto_reloads_clip, WeaponReload::Yes);
    assert!(w.projectile_object.is_none());
}

#[test]
fn shot_delay_forms() {
    assert_eq!(parse_shot_delay("1000").unwrap(), (30, 30));
    assert_eq!(parse_shot_delay("Min:100 Max:200").unwrap(), (3, 6));
    assert_eq!(parse_shot_delay("Min: 100 Max: 200").unwrap(), (3, 6));
    assert_eq!(parse_shot_delay("Min:100").unwrap(), (3, 3));
    assert_eq!(parse_shot_delay("Min:100 Foo:200").unwrap(), (3, 3));
    assert!(parse_shot_delay("").is_err());
    let t = build_clean("Weapon W\n  DelayBetweenShots = Min:300 Max:600\nEnd\n");
    let w = t.weapon("W").unwrap();
    assert_eq!((w.min_delay_between_shots, w.max_delay_between_shots), (9, 18));
}

#[test]
fn weapon_none_projectile_and_duplicates() {
    let (t, w) = build(
        "
Weapon W
  ProjectileObject = None
  PrimaryDamage = 1
End
Weapon W
  PrimaryDamage = 99
End
",
    );
    let wt = t.weapon("W").unwrap();
    assert!(wt.projectile_object.is_none());
    assert_eq!(wt.primary_damage, 1.0, "a repeated Weapon is rejected outside override loads");
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].kind, WarningKind::DuplicateIgnored);
}

#[test]
fn weapon_unknown_field_bad_value_and_missing_reference_warn() {
    let (_, w) = build(
        "
Weapon W
  Bogus = 1
  PrimaryDamage = abc
  DamageType = NOT_A_TYPE
  HistoricBonusWeapon = Ghost
End
",
    );
    let kinds: Vec<_> = w.iter().map(|x| x.kind).collect();
    assert_eq!(
        kinds,
        [WarningKind::UnknownField, WarningKind::BadValue, WarningKind::BadValue, WarningKind::UnknownReference]
    );
    assert!(w[0].message.contains("Bogus"));
    assert!(w[0].scope.contains("Weapon W"));
}

// ---------------------------------------------------------------------------------------------
// locomotor

#[test]
fn locomotor_units_and_validation() {
    let t = build_clean(
        "
Locomotor L
  Surfaces = GROUND CLIFF
  Speed = 30
  SpeedDamaged = 15
  TurnRate = 180
  Acceleration = 1000
  Braking = 900
  MinSpeed = 3
  MinTurnSpeed = 6
  PreferredHeight = 100
  ZAxisBehavior = SURFACE_RELATIVE_HEIGHT
  Appearance = HOVER
  GroupMovementPriority = MOVES_FRONT
  AccelerationPitchLimit = 5
  BounceAmount = 30
  Extra2DFriction = 3
  SlideIntoPlaceTime = 1000
  CanMoveBackwards = Yes
End
",
    );
    let l = t.locomotor("L").unwrap();
    assert!(l.surfaces.contains(LocomotorSurface::Ground) && l.surfaces.contains(LocomotorSurface::Cliff));
    assert_eq!(l.max_speed, velocity_per_sec_to_per_frame(30.0));
    assert_eq!(l.max_speed_damaged, velocity_per_sec_to_per_frame(15.0));
    assert_eq!(l.max_turn_rate, deg_per_sec_to_rad_per_frame(180.0));
    // omitted damaged values default to the undamaged ones (`validate`)
    assert_eq!(l.max_turn_rate_damaged, l.max_turn_rate);
    assert_eq!(l.acceleration_damaged, l.acceleration);
    assert_eq!(l.acceleration, accel_per_sec2_to_per_frame2(1000.0));
    assert_eq!(l.braking, accel_per_sec2_to_per_frame2(900.0));
    assert_eq!(l.min_speed, velocity_per_sec_to_per_frame(3.0));
    assert_eq!(l.min_turn_speed, velocity_per_sec_to_per_frame(6.0));
    assert_eq!(l.preferred_height, 100.0); // plain Real, not converted
    assert_eq!(l.behavior_z, LocomotorBehaviorZ::SurfaceRelativeHeight);
    assert_eq!(l.appearance, LocomotorAppearance::Hover);
    assert_eq!(l.move_priority, LocomotorPriority::MovesFront);
    assert_eq!(l.accel_pitch_limit, degrees_to_radians(5.0));
    assert_eq!(l.bounce_kick, deg_per_sec_to_rad_per_frame(30.0));
    assert_eq!(l.extra_2d_friction, friction_per_sec_to_per_frame(3.0));
    assert_eq!(l.ultra_accurate_slide_into_place_factor, 30.0); // 1000 ms as fractional frames
    assert!(l.can_move_backward);
}

#[test]
fn locomotor_defaults_and_wings_thrust_healing() {
    let t = build_clean(
        "
Locomotor Plain
End
Locomotor Jet
  Appearance = WINGS
End
Locomotor Missile
  Appearance = THRUST
End
",
    );
    let p = t.locomotor("Plain").unwrap();
    assert_eq!(p.braking, 99999.0);
    assert_eq!(p.min_turn_speed, 99999.0);
    assert_eq!(p.pitch_stiffness, 0.1);
    assert_eq!(p.pitch_damping, 0.9);
    assert_eq!(p.close_enough_dist, 1.0);
    assert_eq!(p.wander_length_factor, 1.0);
    assert_eq!(p.speed_limit_z, 999999.0);
    assert_eq!(p.appearance, LocomotorAppearance::Other);
    let j = t.locomotor("Jet").unwrap();
    assert_eq!((j.min_speed, j.min_turn_speed), (0.01, 99999.0)); // only non-positive values are healed
    let m = t.locomotor("Missile").unwrap();
    assert_eq!((m.max_speed, m.max_speed_damaged, m.min_speed), (0.01, 0.01, 0.01));
}

#[test]
fn locomotor_redefinition_overlays() {
    let t = build_clean(
        "
Locomotor L
  Speed = 30
  Acceleration = 100
End
Locomotor L
  Speed = 60
End
",
    );
    let l = t.locomotor("L").unwrap();
    assert_eq!(l.max_speed, velocity_per_sec_to_per_frame(60.0));
    assert_eq!(l.acceleration, accel_per_sec2_to_per_frame2(100.0), "fields not mentioned survive");
    assert_eq!(t.locomotors().len(), 1);
}

#[test]
fn locomotor_thrust_with_lift_is_rejected() {
    let (_, w) = build("Locomotor L\n  Appearance = THRUST\n  Lift = 10\nEnd\n");
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].kind, WarningKind::Structure);
}

// ---------------------------------------------------------------------------------------------
// thing templates

#[test]
fn thing_scalar_fields_and_conversions() {
    let t = build_clean(&format!(
        "{DEFAULT}
Object Tank
  Side = America
  EditorSorting = VEHICLE
  KindOf = VEHICLE SELECTABLE CAN_ATTACK
  BuildCost = 900
  BuildTime = 10.0
  VisionRange = 150
  ShroudClearingRange = 300
  PlacementViewAngle = 90
  ExperienceValue = 100 100 200 400
  ExperienceRequired = 0 200 300 600
  SkillPointValue = 1 2 3 4
  CommandSet = TankCommandSet
  Buildable = Ignore_Prerequisites
  RadarPriority = UNIT
  DisplayColor = R:10 G:20 B:30
  OcclusionDelay = 1000
  Scale = 1.5
  Shadow = SHADOW_VOLUME
  ShadowSizeX = 4.5
  VoiceSelect = TankVoice
  VoiceMove = NoSound
  UpgradeCameo2 = Upgrade_X
  MaxSimultaneousOfType = 4
  CrusherLevel = 2
  CrushableLevel = 3
  ThreatValue = 7
  TransportSlotCount = 3
  BuildVariations = TankA TankB
  Geometry = BOX
  GeometryMajorRadius = 15
  GeometryMinorRadius = 10
  GeometryHeight = 10
  GeometryIsSmall = No
  UnitSpecificSounds
    TurretMove = TurretSound
  End
  Prerequisites
    Object = WarFactory Palace
    Science = SCIENCE_X
  End
End
"
    ));
    let o = t.thing("Tank").unwrap();
    assert_eq!(o.side, "America");
    assert_eq!(o.editor_sorting, EditorSorting::Vehicle);
    assert!(o.is_kind_of(KindOf::Vehicle) && o.is_kind_of(KindOf::CanAttack));
    assert_eq!(o.build_cost, 900);
    assert_eq!(o.build_time, 10.0);
    assert_eq!(o.vision_range, 150.0);
    assert_eq!(o.shroud_clearing_range, 300.0);
    assert!((o.placement_view_angle - STD_PI / 2.0).abs() < 1e-6);
    assert_eq!(o.experience_value, [100, 100, 200, 400]);
    assert_eq!(o.experience_required, [0, 200, 300, 600]);
    assert_eq!(o.skill_point_value, [1, 2, 3, 4]);
    assert_eq!(o.buildable, BuildableStatus::IgnorePrerequisites);
    assert_eq!(o.radar_priority, RadarPriority::Unit);
    assert_eq!(o.display_color, Some([10, 20, 30, 255]));
    assert_eq!(o.occlusion_delay, 30);
    assert_eq!(o.scale, 1.5);
    assert!(o.shadow_type.contains(ShadowType::ShadowVolume));
    assert_eq!(o.sounds.get("VoiceSelect").map(String::as_str), Some("TankVoice"));
    assert!(!o.sounds.contains_key("VoiceMove"));
    assert_eq!(o.upgrade_cameos[1].as_deref(), Some("Upgrade_X"));
    assert_eq!(o.max_simultaneous_of_type, MaxSimultaneous::Count(4));
    assert_eq!((o.crusher_level, o.crushable_level, o.threat_value, o.transport_slot_count), (2, 3, 7, 3));
    assert_eq!(o.build_variations, ["TankA", "TankB"]);
    assert_eq!(o.per_unit_sounds["TurretMove"], "TurretSound");
    assert_eq!(
        o.prerequisites,
        [
            Prerequisite::AnyOfObjects(vec!["WarFactory".into(), "Palace".into()]),
            Prerequisite::Science("SCIENCE_X".into())
        ]
    );
    // geometry and the derived bounding radii (calcBoundingStuff)
    let g = o.geometry;
    assert_eq!(g.geometry_type, GeometryType::Box);
    assert_eq!((g.major_radius, g.minor_radius, g.height, g.is_small), (15.0, 10.0, 10.0, false));
    assert_eq!(g.bounding_circle_radius, ((15.0f64 * 15.0 + 10.0 * 10.0).sqrt()) as f32);
    assert_eq!(g.bounding_sphere_radius, ((15.0f64 * 15.0 + 10.0 * 10.0 + 25.0).sqrt()) as f32);
}

#[test]
fn defaults_come_from_default_thing_template_and_the_constructor() {
    let t = build_clean(&format!("{DEFAULT}\nObject Bare\nEnd\n"));
    let o = t.thing("Bare").unwrap();
    assert_eq!(o.shroud_clearing_range, -1.0);
    assert_eq!(o.build_time, 1.0);
    assert_eq!(o.crushable_level, 255);
    assert_eq!(o.skill_point_value, [-999; 4]);
    assert_eq!(o.occlusion_delay, 90);
    assert_eq!(o.max_simultaneous_of_type, MaxSimultaneous::Count(0));
    // module and set copies from the default template
    assert_eq!(tags(o), ["ModuleTag_DefaultInactiveBody", "ModuleTag_DefaultDestroyDie", "ModuleTag_DefaultW3DDefaultDraw", "ModuleTag_DefaultAutoHealBehavior", "ModuleTag_DefaultStealthUpdate"]);
    assert!(o.modules.iter().all(|m| m.copied_from_default));
    // ids are assigned in definition order
    assert_eq!(t.thing_id("DefaultThingTemplate"), Some(ThingId(0)));
    assert_eq!(o.id, ThingId(1));
}

#[test]
fn geometry_set_normalises_but_ini_fields_do_not() {
    let g = Geometry::new(GeometryType::Sphere, true, 99.0, 4.0, 77.0);
    assert_eq!((g.major_radius, g.minor_radius, g.height), (4.0, 4.0, 4.0));
    let g = Geometry::new(GeometryType::Cylinder, false, 8.0, 3.0, 77.0);
    assert_eq!((g.major_radius, g.minor_radius, g.height), (3.0, 3.0, 8.0));
    assert_eq!(g.bounding_sphere_radius, 4.0); // height / 2
    // INI fields set the members one by one: a sphere with a height keeps it
    let t = build_clean("Object S\n  Geometry = SPHERE\n  GeometryMajorRadius = 2\n  GeometryHeight = 9\nEnd\n");
    let g = t.thing("S").unwrap().geometry;
    assert_eq!((g.major_radius, g.height, g.bounding_sphere_radius), (2.0, 9.0, 2.0));
}

#[test]
fn modules_replace_defaults_by_interface() {
    let t = build_clean(&format!(
        "{DEFAULT}
Object Tank
  KindOf = VEHICLE
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 480
    InitialHealth = 480
  End
  Draw = W3DTankDraw ModuleTag_Draw
  End
  Behavior = SlowDeathBehavior ModuleTag_Die
    DestructionDelay = 100
  End
End
"
    ));
    let o = t.thing("Tank").unwrap();
    // InactiveBody (BODY), W3DDefaultDraw (DRAW) and DestroyDie (DIE) are all replaced;
    // AutoHeal is inheritable but the object is not trainable, so SlowDeath's UPDATE bit removes it too;
    // StealthUpdate is overrideable-by-like-kind and a vehicle is a candidate, so it stays.
    assert_eq!(tags(o), ["ModuleTag_DefaultStealthUpdate", "ModuleTag_Body", "ModuleTag_Draw", "ModuleTag_Die"]);
    assert_eq!(o.body().unwrap().max_health, 480.0);
}

#[test]
fn inheritable_default_survives_for_trainable_objects() {
    let t = build_clean(&format!(
        "{DEFAULT}
Object Soldier
  KindOf = INFANTRY
  IsTrainable = Yes
  Behavior = SlowDeathBehavior ModuleTag_Die
  End
End
Object Building
  KindOf = STRUCTURE
  IsTrainable = Yes
  Behavior = SlowDeathBehavior ModuleTag_Die
  End
End
Object Replacer
  KindOf = VEHICLE
  Behavior = StealthUpdate ModuleTag_MyStealth
  End
End
"
    ));
    let s = t.thing("Soldier").unwrap();
    assert!(tags(s).contains(&"ModuleTag_DefaultAutoHealBehavior"), "{:?}", tags(s));
    assert!(tags(s).contains(&"ModuleTag_DefaultStealthUpdate"));
    // structures are `disallowed`: the default stealth module goes as soon as any update module is added
    let b = t.thing("Building").unwrap();
    assert!(!tags(b).contains(&"ModuleTag_DefaultStealthUpdate"), "{:?}", tags(b));
    // naming the same class replaces the default
    let r = t.thing("Replacer").unwrap();
    assert!(!tags(r).contains(&"ModuleTag_DefaultStealthUpdate"));
    assert!(tags(r).contains(&"ModuleTag_MyStealth"));
}

#[test]
fn add_remove_replace_modules() {
    let t = build_clean(&format!(
        "{DEFAULT}
Object Base
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 100
  End
  Behavior = FXListDie ModuleTag_A
    DeathFX = FX_A
  End
  Behavior = CreateObjectDie ModuleTag_B
  End
End
Object Base
  AddModule
    Behavior = FXListDie ModuleTag_C
    End
  End
  RemoveModule = ModuleTag_B
  ReplaceModule ModuleTag_A
    Behavior = FXListDie ModuleTag_A2
      DeathFX = FX_A2
    End
  End
End
"
    ));
    let o = t.thing("Base").unwrap();
    // the overlay keeps the earlier definition's modules (a repeated definition is not re-flagged as
    // copied from the default). FXListDie (DIE | UPGRADE) displaced the default DestroyDie and
    // the untrainable auto-heal; the default draw and stealth modules remain.
    assert_eq!(
        tags(o),
        ["ModuleTag_DefaultW3DDefaultDraw", "ModuleTag_DefaultStealthUpdate", "ModuleTag_Body", "ModuleTag_C", "ModuleTag_A2"]
    );
}

#[test]
fn module_edit_errors_are_reported() {
    let (t, w) = build(&format!(
        "{DEFAULT}
Object X
  RemoveModule = ModuleTag_Nope
  ReplaceModule ModuleTag_Nope
    Behavior = SlowDeathBehavior ModuleTag_N
    End
  End
  Behavior = NotARealModule ModuleTag_Z
  End
  Body = SlowDeathBehavior ModuleTag_Y
  End
  Draw = SlowDeathBehavior ModuleTag_Y2
  End
End
"
    ));
    assert_eq!(w.len(), 5, "{w:#?}");
    assert!(w.iter().all(|x| x.kind == WarningKind::Structure));
    assert_eq!(t.thing("X").unwrap().modules.len(), 5, "failed edits add nothing");
}

#[test]
fn replace_module_requires_same_class_and_new_tag() {
    let (_, w) = build(&format!(
        "{DEFAULT}
Object X
  Behavior = FXListDie ModuleTag_A
  End
  ReplaceModule ModuleTag_A
    Behavior = CreateObjectDie ModuleTag_A2
    End
  End
  ReplaceModule ModuleTag_A2
    Behavior = CreateObjectDie ModuleTag_A2
    End
  End
End
"
    ));
    // the first replaces FXListDie with a different class: rejected, and the removed module stays removed,
    // so the second ReplaceModule cannot find its target
    assert_eq!(w.len(), 2, "{w:#?}");
}

#[test]
fn ai_replacement_clears_the_previous_ai_and_its_locomotors() {
    let (t, w) = build(&format!(
        "{DEFAULT}
Locomotor L1
  Speed = 10
End
Locomotor L2
  Speed = 20
End
Object U
  Behavior = AIUpdateInterface ModuleTag_AI1
  End
  Locomotor = SET_NORMAL L1
  Behavior = JetAIUpdate ModuleTag_AI2
  End
  Locomotor = SET_NORMAL L2
End
Object NoAi
  Locomotor = SET_NORMAL L1
End
"
    ));
    let u = t.thing("U").unwrap();
    assert_eq!(u.modules.iter().filter(|m| m.is_ai()).count(), 1);
    assert_eq!(u.ai().unwrap().class_name, "JetAIUpdate");
    let set = u.locomotor_set(LocomotorSetType::SetNormal);
    assert_eq!(set.len(), 1);
    assert_eq!(set[0].name, "L2");
    assert_eq!(t.locomotor_by_id(set[0].id.unwrap()).name, "L2");
    // a locomotor without an AI module is a structural problem
    assert_eq!(w.len(), 1, "{w:#?}");
    assert_eq!(w[0].kind, WarningKind::Structure);
}

#[test]
fn ai_update_data_and_turrets() {
    let t = build_clean(
        "
Object Tank
  Behavior = AIUpdateInterface ModuleTag_AI
    Turret
      TurretTurnRate = 180
      TurretPitchRate = 60
      NaturalTurretAngle = 90
      FiresWhileTurning = Yes
      ControlledWeaponSlots = PRIMARY SECONDARY
      TurretFireAngleSweep = PRIMARY 15
      TurretSweepSpeedModifier = SECONDARY 0.5
      RecenterTime = 5000
      MinIdleScanInterval = 1000
      AllowsPitch = Yes
    End
    AltTurret
      TurretTurnRate = 90
    End
    AutoAcquireEnemiesWhenIdle = Yes ATTACK_BUILDINGS
    MoodAttackCheckRate = 500
    ForbidPlayerCommands = Yes
  End
  Locomotor = SET_NORMAL Loco
  Locomotor = SET_WANDER None
End
Locomotor Loco
End
",
    );
    let ai = t.thing("Tank").unwrap().ai().unwrap();
    let tur = ai.turret.as_ref().unwrap();
    assert_eq!(tur.turn_rate, deg_per_sec_to_rad_per_frame(180.0));
    assert_eq!(tur.pitch_rate, deg_per_sec_to_rad_per_frame(60.0));
    assert_eq!(tur.natural_turret_angle, degrees_to_radians(90.0));
    assert!(tur.fires_while_turning && tur.allows_pitch);
    assert!(tur.controlled_weapon_slots.contains(WeaponSlot::Primary) && tur.controlled_weapon_slots.contains(WeaponSlot::Secondary));
    assert_eq!(tur.turret_fire_angle_sweep, [degrees_to_radians(15.0), 0.0, 0.0]);
    assert_eq!(tur.turret_sweep_speed_modifier, [1.0, 0.5, 1.0]);
    assert_eq!(tur.recenter_time, 150);
    assert_eq!(tur.min_idle_scan_interval, 30);
    assert_eq!(tur.max_idle_scan_interval, 9_999_999);
    assert_eq!(ai.alt_turret.as_ref().unwrap().turn_rate, deg_per_sec_to_rad_per_frame(90.0));
    assert_eq!(ai.alt_turret.as_ref().unwrap().pitch_rate, 0.01, "DEFAULT_PITCH_RATE");
    assert!(ai.auto_acquire_enemies_when_idle.contains(AutoAcquire::Yes));
    assert!(ai.auto_acquire_enemies_when_idle.contains(AutoAcquire::AttackBuildings));
    assert_eq!(ai.mood_attack_check_rate, 15);
    assert!(ai.forbid_player_commands && !ai.turrets_linked);
    assert_eq!(ai.locomotor_sets[&LocomotorSetType::SetNormal].len(), 1);
    assert!(ai.locomotor_sets[&LocomotorSetType::SetWander].is_empty(), "`None` entries are skipped");
}

#[test]
fn derived_ai_classes_keep_their_own_fields_raw() {
    let (t, w) = build(
        "
Object Heli
  Behavior = JetAIUpdate ModuleTag_AI
    OutOfAmmoDamagePerSecond = 10%
    AutoAcquireEnemiesWhenIdle = Yes
  End
End
",
    );
    // JetAIUpdate adds fields the base table does not know; they are kept, not reported
    assert!(w.is_empty(), "{w:#?}");
    let ai = t.thing("Heli").unwrap().ai().unwrap();
    assert_eq!(ai.class_name, "JetAIUpdate");
    assert!(ai.auto_acquire_enemies_when_idle.contains(AutoAcquire::Yes));
    assert_eq!(ai.extra.len(), 1, "the derived-class field stays raw");
}

#[test]
fn exact_ai_interface_reports_unknown_fields() {
    let (_, w) = build("Object U\n  Behavior = AIUpdateInterface ModuleTag_AI\n    Bogus = 1\n  End\nEnd\n");
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].kind, WarningKind::UnknownField);
    assert!(w[0].scope.contains("AIUpdateInterface ModuleTag_AI"), "{}", w[0].scope);
}

#[test]
fn body_and_physics_modules() {
    let t = build_clean(
        "
GameData
  Gravity = -64.0
End
Object Tank
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 480.0
    InitialHealth = 400.0
    SubdualDamageCap = 1000
    SubdualDamageHealRate = 500
    SubdualDamageHealAmount = 50
  End
  Behavior = PhysicsBehavior ModuleTag_Phys
    Mass = 50.0
    ForwardFriction = 6
    AllowBouncing = Yes
    MinFallHeightForDamage = 10
  End
End
Object Hive
  Body = HiveStructureBody ModuleTag_Body
    MaxHealth = 10
    PropagateDamageTypesToSlavesWhenExisting = NONE +EXPLOSION +CRUSH
  End
End
",
    );
    let b = t.thing("Tank").unwrap().body().unwrap();
    assert_eq!((b.max_health, b.initial_health), (480.0, 400.0));
    assert_eq!(b.subdual_damage_heal_rate, 15);
    assert_eq!(b.class, BodyClass::Active);
    let p = t
        .thing("Tank")
        .unwrap()
        .modules
        .iter()
        .find_map(|m| if let ModuleData::Physics(p) = &m.data { Some(p) } else { None })
        .unwrap();
    assert_eq!(p.mass, 50.0);
    assert_eq!(p.forward_friction, friction_per_sec_to_per_frame(6.0));
    assert_eq!(p.lateral_friction, 0.15, "defaults are already per frame");
    assert!(p.allow_bouncing && p.allow_collide_force);
    // gravity -64 /s^2 -> per frame^2, then v = sqrt(|2 g h|)
    let g = accel_per_sec2_to_per_frame2(-64.0);
    assert_eq!(t.gravity, g);
    assert_eq!(p.min_fall_speed_for_damage, module::height_to_speed(g, 10.0));
    let hb = t.thing("Hive").unwrap().body().unwrap();
    assert_eq!(hb.class, BodyClass::HiveStructure);
    // parseDamageTypeFlags: starts at ALL, NONE clears, +TYPE adds
    let want: Mask<DamageType> = [DamageType::Explosion, DamageType::Crush].into_iter().collect();
    assert_eq!(hb.damage_types_to_propagate_to_slaves, want);
    assert!(hb.damage_types_to_swallow.is_empty());
}

#[test]
fn physics_rejects_non_positive_mass() {
    let (_, w) = build("Object O\n  Behavior = PhysicsBehavior ModuleTag_P\n    Mass = 0\n  End\nEnd\n");
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].kind, WarningKind::BadValue);
}

/// `RailroadBehaviorModuleData` derives from `PhysicsBehaviorModuleData` (`RailroadGuideAIUpdate.h:43`)
/// and `buildFieldParse` runs the physics table first (`:51`), so a RailroadBehavior block also carries
/// `Mass`, `AllowBouncing`, ... (the shipped blocks set none of them and get the constructor defaults).
#[test]
fn railroad_behavior_block_carries_the_physics_fields() {
    let t = build_clean(
        "
GameData
  Gravity = -64.0
End
Object Train
  Behavior = RailroadBehavior ModuleTag_Rail
    Mass = 50.0
    AllowBouncing = Yes
    MinFallHeightForDamage = 10
    IsLocomotive = Yes
    CarriageTemplateName = Car
    KillSpeedMin = 2.5
    RunningGarrisonSpeedMax = 0.5
    WaitAtStationTime = 1000
  End
End
",
    );
    let found = t.thing("Train").unwrap().modules.iter().find_map(|m| match &m.data {
        ModuleData::Logic(l) => match l.as_ref() {
            LogicModule::Railroad { physics, kill_speed_min, running_garrison_speed_max, wait_at_station_time, .. } => {
                Some((physics, *kill_speed_min, *running_garrison_speed_max, *wait_at_station_time))
            }
            _ => None,
        },
        _ => None,
    });
    let (p, kill_speed_min, garrison_speed_max, wait_at_station_time) = found.expect("a RailroadBehavior module");
    assert_eq!(p.mass, 50.0, "the PhysicsBehavior table parsed the block's Mass");
    assert!(p.allow_bouncing);
    assert_eq!(p.min_fall_speed_for_damage, module::height_to_speed(t.gravity, 10.0));
    assert_eq!(kill_speed_min, 2.5);
    assert_eq!(garrison_speed_max, 0.5);
    assert_eq!(wait_at_station_time, 30, "1000 ms is 30 logic frames");
}

#[test]
fn inactive_body_has_no_fields() {
    let (_, w) = build("Object O\n  Body = InactiveBody ModuleTag_B\n    MaxHealth = 5\n  End\nEnd\n");
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].kind, WarningKind::UnknownField);
}

#[test]
fn unparsed_modules_keep_every_field_and_nested_block() {
    let t = build_clean(
        "
Object O
  Draw = W3DModelDraw ModuleTag_Draw
    DefaultConditionState
      Model = FOO
    End
    OkToChangeModelColor = Yes
  End
End
",
    );
    let m = t.thing("O").unwrap().module_by_tag("ModuleTag_Draw").unwrap();
    let ModuleData::Unparsed { module_type, tag, fields } = &m.data else { panic!("expected raw") };
    assert_eq!((module_type.as_str(), tag.as_str()), ("W3DModelDraw", "ModuleTag_Draw"));
    assert_eq!(fields.len(), 2);
    assert!(matches!(&fields[0], RawItem::Block(b) if b.kind == "DefaultConditionState" && b.items.len() == 1));
    assert_eq!(diag::raw_value(fields, "OkToChangeModelColor"), Some("Yes"));
}

#[test]
fn armor_and_weapon_sets_replace_defaults_then_accumulate() {
    let t = build_clean(&format!(
        "{DEFAULT}
Armor A1
End
Armor A2
End
Weapon W1
End
Weapon W2
End
Weapon W3
End
Object U
  ArmorSet
    Conditions = None
    Armor = A1
    DamageFX = FX1
  End
  ArmorSet
    Conditions = VETERAN ELITE
    Armor = A2
  End
  WeaponSet
    Conditions = None
    Weapon = PRIMARY W1
    Weapon = SECONDARY W2
    AutoChooseSources = SECONDARY FROM_PLAYER
    PreferredAgainst = PRIMARY INFANTRY
    ShareWeaponReloadTime = Yes
  End
  WeaponSet
    Conditions = PLAYER_UPGRADE
    Weapon = PRIMARY W3
  End
End
Object Plain
End
"
    ));
    let u = t.thing("U").unwrap();
    // the first ArmorSet / WeaponSet drops the copy of the default's set
    assert_eq!(u.armor_sets.len(), 2);
    assert_eq!(u.weapon_sets.len(), 2);
    assert_eq!(u.armor_sets[0].damage_fx.as_deref(), Some("FX1"));
    assert_eq!(u.armor_sets[0].armor.as_ref().unwrap().name, "A1");
    assert!(u.armor_sets[1].conditions.contains(ArmorSetType::Veteran) && u.armor_sets[1].conditions.contains(ArmorSetType::Elite));
    let ws = &u.weapon_sets[0];
    assert_eq!(t.weapon_by_id(ws.weapons[0].as_ref().unwrap().id.unwrap()).name, "W1");
    assert_eq!(ws.weapons[1].as_ref().unwrap().name, "W2");
    assert!(ws.weapons[2].is_none());
    assert_eq!(ws.auto_choose_sources[1], Mask::EMPTY.with(CommandSource::FromPlayer));
    assert_eq!(ws.auto_choose_sources[0].bits(), u32::MAX as u128, "default: any source");
    assert!(ws.preferred_against[0].contains(KindOf::Infantry));
    assert!(ws.share_weapon_reload_time && !ws.weapon_lock_shared_across_sets);
    assert!(u.weapon_sets[1].conditions.contains(WeaponSetType::PlayerUpgrade));
    assert!(u.weapon_set_for(Mask::EMPTY).is_some());
    // an object that defines none keeps the default's
    let p = t.thing("Plain").unwrap();
    assert_eq!(p.armor_sets.len(), 1);
    assert_eq!(p.weapon_sets.len(), 1);
}

#[test]
fn missing_references_warn_without_dropping_the_set() {
    let (t, w) = build("Object U\n  WeaponSet\n    Weapon = PRIMARY Ghost\n  End\n  ArmorSet\n    Armor = NoSuch\n  End\nEnd\n");
    assert_eq!(w.len(), 2, "{w:#?}");
    assert!(w.iter().all(|x| x.kind == WarningKind::UnknownReference));
    let u = t.thing("U").unwrap();
    assert_eq!(u.weapon_sets[0].weapons[0].as_ref().unwrap().id, None);
}

// ---------------------------------------------------------------------------------------------
// reskin and override semantics

#[test]
fn object_reskin_copies_the_parent_and_replaces_only_appearance() {
    let t = build_clean(&format!(
        "{DEFAULT}
Armor A
End
Object Parent
  Side = America
  KindOf = VEHICLE
  BuildCost = 500
  ArmorSet
    Conditions = None
    Armor = A
  End
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 300
  End
  Draw = W3DModelDraw ModuleTag_Draw
    OkToChangeModelColor = Yes
  End
  Geometry = BOX
  GeometryMajorRadius = 10
  GeometryMinorRadius = 5
  GeometryHeight = 4
  FenceWidth = 2
End
ObjectReskin Child Parent
  Draw = W3DModelDraw ModuleTag_Draw2
    OkToChangeModelColor = No
  End
  GeometryMajorRadius = 12
  FenceWidth = 3
  MaxSimultaneousOfType = 2
End
"
    ));
    let p = t.thing("Parent").unwrap();
    let c = t.thing("Child").unwrap();
    assert_eq!(c.name, "Child");
    assert_ne!(c.id, p.id);
    assert_eq!(c.reskinned_from.as_deref(), Some("Parent"));
    // everything else is the parent's
    assert_eq!((c.side.as_str(), c.build_cost), ("America", 500));
    assert!(c.is_kind_of(KindOf::Vehicle));
    assert_eq!(c.armor_sets, p.armor_sets);
    assert_eq!(c.body().unwrap().max_health, 300.0);
    // Draw is replaced
    let draws: Vec<_> = c.modules_of(ModuleKind::Draw).map(|m| m.tag.as_str()).collect();
    assert_eq!(draws, ["ModuleTag_Draw2"]);
    // geometry fields, fence width and MaxSimultaneous are reskin-able
    assert_eq!(c.geometry.major_radius, 12.0);
    assert_eq!(c.geometry.minor_radius, 5.0);
    assert_eq!(c.fence_width, 3.0);
    assert_eq!(c.max_simultaneous_of_type, MaxSimultaneous::Count(2));
    // the parent is untouched
    assert_eq!(p.geometry.major_radius, 10.0);
    assert_eq!(p.fence_width, 2.0);
    assert_eq!(p.modules_of(ModuleKind::Draw).count(), 1);
}

#[test]
fn object_reskin_rejects_other_fields_and_missing_parents() {
    let (t, w) = build(
        "
Object P
  BuildCost = 5
End
ObjectReskin C P
  BuildCost = 99
End
ObjectReskin Orphan Nobody
End
",
    );
    assert_eq!(w.len(), 2, "{w:#?}");
    assert_eq!(w[0].kind, WarningKind::UnknownField);
    assert_eq!(w[1].kind, WarningKind::UnknownReference);
    assert_eq!(t.thing("C").unwrap().build_cost, 5, "BuildCost is not in the reskin table");
}

#[test]
fn repeated_object_definitions_overlay() {
    let t = build_clean(&format!(
        "{DEFAULT}
Object Thing
  BuildCost = 100
  VisionRange = 50
  KindOf = VEHICLE
  Body = ActiveBody ModuleTag_Body
    MaxHealth = 10
  End
End
Object Thing
  BuildCost = 200
  KindOf = +SELECTABLE
End
"
    ));
    let o = t.thing("Thing").unwrap();
    assert_eq!(o.build_cost, 200, "later definition wins per field");
    assert_eq!(o.vision_range, 50.0, "fields it omits survive");
    assert!(o.is_kind_of(KindOf::Vehicle) && o.is_kind_of(KindOf::Selectable));
    assert_eq!(o.body().unwrap().max_health, 10.0);
    assert_eq!(t.things().len(), 2, "one template per name");
}

#[test]
fn names_are_case_sensitive() {
    let t = build_clean("Weapon Gun\nEnd\nObject Obj\nEnd\nArmor Ar\nEnd\nLocomotor Lo\nEnd\n");
    assert!(t.weapon("Gun").is_some() && t.weapon("gun").is_none());
    assert!(t.thing("Obj").is_some() && t.thing("obj").is_none());
    assert!(t.armor("Ar").is_some() && t.armor("ar").is_none());
    assert!(t.locomotor("Lo").is_some() && t.locomotor("lo").is_none());
}

#[test]
fn thing_unknown_and_bad_fields_are_reported() {
    let (_, w) = build("Object O\n  Bogus = 1\n  BuildCost = lots\n  KindOf = NOPE\n  Geometry = TRIANGLE\n  DisplayColor = R:1\nEnd\n");
    let kinds: Vec<_> = w.iter().map(|x| x.kind).collect();
    assert_eq!(kinds[0], WarningKind::UnknownField);
    assert!(kinds[1..].iter().all(|k| *k == WarningKind::BadValue));
    assert_eq!(w.len(), 5);
    assert!(w[0].scope.starts_with("Object O"));
    assert!(w[0].line > 0);
}

#[test]
fn registry_order_is_deterministic_definition_order() {
    let (a, _) = build("Weapon Z\nEnd\nWeapon A\nEnd\nWeapon M\nEnd\nObject Q\nEnd\nObject B\nEnd\n");
    let names: Vec<_> = a.weapons().iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["Z", "A", "M"]);
    assert_eq!(a.weapon_id("A"), Some(WeaponId(1)));
    let names: Vec<_> = a.things().iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["Q", "B"]);
}

// ----- map.ini style overrides (INI_LOAD_CREATE_OVERRIDES) ----------------------------------

/// Base text, then override text loaded on top (`first_override` = end of the base).
fn build_overlay(base: &str, over: &str) -> (Templates, Vec<Warning>) {
    let mut db = IniDb::new();
    db.extend(zr_ini::parse("base.ini", base).expect("base parses"));
    let cp = db.checkpoint();
    db.extend(zr_ini::parse("map.ini", over).expect("override parses"));
    Templates::from_ini_overlay(&db, cp)
}

fn max_health(t: &Templates, name: &str) -> f32 {
    let b = t.thing(name).unwrap().body().expect("body");
    b.max_health
}

const TANK: &str = "
Object Tank
  KindOf = VEHICLE SCORE
  Body = ActiveBody ModuleTag_02
    MaxHealth = 300.0
    InitialHealth = 300.0
  End
  Behavior = AIUpdateInterface ModuleTag_03
  End
  Behavior = SlowDeathBehavior ModuleTag_04
  End
  VisionRange = 100
End
";

#[test]
fn override_replace_module_changes_body() {
    let base = format!("{DEFAULT}{TANK}");
    let over = "
Object Tank
  ReplaceModule ModuleTag_02
    Body = ActiveBody ModuleTag_02_Override
      MaxHealth = 999.0
      InitialHealth = 999.0
    End
  End
  VisionRange = 50
End
";
    let (t, w) = build_overlay(&base, over);
    assert!(w.is_empty(), "{w:#?}");
    assert_eq!(max_health(&t, "Tank"), 999.0);
    let tank = t.thing("Tank").unwrap();
    assert!(tank.module_by_tag("ModuleTag_02").is_none());
    assert!(tank.module_by_tag("ModuleTag_02_Override").is_some());
    assert!(tank.ai().is_some(), "other modules survive");
    assert_eq!(tank.vision_range, 50.0);
    // the base build is unaffected
    let (b, _) = build(&base);
    assert_eq!(max_health(&b, "Tank"), 300.0);
}

#[test]
fn override_add_module_second_body_wins() {
    // Engine: AddModule just appends; `Object::Object` keeps the last body module.
    let base = format!("{DEFAULT}{TANK}");
    let over = "
Object Tank
  AddModule
    Body = ActiveBody ModuleTag_Map
      MaxHealth = 777.0
      InitialHealth = 777.0
    End
  End
End
";
    let (t, w) = build_overlay(&base, over);
    assert!(w.is_empty(), "{w:#?}");
    assert_eq!(max_health(&t, "Tank"), 777.0);
}

#[test]
fn override_does_not_clear_default_modules() {
    // Base load: a new module sharing an interface drops "copied from default" modules (the AI
    // module added after reskin / default copy). Override load: nothing is cleared.
    let base = format!("{DEFAULT}{TANK}");
    let over = "
Object Tank
  AddModule
    Behavior = SlowDeathBehavior ModuleTag_Map
    End
  End
End
";
    let (t, w) = build_overlay(&base, over);
    assert!(w.is_empty(), "{w:#?}");
    let tank = t.thing("Tank").unwrap();
    assert!(tank.module_by_tag("ModuleTag_04").is_some());
    assert!(tank.module_by_tag("ModuleTag_Map").is_some());
    // the Default* modules of the base template were already gone and stay gone
    assert!(tank.module_by_tag("ModuleTag_DefaultDestroyDie").is_none());
}

#[test]
fn override_bare_module_is_rejected() {
    let base = format!("{DEFAULT}{TANK}");
    let over = "
Object Tank
  Body = ActiveBody ModuleTag_Bare
    MaxHealth = 5.0
  End
  VisionRange = 7
End
";
    let (t, w) = build_overlay(&base, over);
    assert_eq!(w.len(), 1, "{w:#?}");
    assert_eq!(max_health(&t, "Tank"), 300.0);
    assert_eq!(t.thing("Tank").unwrap().vision_range, 7.0);
}

// ---------------------------------------------------------------------------------------------
// upgrades

#[test]
fn upgrade_fields_defaults_and_overlay() {
    let t = build_clean(
        "
Upgrade DefaultUpgrade
  Type = PLAYER
  BuildTime = 5.0
  BuildCost = 500
  ButtonImage = SCTempDefaultInventory
End
Upgrade Upgrade_AmericaRadar
  DisplayName = UPGRADE:Radar
  Type = OBJECT
  BuildTime = 10.0
  BuildCost = 500
  ResearchSound = NoSound
  AcademyClassify = ACT_UPGRADE_RADAR
End
Upgrade Upgrade_AmericaRadar
  BuildCost = 750
  UnitSpecificSound = RadarSound
End
Upgrade CopyCat
End
",
    );
    assert_eq!(t.upgrades().len(), 3);
    let def = t.upgrade("DefaultUpgrade").unwrap();
    assert_eq!(def.upgrade_type, UpgradeType::Player);
    assert_eq!((def.build_time, def.build_cost), (5.0, 500));
    assert_eq!(def.button_image.as_deref(), Some("SCTempDefaultInventory"));
    // a repeated definition overlays; fields it omits survive
    let r = t.upgrade("Upgrade_AmericaRadar").unwrap();
    assert_eq!(r.upgrade_type, UpgradeType::Object);
    assert_eq!(r.build_time, 10.0);
    assert_eq!(r.build_cost, 750);
    assert_eq!(r.display_name.as_deref(), Some("UPGRADE:Radar"));
    assert_eq!(r.academy_classification, AcademyClassification::UpgradeRadar);
    assert!(r.research_sound.is_none(), "NoSound leaves the default (empty) value");
    assert_eq!(r.unit_specific_sound.as_deref(), Some("RadarSound"));
    // a brand-new upgrade starts as a copy of DefaultUpgrade (`UpgradeCenter::newUpgrade`)
    let c = t.upgrade("CopyCat").unwrap();
    assert_eq!((c.upgrade_type, c.build_time, c.build_cost), (UpgradeType::Player, 5.0, 500));
}

#[test]
fn upgrade_unknown_and_bad_fields_warn() {
    let (_, w) = build("Upgrade U\n  Bogus = 1\n  Type = NOPE\nEnd\n");
    assert_eq!(w.len(), 2);
    assert_eq!(w[0].kind, WarningKind::UnknownField);
    assert_eq!(w[1].kind, WarningKind::BadValue);
}

// ---------------------------------------------------------------------------------------------
// object creation lists

#[test]
fn ocl_nuggets_are_typed() {
    let t = build_clean(
        "
Weapon Gun
End
ObjectCreationList OCL_Test
  CreateObject
    ObjectNames = Foo Bar
    Count = 2
    Disposition = RANDOM_FORCE
    Offset = X:1 Y:2 Z:3
  End
  CreateDebris
    ModelNames = D1 D2 D3
    Mass = 5
    SpinRate = 180
    ExtraFriction = 6
    MinLifetime = 1000
    AnimationSet = A B C
    Disposition = LIKE_EXISTING INHERIT_VELOCITY
  End
  ApplyRandomForce
    MinForceMagnitude = 60
    MaxForceMagnitude = 100
    MinForcePitch = 70
    MaxForcePitch = 90
    SpinRate = 120
  End
  FireWeapon
    Weapon = Gun
  End
  Attack
    WeaponSlot = SECONDARY
    NumberOfShots = 9
    DeliveryDecalRadius = 200
  End
End
",
    );
    let ocl = t.ocl("OCL_Test").unwrap();
    assert_eq!(ocl.nuggets.len(), 5);
    let Nugget::CreateObject(o) = &ocl.nuggets[0] else { panic!("expected CreateObject") };
    assert_eq!(o.names, ["Foo", "Bar"], "every name token is kept");
    assert_eq!(o.count, 2);
    assert_eq!(o.offset, (1.0, 2.0, 3.0));
    assert!(o.disposition.contains(DebrisDisposition::RandomForce));
    assert!(!o.disposition.contains(DebrisDisposition::OnGroundAligned), "a bare list replaces");
    let Nugget::CreateDebris(d) = &ocl.nuggets[1] else { panic!("expected CreateDebris") };
    assert_eq!(d.names, ["D1", "D2", "D3"]);
    assert_eq!(d.mass, 5.0);
    assert_eq!(d.spin_rate, deg_per_sec_to_rad_per_frame(180.0));
    assert_eq!(d.extra_friction, friction_per_sec_to_per_frame(6.0));
    assert_eq!(d.min_lifetime, duration_ms_to_frames_ceil(1000));
    assert_eq!(d.animation_sets, [AnimSet { initial: "A".into(), flying: "B".into(), final_: "C".into() }]);
    assert!(d.disposition.contains(DebrisDisposition::LikeExisting));
    assert!(d.disposition.contains(DebrisDisposition::InheritVelocity));
    assert!(!d.disposition.contains(DebrisDisposition::OnGroundAligned));
    let Nugget::ApplyRandomForce(f) = &ocl.nuggets[2] else { panic!("expected ApplyRandomForce") };
    assert_eq!(f.spin_rate, deg_per_sec_to_rad_per_frame(120.0));
    assert_eq!(f.min_force_pitch, degrees_to_radians(70.0));
    assert_eq!(f.max_force_pitch, degrees_to_radians(90.0));
    let Nugget::FireWeapon(w) = &ocl.nuggets[3] else { panic!("expected FireWeapon") };
    assert_eq!(w.weapon.name, "Gun");
    assert_eq!(t.weapon_by_id(w.weapon.id.unwrap()).name, "Gun");
    let Nugget::Attack(a) = &ocl.nuggets[4] else { panic!("expected Attack") };
    assert_eq!(a.number_of_shots, 9);
    assert_eq!(a.weapon_slot, WeaponSlot::Secondary);
    assert_eq!(a.delivery_decal_radius, 200.0);
}

#[test]
fn ocl_redefinition_replaces() {
    let t = build_clean(
        "
ObjectCreationList O
  CreateObject
    ObjectNames = A
  End
End
ObjectCreationList O
  CreateDebris
    ModelNames = B
    Mass = 1
  End
End
",
    );
    assert_eq!(t.ocls().len(), 1);
    let ocl = t.ocl("O").unwrap();
    assert_eq!(ocl.nuggets.len(), 1, "clear() on every definition: the earlier nugget is gone");
    assert!(matches!(ocl.nuggets[0], Nugget::CreateDebris(_)));
}

#[test]
fn ocl_deliver_payload_and_missing_reference() {
    let (t, w) = build(
        "
ObjectCreationList O
  DeliverPayload
    Transport = CargoPlane
    StartAtPreferredHeight = No
    FormationSize = 3
    FormationSpacing = 40
    Payload = Bomb 2
    Payload = Trooper
    DeliveryDistance = 140
    DropOffset = X:0 Y:0 Z:-10
    DropDelay = 500
    StrafingWeaponSlot = TERTIARY
    VisiblePayloadWeaponTemplate = Gun
  End
End
Weapon Gun
End
",
    );
    assert!(w.is_empty(), "{w:#?}");
    let Nugget::DeliverPayload(d) = &t.ocl("O").unwrap().nuggets[0] else { panic!("expected DeliverPayload") };
    assert_eq!(d.transport, "CargoPlane");
    assert!(!d.start_at_preferred_height);
    assert_eq!((d.formation_size, d.formation_spacing), (3, 40.0));
    assert_eq!(d.payload, [Payload { name: "Bomb".into(), count: 2 }, Payload { name: "Trooper".into(), count: 1 }]);
    assert_eq!(d.data.delivery_distance, 140.0);
    assert_eq!(d.data.drop_offset, (0.0, 0.0, -10.0));
    assert_eq!(d.data.drop_delay, 15);
    assert_eq!(d.data.strafing_weapon_slot, Some(WeaponSlot::Tertiary));
    assert_eq!(d.data.visible_payload_weapon_template.as_ref().unwrap().name, "Gun");
}

// ---------------------------------------------------------------------------------------------
// die modules

const DIE_OBJECTS: &str = "
Weapon Gun
End
Upgrade Upgrade_A
End
Upgrade Upgrade_B
End
ObjectCreationList OCL_X
  FireWeapon
    Weapon = Gun
  End
End
Object Tank
  Behavior = SlowDeathBehavior ModuleTag_SD
    DeathTypes = NONE +EXPLODED
    VeterancyLevels = NONE +VETERAN
    ExemptStatus = UNDER_CONSTRUCTION
    RequiredStatus = SOLD
    SinkRate = 30
    DestructionDelay = 1000
    FX = INITIAL FX_A FX_B
    OCL = FINAL OCL_X
    Weapon = MIDPOINT Gun
    FlingPitch = 45
  End
  Behavior = CreateObjectDie ModuleTag_CO
    CreationList = OCL_X
    TransferPreviousHealth = Yes
  End
  Behavior = FXListDie ModuleTag_FX
    DeathFX = FX_D
    StartsActive = No
    TriggeredBy = Upgrade_A
    ConflictsWith = Upgrade_B
  End
  Behavior = InstantDeathBehavior ModuleTag_ID
    FX = FX_E
    OCL = OCL_X
    Weapon = Gun
  End
  Behavior = DestroyDie ModuleTag_D
  End
End
";

#[test]
fn die_mux_and_slow_death() {
    let t = build_clean(DIE_OBJECTS);
    let tank = t.thing("Tank").unwrap();
    let ModuleData::Die(die) = &tank.module_by_tag("ModuleTag_SD").unwrap().data else { panic!("expected die") };
    assert!(die.mux.death_types.contains(DeathType::Exploded));
    assert!(!die.mux.death_types.contains(DeathType::Normal));
    // VeterancyLevelFlags: NONE +VETERAN sets bit (level-1); REGULAR never matches
    assert!(die.mux.applies_to_veterancy(1));
    assert!(!die.mux.applies_to_veterancy(2));
    assert!(!die.mux.applies_to_veterancy(0));
    assert!(die.mux.exempt_status.contains(ObjectStatus::UnderConstruction));
    assert!(die.mux.required_status.contains(ObjectStatus::Sold));
    let DieKind::SlowDeath(sd) = &die.kind else { panic!("expected slow death") };
    assert_eq!(sd.sink_rate, velocity_per_sec_to_per_frame(30.0));
    assert_eq!(sd.destruction_delay, 30);
    assert_eq!(sd.fling_pitch, degrees_to_radians(45.0));
    assert_eq!(sd.fx[&SlowDeathPhase::Initial], [Some("FX_A".into()), Some("FX_B".into())]);
    let ocl = &sd.ocl[&SlowDeathPhase::Final];
    assert_eq!(ocl[0].name, "OCL_X");
    assert!(ocl[0].id.is_some());
    let wep = &sd.weapons[&SlowDeathPhase::Midpoint];
    assert_eq!(wep[0].name, "Gun");
    assert!(wep[0].id.is_some());
}

#[test]
fn die_kinds_create_fxlist_instant_destroy() {
    let t = build_clean(DIE_OBJECTS);
    let tank = t.thing("Tank").unwrap();
    let ModuleData::Die(co) = &tank.module_by_tag("ModuleTag_CO").unwrap().data else { panic!() };
    let DieKind::CreateObject { ocl, transfer_previous_health, transfer_selection } = &co.kind else { panic!() };
    assert_eq!(ocl.as_ref().unwrap().name, "OCL_X");
    assert!(ocl.as_ref().unwrap().id.is_some());
    assert!(*transfer_previous_health && !*transfer_selection);
    let ModuleData::Die(fx) = &tank.module_by_tag("ModuleTag_FX").unwrap().data else { panic!() };
    let DieKind::FxList { default_death_fx, orient_to_object, initially_active, upgrade } = &fx.kind else { panic!() };
    assert_eq!(default_death_fx.as_deref(), Some("FX_D"));
    assert!(*orient_to_object && !*initially_active);
    assert_eq!(upgrade.triggered_by, ["Upgrade_A"]);
    assert_eq!(upgrade.conflicts_with, ["Upgrade_B"]);
    let ModuleData::Die(id) = &tank.module_by_tag("ModuleTag_ID").unwrap().data else { panic!() };
    let DieKind::InstantDeath { fx, ocl, weapons } = &id.kind else { panic!() };
    assert_eq!(fx, &[Some("FX_E".into())]);
    assert_eq!(ocl[0].name, "OCL_X");
    assert_eq!(weapons[0].name, "Gun");
    let ModuleData::Die(d) = &tank.module_by_tag("ModuleTag_D").unwrap().data else { panic!() };
    assert_eq!(d.kind, DieKind::Destroy);
}

// ---------------------------------------------------------------------------------------------
// upgrade modules and production

#[test]
fn upgrade_modules_and_production_update() {
    let t = build_clean(
        "
Upgrade Upgrade_A
End
Object U
  Behavior = ArmorUpgrade ModuleTag_A
    TriggeredBy = Upgrade_A
    RequiresAllTriggers = Yes
  End
  Behavior = MaxHealthUpgrade ModuleTag_M
    TriggeredBy = Upgrade_A
    AddMaxHealth = 100
    ChangeType = PRESERVE_RATIO
  End
  Behavior = ProductionUpdate ModuleTag_P
    MaxQueueEntries = 5
    DoorOpeningTime = 1000
    QuantityModifier = Foo 2
    QuantityModifier = Bar
    DisabledTypesToProcess = DISABLED_HELD DISABLED_UNDERPOWERED
  End
End
",
    );
    let u = t.thing("U").unwrap();
    let ModuleData::Upgrade(a) = &u.module_by_tag("ModuleTag_A").unwrap().data else { panic!() };
    assert_eq!(a.kind, UpgradeKind::Armor);
    assert_eq!(a.mux.triggered_by, ["Upgrade_A"]);
    assert!(a.mux.requires_all_triggers);
    let ModuleData::Upgrade(m) = &u.module_by_tag("ModuleTag_M").unwrap().data else { panic!() };
    let UpgradeKind::MaxHealth { add_max_health, change_type } = &m.kind else { panic!() };
    assert_eq!(*add_max_health, 100.0);
    assert_eq!(*change_type, MaxHealthChangeType::PreserveRatio);
    assert_eq!(m.mux.triggered_by, ["Upgrade_A"]);
    let ModuleData::Production(p) = &u.module_by_tag("ModuleTag_P").unwrap().data else { panic!() };
    assert_eq!(p.max_queue_entries, 5);
    assert_eq!(p.door_opening_time, 30);
    assert_eq!(
        p.quantity_modifiers,
        [QuantityModifier { template_name: "Foo".into(), quantity: 2 }, QuantityModifier { template_name: "Bar".into(), quantity: 1 }]
    );
    assert!(p.disabled_types_to_process.contains(DisabledType::Held));
    assert!(p.disabled_types_to_process.contains(DisabledType::Underpowered));
    assert!(!p.disabled_types_to_process.contains(DisabledType::Default));
}

#[test]
fn override_armor_and_weapon_sets_restart() {
    let base = format!(
        "{DEFAULT}
Armor A1
  Armor = DEFAULT 50%
End
Armor A2
  Armor = DEFAULT 10%
End
Weapon W1
  PrimaryDamage = 10.0
End
Weapon W2
  PrimaryDamage = 20.0
End
Object U
  ArmorSet
    Conditions = None
    Armor = A1
  End
  WeaponSet
    Conditions = None
    Weapon = PRIMARY W1
  End
End
"
    );
    let over = "
Object U
  ArmorSet
    Conditions = None
    Armor = A2
  End
  WeaponSet
    Conditions = None
    Weapon = PRIMARY W2
  End
End
Weapon W1
  PrimaryDamage = 99.0
End
";
    let (t, w) = build_overlay(&base, over);
    assert!(w.is_empty(), "{w:#?}");
    let u = t.thing("U").unwrap();
    assert_eq!(u.armor_sets.len(), 1);
    assert_eq!(u.armor_sets[0].armor.as_ref().unwrap().name, "A2");
    assert_eq!(u.weapon_sets.len(), 1);
    assert_eq!(t.weapon("W1").unwrap().primary_damage, 99.0);
    // the base load keeps the first weapon definition and warns about the duplicate
    let mut db = IniDb::new();
    db.extend(zr_ini::parse("a.ini", &format!("{base}{over}")).unwrap());
    let (b, w) = Templates::from_ini(&db);
    assert_eq!(b.weapon("W1").unwrap().primary_damage, 10.0);
    assert!(w.iter().any(|x| x.kind == WarningKind::DuplicateIgnored));
}

// ---------------------------------------------------------------------------------------------
// remaining logic modules (`crate::logic`) and client draw data (`crate::draw`)

#[test]
fn logic_die_and_update_modules() {
    let t = build_clean(
        "
Object L
  Behavior = JetSlowDeathBehavior ModuleTag_J
    DeathTypes = ALL -CRUSHED
    SinkRate = 30
    DestructionDelay = 1000
    FXOnGroundDeath = FX_Ground
    OCLOnGroundDeath = OCL_Ground
    DelaySecondaryFromInitialDeath = 500
    RollRateDelta = 50%
    FallHowFast = 10%
  End
  Behavior = HeightDieUpdate ModuleTag_H
    TargetHeight = 40
    OnlyWhenMovingDown = Yes
    SnapToGroundOnDeath = Yes
    InitialDelay = 250
  End
  Behavior = AutoFindHealingUpdate ModuleTag_F
    ScanRate = 1000
    ScanRange = 200
    NeverHeal = 0.9
    AlwaysHeal = 0.3
  End
  Behavior = DeletionUpdate ModuleTag_D
    MinLifetime = 1000
    MaxLifetime = 2000
  End
  Behavior = EMPUpdate ModuleTag_E
    Lifetime = 1000
    TargetScaleMax = 2.0
    StartColor = R:255 G:0 B:0
    DoesNotAffect = ALLIES
    VictimRequiredKindOf = VEHICLE STRUCTURE
  End
  Behavior = OCLUpdate ModuleTag_O
    OCL = OCL_One
    MinDelay = 500
    MaxDelay = 1000
    CreateAtEdge = Yes
  End
  Behavior = BridgeBehavior ModuleTag_B
    LateralScaffoldSpeed = 30
    BridgeDieFX = FX:BridgeFX Delay:1000 Bone:Root
    BridgeDieOCL = OCL:BridgeOCL Delay:2000
  End
  Behavior = SabotagePowerPlantCrateCollide ModuleTag_C
    RequiredKindOf = VEHICLE
    SabotagePowerDuration = 2000
  End
End
",
    );
    let l = t.thing("L").unwrap();
    let ModuleData::Logic(jet) = &l.module_by_tag("ModuleTag_J").unwrap().data else { panic!("jet") };
    let LogicModule::JetSlowDeath { slow, mux, fx_on_ground_death, delay_secondary_from_initial_death, roll_rate_delta, fall_how_fast, .. } =
        jet.as_ref()
    else {
        panic!("expected JetSlowDeath")
    };
    assert!(!mux.death_types.contains(DeathType::Crushed));
    assert!(mux.death_types.contains(DeathType::Normal));
    assert_eq!(slow.sink_rate, velocity_per_sec_to_per_frame(30.0));
    assert_eq!(slow.destruction_delay, 30);
    assert_eq!(fx_on_ground_death.as_deref(), Some("FX_Ground"));
    assert_eq!(*delay_secondary_from_initial_death, 15);
    assert!((*roll_rate_delta - 0.5).abs() < 1e-6);
    assert!((*fall_how_fast - 0.1).abs() < 1e-6);

    let ModuleData::Logic(h) = &l.module_by_tag("ModuleTag_H").unwrap().data else { panic!("height") };
    let LogicModule::HeightDie { target_height, only_when_moving_down, snap_to_ground_on_death, initial_delay, .. } = h.as_ref() else {
        panic!()
    };
    assert_eq!(*target_height, 40.0);
    assert!(*only_when_moving_down && *snap_to_ground_on_death);
    assert_eq!(*initial_delay, 8); // 250 ms

    let ModuleData::Logic(f) = &l.module_by_tag("ModuleTag_F").unwrap().data else { panic!("heal") };
    let LogicModule::AutoFindHealing { scan_rate, scan_range, never_heal, always_heal } = f.as_ref() else { panic!() };
    assert_eq!(*scan_rate, 30);
    assert_eq!(*scan_range, 200.0);
    assert!((*never_heal - 0.9).abs() < 1e-6);
    assert!((*always_heal - 0.3).abs() < 1e-6);

    let ModuleData::Logic(d) = &l.module_by_tag("ModuleTag_D").unwrap().data else { panic!("deletion") };
    assert_eq!(*d.as_ref(), LogicModule::Deletion { min_lifetime: 30, max_lifetime: 60 });

    let ModuleData::Logic(e) = &l.module_by_tag("ModuleTag_E").unwrap().data else { panic!("emp") };
    let LogicModule::Emp { lifetime, target_scale_max, start_color, does_not_affect, victim_required_kind_of, .. } = e.as_ref() else {
        panic!()
    };
    assert_eq!(*lifetime, 30);
    assert_eq!(*target_scale_max, 2.0);
    assert_eq!(*start_color, [255, 0, 0, 255]);
    assert!(does_not_affect.contains(WeaponAffects::Allies));
    assert!(victim_required_kind_of.contains(KindOf::Vehicle));
    assert!(victim_required_kind_of.contains(KindOf::Structure));

    let ModuleData::Logic(o) = &l.module_by_tag("ModuleTag_O").unwrap().data else { panic!("ocl") };
    let LogicModule::OclUpdate { ocl, min_delay, max_delay, create_at_edge, .. } = o.as_ref() else { panic!() };
    assert_eq!(ocl.as_deref(), Some("OCL_One"));
    assert_eq!((*min_delay, *max_delay, *create_at_edge), (15, 30, true));

    let ModuleData::Logic(b) = &l.module_by_tag("ModuleTag_B").unwrap().data else { panic!("bridge") };
    let LogicModule::Bridge { lateral_scaffold_speed, die_fx, die_ocl, .. } = b.as_ref() else { panic!() };
    assert_eq!(*lateral_scaffold_speed, velocity_per_sec_to_per_frame(30.0));
    assert_eq!(die_fx[0].fx.as_deref(), Some("BridgeFX"));
    assert_eq!(die_fx[0].delay, 30);
    assert_eq!(die_fx[0].bone.as_deref(), Some("Root"));
    assert_eq!(die_ocl[0].ocl.as_deref(), Some("BridgeOCL"));
    assert_eq!(die_ocl[0].delay, 60);

    let ModuleData::CrateCollide(c) = &l.module_by_tag("ModuleTag_C").unwrap().data else { panic!("crate") };
    assert!(c.required_kind_of.contains(KindOf::Vehicle));
    assert_eq!(c.kind, crate::modules2::CrateKind::SabotagePowerPlant { power_sabotage_frames: 60 });
}

#[test]
fn special_ability_update_timings_parse() {
    // `SpecialAbilityUpdateModuleData` (SpecialAbilityUpdate.h:89-114): the ms timings become frames
    // (PreparationTime 20000 -> 600, UnpackTime 3000 -> 90, PackTime 2000 -> 60), and the unset fields
    // keep the engine defaults (HUGE ranges, LOS on, skill points -1).
    let t = build_clean(
        "
SpecialPower P
  Enum = SPECIAL_INFANTRY_CAPTURE_BUILDING
  ReloadTime = 1000
End
Object O
  Behavior = SpecialAbility ModuleTag_A
    SpecialPowerTemplate = P
    UpdateModuleStartsAttack = Yes
  End
  Behavior = SpecialAbilityUpdate ModuleTag_U
    SpecialPowerTemplate = P
    StartAbilityRange = 5.0
    UnpackTime = 3000
    PreparationTime = 20000
    PackTime = 2000
    PackUnpackVariationFactor = 0.25
    SkipPackingWithNoTarget = Yes
  End
End
",
    );
    let o = t.thing("O").unwrap();
    let ModuleData::SpecialAbility(u) = &o.module_by_tag("ModuleTag_U").unwrap().data else { panic!() };
    assert!(!u.power_module);
    assert_eq!((u.preparation_frames, u.unpack_time, u.pack_time), (600, 90, 60));
    assert_eq!(u.start_ability_range, 5.0);
    assert_eq!(u.pack_unpack_variation, 0.25);
    assert!(u.skip_packing_with_no_target);
    assert_eq!(u.ability_abort_range, 10_000_000.0);
    assert!(u.approach_requires_los);
    assert_eq!(u.skill_points_for_triggering, -1);
    assert!(u.need_to_face);
    // The power module carries no timings: it keeps the defaults (no preparation, unlimited range).
    let ModuleData::SpecialAbility(a) = &o.module_by_tag("ModuleTag_A").unwrap().data else { panic!() };
    assert!(a.power_module);
    assert_eq!((a.preparation_frames, a.unpack_time, a.start_ability_range), (0, 0, 10_000_000.0));
}

#[test]
fn special_power_pause_flags_parse() {
    // `SpecialPowerModuleData::buildFieldParse` (SpecialPowerModule.cpp:74-86): the four flags on
    // a `SpecialAbility` (a SpecialPowerModule) and an `OCLSpecialPower`; `SpecialAbilityUpdate` is
    // an update module, so `power_module` is false there.
    let t = build_clean(
        "
SpecialPower P
  Enum = SPECIAL_TEST
  ReloadTime = 1000
End
Object O
  Behavior = SpecialAbility ModuleTag_A
    SpecialPowerTemplate = P
    UpdateModuleStartsAttack = Yes
    StartsPaused = Yes
    StartsReady = No
  End
  Behavior = SpecialAbilityUpdate ModuleTag_U
    SpecialPowerTemplate = P
    StartsPaused = Yes
  End
  Behavior = OCLSpecialPower ModuleTag_O
    SpecialPowerTemplate = P
    OCL = SomeOcl
    ScriptedSpecialPowerOnly = Yes
  End
End
",
    );
    let o = t.thing("O").unwrap();
    let ModuleData::SpecialAbility(a) = &o.module_by_tag("ModuleTag_A").unwrap().data else { panic!() };
    assert!(a.power_module);
    assert!(a.flags.starts_paused && a.flags.update_module_starts_attack);
    assert!(!a.flags.starts_ready);
    let ModuleData::SpecialAbility(u) = &o.module_by_tag("ModuleTag_U").unwrap().data else { panic!() };
    assert!(!u.power_module && u.flags.starts_paused);
    let ModuleData::SpecialPower(p) = &o.module_by_tag("ModuleTag_O").unwrap().data else { panic!() };
    assert!(p.flags.scripted_special_power_only && !p.flags.starts_paused);
}

#[test]
fn logic_upgrade_create_and_special_power_modules() {
    let t = build_clean(
        "
SpecialPower P
  Enum = SPECIAL_TEST
  ReloadTime = 1000
End
Object U
  Behavior = UnpauseSpecialPowerUpgrade ModuleTag_U
    SpecialPowerTemplate = P
    TriggeredBy = Upgrade_A
  End
  Behavior = CostModifierUpgrade ModuleTag_C
    EffectKindOf = VEHICLE
    Percentage = 25%
  End
  Behavior = SubObjectsUpgrade ModuleTag_S
    ShowSubObjects = TurretA TurretB
    HideSubObjects = TurretC
  End
  Behavior = GrantUpgradeCreate ModuleTag_G
    UpgradeToGrant = Upgrade_A
    ExemptStatus = UNDER_CONSTRUCTION
  End
  Behavior = LockWeaponCreate ModuleTag_L
    SlotToLock = SECONDARY
  End
  Behavior = GrantStealthBehavior ModuleTag_T
    StartRadius = 10
    FinalRadius = 100
    RadiusGrowRate = 5
    KindOf = INFANTRY
  End
  Behavior = SpyVisionSpecialPower ModuleTag_V
    SpecialPowerTemplate = P
    BaseDuration = 1000
    BonusDurationPerCaptured = 500
    MaxDuration = 10000
  End
  Behavior = CashBountyPower ModuleTag_B
    SpecialPowerTemplate = P
    Bounty = 10%
  End
End
",
    );
    let u = t.thing("U").unwrap();
    let ModuleData::Logic(up) = &u.module_by_tag("ModuleTag_U").unwrap().data else { panic!() };
    let LogicModule::UnpauseSpecialPower { special_power, mux } = up.as_ref() else { panic!() };
    assert_eq!(special_power.as_deref(), Some("P"));
    assert_eq!(mux.triggered_by, ["Upgrade_A"]);

    let ModuleData::Logic(cm) = &u.module_by_tag("ModuleTag_C").unwrap().data else { panic!() };
    let LogicModule::CostModifier { effect_kind_of, percentage, .. } = cm.as_ref() else { panic!() };
    assert!(effect_kind_of.contains(KindOf::Vehicle));
    assert!((*percentage - 0.25).abs() < 1e-6);

    let ModuleData::Logic(so) = &u.module_by_tag("ModuleTag_S").unwrap().data else { panic!() };
    let LogicModule::SubObjects { show_sub_objects, hide_sub_objects, .. } = so.as_ref() else { panic!() };
    assert_eq!(show_sub_objects, &["TurretA".to_string(), "TurretB".to_string()]);
    assert_eq!(hide_sub_objects, &["TurretC".to_string()]);

    let ModuleData::Logic(g) = &u.module_by_tag("ModuleTag_G").unwrap().data else { panic!() };
    let LogicModule::GrantUpgradeCreate { upgrade_to_grant, exempt_status } = g.as_ref() else { panic!() };
    assert_eq!(upgrade_to_grant, "Upgrade_A");
    assert!(exempt_status.contains(ObjectStatus::UnderConstruction));

    let ModuleData::Logic(lw) = &u.module_by_tag("ModuleTag_L").unwrap().data else { panic!() };
    assert_eq!(*lw.as_ref(), LogicModule::LockWeaponCreate { slot: WeaponSlot::Secondary });

    let ModuleData::Logic(gs) = &u.module_by_tag("ModuleTag_T").unwrap().data else { panic!() };
    let LogicModule::GrantStealth { start_radius, final_radius, kind_of, .. } = gs.as_ref() else { panic!() };
    assert_eq!((*start_radius, *final_radius), (10.0, 100.0));
    assert!(kind_of.contains(KindOf::Infantry));

    let ModuleData::SpecialPower(v) = &u.module_by_tag("ModuleTag_V").unwrap().data else { panic!("spy vision") };
    assert_eq!(v.spy_vision_base_duration, 30);
    assert_eq!(v.spy_vision_bonus_per_captured, 15);
    assert_eq!(v.spy_vision_max_duration, 300);
    let ModuleData::SpecialPower(b) = &u.module_by_tag("ModuleTag_B").unwrap().data else { panic!("bounty") };
    assert!((b.bounty - 0.1).abs() < 1e-6);
}

/// The upgrade families the `zr-game` adapter used to drop (`RadarUpgrade`, `GrantScienceUpgrade`,
/// `ReplaceObjectUpgrade`, `PassengersFireUpgrade`) parse their own fields plus the shared
/// `UpgradeMuxData` (`RequiresAllTriggers`, `ConflictsWith`, `TriggeredBy`).
#[test]
fn upgrade_families_the_adapter_dropped_parse() {
    let t = build_clean(
        "
Object U
  Behavior = RadarUpgrade ModuleTag_R
    DisableProof = Yes
    TriggeredBy = Upgrade_Radar
    ConflictsWith = Upgrade_NoRadar
    RequiresAllTriggers = Yes
  End
  Behavior = GrantScienceUpgrade ModuleTag_S
    GrantScience = SCIENCE_MOAB
    TriggeredBy = Upgrade_MOAB
  End
  Behavior = ReplaceObjectUpgrade ModuleTag_B
    ReplaceObject = GLACommandCenter
    TriggeredBy = Upgrade_BecomeReal
  End
  Behavior = PassengersFireUpgrade ModuleTag_F
    TriggeredBy = Upgrade_BattleBunker
    RemovesUpgrades = Upgrade_Old
  End
End
",
    );
    let u = t.thing("U").unwrap();

    let ModuleData::Logic(r) = &u.module_by_tag("ModuleTag_R").unwrap().data else { panic!() };
    let LogicModule::RadarUpgrade { disable_proof, mux } = r.as_ref() else { panic!() };
    assert!(*disable_proof);
    assert_eq!(mux.triggered_by, ["Upgrade_Radar"]);
    assert_eq!(mux.conflicts_with, ["Upgrade_NoRadar"]);
    assert!(mux.requires_all_triggers);

    let ModuleData::Logic(s) = &u.module_by_tag("ModuleTag_S").unwrap().data else { panic!() };
    let LogicModule::GrantScience { grant_science, mux } = s.as_ref() else { panic!() };
    assert_eq!(grant_science, "SCIENCE_MOAB");
    assert_eq!(mux.triggered_by, ["Upgrade_MOAB"]);

    let ModuleData::Logic(b) = &u.module_by_tag("ModuleTag_B").unwrap().data else { panic!() };
    let LogicModule::ReplaceObject { replace_object, mux } = b.as_ref() else { panic!() };
    assert_eq!(replace_object, "GLACommandCenter");
    assert_eq!(mux.triggered_by, ["Upgrade_BecomeReal"]);

    let ModuleData::Logic(f) = &u.module_by_tag("ModuleTag_F").unwrap().data else { panic!() };
    let LogicModule::PassengersFireUpgrade { mux } = f.as_ref() else { panic!() };
    assert_eq!(mux.triggered_by, ["Upgrade_BattleBunker"]);
    assert_eq!(mux.removes_upgrades, ["Upgrade_Old"]);
}

#[test]
fn contain_exit_dock_and_draw_extensions() {
    let t = build_clean(
        "
Object O
  Behavior = ParachuteContain ModuleTag_P
    ContainMax = 5
    PitchRateMax = 90
    LowAltitudeDamping = 0.5
    FreeFallDamagePercent = 20%
  End
  Behavior = RiderChangeContain ModuleTag_R
    Slots = 2
    Rider1 = AmericaInfantryRanger NONE NONE NONE RangerCommandSet SET_NORMAL
    ScuttleDelay = 2000
  End
  Behavior = SpawnPointProductionExitUpdate ModuleTag_E
    SpawnPointBoneName = SpawnBone
  End
  Behavior = RailedTransportDockUpdate ModuleTag_D
    PullInsideDuration = 1000
    PushOutsideDuration = 500
    ToleranceDistance = 25
  End
  Draw = W3DTankDraw ModuleTag_Draw
    TreadDebrisLeft = LeftTrack
    TreadAnimationRate = 30
    TreadPivotSpeedFraction = 0.5
    DefaultConditionState
      Model = TankA
    End
  End
End
",
    );
    let o = t.thing("O").unwrap();
    let ModuleData::Contain(par) = &o.module_by_tag("ModuleTag_P").unwrap().data else { panic!("parachute") };
    assert_eq!(par.kind, ContainKind::Parachute);
    assert_eq!(par.contain_max, 5);
    assert_eq!(par.pitch_rate_max, deg_per_sec_to_rad_per_frame(90.0));
    assert_eq!(par.low_altitude_damping, 0.5);
    assert!((par.free_fall_damage_percent - 0.2).abs() < 1e-6);

    let ModuleData::Contain(rid) = &o.module_by_tag("ModuleTag_R").unwrap().data else { panic!("rider") };
    assert_eq!(rid.kind, ContainKind::RiderChange);
    assert_eq!(rid.slot_capacity, 2);
    assert_eq!(rid.riders[0].template_name, "AmericaInfantryRanger");
    assert_eq!(rid.riders[0].command_set, "RangerCommandSet");
    assert_eq!(rid.scuttle_frames, 60);

    let ModuleData::ProductionExit(e) = &o.module_by_tag("ModuleTag_E").unwrap().data else { panic!("exit") };
    assert_eq!(e.kind, ProductionExitKind::SpawnPoint);
    assert_eq!(e.spawn_point_bone_name, "SpawnBone");

    let ModuleData::Dock(d) = &o.module_by_tag("ModuleTag_D").unwrap().data else { panic!("dock") };
    assert_eq!(d.kind, DockKind::RailedTransport);
    assert_eq!((d.pull_inside_duration, d.push_outside_duration), (30, 15));
    assert_eq!(d.tolerance_distance, 25.0);

    let ModuleData::Draw(dr) = &o.module_by_tag("ModuleTag_Draw").unwrap().data else { panic!("draw") };
    let DrawModule::TankDraw { tread_debris_left, tread_animation_rate, tread_pivot_speed_fraction, .. } = dr.as_ref() else {
        panic!("tank draw")
    };
    assert_eq!(tread_debris_left, "LeftTrack");
    assert_eq!(*tread_animation_rate, velocity_per_sec_to_per_frame(30.0));
    assert_eq!(*tread_pivot_speed_fraction, 0.5);
}

#[test]
fn rider_change_riders_are_typed() {
    // Synthetic rider table: positional tokens, `NONE` for unset condition / command set.
    let t = build_clean(
        "
Object B
  Behavior = RiderChangeContain ModuleTag_R
    Slots = 1
    Rider1 = SynthRebel RIDER1 WEAPON_RIDER1 STATUS_RIDER1 SynthCommandSet SET_NORMAL
    Rider3 = SynthTerrorist NONE WEAPON_RIDER3 STATUS_RIDER3 NONE SET_NORMAL
    ScuttleDelay = 1000
  End
End
",
    );
    let o = t.thing("B").unwrap();
    let ModuleData::Contain(c) = &o.module_by_tag("ModuleTag_R").unwrap().data else { panic!("rider change") };
    assert_eq!(c.kind, ContainKind::RiderChange);
    assert_eq!(c.scuttle_state, "TOPPLED");
    assert_eq!(c.scuttle_frames, 30);
    // Rider3 creates the unset Rider2 slot in between.
    assert_eq!(c.riders.len(), 3);
    assert_eq!(c.riders[0].template_name, "SynthRebel");
    assert_eq!(c.riders[0].model_condition, "RIDER1");
    assert_eq!(c.riders[0].weapon_set.bits(), 1 << 9);
    assert_eq!(c.riders[0].object_status.bits(), 1 << 30);
    assert_eq!(c.riders[0].command_set, "SynthCommandSet");
    assert_eq!(c.riders[0].locomotor_set, LocomotorSetType::SetNormal);
    assert!(c.riders[1].template_name.is_empty());
    assert_eq!(c.riders[2].model_condition, "NONE");
    assert_eq!(c.riders[2].object_status.bits(), 1 << 32);
}

#[test]
fn rider_change_rejects_short_or_unknown_rider_lines() {
    let (_, w) = build(
        "
Object B
  Behavior = RiderChangeContain ModuleTag_R
    Rider1 = SynthRebel RIDER1 WEAPON_RIDER1
    Rider2 = SynthRebel RIDER1 WEAPON_RIDER1 STATUS_RIDER1 NONE SET_BOGUS
  End
End
",
    );
    assert_eq!(w.len(), 2, "{w:#?}");
}

#[test]
fn fire_weapon_collide_status_masks_are_typed() {
    let t = build_clean(
        "
Object F
  Behavior = FireWeaponCollide ModuleTag_F
    FireOnce = Yes
    RequiredStatus = STATUS_RIDER1 AFLAME
    ForbiddenStatus = STATUS_RIDER2
  End
End
",
    );
    let o = t.thing("F").unwrap();
    let ModuleData::FireWeaponCollide(d) = &o.module_by_tag("ModuleTag_F").unwrap().data else { panic!("fire weapon collide") };
    assert_eq!(d.required_status.bits(), (1 << 30) | (1 << 11));
    assert_eq!(d.forbidden_status.bits(), 1 << 31);
    assert!(d.fire_once);
}
