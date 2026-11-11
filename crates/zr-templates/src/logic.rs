//! Remaining GameLogic "logic" module families typed from their `buildFieldParse` tables
//! (`GameLogic/Object/**` and the matching `Include/GameLogic/Module/*.h` in GeneralsMD).
//!
//! These modules are typed for data completeness; the simulation does not read them yet (they are
//! inert). Reference-style fields (OCL / FX / weapon / particle / audio / science / template names)
//! are kept as plain names rather than resolved [`Ref`]s, because nothing consumes them; this also
//! avoids dangling-reference warnings for names that are client-only.
//!
//! Unit conventions follow the engine's INI parsers (see `crate::value`): durations are milliseconds
//! to logic frames, velocities per second to per frame, angles to radians, percents to fractions.

use zr_ini::Block;

use crate::behavior::fields;
use crate::diag::Diag;
use crate::die::{DieMuxData, SlowDeathData};
use crate::flags::{KindOfMask, Mask};
use crate::names::{DamageType, ObjectStatus, ShadowType, WeaponSlot};
use crate::upgrade_module::UpgradeMuxData;
use crate::value::{parse_color, Args};

mod updates;

/// `RadiusDecalTemplate::parseRadiusDecalTemplate` (`GameClient/RadiusDecal.cpp`).
#[derive(Debug, Clone, PartialEq)]
pub struct RadiusDecalData {
    pub texture: Option<String>,
    pub style: Mask<ShadowType>,
    pub min_opacity: f32,
    pub max_opacity: f32,
    pub opacity_throb_time: u32,
    pub color: [u8; 4],
    pub only_visible_to_owning_player: bool,
}

impl Default for RadiusDecalData {
    fn default() -> Self {
        RadiusDecalData {
            texture: None,
            style: Mask::EMPTY.with(ShadowType::ShadowAlphaDecal),
            min_opacity: 1.0,
            max_opacity: 1.0,
            opacity_throb_time: crate::value::LOGICFRAMES_PER_SECOND,
            color: [0; 4],
            only_visible_to_owning_player: true,
        }
    }
}

/// `RadiusDecalTemplate::parseRadiusDecalTemplate`.
pub(crate) fn parse_radius_decal(block: &Block, d: &mut Diag) -> RadiusDecalData {
    let mut p = RadiusDecalData::default();
    fields(block, d, "RadiusDecal", |k, a| {
        match k {
            "Texture" => p.texture = opt_name(a.next()?),
            "Style" => p.style = p.style.parse(a.rest())?,
            "OpacityMin" => p.min_opacity = a.percent()?,
            "OpacityMax" => p.max_opacity = a.percent()?,
            "OpacityThrobTime" => p.opacity_throb_time = a.duration_frames()?,
            "Color" => p.color = parse_color(&a.rest().join(" "))?,
            "OnlyVisibleToOwningPlayer" => p.only_visible_to_owning_player = a.boolean()?,
            _ => {
                let _ = a.rest();
            }
        }
        Ok(true)
    });
    p
}

/// `None`/`NoSound`/empty become `None`, like the engine's empty template refs.
pub(crate) fn opt_name(t: &str) -> Option<String> {
    if t.is_empty() || t.eq_ignore_ascii_case("None") || t.eq_ignore_ascii_case("NoSound") {
        None
    } else {
        Some(t.to_string())
    }
}

/// One optional template/name token.
pub(crate) fn name(a: &mut Args) -> Result<Option<String>, String> {
    Ok(opt_name(a.next()?))
}

/// A space-separated list of names.
pub(crate) fn names(a: &mut Args) -> Vec<String> {
    a.rest().iter().map(|s| s.to_string()).collect()
}

/// Every `KindOf` bit set (the engine's default for several `KindOf` fields).
pub(crate) fn all_kinds() -> KindOfMask {
    use crate::flags::NameEnum;
    (0..crate::names::KindOf::count()).fold(Mask::EMPTY, |m, i| m.with(crate::names::KindOf::from_index(i)))
}

/// A parseable `BlastInfo` of `NeutronMissileSlowDeathBehavior`.
#[derive(Debug, Clone, PartialEq)]
pub struct NeutronBlast {
    pub enabled: bool,
    pub delay: f32,
    pub scorch_delay: f32,
    pub inner_radius: f32,
    pub outer_radius: f32,
    pub max_damage: f32,
    pub min_damage: f32,
    pub topple_speed: f32,
    pub push_force: f32,
}

impl Default for NeutronBlast {
    fn default() -> Self {
        NeutronBlast {
            enabled: false,
            delay: 0.0,
            scorch_delay: 0.0,
            inner_radius: 0.0,
            outer_radius: 0.0,
            max_damage: 0.0,
            min_damage: 0.0,
            topple_speed: 0.0,
            push_force: 0.0,
        }
    }
}

