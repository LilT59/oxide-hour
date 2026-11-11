//! `ObjectCreationList`. Ported from `GameLogic/Object/ObjectCreationList.cpp`
//! (`TheObjectCreationListFieldParse`, the six `ObjectCreationNugget` implementations) and
//! `DeliverPayloadAIUpdate.cpp` (`DeliverPayloadData::getFieldParse`).
//!
//! `ObjectCreationList NAME ... End` blocks. `parseObjectCreationListDefinition` calls
//! `ocl.clear()` before parsing, so a repeated definition replaces the earlier one entirely (it
//! does not overlay). Nuggets are nested blocks: `CreateObject`, `CreateDebris`,
//! `ApplyRandomForce`, `DeliverPayload`, `FireWeapon`, `Attack`.
//!
//! # Retail `ObjectNames` / `ModelNames` quirk
//!
//! The retail engine's `parseDebrisObjectNames` advances the token cursor twice per iteration
//! (once in the loop body, once in the `for` increment), so it only keeps every other name. The
//! TheSuperHackers fork preserves this only under `RETAIL_COMPATIBLE_CRC`; this port reads every
//! token, matching the fixed branch (the simulation is not CRC-compatible with retail).

use zr_ini::{Block, Item};

use crate::diag::{Diag, RawBlock, RawItem, WarningKind};
use crate::flags::Mask;
use crate::names::{DebrisDisposition, ShadowType, WeaponSlot};
use crate::registry::{Ref, WeaponId};
use crate::value::{friction_per_sec_to_per_frame, parse_coord3d, Args};

/// One `ObjectCreationList` definition.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectCreationList {
    pub name: String,
    pub nuggets: Vec<Nugget>,
}

/// One nugget of an OCL, in file order.
#[derive(Debug, Clone, PartialEq)]
pub enum Nugget {
    CreateObject(GenericNugget),
    CreateDebris(GenericNugget),
    ApplyRandomForce(ApplyRandomForce),
    DeliverPayload(DeliverPayload),
    FireWeapon(FireWeaponNugget),
    Attack(AttackNugget),
    /// A nugget key the field table does not know, kept verbatim.
    Unparsed { kind: String, items: Vec<RawItem> },
}

/// `GenericObjectCreationNugget`, used by both `CreateObject` (`m_nameAreObjects = true`) and
/// `CreateDebris` (`false`). Defaults are the constructor's.
#[derive(Debug, Clone, PartialEq)]
pub struct GenericNugget {
    /// `true` for `CreateObject` (`ObjectNames`), `false` for `CreateDebris` (`ModelNames`).
    pub is_object: bool,
    pub names: Vec<String>,
    pub put_in_container: Option<String>,
    pub particle_system: Option<String>,
    pub count: i32,
    pub ignore_primary_obstacle: bool,
    pub orient_in_force_direction: bool,
    pub extra_bounciness: f32,
    /// `ExtraFriction`, per frame (`parseFrictionPerSec`).
    pub extra_friction: f32,
    pub offset: (f32, f32, f32),
    pub disposition: Mask<DebrisDisposition>,
    pub disposition_intensity: f32,
    /// Angular rates, radians per frame (default -1).
    pub spin_rate: f32,
    pub yaw_rate: f32,
    pub roll_rate: f32,
    pub pitch_rate: f32,
    pub min_force_magnitude: f32,
    pub max_force_magnitude: f32,
    /// Radians.
    pub min_force_pitch: f32,
    pub max_force_pitch: f32,
    /// Frames.
    pub min_lifetime: u32,
    pub max_lifetime: u32,
    pub spread_formation: bool,
    pub min_distance_a_formation: f32,
    pub min_distance_b_formation: f32,
    pub max_distance_formation: f32,
    pub fade_in: bool,
    pub fade_out: bool,
    pub fade_frames: u32,
    pub fade_sound: Option<String>,
    pub preserve_layer: bool,
    pub dies_on_bad_land: bool,
    // `CreateObject` only.
    pub contain_inside_source_object: bool,
    pub object_count: i32,
    pub inherits_veterancy: bool,
    pub skip_if_significantly_airborne: bool,
    pub invulnerable_time: u32,
    /// Percent fields (`parsePercentToReal`).
    pub min_health: f32,
    pub max_health: f32,
    pub requires_live_player: bool,
    // `CreateDebris` only.
    pub mass: f32,
    pub animation_sets: Vec<AnimSet>,
    pub fx_final: Option<String>,
    pub ok_to_change_model_color: bool,
    pub min_lod_required: Option<String>,
    pub shadow_type: Mask<ShadowType>,
    pub bounce_sound: Option<String>,
    /// Any field not in the tables above, kept verbatim.
    pub extra: Vec<RawItem>,
}

