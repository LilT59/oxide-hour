//! The sleepy-heap families whose update needs the whole `GameLogic`: `AutoHealBehavior`,
//! `FlammableUpdate` and `PoisonedBehavior` (GameLogic/Object/Behavior and Object/Update in the C++ tree).
//!
//! A family is a heap slot with no boxed `UpdateModule`: `schedule.rs` keeps its object and its
//! [`FamilyKind`], and `GameLogic::run_sleepy_updates_with` calls [`GameLogic::run_family`] for it, the same
//! way the AI slots call `ai_call`. The update returns the sleep its C++ `update()` returns, and the C++
//! `setWakeFrame` calls reach the heap through `UpdateSchedule::awaken`. Each family's state is the
//! `BehaviorState` fields of its object (behaviors.rs), named after the C++ members.

use crate::behaviors::{AutoHealTemplate, BehaviorState};
use crate::body::DamageInfo;
use crate::damage::{DamageType, DeathType, Relationship};
use crate::die::DieTemplate;
use crate::disabled::DisabledType;
use crate::logic::GameLogic;
use crate::module::{UPDATE_SLEEP_FOREVER, UPDATE_SLEEP_NONE};
use crate::object::{Object, ObjectId};
use crate::partition::DistanceMode;
use crate::schedule::UpdateHandle;
use crate::template::ObjectTemplate;

/// Which C++ module a family slot is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FamilyKind {
    /// `AutoHealBehavior`: one slot per module; the value is the module's index in `ObjectTemplate::auto_heal`
    /// (an object may carry two, lane 178).
    AutoHeal(usize),
    Flammable,
    Poisoned,
    /// `SlowDeathBehavior` (and its JetSlowDeath / Helicopter / NeutronMissile subclasses): one slot per die module.
    SlowDeath,
    /// `ToppleUpdate`.
    Topple,
    /// `StealthUpdate`.
    Stealth,
    /// `StealthDetectorUpdate`.
    StealthDetector,
    /// `ProductionUpdate` (production.rs): awake every frame (`update` returns NONE).
    Production,
    /// `ParkingPlaceBehavior` (jet_ai.rs): an airfield's parking/runway/heal module, awake every frame
    /// (`ParkingPlaceBehavior.cpp:57`, `:755`).
    ParkingPlace,
    /// A *second* `ProductionUpdate` module of the same object. The retail data carries one on the China
    /// Gattling Cannon (`ChinaGattlingCannon.ini`, `ModuleTag_07` and `ModuleTag_10`), so the C++ object has
    /// two modules and two sleepy-heap slots. `Object::getProductionUpdateInterface` (`Object.cpp`) returns
    /// the first module, so nothing ever queues into the second one: its queue and door stay empty and its
    /// `update` only runs `updateDoors` on them. The port keeps one production state per object, so this
    /// slot is the second module's heap entry: awake every frame, no state (AN1 s1 f2727, lane 170).
    ProductionIdle,
    /// `PhysicsBehavior` (physics.rs): `PHASE_PHYSICS` (PhysicsUpdate.h:220).
    Physics,
    /// A contain module (`GameLogic/Object/Contain/`): `OpenContain` and every subclass are
    /// `UpdateModule`s (`OpenContain.h:80`), one heap slot per INI module. The value is the C++ class.
    Contain(crate::contain::ContainKind),
    /// `TechBuildingBehavior` (TechBuildingBehavior.cpp): the captured model condition and the pulse FX.
    TechBuilding,
    /// A production exit module (`ProductionExitUpdate/`): the `Default` / `SupplyCenter` / `SpawnPoint` classes sleep
    /// forever; `QueueProductionExitUpdate` ticks its exit delay every frame.
    ProductionExit(crate::production::ProductionExitClass),
    /// `PowerPlantUpdate` (PowerPlantUpdate.cpp): asleep until `extendRods(TRUE)`.
    PowerPlant,
    /// `OverchargeBehavior` (OverchargeBehavior.cpp): asleep until `enable(TRUE)`.
    Overcharge,
    /// `AutoDepositUpdate` (AutoDepositUpdate.cpp): the income timer, awake every frame.
    AutoDeposit,
    /// `BaseRegenerateUpdate` (BaseRenerateUpdate.cpp): heals on its own three-frame timer.
    BaseRegenerate,
    /// A `DockUpdate` module (`DockUpdate/`): awake every frame (`update` returns NONE, DockUpdate.cpp:416).
    Dock(crate::dock::DockClass),
    /// `RadarUpdate` (RadarUpdate.cpp): awake every frame; its update only touches drawable model conditions.
    Radar,
    /// `SpawnBehavior` (SpawnBehavior.cpp): the 15-frame spawn countdown, awake every frame.
    Spawn,
    /// `SlavedUpdate` (SlavedUpdate.cpp): awake every frame; the body runs every `SLAVED_UPDATE_RATE` frames.
    Slaved,
    /// `SpectreGunshipDeploymentUpdate` (SpectreGunshipDeploymentUpdate.cpp): awake until sold / under construction / dead.
    SpectreDeployment,
    /// `BridgeBehavior` (BridgeBehavior.cpp): awake every frame; the death FX/OCL are client-only in the port.
    Bridge,
    /// `DeletionUpdate` (DeletionUpdate.cpp): asleep until the lifetime rolled in its constructor, then it *destroys*
    /// the object (no death pipeline) and sleeps forever.
    Deletion,
    /// `DynamicShroudClearingRangeUpdate` (DynamicShroudClearingRangeUpdate.cpp): awake every frame.
    DynamicShroud,
    /// `AutoFindHealingUpdate` (AutoFindHealingUpdate.cpp): a hurt unit scans for a `KINDOF_HEAL_PAD` object.
    AutoFindHealing,
    /// `CommandButtonHuntUpdate` (CommandButtonHuntUpdate.cpp): the scripted command-button hunt. Asleep until
    /// `setCommandButton` wakes it (`CommandButtonHuntUpdate.cpp:137`).
    CommandButtonHunt,
    /// `SpecialAbilityUpdate` (SpecialAbilityUpdate.cpp): one slot per module; the value is the module's index in
    /// `ObjectTemplate::special_abilities`. Asleep until `initiateIntentToDoSpecialPower` wakes it (:556).
    SpecialAbility(usize),
    /// `ProneUpdate` (ProneUpdate.cpp): the prone countdown after a hit. No constructor wake (:63-66);
    /// `update` returns NONE (:78-88).
    Prone,
    /// `DemoTrapUpdate` (DemoTrapUpdate.cpp): the proximity / manual detonation of a GLA demo trap.
    DemoTrap,
    /// `CleanupHazardUpdate` (CleanupHazardUpdate.cpp): the Humvee's hazard scan and attack.
    CleanupHazard,
    /// `GenerateMinefieldBehavior` (GenerateMinefieldBehavior.cpp:457-496): awake every frame while the field is
    /// upgradable and not yet upgraded, asleep forever otherwise. Registered at the creation frame (its constructor
    /// sets no wake, :107-114).
    GenerateMinefield,
    /// `HordeUpdate` (HordeUpdate.cpp:161-169, :377): the constructor's random first wake, then `UPDATE_SLEEP(UpdateRate)`
    /// for infantry and `UPDATE_SLEEP_NONE` for the rest (horde.rs).
    Horde,
    /// `AssistedTargetingUpdate` (AssistedTargetingUpdate.cpp:66-76): no constructor wake in Zero Hour (the
    /// `RTS_GENERALS || !RETAIL_COMPATIBLE_CRC` block is off), so `registerObject` puts it at the creation frame
    /// (GameLogic.cpp:4041-4046); `update` returns `UPDATE_SLEEP_FOREVER` (:145-148). The assist itself is not ported.
    AssistedTargeting,
}

impl FamilyKind {
    /// The C++ class name, as the `ZR_L99_HEAP` dump prints it.
    pub(crate) fn class_name(self) -> &'static str {
        match self {
            FamilyKind::AutoHeal(_) => "AutoHealBehavior",
            FamilyKind::Flammable => "FlammableUpdate",
            FamilyKind::Poisoned => "PoisonedBehavior",
            FamilyKind::SlowDeath => "SlowDeathBehavior",
            FamilyKind::Topple => "ToppleUpdate",
            FamilyKind::Stealth => "StealthUpdate",
            FamilyKind::StealthDetector => "StealthDetectorUpdate",
            FamilyKind::Production => "ProductionUpdate",
            FamilyKind::ProductionIdle => "ProductionUpdate",
            FamilyKind::ParkingPlace => "ParkingPlaceBehavior",
            FamilyKind::Physics => "PhysicsBehavior",
            FamilyKind::Contain(kind) => kind.class_name(),
            FamilyKind::TechBuilding => "TechBuildingBehavior",
            FamilyKind::ProductionExit(class) => class.class_name(),
            FamilyKind::PowerPlant => "PowerPlantUpdate",
            FamilyKind::Overcharge => "OverchargeBehavior",
            FamilyKind::AutoDeposit => "AutoDepositUpdate",
            FamilyKind::BaseRegenerate => "BaseRegenerateUpdate",
            FamilyKind::Dock(class) => class.class_name(),
            FamilyKind::Radar => "RadarUpdate",
            FamilyKind::Spawn => "SpawnBehavior",
            FamilyKind::Slaved => "SlavedUpdate",
            FamilyKind::SpectreDeployment => "SpectreGunshipDeploymentUpdate",
            FamilyKind::Bridge => "BridgeBehavior",
            FamilyKind::Deletion => "DeletionUpdate",
            FamilyKind::DynamicShroud => "DynamicShroudClearingRangeUpdate",
            FamilyKind::AutoFindHealing => "AutoFindHealingUpdate",
            FamilyKind::CommandButtonHunt => "CommandButtonHuntUpdate",
            FamilyKind::SpecialAbility(_) => "SpecialAbilityUpdate",
            FamilyKind::Prone => "ProneUpdate",
            FamilyKind::DemoTrap => "DemoTrapUpdate",
            FamilyKind::CleanupHazard => "CleanupHazardUpdate",
            FamilyKind::GenerateMinefield => "GenerateMinefieldBehavior",
            FamilyKind::Horde => "HordeUpdate",
            FamilyKind::AssistedTargeting => "AssistedTargetingUpdate",
        }
    }

    /// The [`SleepyModuleKind`] this family is, for its position in the template's INI order. `die` is the die-module
    /// index of a SlowDeath family.
    pub(crate) fn sleepy_kind(self, die: usize) -> crate::template::SleepyModuleKind {
        use crate::template::SleepyModuleKind as K;
        match self {
            FamilyKind::AutoHeal(_) => K::AutoHeal,
            FamilyKind::Flammable => K::Flammable,
            FamilyKind::Poisoned => K::Poisoned,
            FamilyKind::SlowDeath => K::SlowDeath { die },
            FamilyKind::Topple => K::Topple,
            FamilyKind::Stealth => K::Stealth,
            FamilyKind::StealthDetector => K::StealthDetector,
            FamilyKind::Production => K::Production,
            FamilyKind::ProductionIdle => K::Production,
            FamilyKind::ParkingPlace => K::ParkingPlace,
            FamilyKind::Physics => K::Physics,
            FamilyKind::Contain(kind) => kind.sleepy_kind(),
            FamilyKind::TechBuilding => K::TechBuilding,
            FamilyKind::ProductionExit(_) => K::ProductionExit,
            FamilyKind::PowerPlant => K::PowerPlant,
            FamilyKind::Overcharge => K::Overcharge,
            FamilyKind::AutoDeposit => K::AutoDeposit,
            FamilyKind::BaseRegenerate => K::BaseRegenerate,
            // The adapter's INI order keeps `RepairDockUpdate` as its own `SleepyModuleKind` (it is its own
            // `ModuleData` arm) while the two supply docks are `Dock`; a repair dock family must look up that
            // variant, or it is not found in the INI order and is pushed after every listed module (which
            // re-ordered the f1123 heap against C++).
            FamilyKind::Dock(crate::dock::DockClass::Repair) => K::RepairDock,
            FamilyKind::Dock(_) => K::Dock,
            FamilyKind::Radar => K::Radar,
            FamilyKind::Spawn => K::Spawn,
            FamilyKind::Slaved => K::Slaved,
            FamilyKind::SpectreDeployment => K::Spectre,
            FamilyKind::Bridge => K::Bridge,
            FamilyKind::Deletion => K::Deletion,
            FamilyKind::DynamicShroud => K::DynamicShroud,
            FamilyKind::AutoFindHealing => K::AutoFindHealing,
            FamilyKind::CommandButtonHunt => K::CommandButtonHunt,
            FamilyKind::SpecialAbility(_) => K::SpecialAbility,
            FamilyKind::Prone => K::Prone,
            FamilyKind::DemoTrap => K::DemoTrap,
            FamilyKind::CleanupHazard => K::CleanupHazard,
            FamilyKind::GenerateMinefield => K::GenerateMinefield,
            FamilyKind::Horde => K::Horde,
            FamilyKind::AssistedTargeting => K::AssistedTargeting,
        }
    }

    /// `getUpdatePhase()` of the C++ module: PHASE_PHYSICS for `PhysicsBehavior` (PhysicsUpdate.h:220), else
    /// PHASE_NORMAL (no override).
    pub(crate) fn phase(self) -> crate::module::SleepyUpdatePhase {
        match self {
            FamilyKind::Physics => crate::module::SleepyUpdatePhase::Physics,
            _ => crate::module::SleepyUpdatePhase::Normal,
        }
    }

    /// `getDisabledTypesToProcess()` of the C++ module (GameLogic.cpp:3851-3885): AutoHeal, Stealth and
    /// StealthDetector run while HELD (AutoHealBehavior.h:136, StealthUpdate.h, StealthDetectorUpdate.h); Poisoned and
    /// SlowDeath run always (PoisonedBehavior.h:80, SlowDeathBehavior.h); Flammable and ToppleUpdate have no override
    /// and run only while the object has no disabled flags (UpdateModule.h:172-175).
    pub(crate) fn types_to_process(self) -> u16 {
        let held = DisabledType::Held.bit();
        match self {
            FamilyKind::AutoHeal(_) | FamilyKind::Stealth | FamilyKind::StealthDetector => held,
            FamilyKind::Flammable | FamilyKind::Topple => 0,
            FamilyKind::Poisoned | FamilyKind::SlowDeath | FamilyKind::Physics => u16::MAX,
            // Contain modules have no `getDisabledTypesToProcess` override (OpenContain.h, and the
            // subclasses' headers): the base `DISABLEDMASK_NONE` (UpdateModule.h:172-175).
            FamilyKind::Contain(_) => 0,
            // The template's `DisabledTypesToProcess` (default HELD, production.rs), set at registration.
            FamilyKind::Production | FamilyKind::ProductionIdle => held,
            // `BaseRegenerateUpdate` runs while UNDERPOWERED (BaseRegenerateUpdate.h:76).
            FamilyKind::BaseRegenerate => DisabledType::Underpowered.bit(),
            // `SpectreGunshipDeploymentUpdate` runs while SUBDUED, UNDERPOWERED, EMP or HACKED
            // (SpectreGunshipDeploymentUpdate.h:135).
            FamilyKind::SpectreDeployment => {
                DisabledType::Subdued.bit()
                    | DisabledType::Underpowered.bit()
                    | DisabledType::Emp.bit()
                    | DisabledType::Hacked.bit()
            }
            // No override in their headers: the base `DISABLEDMASK_NONE` (UpdateModule.h:172-175).
            FamilyKind::TechBuilding
            | FamilyKind::ProductionExit(_)
            | FamilyKind::ParkingPlace
            | FamilyKind::PowerPlant
            | FamilyKind::Overcharge
            | FamilyKind::AutoDeposit
            | FamilyKind::Dock(_)
            | FamilyKind::Radar
            | FamilyKind::Spawn
            | FamilyKind::Slaved
            | FamilyKind::Bridge
            | FamilyKind::Deletion
            | FamilyKind::DynamicShroud
            | FamilyKind::AutoFindHealing
            | FamilyKind::CommandButtonHunt
            | FamilyKind::SpecialAbility(_)
            | FamilyKind::Prone
            | FamilyKind::DemoTrap
            | FamilyKind::CleanupHazard
            | FamilyKind::GenerateMinefield
            | FamilyKind::Horde
            | FamilyKind::AssistedTargeting => 0,
        }
    }
}

/// `getDisabledTypesToProcess()` of an object helper (object_helpers.rs): the helpers that override it run while any
/// flag is set (`DISABLEDMASK_ALL`: SubdualDamageHelper, StatusDamageHelper, ObjectDefectionHelper, FiringTracker,
/// TempWeaponBonusHelper, cited in their headers); ObjectSMCHelper and ObjectWeaponStatusHelper have none, so the base
/// `DISABLEDMASK_NONE` applies (UpdateModule.h:172-175).
pub(crate) fn helper_types_to_process(kind: crate::object_helpers::HelperKind) -> u16 {
    use crate::object_helpers::HelperKind;
    match kind {
        HelperKind::SmcHelper | HelperKind::WeaponStatus => 0,
        _ => u16::MAX,
    }
}