/// A classified module outside the families the simulation already reads (`Body`, `AI`, ...).
#[derive(Debug, Clone, PartialEq)]
pub enum LogicModule {
    // ---- Die / death-adjacent ----
    JetSlowDeath {
        slow: SlowDeathData,
        mux: DieMuxData,
        fx_on_ground_death: Option<String>,
        ocl_on_ground_death: Option<String>,
        fx_initial_death: Option<String>,
        ocl_initial_death: Option<String>,
        delay_secondary_from_initial_death: u32,
        fx_secondary: Option<String>,
        ocl_secondary: Option<String>,
        fx_hit_ground: Option<String>,
        ocl_hit_ground: Option<String>,
        delay_final_blow_up_from_hit_ground: u32,
        fx_final_blow_up: Option<String>,
        ocl_final_blow_up: Option<String>,
        death_loop_sound: Option<String>,
        roll_rate: f32,
        roll_rate_delta: f32,
        pitch_rate: f32,
        fall_how_fast: f32,
    },
    HelicopterSlowDeath {
        slow: SlowDeathData,
        mux: DieMuxData,
        spiral_orbit_turn_rate: f32,
        spiral_orbit_forward_speed: f32,
        spiral_orbit_forward_speed_damping: f32,
        min_self_spin: f32,
        max_self_spin: f32,
        self_spin_update_delay: f32,
        self_spin_update_amount: f32,
        fall_how_fast: f32,
        min_blade_fly_off_delay: f32,
        max_blade_fly_off_delay: f32,
        attach_particle: Option<String>,
        attach_particle_bone: String,
        attach_particle_loc: (f32, f32, f32),
        blade_object_name: String,
        blade_bone_name: String,
        ocl_eject_pilot: Option<String>,
        fx_blade: Option<String>,
        ocl_blade: Option<String>,
        fx_hit_ground: Option<String>,
        ocl_hit_ground: Option<String>,
        fx_final_blow_up: Option<String>,
        ocl_final_blow_up: Option<String>,
        delay_from_ground_to_final_death: f32,
        final_rubble_object: String,
        sound_death_loop: Option<String>,
        max_braking: f32,
    },
    BattleBusSlowDeath {
        slow: SlowDeathData,
        mux: DieMuxData,
        fx_start_undeath: Option<String>,
        ocl_start_undeath: Option<String>,
        fx_hit_ground: Option<String>,
        ocl_hit_ground: Option<String>,
        throw_force: f32,
        percent_damage_to_passengers: f32,
        empty_hulk_destruction_delay: u32,
    },
    NeutronMissileSlowDeath {
        slow: SlowDeathData,
        mux: DieMuxData,
        scorch_size: f32,
        fx_list: Option<String>,
        blasts: Vec<NeutronBlast>,
    },
    HeightDie {
        target_height: f32,
        target_height_includes_structures: bool,
        only_when_moving_down: bool,
        destroy_attached_particles_at_height: f32,
        snap_to_ground_on_death: bool,
        initial_delay: u32,
    },
    UpgradeDie {
        upgrade_to_remove: String,
        mux: DieMuxData,
    },
    SpecialPowerCompletionDie {
        special_power: Option<String>,
        mux: DieMuxData,
    },
    DamDie {
        mux: DieMuxData,
    },
    StructureCollapse {
        min_collapse_delay: u32,
        max_collapse_delay: u32,
        min_burst_delay: u32,
        max_burst_delay: u32,
        collapse_damping: f32,
        max_shudder: f32,
        big_burst_frequency: i32,
        ocls: Vec<(String, Vec<String>)>,
        fxs: Vec<(String, Vec<Option<String>>)>,
        mux: DieMuxData,
    },
    StructureTopple {
        min_topple_delay: u32,
        max_topple_delay: u32,
        min_topple_burst_delay: u32,
        max_topple_burst_delay: u32,
        structural_integrity: f32,
        structural_decay: f32,
        damage_fx_types: Mask<DamageType>,
        toppling_fx: Option<String>,
        topple_delay_fx: Option<String>,
        topple_start_fx: Option<String>,
        topple_done_fx: Option<String>,
        crushing_fx: Option<String>,
        crushing_weapon_name: String,
        ocls: Vec<(String, Vec<String>)>,
        angle_fx: Vec<(f32, Option<String>)>,
        mux: DieMuxData,
    },
    // ---- Upgrade ----
    UnpauseSpecialPower { special_power: Option<String>, mux: UpgradeMuxData },
    StealthUpgrade { mux: UpgradeMuxData },
    ReplaceObject { replace_object: String, mux: UpgradeMuxData },
    SubObjects { show_sub_objects: Vec<String>, hide_sub_objects: Vec<String>, mux: UpgradeMuxData },
    RadarUpgrade { disable_proof: bool, mux: UpgradeMuxData },
    PowerPlantUpgrade { mux: UpgradeMuxData },
    GrantScience { grant_science: String, mux: UpgradeMuxData },
    CostModifier { effect_kind_of: KindOfMask, percentage: f32, mux: UpgradeMuxData },
    PassengersFireUpgrade { mux: UpgradeMuxData },
    // ---- Create ----
    GrantUpgradeCreate { upgrade_to_grant: String, exempt_status: Mask<ObjectStatus> },
    SpecialPowerCreate,
    PreorderCreate,
    SupplyCenterCreate,
    SupplyWarehouseCreate,
    LockWeaponCreate { slot: WeaponSlot },
    GrantStealth {
        start_radius: f32,
        final_radius: f32,
        radius_grow_rate: f32,
        kind_of: KindOfMask,
        radius_particle_system: Option<String>,
    },
    // ---- Update / Behavior (parsed in `logic::updates`) ----
    AutoFindHealing { scan_rate: u32, scan_range: f32, never_heal: f32, always_heal: f32 },
    MobMemberSlaved {
        must_catch_up_radius: i32,
        catch_up_crisis_bail_time: u32,
        no_need_to_catch_up_radius: i32,
        squirrelliness: f32,
    },
    Railroad {
        path_prefix_name: String,
        crash_fx_template_name: String,
        /// The `PhysicsBehaviorModuleData` half of the block (`RailroadBehaviorModuleData` derives from
        /// it, `RailroadGuideAIUpdate.h:43`, and its field parse runs that table first, `:51`).
        physics: crate::module::PhysicsData,
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
    },
    PointDefenseLaser {
        weapon_template: Option<String>,
        primary_target_types: KindOfMask,
        secondary_target_types: KindOfMask,
        scan_rate: u32,
        scan_range: f32,
        predict_target_velocity_factor: f32,
    },
    Hijacker { attach_to_target_bone: String, parachute_name: String },
    PilotFindVehicle { scan_rate: u32, scan_range: f32, min_health: f32 },
    SpectreGunship {
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
        attack_area_decal: Option<RadiusDecalData>,
        targeting_reticle_decal: Option<RadiusDecalData>,
    },
    SpectreGunshipDeployment {
        gunship_template_name: String,
        required_science: Option<String>,
        special_power: Option<String>,
        attack_area_radius: f32,
        create_location: String,
    },
    MissileLauncherBuilding {
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
    },
    ParticleUplinkCannon {
        special_power: Option<String>,
        begin_charge_frames: u32,
        raise_antenna_frames: u32,
        ready_delay_frames: u32,
        width_grow_frames: u32,
        beam_travel_frames: u32,
        total_firing_frames: u32,
        reveal_range: f32,
        outer_effect_bone_name: String,
        outer_effect_num_bones: u32,
        outer_nodes_light_flare_particle_system: String,
        outer_nodes_medium_flare_particle_system: String,
        outer_nodes_intense_flare_particle_system: String,
        connector_bone_name: String,
        connector_medium_laser_name: String,
        connector_intense_laser_name: String,
        connector_medium_flare: String,
        connector_intense_flare: String,
        fire_bone_name: String,
        laser_base_light_flare_particle_system: String,
        laser_base_medium_flare_particle_system: String,
        laser_base_intense_flare_particle_system: String,
        particle_beam_laser_name: String,
        swath_of_death_distance: f32,
        swath_of_death_amplitude: f32,
        total_scorch_marks: u32,
        scorch_mark_scalar: f32,
        beam_launch_fx: Option<String>,
        delay_between_launch_fx: u32,
        ground_hit_fx: Option<String>,
        damage_per_second: f32,
        total_damage_pulses: u32,
        damage_type: Mask<DamageType>,
        death_type: String,
        damage_radius_scalar: f32,
        powering_up_sound_loop: Option<String>,
        unpack_to_idle_sound_loop: Option<String>,
        firing_to_pack_sound_loop: Option<String>,
        ground_annihilation_sound_loop: Option<String>,
        damage_pulse_remnant_object_name: String,
        manual_driving_speed: f32,
        manual_fast_driving_speed: f32,
        double_click_to_fast_drive_delay: u32,
    },
    SmartBombTargetHoming { course_correction_scalar: f32 },
    AssistedTargeting {
        assisting_clip_size: i32,
        assisting_weapon_slot: WeaponSlot,
        laser_from_assisted: String,
        laser_to_target: String,
    },
    Emp {
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
    },
    Deletion { min_lifetime: u32, max_lifetime: u32 },
    AnimationSteering { min_transition_time: u32 },
    FireOclAfterWeaponCooldown {
        weapon_slot: WeaponSlot,
        ocl: Option<String>,
        min_shots_to_create_ocl: u32,
        ocl_lifetime_per_second: u32,
        ocl_lifetime_max_cap: u32,
        mux: UpgradeMuxData,
    },
    SpyVisionUpdate {
        needs_upgrade: bool,
        self_powered: bool,
        self_powered_duration: u32,
        self_powered_interval: u32,
        spy_on_kind_of: KindOfMask,
        mux: UpgradeMuxData,
    },
    BoneFxUpdate {
        damage_fx_types: Mask<DamageType>,
        damage_ocl_types: Mask<DamageType>,
        damage_particle_types: Mask<DamageType>,
        entries: Vec<BoneFxEntry>,
    },
    FireSpread {
        ocl_embers: Option<String>,
        min_spread_delay: u32,
        max_spread_delay: u32,
        spread_try_range: f32,
    },
    RadarUpdate { radar_extend_time: f32 },
    ProjectileStreamUpdate,
    RadiusDecalUpdate,
    TechBuilding { pulse_fx: Option<String>, pulse_fx_rate: u32 },
    DynamicShroudClearingRange {
        change_interval: u32,
        grow_interval: u32,
        shrink_delay: u32,
        shrink_time: u32,
        grow_delay: u32,
        grow_time: u32,
        final_vision: f32,
        grid_decal: Option<RadiusDecalData>,
    },
    FireWeaponWhenDamaged {
        starts_active: bool,
        reaction_weapon_pristine: Option<String>,
        reaction_weapon_damaged: Option<String>,
        reaction_weapon_really_damaged: Option<String>,
        reaction_weapon_rubble: Option<String>,
        continuous_weapon_pristine: Option<String>,
        continuous_weapon_damaged: Option<String>,
        continuous_weapon_really_damaged: Option<String>,
        continuous_weapon_rubble: Option<String>,
        damage_types: Mask<DamageType>,
        damage_amount: f32,
        mux: UpgradeMuxData,
    },
    Float { enabled: bool },
    OclUpdate {
        ocl: Option<String>,
        faction_ocl: Vec<(String, String)>,
        min_delay: u32,
        max_delay: u32,
        create_at_edge: bool,
        faction_triggered: bool,
    },
    BattlePlan {
        special_power: Option<String>,
        bombardment_plan_animation_time: u32,
        hold_the_line_plan_animation_time: u32,
        search_and_destroy_plan_animation_time: u32,
        transition_idle_time: u32,
        bombardment_plan_unpack_sound_name: String,
        bombardment_plan_pack_sound_name: String,
        bombardment_message_label: String,
        bombardment_announcement_name: String,
        search_and_destroy_plan_unpack_sound_name: String,
        search_and_destroy_plan_idle_loop_sound_name: String,
        search_and_destroy_plan_pack_sound_name: String,
        search_and_destroy_message_label: String,
        search_and_destroy_announcement_name: String,
        hold_the_line_plan_unpack_sound_name: String,
        hold_the_line_plan_pack_sound_name: String,
        hold_the_line_message_label: String,
        hold_the_line_announcement_name: String,
        valid_member_kind_of: KindOfMask,
        invalid_member_kind_of: KindOfMask,
        battle_plan_change_paralyze_time: u32,
        hold_the_line_plan_armor_damage_scalar: f32,
        search_and_destroy_plan_sight_range_scalar: f32,
        strategy_center_search_and_destroy_sight_range_scalar: f32,
        strategy_center_search_and_destroy_detects_stealth: bool,
        strategy_center_hold_the_line_max_health_scalar: f32,
        strategy_center_hold_the_line_max_health_change_type: String,
        vision_object_name: String,
    },
    NeutronMissile {
        distance_to_travel_before_turning: f32,
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
        delivery_decal: Option<RadiusDecalData>,
        delivery_decal_radius: f32,
    },
    WeaponBonusUpdate {
        required_affect_kind_of: KindOfMask,
        forbidden_affect_kind_of: KindOfMask,
        bonus_duration: u32,
        bonus_delay: u32,
        bonus_range: f32,
        bonus_condition_type: String,
    },
    FirestormDynamicGeometry {
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
    },
    NeutronBlast { blast_radius: f32, affect_airborne: bool, affect_allies: bool },
    TensileFormation { enabled: bool, crack_sound: Option<String> },
    WaveGuide {
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
    },
    BunkerBuster {
        upgrade_required: Option<String>,
        detonation_fx: Option<String>,
        crash_through_bunker_fx: Option<String>,
        crash_through_bunker_fx_frequency: u32,
        seismic_effect_radius: f32,
        seismic_effect_magnitude: f32,
        shockwave_weapon_template: Option<String>,
        occupant_damage_weapon_template: Option<String>,
    },
    Checkpoint { scan_delay_time: u32 },
    EnemyNear { scan_delay_time: u32 },
    LeafletDrop {
        delay: u32,
        disabled_duration: u32,
        affect_radius: f32,
        leaflet_fx_particle_system: Option<String>,
    },
    StickyBomb {
        attach_to_target_bone: String,
        offset_z: f32,
        geometry_based_damage_weapon: Option<String>,
        geometry_based_damage_fx: Option<String>,
    },
    SupplyWarehouseCrippling {
        self_heal_supression: u32,
        self_heal_delay: u32,
        self_heal_amount: f32,
    },
    Bridge {
        lateral_scaffold_speed: f32,
        vertical_scaffold_speed: f32,
        die_fx: Vec<BridgeFx>,
        die_ocl: Vec<BridgeOcl>,
    },
    BridgeTower,
    BridgeScaffold,
    BoneFxDamage,
    /// Sabotage / convert crate collides (a `CrateCollideModuleData` subclass).
    CrateCollide {
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
    },
}

