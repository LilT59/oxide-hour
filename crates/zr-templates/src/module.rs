//! Module blocks of a thing template (`Body`, `Behavior`, `Draw`, `ClientUpdate`) and the typed
//! module data the simulation needs first. Everything else stays raw in [`ModuleData::Unparsed`].
//!
//! Ported from `ThingTemplate::parseModuleName` (`Core/.../Thing/ThingTemplate.cpp`),
//! `ModuleFactory` (`Common/Thing/ModuleFactory.cpp`), `ActiveBody.cpp` and friends,
//! `AIUpdate.cpp`, `TurretAI.cpp` and `PhysicsUpdate.cpp`.

use std::collections::BTreeMap;

use zr_ini::{Block, Item};

use crate::diag::{Diag, RawBlock, RawItem, WarningKind};
use crate::contain::{ContainData, ContainKind};
use crate::die::{is_typed_die_class, DieData};
use crate::flags::{Mask, NameEnum};
use crate::module_table::{iface, AI_MODULES, MODULE_INTERFACES};
use crate::dock::{is_typed_dock_class, DockData};
use crate::production::ProductionData;
use crate::production_exit::{is_typed_production_exit_class, ProductionExitData};
use crate::projectile::{DumbProjectileData, MissileAiData};
use crate::upgrade_module::{is_typed_upgrade_class, UpgradeData};
use crate::names::*;
use crate::registry::{LocomotorId, Ref, WeaponId};
use crate::value::{friction_per_sec_to_per_frame, Args, LOGICFRAMES_PER_SECOND};

/// Which of the template's three module lists a module lives in. `Body = ...` modules are
/// `Behavior` modules whose interface mask has the `BODY` bit (`MODULETYPE_BEHAVIOR`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModuleKind {
    Behavior,
    Draw,
    ClientUpdate,
}

impl ModuleKind {
    pub fn name(self) -> &'static str {
        match self {
            ModuleKind::Behavior => "Behavior",
            ModuleKind::Draw => "Draw",
            ModuleKind::ClientUpdate => "ClientUpdate",
        }
    }
}

/// One module entry (`ModuleInfo::Nugget`).
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub kind: ModuleKind,
    /// Module class name, e.g. `ActiveBody`, `W3DModelDraw`.
    pub name: String,
    /// The mandatory unique tag, e.g. `ModuleTag_01`.
    pub tag: String,
    /// `MODULEINTERFACE_*` bits ([`crate::iface`]).
    pub interface_mask: u32,
    /// Declared inside `InheritableModule`.
    pub inheritable: bool,
    /// Declared inside `OverrideableByLikeKind`.
    pub overrideable_by_like_kind: bool,
    /// Copied from `DefaultThingTemplate` (or a reskin parent) and not yet replaced.
    pub copied_from_default: bool,
    pub data: ModuleData,
}

impl Module {
    /// `ModuleData::isAiModuleData`.
    pub fn is_ai(&self) -> bool {
        matches!(self.data, ModuleData::Ai(_)) || AI_MODULES.binary_search(&self.name.as_str()).is_ok()
    }
    pub fn is_body(&self) -> bool {
        self.interface_mask & iface::BODY != 0
    }
}