impl GenericNugget {
    fn new(is_object: bool) -> Self {
        GenericNugget {
            is_object,
            names: Vec::new(),
            put_in_container: None,
            particle_system: None,
            count: 1,
            ignore_primary_obstacle: false,
            orient_in_force_direction: false,
            extra_bounciness: 0.0,
            extra_friction: 0.0,
            offset: (0.0, 0.0, 0.0),
            disposition: Mask::EMPTY.with(DebrisDisposition::OnGroundAligned),
            disposition_intensity: 0.0,
            spin_rate: -1.0,
            yaw_rate: -1.0,
            roll_rate: -1.0,
            pitch_rate: -1.0,
            min_force_magnitude: 0.0,
            max_force_magnitude: 0.0,
            min_force_pitch: 0.0,
            max_force_pitch: 0.0,
            min_lifetime: 0,
            max_lifetime: 0,
            spread_formation: false,
            min_distance_a_formation: 0.0,
            min_distance_b_formation: 0.0,
            max_distance_formation: 0.0,
            fade_in: false,
            fade_out: false,
            fade_frames: 0,
            fade_sound: None,
            preserve_layer: true,
            dies_on_bad_land: false,
            contain_inside_source_object: false,
            object_count: 0,
            inherits_veterancy: false,
            skip_if_significantly_airborne: false,
            invulnerable_time: 0,
            min_health: 1.0,
            max_health: 1.0,
            requires_live_player: false,
            mass: 0.0,
            animation_sets: Vec::new(),
            fx_final: None,
            ok_to_change_model_color: false,
            min_lod_required: None,
            shadow_type: Mask::EMPTY,
            bounce_sound: None,
            extra: Vec::new(),
        }
    }
}

/// One `AnimationSet = initial flying final` (`parseAnimSet`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimSet {
    pub initial: String,
    pub flying: String,
    pub final_: String,
}

/// `ApplyRandomForceNugget`.
#[derive(Debug, Clone, PartialEq)]
pub struct ApplyRandomForce {
    /// `SpinRate`, radians per frame.
    pub spin_rate: f32,
    pub min_force_magnitude: f32,
    pub max_force_magnitude: f32,
    /// Radians.
    pub min_force_pitch: f32,
    pub max_force_pitch: f32,
}

/// `FireWeaponNugget`.
#[derive(Debug, Clone, PartialEq)]
pub struct FireWeaponNugget {
    pub weapon: Ref<WeaponId>,
}

/// `AttackNugget`.
#[derive(Debug, Clone, PartialEq)]
pub struct AttackNugget {
    pub number_of_shots: i32,
    pub weapon_slot: WeaponSlot,
    pub delivery_decal: Option<RawBlock>,
    pub delivery_decal_radius: f32,
}

/// `DeliverPayloadNugget::Payload`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    pub name: String,
    pub count: i32,
}

/// `DeliverPayloadNugget` plus the embedded `DeliverPayloadData`.
#[derive(Debug, Clone, PartialEq)]
pub struct DeliverPayload {
    pub transport: String,
    pub start_at_preferred_height: bool,
    pub start_at_max_speed: bool,
    pub formation_size: u32,
    pub formation_spacing: f32,
    pub convergence_factor: f32,
    pub error_radius: f32,
    pub delay_delivery_frames_max: u32,
    pub payload: Vec<Payload>,
    pub put_in_container: Option<String>,
    pub data: DeliverPayloadData,
}