/// One `BoneFXUpdate` entry (`Bone:<bone> OnlyOnce:<Y/N> <min> <max> FXList:<name>`).
#[derive(Debug, Clone, PartialEq)]
pub struct BoneFxEntry {
    /// Body state: 0 pristine, 1 damaged, 2 really damaged, 3 rubble.
    pub state: u8,
    /// 1-based slot.
    pub slot: u8,
    /// 0 FXList, 1 OCL, 2 particle system.
    pub kind: u8,
    pub bone: String,
    pub only_once: bool,
    pub min_delay: f32,
    pub max_delay: f32,
    pub name: Option<String>,
}

/// One `BridgeDieFX` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeFx {
    pub fx: Option<String>,
    pub delay: u32,
    pub bone: Option<String>,
}

/// One `BridgeDieOCL` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct BridgeOcl {
    pub ocl: Option<String>,
    pub delay: u32,
    pub bone: Option<String>,
}

impl LogicModule {
    /// `ModuleFactory::newModuleDataFromINI` for one of the classes this module types.
    pub(crate) fn parse(class: &str, block: &Block, gravity: f32, d: &mut Diag) -> Option<LogicModule> {
        if let Some(m) = parse_die(class, block, d) {
            return Some(m);
        }
        if let Some(m) = parse_upgrade(class, block, d) {
            return Some(m);
        }
        if let Some(m) = parse_create(class, block, d) {
            return Some(m);
        }
        updates::parse(class, block, gravity, d)
    }
}