/// Parsed module configuration.
#[derive(Debug, Clone, PartialEq)]
pub enum ModuleData {
    /// `ActiveBody` family.
    Body(BodyData),
    /// `AIUpdateInterface` and every module class derived from it.
    Ai(AiUpdateData),
    /// `PhysicsBehavior`.
    Physics(PhysicsData),
    /// A die module (`DestroyDie`, `InstantDeathBehavior`, `CreateObjectDie`, `FXListDie`,
    /// `SlowDeathBehavior`).
    Die(DieData),
    /// An upgrade module (`ArmorUpgrade`, `WeaponSetUpgrade`, `LocomotorSetUpgrade`,
    /// `MaxHealthUpgrade`).
    Upgrade(UpgradeData),
    /// `ProductionUpdate`.
    Production(ProductionData),
    /// A production exit update (`DefaultProductionExitUpdate`, `QueueProductionExitUpdate`,
    /// `SupplyCenterProductionExitUpdate`).
    ProductionExit(ProductionExitData),
    /// A supply dock update (`SupplyWarehouseDockUpdate`, `SupplyCenterDockUpdate`).
    Dock(DockData),
    /// `DumbProjectileBehavior` (Bezier ballistic arc).
    DumbProjectile(Box<DumbProjectileData>),
    /// A contain module (`OpenContain`, `TransportContain`, `GarrisonContain`, `OverlordContain`,
    /// `HelixContain`, `TunnelContain`).
    Contain(Box<ContainData>),
    /// A special-power module (`OCLSpecialPower`, `FireWeaponPower`, `CashHackSpecialPower`).
    SpecialPower(Box<crate::special_power::SpecialPowerModuleData>),
    /// A unit special ability (`SpecialAbilityUpdate`: Black Lotus, Jarmen Kell, Burton, ...).
    SpecialAbility(Box<crate::special_power::SpecialAbilityData>),
    /// `AutoHealBehavior`.
    AutoHeal(Box<crate::behavior::AutoHealData>),
    /// `FlammableUpdate`.
    Flammable(crate::behavior::FlammableData),
    /// `PoisonedBehavior`.
    Poisoned(crate::behavior::PoisonedData),
    /// `FireWeaponWhenDeadBehavior`.
    FireWeaponWhenDead(crate::behavior::FireWeaponWhenDeadData),
    /// `LifetimeUpdate`.
    Lifetime(crate::behavior::LifetimeData),
    /// `StealthUpdate`.
    Stealth(Box<crate::behavior::StealthData>),
    /// `StealthDetectorUpdate`.
    StealthDetector(crate::behavior::StealthDetectorData),
    /// `SquishCollide` (marker).
    SquishCollide(crate::modules2::SquishCollideData),
    /// A crate collision (`MoneyCrateCollide`, `HealCrateCollide`, ...).
    CrateCollide(Box<crate::modules2::CrateCollideData>),
    /// `FireWeaponCollide` (shoot the object that collides with me).
    FireWeaponCollide(crate::modules2::FireWeaponCollideData),
    /// `VeterancyGainCreate`.
    VeterancyGainCreate(crate::modules2::VeterancyGainCreateData),
    /// `ToppleUpdate`.
    Topple(crate::modules2::ToppleData),
    /// `SpawnBehavior`.
    Spawn(Box<crate::modules2::SpawnData>),
    /// `SlavedUpdate`.
    Slaved(crate::modules2::SlavedData),
    /// `HordeUpdate`.
    Horde(crate::modules2::HordeData),
    /// `AutoDepositUpdate`.
    AutoDeposit(crate::modules2::AutoDepositData),
    /// `BaseRegenerateUpdate`.
    BaseRegenerate(crate::modules2::BaseRegenerateData),
    /// `PropagandaTowerBehavior`.
    Propaganda(crate::modules2::PropagandaData),
    /// `FireWeaponUpdate`.
    FireWeapon(crate::modules2::FireWeaponData),
    /// `RepairDockUpdate`.
    RepairDock(crate::modules2::RepairDockData),
    /// `OverchargeBehavior` / `PowerPlantUpdate`.
    Overcharge(crate::modules2::OverchargeData),
    /// `MinefieldBehavior`.
    Minefield(Box<crate::modules2::MinefieldData>),
    /// `ProneUpdate` / `DemoTrapUpdate` / `CleanupHazardUpdate` / `CommandButtonHuntUpdate`.
    SimpleUpdate(crate::modules2::SimpleUpdateData),
    /// `TransitionDamageFX`.
    TransitionDamage(crate::modules2::TransitionDamageData),
    /// `ParkingPlaceBehavior`.
    ParkingPlace(crate::modules2::ParkingPlaceData),
    /// `CountermeasuresBehavior`.
    Countermeasures(crate::modules2::CountermeasuresData),
    /// `RebuildHoleBehavior`.
    RebuildHole(crate::modules2::RebuildHoleData),
    /// `RebuildHoleExposeDie`.
    RebuildHoleExpose(crate::modules2::RebuildHoleExposeData),
    /// `GenerateMinefieldBehavior`.
    GenerateMinefield(crate::modules2::GenerateMinefieldData),
    /// One of the remaining typed `GameLogic/Object` logic modules (see [`crate::logic`]). These are
    /// parsed for data completeness; the simulation does not read them yet.
    Logic(Box<crate::logic::LogicModule>),
    /// A client draw module with data beyond the condition states `zr-w3d` parses (see
    /// [`crate::draw`]): the tread/tire fields of the W3D tank/truck draws.
    Draw(Box<crate::draw::DrawModule>),
    /// Not interpreted yet; the fields exactly as written.
    Unparsed { module_type: String, tag: String, fields: Vec<RawItem> },
}

/// `MODULEINTERFACE_*` lookup by module class name (`ModuleFactory::findModuleInterfaceMask`).
pub fn interface_mask(class_name: &str) -> Option<u32> {
    MODULE_INTERFACES
        .binary_search_by(|(n, _)| (*n).cmp(class_name))
        .ok()
        .map(|i| MODULE_INTERFACES[i].1)
}