/// `DeliverPayloadData`. Defaults are its constructor's.
#[derive(Debug, Clone, PartialEq)]
pub struct DeliverPayloadData {
    pub delivery_distance: f32,
    pub pre_open_distance: f32,
    pub max_attempts: i32,
    /// Frames.
    pub drop_delay: u32,
    pub drop_offset: (f32, f32, f32),
    pub drop_variance: (f32, f32, f32),
    pub inherit_transport_velocity: bool,
    /// Radians per frame.
    pub exit_pitch_rate: f32,
    pub is_parachute_directly: bool,
    pub visible_items_dropped_per_interval: i32,
    pub visible_drop_bone_name: Option<String>,
    pub visible_sub_object_name: Option<String>,
    pub visible_num_bones: i32,
    pub visible_payload_template_name: Option<String>,
    pub visible_payload_weapon_template: Option<Ref<WeaponId>>,
    pub self_destruct_object: bool,
    pub fire_weapon: bool,
    pub dive_start_distance: f32,
    pub dive_end_distance: f32,
    pub strafing_weapon_slot: Option<WeaponSlot>,
    pub strafe_fx: Option<String>,
    pub strafe_length: f32,
    pub delivery_decal: Option<RawBlock>,
    pub delivery_decal_radius: f32,
}

impl DeliverPayloadData {
    fn new() -> Self {
        DeliverPayloadData {
            delivery_distance: 0.0,
            pre_open_distance: 0.0,
            max_attempts: 1,
            drop_delay: 0,
            drop_offset: (0.0, 0.0, 0.0),
            drop_variance: (0.0, 0.0, 0.0),
            inherit_transport_velocity: false,
            exit_pitch_rate: 0.0,
            is_parachute_directly: false,
            visible_items_dropped_per_interval: 0,
            visible_drop_bone_name: None,
            visible_sub_object_name: None,
            visible_num_bones: 0,
            visible_payload_template_name: None,
            visible_payload_weapon_template: None,
            self_destruct_object: false,
            fire_weapon: false,
            dive_start_distance: 0.0,
            dive_end_distance: 0.0,
            strafing_weapon_slot: None,
            strafe_fx: None,
            strafe_length: 0.0,
            delivery_decal: None,
            delivery_decal_radius: 0.0,
        }
    }
}

impl ObjectCreationList {
    pub fn new(name: &str) -> Self {
        ObjectCreationList { name: name.to_string(), nuggets: Vec::new() }
    }

    /// `ObjectCreationListStore::parseObjectCreationListDefinition` (a fresh `ObjectCreationList`,
    /// since the caller `clear()`s it).
    pub(crate) fn from_block(b: &Block, d: &mut Diag) -> Self {
        let mut ocl = ObjectCreationList::new(b.name().unwrap_or(""));
        d.push_scope(format!("ObjectCreationList {}", ocl.name));
        for item in &b.items {
            match item {
                Item::Field(f) => d.unknown_field(f),
                Item::Block(nb) => {
                    let nugget = parse_nugget(nb, d);
                    ocl.nuggets.push(nugget);
                }
            }
        }
        d.pop_scope();
        ocl
    }
}

/// `TheObjectCreationListFieldParse` dispatch.
fn parse_nugget(b: &Block, d: &mut Diag) -> Nugget {
    d.push_scope(b.kind.clone());
    let nugget = match b.kind.as_str() {
        "CreateObject" => Nugget::CreateObject(parse_generic(b, true, d)),
        "CreateDebris" => Nugget::CreateDebris(parse_generic(b, false, d)),
        "ApplyRandomForce" => Nugget::ApplyRandomForce(parse_apply_random_force(b, d)),
        "DeliverPayload" => Nugget::DeliverPayload(parse_deliver_payload(b, d)),
        "FireWeapon" => Nugget::FireWeapon(parse_fire_weapon(b, d)),
        "Attack" => Nugget::Attack(parse_attack(b, d)),
        _ => {
            d.warn(WarningKind::UnknownField, &b.kind, b.line, format!("unknown OCL nugget `{}`", b.kind));
            Nugget::Unparsed { kind: b.kind.clone(), items: RawItem::from_block_items(&b.items) }
        }
    };
    d.pop_scope();
    nugget
}