/// Parse a `SlowDeathBehavior`-derived module's own fields after the shared slow-death + `DieMux`
/// fields. `own` receives keys the shared tables did not consume.
fn parse_slow_death(
    block: &Block,
    d: &mut Diag,
    scope: &str,
    mut own: impl FnMut(&str, &mut Args, &mut SlowDeathData) -> Result<bool, String>,
) -> (SlowDeathData, DieMuxData) {
    let mut slow = SlowDeathData::new();
    let mut mux = DieMuxData::default();
    fields(block, d, scope, |k, a| {
        if slow.field(k, a)? {
            return Ok(true);
        }
        if mux.field(k, a)? {
            return Ok(true);
        }
        if own(k, a, &mut slow)? {
            return Ok(true);
        }
        let _ = a.rest();
        Ok(true)
    });
    (slow, mux)
}

fn parse_die(class: &str, block: &Block, d: &mut Diag) -> Option<LogicModule> {
    Some(match class {
        "JetSlowDeathBehavior" => {
            let mut fx_on_ground_death = None;
            let mut ocl_on_ground_death = None;
            let mut fx_initial_death = None;
            let mut ocl_initial_death = None;
            let mut delay_secondary_from_initial_death = 0;
            let mut fx_secondary = None;
            let mut ocl_secondary = None;
            let mut fx_hit_ground = None;
            let mut ocl_hit_ground = None;
            let mut delay_final_blow_up_from_hit_ground = 0;
            let mut fx_final_blow_up = None;
            let mut ocl_final_blow_up = None;
            let mut death_loop_sound = None;
            let mut roll_rate = 0.0;
            let mut roll_rate_delta = 1.0;
            let mut pitch_rate = 0.0;
            let mut fall_how_fast = 0.0;
            let (slow, mux) = parse_slow_death(block, d, class, |k, a, _| {
                match k {
                    "FXOnGroundDeath" => fx_on_ground_death = name(a)?,
                    "OCLOnGroundDeath" => ocl_on_ground_death = name(a)?,
                    "FXInitialDeath" => fx_initial_death = name(a)?,
                    "OCLInitialDeath" => ocl_initial_death = name(a)?,
                    "DelaySecondaryFromInitialDeath" => delay_secondary_from_initial_death = a.duration_frames()?,
                    "FXSecondary" => fx_secondary = name(a)?,
                    "OCLSecondary" => ocl_secondary = name(a)?,
                    "FXHitGround" => fx_hit_ground = name(a)?,
                    "OCLHitGround" => ocl_hit_ground = name(a)?,
                    "DelayFinalBlowUpFromHitGround" => delay_final_blow_up_from_hit_ground = a.duration_frames()?,
                    "FXFinalBlowUp" => fx_final_blow_up = name(a)?,
                    "OCLFinalBlowUp" => ocl_final_blow_up = name(a)?,
                    "DeathLoopSound" => death_loop_sound = name(a)?,
                    "RollRate" => roll_rate = a.real()?,
                    "RollRateDelta" => roll_rate_delta = a.percent()?,
                    "PitchRate" => pitch_rate = a.real()?,
                    "FallHowFast" => fall_how_fast = a.percent()?,
                    _ => return Ok(false),
                }
                Ok(true)
            });
            LogicModule::JetSlowDeath {
                slow,
                mux,
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
                death_loop_sound,
                roll_rate,
                roll_rate_delta,
                pitch_rate,
                fall_how_fast,
            }
        }
        "HelicopterSlowDeathBehavior" => {
            let mut p = HelicopterSlowDeathFields::default();
            let (slow, mux) = parse_slow_death(block, d, class, |k, a, _| p.field(k, a));
            p.into_variant(slow, mux)
        }
        "BattleBusSlowDeathBehavior" => {
            let mut fx_start_undeath = None;
            let mut ocl_start_undeath = None;
            let mut fx_hit_ground = None;
            let mut ocl_hit_ground = None;
            let mut throw_force = 1.0;
            let mut percent_damage_to_passengers = 0.0;
            let mut empty_hulk_destruction_delay = 0;
            let (slow, mux) = parse_slow_death(block, d, class, |k, a, _| {
                match k {
                    "FXStartUndeath" => fx_start_undeath = name(a)?,
                    "OCLStartUndeath" => ocl_start_undeath = name(a)?,
                    "FXHitGround" => fx_hit_ground = name(a)?,
                    "OCLHitGround" => ocl_hit_ground = name(a)?,
                    "ThrowForce" => throw_force = a.real()?,
                    "PercentDamageToPassengers" => percent_damage_to_passengers = a.percent()?,
                    "EmptyHulkDestructionDelay" => empty_hulk_destruction_delay = a.duration_frames()?,
                    _ => return Ok(false),
                }
                Ok(true)
            });
            LogicModule::BattleBusSlowDeath {
                slow,
                mux,
                fx_start_undeath,
                ocl_start_undeath,
                fx_hit_ground,
                ocl_hit_ground,
                throw_force,
                percent_damage_to_passengers,
                empty_hulk_destruction_delay,
            }
        }
        "NeutronMissileSlowDeathBehavior" => {
            let mut scorch_size = 0.0;
            let mut fx_list = None;
            let mut blasts: Vec<NeutronBlast> = (0..9).map(|_| NeutronBlast::default()).collect();
            let (slow, mux) = parse_slow_death(block, d, class, |k, a, _| {
                if k == "ScorchMarkSize" {
                    scorch_size = a.real()?;
                    return Ok(true);
                }
                if k == "FXList" {
                    fx_list = name(a)?;
                    return Ok(true);
                }
                if let Some(rest) = k.strip_prefix("Blast") {
                    let n = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
                    if n > 0 {
                        if let Ok(idx) = rest[..n].parse::<usize>() {
                            if (1..=9).contains(&idx) {
                                let b = &mut blasts[idx - 1];
                                match &rest[n..] {
                                    "Enabled" => b.enabled = a.boolean()?,
                                    "Delay" => b.delay = a.duration_real()?,
                                    "ScorchDelay" => b.scorch_delay = a.duration_real()?,
                                    "InnerRadius" => b.inner_radius = a.real()?,
                                    "OuterRadius" => b.outer_radius = a.real()?,
                                    "MaxDamage" => b.max_damage = a.real()?,
                                    "MinDamage" => b.min_damage = a.real()?,
                                    "ToppleSpeed" => b.topple_speed = a.real()?,
                                    "PushForce" => b.push_force = a.real()?,
                                    _ => return Ok(false),
                                }
                                return Ok(true);
                            }
                        }
                    }
                }
                Ok(false)
            });
            LogicModule::NeutronMissileSlowDeath { slow, mux, scorch_size, fx_list, blasts }
        }
        "HeightDieUpdate" => {
            let mut p = HeightDieData { destroy_attached_particles_at_height: -1.0, ..HeightDieData::default() };
            fields(block, d, class, |k, a| {
                match k {
                    "TargetHeight" => p.target_height = a.real()?,
                    "TargetHeightIncludesStructures" => p.target_height_includes_structures = a.boolean()?,
                    "OnlyWhenMovingDown" => p.only_when_moving_down = a.boolean()?,
                    "DestroyAttachedParticlesAtHeight" => p.destroy_attached_particles_at_height = a.real()?,
                    "SnapToGroundOnDeath" => p.snap_to_ground_on_death = a.boolean()?,
                    "InitialDelay" => p.initial_delay = a.duration_frames()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::HeightDie {
                target_height: p.target_height,
                target_height_includes_structures: p.target_height_includes_structures,
                only_when_moving_down: p.only_when_moving_down,
                destroy_attached_particles_at_height: p.destroy_attached_particles_at_height,
                snap_to_ground_on_death: p.snap_to_ground_on_death,
                initial_delay: p.initial_delay,
            }
        }
        "UpgradeDie" => {
            let mut upgrade_to_remove = String::new();
            let mut mux = DieMuxData::default();
            fields(block, d, class, |k, a| {
                if mux.field(k, a)? {
                    return Ok(true);
                }
                if k == "UpgradeToRemove" {
                    upgrade_to_remove = a.next()?.to_string();
                    return Ok(true);
                }
                let _ = a.rest();
                Ok(true)
            });
            LogicModule::UpgradeDie { upgrade_to_remove, mux }
        }
        "SpecialPowerCompletionDie" => {
            let mut special_power = None;
            let mut mux = DieMuxData::default();
            fields(block, d, class, |k, a| {
                if mux.field(k, a)? {
                    return Ok(true);
                }
                if k == "SpecialPowerTemplate" {
                    special_power = name(a)?;
                    return Ok(true);
                }
                let _ = a.rest();
                Ok(true)
            });
            LogicModule::SpecialPowerCompletionDie { special_power, mux }
        }
        "DamDie" => {
            let mut mux = DieMuxData::default();
            fields(block, d, class, |k, a| {
                if mux.field(k, a)? {
                    return Ok(true);
                }
                let _ = a.rest();
                Ok(true)
            });
            LogicModule::DamDie { mux }
        }
        "StructureCollapseUpdate" => {
            let mut p = StructureCollapseData { min_burst_delay: 9999, ..StructureCollapseData::default() };
            let mut mux = DieMuxData::default();
            fields(block, d, class, |k, a| {
                if mux.field(k, a)? {
                    return Ok(true);
                }
                match k {
                    "MinCollapseDelay" => p.min_collapse_delay = a.duration_frames()?,
                    "MaxCollapseDelay" => p.max_collapse_delay = a.duration_frames()?,
                    "MinBurstDelay" => p.min_burst_delay = a.duration_frames()?,
                    "MaxBurstDelay" => p.max_burst_delay = a.duration_frames()?,
                    "CollapseDamping" => p.collapse_damping = a.real()?,
                    "MaxShudder" => p.max_shudder = a.real()?,
                    "BigBurstFrequency" => p.big_burst_frequency = a.int()?,
                    "OCL" => append_phase(&mut p.ocl, a, true)?,
                    "FXList" => append_phase_bytes(&mut p.fx, a)?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::StructureCollapse {
                min_collapse_delay: p.min_collapse_delay,
                max_collapse_delay: p.max_collapse_delay,
                min_burst_delay: p.min_burst_delay,
                max_burst_delay: p.max_burst_delay,
                collapse_damping: p.collapse_damping,
                max_shudder: p.max_shudder,
                big_burst_frequency: p.big_burst_frequency,
                ocls: p.ocl,
                fxs: p.fx,
                mux,
            }
        }
        "StructureToppleUpdate" => {
            let mut p = StructureToppleData::default();
            let mut mux = DieMuxData::default();
            fields(block, d, class, |k, a| {
                if mux.field(k, a)? {
                    return Ok(true);
                }
                match k {
                    "MinToppleDelay" => p.min_topple_delay = a.duration_frames()?,
                    "MaxToppleDelay" => p.max_topple_delay = a.duration_frames()?,
                    "MinToppleBurstDelay" => p.min_topple_burst_delay = a.duration_frames()?,
                    "MaxToppleBurstDelay" => p.max_topple_burst_delay = a.duration_frames()?,
                    "StructuralIntegrity" => p.structural_integrity = a.real()?,
                    "StructuralDecay" => p.structural_decay = a.real()?,
                    "DamageFXTypes" => p.damage_fx_types = crate::value::parse_flag_list_all(a.rest())?,
                    "TopplingFX" => p.toppling_fx = name(a)?,
                    "ToppleDelayFX" => p.topple_delay_fx = name(a)?,
                    "ToppleStartFX" => p.topple_start_fx = name(a)?,
                    "ToppleDoneFX" => p.topple_done_fx = name(a)?,
                    "CrushingFX" => p.crushing_fx = name(a)?,
                    "CrushingWeaponName" => p.crushing_weapon_name = a.next()?.to_string(),
                    "OCL" => append_phase(&mut p.ocl, a, true)?,
                    "AngleFX" => {
                        let deg = a.real()?;
                        let fx = name(a)?;
                        p.angle_fx.push((crate::value::degrees_to_radians(deg), fx));
                    }
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::StructureTopple {
                min_topple_delay: p.min_topple_delay,
                max_topple_delay: p.max_topple_delay,
                min_topple_burst_delay: p.min_topple_burst_delay,
                max_topple_burst_delay: p.max_topple_burst_delay,
                structural_integrity: p.structural_integrity,
                structural_decay: p.structural_decay,
                damage_fx_types: p.damage_fx_types,
                toppling_fx: p.toppling_fx,
                topple_delay_fx: p.topple_delay_fx,
                topple_start_fx: p.topple_start_fx,
                topple_done_fx: p.topple_done_fx,
                crushing_fx: p.crushing_fx,
                crushing_weapon_name: p.crushing_weapon_name,
                ocls: p.ocl,
                angle_fx: p.angle_fx,
                mux,
            }
        }
        _ => return None,
    })
}

fn append_phase(out: &mut Vec<(String, Vec<String>)>, a: &mut Args, _ocl: bool) -> Result<(), String> {
    let phase = a.next()?.to_string();
    out.push((phase, names(a)));
    Ok(())
}

fn append_phase_bytes(out: &mut Vec<(String, Vec<Option<String>>)>, a: &mut Args) -> Result<(), String> {
    let phase = a.next()?.to_string();
    out.push((phase, a.rest().iter().map(|t| opt_name(t)).collect()));
    Ok(())
}

fn parse_upgrade(class: &str, block: &Block, d: &mut Diag) -> Option<LogicModule> {
    Some(match class {
        "UnpauseSpecialPowerUpgrade" => {
            let (mut special_power, mux) = parse_upgrade_base(block, d, class, |k, a| {
                if k == "SpecialPowerTemplate" {
                    return name(a);
                }
                Ok(None)
            });
            LogicModule::UnpauseSpecialPower { special_power: special_power.take(), mux }
        }
        "StealthUpgrade" => LogicModule::StealthUpgrade { mux: parse_upgrade_mux(block, d, class) },
        "ReplaceObjectUpgrade" => {
            let (mut replace_object, mux) = parse_upgrade_base(block, d, class, |k, a| {
                if k == "ReplaceObject" {
                    return Ok(Some(a.next()?.to_string()));
                }
                Ok(None)
            });
            LogicModule::ReplaceObject { replace_object: replace_object.take().unwrap_or_default(), mux }
        }
        "SubObjectsUpgrade" => {
            let mut show = Vec::new();
            let mut hide = Vec::new();
            let mux = parse_upgrade_mux_extra(block, d, class, |k, a| {
                match k {
                    "ShowSubObjects" => show.extend(names(a)),
                    "HideSubObjects" => hide.extend(names(a)),
                    _ => return Ok(false),
                }
                Ok(true)
            });
            LogicModule::SubObjects { show_sub_objects: show, hide_sub_objects: hide, mux }
        }
        "RadarUpgrade" => {
            let mut disable_proof = false;
            let mux = parse_upgrade_mux_extra(block, d, class, |k, a| {
                match k {
                    "DisableProof" => disable_proof = a.boolean()?,
                    _ => return Ok(false),
                }
                Ok(true)
            });
            LogicModule::RadarUpgrade { disable_proof, mux }
        }
        "PowerPlantUpgrade" => LogicModule::PowerPlantUpgrade { mux: parse_upgrade_mux(block, d, class) },
        "GrantScienceUpgrade" => {
            let (mut grant_science, mux) = parse_upgrade_base(block, d, class, |k, a| {
                if k == "GrantScience" {
                    return Ok(Some(a.next()?.to_string()));
                }
                Ok(None)
            });
            LogicModule::GrantScience { grant_science: grant_science.take().unwrap_or_default(), mux }
        }
        "CostModifierUpgrade" => {
            let mut effect_kind_of = KindOfMask::EMPTY;
            let mut percentage = 0.0;
            let mux = parse_upgrade_mux_extra(block, d, class, |k, a| {
                match k {
                    "EffectKindOf" => effect_kind_of = effect_kind_of.parse(a.rest())?,
                    "Percentage" => percentage = a.percent()?,
                    _ => return Ok(false),
                }
                Ok(true)
            });
            LogicModule::CostModifier { effect_kind_of, percentage, mux }
        }
        "PassengersFireUpgrade" => LogicModule::PassengersFireUpgrade { mux: parse_upgrade_mux(block, d, class) },
        _ => return None,
    })
}

/// Parse one extra own string field plus the shared `UpgradeMuxData`.
fn parse_upgrade_base(
    block: &Block,
    d: &mut Diag,
    scope: &str,
    mut own: impl FnMut(&str, &mut Args) -> Result<Option<String>, String>,
) -> (Option<String>, UpgradeMuxData) {
    let mut value = None;
    let mut mux = UpgradeMuxData::default();
    fields(block, d, scope, |k, a| {
        if value.is_none() {
            if let Some(v) = own(k, a)? {
                value = Some(v);
                return Ok(true);
            }
        }
        if mux.field(k, a)? {
            return Ok(true);
        }
        let _ = a.rest();
        Ok(true)
    });
    (value, mux)
}

fn parse_upgrade_mux(block: &Block, d: &mut Diag, scope: &str) -> UpgradeMuxData {
    parse_upgrade_mux_extra(block, d, scope, |_, _| Ok(false))
}

fn parse_upgrade_mux_extra(
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

fn parse_create(class: &str, block: &Block, d: &mut Diag) -> Option<LogicModule> {
    Some(match class {
        "GrantUpgradeCreate" => {
            let mut upgrade_to_grant = String::new();
            let mut exempt_status = Mask::EMPTY;
            fields(block, d, class, |k, a| {
                match k {
                    "UpgradeToGrant" => upgrade_to_grant = a.next()?.to_string(),
                    "ExemptStatus" => exempt_status = exempt_status.parse(a.rest())?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::GrantUpgradeCreate { upgrade_to_grant, exempt_status }
        }
        "SpecialPowerCreate" => LogicModule::SpecialPowerCreate,
        "PreorderCreate" => LogicModule::PreorderCreate,
        "SupplyCenterCreate" => LogicModule::SupplyCenterCreate,
        "SupplyWarehouseCreate" => LogicModule::SupplyWarehouseCreate,
        "LockWeaponCreate" => {
            let mut slot = WeaponSlot::Primary;
            fields(block, d, class, |k, a| {
                match k {
                    "SlotToLock" => slot = a.index()?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::LockWeaponCreate { slot }
        }
        "GrantStealthBehavior" => {
            let mut p = GrantStealthData::new();
            fields(block, d, class, |k, a| {
                match k {
                    "StartRadius" => p.start_radius = a.real()?,
                    "FinalRadius" => p.final_radius = a.real()?,
                    "RadiusGrowRate" => p.radius_grow_rate = a.real()?,
                    "KindOf" => p.kind_of = p.kind_of.parse(a.rest())?,
                    "RadiusParticleSystemName" => p.radius_particle_system = name(a)?,
                    _ => {
                        let _ = a.rest();
                    }
                }
                Ok(true)
            });
            LogicModule::GrantStealth {
                start_radius: p.start_radius,
                final_radius: p.final_radius,
                radius_grow_rate: p.radius_grow_rate,
                kind_of: p.kind_of,
                radius_particle_system: p.radius_particle_system,
            }
        }
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------
// Small helper state structs (kept private so the enum stays the public surface).

#[derive(Default)]
struct HeightDieData {
    target_height: f32,
    target_height_includes_structures: bool,
    only_when_moving_down: bool,
    destroy_attached_particles_at_height: f32,
    snap_to_ground_on_death: bool,
    initial_delay: u32,
}

#[derive(Default)]
struct StructureCollapseData {
    min_collapse_delay: u32,
    max_collapse_delay: u32,
    min_burst_delay: u32,
    max_burst_delay: u32,
    collapse_damping: f32,
    max_shudder: f32,
    big_burst_frequency: i32,
    ocl: Vec<(String, Vec<String>)>,
    fx: Vec<(String, Vec<Option<String>>)>,
}

struct StructureToppleData {
    min_topple_delay: u32,
    max_topple_delay: u32,
    min_topple_burst_delay: u32,
    max_topple_burst_delay: u32,
    structural_integrity: f32,
    structural_decay: f32,
    damage_fx_types: Mask<DamageType>,
    toppling_fx: Option<String>,
    topple_delay_fx: Option<String>,
    topple_start_fx: Option<String>,
    topple_done_fx: Option<String>,
    crushing_fx: Option<String>,
    crushing_weapon_name: String,
    ocl: Vec<(String, Vec<String>)>,
    angle_fx: Vec<(f32, Option<String>)>,
}

impl Default for StructureToppleData {
    fn default() -> Self {
        StructureToppleData {
            min_topple_delay: 0,
            max_topple_delay: 0,
            min_topple_burst_delay: 0,
            max_topple_burst_delay: 0,
            structural_integrity: 0.1,
            structural_decay: 0.0,
            damage_fx_types: DamageType::ALL.iter().copied().collect(),
            toppling_fx: None,
            topple_delay_fx: None,
            topple_start_fx: None,
            topple_done_fx: None,
            crushing_fx: None,
            crushing_weapon_name: String::new(),
            ocl: Vec::new(),
            angle_fx: Vec::new(),
        }
    }
}

#[derive(Default)]
struct GrantStealthData {
    start_radius: f32,
    final_radius: f32,
    radius_grow_rate: f32,
    kind_of: KindOfMask,
    radius_particle_system: Option<String>,
}

impl GrantStealthData {
    fn new() -> Self {
        GrantStealthData { final_radius: 200.0, radius_grow_rate: 10.0, kind_of: all_kinds(), ..GrantStealthData::default() }
    }
}

/// `HelicopterSlowDeathBehavior` own fields + conversion into the enum variant's field set.
#[derive(Default)]
struct HelicopterSlowDeathFields {
    spiral_orbit_turn_rate: f32,
    spiral_orbit_forward_speed: f32,
    spiral_orbit_forward_speed_damping: f32,
    min_self_spin: f32,
    max_self_spin: f32,
    self_spin_update_delay: f32,
    self_spin_update_amount: f32,
    fall_how_fast: f32,
    min_blade_fly_off_delay: f32,
    max_blade_fly_off_delay: f32,
    attach_particle: Option<String>,
    attach_particle_bone: String,
    attach_particle_loc: (f32, f32, f32),
    blade_object_name: String,
    blade_bone_name: String,
    ocl_eject_pilot: Option<String>,
    fx_blade: Option<String>,
    ocl_blade: Option<String>,
    fx_hit_ground: Option<String>,
    ocl_hit_ground: Option<String>,
    fx_final_blow_up: Option<String>,
    ocl_final_blow_up: Option<String>,
    delay_from_ground_to_final_death: f32,
    final_rubble_object: String,
    sound_death_loop: Option<String>,
    max_braking: f32,
}

impl HelicopterSlowDeathFields {
    fn field(&mut self, k: &str, a: &mut Args) -> Result<bool, String> {
        match k {
            "SpiralOrbitTurnRate" => self.spiral_orbit_turn_rate = a.angular_velocity()?,
            "SpiralOrbitForwardSpeed" => self.spiral_orbit_forward_speed = a.velocity()?,
            "SpiralOrbitForwardSpeedDamping" => self.spiral_orbit_forward_speed_damping = a.real()?,
            "MinSelfSpin" => self.min_self_spin = a.angular_velocity()?,
            "MaxSelfSpin" => self.max_self_spin = a.angular_velocity()?,
            "SelfSpinUpdateDelay" => self.self_spin_update_delay = a.duration_real()?,
            "SelfSpinUpdateAmount" => self.self_spin_update_amount = a.angle()?,
            "FallHowFast" => self.fall_how_fast = a.percent()?,
            "MinBladeFlyOffDelay" => self.min_blade_fly_off_delay = a.duration_real()?,
            "MaxBladeFlyOffDelay" => self.max_blade_fly_off_delay = a.duration_real()?,
            "AttachParticle" => self.attach_particle = name(a)?,
            "AttachParticleBone" => self.attach_particle_bone = a.next()?.to_string(),
            "AttachParticleLoc" => self.attach_particle_loc = crate::value::parse_coord3d(&a.rest().join(" "))?,
            "BladeObjectName" => self.blade_object_name = a.next()?.to_string(),
            "BladeBoneName" => self.blade_bone_name = a.next()?.to_string(),
            "OCLEjectPilot" => self.ocl_eject_pilot = name(a)?,
            "FXBlade" => self.fx_blade = name(a)?,
            "OCLBlade" => self.ocl_blade = name(a)?,
            "FXHitGround" => self.fx_hit_ground = name(a)?,
            "OCLHitGround" => self.ocl_hit_ground = name(a)?,
            "FXFinalBlowUp" => self.fx_final_blow_up = name(a)?,
            "OCLFinalBlowUp" => self.ocl_final_blow_up = name(a)?,
            "DelayFromGroundToFinalDeath" => self.delay_from_ground_to_final_death = a.duration_real()?,
            "FinalRubbleObject" => self.final_rubble_object = a.next()?.to_string(),
            "SoundDeathLoop" => self.sound_death_loop = name(a)?,
            "MaxBraking" => self.max_braking = a.acceleration()?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

// Build the enum variant from the parsed helper struct.
impl HelicopterSlowDeathFields {
    fn into_variant(self, slow: SlowDeathData, mux: DieMuxData) -> LogicModule {
        LogicModule::HelicopterSlowDeath {
            slow,
            mux,
            spiral_orbit_turn_rate: self.spiral_orbit_turn_rate,
            spiral_orbit_forward_speed: self.spiral_orbit_forward_speed,
            spiral_orbit_forward_speed_damping: self.spiral_orbit_forward_speed_damping,
            min_self_spin: self.min_self_spin,
            max_self_spin: self.max_self_spin,
            self_spin_update_delay: self.self_spin_update_delay,
            self_spin_update_amount: self.self_spin_update_amount,
            fall_how_fast: self.fall_how_fast,
            min_blade_fly_off_delay: self.min_blade_fly_off_delay,
            max_blade_fly_off_delay: self.max_blade_fly_off_delay,
            attach_particle: self.attach_particle,
            attach_particle_bone: self.attach_particle_bone,
            attach_particle_loc: self.attach_particle_loc,
            blade_object_name: self.blade_object_name,
            blade_bone_name: self.blade_bone_name,
            ocl_eject_pilot: self.ocl_eject_pilot,
            fx_blade: self.fx_blade,
            ocl_blade: self.ocl_blade,
            fx_hit_ground: self.fx_hit_ground,
            ocl_hit_ground: self.ocl_hit_ground,
            fx_final_blow_up: self.fx_final_blow_up,
            ocl_final_blow_up: self.ocl_final_blow_up,
            delay_from_ground_to_final_death: self.delay_from_ground_to_final_death,
            final_rubble_object: self.final_rubble_object,
            sound_death_loop: self.sound_death_loop,
            max_braking: self.max_braking,
        }
    }
}
