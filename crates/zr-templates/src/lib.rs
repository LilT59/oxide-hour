//! Typed, engine-faithful game data built from [`zr_ini::IniDb`].
//!
//! This is the data layer the simulation reads: [`ThingTemplate`] (objects, with typed body / AI /
//! physics modules and raw everything else), [`WeaponTemplate`], [`LocomotorTemplate`] and
//! [`ArmorTemplate`], all held by a [`Templates`] registry.
//!
//! # Unit conventions
//!
//! Values follow the engine's INI field parsers (`Core/GameEngine/Source/Common/INI/INI.cpp`,
//! `GameCommon.h`), as AGENTS.md requires:
//!
//! * durations in milliseconds become logic frames (30 per second): `parseDurationUnsignedInt`
//!   rounds up (`ceilf`), `parseDurationReal` keeps the fraction;
//! * velocities per second become per frame (`* 1/30`), accelerations per second^2 become per frame^2
//!   (`* 1/30 * 1/30`);
//! * angles in degrees become radians (`* PI/180`), angular velocities degrees/s become radians/frame;
//! * percents become fractions (`50%` is `0.5`).
//!
//! Everything is `f32` with plain IEEE arithmetic, like the engine's `Real`.
//!
//! # Warnings
//!
//! Nothing is dropped silently: unknown fields, rejected values, dangling references and structural
//! problems are returned as [`Warning`]s by [`Templates::from_ini`].

pub mod ai_data;
pub mod armor;
pub mod behavior;
pub mod contain;
pub mod diag;
pub mod die;
pub mod dock;
pub mod draw;
pub mod flags;
pub mod locomotor;
pub mod logic;
pub mod module;
mod module_table;
pub mod modules2;
pub mod names;
pub mod ocl;
pub mod production;
pub mod production_exit;
pub mod projectile;
pub mod registry;
pub mod special_power;
pub mod thing;
pub mod upgrade;
pub mod upgrade_module;
pub mod value;
pub mod weapon;

pub use ai_data::{AiData, AiSideBuildList, AiSideInfo, BuildListEntry};
pub use armor::ArmorTemplate;
pub use contain::{ContainData, ContainKind};
pub use dock::{DockData, DockKind};
pub use diag::{RawBlock, RawField, RawItem, Warning, WarningKind};
pub use die::{DieData, DieKind, DieMuxData, SlowDeathData};
pub use draw::DrawModule;
pub use flags::{ArmorSetFlags, KindOfMask, Mask, NameEnum, WeaponSetFlags};
pub use locomotor::LocomotorTemplate;
pub use logic::{BridgeFx, BridgeOcl, LogicModule, NeutronBlast, RadiusDecalData};
pub use module::{
    AiUpdateData, BodyClass, BodyData, Module, ModuleData, ModuleKind, PhysicsData, TurretAiData,
};
pub use module_table::iface;
pub use names::*;
pub use ocl::{
    AnimSet, ApplyRandomForce, AttackNugget, DeliverPayload, DeliverPayloadData, FireWeaponNugget, GenericNugget,
    Nugget, ObjectCreationList, Payload,
};
pub use production::{ProductionData, QuantityModifier};
pub use production_exit::{ProductionExitData, ProductionExitKind};
pub use projectile::{DumbProjectileData, MissileAiData};
pub use registry::{ArmorId, LocomotorId, OclId, Ref, Templates, ThingId, UpgradeId, WeaponId};
pub use special_power::{RankInfo, ScienceInfo, SpecialPowerTemplate};
pub use thing::{
    ArmorSet, Geometry, MaxSimultaneous, Prerequisite, ThingTemplate, WeaponSet,
};
pub use upgrade::UpgradeTemplate;
pub use upgrade_module::{UpgradeData, UpgradeKind, UpgradeMuxData};
pub use weapon::{WeaponAnti, WeaponTemplate};

#[cfg(test)]
mod tests;