/// `GenericObjectCreationNugget::getCommonFieldParse` + `parseObject` / `parseDebris`.
fn parse_generic(b: &Block, is_object: bool, d: &mut Diag) -> GenericNugget {
    let mut n = GenericNugget::new(is_object);
    for item in &b.items {
        let Item::Field(f) = item else {
            // only `DeliveryDecal` nests, and only in DeliverPayload / Attack
            if let Item::Block(nb) = item {
                n.extra.push(RawItem::Block(RawBlock::from(nb)));
            }
            continue;
        };
        let mut a = Args::new(&f.value);
        let r: Result<bool, String> = (|| {
            match f.key.as_str() {
                // common
                "PutInContainer" => n.put_in_container = none_if_empty(a.next()?.to_string()),
                "ParticleSystem" => n.particle_system = none_if_empty(a.next()?.to_string()),
                "Count" => n.count = a.int()?,
                "IgnorePrimaryObstacle" => n.ignore_primary_obstacle = a.boolean()?,
                "OrientInForceDirection" => n.orient_in_force_direction = a.boolean()?,
                "ExtraBounciness" => n.extra_bounciness = a.real()?,
                "ExtraFriction" => n.extra_friction = friction_per_sec_to_per_frame(a.real()?),
                "Offset" => n.offset = parse_coord3d(&f.value)?,
                "Disposition" => n.disposition = n.disposition.parse(a.rest())?,
                "DispositionIntensity" => n.disposition_intensity = a.real()?,
                "SpinRate" => n.spin_rate = a.angular_velocity()?,
                "YawRate" => n.yaw_rate = a.angular_velocity()?,
                "RollRate" => n.roll_rate = a.angular_velocity()?,
                "PitchRate" => n.pitch_rate = a.angular_velocity()?,
                "MinForceMagnitude" => n.min_force_magnitude = a.real()?,
                "MaxForceMagnitude" => n.max_force_magnitude = a.real()?,
                "MinForcePitch" => n.min_force_pitch = a.angle()?,
                "MaxForcePitch" => n.max_force_pitch = a.angle()?,
                "MinLifetime" => n.min_lifetime = a.duration_frames()?,
                "MaxLifetime" => n.max_lifetime = a.duration_frames()?,
                "SpreadFormation" => n.spread_formation = a.boolean()?,
                "MinDistanceAFormation" => n.min_distance_a_formation = a.real()?,
                "MinDistanceBFormation" => n.min_distance_b_formation = a.real()?,
                "MaxDistanceFormation" => n.max_distance_formation = a.real()?,
                "FadeIn" => n.fade_in = a.boolean()?,
                "FadeOut" => n.fade_out = a.boolean()?,
                "FadeTime" => n.fade_frames = a.duration_frames()?,
                "FadeSound" => n.fade_sound = none_if_empty(a.next()?.to_string()),
                "PreserveLayer" => n.preserve_layer = a.boolean()?,
                "DiesOnBadLand" => n.dies_on_bad_land = a.boolean()?,
                // CreateObject
                "ContainInsideSourceObject" => n.contain_inside_source_object = a.boolean()?,
                "ObjectNames" => n.names = a.rest().iter().map(|s| s.to_string()).collect(),
                "ObjectCount" => n.object_count = a.int()?,
                "InheritsVeterancy" => n.inherits_veterancy = a.boolean()?,
                "SkipIfSignificantlyAirborne" => n.skip_if_significantly_airborne = a.boolean()?,
                "InvulnerableTime" => n.invulnerable_time = a.duration_frames()?,
                "MinHealth" => n.min_health = a.percent()?,
                "MaxHealth" => n.max_health = a.percent()?,
                "RequiresLivePlayer" => n.requires_live_player = a.boolean()?,
                // CreateDebris
                "ModelNames" => n.names = a.rest().iter().map(|s| s.to_string()).collect(),
                "Mass" => n.mass = positive_nonzero(&mut a)?,
                "AnimationSet" => n.animation_sets.push(parse_anim_set(&mut a)?),
                "FXFinal" => n.fx_final = none_if_none(a.next()?.to_string()),
                "OkToChangeModelColor" => n.ok_to_change_model_color = a.boolean()?,
                "MinLODRequired" => n.min_lod_required = Some(a.next()?.to_string()),
                "Shadow" => n.shadow_type = n.shadow_type.parse(a.rest())?,
                "BounceSound" => n.bounce_sound = none_if_none(a.next()?.to_string()),
                _ => return Ok(false),
            }
            Ok(true)
        })();
        match r {
            Ok(true) => {}
            Ok(false) => {
                d.unknown_field(f);
                n.extra.push(RawItem::Field(crate::diag::RawField {
                    key: f.key.clone(),
                    value: f.value.clone(),
                    line: f.line,
                }));
            }
            Err(e) => d.bad_value(f, e),
        }
    }
    n
}

fn parse_anim_set(a: &mut Args) -> Result<AnimSet, String> {
    let initial = a.next()?.to_string();
    let flying = a.next()?.to_string();
    let final_ = a.next()?.to_string();
    Ok(AnimSet { initial, flying, final_ })
}