// ---------------------------------------------------------------------------------------------
// Bodies

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyClass {
    Inactive,
    Active,
    Structure,
    Immortal,
    Highlander,
    HiveStructure,
    Undead,
}

impl BodyClass {
    pub fn from_class_name(n: &str) -> Option<Self> {
        Some(match n {
            "InactiveBody" => BodyClass::Inactive,
            "ActiveBody" => BodyClass::Active,
            "StructureBody" => BodyClass::Structure,
            "ImmortalBody" => BodyClass::Immortal,
            "HighlanderBody" => BodyClass::Highlander,
            "HiveStructureBody" => BodyClass::HiveStructure,
            "UndeadBody" => BodyClass::Undead,
            _ => return None,
        })
    }
}

/// `ActiveBodyModuleData` plus the fields of its subclasses (`UndeadBody`, `HiveStructureBody`).
#[derive(Debug, Clone, PartialEq)]
pub struct BodyData {
    pub class: BodyClass,
    pub max_health: f32,
    pub initial_health: f32,
    pub subdual_damage_cap: f32,
    /// `SubdualDamageHealRate`, frames (rounded up).
    pub subdual_damage_heal_rate: u32,
    pub subdual_damage_heal_amount: f32,
    /// `UndeadBody` only.
    pub second_life_max_health: f32,
    /// `HiveStructureBody` only: `PropagateDamageTypesToSlavesWhenExisting`.
    pub damage_types_to_propagate_to_slaves: Mask<DamageType>,
    /// `HiveStructureBody` only: `SwallowDamageTypesIfSlavesNotExisting`.
    pub damage_types_to_swallow: Mask<DamageType>,
}

/// `INI::parseDamageTypeFlags`: starts at ALL; `ALL`, `NONE`, `+TYPE`, `-TYPE`.
fn parse_damage_type_flags(toks: &[&str]) -> Result<Mask<DamageType>, String> {
    let mut m: Mask<DamageType> = DamageType::ALL.iter().copied().collect();
    for t in toks {
        if t.eq_ignore_ascii_case("ALL") {
            m = DamageType::ALL.iter().copied().collect();
        } else if t.eq_ignore_ascii_case("NONE") {
            m = Mask::EMPTY;
        } else if let Some(n) = t.strip_prefix('+') {
            m.insert(DamageType::from_name(n).ok_or_else(|| format!("unknown damage type `{n}`"))?);
        } else if let Some(n) = t.strip_prefix('-') {
            m.remove(DamageType::from_name(n).ok_or_else(|| format!("unknown damage type `{n}`"))?);
        } else {
            return Err(format!("unexpected token `{t}` (expected ALL, NONE, +TYPE or -TYPE)"));
        }
    }
    Ok(m)
}

fn parse_body(class: BodyClass, block: &Block, d: &mut Diag) -> BodyData {
    let mut b = BodyData {
        class,
        max_health: 0.0,
        initial_health: 0.0,
        subdual_damage_cap: 0.0,
        subdual_damage_heal_rate: 0,
        subdual_damage_heal_amount: 0.0,
        second_life_max_health: 1.0,
        damage_types_to_propagate_to_slaves: Mask::EMPTY,
        damage_types_to_swallow: Mask::EMPTY,
    };
    // `InactiveBody` has no module data of its own (plain `ModuleData`): none of the fields are valid.
    let active = class != BodyClass::Inactive;
    for item in &block.items {
        let f = match item {
            Item::Field(f) => f,
            Item::Block(nb) => {
                d.warn(WarningKind::Structure, &nb.kind, nb.line, "unexpected nested block in body module");
                continue;
            }
        };
        let mut a = Args::new(&f.value);
        let r: Result<bool, String> = (|| {
            if active {
                match f.key.as_str() {
                    "MaxHealth" => b.max_health = a.real()?,
                    "InitialHealth" => b.initial_health = a.real()?,
                    "SubdualDamageCap" => b.subdual_damage_cap = a.real()?,
                    "SubdualDamageHealRate" => b.subdual_damage_heal_rate = a.duration_frames()?,
                    "SubdualDamageHealAmount" => b.subdual_damage_heal_amount = a.real()?,
                    "SecondLifeMaxHealth" if class == BodyClass::Undead => b.second_life_max_health = a.real()?,
                    "PropagateDamageTypesToSlavesWhenExisting" if class == BodyClass::HiveStructure => {
                        b.damage_types_to_propagate_to_slaves = parse_damage_type_flags(a.rest())?
                    }
                    "SwallowDamageTypesIfSlavesNotExisting" if class == BodyClass::HiveStructure => {
                        b.damage_types_to_swallow = parse_damage_type_flags(a.rest())?
                    }
                    _ => return Ok(false),
                }
                Ok(true)
            } else {
                Ok(false)
            }
        })();
        match r {
            Ok(true) => {}
            Ok(false) => d.unknown_field(f),
            Err(e) => d.bad_value(f, e),
        }
    }
    b
}