impl BehaviorState {
    /// Remember the heap slot of one family of this object. `die_index` is the template die module of a
    /// SlowDeath slot (ignored by the others).
    pub(crate) fn set_family_handle(&mut self, kind: FamilyKind, die_index: usize, h: UpdateHandle) {
        match kind {
            FamilyKind::AutoHeal(index) => {
                if let Some(a) = self.auto_heal.get_mut(index) {
                    a.handle = Some(h);
                }
            }
            FamilyKind::Flammable => self.flame_handle = Some(h),
            FamilyKind::Poisoned => self.poison_handle = Some(h),
            FamilyKind::Topple => self.topple_handle = Some(h),
            FamilyKind::SlowDeath => self.slow_death_handles.push((die_index, h)),
            FamilyKind::Stealth => self.stealth_handle = Some(h),
            FamilyKind::StealthDetector => self.detector_handle = Some(h),
            FamilyKind::Physics => self.physics_handle = Some(h),
            FamilyKind::TechBuilding => self.tech_handle = Some(h),
            FamilyKind::BaseRegenerate => self.base_regen_handle = Some(h),
            FamilyKind::PowerPlant => self.power_plant_handle = Some(h),
            FamilyKind::Overcharge => self.overcharge_handle = Some(h),
            // `CommandButtonHuntUpdate::setCommandButton` and `SpecialAbilityUpdate::initiateIntentToDoSpecialPower`
            // wake their own modules from outside their update, so these keep their heap slots. A template may
            // carry two hunt modules (the US Ranger: ModuleTag_02 and ModuleTag_09); the engine finds the module
            // with `Object::findModule` (`Object.cpp:2916-2936`), the *first* match in the behaviour array, so only
            // the first slot must be woken (the other's `m_commandButton` stays null).
            FamilyKind::CommandButtonHunt => {
                if self.hunt_handle.is_none() {
                    self.hunt_handle = Some(h);
                }
            }
            FamilyKind::SpecialAbility(index) => self.special_ability_handles.push((index, h)),
            // The rest are never woken from outside their own update: no handle is kept.
            FamilyKind::Production
            | FamilyKind::ProductionIdle
            | FamilyKind::ParkingPlace
            | FamilyKind::Contain(_)
            | FamilyKind::ProductionExit(_)
            | FamilyKind::AutoDeposit
            | FamilyKind::Dock(_)
            | FamilyKind::Radar
            | FamilyKind::Spawn
            | FamilyKind::Slaved
            | FamilyKind::SpectreDeployment
            | FamilyKind::Bridge
            | FamilyKind::Deletion
            | FamilyKind::DynamicShroud
            | FamilyKind::AutoFindHealing
            | FamilyKind::Prone
            | FamilyKind::DemoTrap
            | FamilyKind::CleanupHazard
            | FamilyKind::GenerateMinefield
            | FamilyKind::Horde
            | FamilyKind::AssistedTargeting => {}
        }
    }

    fn family_handle(&self, kind: FamilyKind) -> Option<UpdateHandle> {
        match kind {
            FamilyKind::AutoHeal(index) => self.auto_heal.get(index).and_then(|a| a.handle),
            FamilyKind::Flammable => self.flame_handle,
            FamilyKind::Poisoned => self.poison_handle,
            FamilyKind::Topple => self.topple_handle,
            FamilyKind::SlowDeath => None,
            FamilyKind::Stealth => self.stealth_handle,
            FamilyKind::StealthDetector => self.detector_handle,
            FamilyKind::Physics => self.physics_handle,
            FamilyKind::TechBuilding => self.tech_handle,
            FamilyKind::BaseRegenerate => self.base_regen_handle,
            FamilyKind::PowerPlant => self.power_plant_handle,
            FamilyKind::Overcharge => self.overcharge_handle,
            FamilyKind::CommandButtonHunt => self.hunt_handle,
            // A `SpecialAbilityUpdate` slot is woken by its module index; `family_handle` cannot name it.
            FamilyKind::SpecialAbility(_) => None,
            FamilyKind::Prone => None,
            FamilyKind::Production
            | FamilyKind::ProductionIdle
            | FamilyKind::ParkingPlace
            | FamilyKind::Contain(_)
            | FamilyKind::ProductionExit(_)
            | FamilyKind::AutoDeposit
            | FamilyKind::Dock(_)
            | FamilyKind::Radar
            | FamilyKind::Spawn
            | FamilyKind::Slaved
            | FamilyKind::SpectreDeployment
            | FamilyKind::Bridge
            | FamilyKind::Deletion
            | FamilyKind::DynamicShroud
            | FamilyKind::AutoFindHealing
            | FamilyKind::DemoTrap
            | FamilyKind::CleanupHazard
            | FamilyKind::GenerateMinefield
            | FamilyKind::Horde
            | FamilyKind::AssistedTargeting => None,
        }
    }
}

/// The C++ `setWakeFrame(sleep)` of a relative sleep: the absolute frame it wakes on.
fn wake_at(now: u32, sleep: u32) -> u32 {
    now.saturating_add(sleep)
}

/// `UpdateModule::frameToSleepTime` (UpdateModule.cpp:39-64) for the earlier of two frames, as an absolute wake
/// frame: a frame that is due now (or past) wakes the next frame.
fn earliest_wake(now: u32, frame1: u32, frame2: u32) -> u32 {
    let m = if frame1 == 0 { frame2 } else { frame1.min(frame2) };
    if m > now {
        m
    } else {
        now.saturating_add(UPDATE_SLEEP_NONE)
    }
}

/// The body is below its maximum health (AutoHealBehavior's `getHealth() < getMaxHealth()` test).
fn is_hurt(o: &Object) -> bool {
    o.body.as_ref().is_some_and(|b| b.health() < b.max_health())
}

/// `DSCRU_STATE` (`DynamicShroudClearingRangeUpdate.h:104-112`), in the engine's order.
pub(crate) const DS_STATE_NOT_STARTED_YET: u8 = 0;
const DS_STATE_GROWING: u8 = 1;
const DS_STATE_SUSTAINING: u8 = 2;
const DS_STATE_SHRINKING: u8 = 3;
const DS_STATE_DONE_FOREVER: u8 = 4;
const DS_STATE_SLEEPING: u8 = 5;

impl GameLogic {
    /// The families a new object registers, in order, with each one's first wake frame (`0` = now). Sets the
    /// initial state the constructors give (`state`). `auto_heal_wakes` holds one wake per AutoHeal module, in the
    /// template's order: `UpdateSleepTime(UPDATE_SLEEP_FOREVER)` when it does not start active, else the phase it drew
    /// in `1..=delay` (`AutoHealBehavior.cpp:98-109`). The draws happened in `create_object_full`'s constructor pass,
    /// in the template's INI module order.
    pub(crate) fn family_registrations(
        &mut self,
        template: &ObjectTemplate,
        frame: u32,
        state: &mut BehaviorState,
        auto_heal_wakes: &[u32],
    ) -> Vec<(FamilyKind, u32, usize)> {
        let mut out = Vec::new();
        // `AutoHealBehavior`'s constructor (AutoHealBehavior.cpp:90-112), once per module: `m_upgradeExecuted` is the
        // StartsActive flag, and the module registers its first wake.
        for (index, t) in template.auto_heal.iter().enumerate() {
            state.auto_heal.push(crate::behaviors::AutoHealState { started: t.starts_active, ..Default::default() });
            out.push((FamilyKind::AutoHeal(index), auto_heal_wakes.get(index).copied().unwrap_or(UPDATE_SLEEP_FOREVER), 0));
        }
        if let Some(t) = &template.flammable {
            // FlammableUpdate's constructor (FlammableUpdate.cpp:76-90): the flame damage limit starts full.
            state.flame_damage_accum = t.flame_damage_limit;
            out.push((FamilyKind::Flammable, UPDATE_SLEEP_FOREVER, 0));
        }
        if template.poisoned.is_some() {
            out.push((FamilyKind::Poisoned, UPDATE_SLEEP_FOREVER, 0));
        }
        // `SlowDeathBehavior`'s constructor sleeps forever (SlowDeathBehavior.cpp:88) for every death module of the
        // template that is a slow death (`JetSlowDeathBehavior` included).
        for (index, dm) in template.die.iter().enumerate() {
            if matches!(dm.kind, DieTemplate::SlowDeath(_) | DieTemplate::JetSlowDeath(_)) {
                out.push((FamilyKind::SlowDeath, UPDATE_SLEEP_FOREVER, index));
            }
        }
        if template.topple.is_some() {
            // ToppleUpdate's constructor (ToppleUpdate.cpp:57-75) sleeps forever while upright.
            out.push((FamilyKind::Topple, UPDATE_SLEEP_FOREVER, 0));
        }
        if let Some(t) = &template.stealth {
            // StealthUpdate's constructor (StealthUpdate.cpp:155-157): forever for a special-power grant, else the next frame.
            let wake = if t.granted_by_special_power { UPDATE_SLEEP_FOREVER } else { frame.saturating_add(UPDATE_SLEEP_NONE) };
            out.push((FamilyKind::Stealth, wake, 0));
        }
        if let Some(c) = &template.contain {
            // Every contain class derives from `OpenContain : UpdateModule` (`OpenContain.h:80`), whose
            // constructors set no wake (OpenContain.cpp:121-153; TransportContain.cpp:135-140,
            // GarrisonContain.cpp:507-541, ...), so `registerObject` puts the slot at the creation frame
            // (GameLogic.cpp:4041-4046). Its `update` returns `UPDATE_SLEEP_NONE` on every path.
            out.push((FamilyKind::Contain(c.kind), 0, 0));
        }
        if template.production.is_some() {
            // `ProductionUpdate` has no constructor wake: it is registered at the creation frame (`registerObject`).
            // The retail data can list the module twice (the China Gattling Cannon, `ChinaGattlingCannon.ini`
            // `ModuleTag_07` + `ModuleTag_10`): every copy is its own `UpdateModule` and its own heap slot
            // (`registerObject` walks `m_behaviors`), and `Object::getProductionUpdateInterface` returns the
            // first, so the extra copies never see a queue.
            let copies = template
                .update_module_order
                .iter()
                .filter(|k| **k == crate::template::SleepyModuleKind::Production)
                .count()
                .max(1);
            out.push((FamilyKind::Production, 0, 0));
            for _ in 1..copies {
                out.push((FamilyKind::ProductionIdle, 0, 0));
            }
        }
        if template.parking_place.is_some() {
            // `ParkingPlaceBehavior`'s constructor sets `setWakeFrame(getObject(), UPDATE_SLEEP_NONE)`
            // (`ParkingPlaceBehavior.cpp:57`), so its first update is the frame after creation.
            out.push((FamilyKind::ParkingPlace, frame.saturating_add(UPDATE_SLEEP_NONE), 0));
        }
        if template.physics.is_some() {
            // `PhysicsBehavior`'s constructor: `setWakeFrame(getObject(), UPDATE_SLEEP_NONE)` (PhysicsUpdate.cpp:231-233).
            out.push((FamilyKind::Physics, frame.saturating_add(UPDATE_SLEEP_NONE), 0));
        }
        if let Some(d) = &template.stealth_detector {
            // StealthDetectorUpdate's constructor (StealthDetectorUpdate.cpp:90): forever when initially disabled, else
            // its random phase, which the creation draws already stored in `detect_next`.
            let wake = if d.initially_disabled { UPDATE_SLEEP_FOREVER } else { state.detect_next };
            out.push((FamilyKind::StealthDetector, wake, 0));
        }
        // `TechBuildingBehavior`'s constructor calls `setWakeFrame(getObject(), UPDATE_SLEEP_NONE)`
        // (TechBuildingBehavior.cpp:76): its first evaluation is the next frame.
        if template.update_module_order.contains(&crate::template::SleepyModuleKind::TechBuilding)
            || template.tech_building.is_some()
        {
            out.push((FamilyKind::TechBuilding, frame.saturating_add(UPDATE_SLEEP_NONE), 0));
        }
        // A production exit module: `DefaultProductionExitUpdate` / `SupplyCenterProductionExitUpdate` /
        // `SpawnPointProductionExitUpdate` sleep forever in their constructors (DefaultProductionExitUpdate.cpp:49,
        // SupplyCenterProductionExitUpdate.h:91, SpawnPointProductionExitUpdate.cpp:55); `QueueProductionExitUpdate`
        // has no constructor wake and ticks every frame (`QueueProductionExitUpdate.cpp:215`). The INI order names the
        // module; a synthetic template's only marker is a non-default exit (the adapter defaults `production.exit` when
        // the template has no exit module, so a `Default` exit needs the order entry).
        let exit = template.production.as_ref().map(|p| p.exit.class());
        if template.update_module_order.contains(&crate::template::SleepyModuleKind::ProductionExit)
            || exit.is_some_and(|e| e != crate::production::ProductionExitClass::Default)
        {
            if let Some(exit) = exit {
                let wake = match exit {
                    crate::production::ProductionExitClass::Queue => 0,
                    _ => UPDATE_SLEEP_FOREVER,
                };
                out.push((FamilyKind::ProductionExit(exit), wake, 0));
            }
        }
        // `PowerPlantUpdate`'s constructor sleeps forever (PowerPlantUpdate.cpp:62) and `OverchargeBehavior`'s
        // (OverchargeBehavior.cpp:83-88) too. They are separate INI modules and separate heap entries; the order
        // names one per module. A template the order does not name gets one slot for each module it carries.
        let mut power_plant = template.update_module_order.iter().filter(|k| **k == crate::template::SleepyModuleKind::PowerPlant).count();
        if power_plant == 0 && template.power_plant.is_some() {
            power_plant = 1;
        }
        for _ in 0..power_plant {
            out.push((FamilyKind::PowerPlant, UPDATE_SLEEP_FOREVER, 0));
        }
        let mut overcharge = template.update_module_order.iter().filter(|k| **k == crate::template::SleepyModuleKind::Overcharge).count();
        if overcharge == 0 && template.overcharge.is_some() {
            overcharge = 1;
        }
        for _ in 0..overcharge {
            out.push((FamilyKind::Overcharge, UPDATE_SLEEP_FOREVER, 0));
        }
        if let Some(t) = &template.auto_deposit {
            // `AutoDepositUpdate`'s constructor sets `m_depositOnFrame = now + DepositTiming` and does not call
            // `setWakeFrame` (`AutoDepositUpdate.cpp:102-107`), so it is registered at the creation frame.
            state.deposit_next = frame.saturating_add(t.deposit_timing);
            out.push((FamilyKind::AutoDeposit, 0, 0));
        }
        if template.base_regenerate {
            // `BaseRegenerateUpdate`'s constructor: forever when the rule is off, else the next frame
            // (BaseRenerateUpdate.cpp:76-84).
            let wake = if self.rules.base_regen_health_percent_per_second == 0.0 {
                UPDATE_SLEEP_FOREVER
            } else {
                frame.saturating_add(UPDATE_SLEEP_NONE)
            };
            out.push((FamilyKind::BaseRegenerate, wake, 0));
        }
        // A `DockUpdate` module: the constructors set no wake, so registration is the creation frame; `update` returns
        // NONE every frame (DockUpdate.cpp:416).
        if let Some(d) = &template.dock {
            let class = match d.kind {
                crate::supply::DockKind::Warehouse => crate::dock::DockClass::SupplyWarehouse,
                crate::supply::DockKind::Center => crate::dock::DockClass::SupplyCenter,
            };
            out.push((FamilyKind::Dock(class), 0, 0));
        } else if template.repair_dock.is_some() {
            out.push((FamilyKind::Dock(crate::dock::DockClass::Repair), 0, 0));
        }
        // `RadarUpdate` has no constructor wake (`RadarUpdate.cpp:55-63`) and returns NONE every frame.
        if template.update_module_order.contains(&crate::template::SleepyModuleKind::Radar) {
            out.push((FamilyKind::Radar, 0, 0));
        }
        if template.spawn.is_some() {
            // `SpawnBehavior`'s constructor sets no wake (`SpawnBehavior.cpp:57-90`) and returns NONE every frame.
            out.push((FamilyKind::Spawn, 0, 0));
        }
        if template.slaved.is_some() {
            // `SlavedUpdate`'s constructor sets no wake (SlavedUpdate.cpp:59-68) and `update` returns NONE on every path
            // (:110-280): awake every frame, its body gated by `m_framesToWait` (`slaved_update`). One heap slot per slaved
            // module, so a stinger soldier's `SlavedUpdate` takes part in the heap's tie order as in C++.
            out.push((FamilyKind::Slaved, 0, 0));
        }
        if template.spectre_deployment.is_some() {
            // `SpectreGunshipDeploymentUpdate` has no constructor wake and returns NONE while operational
            // (SpectreGunshipDeploymentUpdate.cpp:110-114, :261-277).
            out.push((FamilyKind::SpectreDeployment, 0, 0));
        }
        if template.is_bridge {
            // `BridgeBehavior`'s constructor sets no wake (BridgeBehavior.cpp:229-278); `update` returns NONE
            // every frame (:706-841).
            out.push((FamilyKind::Bridge, 0, 0));
        }
        if template.deletion.is_some() {
            // `DeletionUpdate`'s constructor rolls the lifetime and sleeps until it (`DeletionUpdate.cpp:53-61`);
            // `state.deletion_die` is that absolute frame (the draw already happened in `create_object_full`).
            out.push((FamilyKind::Deletion, state.deletion_die, 0));
        }
        if template.dynamic_shroud.is_some() {
            // `DynamicShroudClearingRangeUpdate`'s constructor sets no wake (`DynamicShroudClearingRangeUpdate.cpp:84-128`),
            // so `registerObject` puts it at the creation frame; its `update` returns NONE every frame (:295).
            out.push((FamilyKind::DynamicShroud, 0, 0));
        }
        // `AutoFindHealingUpdate`'s constructor sets no wake (`AutoFindHealingUpdate.cpp:80-83`): registered at the
        // creation frame; its update counts `ScanRate` down itself and returns NONE every frame (:102-147).
        if template.auto_find_healing.is_some() {
            out.push((FamilyKind::AutoFindHealing, 0, 0));
        }
        // `CommandButtonHuntUpdate`'s constructor sleeps forever (`CommandButtonHuntUpdate.cpp:83`); `setCommandButton`
        // (`:137`) is the only wake. One slot per INI module: the US ranger carries two (`AmericaInfantry.ini`
        // ModuleTag_02 and ModuleTag_09).
        let mut hunts = template
            .update_module_order
            .iter()
            .filter(|k| **k == crate::template::SleepyModuleKind::CommandButtonHunt)
            .count();
        if hunts == 0 && template.command_button_hunt.is_some() {
            hunts = 1;
        }
        for _ in 0..hunts {
            out.push((FamilyKind::CommandButtonHunt, UPDATE_SLEEP_FOREVER, 0));
        }
        // `SpecialAbilityUpdate`'s constructor sleeps forever (`SpecialAbilityUpdate.cpp:86`) and
        // `initiateIntentToDoSpecialPower` wakes it (:556). One slot per update module; the `SpecialAbility` power
        // module it pairs with is not an `UpdateModule` (`SpecialAbilityUpdate.h:169`).
        for (index, ability) in template.special_abilities.iter().enumerate() {
            if !ability.power_module {
                out.push((FamilyKind::SpecialAbility(index), UPDATE_SLEEP_FOREVER, 0));
            }
        }
        // `DemoTrapUpdate` / `CleanupHazardUpdate` have no constructor wake (`DemoTrapUpdate.cpp:73-99`,
        // `CleanupHazardUpdate.cpp:80-99`) and return NONE every frame (`:131`, `:126`).
        if template.demo_trap.is_some() {
            out.push((FamilyKind::DemoTrap, 0, 0));
        }
        if template.cleanup_hazard.is_some() {
            out.push((FamilyKind::CleanupHazard, 0, 0));
        }
        // `ProneUpdate`'s constructor sets no wake (ProneUpdate.cpp:63-66): registered at the creation
        // frame; its `update` returns NONE every frame (:78-88).
        if template.prone.is_some() {
            out.push((FamilyKind::Prone, 0, 0));
        }
        // `GenerateMinefieldBehavior` is an `UpdateModule` whatever its INI (its constructor sets no wake,
        // GenerateMinefieldBehavior.cpp:107-114), so `registerObject` puts one heap entry at the creation frame.
        if template.generate_minefield.is_some() {
            out.push((FamilyKind::GenerateMinefield, 0, 0));
        }
        // `HordeUpdate`'s constructor (HordeUpdate.cpp:161-169) sets `setWakeFrame(UPDATE_SLEEP(GameLogicRandomValue(1,
        // UpdateRate)))`: the absolute frame is `state.horde_wake`, drawn in `create_object_full`'s constructor pass.
        if template.horde.is_some() {
            out.push((FamilyKind::Horde, state.horde_wake, 0));
        }
        // `AssistedTargetingUpdate` (AssistedTargetingUpdate.cpp:66-76): no constructor wake in Zero Hour, so the
        // creation frame (GameLogic.cpp:4041-4046); one heap entry per INI module.
        let assisted = template
            .update_module_order
            .iter()
            .filter(|k| **k == crate::template::SleepyModuleKind::AssistedTargeting)
            .count();
        for _ in 0..assisted {
            out.push((FamilyKind::AssistedTargeting, 0, 0));
        }
        out
    }