fn parse_apply_random_force(b: &Block, d: &mut Diag) -> ApplyRandomForce {
    let mut n = ApplyRandomForce {
        spin_rate: 0.0,
        min_force_magnitude: 0.0,
        max_force_magnitude: 0.0,
        min_force_pitch: 0.0,
        max_force_pitch: 0.0,
    };
    for item in &b.items {
        let Item::Field(f) = item else {
            d.warn(WarningKind::Structure, "", 0, "unexpected nested block in ApplyRandomForce");
            continue;
        };
        let mut a = Args::new(&f.value);
        let r: Result<bool, String> = (|| {
            match f.key.as_str() {
                "SpinRate" => n.spin_rate = a.angular_velocity()?,
                "MinForceMagnitude" => n.min_force_magnitude = a.real()?,
                "MaxForceMagnitude" => n.max_force_magnitude = a.real()?,
                "MinForcePitch" => n.min_force_pitch = a.angle()?,
                "MaxForcePitch" => n.max_force_pitch = a.angle()?,
                _ => return Ok(false),
            }
            Ok(true)
        })();
        match r {
            Ok(true) => {}
            Ok(false) => d.unknown_field(f),
            Err(e) => d.bad_value(f, e),
        }
    }
    n
}

fn parse_fire_weapon(b: &Block, d: &mut Diag) -> FireWeaponNugget {
    let mut weapon = Ref::named(String::new());
    for item in &b.items {
        let Item::Field(f) = item else {
            d.warn(WarningKind::Structure, "", 0, "unexpected nested block in FireWeapon");
            continue;
        };
        let mut a = Args::new(&f.value);
        match f.key.as_str() {
            "Weapon" => match a.next() {
                Ok(t) => weapon = Ref::named(t.to_string()),
                Err(e) => d.bad_value(f, e),
            },
            _ => d.unknown_field(f),
        }
    }
    FireWeaponNugget { weapon }
}

fn parse_attack(b: &Block, d: &mut Diag) -> AttackNugget {
    let mut n = AttackNugget {
        number_of_shots: 1,
        weapon_slot: WeaponSlot::Primary,
        delivery_decal: None,
        delivery_decal_radius: 0.0,
    };
    for item in &b.items {
        match item {
            Item::Block(nb) if nb.kind == "DeliveryDecal" => {
                n.delivery_decal = Some(RawBlock::from(nb));
            }
            Item::Block(nb) => d.warn(WarningKind::Structure, &nb.kind, nb.line, "unexpected nested block"),
            Item::Field(f) => {
                let mut a = Args::new(&f.value);
                let r: Result<bool, String> = (|| {
                    match f.key.as_str() {
                        "NumberOfShots" => n.number_of_shots = a.int()?,
                        "WeaponSlot" => n.weapon_slot = a.index()?,
                        "DeliveryDecalRadius" => n.delivery_decal_radius = a.real()?,
                        _ => return Ok(false),
                    }
                    Ok(true)
                })();
                match r {
                    Ok(true) => {}
                    Ok(false) => d.unknown_field(f),
                    Err(e) => d.bad_value(f, e),
                }
            }
        }
    }
    n
}