// ---------------------------------------------------------------------------------------------
// AI

/// `TurretAIData` (`TurretAI.cpp`). Angles are radians, rates radians per frame, times frames.
#[derive(Debug, Clone, PartialEq)]
pub struct TurretAiData {
    /// `TurretTurnRate`, radians per frame (default 0.01 = `DEFAULT_TURN_RATE`).
    pub turn_rate: f32,
    /// `TurretPitchRate` (default 0.01).
    pub pitch_rate: f32,
    pub natural_turret_angle: f32,
    pub natural_turret_pitch: f32,
    pub fire_pitch: f32,
    pub min_physical_pitch: f32,
    pub ground_unit_pitch: f32,
    /// `TurretFireAngleSweep` per weapon slot.
    pub turret_fire_angle_sweep: [f32; 3],
    /// `TurretSweepSpeedModifier` per weapon slot (default 1).
    pub turret_sweep_speed_modifier: [f32; 3],
    pub controlled_weapon_slots: Mask<WeaponSlot>,
    pub allows_pitch: bool,
    pub min_idle_scan_angle: f32,
    pub max_idle_scan_angle: f32,
    pub min_idle_scan_interval: u32,
    pub max_idle_scan_interval: u32,
    /// `RecenterTime`, frames (default 2 seconds).
    pub recenter_time: u32,
    pub initially_disabled: bool,
    pub fires_while_turning: bool,
}

impl Default for TurretAiData {
    fn default() -> Self {
        TurretAiData {
            turn_rate: 0.01,
            pitch_rate: 0.01,
            natural_turret_angle: 0.0,
            natural_turret_pitch: 0.0,
            fire_pitch: 0.0,
            min_physical_pitch: 0.0,
            ground_unit_pitch: 0.0,
            turret_fire_angle_sweep: [0.0; 3],
            turret_sweep_speed_modifier: [1.0; 3],
            controlled_weapon_slots: Mask::EMPTY,
            allows_pitch: false,
            min_idle_scan_angle: 0.0,
            max_idle_scan_angle: 0.0,
            min_idle_scan_interval: 9_999_999,
            max_idle_scan_interval: 9_999_999,
            recenter_time: 2 * LOGICFRAMES_PER_SECOND,
            initially_disabled: false,
            fires_while_turning: false,
        }
    }
}