    /// `StealthUpdate::receiveGrant` (StealthUpdate.cpp:215): an active grant wakes the module next frame.
    pub(crate) fn wake_stealth_update(&mut self, id: ObjectId) {
        let now = self.frame;
        self.wake_family(id, FamilyKind::Stealth, now, now.saturating_add(UPDATE_SLEEP_NONE));
    }

    /// Wake the SlowDeath module of die module `die_index` of `id` to `when` (`SlowDeathBehavior::beginSlowDeath`'s
    /// `setWakeFrame`).
    pub(crate) fn wake_die_module(&mut self, id: ObjectId, die_index: usize, when: u32) {
        let now = self.frame;
        let handle = self
            .objects
            .get(&id)
            .and_then(|o| o.behavior.slow_death_handles.iter().find(|(i, _)| *i == die_index).map(|(_, h)| *h));
        if let Some(h) = handle {
            self.schedule.awaken(h, now, when);
        }
    }

    /// `PhysicsBehavior::setWakeFrame(NONE)` from outside the module's own update (`applyForce`, PhysicsUpdate.cpp:401-402):
    /// the next frame. The running module ignores it (GameLogic.cpp:3094), as C++ does inside its own update.
    pub(crate) fn wake_physics(&mut self, id: ObjectId) {
        let now = self.frame;
        self.wake_family(id, FamilyKind::Physics, now, now.saturating_add(UPDATE_SLEEP_NONE));
    }

    /// Wake the ToppleUpdate of `id` to `when` (`applyTopplingForce`'s `setWakeFrame`).
    pub(crate) fn wake_topple(&mut self, id: ObjectId, when: u32) {
        let now = self.frame;
        self.wake_family(id, FamilyKind::Topple, now, when);
    }

    /// `CommandButtonHuntUpdate::setCommandButton`'s `setWakeFrame(getObject(), UPDATE_SLEEP_NONE)`
    /// (`CommandButtonHuntUpdate.cpp:137`): the module runs on the next frame.
    pub(crate) fn wake_command_button_hunt(&mut self, id: ObjectId) {
        let now = self.frame;
        self.wake_family(id, FamilyKind::CommandButtonHunt, now, now.saturating_add(UPDATE_SLEEP_NONE));
    }

    /// `SpecialAbilityUpdate::initiateIntentToDoSpecialPower`'s `setWakeFrame(getObject(), UPDATE_SLEEP_NONE)`
    /// (`SpecialAbilityUpdate.cpp:556`) for the update module at `index` in the template's `special_abilities`.
    pub(crate) fn wake_special_ability(&mut self, id: ObjectId, index: usize) {
        let now = self.frame;
        let handle = self
            .objects
            .get(&id)
            .and_then(|o| o.behavior.special_ability_handles.iter().find(|(i, _)| *i == index).map(|(_, h)| *h));
        if let Some(h) = handle {
            self.schedule.awaken(h, now, now.saturating_add(UPDATE_SLEEP_NONE));
        }
    }

    /// `TechBuildingBehavior::onCapture` (TechBuildingBehavior.cpp:139-145): wake the module next frame so it
    /// re-evaluates the captured model condition.
    pub(crate) fn tech_building_on_capture(&mut self, id: ObjectId) {
        let now = self.frame;
        self.wake_family(id, FamilyKind::TechBuilding, now, now.saturating_add(UPDATE_SLEEP_NONE));
    }

    /// `TechBuildingBehavior::onDie` (TechBuildingBehavior.cpp:122-132): the captured condition clears and the
    /// building goes back to the neutral player's default team so no player keeps its bonus.
    pub(crate) fn tech_building_on_die(&mut self, id: ObjectId) {
        if !self.objects.get(&id).is_some_and(|o| o.behavior.tech_handle.is_some()) {
            return;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.tech_captured = false;
        }
        if let Some(neutral) = self.neutral_player() {
            self.set_object_team(id, neutral);
        }
    }

    /// The update of a family slot: the sleep its C++ `update()` returns (GameLogic.cpp:3851-3905). A destroyed
    /// object's entry sleeps forever (it is erased at the end of the frame).
    pub(crate) fn run_family(&mut self, kind: FamilyKind, id: ObjectId, now: u32) -> u32 {
        if !self.objects.get(&id).is_some_and(|o| !o.destroyed) {
            return UPDATE_SLEEP_FOREVER;
        }
        match kind {
            FamilyKind::AutoHeal(index) => self.auto_heal_update(id, index, now),
            FamilyKind::Flammable => self.flame_update(id, now),
            FamilyKind::Poisoned => self.poison_update(id, now),
            FamilyKind::SlowDeath => self.slow_death_update(id, now),
            FamilyKind::Topple => self.topple_update(id, now),
            FamilyKind::Stealth => self.stealth_update(id, now),
            FamilyKind::StealthDetector => self.stealth_detector_update(id, now),
            FamilyKind::Production => self.production_update(id),
            // The second `ProductionUpdate` module of a template that lists it twice: its queue is never used,
            // so its `update` has nothing to do and returns `UPDATE_SLEEP_NONE` (`ProductionUpdate.cpp`).
            FamilyKind::ProductionIdle => crate::module::UPDATE_SLEEP_NONE,
            FamilyKind::ParkingPlace => self.parking_place_update(id),
            FamilyKind::Physics => self.physics_update(id, now),
            // A contain module's `update()` (contain.rs's `contain_family_update`), exactly once per
            // C++ wake; every class returns `UPDATE_SLEEP_NONE`.
            FamilyKind::Contain(contain) => self.contain_family_update(id, contain),
            FamilyKind::TechBuilding => self.tech_building_update(id),
            FamilyKind::ProductionExit(class) => self.production_exit_update(id, class),
            FamilyKind::PowerPlant => {
                // `PowerPlantUpdate::update` (PowerPlantUpdate.cpp:108-118): clear `POWER_PLANT_UPGRADING`, set
                // `POWER_PLANT_UPGRADED` and `m_extended = TRUE`, then sleep forever. The model conditions are
                // drawable state the sim does not derive; `m_extended` gates a later `extendRods`, and the drawable
                // half's `m_rodsExtendTime` wait is the wake.
                if let Some(o) = self.objects.get_mut(&id) {
                    o.behavior.power_plant_extended = true;
                }
                UPDATE_SLEEP_FOREVER
            }
            FamilyKind::Overcharge => self.overcharge_update(id, now),
            FamilyKind::AutoDeposit => self.auto_deposit_update(id, now),
            FamilyKind::BaseRegenerate => self.base_regenerate_update(id, now),
            FamilyKind::Dock(_) => self.dock_update(id),
            FamilyKind::Radar => {
                // `RadarUpdate::update` (RadarUpdate.cpp:93-123): once `m_extendDoneFrame` has passed,
                // clear it and mark the extension complete (the `RADAR_EXTENDING` -> `RADAR_UPGRADED`
                // model conditions are derived from this state in `condition.rs`). Both early returns
                // (`m_extendDoneFrame == 0`, `m_extendComplete`) and every other path return NONE.
                let now = self.frame;
                if let Some(o) = self.objects.get_mut(&id) {
                    let done = o.behavior.radar_extend_done_frame;
                    if done != 0 && !o.behavior.radar_extend_complete && now > done {
                        o.behavior.radar_extend_complete = true;
                        o.behavior.radar_extend_done_frame = 0;
                    }
                }
                UPDATE_SLEEP_NONE
            }
            FamilyKind::Spawn => self.spawn_update(id, now),
            FamilyKind::Slaved => self.slaved_update(id, now),
            FamilyKind::SpectreDeployment => self.spectre_deployment_update(id),
            FamilyKind::Bridge => {
                // `BridgeBehavior::update` (BridgeBehavior.cpp:706-841): the death FX and OCL effects of a destroyed
                // bridge. The terrain bridge's damage state is synced from the damage path
                // (`updateBridgeDamageStates`, TerrainLogic.cpp:1827) and the FX are client-only, so the module
                // itself only has to be awake every frame.
                UPDATE_SLEEP_NONE
            }
            FamilyKind::Deletion => {
                // `DeletionUpdate::update` (DeletionUpdate.cpp:100-106): the lifetime has run out, so the object is
                // *destroyed* (not killed: no death pipeline), and the module sleeps forever.
                self.destroy_object(id);
                UPDATE_SLEEP_FOREVER
            }
            FamilyKind::DynamicShroud => {
                // `DynamicShroudClearingRangeUpdate::update` (DynamicShroudClearingRangeUpdate.cpp:200-295): the
                // grow/sustain/shrink ramp that feeds `Object::setShroudClearingRange` on its change interval. It
                // returns `UPDATE_SLEEP_NONE` on every path.
                self.dynamic_shroud_update(id, now);
                UPDATE_SLEEP_NONE
            }
            FamilyKind::AutoFindHealing => self.auto_find_healing_update(id),
            FamilyKind::CommandButtonHunt => self.command_button_hunt_update(id, now),
            FamilyKind::SpecialAbility(index) => self.special_ability_update(id, index),
            FamilyKind::Prone => {
                // `ProneUpdate::update` (ProneUpdate.cpp:78-88): count the prone frames down and drop
                // the effects at zero; NONE always.
                self.update_prone(id, now);
                UPDATE_SLEEP_NONE
            }
            FamilyKind::DemoTrap => {
                // `DemoTrapUpdate::update` (DemoTrapUpdate.cpp:131-...): proximity / manual detonation, NONE always.
                self.demo_trap_update(id);
                UPDATE_SLEEP_NONE
            }
            FamilyKind::CleanupHazard => {
                // `CleanupHazardUpdate::update` (CleanupHazardUpdate.cpp:126-...): the scan and the attack order.
                self.cleanup_hazard_update(id);
                UPDATE_SLEEP_NONE
            }
            // `GenerateMinefieldBehavior::update` (GenerateMinefieldBehavior.cpp:457-496): its own return value.
            FamilyKind::GenerateMinefield => self.generate_minefield_update(id),
            // `HordeUpdate::update` (HordeUpdate.cpp:255-375): horde.rs.
            FamilyKind::Horde => self.horde_update(id, now),
            // `AssistedTargetingUpdate::update` (AssistedTargetingUpdate.cpp:145-148) returns FOREVER. Its assist
            // (`assistAttack`, called from the requesting weapon's assist) is not ported; only the heap entry is.
            FamilyKind::AssistedTargeting => UPDATE_SLEEP_FOREVER,
        }
    }

    /// `DynamicShroudClearingRangeUpdate::update` (DynamicShroudClearingRangeUpdate.cpp:200-295): the
    /// grow/sustain/shrink ramp. The grid decals are a client effect (`createGridDecals`/`animateGridDecals`),
    /// so they are not simulated; the state machine, its timers and `Object::setShroudClearingRange` (:283) are.
    ///
    /// The C++ deadlines are `UnsignedInt` while `m_stateCountDown` is an `Int`: the comparisons promote the
    /// countdown, so a countdown of 0 is caught by the first test and negative values never reach them.
    fn dynamic_shroud_update(&mut self, id: ObjectId, now: u32) {
        let Some(ds) = self.objects.get(&id).and_then(|o| o.template.dynamic_shroud.clone()) else { return };
        let Some(b) = self.objects.get(&id).map(|o| &o.behavior) else { return };
        if b.ds_state == DS_STATE_SLEEPING {
            return;
        }
        let mut state = b.ds_state;
        let mut countdown = b.ds_state_countdown;
        let mut change_countdown = b.ds_change_interval_countdown;
        let mut current = b.ds_current_clearing_range;
        let native = b.ds_native_clearing_range;
        if countdown <= 0 || now > b.ds_done_forever_frame {
            state = DS_STATE_DONE_FOREVER;
        } else if (countdown as u32) <= b.ds_shrink_start_deadline {
            state = DS_STATE_SHRINKING;
        } else if (countdown as u32) <= b.ds_sustain_deadline {
            state = DS_STATE_SUSTAINING;
        } else if (countdown as u32) <= b.ds_grow_start_deadline {
            state = DS_STATE_GROWING;
        }
        match state {
            DS_STATE_NOT_STARTED_YET => {}
            DS_STATE_GROWING => {
                current += native / ds.grow_time.max(1) as f32;
                if current >= native {
                    state = DS_STATE_SUSTAINING;
                }
            }
            DS_STATE_SUSTAINING => {
                current = native;
            }
            DS_STATE_SHRINKING => {
                current -= (native - ds.final_vision) / ds.shrink_time.max(1) as f32;
            }
            DS_STATE_DONE_FOREVER => {
                current = ds.final_vision;
            }
            _ => {}
        }
        // "it is important that this gets called every frame without sleeping" (:287).
        if countdown > 0 {
            countdown -= 1;
        }
        if change_countdown > 0 {
            change_countdown -= 1;
        } else {
            change_countdown = if state == DS_STATE_GROWING { ds.grow_interval } else { ds.change_interval };
            if let Some(o) = self.objects.get_mut(&id) {
                o.set_shroud_clearing_range(current);
            }
            if state == DS_STATE_DONE_FOREVER {
                state = DS_STATE_SLEEPING;
            }
        }
        if let Some(o) = self.objects.get_mut(&id) {
            let b = &mut o.behavior;
            b.ds_state = state;
            b.ds_state_countdown = countdown;
            b.ds_change_interval_countdown = change_countdown;
            b.ds_current_clearing_range = current;
        }
    }

