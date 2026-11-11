//! `Update` / `Behavior` logic modules typed from their `buildFieldParse` tables. Dispatched from
//! [`crate::logic::LogicModule::parse`]; all data is inert (the simulation does not read it yet).

use zr_ini::{Block, Item};

use crate::behavior::fields;
use crate::diag::{Diag, WarningKind};
use crate::flags::{KindOfMask, Mask};
use crate::names::{DamageType, WeaponSlot};
use crate::upgrade_module::UpgradeMuxData;
use crate::value::Args;

use super::{
    name, names, opt_name, parse_radius_decal, BoneFxEntry, BridgeFx, BridgeOcl, LogicModule,
};

/// `ModuleFactory::newModuleDataFromINI` for the update/behavior classes this module types.
pub(crate) fn parse(class: &str, block: &Block, gravity: f32, d: &mut Diag) -> Option<LogicModule> {
    Some(match class {
        "AutoFindHealingUpdate" => {
            let mut p = AutoFindHealing { never_heal: 0.95, always_heal: 0.25, ..Default::default() };
            fields(block, d, class, |k, a| {
                match k {
                    "ScanRate" => p.scan_rate = a.duration_frames()?,
                    "ScanRange" => p.scan_range = a.real()?,
                    "NeverHeal" => p.never_heal = a.real()?,
                    "AlwaysHeal" => p.always_heal = a.real()?,
                    _ => return Ok(false),
                }
                Ok(true)
            });
            LogicModule::AutoFindHealing {
                scan_rate: p.scan_rate,
                scan_range: p.scan_range,
                never_heal: p.never_heal,
                always_heal: p.always_heal,
            }
        }
        "MobMemberSlavedUpdate" => {
            let mut must_catch_up_radius = 50;
            let mut catch_up_crisis_bail_time = 999_999;
            let mut no_need_to_catch_up_radius = 25;
            let mut squirrelliness = 0.0;
            fields(block, d, class, |k, a| {
                match k {
                    "MustCatchUpRadius" => must_catch_up_radius = a.int()?,
                    "CatchUpCrisisBailTime" => catch_up_crisis_bail_time = a.uint()?,
                    "NoNeedToCatchUpRadius" => no_need_to_catch_up_radius = a.int()?,
                    "Squirrelliness" => squirrelliness = a.real()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::MobMemberSlaved {
                must_catch_up_radius,
                catch_up_crisis_bail_time,
                no_need_to_catch_up_radius,
                squirrelliness,
            }
        }
        "RailroadBehavior" => {
            let mut p = Railroad {
                running_garrison_speed_max: 1.0,
                kill_speed_min: 1.0,
                speed_max: 4.0,
                acceleration: 1.01,
                braking: 0.99,
                friction: 0.97,
                wait_at_station_time: 150,
                ..Default::default()
            };
            // `RailroadBehaviorModuleData::buildFieldParse` runs `PhysicsBehaviorModuleData::`
            // `buildFieldParse(p)` first (`RailroadGuideAIUpdate.h:51`), so the same block also carries
            // the physics fields (`Mass`, `AllowBouncing`, ...). The shipped RailroadBehavior blocks set
            // none of them; the defaults are the PhysicsBehaviorModuleData constructor's.
            let mut physics = crate::module::PhysicsData::new(gravity);
            fields(block, d, class, |k, a| {
                match k {
                    "PathPrefixName" => p.path_prefix_name = a.next()?.to_string(),
                    "CrashFXTemplateName" => p.crash_fx_template_name = a.next()?.to_string(),
                    "IsLocomotive" => p.is_locomotive = a.boolean()?,
                    "CarriageTemplateName" => p.carriage_template_names.extend(names(a)),
                    "BigMetalBounceSound" => p.big_metal_bounce_sound = name(a)?,
                    "SmallMetalBounceSound" => p.small_metal_bounce_sound = name(a)?,
                    "MeatyBounceSound" => p.meaty_bounce_sound = name(a)?,
                    "RunningGarrisonSpeedMax" => p.running_garrison_speed_max = a.real()?,
                    "KillSpeedMin" => p.kill_speed_min = a.real()?,
                    "SpeedMax" => p.speed_max = a.real()?,
                    "Acceleration" => p.acceleration = a.real()?,
                    "Braking" => p.braking = a.real()?,
                    "WaitAtStationTime" => p.wait_at_station_time = a.duration_frames()?,
                    "RunningSound" => p.running_sound = name(a)?,
                    "ClicketyClackSound" => p.clickety_clack_sound = name(a)?,
                    "WhistleSound" => p.whistle_sound = name(a)?,
                    "Friction" => p.friction = a.real()?,
                    _ => {
                        if !crate::module::physics_field(&mut physics, k, a, gravity)? {
                            let _ = a.rest();
                        }
                    }
                }
                Ok(true)
            });
            LogicModule::Railroad {
                path_prefix_name: p.path_prefix_name,
                crash_fx_template_name: p.crash_fx_template_name,
                physics,
                is_locomotive: p.is_locomotive,
                carriage_template_names: p.carriage_template_names,
                big_metal_bounce_sound: p.big_metal_bounce_sound,
                small_metal_bounce_sound: p.small_metal_bounce_sound,
                meaty_bounce_sound: p.meaty_bounce_sound,
                running_garrison_speed_max: p.running_garrison_speed_max,
                kill_speed_min: p.kill_speed_min,
                speed_max: p.speed_max,
                acceleration: p.acceleration,
                braking: p.braking,
                wait_at_station_time: p.wait_at_station_time,
                running_sound: p.running_sound,
                clickety_clack_sound: p.clickety_clack_sound,
                whistle_sound: p.whistle_sound,
                friction: p.friction,
            }
        }
        "PointDefenseLaserUpdate" => {
            let mut weapon_template = None;
            let mut primary_target_types = KindOfMask::EMPTY;
            let mut secondary_target_types = KindOfMask::EMPTY;
            let mut scan_rate = 0;
            let mut scan_range = 0.0;
            let mut predict_target_velocity_factor = 0.0;
            fields(block, d, class, |k, a| {
                match k {
                    "WeaponTemplate" => weapon_template = name(a)?,
                    "PrimaryTargetTypes" => primary_target_types = primary_target_types.parse(a.rest())?,
                    "SecondaryTargetTypes" => secondary_target_types = secondary_target_types.parse(a.rest())?,
                    "ScanRate" => scan_rate = a.duration_frames()?,
                    "ScanRange" => scan_range = a.real()?,
                    "PredictTargetVelocityFactor" => predict_target_velocity_factor = a.real()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::PointDefenseLaser {
                weapon_template,
                primary_target_types,
                secondary_target_types,
                scan_rate,
                scan_range,
                predict_target_velocity_factor,
            }
        }
        "HijackerUpdate" => {
            let mut attach_to_target_bone = String::new();
            let mut parachute_name = String::new();
            fields(block, d, class, |k, a| {
                match k {
                    "AttachToTargetBone" => attach_to_target_bone = a.next()?.to_string(),
                    "ParachuteName" => parachute_name = a.next()?.to_string(),
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::Hijacker { attach_to_target_bone, parachute_name }
        }
        "PilotFindVehicleUpdate" => {
            let mut scan_rate = 0;
            let mut scan_range = 0.0;
            let mut min_health = 0.5;
            fields(block, d, class, |k, a| {
                match k {
                    "ScanRate" => scan_rate = a.duration_frames()?,
                    "ScanRange" => scan_range = a.real()?,
                    "MinHealth" => min_health = a.real()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::PilotFindVehicle { scan_rate, scan_range, min_health }
        }
        "SpectreGunshipUpdate" => parse_spectre_gunship(block, d),
        "SpectreGunshipDeploymentUpdate" => {
            let mut p = SpectreDeployment::default();
            fields(block, d, class, |k, a| {
                match k {
                    "GunshipTemplateName" => p.gunship_template_name = a.next()?.to_string(),
                    "RequiredScience" => p.required_science = name(a)?,
                    "SpecialPowerTemplate" => p.special_power = name(a)?,
                    "AttackAreaRadius" => p.attack_area_radius = a.real()?,
                    "CreateLocation" => p.create_location = a.next()?.to_string(),
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::SpectreGunshipDeployment {
                gunship_template_name: p.gunship_template_name,
                required_science: p.required_science,
                special_power: p.special_power,
                attack_area_radius: p.attack_area_radius,
                create_location: p.create_location,
            }
        }
        "MissileLauncherBuildingUpdate" => {
            let mut p = MissileLauncherBuilding::default();
            fields(block, d, class, |k, a| {
                match k {
                    "SpecialPowerTemplate" => p.special_power = name(a)?,
                    "DoorOpenTime" => p.door_open_time = a.duration_frames()?,
                    "DoorWaitOpenTime" => p.door_wait_open_time = a.duration_frames()?,
                    "DoorCloseTime" => p.door_close_time = a.duration_frames()?,
                    "DoorOpeningFX" => p.door_opening_fx = name(a)?,
                    "DoorOpenFX" => p.door_open_fx = name(a)?,
                    "DoorWaitingToCloseFX" => p.door_waiting_to_close_fx = name(a)?,
                    "DoorClosingFX" => p.door_closing_fx = name(a)?,
                    "DoorClosedFX" => p.door_closed_fx = name(a)?,
                    "DoorOpenIdleAudio" => p.door_open_idle_audio = name(a)?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::MissileLauncherBuilding {
                special_power: p.special_power,
                door_open_time: p.door_open_time,
                door_wait_open_time: p.door_wait_open_time,
                door_close_time: p.door_close_time,
                door_opening_fx: p.door_opening_fx,
                door_open_fx: p.door_open_fx,
                door_waiting_to_close_fx: p.door_waiting_to_close_fx,
                door_closing_fx: p.door_closing_fx,
                door_closed_fx: p.door_closed_fx,
                door_open_idle_audio: p.door_open_idle_audio,
            }
        }
        "ParticleUplinkCannonUpdate" => parse_particle_uplink(block, d),
        "SmartBombTargetHomingUpdate" => {
            let mut course_correction_scalar = 0.99;
            fields(block, d, class, |k, a| {
                match k {
                    "CourseCorrectionScalar" => course_correction_scalar = a.real()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::SmartBombTargetHoming { course_correction_scalar }
        }
        "AssistedTargetingUpdate" => {
            let mut assisting_clip_size = 1;
            let mut assisting_weapon_slot = WeaponSlot::Primary;
            let mut laser_from_assisted = String::new();
            let mut laser_to_target = String::new();
            fields(block, d, class, |k, a| {
                match k {
                    "AssistingClipSize" => assisting_clip_size = a.int()?,
                    "AssistingWeaponSlot" => assisting_weapon_slot = a.index()?,
                    "LaserFromAssisted" => laser_from_assisted = a.next()?.to_string(),
                    "LaserToTarget" => laser_to_target = a.next()?.to_string(),
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::AssistedTargeting { assisting_clip_size, assisting_weapon_slot, laser_from_assisted, laser_to_target }
        }
        "EMPUpdate" => {
            let mut p = Emp::default();
            fields(block, d, class, |k, a| {
                match k {
                    "Lifetime" => p.lifetime = a.duration_frames()?,
                    "StartFadeTime" => p.start_fade_time = a.duration_frames()?,
                    "StartScale" => p.start_scale = a.real()?,
                    "DisabledDuration" => p.disabled_duration = a.duration_frames()?,
                    "TargetScaleMax" => p.target_scale_max = a.real()?,
                    "TargetScaleMin" => p.target_scale_min = a.real()?,
                    "StartColor" => p.start_color = crate::value::parse_color(&a.rest().join(" "))?,
                    "EndColor" => p.end_color = crate::value::parse_color(&a.rest().join(" "))?,
                    "DisableFXParticleSystem" => p.disable_fx_particle_system = name(a)?,
                    "SparksPerCubicFoot" => p.sparks_per_cubic_foot = a.real()?,
                    "EffectRadius" => p.effect_radius = a.real()?,
                    "DoesNotAffect" => p.does_not_affect = p.does_not_affect.parse(a.rest())?,
                    "DoesNotAffectMyOwnBuildings" => p.does_not_affect_my_own_buildings = a.boolean()?,
                    "VictimRequiredKindOf" => p.victim_required_kind_of = p.victim_required_kind_of.parse(a.rest())?,
                    "VictimForbiddenKindOf" => p.victim_forbidden_kind_of = p.victim_forbidden_kind_of.parse(a.rest())?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::Emp {
                lifetime: p.lifetime,
                start_fade_time: p.start_fade_time,
                start_scale: p.start_scale,
                disabled_duration: p.disabled_duration,
                target_scale_max: p.target_scale_max,
                target_scale_min: p.target_scale_min,
                start_color: p.start_color,
                end_color: p.end_color,
                disable_fx_particle_system: p.disable_fx_particle_system,
                sparks_per_cubic_foot: p.sparks_per_cubic_foot,
                effect_radius: p.effect_radius,
                does_not_affect: p.does_not_affect,
                does_not_affect_my_own_buildings: p.does_not_affect_my_own_buildings,
                victim_required_kind_of: p.victim_required_kind_of,
                victim_forbidden_kind_of: p.victim_forbidden_kind_of,
            }
        }
        "DeletionUpdate" => {
            let (mut min_lifetime, mut max_lifetime) = (0, 0);
            fields(block, d, class, |k, a| {
                match k {
                    "MinLifetime" => min_lifetime = a.duration_frames()?,
                    "MaxLifetime" => max_lifetime = a.duration_frames()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::Deletion { min_lifetime, max_lifetime }
        }
        "AnimationSteeringUpdate" => {
            let mut min_transition_time = 0;
            fields(block, d, class, |k, a| {
                match k {
                    "MinTransitionTime" => min_transition_time = a.duration_frames()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::AnimationSteering { min_transition_time }
        }
        "FireOCLAfterWeaponCooldownUpdate" => {
            let mut p = FireOcl::default();
            let mux = parse_own_mux(block, d, class, |k, a| {
                match k {
                    "WeaponSlot" => p.weapon_slot = a.index()?,
                    "OCL" => p.ocl = name(a)?,
                    "MinShotsToCreateOCL" => p.min_shots = a.uint()?,
                    "OCLLifetimePerSecond" => p.ocl_lifetime_per_second = a.uint()?,
                    "OCLLifetimeMaxCap" => p.ocl_lifetime_max_cap = a.duration_frames()?,
                    _ => return Ok(false),
                }
                Ok(true)
            });
            LogicModule::FireOclAfterWeaponCooldown {
                weapon_slot: p.weapon_slot,
                ocl: p.ocl,
                min_shots_to_create_ocl: p.min_shots,
                ocl_lifetime_per_second: p.ocl_lifetime_per_second,
                ocl_lifetime_max_cap: p.ocl_lifetime_max_cap,
                mux,
            }
        }
        "SpyVisionUpdate" => {
            let mut p = SpyVision::default();
            let mux = parse_own_mux(block, d, class, |k, a| {
                match k {
                    "NeedsUpgrade" => p.needs_upgrade = a.boolean()?,
                    "SelfPowered" => p.self_powered = a.boolean()?,
                    "SelfPoweredDuration" => p.self_powered_duration = a.duration_frames()?,
                    "SelfPoweredInterval" => p.self_powered_interval = a.duration_frames()?,
                    "SpyOnKindof" => p.spy_on_kind_of = p.spy_on_kind_of.parse(a.rest())?,
                    _ => return Ok(false),
                }
                Ok(true)
            });
            LogicModule::SpyVisionUpdate {
                needs_upgrade: p.needs_upgrade,
                self_powered: p.self_powered,
                self_powered_duration: p.self_powered_duration,
                self_powered_interval: p.self_powered_interval,
                spy_on_kind_of: p.spy_on_kind_of,
                mux,
            }
        }
        "BoneFXUpdate" => parse_bone_fx(block, d),
        "FireSpreadUpdate" => {
            let mut ocl_embers = None;
            let (mut min_spread_delay, mut max_spread_delay, mut spread_try_range) = (0, 0, 0.0);
            fields(block, d, class, |k, a| {
                match k {
                    "OCLEmbers" => ocl_embers = name(a)?,
                    "MinSpreadDelay" => min_spread_delay = a.duration_frames()?,
                    "MaxSpreadDelay" => max_spread_delay = a.duration_frames()?,
                    "SpreadTryRange" => spread_try_range = a.real()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::FireSpread { ocl_embers, min_spread_delay, max_spread_delay, spread_try_range }
        }
        "RadarUpdate" => {
            let mut radar_extend_time = 0.0;
            fields(block, d, class, |k, a| {
                match k {
                    "RadarExtendTime" => radar_extend_time = a.duration_real()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::RadarUpdate { radar_extend_time }
        }
        "ProjectileStreamUpdate" => LogicModule::ProjectileStreamUpdate,
        "RadiusDecalUpdate" => LogicModule::RadiusDecalUpdate,
        "TechBuildingBehavior" => {
            let mut pulse_fx = None;
            let mut pulse_fx_rate = 0;
            fields(block, d, class, |k, a| {
                match k {
                    "PulseFX" => pulse_fx = name(a)?,
                    "PulseFXRate" => pulse_fx_rate = a.duration_frames()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::TechBuilding { pulse_fx, pulse_fx_rate }
        }
        "DynamicShroudClearingRangeUpdate" => parse_dynamic_shroud(block, d),
        "FireWeaponWhenDamagedBehavior" => parse_fire_weapon_when_damaged(block, d),
        "FloatUpdate" => {
            let mut enabled = false;
            fields(block, d, class, |k, a| {
                match k {
                    "Enabled" => enabled = a.boolean()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::Float { enabled }
        }
        "OCLUpdate" => {
            let mut p = OclUpdate::default();
            fields(block, d, class, |k, a| {
                match k {
                    "OCL" => p.ocl = name(a)?,
                    "FactionOCL" => parse_faction_ocl(a, &mut p.faction_ocl),
                    "MinDelay" => p.min_delay = a.duration_frames()?,
                    "MaxDelay" => p.max_delay = a.duration_frames()?,
                    "CreateAtEdge" => p.create_at_edge = a.boolean()?,
                    "FactionTriggered" => p.faction_triggered = a.boolean()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::OclUpdate {
                ocl: p.ocl,
                faction_ocl: p.faction_ocl,
                min_delay: p.min_delay,
                max_delay: p.max_delay,
                create_at_edge: p.create_at_edge,
                faction_triggered: p.faction_triggered,
            }
        }
        "BattlePlanUpdate" => {
            let mut p = BattlePlan {
                armor_scalar: 1.0,
                sad_sight: 1.0,
                strategy_sad_sight: 1.0,
                strategy_sad_stealth: true,
                strategy_hold_health: 1.0,
                strategy_hold_change: "PRESERVE_RATIO".to_string(),
                ..Default::default()
            };
            fields(block, d, class, |k, a| {
                match k {
                    "SpecialPowerTemplate" => p.special_power = name(a)?,
                    "BombardmentPlanAnimationTime" => p.bombardment_animation = a.duration_frames()?,
                    "HoldTheLinePlanAnimationTime" => p.hold_animation = a.duration_frames()?,
                    "SearchAndDestroyPlanAnimationTime" => p.sad_animation = a.duration_frames()?,
                    "TransitionIdleTime" => p.transition_idle = a.duration_frames()?,
                    "BombardmentPlanUnpackSoundName" => p.bombardment_unpack = a.next()?.to_string(),
                    "BombardmentPlanPackSoundName" => p.bombardment_pack = a.next()?.to_string(),
                    "BombardmentMessageLabel" => p.bombardment_label = a.next()?.to_string(),
                    "BombardmentAnnouncementName" => p.bombardment_announce = a.next()?.to_string(),
                    "SearchAndDestroyPlanUnpackSoundName" => p.sad_unpack = a.next()?.to_string(),
                    "SearchAndDestroyPlanIdleLoopSoundName" => p.sad_idle = a.next()?.to_string(),
                    "SearchAndDestroyPlanPackSoundName" => p.sad_pack = a.next()?.to_string(),
                    "SearchAndDestroyMessageLabel" => p.sad_label = a.next()?.to_string(),
                    "SearchAndDestroyAnnouncementName" => p.sad_announce = a.next()?.to_string(),
                    "HoldTheLinePlanUnpackSoundName" => p.hold_unpack = a.next()?.to_string(),
                    "HoldTheLinePlanPackSoundName" => p.hold_pack = a.next()?.to_string(),
                    "HoldTheLineMessageLabel" => p.hold_label = a.next()?.to_string(),
                    "HoldTheLineAnnouncementName" => p.hold_announce = a.next()?.to_string(),
                    "ValidMemberKindOf" => p.valid_kind_of = p.valid_kind_of.parse(a.rest())?,
                    "InvalidMemberKindOf" => p.invalid_kind_of = p.invalid_kind_of.parse(a.rest())?,
                    "BattlePlanChangeParalyzeTime" => p.paralyze = a.duration_frames()?,
                    "HoldTheLinePlanArmorDamageScalar" => p.armor_scalar = a.real()?,
                    "SearchAndDestroyPlanSightRangeScalar" => p.sad_sight = a.real()?,
                    "StrategyCenterSearchAndDestroySightRangeScalar" => p.strategy_sad_sight = a.real()?,
                    "StrategyCenterSearchAndDestroyDetectsStealth" => p.strategy_sad_stealth = a.boolean()?,
                    "StrategyCenterHoldTheLineMaxHealthScalar" => p.strategy_hold_health = a.real()?,
                    "StrategyCenterHoldTheLineMaxHealthChangeType" => p.strategy_hold_change = a.next()?.to_string(),
                    "VisionObjectName" => p.vision_object = a.next()?.to_string(),
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::BattlePlan {
                special_power: p.special_power,
                bombardment_plan_animation_time: p.bombardment_animation,
                hold_the_line_plan_animation_time: p.hold_animation,
                search_and_destroy_plan_animation_time: p.sad_animation,
                transition_idle_time: p.transition_idle,
                bombardment_plan_unpack_sound_name: p.bombardment_unpack,
                bombardment_plan_pack_sound_name: p.bombardment_pack,
                bombardment_message_label: p.bombardment_label,
                bombardment_announcement_name: p.bombardment_announce,
                search_and_destroy_plan_unpack_sound_name: p.sad_unpack,
                search_and_destroy_plan_idle_loop_sound_name: p.sad_idle,
                search_and_destroy_plan_pack_sound_name: p.sad_pack,
                search_and_destroy_message_label: p.sad_label,
                search_and_destroy_announcement_name: p.sad_announce,
                hold_the_line_plan_unpack_sound_name: p.hold_unpack,
                hold_the_line_plan_pack_sound_name: p.hold_pack,
                hold_the_line_message_label: p.hold_label,
                hold_the_line_announcement_name: p.hold_announce,
                valid_member_kind_of: p.valid_kind_of,
                invalid_member_kind_of: p.invalid_kind_of,
                battle_plan_change_paralyze_time: p.paralyze,
                hold_the_line_plan_armor_damage_scalar: p.armor_scalar,
                search_and_destroy_plan_sight_range_scalar: p.sad_sight,
                strategy_center_search_and_destroy_sight_range_scalar: p.strategy_sad_sight,
                strategy_center_search_and_destroy_detects_stealth: p.strategy_sad_stealth,
                strategy_center_hold_the_line_max_health_scalar: p.strategy_hold_health,
                strategy_center_hold_the_line_max_health_change_type: p.strategy_hold_change,
                vision_object_name: p.vision_object,
            }
        }
        "NeutronMissileUpdate" => parse_neutron_missile(block, d),
        "WeaponBonusUpdate" => {
            let mut p = WeaponBonus::default();
            fields(block, d, class, |k, a| {
                match k {
                    "RequiredAffectKindOf" => p.required_kind_of = p.required_kind_of.parse(a.rest())?,
                    "ForbiddenAffectKindOf" => p.forbidden_kind_of = p.forbidden_kind_of.parse(a.rest())?,
                    "BonusDuration" => p.bonus_duration = a.duration_frames()?,
                    "BonusDelay" => p.bonus_delay = a.duration_frames()?,
                    "BonusRange" => p.bonus_range = a.real()?,
                    "BonusConditionType" => p.bonus_condition_type = a.next()?.to_string(),
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::WeaponBonusUpdate {
                required_affect_kind_of: p.required_kind_of,
                forbidden_affect_kind_of: p.forbidden_kind_of,
                bonus_duration: p.bonus_duration,
                bonus_delay: p.bonus_delay,
                bonus_range: p.bonus_range,
                bonus_condition_type: p.bonus_condition_type,
            }
        }
        "FirestormDynamicGeometryInfoUpdate" => parse_firestorm(block, d),
        "NeutronBlastBehavior" => {
            let mut blast_radius = 10.0;
            let mut affect_airborne = true;
            let mut affect_allies = true;
            fields(block, d, class, |k, a| {
                match k {
                    "BlastRadius" => blast_radius = a.real()?,
                    "AffectAirborne" => affect_airborne = a.boolean()?,
                    "AffectAllies" => affect_allies = a.boolean()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::NeutronBlast { blast_radius, affect_airborne, affect_allies }
        }
        "TensileFormationUpdate" => {
            let mut enabled = false;
            let mut crack_sound = None;
            fields(block, d, class, |k, a| {
                match k {
                    "Enabled" => enabled = a.boolean()?,
                    "CrackSound" => crack_sound = name(a)?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::TensileFormation { enabled, crack_sound }
        }
        "WaveGuideUpdate" => {
            let mut p = WaveGuide::default();
            fields(block, d, class, |k, a| {
                match k {
                    "WaveDelay" => p.wave_delay = a.duration_real()?,
                    "YSize" => p.y_size = a.real()?,
                    "LinearWaveSpacing" => p.linear_wave_spacing = a.real()?,
                    "WaveBendMagnitude" => p.wave_bend_magnitude = a.real()?,
                    "WaterVelocity" => p.water_velocity = a.velocity()?,
                    "PreferredHeight" => p.preferred_height = a.real()?,
                    "ShorelineEffectDistance" => p.shoreline_effect_distance = a.real()?,
                    "DamageRadius" => p.damage_radius = a.real()?,
                    "DamageAmount" => p.damage_amount = a.real()?,
                    "ToppleForce" => p.topple_force = a.real()?,
                    "RandomSplashSound" => p.random_splash_sound = name(a)?,
                    "RandomSplashSoundFrequency" => p.random_splash_sound_frequency = a.int()?,
                    "BridgeParticle" => p.bridge_particle = name(a)?,
                    "BridgeParticleAngleFudge" => p.bridge_particle_angle_fudge = a.angle()?,
                    "LoopingSound" => p.looping_sound = name(a)?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::WaveGuide {
                wave_delay: p.wave_delay,
                y_size: p.y_size,
                linear_wave_spacing: p.linear_wave_spacing,
                wave_bend_magnitude: p.wave_bend_magnitude,
                water_velocity: p.water_velocity,
                preferred_height: p.preferred_height,
                shoreline_effect_distance: p.shoreline_effect_distance,
                damage_radius: p.damage_radius,
                damage_amount: p.damage_amount,
                topple_force: p.topple_force,
                random_splash_sound: p.random_splash_sound,
                random_splash_sound_frequency: p.random_splash_sound_frequency,
                bridge_particle: p.bridge_particle,
                bridge_particle_angle_fudge: p.bridge_particle_angle_fudge,
                looping_sound: p.looping_sound,
            }
        }
        "BunkerBusterBehavior" => {
            let mut p = BunkerBuster::default();
            fields(block, d, class, |k, a| {
                match k {
                    "UpgradeRequired" => p.upgrade_required = name(a)?,
                    "DetonationFX" => p.detonation_fx = name(a)?,
                    "CrashThroughBunkerFX" => p.crash_through_fx = name(a)?,
                    "CrashThroughBunkerFXFrequency" => p.crash_through_frequency = a.duration_frames()?,
                    "SeismicEffectRadius" => p.seismic_radius = a.real()?,
                    "SeismicEffectMagnitude" => p.seismic_magnitude = a.real()?,
                    "ShockwaveWeaponTemplate" => p.shockwave_weapon = name(a)?,
                    "OccupantDamageWeaponTemplate" => p.occupant_damage_weapon = name(a)?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::BunkerBuster {
                upgrade_required: p.upgrade_required,
                detonation_fx: p.detonation_fx,
                crash_through_bunker_fx: p.crash_through_fx,
                crash_through_bunker_fx_frequency: p.crash_through_frequency,
                seismic_effect_radius: p.seismic_radius,
                seismic_effect_magnitude: p.seismic_magnitude,
                shockwave_weapon_template: p.shockwave_weapon,
                occupant_damage_weapon_template: p.occupant_damage_weapon,
            }
        }
        "CheckpointUpdate" => LogicModule::Checkpoint { scan_delay_time: parse_scan_delay(block, d, class) },
        "EnemyNearUpdate" => LogicModule::EnemyNear { scan_delay_time: parse_scan_delay(block, d, class) },
        "LeafletDropBehavior" => {
            let mut p = LeafletDrop::default();
            fields(block, d, class, |k, a| {
                match k {
                    "Delay" => p.delay = a.duration_frames()?,
                    "DisabledDuration" => p.disabled_duration = a.duration_frames()?,
                    "AffectRadius" => p.affect_radius = a.real()?,
                    "LeafletFXParticleSystem" => p.leaflet_fx = name(a)?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::LeafletDrop {
                delay: p.delay,
                disabled_duration: p.disabled_duration,
                affect_radius: p.affect_radius,
                leaflet_fx_particle_system: p.leaflet_fx,
            }
        }
        "StickyBombUpdate" => {
            let mut attach_to_target_bone = String::new();
            let mut offset_z = 10.0;
            let mut geometry_based_damage_weapon = None;
            let mut geometry_based_damage_fx = None;
            fields(block, d, class, |k, a| {
                match k {
                    "AttachToTargetBone" => attach_to_target_bone = a.next()?.to_string(),
                    "OffsetZ" => offset_z = a.real()?,
                    "GeometryBasedDamageWeapon" => geometry_based_damage_weapon = name(a)?,
                    "GeometryBasedDamageFX" => geometry_based_damage_fx = name(a)?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::StickyBomb { attach_to_target_bone, offset_z, geometry_based_damage_weapon, geometry_based_damage_fx }
        }
        "SupplyWarehouseCripplingBehavior" => {
            let (mut sup, mut delay, mut amount) = (0, 0, 0.0);
            fields(block, d, class, |k, a| {
                match k {
                    "SelfHealSupression" => sup = a.duration_frames()?,
                    "SelfHealDelay" => delay = a.duration_frames()?,
                    "SelfHealAmount" => amount = a.real()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::SupplyWarehouseCrippling { self_heal_supression: sup, self_heal_delay: delay, self_heal_amount: amount }
        }
        "BridgeBehavior" => parse_bridge(block, d),
        "BridgeTowerBehavior" => LogicModule::BridgeTower,
        "BridgeScaffoldBehavior" => LogicModule::BridgeScaffold,
        "BoneFXDamage" => LogicModule::BoneFxDamage,
        "ConvertToCarBombCrateCollide"
        | "ConvertToHijackedVehicleCrateCollide"
        | "SabotageCommandCenterCrateCollide"
        | "SabotageMilitaryFactoryCrateCollide"
        | "SabotagePowerPlantCrateCollide"
        | "SabotageSuperweaponCrateCollide"
        | "SabotageSupplyCenterCrateCollide"
        | "SabotageFakeBuildingCrateCollide"
        | "SabotageInternetCenterCrateCollide" => parse_crate_collide(class, block, d),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// Complex / indexed parsers

fn parse_own_mux(
    block: &Block,
    d: &mut Diag,
    scope: &str,
    mut own: impl FnMut(&str, &mut Args) -> Result<bool, String>,
) -> UpgradeMuxData {
    let mut mux = UpgradeMuxData::default();
    fields(block, d, scope, |k, a| {
        if own(k, a)? {
            return Ok(true);
        }
        if mux.field(k, a)? {
            return Ok(true);
        }
        let _ = a.rest();
        Ok(true)
    });
    mux
}

fn parse_scan_delay(block: &Block, d: &mut Diag, scope: &str) -> u32 {
    let mut v = crate::value::LOGICFRAMES_PER_SECOND;
    fields(block, d, scope, |k, a| {
        match k {
            "ScanDelayTime" => v = a.duration_frames()?,
            _ => {
                let _ = a.rest();
            }
        }
        Ok(true)
    });
    v
}

fn parse_faction_ocl(a: &mut Args, out: &mut Vec<(String, String)>) {
    let mut faction = String::new();
    let mut ocl = String::new();
    let toks = a.rest();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if let Some(rest) = t.strip_prefix("Faction:") {
            faction = rest.to_string();
        } else if let Some(rest) = t.strip_prefix("OCL:") {
            ocl = rest.to_string();
        } else if t.eq_ignore_ascii_case("Faction") {
            faction = toks.get(i + 1).copied().unwrap_or("").to_string();
            i += 1;
        } else if t.eq_ignore_ascii_case("OCL") {
            ocl = toks.get(i + 1).copied().unwrap_or("").to_string();
            i += 1;
        }
        i += 1;
    }
    if !faction.is_empty() || !ocl.is_empty() {
        out.push((faction, ocl));
    }
}

fn parse_bone_fx(block: &Block, d: &mut Diag) -> LogicModule {
    let mut damage_fx_types: Mask<DamageType> = DamageType::ALL.iter().copied().collect();
    let mut damage_ocl_types: Mask<DamageType> = DamageType::ALL.iter().copied().collect();
    let mut damage_particle_types: Mask<DamageType> = DamageType::ALL.iter().copied().collect();
    let mut entries = Vec::new();
    fields(block, d, "BoneFXUpdate", |k, a| {
        match k {
            "DamageFXTypes" => damage_fx_types = crate::value::parse_flag_list_all(a.rest())?,
            "DamageOCLTypes" => damage_ocl_types = crate::value::parse_flag_list_all(a.rest())?,
            "DamageParticleTypes" => damage_particle_types = crate::value::parse_flag_list_all(a.rest())?,
            _ => {
                if let Some((state, slot, kind)) = bone_fx_key(k) {
                    entries.push(parse_bone_fx_entry(state, slot, kind, a)?);
                    return Ok(true);
                }
                let _ = a.rest();
            }
        }
        Ok(true)
    });
    LogicModule::BoneFxUpdate { damage_fx_types, damage_ocl_types, damage_particle_types, entries }
}

/// Decode `PristineFXList3` / `DamagedOCL2` / `RubbleParticleSystem4` into (state, slot, kind).
fn bone_fx_key(key: &str) -> Option<(u8, u8, u8)> {
    let (state, rest) = if let Some(r) = key.strip_prefix("ReallyDamaged") {
        (2u8, r)
    } else if let Some(r) = key.strip_prefix("Damaged") {
        (1u8, r)
    } else if let Some(r) = key.strip_prefix("Pristine") {
        (0u8, r)
    } else {
        (3u8, key.strip_prefix("Rubble")?)
    };
    let (kind, suffix) = if let Some(r) = rest.strip_prefix("ParticleSystem") {
        (2u8, r)
    } else if let Some(r) = rest.strip_prefix("FXList") {
        (0u8, r)
    } else {
        (1u8, rest.strip_prefix("OCL")?)
    };
    let slot: u8 = suffix.parse().ok()?;
    if !(1..=8).contains(&slot) {
        return None;
    }
    Some((state, slot, kind))
}

fn parse_bone_fx_entry(state: u8, slot: u8, kind: u8, a: &mut Args) -> Result<BoneFxEntry, String> {
    let mut e = BoneFxEntry {
        state,
        slot,
        kind,
        bone: String::new(),
        only_once: true,
        min_delay: 0.0,
        max_delay: 0.0,
        name: None,
    };
    let toks: Vec<String> = a.rest().iter().map(|s| s.to_string()).collect();
    let mut i = 0;
    let mut delays = 0;
    while i < toks.len() {
        let t = &toks[i];
        if let Some(v) = t.strip_prefix("Bone:") {
            e.bone = v.to_string();
            if e.bone.is_empty() {
                e.bone = toks.get(i + 1).cloned().unwrap_or_default();
                i += 1;
            }
        } else if let Some(v) = t.strip_prefix("OnlyOnce:") {
            e.only_once = v.eq_ignore_ascii_case("yes") || v == "1";
        } else if let Some(v) = t.strip_prefix("FXList:") {
            e.name = opt_name(v);
        } else if let Some(v) = t.strip_prefix("OCL:") {
            e.name = opt_name(v);
        } else if let Some(v) = t.strip_prefix("ParticleSystem:") {
            e.name = opt_name(v);
        } else if let Ok(d) = t.parse::<f32>() {
            if delays == 0 {
                e.min_delay = crate::value::duration_ms_to_frames(d);
            } else if delays == 1 {
                e.max_delay = crate::value::duration_ms_to_frames(d);
            }
            delays += 1;
        }
        i += 1;
    }
    Ok(e)
}

fn parse_spectre_gunship(block: &Block, d: &mut Diag) -> LogicModule {
    let mut p = SpectreGunshipData {
        attack_area_radius: 200.0,
        targeting_reticle_radius: 25.0,
        gunship_orbit_radius: 250.0,
        strafing_increment: 20.0,
        orbit_insertion_slope: 0.7,
        howitzer_firing_rate: 10,
        random_offset_for_howitzer: 20.0,
        ..Default::default()
    };
    for item in &block.items {
        match item {
            Item::Block(b) if b.kind == "AttackAreaDecal" => p.attack_area_decal = Some(parse_radius_decal(b, d)),
            Item::Block(b) if b.kind == "TargetingReticleDecal" => p.targeting_reticle_decal = Some(parse_radius_decal(b, d)),
            Item::Block(b) => d.warn(WarningKind::Structure, &b.kind, b.line, "unexpected nested block in SpectreGunshipUpdate"),
            Item::Field(f) => {
                let mut a = Args::new(&f.value);
                let r: Result<(), String> = (|| {
                    match f.key.as_str() {
                        "SpecialPowerTemplate" => p.special_power = name(&mut a)?,
                        "GattlingTemplateName" => p.gattling_template_name = a.next()?.to_string(),
                        "HowitzerFiringRate" => p.howitzer_firing_rate = a.duration_frames()?,
                        "OrbitTime" => p.orbit_time = a.duration_frames()?,
                        "HowitzerFollowLag" => p.howitzer_follow_lag = a.duration_frames()?,
                        "AttackAreaRadius" => p.attack_area_radius = a.real()?,
                        "StrafingIncrement" => p.strafing_increment = a.real()?,
                        "OrbitInsertionSlope" => p.orbit_insertion_slope = a.real()?,
                        "RandomOffsetForHowitzer" => p.random_offset_for_howitzer = a.real()?,
                        "TargetingReticleRadius" => p.targeting_reticle_radius = a.real()?,
                        "GunshipOrbitRadius" => p.gunship_orbit_radius = a.real()?,
                        "HowitzerWeaponTemplate" => p.howitzer_weapon_template = name(&mut a)?,
                        "GattlingStrafeFXParticleSystem" => p.gattling_strafe_fx = name(&mut a)?,
                        _ => {
                            let _ = a.rest();
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = r {
                    d.bad_value(f, e);
                }
            }
        }
    }
    LogicModule::SpectreGunship {
        special_power: p.special_power,
        gattling_template_name: p.gattling_template_name,
        howitzer_firing_rate: p.howitzer_firing_rate,
        orbit_time: p.orbit_time,
        howitzer_follow_lag: p.howitzer_follow_lag,
        attack_area_radius: p.attack_area_radius,
        strafing_increment: p.strafing_increment,
        orbit_insertion_slope: p.orbit_insertion_slope,
        random_offset_for_howitzer: p.random_offset_for_howitzer,
        targeting_reticle_radius: p.targeting_reticle_radius,
        gunship_orbit_radius: p.gunship_orbit_radius,
        howitzer_weapon_template: p.howitzer_weapon_template,
        gattling_strafe_fx: p.gattling_strafe_fx,
        attack_area_decal: p.attack_area_decal,
        targeting_reticle_decal: p.targeting_reticle_decal,
    }
}

fn parse_dynamic_shroud(block: &Block, d: &mut Diag) -> LogicModule {
    let mut p = DynamicShroud::default();
    for item in &block.items {
        match item {
            Item::Block(b) if b.kind == "GridDecalTemplate" => p.grid_decal = Some(parse_radius_decal(b, d)),
            Item::Block(b) => d.warn(WarningKind::Structure, &b.kind, b.line, "unexpected nested block in DynamicShroudClearingRangeUpdate"),
            Item::Field(f) => {
                let mut a = Args::new(&f.value);
                let r: Result<(), String> = (|| {
                    match f.key.as_str() {
                        "ChangeInterval" => p.change_interval = a.duration_frames()?,
                        "GrowInterval" => p.grow_interval = a.duration_frames()?,
                        "ShrinkDelay" => p.shrink_delay = a.duration_frames()?,
                        "ShrinkTime" => p.shrink_time = a.duration_frames()?,
                        "GrowDelay" => p.grow_delay = a.duration_frames()?,
                        "GrowTime" => p.grow_time = a.duration_frames()?,
                        "FinalVision" => p.final_vision = a.real()?,
                        _ => {
                            let _ = a.rest();
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = r {
                    d.bad_value(f, e);
                }
            }
        }
    }
    LogicModule::DynamicShroudClearingRange {
        change_interval: p.change_interval,
        grow_interval: p.grow_interval,
        shrink_delay: p.shrink_delay,
        shrink_time: p.shrink_time,
        grow_delay: p.grow_delay,
        grow_time: p.grow_time,
        final_vision: p.final_vision,
        grid_decal: p.grid_decal,
    }
}

fn parse_neutron_missile(block: &Block, d: &mut Diag) -> LogicModule {
    let mut p = NeutronMissileData {
        max_turn_rate: 999.0,
        relative_speed: 1.0,
        special_accel_factor: 1.0,
        ..Default::default()
    };
    for item in &block.items {
        match item {
            Item::Block(b) if b.kind == "DeliveryDecal" => p.delivery_decal = Some(parse_radius_decal(b, d)),
            Item::Block(b) => d.warn(WarningKind::Structure, &b.kind, b.line, "unexpected nested block in NeutronMissileUpdate"),
            Item::Field(f) => {
                let mut a = Args::new(&f.value);
                let r: Result<(), String> = (|| {
                    match f.key.as_str() {
                        "DistanceToTravelBeforeTurning" => p.initial_dist = a.real()?,
                        "MaxTurnRate" => p.max_turn_rate = a.angular_velocity()?,
                        "ForwardDamping" => p.forward_damping = a.real()?,
                        "RelativeSpeed" => p.relative_speed = a.real()?,
                        "TargetFromDirectlyAbove" => p.target_from_directly_above = a.real()?,
                        "LaunchFX" => p.launch_fx = name(&mut a)?,
                        "SpecialSpeedTime" => p.special_speed_time = a.duration_frames()?,
                        "SpecialSpeedHeight" => p.special_speed_height = a.real()?,
                        "SpecialAccelFactor" => p.special_accel_factor = a.real()?,
                        "SpecialJitterDistance" => p.special_jitter_distance = a.real()?,
                        "IgnitionFX" => p.ignition_fx = name(&mut a)?,
                        "DeliveryDecalRadius" => p.delivery_decal_radius = a.real()?,
                        _ => {
                            let _ = a.rest();
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = r {
                    d.bad_value(f, e);
                }
            }
        }
    }
    LogicModule::NeutronMissile {
        distance_to_travel_before_turning: p.initial_dist,
        max_turn_rate: p.max_turn_rate,
        forward_damping: p.forward_damping,
        relative_speed: p.relative_speed,
        target_from_directly_above: p.target_from_directly_above,
        launch_fx: p.launch_fx,
        special_speed_time: p.special_speed_time,
        special_speed_height: p.special_speed_height,
        special_accel_factor: p.special_accel_factor,
        special_jitter_distance: p.special_jitter_distance,
        ignition_fx: p.ignition_fx,
        delivery_decal: p.delivery_decal,
        delivery_decal_radius: p.delivery_decal_radius,
    }
}

fn parse_particle_uplink(block: &Block, d: &mut Diag) -> LogicModule {
    let mut p = ParticleUplink {
        scorch_scalar: 1.0,
        damage_radius_scalar: 1.0,
        delay_between_launch_fx: 30,
        damage_type: Mask::EMPTY.with(DamageType::Laser),
        death_type: "LASERED".to_string(),
        ..Default::default()
    };
    fields(block, d, "ParticleUplinkCannonUpdate", |k, a| {
        match k {
            "SpecialPowerTemplate" => p.special_power = name(a)?,
            "BeginChargeTime" => p.begin_charge = a.duration_frames()?,
            "RaiseAntennaTime" => p.raise_antenna = a.duration_frames()?,
            "ReadyDelayTime" => p.ready_delay = a.duration_frames()?,
            "WidthGrowTime" => p.width_grow = a.duration_frames()?,
            "BeamTravelTime" => p.beam_travel = a.duration_frames()?,
            "TotalFiringTime" => p.total_firing = a.duration_frames()?,
            "RevealRange" => p.reveal_range = a.real()?,
            "OuterEffectBoneName" => p.outer_bone = a.next()?.to_string(),
            "OuterEffectNumBones" => p.outer_num_bones = a.uint()?,
            "OuterNodesLightFlareParticleSystem" => p.outer_light = a.next()?.to_string(),
            "OuterNodesMediumFlareParticleSystem" => p.outer_medium = a.next()?.to_string(),
            "OuterNodesIntenseFlareParticleSystem" => p.outer_intense = a.next()?.to_string(),
            "ConnectorBoneName" => p.connector_bone = a.next()?.to_string(),
            "ConnectorMediumLaserName" => p.connector_medium_laser = a.next()?.to_string(),
            "ConnectorIntenseLaserName" => p.connector_intense_laser = a.next()?.to_string(),
            "ConnectorMediumFlare" => p.connector_medium_flare = a.next()?.to_string(),
            "ConnectorIntenseFlare" => p.connector_intense_flare = a.next()?.to_string(),
            "FireBoneName" => p.fire_bone = a.next()?.to_string(),
            "LaserBaseLightFlareParticleSystemName" => p.laser_light = a.next()?.to_string(),
            "LaserBaseMediumFlareParticleSystemName" => p.laser_medium = a.next()?.to_string(),
            "LaserBaseIntenseFlareParticleSystemName" => p.laser_intense = a.next()?.to_string(),
            "ParticleBeamLaserName" => p.particle_beam_laser = a.next()?.to_string(),
            "SwathOfDeathDistance" => p.swath_distance = a.real()?,
            "SwathOfDeathAmplitude" => p.swath_amplitude = a.real()?,
            "TotalScorchMarks" => p.total_scorch = a.uint()?,
            "ScorchMarkScalar" => p.scorch_scalar = a.real()?,
            "BeamLaunchFX" => p.beam_launch_fx = name(a)?,
            "DelayBetweenLaunchFX" => p.delay_between_launch_fx = a.duration_frames()?,
            "GroundHitFX" => p.ground_hit_fx = name(a)?,
            "DamagePerSecond" => p.damage_per_second = a.real()?,
            "TotalDamagePulses" => p.total_damage_pulses = a.uint()?,
            "DamageType" => p.damage_type = p.damage_type.parse(a.rest())?,
            "DeathType" => p.death_type = a.next()?.to_string(),
            "DamageRadiusScalar" => p.damage_radius_scalar = a.real()?,
            "PoweringUpSoundLoop" => p.power_up_sound = name(a)?,
            "UnpackToIdleSoundLoop" => p.unpack_sound = name(a)?,
            "FiringToPackSoundLoop" => p.firing_sound = name(a)?,
            "GroundAnnihilationSoundLoop" => p.annihilation_sound = name(a)?,
            "DamagePulseRemnantObjectName" => p.remnant_object = a.next()?.to_string(),
            "ManualDrivingSpeed" => p.manual_speed = a.real()?,
            "ManualFastDrivingSpeed" => p.manual_fast_speed = a.real()?,
            "DoubleClickToFastDriveDelay" => p.double_click_delay = a.uint()?,
            _ => {
                let _ = a.rest();
            }
        }
        Ok(true)
    });
    LogicModule::ParticleUplinkCannon {
        special_power: p.special_power,
        begin_charge_frames: p.begin_charge,
        raise_antenna_frames: p.raise_antenna,
        ready_delay_frames: p.ready_delay,
        width_grow_frames: p.width_grow,
        beam_travel_frames: p.beam_travel,
        total_firing_frames: p.total_firing,
        reveal_range: p.reveal_range,
        outer_effect_bone_name: p.outer_bone,
        outer_effect_num_bones: p.outer_num_bones,
        outer_nodes_light_flare_particle_system: p.outer_light,
        outer_nodes_medium_flare_particle_system: p.outer_medium,
        outer_nodes_intense_flare_particle_system: p.outer_intense,
        connector_bone_name: p.connector_bone,
        connector_medium_laser_name: p.connector_medium_laser,
        connector_intense_laser_name: p.connector_intense_laser,
        connector_medium_flare: p.connector_medium_flare,
        connector_intense_flare: p.connector_intense_flare,
        fire_bone_name: p.fire_bone,
        laser_base_light_flare_particle_system: p.laser_light,
        laser_base_medium_flare_particle_system: p.laser_medium,
        laser_base_intense_flare_particle_system: p.laser_intense,
        particle_beam_laser_name: p.particle_beam_laser,
        swath_of_death_distance: p.swath_distance,
        swath_of_death_amplitude: p.swath_amplitude,
        total_scorch_marks: p.total_scorch,
        scorch_mark_scalar: p.scorch_scalar,
        beam_launch_fx: p.beam_launch_fx,
        delay_between_launch_fx: p.delay_between_launch_fx,
        ground_hit_fx: p.ground_hit_fx,
        damage_per_second: p.damage_per_second,
        total_damage_pulses: p.total_damage_pulses,
        damage_type: p.damage_type,
        death_type: p.death_type,
        damage_radius_scalar: p.damage_radius_scalar,
        powering_up_sound_loop: p.power_up_sound,
        unpack_to_idle_sound_loop: p.unpack_sound,
        firing_to_pack_sound_loop: p.firing_sound,
        ground_annihilation_sound_loop: p.annihilation_sound,
        damage_pulse_remnant_object_name: p.remnant_object,
        manual_driving_speed: p.manual_speed,
        manual_fast_driving_speed: p.manual_fast_speed,
        double_click_to_fast_drive_delay: p.double_click_delay,
    }
}

fn parse_fire_weapon_when_damaged(block: &Block, d: &mut Diag) -> LogicModule {
    let mut p = FireWeaponWhenDamaged::default();
    let mux = parse_own_mux(block, d, "FireWeaponWhenDamagedBehavior", |k, a| {
        match k {
            "StartsActive" => p.starts_active = a.boolean()?,
            "ReactionWeaponPristine" => p.reaction_pristine = name(a)?,
            "ReactionWeaponDamaged" => p.reaction_damaged = name(a)?,
            "ReactionWeaponReallyDamaged" => p.reaction_really_damaged = name(a)?,
            "ReactionWeaponRubble" => p.reaction_rubble = name(a)?,
            "ContinuousWeaponPristine" => p.continuous_pristine = name(a)?,
            "ContinuousWeaponDamaged" => p.continuous_damaged = name(a)?,
            "ContinuousWeaponReallyDamaged" => p.continuous_really_damaged = name(a)?,
            "ContinuousWeaponRubble" => p.continuous_rubble = name(a)?,
            "DamageTypes" => p.damage_types = crate::value::parse_flag_list_all(a.rest())?,
            "DamageAmount" => p.damage_amount = a.real()?,
            _ => return Ok(false),
        }
        Ok(true)
    });
    LogicModule::FireWeaponWhenDamaged {
        starts_active: p.starts_active,
        reaction_weapon_pristine: p.reaction_pristine,
        reaction_weapon_damaged: p.reaction_damaged,
        reaction_weapon_really_damaged: p.reaction_really_damaged,
        reaction_weapon_rubble: p.reaction_rubble,
        continuous_weapon_pristine: p.continuous_pristine,
        continuous_weapon_damaged: p.continuous_damaged,
        continuous_weapon_really_damaged: p.continuous_really_damaged,
        continuous_weapon_rubble: p.continuous_rubble,
        damage_types: p.damage_types,
        damage_amount: p.damage_amount,
        mux,
    }
}

fn parse_firestorm(block: &Block, d: &mut Diag) -> LogicModule {
    let mut p = Firestorm { particle_systems: vec![None; 16], ..Default::default() };
    fields(block, d, "FirestormDynamicGeometryInfoUpdate", |k, a| {
        match k {
            "InitialDelay" => p.initial_delay = a.duration_frames()?,
            "InitialHeight" => p.initial_height = a.real()?,
            "InitialMajorRadius" => p.initial_major_radius = a.real()?,
            "InitialMinorRadius" => p.initial_minor_radius = a.real()?,
            "FinalHeight" => p.final_height = a.real()?,
            "FinalMajorRadius" => p.final_major_radius = a.real()?,
            "FinalMinorRadius" => p.final_minor_radius = a.real()?,
            "TransitionTime" => p.transition_time = a.duration_frames()?,
            "ReverseAtTransitionTime" => p.reverse_at_transition_time = a.boolean()?,
            "DelayBetweenDamageFrames" => p.delay_between_damage_frames = a.duration_real()?,
            "DamageAmount" => p.damage_amount = a.real()?,
            "MaxHeightForDamage" => p.max_height_for_damage = a.real()?,
            "FXList" => p.fx_list = name(a)?,
            "ParticleOffsetZ" => p.particle_offset_z = a.real()?,
            "ScorchSize" => p.scorch_size = a.real()?,
            _ => {
                if let Some(rest) = k.strip_prefix("ParticleSystem") {
                    if let Ok(n) = rest.parse::<usize>() {
                        if (1..=16).contains(&n) {
                            p.particle_systems[n - 1] = name(a)?;
                            return Ok(true);
                        }
                    }
                }
                let _ = a.rest();
            }
        }
        Ok(true)
    });
    LogicModule::FirestormDynamicGeometry {
        initial_delay: p.initial_delay,
        initial_height: p.initial_height,
        initial_major_radius: p.initial_major_radius,
        initial_minor_radius: p.initial_minor_radius,
        final_height: p.final_height,
        final_major_radius: p.final_major_radius,
        final_minor_radius: p.final_minor_radius,
        transition_time: p.transition_time,
        reverse_at_transition_time: p.reverse_at_transition_time,
        delay_between_damage_frames: p.delay_between_damage_frames,
        damage_amount: p.damage_amount,
        max_height_for_damage: p.max_height_for_damage,
        particle_systems: p.particle_systems,
        fx_list: p.fx_list,
        particle_offset_z: p.particle_offset_z,
        scorch_size: p.scorch_size,
    }
}

fn parse_bridge(block: &Block, d: &mut Diag) -> LogicModule {
    let mut lateral_scaffold_speed = 1.0;
    let mut vertical_scaffold_speed = 1.0;
    let mut die_fx = Vec::new();
    let mut die_ocl = Vec::new();
    fields(block, d, "BridgeBehavior", |k, a| {
        match k {
            "LateralScaffoldSpeed" => lateral_scaffold_speed = a.velocity()?,
            "VerticalScaffoldSpeed" => vertical_scaffold_speed = a.velocity()?,
            "BridgeDieFX" => die_fx.push(parse_bridge_fx(a)?),
            "BridgeDieOCL" => die_ocl.push(parse_bridge_ocl(a)?),
            _ => {
                let _ = a.rest();
            }
        }
        Ok(true)
    });
    LogicModule::Bridge { lateral_scaffold_speed, vertical_scaffold_speed, die_fx, die_ocl }
}

fn parse_bridge_fx(a: &mut Args) -> Result<BridgeFx, String> {
    let toks: Vec<String> = a.rest().iter().map(|s| s.to_string()).collect();
    let mut fx: Option<String> = None;
    let mut bone: Option<String> = None;
    let mut delay: u32 = 0;
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if let Some(v) = t.strip_prefix("FX:") {
            fx = opt_name(v);
        } else if let Some(v) = t.strip_prefix("OCL:") {
            fx = opt_name(v);
        } else if let Some(v) = t.strip_prefix("Delay:") {
            if let Ok(ms) = v.parse::<u32>() {
                delay = crate::value::duration_ms_to_frames_ceil(ms);
            }
        } else if let Some(v) = t.strip_prefix("Bone:") {
            bone = opt_name(v);
        } else if t.eq_ignore_ascii_case("FX") {
            fx = toks.get(i + 1).and_then(|s| opt_name(s));
            i += 1;
        } else if t.eq_ignore_ascii_case("Delay") {
            if let Some(ms) = toks.get(i + 1).and_then(|s| s.parse::<u32>().ok()) {
                delay = crate::value::duration_ms_to_frames_ceil(ms);
            }
            i += 1;
        } else if t.eq_ignore_ascii_case("Bone") {
            bone = toks.get(i + 1).and_then(|s| opt_name(s));
            i += 1;
        }
        i += 1;
    }
    Ok(BridgeFx { fx, delay, bone })
}

fn parse_bridge_ocl(a: &mut Args) -> Result<BridgeOcl, String> {
    let toks: Vec<String> = a.rest().iter().map(|s| s.to_string()).collect();
    let mut ocl = None;
    let mut bone = None;
    let mut delay = 0;
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if let Some(v) = t.strip_prefix("OCL:") {
            ocl = opt_name(v);
        } else if let Some(v) = t.strip_prefix("Delay:") {
            if let Ok(ms) = v.parse::<u32>() {
                delay = crate::value::duration_ms_to_frames_ceil(ms);
            }
        } else if let Some(v) = t.strip_prefix("Bone:") {
            bone = opt_name(v);
        } else if t.eq_ignore_ascii_case("OCL") {
            ocl = toks.get(i + 1).and_then(|s| opt_name(s));
            i += 1;
        } else if t.eq_ignore_ascii_case("Delay") {
            if let Some(ms) = toks.get(i + 1).and_then(|s| s.parse::<u32>().ok()) {
                delay = crate::value::duration_ms_to_frames_ceil(ms);
            }
            i += 1;
        } else if t.eq_ignore_ascii_case("Bone") {
            bone = toks.get(i + 1).and_then(|s| opt_name(s));
            i += 1;
        }
        i += 1;
    }
    Ok(BridgeOcl { ocl, delay, bone })
}

fn parse_crate_collide(class: &str, block: &Block, d: &mut Diag) -> LogicModule {
    let mut p = CrateCollide { class_name: class.to_string(), ..Default::default() };
    fields(block, d, class, |k, a| {
        match k {
            "RequiredKindOf" => p.required_kind_of = p.required_kind_of.parse(a.rest())?,
            "ForbiddenKindOf" => p.forbidden_kind_of = p.forbidden_kind_of.parse(a.rest())?,
            "ForbidOwnerPlayer" => p.forbid_owner_player = a.boolean()?,
            "BuildingPickup" => p.building_pickup = a.boolean()?,
            "HumanOnly" => p.human_only = a.boolean()?,
            "AllowMultiPickup" => p.allow_multi_pickup = a.boolean()?,
            "PickupScience" => p.pickup_science = name(a)?,
            "ExecuteFX" => p.execute_fx = name(a)?,
            "ExecuteAnimation" => p.execute_animation = a.next()?.to_string(),
            "ExecuteAnimationTime" => p.execute_animation_time = a.real()?,
            "ExecuteAnimationZRise" => p.execute_animation_z_rise = a.real()?,
            "ExecuteAnimationFades" => p.execute_animation_fades = a.boolean()?,
            "FXList" => p.fx_list = name(a)?,
            "SabotageDuration" => p.sabotage_frames = a.duration_frames()?,
            "SabotagePowerDuration" => p.power_sabotage_frames = a.duration_frames()?,
            "StealCashAmount" => p.steal_cash_amount = a.uint()?,
            "RangeOfEffect" => p.range_of_effect = a.real()?,
            _ => {
                let _ = a.rest();
            }
        }
        Ok(true)
    });
    LogicModule::CrateCollide {
        class_name: p.class_name,
        required_kind_of: p.required_kind_of,
        forbidden_kind_of: p.forbidden_kind_of,
        forbid_owner_player: p.forbid_owner_player,
        building_pickup: p.building_pickup,
        human_only: p.human_only,
        allow_multi_pickup: p.allow_multi_pickup,
        pickup_science: p.pickup_science,
        execute_fx: p.execute_fx,
        execute_animation: p.execute_animation,
        execute_animation_time: p.execute_animation_time,
        execute_animation_z_rise: p.execute_animation_z_rise,
        execute_animation_fades: p.execute_animation_fades,
        fx_list: p.fx_list,
        sabotage_frames: p.sabotage_frames,
        power_sabotage_frames: p.power_sabotage_frames,
        steal_cash_amount: p.steal_cash_amount,
        range_of_effect: p.range_of_effect,
    }
}

// ---------------------------------------------------------------------------------------------
// Private state structs (kept small so the public surface is the `LogicModule` enum).

#[derive(Default)]
struct AutoFindHealing {
    scan_rate: u32,
    scan_range: f32,
    never_heal: f32,
    always_heal: f32,
}

#[derive(Default)]
struct Railroad {
    path_prefix_name: String,
    crash_fx_template_name: String,
    is_locomotive: bool,
    carriage_template_names: Vec<String>,
    big_metal_bounce_sound: Option<String>,
    small_metal_bounce_sound: Option<String>,
    meaty_bounce_sound: Option<String>,
    running_garrison_speed_max: f32,
    kill_speed_min: f32,
    speed_max: f32,
    acceleration: f32,
    braking: f32,
    wait_at_station_time: u32,
    running_sound: Option<String>,
    clickety_clack_sound: Option<String>,
    whistle_sound: Option<String>,
    friction: f32,
}

#[derive(Default)]
struct SpectreDeployment {
    gunship_template_name: String,
    required_science: Option<String>,
    special_power: Option<String>,
    attack_area_radius: f32,
    create_location: String,
}

#[derive(Default)]
struct MissileLauncherBuilding {
    special_power: Option<String>,
    door_open_time: u32,
    door_wait_open_time: u32,
    door_close_time: u32,
    door_opening_fx: Option<String>,
    door_open_fx: Option<String>,
    door_waiting_to_close_fx: Option<String>,
    door_closing_fx: Option<String>,
    door_closed_fx: Option<String>,
    door_open_idle_audio: Option<String>,
}

#[derive(Default)]
struct SpectreGunshipData {
    special_power: Option<String>,
    gattling_template_name: String,
    howitzer_firing_rate: u32,
    orbit_time: u32,
    howitzer_follow_lag: u32,
    attack_area_radius: f32,
    strafing_increment: f32,
    orbit_insertion_slope: f32,
    random_offset_for_howitzer: f32,
    targeting_reticle_radius: f32,
    gunship_orbit_radius: f32,
    howitzer_weapon_template: Option<String>,
    gattling_strafe_fx: Option<String>,
    attack_area_decal: Option<super::RadiusDecalData>,
    targeting_reticle_decal: Option<super::RadiusDecalData>,
}

#[derive(Default)]
struct ParticleUplink {
    special_power: Option<String>,
    begin_charge: u32,
    raise_antenna: u32,
    ready_delay: u32,
    width_grow: u32,
    beam_travel: u32,
    total_firing: u32,
    reveal_range: f32,
    outer_bone: String,
    outer_num_bones: u32,
    outer_light: String,
    outer_medium: String,
    outer_intense: String,
    connector_bone: String,
    connector_medium_laser: String,
    connector_intense_laser: String,
    connector_medium_flare: String,
    connector_intense_flare: String,
    fire_bone: String,
    laser_light: String,
    laser_medium: String,
    laser_intense: String,
    particle_beam_laser: String,
    swath_distance: f32,
    swath_amplitude: f32,
    total_scorch: u32,
    scorch_scalar: f32,
    beam_launch_fx: Option<String>,
    delay_between_launch_fx: u32,
    ground_hit_fx: Option<String>,
    damage_per_second: f32,
    total_damage_pulses: u32,
    damage_type: Mask<DamageType>,
    death_type: String,
    damage_radius_scalar: f32,
    power_up_sound: Option<String>,
    unpack_sound: Option<String>,
    firing_sound: Option<String>,
    annihilation_sound: Option<String>,
    remnant_object: String,
    manual_speed: f32,
    manual_fast_speed: f32,
    double_click_delay: u32,
}

struct FireOcl {
    weapon_slot: WeaponSlot,
    ocl: Option<String>,
    min_shots: u32,
    ocl_lifetime_per_second: u32,
    ocl_lifetime_max_cap: u32,
}

impl Default for FireOcl {
    fn default() -> Self {
        FireOcl {
            weapon_slot: WeaponSlot::Primary,
            ocl: None,
            min_shots: 1,
            ocl_lifetime_per_second: 1000,
            ocl_lifetime_max_cap: 1000,
        }
    }
}

#[derive(Default)]
struct SpyVision {
    needs_upgrade: bool,
    self_powered: bool,
    self_powered_duration: u32,
    self_powered_interval: u32,
    spy_on_kind_of: KindOfMask,
}

#[derive(Default)]
struct DynamicShroud {
    change_interval: u32,
    grow_interval: u32,
    shrink_delay: u32,
    shrink_time: u32,
    grow_delay: u32,
    grow_time: u32,
    final_vision: f32,
    grid_decal: Option<super::RadiusDecalData>,
}

#[derive(Default)]
struct NeutronMissileData {
    initial_dist: f32,
    max_turn_rate: f32,
    forward_damping: f32,
    relative_speed: f32,
    target_from_directly_above: f32,
    launch_fx: Option<String>,
    special_speed_time: u32,
    special_speed_height: f32,
    special_accel_factor: f32,
    special_jitter_distance: f32,
    ignition_fx: Option<String>,
    delivery_decal: Option<super::RadiusDecalData>,
    delivery_decal_radius: f32,
}

#[derive(Default)]
struct OclUpdate {
    ocl: Option<String>,
    faction_ocl: Vec<(String, String)>,
    min_delay: u32,
    max_delay: u32,
    create_at_edge: bool,
    faction_triggered: bool,
}

#[derive(Default)]
struct BattlePlan {
    special_power: Option<String>,
    bombardment_animation: u32,
    hold_animation: u32,
    sad_animation: u32,
    transition_idle: u32,
    bombardment_unpack: String,
    bombardment_pack: String,
    bombardment_label: String,
    bombardment_announce: String,
    sad_unpack: String,
    sad_idle: String,
    sad_pack: String,
    sad_label: String,
    sad_announce: String,
    hold_unpack: String,
    hold_pack: String,
    hold_label: String,
    hold_announce: String,
    valid_kind_of: KindOfMask,
    invalid_kind_of: KindOfMask,
    paralyze: u32,
    armor_scalar: f32,
    sad_sight: f32,
    strategy_sad_sight: f32,
    strategy_sad_stealth: bool,
    strategy_hold_health: f32,
    strategy_hold_change: String,
    vision_object: String,
}

#[derive(Default)]
struct WeaponBonus {
    required_kind_of: KindOfMask,
    forbidden_kind_of: KindOfMask,
    bonus_duration: u32,
    bonus_delay: u32,
    bonus_range: f32,
    bonus_condition_type: String,
}

#[derive(Default)]
struct Firestorm {
    initial_delay: u32,
    initial_height: f32,
    initial_major_radius: f32,
    initial_minor_radius: f32,
    final_height: f32,
    final_major_radius: f32,
    final_minor_radius: f32,
    transition_time: u32,
    reverse_at_transition_time: bool,
    delay_between_damage_frames: f32,
    damage_amount: f32,
    max_height_for_damage: f32,
    particle_systems: Vec<Option<String>>,
    fx_list: Option<String>,
    particle_offset_z: f32,
    scorch_size: f32,
}

#[derive(Default)]
struct WaveGuide {
    wave_delay: f32,
    y_size: f32,
    linear_wave_spacing: f32,
    wave_bend_magnitude: f32,
    water_velocity: f32,
    preferred_height: f32,
    shoreline_effect_distance: f32,
    damage_radius: f32,
    damage_amount: f32,
    topple_force: f32,
    random_splash_sound: Option<String>,
    random_splash_sound_frequency: i32,
    bridge_particle: Option<String>,
    bridge_particle_angle_fudge: f32,
    looping_sound: Option<String>,
}

#[derive(Default)]
struct BunkerBuster {
    upgrade_required: Option<String>,
    detonation_fx: Option<String>,
    crash_through_fx: Option<String>,
    crash_through_frequency: u32,
    seismic_radius: f32,
    seismic_magnitude: f32,
    shockwave_weapon: Option<String>,
    occupant_damage_weapon: Option<String>,
}

#[derive(Default)]
struct LeafletDrop {
    delay: u32,
    disabled_duration: u32,
    affect_radius: f32,
    leaflet_fx: Option<String>,
}

#[derive(Default)]
struct CrateCollide {
    class_name: String,
    required_kind_of: KindOfMask,
    forbidden_kind_of: KindOfMask,
    forbid_owner_player: bool,
    building_pickup: bool,
    human_only: bool,
    allow_multi_pickup: bool,
    pickup_science: Option<String>,
    execute_fx: Option<String>,
    execute_animation: String,
    execute_animation_time: f32,
    execute_animation_z_rise: f32,
    execute_animation_fades: bool,
    fx_list: Option<String>,
    sabotage_frames: u32,
    power_sabotage_frames: u32,
    steal_cash_amount: u32,
    range_of_effect: f32,
}

#[derive(Default)]
struct FireWeaponWhenDamaged {
    starts_active: bool,
    reaction_pristine: Option<String>,
    reaction_damaged: Option<String>,
    reaction_really_damaged: Option<String>,
    reaction_rubble: Option<String>,
    continuous_pristine: Option<String>,
    continuous_damaged: Option<String>,
    continuous_really_damaged: Option<String>,
    continuous_rubble: Option<String>,
    damage_types: Mask<DamageType>,
    damage_amount: f32,
}

#[derive(Default)]
struct Emp {
    lifetime: u32,
    start_fade_time: u32,
    start_scale: f32,
    disabled_duration: u32,
    target_scale_max: f32,
    target_scale_min: f32,
    start_color: [u8; 4],
    end_color: [u8; 4],
    disable_fx_particle_system: Option<String>,
    sparks_per_cubic_foot: f32,
    effect_radius: f32,
    does_not_affect: Mask<crate::names::WeaponAffects>,
    does_not_affect_my_own_buildings: bool,
    victim_required_kind_of: KindOfMask,
    victim_forbidden_kind_of: KindOfMask,
}