impl TurretAiData {
    fn field(&mut self, key: &str, a: &mut Args) -> Result<bool, String> {
        match key {
            "TurretTurnRate" => self.turn_rate = a.angular_velocity()?,
            "TurretPitchRate" => self.pitch_rate = a.angular_velocity()?,
            "NaturalTurretAngle" => self.natural_turret_angle = a.angle()?,
            "NaturalTurretPitch" => self.natural_turret_pitch = a.angle()?,
            "FirePitch" => self.fire_pitch = a.angle()?,
            "MinPhysicalPitch" => self.min_physical_pitch = a.angle()?,
            "GroundUnitPitch" => self.ground_unit_pitch = a.angle()?,
            "TurretFireAngleSweep" => {
                let slot: WeaponSlot = a.index()?;
                self.turret_fire_angle_sweep[slot.index()] = a.angle()?;
            }
            "TurretSweepSpeedModifier" => {
                let slot: WeaponSlot = a.index()?;
                self.turret_sweep_speed_modifier[slot.index()] = a.real()?;
            }
            // `parseTWS`: OR-in each slot named (at least one is required).
            "ControlledWeaponSlots" => {
                let toks = a.rest().to_vec();
                if toks.is_empty() {
                    return Err("missing value".into());
                }
                for tok in toks {
                    let slot = WeaponSlot::from_name(tok).ok_or_else(|| format!("`{tok}` is not a weapon slot"))?;
                    self.controlled_weapon_slots.insert(slot);
                }
            }
            "AllowsPitch" => self.allows_pitch = a.boolean()?,
            "MinIdleScanAngle" => self.min_idle_scan_angle = a.angle()?,
            "MaxIdleScanAngle" => self.max_idle_scan_angle = a.angle()?,
            "MinIdleScanInterval" => self.min_idle_scan_interval = a.duration_frames()?,
            "MaxIdleScanInterval" => self.max_idle_scan_interval = a.duration_frames()?,
            "RecenterTime" => self.recenter_time = a.duration_frames()?,
            "InitiallyDisabled" => self.initially_disabled = a.boolean()?,
            "FiresWhileTurning" => self.fires_while_turning = a.boolean()?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// `AIUpdateModuleData` plus the locomotor sets attached by `Locomotor = SET_x Name...`
/// (`AIUpdateModuleData::parseLocomotorSet`).
#[derive(Debug, Clone, PartialEq)]
pub struct AiUpdateData {
    /// The concrete module class (`AIUpdateInterface`, `JetAIUpdate`, `DozerAIUpdate`, ...).
    pub class_name: String,
    pub turret: Option<TurretAiData>,
    pub alt_turret: Option<TurretAiData>,
    pub auto_acquire_enemies_when_idle: Mask<AutoAcquire>,
    /// `MoodAttackCheckRate`, frames (default 2 seconds).
    pub mood_attack_check_rate: u32,
    pub forbid_player_commands: bool,
    pub turrets_linked: bool,
    /// Locomotor names per set, in file order (`m_locomotorTemplates`).
    pub locomotor_sets: BTreeMap<LocomotorSetType, Vec<Ref<LocomotorId>>>,
    /// `MissileAIUpdate`-only fields (this class derives from `AIUpdateInterface`).
    pub missile: Option<Box<MissileAiData>>,
    /// Fields and blocks specific to a derived AI class (`JetAIUpdate`, `DozerAIUpdate`, ...),
    /// not interpreted yet. Always empty for plain `AIUpdateInterface`.
    pub extra: Vec<RawItem>,
    /// `JetAIUpdate` fields, parsed out of `extra`.
    pub jet: Option<Box<crate::modules2::JetAiData>>,
}

impl AiUpdateData {
    fn new(class_name: &str) -> Self {
        AiUpdateData {
            class_name: class_name.to_string(),
            turret: None,
            alt_turret: None,
            auto_acquire_enemies_when_idle: Mask::EMPTY,
            mood_attack_check_rate: 2 * LOGICFRAMES_PER_SECOND,
            forbid_player_commands: false,
            turrets_linked: false,
            locomotor_sets: BTreeMap::new(),
            missile: None,
            extra: Vec::new(),
            jet: None,
        }
    }
}

fn parse_turret(b: &Block, d: &mut Diag) -> TurretAiData {
    let mut t = TurretAiData::default();
    d.push_scope(b.kind.clone());
    for item in &b.items {
        let Item::Field(f) = item else {
            d.warn(WarningKind::Structure, "", 0, "unexpected nested block in turret");
            continue;
        };
        let mut a = Args::new(&f.value);
        let r = t.field(&f.key, &mut a);
        match r {
            Ok(true) => {}
            Ok(false) => d.unknown_field(f),
            Err(e) => d.bad_value(f, e),
        }
    }
    d.pop_scope();
    t
}

fn parse_ai(class: &str, block: &Block, d: &mut Diag) -> AiUpdateData {
    let mut ai = AiUpdateData::new(class);
    let exact = class == "AIUpdateInterface";
    let is_missile = class == "MissileAIUpdate";
    for item in &block.items {
        match item {
            Item::Block(b) if b.kind == "Turret" || b.kind == "AltTurret" => {
                let slot = if b.kind == "Turret" { &mut ai.turret } else { &mut ai.alt_turret };
                if slot.is_some() {
                    // the engine throws INI_INVALID_DATA: "Only one turret to a customer, for now"
                    d.warn(WarningKind::Structure, &b.kind, b.line, "turret specified twice");
                    continue;
                }
                let t = parse_turret(b, d);
                if b.kind == "Turret" {
                    ai.turret = Some(t);
                } else {
                    ai.alt_turret = Some(t);
                }
            }
            Item::Block(b) => {
                if exact {
                    d.warn(WarningKind::UnknownField, &b.kind, b.line, format!("unknown block `{}`", b.kind));
                } else {
                    ai.extra.push(RawItem::Block(RawBlock::from(b)));
                }
            }
            Item::Field(f) => {
                let mut a = Args::new(&f.value);
                let r: Result<bool, String> = (|| {
                    match f.key.as_str() {
                        "AutoAcquireEnemiesWhenIdle" => {
                            ai.auto_acquire_enemies_when_idle = ai.auto_acquire_enemies_when_idle.parse(a.rest())?
                        }
                        "MoodAttackCheckRate" => ai.mood_attack_check_rate = a.duration_frames()?,
                        "ForbidPlayerCommands" => ai.forbid_player_commands = a.boolean()?,
                        "TurretsLinked" => ai.turrets_linked = a.boolean()?,
                        _ if is_missile => {
                            return ai.missile.get_or_insert_with(|| Box::new(MissileAiData::default())).field(&f.key, &mut a);
                        }
                        _ => return Ok(false),
                    }
                    Ok(true)
                })();
                match r {
                    Ok(true) => {}
                    Ok(false) if !exact => ai.extra.push(RawItem::from_block_items(std::slice::from_ref(item)).remove(0)),
                    Ok(false) => d.unknown_field(f),
                    Err(e) => d.bad_value(f, e),
                }
            }
        }
    }
    if class == "JetAIUpdate" {
        ai.jet = Some(Box::new(crate::modules2::parse_jet_ai(&ai.extra, d)));
    }
    ai
}

// ---------------------------------------------------------------------------------------------
// Physics

/// `PhysicsBehaviorModuleData` (`PhysicsUpdate.cpp`, `PhysicsUpdate.h`). Frictions are per frame.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsData {
    pub mass: f32,
    pub shock_resistance: f32,
    pub shock_max_yaw: f32,
    pub shock_max_pitch: f32,
    pub shock_max_roll: f32,
    pub forward_friction: f32,
    pub lateral_friction: f32,
    pub z_friction: f32,
    pub aerodynamic_friction: f32,
    pub center_of_mass_offset: f32,
    pub allow_bouncing: bool,
    pub allow_collide_force: bool,
    pub kill_when_resting_on_ground: bool,
    /// `MinFallHeightForDamage` converted with `heightToSpeed` (`sqrt(|2 * gravity * height|)`, using the
    /// `GameData` `Gravity` in force when the module was parsed): speed per frame.
    pub min_fall_speed_for_damage: f32,
    pub fall_height_damage_factor: f32,
    pub pitch_roll_yaw_factor: f32,
    pub vehicle_crashes_into_building_weapon: Option<Ref<WeaponId>>,
    pub vehicle_crashes_into_non_building_weapon: Option<Ref<WeaponId>>,
}

/// `heightToSpeed` in `PhysicsUpdate.cpp`: `sqrt(fabs(2.0f * gravity * height))`.
pub fn height_to_speed(gravity: f32, height: f32) -> f32 {
    (2.0f32 * gravity * height).abs().sqrt()
}

impl PhysicsData {
    pub(crate) fn new(gravity: f32) -> Self {
        PhysicsData {
            mass: 1.0,
            shock_resistance: 0.0,
            shock_max_yaw: 0.05,
            shock_max_pitch: 0.025,
            shock_max_roll: 0.025,
            forward_friction: 0.15,
            lateral_friction: 0.15,
            z_friction: 0.8,
            aerodynamic_friction: 0.0,
            center_of_mass_offset: 0.0,
            allow_bouncing: false,
            allow_collide_force: true,
            kill_when_resting_on_ground: false,
            min_fall_speed_for_damage: height_to_speed(gravity, 40.0),
            fall_height_damage_factor: 1.0,
            pitch_roll_yaw_factor: 2.0,
            vehicle_crashes_into_building_weapon: Some(Ref::named("VehicleCrashesIntoBuildingWeapon".into())),
            vehicle_crashes_into_non_building_weapon: Some(Ref::named("VehicleCrashesIntoNonBuildingWeapon".into())),
        }
    }
}

fn positive_nonzero(a: &mut Args) -> Result<f32, String> {
    let v = a.real()?;
    if v <= 0.0 {
        Err(format!("invalid Real value {v}, expected > 0"))
    } else {
        Ok(v)
    }
}

fn weapon_ref(a: &mut Args) -> Option<Ref<WeaponId>> {
    let n = a.string();
    if n.eq_ignore_ascii_case("None") {
        None
    } else {
        Some(Ref::named(n))
    }
}

/// One `PhysicsBehaviorModuleData` field from its `buildFieldParse` table (`PhysicsUpdate.cpp:172-210`).
/// `Ok(false)` means the key is not a physics field. Shared with `RailroadBehavior`, whose module data
/// derives from `PhysicsBehaviorModuleData` and whose parser runs this table first
/// (`RailroadGuideAIUpdate.h:43-51`).
pub(crate) fn physics_field(p: &mut PhysicsData, key: &str, a: &mut Args, gravity: f32) -> Result<bool, String> {
    match key {
        "Mass" => p.mass = positive_nonzero(a)?,
        "ShockResistance" => p.shock_resistance = positive_nonzero(a)?,
        "ShockMaxYaw" => p.shock_max_yaw = positive_nonzero(a)?,
        "ShockMaxPitch" => p.shock_max_pitch = positive_nonzero(a)?,
        "ShockMaxRoll" => p.shock_max_roll = positive_nonzero(a)?,
        "ForwardFriction" => p.forward_friction = friction_per_sec_to_per_frame(a.real()?),
        "LateralFriction" => p.lateral_friction = friction_per_sec_to_per_frame(a.real()?),
        "ZFriction" => p.z_friction = friction_per_sec_to_per_frame(a.real()?),
        "AerodynamicFriction" => p.aerodynamic_friction = friction_per_sec_to_per_frame(a.real()?),
        "CenterOfMassOffset" => p.center_of_mass_offset = a.real()?,
        "AllowBouncing" => p.allow_bouncing = a.boolean()?,
        "AllowCollideForce" => p.allow_collide_force = a.boolean()?,
        "KillWhenRestingOnGround" => p.kill_when_resting_on_ground = a.boolean()?,
        "MinFallHeightForDamage" => p.min_fall_speed_for_damage = height_to_speed(gravity, a.real()?),
        "FallHeightDamageFactor" => p.fall_height_damage_factor = a.real()?,
        "PitchRollYawFactor" => p.pitch_roll_yaw_factor = a.real()?,
        "VehicleCrashesIntoBuildingWeaponTemplate" => p.vehicle_crashes_into_building_weapon = weapon_ref(a),
        "VehicleCrashesIntoNonBuildingWeaponTemplate" => p.vehicle_crashes_into_non_building_weapon = weapon_ref(a),
        _ => return Ok(false),
    }
    Ok(true)
}

pub(crate) fn parse_physics(block: &Block, gravity: f32, d: &mut Diag) -> PhysicsData {
    let mut p = PhysicsData::new(gravity);
    for item in &block.items {
        let f = match item {
            Item::Field(f) => f,
            Item::Block(nb) => {
                d.warn(WarningKind::Structure, &nb.kind, nb.line, "unexpected nested block in PhysicsBehavior");
                continue;
            }
        };
        let mut a = Args::new(&f.value);
        match physics_field(&mut p, &f.key, &mut a, gravity) {
            Ok(true) => {}
            Ok(false) => d.unknown_field(f),
            Err(e) => d.bad_value(f, e),
        }
    }
    p
}

// ---------------------------------------------------------------------------------------------

/// Builds the data for one module block (`ModuleFactory::newModuleDataFromINI`).
pub(crate) fn parse_module_data(class: &str, tag: &str, block: &Block, gravity: f32, d: &mut Diag) -> ModuleData {
    d.push_scope(format!("{} {} {}", block.kind, class, tag));
    let data = if let Some(bc) = BodyClass::from_class_name(class) {
        ModuleData::Body(parse_body(bc, block, d))
    } else if AI_MODULES.binary_search(&class).is_ok() {
        ModuleData::Ai(parse_ai(class, block, d))
    } else if class == "PhysicsBehavior" {
        ModuleData::Physics(parse_physics(block, gravity, d))
    } else if is_typed_die_class(class) {
        ModuleData::Die(DieData::parse(class, block, d))
    } else if is_typed_upgrade_class(class) {
        ModuleData::Upgrade(UpgradeData::parse(class, block, d))
    } else if class == "ProductionUpdate" {
        ModuleData::Production(ProductionData::parse(block, d))
    } else if is_typed_production_exit_class(class) {
        ModuleData::ProductionExit(ProductionExitData::parse(class, block, d))
    } else if is_typed_dock_class(class) {
        ModuleData::Dock(DockData::parse(class, block, d))
    } else if class == "DumbProjectileBehavior" {
        ModuleData::DumbProjectile(Box::new(DumbProjectileData::parse(block, d)))
    } else if ContainKind::from_class_name(class).is_some() {
        ModuleData::Contain(Box::new(ContainData::parse(class, block, d)))
    } else if crate::special_power::is_typed_special_power_class(class) {
        ModuleData::SpecialPower(Box::new(crate::special_power::SpecialPowerModuleData::parse(class, block, d)))
    } else if class == "SpecialAbility" || class == "SpecialAbilityUpdate" {
        ModuleData::SpecialAbility(Box::new(crate::special_power::SpecialAbilityData::parse(class, block, d)))
    } else if class == "AutoHealBehavior" {
        ModuleData::AutoHeal(Box::new(crate::behavior::AutoHealData::parse(block, d)))
    } else if class == "FlammableUpdate" {
        ModuleData::Flammable(crate::behavior::FlammableData::parse(block, d))
    } else if class == "PoisonedBehavior" {
        ModuleData::Poisoned(crate::behavior::PoisonedData::parse(block, d))
    } else if class == "FireWeaponWhenDeadBehavior" {
        ModuleData::FireWeaponWhenDead(crate::behavior::FireWeaponWhenDeadData::parse(block, d))
    } else if class == "LifetimeUpdate" {
        ModuleData::Lifetime(crate::behavior::LifetimeData::parse(block, d))
    } else if class == "StealthUpdate" {
        ModuleData::Stealth(Box::new(crate::behavior::StealthData::parse(block, d)))
    } else if class == "StealthDetectorUpdate" {
        ModuleData::StealthDetector(crate::behavior::StealthDetectorData::parse(block, d))
    } else if crate::modules2::is_typed2_class(class) {
        match class {
            "SquishCollide" => ModuleData::SquishCollide(crate::modules2::SquishCollideData::parse(block, d)),
            "CrateCollide" | "MoneyCrateCollide" | "HealCrateCollide" | "VeterancyCrateCollide" | "UnitCrateCollide"
            | "SalvageCrateCollide" | "ShroudCrateCollide" | "ConvertToCarBombCrateCollide"
            | "ConvertToHijackedVehicleCrateCollide" | "SabotageCommandCenterCrateCollide"
            | "SabotageFakeBuildingCrateCollide" | "SabotageInternetCenterCrateCollide"
            | "SabotageMilitaryFactoryCrateCollide" | "SabotagePowerPlantCrateCollide"
            | "SabotageSuperweaponCrateCollide" | "SabotageSupplyCenterCrateCollide"
            | "SabotageSupplyDropzoneCrateCollide" => {
                ModuleData::CrateCollide(Box::new(crate::modules2::CrateCollideData::parse(class, block, d)))
            }
            "FireWeaponCollide" => ModuleData::FireWeaponCollide(crate::modules2::FireWeaponCollideData::parse(block, d)),
            "VeterancyGainCreate" => ModuleData::VeterancyGainCreate(crate::modules2::VeterancyGainCreateData::parse(block, d)),
            "ToppleUpdate" => ModuleData::Topple(crate::modules2::ToppleData::parse(block, d)),
            "SpawnBehavior" => ModuleData::Spawn(Box::new(crate::modules2::SpawnData::parse(block, d))),
            "SlavedUpdate" => ModuleData::Slaved(crate::modules2::SlavedData::parse(block, d)),
            "HordeUpdate" => ModuleData::Horde(crate::modules2::HordeData::parse(block, d)),
            "AutoDepositUpdate" => ModuleData::AutoDeposit(crate::modules2::AutoDepositData::parse(block, d)),
            "BaseRegenerateUpdate" => ModuleData::BaseRegenerate(crate::modules2::BaseRegenerateData::parse(block, d)),
            "PropagandaTowerBehavior" => ModuleData::Propaganda(crate::modules2::PropagandaData::parse(block, d)),
            "FireWeaponUpdate" => ModuleData::FireWeapon(crate::modules2::FireWeaponData::parse(block, d)),
            "RepairDockUpdate" => ModuleData::RepairDock(crate::modules2::RepairDockData::parse(block, d)),
            "OverchargeBehavior" | "PowerPlantUpdate" => ModuleData::Overcharge(crate::modules2::OverchargeData::parse(block, d)),
            "MinefieldBehavior" => ModuleData::Minefield(Box::new(crate::modules2::MinefieldData::parse(block, d))),
            "TransitionDamageFX" => ModuleData::TransitionDamage(crate::modules2::TransitionDamageData::parse(block, d)),
            "ParkingPlaceBehavior" => ModuleData::ParkingPlace(crate::modules2::ParkingPlaceData::parse(block, d)),
            "CountermeasuresBehavior" => ModuleData::Countermeasures(crate::modules2::CountermeasuresData::parse(block, d)),
            "RebuildHoleBehavior" => ModuleData::RebuildHole(crate::modules2::RebuildHoleData::parse(block, d)),
            "RebuildHoleExposeDie" => ModuleData::RebuildHoleExpose(crate::modules2::RebuildHoleExposeData::parse(block, d)),
            "GenerateMinefieldBehavior" => ModuleData::GenerateMinefield(crate::modules2::GenerateMinefieldData::parse(block, d)),
            _ => ModuleData::SimpleUpdate(crate::modules2::SimpleUpdateData::parse(class, block, d)),
        }
    } else if let Some(m) = crate::logic::LogicModule::parse(class, block, gravity, d) {
        ModuleData::Logic(Box::new(m))
    } else if let Some(m) = crate::draw::DrawModule::parse(class, block, d) {
        ModuleData::Draw(Box::new(m))
    } else {
        ModuleData::Unparsed {
            module_type: class.to_string(),
            tag: tag.to_string(),
            fields: RawItem::from_block_items(&block.items),
        }
    };
    d.pop_scope();
    data
}