    /// `OverchargeBehavior::update` (OverchargeBehavior.cpp:95-160): while overcharged, drain `MaxHealth *
    /// HealthPercentToDrainPerSecond / LOGICFRAMES_PER_SECOND` as `DAMAGE_PENALTY`, then switch the overcharge off once
    /// the body falls below `NotAllowedWhenHealthBelowPercent * MaxHealth`. The C++ update returns `UPDATE_SLEEP_NONE`
    /// on every path, so the module stays awake after `enable(FALSE)` (the loop overwrites the `setWakeFrame(FOREVER)`
    /// `enable` made inside the update, GameLogic.cpp:3851-3882).
    fn overcharge_update(&mut self, id: ObjectId, _now: u32) -> u32 {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.overcharge) else {
            return UPDATE_SLEEP_FOREVER;
        };
        if !self.objects.get(&id).is_some_and(|o| o.behavior.overcharge && !o.is_dying()) {
            return UPDATE_SLEEP_NONE;
        }
        let Some(max) = self.objects.get(&id).and_then(|o| o.body.as_ref()).map(|b| b.max_health()) else {
            return UPDATE_SLEEP_NONE;
        };
        self.damage_object_with_death(
            id,
            DamageInfo {
                source: id,
                amount: max * t.health_percent_to_drain_per_second / crate::math::LOGICFRAMES_PER_SECOND as f32,
                kind: DamageType::Penalty,
            },
            DeathType::Normal,
        );
        // `if (body->getHealth() < body->getMaxHealth() * m_notAllowedWhenHealthBelowPercent) enable(FALSE)`.
        let Some((max, cur)) = self.objects.get(&id).and_then(|o| o.body.as_ref()).map(|b| (b.max_health(), b.health())) else {
            return UPDATE_SLEEP_NONE;
        };
        if cur < max * t.not_allowed_when_health_below_percent {
            self.set_overcharge(id, false);
        }
        UPDATE_SLEEP_NONE
    }

    /// `PowerPlantUpdate::extendRods` (`PowerPlantUpdate.cpp:75-103`): extending wakes the module `RodsExtendTime`
    /// frames later unless it is already extended; retracting sleeps it forever. Both callers are outside the module's
    /// own update (`PowerPlantUpgrade::upgradeImplementation`, `PowerPlantUpgrade.cpp:114-126`, and
    /// `OverchargeBehavior::enable`, `OverchargeBehavior.cpp:183-231`).
    pub(crate) fn extend_power_plant_rods(&mut self, id: ObjectId, extend: bool) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.power_plant) else { return };
        let now = self.frame;
        if extend {
            let already = self.objects.get(&id).is_some_and(|o| o.behavior.power_plant_extended);
            if already {
                return;
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.power_plant_extended = true;
            }
            self.wake_family(id, FamilyKind::PowerPlant, now, now.saturating_add(t.rods_extend_time));
        } else {
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.power_plant_extended = false;
            }
            self.wake_family(id, FamilyKind::PowerPlant, now, UPDATE_SLEEP_FOREVER);
        }
    }

    /// `TechBuildingBehavior::update` (TechBuildingBehavior.cpp:88-118): `MODELCONDITION_CAPTURED` while the
    /// controlling player is on a playable side, then the pulse FX every `PulseFXRate` frames while captured.
    fn tech_building_update(&mut self, id: ObjectId) -> u32 {
        let Some(o) = self.objects.get(&id) else { return UPDATE_SLEEP_FOREVER };
        let captured = self.neutral_player() != Some(o.player);
        let pulse = o.template.tech_building.clone();
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.tech_captured = captured;
        }
        // `if (d->m_pulseFX != nullptr && d->m_pulseFXRate > 0 && captured)` (`:108-112`): play the FX and sleep
        // `PulseFXRate`; otherwise "sleep forever my dear" (`:114-117`). The FX is a client effect record.
        if let Some(t) = pulse {
            if let (Some(name), true) = (t.pulse_fx, captured && t.pulse_fx_rate > 0) {
                self.record_fx(id, name);
                return t.pulse_fx_rate;
            }
        }
        UPDATE_SLEEP_FOREVER
    }

    /// `ProductionExitUpdate`'s `update`: a `QueueProductionExitUpdate` counts its exit delay down every frame,
    /// the other classes sleep forever (`DefaultProductionExitUpdate.h:94`, `SupplyCenterProductionExitUpdate.h:91`,
    /// `SpawnPointProductionExitUpdate.h:85`).
    fn production_exit_update(&mut self, id: ObjectId, class: crate::production::ProductionExitClass) -> u32 {
        if class == crate::production::ProductionExitClass::Queue {
            self.tick_queue_exit(id);
            UPDATE_SLEEP_NONE
        } else {
            UPDATE_SLEEP_FOREVER
        }
    }

    /// `SpectreGunshipDeploymentUpdate::update` (SpectreGunshipDeploymentUpdate.cpp:261-277): sleep forever once
    /// sold, under construction or effectively dead; awake every frame otherwise.
    fn spectre_deployment_update(&mut self, id: ObjectId) -> u32 {
        let Some(o) = self.objects.get(&id) else { return UPDATE_SLEEP_FOREVER };
        if self.is_effectively_dead(id) || o.is_dying() || o.under_construction
            || o.status_mask(self.frame) & crate::object::status::SOLD != 0
        {
            UPDATE_SLEEP_FOREVER
        } else {
            UPDATE_SLEEP_NONE
        }
    }

    /// The disabled-types gate of `GameLogic::update` for heap slot `h` (GameLogic.cpp:3849-3885, the RETAIL_COMPATIBLE_CRC
    /// branch): true when the update is deferred to the next frame. The slot's mask is the module's
    /// `getDisabledTypesToProcess`; the AI module's is `DISABLED_HELD` (AIUpdate.h:305).
    pub(crate) fn disabled_gate_defers(&self, h: u32) -> bool {
        let s = self.schedule.slot(h);
        self.objects.get(&s.object).is_some_and(|o| !o.update_module_processes(s.types_to_process))
    }

    /// Wake a family of `id` to `when` (`UpdateModule::setWakeFrame` through `friend_awakenUpdateModule`).
    pub(crate) fn wake_family(&mut self, id: ObjectId, kind: FamilyKind, now: u32, when: u32) {
        let handle = self.objects.get(&id).and_then(|o| o.behavior.family_handle(kind));
        if let Some(h) = handle {
            self.schedule.awaken(h, now, when);
        }
    }

    /// `AutoFindHealingUpdate::update` (`AutoFindHealingUpdate.cpp:102-147`): a computer player's hurt, idle unit
    /// scans `ScanRate` frames for the closest `KINDOF_HEAL_PAD` object within `ScanRange` and orders itself healed
    /// there (`aiGetHealed(healUnit, CMD_FROM_AI)` -> `privateGetHealed`, `AIUpdate.cpp:3744-3754`). Returns
    /// `UPDATE_SLEEP_NONE` on every path.
    fn auto_find_healing_update(&mut self, id: ObjectId) -> u32 {
        let Some((player, ai_handle, idle, pos, scan_left)) = self.objects.get(&id).map(|o| {
            (o.player, o.ai_handle, o.ai.machine.current == crate::ai_states::AiStateType::Idle, o.position, o.behavior.auto_find_scan_frames)
        }) else {
            return UPDATE_SLEEP_NONE;
        };
        // "if (obj->getControllingPlayer()->getPlayerType() == PLAYER_HUMAN) return UPDATE_SLEEP_NONE;"
        if !self.is_ai_player(player) {
            return UPDATE_SLEEP_NONE;
        }
        if scan_left > 0 {
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.auto_find_scan_frames = scan_left - 1;
            }
            return UPDATE_SLEEP_NONE;
        }
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.auto_find_healing) else { return UPDATE_SLEEP_NONE };
        if let Some(o) = self.objects.get_mut(&id) {
            o.behavior.auto_find_scan_frames = t.scan_frames;
        }
        if ai_handle.is_none() {
            return UPDATE_SLEEP_NONE;
        }
        // "If we're real healthy, don't bother looking for healing."
        let Some((health, max_health)) = self.objects.get(&id).and_then(|o| o.body.as_ref()).map(|b| (b.health(), b.max_health())) else {
            return UPDATE_SLEEP_NONE;
        };
        if health > max_health * t.never_heal {
            return UPDATE_SLEEP_NONE;
        }
        // "For now, only heal if idle. jba." The `AlwaysHeal` branch after it is unreachable in the engine
        // (`:130-138` returns first).
        if !idle {
            return UPDATE_SLEEP_NONE;
        }
        let mut best: Option<(f32, ObjectId)> = None;
        for other in self.partition.objects_in_range(&pos, t.scan_range, DistanceMode::FromCenter2D) {
            let Some(v) = self.objects.get(&other) else { continue };
            if !v.is_kind_of(crate::template::kind_of::HEAL_PAD) {
                continue;
            }
            let d = pos.dist_2d(&v.position);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, other));
            }
        }
        if let Some((_, heal_unit)) = best {
            // `canGetHealedAt(..., CMD_FROM_AI)` then `aiEnter`.
            if self.can_get_healed_at_from(id, heal_unit, true) {
                self.command_enter_from(player, &[id], heal_unit, crate::ai_states::CommandSource::Ai);
            }
        }
        UPDATE_SLEEP_NONE
    }

    /// `AutoHealBehavior::update` (AutoHealBehavior.cpp:166-278): returns the sleep.
    fn auto_heal_update(&mut self, id: ObjectId, index: usize, now: u32) -> u32 {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.auto_heal.get(index).cloned()) else {
            return UPDATE_SLEEP_FOREVER;
        };
        // "do not heal if our status bit is not on" (the upgrade) or "if we are effectively dead".
        let active = self.objects.get(&id).and_then(|o| o.behavior.auto_heal.get(index)).is_some_and(|a| a.started);
        if !active || self.is_effectively_dead(id) {
            return UPDATE_SLEEP_FOREVER;
        }
        if t.affects_whole_player {
            // Every object of the owning player that is hurt and matches the kind filters (checkForAutoHeal,
            // AutoHealBehavior.cpp:59-86), pulsed in turn.
            let Some(player) = self.objects.get(&id).map(|o| o.player) else { return UPDATE_SLEEP_FOREVER };
            let targets: Vec<ObjectId> = self
                .objects
                .values()
                .filter(|o| !o.destroyed && o.player == player && !self.is_effectively_dead(o.id))
                .filter(|o| !(t.skip_self_for_healing && o.id == id))
                .filter(|o| o.template.kind_of & t.kind_of != 0 && o.template.kind_of & t.forbidden_kind_of == 0)
                .filter(|o| is_hurt(o))
                .map(|o| o.id)
                .collect();
            for target in targets {
                self.auto_heal_pulse(id, index, target, &t, now);
            }
            return t.healing_delay;
        }
        if t.radius == 0.0 {
            // ORIGINAL SYSTEM: heal self while hurt, else sleep until the next damage (AutoHealBehavior.cpp:213-228).
            if self.objects.get(&id).is_some_and(is_hurt) {
                self.auto_heal_pulse(id, index, id, &t, now);
                return t.healing_delay;
            }
            return UPDATE_SLEEP_FOREVER;
        }
        // EXPANDED SYSTEM: heal the allied, hurt, matching objects in the radius (AutoHealBehavior.cpp:230-277).
        let Some((pos, owner)) = self.objects.get(&id).map(|o| (o.position, o.player)) else { return UPDATE_SLEEP_FOREVER };
        let found = self.partition.objects_in_range(&pos, t.radius, DistanceMode::FromCenter2D);
        let targets: Vec<ObjectId> = found
            .into_iter()
            .filter(|&other| !(t.skip_self_for_healing && other == id))
            .filter(|&other| {
                self.objects.get(&other).is_some_and(|o| {
                    !o.destroyed
                        && !self.is_effectively_dead(o.id)
                        && self.relationship(owner, o.player) == Relationship::Allies
                        && o.template.kind_of & t.kind_of != 0
                        && o.template.kind_of & t.forbidden_kind_of == 0
                        && is_hurt(o)
                })
            })
            .collect();
        for target in targets {
            self.auto_heal_pulse(id, index, target, &t, now);
        }
        if t.single_burst {
            UPDATE_SLEEP_FOREVER
        } else {
            t.healing_delay
        }
    }

    /// `AutoHealBehavior::pulseHealObject` (AutoHealBehavior.cpp:283-303): the heal, then the soonest next heal.
    fn auto_heal_pulse(&mut self, healer: ObjectId, index: usize, target: ObjectId, t: &AutoHealTemplate, now: u32) {
        self.heal_object(target, t.healing_amount);
        if let Some(a) = self.objects.get_mut(&healer).and_then(|o| o.behavior.auto_heal.get_mut(index)) {
            a.next = now.saturating_add(t.healing_delay);
        }
    }

    /// `AutoHealBehavior::onDamage` (AutoHealBehavior.cpp:140-161) for every AutoHeal module of `id`: a radius-0 heal
    /// that is active wakes for the start delay after damage, or at once when its heal delay has passed.
    pub(crate) fn auto_heal_on_damage(&mut self, id: ObjectId) {
        let count = self.objects.get(&id).map_or(0, |o| o.template.auto_heal.len());
        for index in 0..count {
            self.auto_heal_damage_module(id, index);
        }
    }

    fn auto_heal_damage_module(&mut self, id: ObjectId, index: usize) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.auto_heal.get(index).cloned()) else { return };
        let Some(a) = self.objects.get(&id).and_then(|o| o.behavior.auto_heal.get(index)) else { return };
        if !a.started || t.radius != 0.0 {
            return;
        }
        let next = a.next;
        let now = self.frame;
        let when = if t.start_healing_delay > 0 {
            Some(wake_at(now, t.start_healing_delay))
        } else if now > next {
            Some(wake_at(now, UPDATE_SLEEP_NONE))
        } else {
            None
        };
        if let Some(when) = when {
            self.wake_family(id, FamilyKind::AutoHeal(index), now, when);
        }
    }

    /// `UpgradeMux::attemptUpgrade` for an AutoHeal (UpgradeModule.cpp:111-150), run by `update_upgrade_modules` on every
    /// grant: the module activates when one of its TriggeredBy upgrades is owned and no ConflictsWith one is, and
    /// `upgradeImplementation` wakes it for the next frame (AutoHealBehavior.h:143-145). Nothing happens while the
    /// object is under construction (Object.cpp:2481-2482). An undone module (`undoUpgrade`) waits for the next event.
    /// Not honoured: RequiresAllTriggers (no template field).
    // Called from `upgrade.rs::update_upgrade_modules`.
    pub(crate) fn auto_heal_upgrade_event(&mut self, id: ObjectId) {
        let count = self.objects.get(&id).map_or(0, |o| o.template.auto_heal.len());
        for index in 0..count {
            self.auto_heal_upgrade_module(id, index);
        }
    }

    /// One AutoHeal module's `UpgradeMux::attemptUpgrade` (see [`GameLogic::auto_heal_upgrade_event`]).
    fn auto_heal_upgrade_module(&mut self, id: ObjectId, index: usize) {
        let now = self.frame;
        let Some(o) = self.objects.get(&id) else { return };
        if o.destroyed || o.under_construction || o.behavior.auto_heal.get(index).is_none_or(|a| a.started) {
            return;
        }
        let Some(t) = o.template.auto_heal.get(index).cloned() else { return };
        // The activation is by TriggeredBy only: StartsActive is the constructor's, not an upgrade's.
        if t.triggered_by.is_empty() || !self.behavior_upgrade_active(id, false, &t.triggered_by, &t.conflicts_with) {
            return;
        }
        if let Some(a) = self.objects.get_mut(&id).and_then(|o| o.behavior.auto_heal.get_mut(index)) {
            a.started = true;
        }
        self.wake_family(id, FamilyKind::AutoHeal(index), now, wake_at(now, UPDATE_SLEEP_NONE));
    }

    /// `FlammableUpdate::onDamage` (FlammableUpdate.cpp:100-131) for flame damage that dealt `dealt`.
    pub(crate) fn on_flame_damage(&mut self, id: ObjectId, dealt: f32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.flammable.clone()) else { return };
        let now = self.frame;
        let ignite = {
            let Some(o) = self.objects.get_mut(&id) else { return };
            // `now - m_flameDamageExpirationDelay > m_lastFlameDamageDealt` in unsigned frames, as the engine computes it.
            if now.wrapping_sub(t.flame_damage_expiration) > o.behavior.flame_last_damage {
                o.behavior.flame_damage_accum = t.flame_damage_limit;
            }
            o.behavior.flame_last_damage = now;
            // `m_flameSource` is the object itself under RETAIL_COMPATIBLE_CRC, so nothing else is stored.
            // The limit only drops while the object is neither burning nor burned.
            if o.behavior.aflame_until != 0 || o.behavior.flame_burned {
                false
            } else {
                o.behavior.flame_damage_accum -= dealt;
                o.behavior.flame_damage_accum <= 0.0
            }
        };
        if ignite {
            self.try_to_ignite(id, now);
        }
    }

    /// `FlammableUpdate::tryToIgnite` (FlammableUpdate.cpp:198-228): the object burns for `AflameDuration`, and the
    /// module wakes at its first timer.
    fn try_to_ignite(&mut self, id: ObjectId, now: u32) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.flammable.clone()) else { return };
        if let Some(o) = self.objects.get_mut(&id) {
            if o.behavior.aflame_until != 0 || o.behavior.flame_burned {
                return;
            }
            o.behavior.aflame_until = now.saturating_add(t.aflame_duration);
            o.behavior.burned_until = if t.burned_delay != 0 { now.saturating_add(t.burned_delay) } else { 0 };
            o.behavior.flame_damage_next = if t.aflame_damage_delay != 0 { now.saturating_add(t.aflame_damage_delay) } else { 0 };
        }
        let sleep = self.flame_sleep(id, now);
        self.wake_family(id, FamilyKind::Flammable, now, wake_at(now, sleep));
    }

    /// `FlammableUpdate::calcSleepTime` (FlammableUpdate.cpp:178-196): the sleep until the soonest of the burn
    /// timers, or forever when not aflame.
    fn flame_sleep(&self, id: ObjectId, now: u32) -> u32 {
        let Some(b) = self.objects.get(&id).map(|o| &o.behavior) else { return UPDATE_SLEEP_FOREVER };
        if b.aflame_until == 0 || b.aflame_until <= now {
            return UPDATE_SLEEP_FOREVER;
        }
        let mut soonest = b.aflame_until;
        if b.burned_until != 0 && b.burned_until < soonest && b.burned_until > now {
            soonest = b.burned_until;
        }
        if b.flame_damage_next != 0 && b.flame_damage_next < soonest && b.flame_damage_next > now {
            soonest = b.flame_damage_next;
        }
        soonest - now
    }

    /// `FlammableUpdate::update` (FlammableUpdate.cpp:133-176): the burn damage tick, the burned status and the end
    /// of the fire. Returns the sleep.
    fn flame_update(&mut self, id: ObjectId, now: u32) -> u32 {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.flammable.clone()) else { return UPDATE_SLEEP_FOREVER };
        let due = self.objects.get(&id).map(|o| o.behavior.flame_damage_next);
        if due.is_some_and(|d| d != 0 && now >= d) {
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.flame_damage_next = now.saturating_add(t.aflame_damage_delay);
            }
            // `doAflameDamage` (FlammableUpdate.cpp:230-243): DAMAGE_FLAME from the object itself (`m_flameSource`).
            self.damage_object_with_death(
                id,
                DamageInfo { source: id, amount: t.aflame_damage_amount, kind: DamageType::Flame },
                DeathType::Burned,
            );
        }
        if let Some(o) = self.objects.get_mut(&id) {
            let b = &mut o.behavior;
            if b.burned_until != 0 && now >= b.burned_until {
                b.flame_burned = true; // OBJECT_STATUS_BURNED and MODELCONDITION_SMOLDERING
            }
            if b.aflame_until != 0 && now >= b.aflame_until {
                b.aflame_until = 0; // the AFLAME status is cleared; FS_BURNED or FS_NORMAL follows the burned flag
            }
        }
        self.flame_sleep(id, now)
    }

    /// `PoisonedBehavior::onDamage` (PoisonedBehavior.cpp:87-93) with `startPoisonedEffects` (:155-184): a poison
    /// hit starts or extends the damage-over-time, and the module wakes at its first timer.
    pub(crate) fn on_poison_damage(&mut self, id: ObjectId, dealt: f32, death: DeathType) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.poisoned.clone()) else { return };
        let now = self.frame;
        {
            let Some(o) = self.objects.get_mut(&id) else { return };
            let b = &mut o.behavior;
            b.poison_amount = dealt;
            b.poison_until = now.saturating_add(t.poison_duration);
            let next = now.saturating_add(t.poison_damage_interval);
            b.poison_next = if b.poison_next != 0 { b.poison_next.min(next) } else { next };
            b.poison_death = Some(death);
        }
        let sleep = self.poison_sleep(id, now);
        self.wake_family(id, FamilyKind::Poisoned, now, wake_at(now, sleep));
    }

    /// `PoisonedBehavior::onHealing` (PoisonedBehavior.cpp:95-101): a heal stops the poison and the module sleeps forever.
    pub(crate) fn on_poison_healing(&mut self, id: ObjectId) {
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&id) {
            let b = &mut o.behavior;
            b.poison_until = 0; // stopPoisonedEffects
            b.poison_next = 0;
            b.poison_amount = 0.0;
        }
        self.wake_family(id, FamilyKind::Poisoned, now, UPDATE_SLEEP_FOREVER);
    }

    /// `PoisonedBehavior::calcSleepTime` (PoisonedBehavior.cpp:144-153): until the next damage tick or the end of
    /// the poison, whichever is first; forever when not poisoned.
    fn poison_sleep(&self, id: ObjectId, now: u32) -> u32 {
        let Some(b) = self.objects.get(&id).map(|o| &o.behavior) else { return UPDATE_SLEEP_FOREVER };
        if b.poison_until == 0 || b.poison_until == now {
            return UPDATE_SLEEP_FOREVER;
        }
        let wake = earliest_wake(now, b.poison_next, b.poison_until);
        wake - now
    }

    /// `PoisonedBehavior::update` (PoisonedBehavior.cpp:104-142): the damage tick, then the end of the poison.
    fn poison_update(&mut self, id: ObjectId, now: u32) -> u32 {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.poisoned.clone()) else { return UPDATE_SLEEP_FOREVER };
        let Some((stop, due, amount, death)) = self
            .objects
            .get(&id)
            .map(|o| (o.behavior.poison_until, o.behavior.poison_next, o.behavior.poison_amount, o.behavior.poison_death))
        else {
            return UPDATE_SLEEP_FOREVER;
        };
        if stop == 0 {
            return UPDATE_SLEEP_FOREVER;
        }
        if due != 0 && now >= due {
            // The tick is `DAMAGE_UNRESISTABLE` from no source (`m_poisonSource` is not kept under RETAIL_COMPATIBLE_CRC),
            // with the death type of the poison hit.
            self.damage_object_with_death(
                id,
                DamageInfo { source: ObjectId::INVALID, amount, kind: DamageType::Unresistable },
                death.unwrap_or(DeathType::Poisoned),
            );
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.poison_next = now.saturating_add(t.poison_damage_interval);
            }
        }
        if now >= stop && !self.is_effectively_dead(id) {
            // `stopPoisonedEffects` (PoisonedBehavior.cpp:186-199).
            if let Some(o) = self.objects.get_mut(&id) {
                o.behavior.poison_until = 0;
                o.behavior.poison_next = 0;
                o.behavior.poison_amount = 0.0;
            }
        }
        self.poison_sleep(id, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behaviors::{FlammableTemplate, PoisonedTemplate, StealthDetectorTemplate, StealthTemplate};
    use crate::die::{DieModuleTemplate, DieMux, SlowDeathTemplate};
    use crate::modules2::ToppleTemplate;
    use crate::body::{ActiveBody, BodyModule};
    use crate::disabled::DisabledType;
    use crate::body::DamageInfo as Hit;
    use crate::math::Coord3D;
    use crate::template::kind_of;
    use crate::terrain::TerrainLogic;
    use std::sync::Arc;

    fn unit(name: &str) -> ObjectTemplate {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 8.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t
    }

    fn auto_heal(starts_active: bool, delay: u32, start: u32) -> AutoHealTemplate {
        AutoHealTemplate {
            healing_amount: 10.0,
            healing_delay: delay,
            start_healing_delay: start,
            radius: 0.0,
            affects_whole_player: false,
            skip_self_for_healing: false,
            single_burst: false,
            kind_of: kind_of::VEHICLE,
            forbidden_kind_of: 0,
            starts_active,
            triggered_by: vec![],
            conflicts_with: vec![],
        }
    }

    fn flammable() -> FlammableTemplate {
        FlammableTemplate {
            burned_delay: 0,
            aflame_duration: 300,
            aflame_damage_delay: 30,
            aflame_damage_amount: 10.0,
            flame_damage_limit: 20.0,
            flame_damage_expiration: 60,
        }
    }

    fn logic() -> GameLogic {
        GameLogic::new(TerrainLogic::flat(100, 100, 0), 1)
    }

    fn place(g: &mut GameLogic, t: ObjectTemplate) -> ObjectId {
        g.create_object(&Arc::new(t), Coord3D::new(200.0, 200.0, 0.0), 0.0, 0)
    }

    /// The heap key of `kind` on `id` (`(wake << 2) | phase`), or `None` when the object has no such family.
    fn key(g: &GameLogic, id: ObjectId, kind: FamilyKind) -> Option<u32> {
        let h = g.objects.get(&id)?.behavior.family_handle(kind)?;
        Some(g.schedule.slot(h.0).next_call_and_phase)
    }

    /// `AutoHealBehavior`'s constructor without StartsActive sleeps forever (AutoHealBehavior.cpp:106-109), and the
    /// module is in the heap at that key. Revert check: without the registration the family is absent.
    #[test]
    fn auto_heal_is_in_the_heap_asleep_without_an_activation() {
        let mut g = logic();
        let mut t = unit("Inert");
        t.auto_heal = vec![auto_heal(false, 30, 0)];
        let id = place(&mut g, t);
        assert_eq!(key(&g, id, FamilyKind::AutoHeal(0)), Some((UPDATE_SLEEP_FOREVER << 2) | 2));
    }

    /// StartsActive: the constructor draws a random phase in `1..=delay` (AutoHealBehavior.cpp:103-104), so the
    /// first wake is that frame. Revert check: a FOREVER registration puts the key at FOREVER.
    #[test]
    fn a_starts_active_auto_heal_wakes_at_a_random_phase_in_its_delay() {
        let mut g = logic();
        let mut t = unit("Active");
        t.auto_heal = vec![auto_heal(true, 30, 0)];
        let id = place(&mut g, t);
        let wake = key(&g, id, FamilyKind::AutoHeal(0)).expect("registered") >> 2;
        assert!((1..=30).contains(&wake), "random phase in 1..=30, got {wake}");
    }

    /// `AutoHealBehavior::onDamage` (AutoHealBehavior.cpp:140-161): with a start delay the damage wakes the module
    /// `StartHealingDelay` frames later. Revert check: without the damage hook the key stays at the constructor's
    /// phase (not 20 for this seed, asserted first).
    #[test]
    fn damage_wakes_a_start_delayed_auto_heal() {
        let mut g = logic();
        let mut t = unit("Delayed");
        t.auto_heal = vec![auto_heal(true, 1000, 20)];
        let id = place(&mut g, t);
        assert_ne!(key(&g, id, FamilyKind::AutoHeal(0)).expect("registered") >> 2, 20, "the constructor's phase is not the start delay");
        g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 50.0, kind: DamageType::Explosion });
        assert_eq!(key(&g, id, FamilyKind::AutoHeal(0)).expect("registered") >> 2, 20);
    }

    fn gated_heal(triggers: &str) -> ObjectTemplate {
        let mut t = unit("Gated");
        let mut a = auto_heal(false, 5, 0);
        a.triggered_by = vec![triggers.to_owned()];
        t.auto_heal = vec![a];
        t
    }

    /// An upgrade-gated AutoHeal activates on its TriggeredBy upgrade's event (`upgradeImplementation` wakes it,
    /// AutoHealBehavior.h:143-145), and then heals. The event is `auto_heal_upgrade_event`, which `update_upgrade_modules`
    /// calls on every grant. Revert check: without the event's wake the healing never starts.
    #[test]
    fn an_upgrade_triggered_auto_heal_heals_once_the_upgrade_lands() {
        let mut g = logic();
        let id = place(&mut g, gated_heal("Upgrade_Test"));
        g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 50.0, kind: DamageType::Explosion });
        let hurt = g.find_object(id).unwrap().health();
        g.run_frames(10);
        assert_eq!(g.find_object(id).unwrap().health(), hurt, "not active before the upgrade");
        g.objects.get_mut(&id).unwrap().upgrades.insert("Upgrade_Test".to_owned());
        g.auto_heal_upgrade_event(id);
        g.run_frames(10);
        assert!(g.find_object(id).unwrap().health() > hurt, "healed after the upgrade");
    }

    /// `Object::updateUpgradeModules` returns while the object is under construction (Object.cpp:2481-2482): the
    /// upgrade event does not activate the module. Revert check: without the construction test it activates.
    #[test]
    fn an_upgrade_under_construction_does_not_activate_auto_heal() {
        let mut g = logic();
        let id = place(&mut g, gated_heal("Upgrade_Test"));
        g.objects.get_mut(&id).unwrap().under_construction = true;
        g.objects.get_mut(&id).unwrap().upgrades.insert("Upgrade_Test".to_owned());
        g.auto_heal_upgrade_event(id);
        assert!(!g.objects[&id].behavior.auto_heal[0].started, "no activation while under construction");
    }

    /// `undoUpgrade` (Object.cpp:2245) clears the executed flag, and the module stays off until the next upgrade event
    /// (`UpgradeModule.cpp:111-150`), not until the next frame. Revert check: a per-frame poll would reactivate it.
    #[test]
    fn an_undone_auto_heal_waits_for_the_next_upgrade_event() {
        let mut g = logic();
        let id = place(&mut g, gated_heal("Upgrade_Test"));
        g.objects.get_mut(&id).unwrap().upgrades.insert("Upgrade_Test".to_owned());
        g.auto_heal_upgrade_event(id);
        assert!(g.objects[&id].behavior.auto_heal[0].started);
        // `AutoHealBehavior::undoUpgrade` (through disabled.rs's pilot-snipe reset).
        g.objects.get_mut(&id).unwrap().behavior.auto_heal[0].started = false;
        g.run_frames(3);
        assert!(!g.objects[&id].behavior.auto_heal[0].started, "not reactivated by a frame");
        g.auto_heal_upgrade_event(id);
        assert!(g.objects[&id].behavior.auto_heal[0].started, "reactivated by the next event");
    }

    /// A module with neither StartsActive nor TriggeredBy never activates (`wouldUpgrade` needs an activation mask,
    /// UpgradeModule.cpp:124-150; FireWeaponWhenDeadBehavior.cpp:83 and CountermeasuresBehavior.cpp:204 test only
    /// `isUpgradeActive`). Revert check: with the old "empty TriggeredBy means active" rule this is true.
    #[test]
    fn a_module_with_no_activation_upgrade_is_never_active() {
        let mut g = logic();
        let id = place(&mut g, unit("Plain"));
        assert!(!g.behavior_upgrade_active(id, false, &[], &[]));
        assert!(g.behavior_upgrade_active(id, true, &[], &[]), "StartsActive is active");
    }

    /// A particle-beam hit counts as flame damage (FlammableUpdate.cpp:102, `DAMAGE_FLAME || DAMAGE_PARTICLE_BEAM`):
    /// it reaches the flame limit and ignites. Revert check: without ParticleBeam in the flame hook the key stays FOREVER.
    #[test]
    fn a_particle_beam_hit_ignites_a_flammable() {
        let mut g = logic();
        let mut t = unit("Beamed");
        t.flammable = Some(flammable());
        let id = place(&mut g, t);
        g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 20.0, kind: DamageType::ParticleBeam });
        assert_eq!(key(&g, id, FamilyKind::Flammable).expect("registered") >> 2, 30, "aflame: the first burn tick");
    }

    /// `ActiveBody::attemptHealing` runs `onHealing`, not `onDamage` (ActiveBody.cpp:800-860): a heal does not arm an
    /// AutoHeal's start delay the way a hit does. Revert check: with the heal routed to `onDamage` the key goes to 20.
    #[test]
    fn a_heal_does_not_wake_an_auto_heal_as_a_hit_does() {
        let mut g = logic();
        let mut t = unit("Delayed");
        t.auto_heal = vec![auto_heal(true, 1000, 20)];
        let id = place(&mut g, t);
        g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 50.0, kind: DamageType::Explosion });
        let h = g.objects[&id].behavior.auto_heal[0].handle.expect("registered");
        g.schedule.set_next_call_frame(h.0, 1000);
        g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 5.0, kind: DamageType::Healing });
        assert_eq!(key(&g, id, FamilyKind::AutoHeal(0)).expect("registered") >> 2, 1000, "the heal does not wake it");
    }

    /// `PoisonedBehavior::onHealing` (PoisonedBehavior.cpp:95-101): a heal stops the poison. Revert check: without the
    /// heal hook the poison stays on.
    #[test]
    fn a_heal_cures_the_poison() {
        let mut g = logic();
        let mut t = unit("Victim");
        t.poisoned = Some(PoisonedTemplate { poison_damage_interval: 30, poison_duration: 300 });
        let id = place(&mut g, t);
        g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 5.0, kind: DamageType::Poison });
        assert!(g.objects[&id].behavior.poison_until != 0, "poisoned");
        g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 2.0, kind: DamageType::Healing });
        assert_eq!(g.objects[&id].behavior.poison_until, 0, "cured");
        assert_eq!(key(&g, id, FamilyKind::Poisoned), Some((UPDATE_SLEEP_FOREVER << 2) | 2));
    }

    /// The disabled-types gate of the sleepy loop (GameLogic.cpp:3851-3885, the RETAIL_COMPATIBLE_CRC branch): AutoHeal's
    /// `getDisabledTypesToProcess` is `DISABLED_HELD` (AutoHealBehavior.h:136), so an EMP-disabled unit does not heal
    /// until the disable ends. Revert check: without the gate the unit heals while disabled.
    #[test]
    fn an_emp_disabled_auto_heal_does_not_heal() {
        let mut g = logic();
        let mut t = unit("Disabled");
        t.auto_heal = vec![auto_heal(true, 5, 0)];
        let id = place(&mut g, t);
        g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 50.0, kind: DamageType::Explosion });
        let hurt = g.find_object(id).unwrap().health();
        g.set_disabled(id, DisabledType::Emp);
        g.run_frames(12);
        assert_eq!(g.find_object(id).unwrap().health(), hurt, "no heal while EMP-disabled");
        g.clear_disabled(id, DisabledType::Emp);
        g.run_frames(12);
        assert!(g.find_object(id).unwrap().health() > hurt, "heals once the disable ends");
    }

    /// `FlammableUpdate` has no `getDisabledTypesToProcess` override, so its base `DISABLEDMASK_NONE` runs it only while
    /// the unit has no disabled flags (GameLogic.cpp:3851-3885). A disabled burning unit takes no burn tick.
    /// Revert check: without the gate the tick at frame 30 hits while disabled.
    #[test]
    fn a_disabled_burning_unit_takes_no_burn_tick() {
        let mut g = logic();
        let mut t = unit("Burner");
        t.flammable = Some(flammable());
        let id = place(&mut g, t);
        g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 20.0, kind: DamageType::Flame });
        g.set_disabled(id, DisabledType::Emp);
        let before = g.find_object(id).unwrap().health();
        g.run_frames(31);
        assert_eq!(g.find_object(id).unwrap().health(), before, "the tick waits for the disable to end");
    }

    /// `FlammableUpdate::tryToIgnite` (FlammableUpdate.cpp:198-228) wakes the module at its first timer: the burn tick
    /// (`AflameDamageDelay`), which is due before the end of the fire. Revert check: without the wake the key stays
    /// at FOREVER.
    #[test]
    fn ignition_wakes_the_flammable_at_its_first_burn_tick() {
        let mut g = logic();
        let mut t = unit("Flamer");
        t.flammable = Some(flammable());
        let id = place(&mut g, t);
        assert_eq!(key(&g, id, FamilyKind::Flammable), Some((UPDATE_SLEEP_FOREVER << 2) | 2));
        g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 20.0, kind: DamageType::Flame });
        assert_eq!(key(&g, id, FamilyKind::Flammable).expect("registered") >> 2, 30);
    }

    /// `PoisonedBehavior::startPoisonedEffects` (PoisonedBehavior.cpp:155-184) wakes the module at the first tick.
    /// Revert check: without the wake the key stays at FOREVER.
    #[test]
    fn a_poison_hit_wakes_the_poisoned_module_at_its_first_tick() {
        let mut g = logic();
        let mut t = unit("Victim");
        t.poisoned = Some(PoisonedTemplate { poison_damage_interval: 30, poison_duration: 300 });
        let id = place(&mut g, t);
        g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 5.0, kind: DamageType::Poison });
        assert_eq!(key(&g, id, FamilyKind::Poisoned).expect("registered") >> 2, 30);
    }

    /// A guard, not a fix check: after the aflame duration the fire ends and the family sleeps forever again
    /// (FlammableUpdate.cpp:133-196: `update` and `calcSleepTime`, which returns FOREVER once not aflame).
    #[test]
    fn the_burn_family_sleeps_forever_after_the_fire() {
        let mut g = logic();
        let mut t = unit("Burner");
        // Small ticks, so the object outlives the fire.
        t.flammable = Some(FlammableTemplate { aflame_damage_amount: 1.0, ..flammable() });
        let id = place(&mut g, t);
        g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 20.0, kind: DamageType::Flame });
        g.run_frames(301);
        assert_eq!(key(&g, id, FamilyKind::Flammable), Some((UPDATE_SLEEP_FOREVER << 2) | 2));
    }

    fn slow_death_die(destruction_delay: u32, sink_delay: u32) -> DieModuleTemplate {
        DieModuleTemplate {
            mux: DieMux { death_types: u32::MAX, veterancy_levels: u32::MAX, ..DieMux::default() },
            kind: DieTemplate::SlowDeath(Arc::new(SlowDeathTemplate {
                probability_modifier: 1,
                modifier_bonus_per_overkill_percent: 0.0,
                sink_rate: 0.0,
                sink_delay,
                sink_delay_variance: 0,
                destruction_delay,
                destruction_delay_variance: 0,
                destruction_altitude: -10.0,
                fx: [Vec::new(), Vec::new(), Vec::new()],
                ocl: [Vec::new(), Vec::new(), Vec::new()],
                weapons: [Vec::new(), Vec::new(), Vec::new()],
                fling_force: 0.0,
                fling_force_variance: 0.0,
                fling_pitch: 0.0,
                fling_pitch_variance: 0.0,
                special: None,
            })),
        }
    }

    /// `SlowDeathBehavior::beginSlowDeath` wakes the module at `whenToWakeTime`, the earliest of its timers
    /// (SlowDeathBehavior.cpp:322-330). Registered forever (the constructor, SlowDeathBehavior.cpp:88).
    /// Revert check: without the wake in `schedule_slow_death` the key stays at FOREVER.
    #[test]
    fn a_slow_death_wakes_at_its_first_timer() {
        let mut g = logic();
        let mut t = unit("Slow");
        t.die = vec![slow_death_die(100, 10)];
        let id = place(&mut g, t);
        let slow_key = |g: &GameLogic| {
            let (_, h) = g.objects[&id].behavior.slow_death_handles[0];
            g.schedule.slot(h.0).next_call_and_phase
        };
        assert_eq!(slow_key(&g), (UPDATE_SLEEP_FOREVER << 2) | 2);
        g.damage_object_with_death(id, Hit { source: ObjectId::INVALID, amount: 9999.0, kind: DamageType::Explosion }, DeathType::Normal);
        assert_eq!(slow_key(&g) >> 2, 10, "the sink timer, the earliest");
    }

    /// `ToppleUpdate::applyTopplingForce` wakes the module next frame (ToppleUpdate.cpp:...`setWakeFrame(NONE)`).
    /// Revert check: without `wake_topple` in `topple` the key stays at FOREVER.
    #[test]
    fn a_topple_wakes_the_module_for_the_next_frame() {
        let mut g = logic();
        let mut t = unit("Tree");
        t.topple = Some(ToppleTemplate { kill_when_start_toppling: false, kill_when_finished_toppling: false, topple_left_or_right_only: false });
        let id = place(&mut g, t);
        assert_eq!(key(&g, id, FamilyKind::Topple), Some((UPDATE_SLEEP_FOREVER << 2) | 2));
        g.topple(id, false);
        assert_eq!(key(&g, id, FamilyKind::Topple).expect("registered") >> 2, 1, "the next frame");
    }

    fn stealth(granted: bool) -> StealthTemplate {
        StealthTemplate {
            stealth_delay: 0,
            move_threshold_speed: 0.0,
            forbidden_conditions: 0,
            required_status: 0,
            forbidden_status: 0,
            innate_stealth: !granted,
            granted_by_special_power: granted,
            disguises_as_team: false,
            reveal_distance_from_target: 0.0,
            order_idle_enemies_to_attack_on_reveal: false,
            use_rider_stealth: false,
        }
    }

    /// `StealthUpdate`'s constructor wakes it next frame, or forever for a special-power grant (StealthUpdate.cpp:155-157),
    /// and `receiveGrant(TRUE)` wakes it next frame (StealthUpdate.cpp:215). Revert check: without `wake_stealth_update`
    /// in `stealth_receive_grant` the granted key stays at FOREVER.
    #[test]
    fn a_stealth_grant_wakes_the_module_for_the_next_frame() {
        let mut g = logic();
        let mut t = unit("Granted");
        t.stealth = Some(stealth(true));
        let granted = place(&mut g, t);
        assert_eq!(key(&g, granted, FamilyKind::Stealth), Some((UPDATE_SLEEP_FOREVER << 2) | 2), "asleep until a grant");
        g.stealth_receive_grant(granted, true, 5);
        assert_eq!(key(&g, granted, FamilyKind::Stealth).expect("registered") >> 2, 1, "woken for the next frame");

        let mut t = unit("Innate");
        t.stealth = Some(stealth(false));
        let innate = place(&mut g, t);
        assert_eq!(key(&g, innate, FamilyKind::Stealth).expect("registered") >> 2, 1, "an innate module starts on the next frame");
    }

    /// `SubdualDamageHelper::update` heals through the body, and the unit leaves `DISABLED_SUBDUED` when the heal takes
    /// it out of subdual (`ActiveBody::onSubdualChange`, ActiveBody.cpp:1285-1321). The sleepy loop calls
    /// `on_subdual_change` after the helper's update. Revert check: without that call the disabled flag stays set.
    #[test]
    fn a_subdual_heal_clears_the_subdued_disable() {
        let mut g = logic();
        let id = place(&mut g, unit("Subdued"));
        let body = ActiveBody::new(100.0, 100.0, 0.7, 0.35).with_subdual(1000.0, 10, 50.0);
        g.objects.get_mut(&id).unwrap().body = Some(Box::new(body) as Box<dyn BodyModule>);
        let hit = DamageInfo { source: ObjectId::INVALID, amount: 100.0, kind: DamageType::SubdualVehicle };
        let was = g.objects[&id].body.as_ref().unwrap().is_subdued();
        g.objects.get_mut(&id).unwrap().body.as_mut().unwrap().attempt_damage(&hit);
        g.after_subdual_hit(id, was);
        assert!(g.objects[&id].is_disabled_by_type(DisabledType::Subdued), "subdued by 100 damage on 100 health");
        g.run_frames(25);
        assert!(!g.objects[&id].is_disabled_by_type(DisabledType::Subdued), "the first heal step takes it out of subdual");
    }

    fn garrison_contain() -> crate::contain::ContainTemplate {
        crate::contain::ContainTemplate {
            kind: crate::contain::ContainKind::Garrison,
            capacity: 5,
            allow_inside_kind_of: None,
            forbid_inside_kind_of: 0,
            allow_allies_inside: true,
            allow_enemies_inside: true,
            allow_neutral_inside: true,
            passengers_allowed_to_fire: true,
            damage_percentage_to_units: 0.0,
            number_of_exit_paths: 1,
            exit_delay: 0,
            door_open_time: 1,
            scatter_nearby_on_exit: true,
            orient_like_container_on_exit: false,
            reset_mood_check_on_exit: false,
            health_regen: 0.0,
            do_i_heal_objects: false,
            heal_frames: 1.0,
            initial_payload: Vec::new(),
            tunnel_frames_for_full_heal: 1.0,
            is_enclosing_container: true,
            fire_points: crate::contain::GarrisonFirePoints::default(),
            passengers_in_turret: false,
            weapon_bonus_passed_to_passengers: false,
            immune_to_clear_building_attacks: false,
        }
    }

    /// Every contain module class derives from `OpenContain : UpdateModule` (`OpenContain.h:80`), so
    /// each gets one sleepy-heap slot, registered at the creation frame (the constructors set no
    /// wake), and its class name is the C++ class. Revert check: without the contain registration the
    /// family list has no `Contain` entry for any class.
    #[test]
    fn every_contain_class_registers_its_own_slot() {
        use crate::contain::ContainKind;
        let kinds = [
            ContainKind::Open,
            ContainKind::Transport,
            ContainKind::Garrison,
            ContainKind::Tunnel,
            ContainKind::Heal,
            ContainKind::Overlord,
            ContainKind::Helix,
            ContainKind::Parachute,
            ContainKind::InternetHack,
            ContainKind::RailedTransport,
            ContainKind::RiderChange,
            ContainKind::MobNexus,
            ContainKind::Cave,
        ];
        let mut g = logic();
        for kind in kinds {
            let mut t = unit("Container");
            let mut c = garrison_contain();
            c.kind = kind;
            t.contain = Some(std::sync::Arc::new(c));
            let mut state = BehaviorState::default();
            let regs = g.family_registrations(&t, 0, &mut state, &[]);
            let mine: Vec<_> = regs.iter().filter(|(k, _, _)| matches!(k, FamilyKind::Contain(_))).collect();
            assert_eq!(mine.len(), 1, "{kind:?} has exactly one contain slot, got {regs:?}");
            assert_eq!(*mine[0], (FamilyKind::Contain(kind), 0, 0), "{kind:?} is registered at the creation frame");
            assert_eq!(FamilyKind::Contain(kind).class_name(), kind.class_name(), "{kind:?} class name");
        }
    }

    /// `GarrisonContain` is a sleepy module (`UpdateModule` through `OpenContain`) whose update returns NONE every frame
    /// (GarrisonContain.cpp:969, OpenContain.cpp:217), and its constructors set no wake, so it is registered at the
    /// creation frame. Revert check: without the Garrison registration the family list has no `GarrisonContain`.
    #[test]
    fn a_garrison_container_registers_as_an_awake_family() {
        let g = logic();
        let mut t = unit("Bunker");
        t.contain = Some(std::sync::Arc::new(garrison_contain()));
        let mut state = BehaviorState::default();
        let mut g = g;
        let regs = g.family_registrations(&t, 0, &mut state, &[]);
        assert!(
            regs.iter().any(|(k, wake, _)| *k == FamilyKind::Contain(crate::contain::ContainKind::Garrison) && *wake == 0),
            "registered at the creation frame"
        );
    }

    /// `PhysicsBehavior::applyForce` from an AI update wakes the sleeping module (PhysicsUpdate.cpp:401-402): the flag a
    /// locomotor kernel sets is turned into the next-frame wake at once, and cleared. Revert check: without the flag
    /// handling the module stays at FOREVER.
    #[test]
    fn a_flagged_physics_module_wakes_for_the_next_frame() {
        let mut g = logic();
        let mut t = unit("Mover");
        t.physics = Some(crate::physics::PhysicsTemplate { mass: 1.0, ..crate::physics::PhysicsTemplate::default() });
        let id = place(&mut g, t);
        g.run_frames(2);
        assert_eq!(g.physics_wake_frame(id), u32::MAX, "at rest, so asleep");
        g.objects.get_mut(&id).unwrap().physics.as_mut().unwrap().wake_requested = true;
        g.wake_flagged_physics(id);
        assert_eq!(g.physics_wake_frame(id), g.frame() + 1, "woken for the next frame");
        assert!(!g.objects[&id].physics.as_ref().unwrap().wake_requested, "the flag is consumed");
    }

    /// `StealthDetectorUpdate`'s constructor draws its random phase in `1..=DetectionRate` and sleeps until it
    /// (StealthDetectorUpdate.cpp:90). Revert check: without the detector registration the family is absent.
    #[test]
    fn a_detector_sleeps_until_its_random_phase() {
        let mut g = logic();
        let mut t = unit("Detector");
        t.stealth_detector = Some(StealthDetectorTemplate {
            detection_rate: 30,
            detection_range: 100.0,
            initially_disabled: false,
            extra_required_kind_of: 0,
            extra_forbidden_kind_of: 0,
            can_detect_while_garrisoned: false,
            can_detect_while_contained: false,
        });
        let id = place(&mut g, t);
        let wake = key(&g, id, FamilyKind::StealthDetector).expect("registered") >> 2;
        assert!((1..=30).contains(&wake), "random phase in 1..=30, got {wake}");
    }

    // ---- wave 15, lane 117: the remaining C++-only heap families -------------------------------------------

    /// The heap key of a family slot even when the family keeps no handle (the constructor wake / return).
    fn family_key(g: &GameLogic, id: ObjectId, kind: FamilyKind) -> Option<u32> {
        g.schedule
            .heap_order()
            .iter()
            .map(|&h| g.schedule.slot(h))
            .find(|s| s.object == id && s.family == Some(kind))
            .map(|s| s.next_call_and_phase)
    }

    /// `TechBuildingBehavior`'s constructor wakes it next frame and its update sets `MODELCONDITION_CAPTURED` while the
    /// controlling player is on a playable side (TechBuildingBehavior.cpp:76, :88-118); a capture wakes it next frame
    /// (:139-145) and its death clears the captured flag and returns the building to the neutral side (:122-132).
    /// Revert check: without the TechBuilding registration the family is absent.
    #[test]
    fn a_tech_building_follows_its_capture_state() {
        let mut g = logic();
        g.set_neutral_player(Some(9));
        let mut t = unit("Derrick");
        t.kind_of = crate::template::kind_of::STRUCTURE;
        t.update_module_order = vec![crate::template::SleepyModuleKind::TechBuilding];
        let id = place(&mut g, t);
        assert_eq!(family_key(&g, id, FamilyKind::TechBuilding), Some((g.frame() + 1) << 2 | 2), "ctor NONE: next frame");
        let captured = crate::condition::bit_for_name("CAPTURED").expect("CAPTURED is a condition");
        g.run_frames(2);
        assert!(g.objects[&id].model_conditions.test(captured), "player 0 is a playable side");
        assert_eq!(family_key(&g, id, FamilyKind::TechBuilding), Some((UPDATE_SLEEP_FOREVER << 2) | 2), "no pulse FX");
        // A capture wakes the module for the next frame (`Object::onCapture` -> `setWakeFrame(NONE)`).
        g.on_object_captured(id, 0, 9);
        assert_eq!(family_key(&g, id, FamilyKind::TechBuilding).expect("registered") >> 2, g.frame() + 1);
        // Losing it back to the neutral side clears the captured condition on the next update.
        g.set_object_team(id, 9);
        g.run_frames(2);
        assert!(!g.objects[&id].model_conditions.test(captured), "the neutral side is not playable");
        // Death returns it to the neutral side (already there) and clears the flag.
        g.objects.get_mut(&id).unwrap().behavior.tech_captured = true;
        g.tech_building_on_die(id);
        assert!(!g.objects[&id].behavior.tech_captured);
    }

    fn production_exit_template(name: &str, exit: crate::production::ProductionExit) -> ObjectTemplate {
        let mut t = unit(name);
        t.kind_of = crate::template::kind_of::STRUCTURE;
        t.update_module_order = vec![crate::template::SleepyModuleKind::ProductionExit];
        t.production = Some(crate::production::ProductionTemplate { exit, ..Default::default() });
        t
    }

    /// `DefaultProductionExitUpdate` / `SupplyCenterProductionExitUpdate` sleep forever from their constructors; a
    /// `QueueProductionExitUpdate` has no constructor wake and counts its exit delay down every frame
    /// (DefaultProductionExitUpdate.cpp:49, SupplyCenterProductionExitUpdate.h:91, QueueProductionExitUpdate.cpp:215).
    /// Revert check: without the ProductionExit registration every family key is absent.
    #[test]
    fn production_exits_sleep_forever_except_the_queue() {
        use crate::production::{ProductionExit, ProductionExitClass};
        let mut g = logic();
        for (name, exit, class) in [
            ("DefaultExit", ProductionExit::Default { unit_create_point: (0.0, 0.0, 0.0), natural_rally_point: (0.0, 0.0, 0.0) }, ProductionExitClass::Default),
            ("CenterExit", ProductionExit::SupplyCenter { unit_create_point: (0.0, 0.0, 0.0), natural_rally_point: (0.0, 0.0, 0.0), grant_temporary_stealth: 0 }, ProductionExitClass::SupplyCenter),
        ] {
            let id = place(&mut g, production_exit_template(name, exit));
            assert_eq!(family_key(&g, id, FamilyKind::ProductionExit(class)), Some((UPDATE_SLEEP_FOREVER << 2) | 2));
        }
        let queue = ProductionExit::Queue {
            unit_create_point: (0.0, 0.0, 0.0),
            natural_rally_point: (0.0, 0.0, 0.0),
            exit_delay_frames: 30,
            initial_burst: 0,
            allow_airborne_creation: false,
        };
        let id = place(&mut g, production_exit_template("QueueExit", queue));
        assert_eq!(family_key(&g, id, FamilyKind::ProductionExit(ProductionExitClass::Queue)).expect("registered") >> 2, 1, "no ctor wake: the creation frame");
        g.objects.get_mut(&id).unwrap().production.as_mut().unwrap().exit_delay_left = 5;
        g.run_frames(2);
        assert_eq!(g.objects[&id].production.as_ref().unwrap().exit_delay_left, 4, "the delay counts down every frame");
    }

    /// `PowerPlantUpdate`'s constructor sleeps forever (PowerPlantUpdate.cpp:62). Revert check: without the PowerPlant
    /// registration the family is absent.
    #[test]
    fn a_power_plant_update_starts_asleep() {
        let mut g = logic();
        let mut t = unit("Plant");
        t.update_module_order = vec![crate::template::SleepyModuleKind::PowerPlant];
        let id = place(&mut g, t);
        assert_eq!(family_key(&g, id, FamilyKind::PowerPlant), Some((UPDATE_SLEEP_FOREVER << 2) | 2));
    }

    /// `AutoDepositUpdate`'s constructor sets `m_depositOnFrame = now + DepositTiming` (AutoDepositUpdate.cpp:104) and
    /// its first update deposits on that frame. Revert check: without the AutoDeposit registration the family is
    /// absent (and the old lazy latch left `deposit_next` at 0 at creation).
    #[test]
    fn auto_deposit_takes_its_first_deposit_from_the_constructor_timer() {
        let mut g = logic();
        g.add_player(0, 0);
        let mut t = unit("Derrick");
        t.kind_of = crate::template::kind_of::STRUCTURE;
        t.auto_deposit = Some(crate::modules2::AutoDepositTemplate {
            deposit_timing: 10,
            deposit_amount: 100,
            initial_capture_bonus: 0,
            upgraded_boost: Vec::new(),
        });
        let id = place(&mut g, t);
        assert_eq!(g.objects[&id].behavior.deposit_next, 10, "the constructor's m_depositOnFrame");
        assert!(family_key(&g, id, FamilyKind::AutoDeposit).is_some());
        g.run_frames(10);
        assert_eq!(g.players[&0].money, 0, "the deposit is not due yet");
        g.run_frames(1);
        assert_eq!(g.players[&0].money, 100, "deposits on its timer");
        // `Player::becomingTeamMember` re-arms the timer when the object joins a team (`Player.cpp:1036`).
        g.objects.get_mut(&id).unwrap().behavior.deposit_next = 9999;
        g.set_object_team(id, 0);
        assert_eq!(g.objects[&id].behavior.deposit_next, g.frame() + 10, "a team join re-arms the deposit timer");
    }

    /// `BaseRegenerateUpdate` wakes after `BaseRegenDelay` on a non-healing hit (BaseRenerateUpdate.cpp:95-106), heals
    /// on its three-frame timer (:113-146) and sleeps forever at full health / when the rule is off (:76-84).
    /// Revert check: without the BaseRegenerate registration the family is absent.
    #[test]
    fn base_regenerate_heals_on_its_timer_and_sleeps_at_full_health() {
        let mut g = logic();
        g.rules.base_regen_health_percent_per_second = 0.1;
        g.rules.base_regen_delay = 7;
        let mut t = unit("Base");
        t.kind_of = crate::template::kind_of::STRUCTURE;
        t.base_regenerate = true;
        t.max_health = 100.0;
        let id = place(&mut g, t);
        // At full health the first update (frame 1) sleeps forever.
        g.run_frames(2);
        assert_eq!(family_key(&g, id, FamilyKind::BaseRegenerate), Some((UPDATE_SLEEP_FOREVER << 2) | 2));
        g.damage_object(id, Hit { source: ObjectId::INVALID, amount: 50.0, kind: DamageType::Explosion });
        assert_eq!(family_key(&g, id, FamilyKind::BaseRegenerate).expect("registered") >> 2, g.frame() + 7, "BaseRegenDelay");
        let hurt = g.find_object(id).unwrap().health();
        g.run_frames(8);
        assert!(g.find_object(id).unwrap().health() > hurt, "regenerating");
    }

    /// A `DockUpdate` module is awake every frame and grants entry to the first approach position a docker has
    /// reached (DockUpdate.cpp:416-455). Revert check: without the Dock registration the family is absent.
    #[test]
    fn a_dock_update_grants_entry_to_a_reached_approach() {
        use crate::supply::{DockKind, DockTemplate};
        let mut g = logic();
        let mut t = unit("Dock");
        t.kind_of = crate::template::kind_of::STRUCTURE;
        t.dock = Some(DockTemplate { kind: DockKind::Warehouse, starting_boxes: 1, delete_when_empty: false, number_approach_positions: 2, allows_passthrough: false });
        let dock = place(&mut g, t);
        let docker = place(&mut g, unit("Truck"));
        assert!(family_key(&g, dock, FamilyKind::Dock(crate::dock::DockClass::SupplyWarehouse)).is_some());
        g.dock_reserve_approach(dock, docker);
        g.dock_on_approach_reached(dock, docker);
        assert_eq!(g.objects[&dock].dock_state.active_docker, ObjectId::INVALID, "not before the update");
        g.run_frames(2);
        assert_eq!(g.objects[&dock].dock_state.active_docker, docker, "the dock update grants entry");
    }

    /// A repair dock's module is the `DockUpdate` family with the `RepairDockUpdate` class name; its healing runs in
    /// the dock action, so the update only has to be awake (DockUpdate.cpp:416).
    #[test]
    fn a_repair_dock_is_the_repair_class() {
        let mut g = logic();
        let mut t = unit("Repair");
        t.kind_of = crate::template::kind_of::STRUCTURE;
        t.repair_dock = Some(crate::modules2::RepairDockTemplate { frames_for_full_heal: 10.0, number_approach_positions: 1, allows_passthrough: false });
        let id = place(&mut g, t);
        assert_eq!(FamilyKind::Dock(crate::dock::DockClass::Repair).class_name(), "RepairDockUpdate");
        assert!(family_key(&g, id, FamilyKind::Dock(crate::dock::DockClass::Repair)).is_some());
    }

    /// The adapter's INI module order names `RepairDockUpdate` with its own `SleepyModuleKind::RepairDock`
    /// (`ModuleData::RepairDock`, `zr-game/src/adapter.rs`), while the two supply docks are `Dock`. A repair dock
    /// family that resolved to `Dock` was not found in that order and was registered after every listed module:
    /// at AN3 f1123 the GLA Arms Dealer's `RepairDockUpdate` (INI `ModuleTag_12`) was pushed after its
    /// `FlammableUpdate` (`ModuleTag_14`), swapping two equal-key tail entries of the sleepy heap against C++.
    /// Revert check: `FamilyKind::Dock(_) => K::Dock` fails the first assertion.
    #[test]
    fn a_repair_dock_resolves_to_its_own_ini_order_variant() {
        use crate::dock::DockClass;
        use crate::template::SleepyModuleKind as K;
        assert_eq!(FamilyKind::Dock(DockClass::Repair).sleepy_kind(0), K::RepairDock);
        assert_eq!(FamilyKind::Dock(DockClass::SupplyWarehouse).sleepy_kind(0), K::Dock);
        assert_eq!(FamilyKind::Dock(DockClass::SupplyCenter).sleepy_kind(0), K::Dock);
    }

    /// `AssistedTargetingUpdate` (AmericaPatriotBattery) has no constructor wake in Zero Hour
    /// (AssistedTargetingUpdate.cpp:72-75 is compiled out), so `registerObject` puts its slot at the creation frame
    /// (GameLogic.cpp:4041-4046); the first update returns FOREVER (:145-148), so it sleeps after one pop. Lane 192:
    /// without the slot the sleepy heap is one entry short from f2742 on (AH7 s1), and the same-key tie order differs.
    /// Revert check: without the `AssistedTargeting` registration the family is absent and the first assertion fails.
    #[test]
    fn an_assisted_targeting_module_is_one_heap_slot_that_sleeps_after_its_first_pop() {
        let mut g = logic();
        let mut t = unit("PatriotBattery");
        t.kind_of = crate::template::kind_of::STRUCTURE;
        t.update_module_order = vec![crate::template::SleepyModuleKind::AssistedTargeting];
        let id = place(&mut g, t);
        assert_eq!(family_key(&g, id, FamilyKind::AssistedTargeting).expect("registered") >> 2, 1);
        g.run_frames(2);
        assert_eq!(family_key(&g, id, FamilyKind::AssistedTargeting).expect("registered") >> 2, UPDATE_SLEEP_FOREVER);
        assert_eq!(FamilyKind::AssistedTargeting.class_name(), "AssistedTargetingUpdate");
    }

    /// `RadarUpdate` has no constructor wake and returns NONE every frame (RadarUpdate.cpp:55-63, :93-123).
    /// Revert check: without the Radar registration the family is absent.
    #[test]
    fn a_radar_update_is_awake_every_frame() {
        let mut g = logic();
        let mut t = unit("CommandCenter");
        t.update_module_order = vec![crate::template::SleepyModuleKind::Radar];
        let id = place(&mut g, t);
        assert_eq!(family_key(&g, id, FamilyKind::Radar).expect("registered") >> 2, 1);
        g.run_frames(3);
        assert_eq!(family_key(&g, id, FamilyKind::Radar).expect("registered") >> 2, g.frame(), "still due now");
    }

    /// `SpawnBehavior` is awake every frame and makes its first batch on its first update (SpawnBehavior.cpp:192-265).
    /// Revert check: without the Spawn registration the family is absent and nothing is spawned.
    #[test]
    fn spawn_behavior_makes_its_first_batch() {
        use crate::template::SimpleTemplateStore;
        let mut g = logic();
        let mut store = SimpleTemplateStore::default();
        let spawned = store.add(unit("Spawned"));
        g.set_template_source(Arc::new(store));
        let mut t = unit("Spawner");
        t.kind_of = crate::template::kind_of::STRUCTURE;
        t.spawn = Some(crate::modules2::SpawnTemplate {
            spawn_number: 2,
            spawn_replace_delay: 9999,
            one_shot: true,
            spawn_template_names: vec![spawned.name.clone()],
            spawned_require_spawner: false,
            initial_burst: 0,
            aggregate_health: false,
        });
        let id = place(&mut g, t);
        assert!(family_key(&g, id, FamilyKind::Spawn).is_some());
        // The first update makes the two entries (0 and 1, SpawnBehavior.cpp:220-223). Entry 0 is past on the first
        // update; entry 1 needs a later frame and the next check is one SPAWN_UPDATE_RATE on (:230-235).
        g.run_frames(3);
        assert_eq!(g.objects.values().filter(|o| o.template.name == "Spawned").count(), 1, "the first entry at once");
        g.run_frames(15);
        assert_eq!(g.objects.values().filter(|o| o.template.name == "Spawned").count(), 2, "the second entry at the next check");
    }

    /// `SpectreGunshipDeploymentUpdate` sleeps forever once sold (SpectreGunshipDeploymentUpdate.cpp:261-277).
    /// Revert check: without the registration the family is absent.
    #[test]
    fn a_spectre_deployment_sleeps_forever_when_sold() {
        let mut g = logic();
        let mut t = unit("CommandCenter");
        t.update_module_order = vec![crate::template::SleepyModuleKind::Spectre];
        t.spectre_deployment = Some(crate::superweapon::SpectreDeploymentTemplate {
            gunship: None,
            attack_area_radius: 200.0,
            create_location: 3,
            gunship_orbit_radius: 0.0,
            special_power: None,
        });
        let id = place(&mut g, t);
        assert_eq!(family_key(&g, id, FamilyKind::SpectreDeployment).expect("registered") >> 2, 1);
        g.run_frames(1);
        assert_eq!(family_key(&g, id, FamilyKind::SpectreDeployment).expect("registered") >> 2, g.frame(), "awake while operational");
        g.objects.get_mut(&id).unwrap().status_bits |= crate::object::status::SOLD;
        g.run_frames(1);
        assert_eq!(family_key(&g, id, FamilyKind::SpectreDeployment), Some((UPDATE_SLEEP_FOREVER << 2) | 2));
    }

    fn heal_contain() -> crate::contain::ContainTemplate {
        crate::contain::ContainTemplate {
            kind: crate::contain::ContainKind::Heal,
            capacity: 2,
            allow_inside_kind_of: None,
            forbid_inside_kind_of: 0,
            allow_allies_inside: true,
            allow_enemies_inside: true,
            allow_neutral_inside: true,
            passengers_allowed_to_fire: false,
            damage_percentage_to_units: 0.0,
            number_of_exit_paths: 1,
            exit_delay: 0,
            door_open_time: 1,
            scatter_nearby_on_exit: true,
            orient_like_container_on_exit: false,
            reset_mood_check_on_exit: false,
            health_regen: 0.0,
            do_i_heal_objects: false,
            heal_frames: 10.0,
            initial_payload: Vec::new(),
            tunnel_frames_for_full_heal: 10.0,
            is_enclosing_container: false,
            fire_points: crate::contain::GarrisonFirePoints::default(),
            passengers_in_turret: false,
            weapon_bonus_passed_to_passengers: false,
            immune_to_clear_building_attacks: false,
        }
    }

    /// `HealContain::update` (HealContain.cpp:92-180): each frame heals `maxHealth / TimeForFullHeal`, and once the
    /// unit has been inside for that many frames it is healed to full and leaves. Revert check: without the HealContain
    /// registration the family is absent (and nothing heals).
    #[test]
    fn a_heal_contain_heals_its_passengers_and_ejects_them() {
        let mut g = logic();
        let mut t = unit("Barracks");
        t.kind_of = crate::template::kind_of::STRUCTURE;
        t.update_module_order = vec![crate::template::SleepyModuleKind::HealContain];
        t.contain = Some(Arc::new(heal_contain()));
        let barracks = place(&mut g, t);
        let patient = place(&mut g, unit("Ranger"));
        g.force_enter_contain(barracks, patient);
        g.damage_object(patient, Hit { source: ObjectId::INVALID, amount: 50.0, kind: DamageType::Explosion });
        let hurt = g.find_object(patient).unwrap().health();
        assert!(family_key(&g, barracks, FamilyKind::Contain(crate::contain::ContainKind::Heal)).is_some());
        g.run_frames(2);
        assert!(g.find_object(patient).unwrap().health() > hurt, "healed while inside");
        g.run_frames(9);
        assert_eq!(g.find_object(patient).unwrap().health(), 100.0, "full health at TimeForFullHeal");
        assert_eq!(g.find_object(patient).unwrap().contained_by, None, "and ejected");
    }

    /// `BridgeBehavior`'s constructor sets no wake and its update returns NONE every frame (BridgeBehavior.cpp:229-278,
    /// :706-841). Revert check: without the registration the family is absent.
    #[test]
    fn a_bridge_behavior_is_registered_awake() {
        let mut g = logic();
        let mut t = unit("Bridge");
        t.kind_of = crate::template::kind_of::STRUCTURE;
        t.is_bridge = true;
        let id = place(&mut g, t);
        assert_eq!(family_key(&g, id, FamilyKind::Bridge).expect("registered") >> 2, 1);
        g.run_frames(2);
        assert_eq!(family_key(&g, id, FamilyKind::Bridge).expect("registered") >> 2, g.frame());
    }

    /// One half of a `SpecialAbility`/`SpecialAbilityUpdate` pair (`SpecialAbilityUpdate.h:169`).
    fn ability(power: &str, power_module: bool) -> Arc<crate::special_power::SpecialAbilityTemplate> {
        Arc::new(crate::special_power::SpecialAbilityTemplate {
            power: power.to_string(),
            start_ability_range: 500.0,
            ability_abort_range: 10_000_000.0,
            approach_requires_los: false,
            preparation_frames: 0,
            pack_time: 0,
            unpack_time: 0,
            pack_unpack_variation: 0.0,
            skip_packing_with_no_target: false,
            award_xp_for_triggering: 0,
            skill_points_for_triggering: -1,
            effect_duration: 100,
            effect_value: 0,
            special_object: None,
            max_special_objects: 1,
            need_to_face: false,
            flee_range_after_completion: 0.0,
            starts_paused: false,
            power_module,
        })
    }

    fn slot_key(g: &GameLogic, h: crate::schedule::UpdateHandle) -> u32 {
        g.schedule.slot(h.0).next_call_and_phase
    }

    /// `CommandButtonHuntUpdate`'s constructor sleeps forever (`CommandButtonHuntUpdate.cpp:79-85`) and
    /// `SpecialAbilityUpdate`'s too (`SpecialAbilityUpdate.cpp:70-89`), one slot per module: the GLA rebel carries
    /// one hunt and two ability modules (`GLAInfantry.ini` ModuleTag_04, ModuleTag_18 and ModuleTag_21), the US
    /// ranger two hunts and one ability (`AmericaInfantry.ini`). The power-module half of an ability pair is not an
    /// `UpdateModule` and gets no slot (`SpecialAbilityUpdate.h:169`). Revert check: without the two registrations the
    /// handles are `None` / empty.
    #[test]
    fn the_hunt_and_ability_modules_hold_asleep_slots() {
        use crate::template::SleepyModuleKind as K;
        let mut g = logic();
        let mut t = unit("Rebel");
        t.command_button_hunt = Some(crate::modules4::CommandButtonHuntTemplate { scan_frames: 30, scan_range: 9999.0 });
        t.update_module_order = vec![K::CommandButtonHunt, K::SpecialAbility, K::SpecialAbility];
        t.special_abilities = vec![ability("Capture", true), ability("Trap", true), ability("Capture", false), ability("Trap", false)];
        let id = place(&mut g, t);
        let asleep = (UPDATE_SLEEP_FOREVER << 2) | 2;
        let hunt = g.objects[&id].behavior.hunt_handle.expect("the hunt module has a slot");
        assert_eq!(slot_key(&g, hunt), asleep);
        let abilities = g.objects[&id].behavior.special_ability_handles.clone();
        assert_eq!(abilities.len(), 2, "one slot per SpecialAbilityUpdate module");
        assert_eq!(abilities.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![2, 3]);
        for (_, h) in abilities {
            assert_eq!(slot_key(&g, h), asleep);
        }
    }

    /// Duplicate INI kinds keep their positions: the US ranger's two `CommandButtonHuntUpdate` modules are
    /// ModuleTag_02 and ModuleTag_09, with the AI, physics and production modules between them
    /// (`AmericaInfantry.ini:527-...`). `registerObject` walks the behaviour array in INI order
    /// (GameLogic.cpp:4013-4046), so the pending walk must consume one order entry per module instead of sorting
    /// both to the first entry. Revert check: the old first-position sort yields Hunt, Hunt, Physics.
    #[test]
    fn duplicate_ini_kinds_keep_their_positions() {
        use crate::template::SleepyModuleKind as K;
        let mut g = logic();
        let mut t = unit("Ranger");
        t.command_button_hunt = Some(crate::modules4::CommandButtonHuntTemplate { scan_frames: 30, scan_range: 9999.0 });
        t.physics = Some(crate::physics::PhysicsTemplate::default());
        t.update_module_order = vec![K::CommandButtonHunt, K::Physics, K::CommandButtonHunt];
        let id = place(&mut g, t);
        let kinds: Vec<_> = g.objects[&id]
            .update_handles
            .iter()
            .filter_map(|h| g.schedule.slot(h.0).family)
            .collect();
        assert_eq!(
            kinds,
            vec![FamilyKind::CommandButtonHunt, FamilyKind::Physics, FamilyKind::CommandButtonHunt],
            "the two hunt modules sit on either side of physics, as in the INI"
        );
    }

    /// `ScriptActions.cpp:2197-2203` finds the hunt module with
    /// `obj->findUpdateModule(NAMEKEY("CommandButtonHuntUpdate"))`, and `Object::findModule`
    /// (`Object.cpp:2916-2936`) returns the **first** match in the behaviour array. A template with two
    /// hunt modules (the US Ranger) therefore wakes only the first slot; the second's `m_commandButton`
    /// stays null and its slot sleeps forever.
    /// Revert check: with `hunt_handle = Some(h)` on every registration the handle is the second
    /// (post-physics) slot and the first stays at `UPDATE_SLEEP_FOREVER`.
    #[test]
    fn the_hunt_wake_uses_the_first_ini_slot() {
        use crate::template::SleepyModuleKind as K;
        let mut g = logic();
        let mut t = unit("Ranger");
        t.command_button_hunt = Some(crate::modules4::CommandButtonHuntTemplate { scan_frames: 30, scan_range: 9999.0 });
        t.physics = Some(crate::physics::PhysicsTemplate::default());
        t.update_module_order = vec![K::CommandButtonHunt, K::Physics, K::CommandButtonHunt];
        let id = place(&mut g, t);
        let hunts: Vec<_> = g.objects[&id]
            .update_handles
            .iter()
            .copied()
            .filter(|h| g.schedule.slot(h.0).family == Some(FamilyKind::CommandButtonHunt))
            .collect();
        assert_eq!(hunts.len(), 2, "two hunt modules");
        assert_eq!(g.objects[&id].behavior.hunt_handle, Some(hunts[0]), "the handle is the first INI slot");
        assert!(g.set_hunt_command(id, crate::modules4::HuntCommand::Weapon));
        assert_eq!(slot_key(&g, hunts[0]) >> 2, g.frame() + 1, "the first slot woke");
        assert!(slot_key(&g, hunts[1]) >> 2 > g.frame() + 1, "the second stays asleep");
    }

    /// `CommandButtonHuntUpdate::setCommandButton`'s `setWakeFrame(getObject(), UPDATE_SLEEP_NONE)`
    /// (`CommandButtonHuntUpdate.cpp:137`) wakes the module for the next frame. Revert check: without
    /// `wake_command_button_hunt` the slot stays at `UPDATE_SLEEP_FOREVER`.
    #[test]
    fn a_hunt_command_wakes_the_hunt_module() {
        let mut g = logic();
        let mut t = unit("Rebel");
        t.command_button_hunt = Some(crate::modules4::CommandButtonHuntTemplate { scan_frames: 30, scan_range: 9999.0 });
        let id = place(&mut g, t);
        let hunt = g.objects[&id].behavior.hunt_handle.expect("slot");
        assert!(g.set_hunt_command(id, crate::modules4::HuntCommand::Weapon));
        assert_eq!(slot_key(&g, hunt) >> 2, g.frame() + 1, "woken for the next frame");
    }

    /// `SpecialAbilityUpdate::initiateIntentToDoSpecialPower` wakes only the update module of the power being started
    /// (`SpecialAbilityUpdate.cpp:471-559`, the `setWakeFrame` at :556); the other ability's module stays asleep.
    /// Revert check: without the wake both slots stay at `UPDATE_SLEEP_FOREVER`.
    #[test]
    fn starting_an_ability_wakes_only_its_module() {
        let mut g = logic();
        let mut t = unit("Rebel");
        t.special_abilities = vec![ability("Capture", true), ability("Trap", true), ability("Capture", false), ability("Trap", false)];
        let id = place(&mut g, t);
        let victim = place(&mut g, unit("Enemy"));
        let handles = g.objects[&id].behavior.special_ability_handles.clone();
        let capture = handles.iter().find(|(i, _)| *i == 2).map(|(_, h)| *h).expect("capture slot");
        let trap = handles.iter().find(|(i, _)| *i == 3).map(|(_, h)| *h).expect("trap slot");
        assert!(g.command_do_special_power(0, id, "Capture", crate::special_power::SpecialPowerTarget::Object(victim)));
        assert_eq!(slot_key(&g, capture) >> 2, g.frame() + 1, "the started ability's module wakes");
        assert_eq!(slot_key(&g, trap), (UPDATE_SLEEP_FOREVER << 2) | 2, "the other one stays asleep");
    }

    /// `AutoFindHealingUpdate` has no constructor wake (`AutoFindHealingUpdate.cpp:80-83`), so `registerObject` puts
    /// it on frame 1 (`GameLogic.cpp:4039-4046`) and its update returns NONE every frame. Revert check: without the
    /// registration no slot exists.
    #[test]
    fn auto_find_healing_is_registered_awake() {
        let mut g = logic();
        // The engine's update only runs for a computer player (`getPlayerType() == PLAYER_HUMAN` returns early,
        // AutoFindHealingUpdate.cpp:106).
        g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
        let mut t = unit("Ranger");
        t.kind_of = crate::template::kind_of::INFANTRY;
        t.auto_find_healing = Some(crate::template::AutoFindHealingTemplate {
            scan_frames: 30,
            scan_range: 200.0,
            never_heal: 0.85,
            always_heal: 0.25,
        });
        let id = place(&mut g, t);
        let key = g
            .schedule
            .heap_order()
            .iter()
            .map(|&h| g.schedule.slot(h))
            .find(|s| s.object == id && s.family == Some(FamilyKind::AutoFindHealing))
            .map(|s| s.next_call_and_phase);
        assert_eq!(key, Some(1 << 2 | 2));
        g.run_frames(2);
        assert_eq!(g.objects[&id].behavior.auto_find_scan_frames, 30, "the scan timer");
    }

    /// `CommandButtonHuntUpdate::huntEnter` returns `UPDATE_SLEEP(ScanRate)` (`CommandButtonHuntUpdate.cpp:240`) and
    /// `huntWeapon` returns NONE (`:190`): the slot's next wake is the return value. Revert check: a slot that always
    /// sleeps forever (or always NONE) fails one of the two assertions.
    #[test]
    fn the_hunt_update_sleeps_for_the_scan_rate_or_stays_awake() {
        use crate::enter_actions::EnterKind;
        use crate::modules4::HuntCommand;
        let mut g = logic();
        let mut t = unit("Rebel");
        t.command_button_hunt = Some(crate::modules4::CommandButtonHuntTemplate { scan_frames: 30, scan_range: 9999.0 });
        let id = place(&mut g, t);
        let hunt = g.objects[&id].behavior.hunt_handle.expect("slot");
        assert!(g.set_hunt_command(id, HuntCommand::Enter { kind: EnterKind::HijackVehicle, object_type: String::new() }));
        // `setCommandButton` woke it for frame 1; that update returns ScanRate.
        g.run_frames(2);
        let ran = g.frame() - 1;
        assert_eq!(slot_key(&g, hunt) >> 2, ran + 30, "huntEnter sleeps ScanRate");
        let ran = g.frame();
        assert!(g.set_hunt_command(id, HuntCommand::Weapon));
        g.run_frames(2);
        assert_eq!(slot_key(&g, hunt) >> 2, ran + 2, "huntWeapon returns UPDATE_SLEEP_NONE");
    }

    /// `SpecialAbilityUpdate::update` returns `calcSleepTime()`: NONE while the ability is active
    /// (`SpecialAbilityUpdate.h:252-255`). Revert check: a slot that sleeps forever after the wake never advances the
    /// preparation.
    #[test]
    fn an_active_ability_slot_stays_awake() {
        let mut g = logic();
        let mut t = unit("Rebel");
        t.locomotor = Some(crate::template::LocomotorParams::from_ini(
            crate::template::LocomotorKind::Treads,
            30.0,
            60.0,
            120.0,
            120.0,
        ));
        let mut preparing = (*ability("Capture", false)).clone();
        preparing.preparation_frames = 10;
        t.special_abilities = vec![ability("Capture", true), Arc::new(preparing)];
        let id = place(&mut g, t);
        let victim = place(&mut g, unit("Enemy"));
        let slot = g.objects[&id].behavior.special_ability_handles[0].1;
        assert!(g.command_do_special_power(0, id, "Capture", crate::special_power::SpecialPowerTarget::Object(victim)));
        g.run_frames(2);
        assert!(g.objects[&id].ability.is_some(), "still active");
        assert_eq!(slot_key(&g, slot) >> 2, g.frame(), "NONE keeps it due every frame");
    }

    fn minefield(upgradable: bool) -> crate::modules4::GenerateMinefieldTemplate {
        crate::modules4::GenerateMinefieldTemplate {
            mine_name: Some("ChinaMine".into()),
            upgraded_mine_name: Some("ChinaEMPMine".into()),
            upgraded_trigger: Some("Upgrade_ChinaEMPMines".into()),
            distance_around_object: 10.0,
            mines_per_square_foot: 0.0,
            generate_only_on_death: false,
            border_only: true,
            smart_border: false,
            smart_border_skip_interior: false,
            always_circular: false,
            upgradable,
            random_jitter: 0.0,
            skip_if_this_much_under_structure: 0.33,
            starts_active: false,
            triggered_by: vec![],
            conflicts_with: vec![],
        }
    }

    /// `GenerateMinefieldBehavior` is an `UpdateModule` (GenerateMinefieldBehavior.h:68), so `registerObject` puts a heap
    /// entry for it in the INI order (ChinaBarracks: after `QueueProductionExitUpdate`, before `FlammableUpdate`). Its
    /// `update` returns NONE while the field is upgradable and not yet upgraded (GenerateMinefieldBehavior.cpp:492), so
    /// the entry is due every frame. The AN5 seed-1 heap at f1000 and f1163 holds one such entry per China building, and
    /// those entries change the equal-key pop order of the barracks' production and queue exit (AN5 f1999). Revert check:
    /// without the registration the family is absent from the heap.
    #[test]
    fn an_upgradable_minefield_is_a_heap_entry_awake_every_frame() {
        let mut g = logic();
        let mut t = unit("Barracks");
        t.kind_of = crate::template::kind_of::STRUCTURE;
        t.generate_minefield = Some(minefield(true));
        t.update_module_order = vec![crate::template::SleepyModuleKind::GenerateMinefield];
        let id = place(&mut g, t);
        assert_eq!(family_key(&g, id, FamilyKind::GenerateMinefield), Some(g.frame().max(1) << 2 | 2), "registered at the creation frame, PHASE_NORMAL");
        g.run_frames(2);
        assert_eq!(family_key(&g, id, FamilyKind::GenerateMinefield), Some(g.frame() << 2 | 2), "NONE: due every frame");
    }

    /// Without `Upgradable` the update returns `UPDATE_SLEEP_FOREVER` (GenerateMinefieldBehavior.cpp:495): the entry is in
    /// the heap, but asleep after its first update. Revert check: without the registration the family is absent.
    #[test]
    fn a_minefield_that_is_not_upgradable_sleeps_forever_after_its_first_update() {
        let mut g = logic();
        let mut t = unit("Mines");
        t.kind_of = crate::template::kind_of::STRUCTURE;
        t.generate_minefield = Some(minefield(false));
        let id = place(&mut g, t);
        g.run_frames(2);
        assert_eq!(family_key(&g, id, FamilyKind::GenerateMinefield), Some((UPDATE_SLEEP_FOREVER << 2) | 2));
    }
}