fn parse_deliver_payload(b: &Block, d: &mut Diag) -> DeliverPayload {
    let mut n = DeliverPayload {
        transport: String::new(),
        start_at_preferred_height: true,
        start_at_max_speed: false,
        formation_size: 1,
        formation_spacing: 25.0,
        convergence_factor: 0.0,
        error_radius: 0.0,
        delay_delivery_frames_max: 0,
        payload: Vec::new(),
        put_in_container: None,
        data: DeliverPayloadData::new(),
    };
    for item in &b.items {
        match item {
            Item::Block(nb) if nb.kind == "DeliveryDecal" => {
                n.data.delivery_decal = Some(RawBlock::from(nb));
            }
            Item::Block(nb) => d.warn(WarningKind::Structure, &nb.kind, nb.line, "unexpected nested block"),
            Item::Field(f) => {
                let mut a = Args::new(&f.value);
                let r: Result<bool, String> = (|| {
                    // DeliverPayloadNugget-specific data.
                    match f.key.as_str() {
                        "Transport" => {
                            n.transport = a.next()?.to_string();
                            return Ok(true);
                        }
                        "StartAtPreferredHeight" => {
                            n.start_at_preferred_height = a.boolean()?;
                            return Ok(true);
                        }
                        "StartAtMaxSpeed" => {
                            n.start_at_max_speed = a.boolean()?;
                            return Ok(true);
                        }
                        "FormationSize" => {
                            n.formation_size = a.uint()?;
                            return Ok(true);
                        }
                        "FormationSpacing" => {
                            n.formation_spacing = a.real()?;
                            return Ok(true);
                        }
                        "WeaponConvergenceFactor" => {
                            n.convergence_factor = a.real()?;
                            return Ok(true);
                        }
                        "WeaponErrorRadius" => {
                            n.error_radius = a.real()?;
                            return Ok(true);
                        }
                        "DelayDeliveryMax" => {
                            n.delay_delivery_frames_max = a.duration_frames()?;
                            return Ok(true);
                        }
                        "Payload" => {
                            let name = a.next()?.to_string();
                            let count = match a.next_opt() {
                                Some(t) => crate::value::scan_int(t)?,
                                None => 1,
                            };
                            n.payload.push(Payload { name, count });
                            return Ok(true);
                        }
                        "PutInContainer" => {
                            n.put_in_container = none_if_empty(a.next()?.to_string());
                            return Ok(true);
                        }
                        _ => {}
                    }
                    // DeliverPayloadData.
                    match f.key.as_str() {
                        "DeliveryDistance" => n.data.delivery_distance = a.real()?,
                        "PreOpenDistance" => n.data.pre_open_distance = a.real()?,
                        "MaxAttempts" => n.data.max_attempts = a.int()?,
                        "DropDelay" => n.data.drop_delay = a.duration_frames()?,
                        "DropOffset" => n.data.drop_offset = parse_coord3d(&f.value)?,
                        "DropVariance" => n.data.drop_variance = parse_coord3d(&f.value)?,
                        "InheritTransportVelocity" => n.data.inherit_transport_velocity = a.boolean()?,
                        "ExitPitchRate" => n.data.exit_pitch_rate = a.angular_velocity()?,
                        "ParachuteDirectly" => n.data.is_parachute_directly = a.boolean()?,
                        "VisibleItemsDroppedPerInterval" => n.data.visible_items_dropped_per_interval = a.int()?,
                        "VisibleDropBoneBaseName" => n.data.visible_drop_bone_name = none_if_empty(a.next()?.to_string()),
                        "VisibleSubObjectBaseName" => n.data.visible_sub_object_name = none_if_empty(a.next()?.to_string()),
                        "VisibleNumBones" => n.data.visible_num_bones = a.int()?,
                        "VisiblePayloadTemplateName" => {
                            n.data.visible_payload_template_name = none_if_empty(a.next()?.to_string())
                        }
                        "VisiblePayloadWeaponTemplate" => {
                            n.data.visible_payload_weapon_template = Some(Ref::named(a.next()?.to_string()))
                        }
                        "SelfDestructObject" => n.data.self_destruct_object = a.boolean()?,
                        "FireWeapon" => n.data.fire_weapon = a.boolean()?,
                        "DiveStartDistance" => n.data.dive_start_distance = a.real()?,
                        "DiveEndDistance" => n.data.dive_end_distance = a.real()?,
                        "StrafingWeaponSlot" => n.data.strafing_weapon_slot = Some(a.index()?),
                        "StrafeWeaponFX" => n.data.strafe_fx = none_if_none(a.next()?.to_string()),
                        "StrafeLength" => n.data.strafe_length = a.real()?,
                        "DeliveryDecalRadius" => n.data.delivery_decal_radius = a.real()?,
                        _ => return Ok(false),
                    }
                    Ok(true)
                })();
                match r {
                    Ok(true) => {}
                    Ok(false) => d.unknown_field(f),
                    Err(e) => d.bad_value(f, e),
                }
            }
        }
    }
    n
}

/// `parsePositiveNonZeroReal`.
fn positive_nonzero(a: &mut Args) -> Result<f32, String> {
    let v = a.real()?;
    if v <= 0.0 {
        Err(format!("invalid Real value {v}, expected > 0"))
    } else {
        Ok(v)
    }
}

/// `INI::parseFXList` / `parseAudioEventRTS`: the keyword `None` (or empty) means no name.
fn none_if_none(s: String) -> Option<String> {
    if s.is_empty() || s.eq_ignore_ascii_case("None") {
        None
    } else {
        Some(s)
    }
}

/// An ASCII string field: empty means unset.
fn none_if_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}
