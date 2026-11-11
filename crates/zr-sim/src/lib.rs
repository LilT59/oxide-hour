//! Deterministic, headless simulation core, structured after the original `GameLogic`.
//!
//! No rendering, no wall-clock time, no `std` transcendental math: every input arrives as a
//! [`Command`] and every random number comes from the seeded [`GameRandom`].
//!
//! | Module | Original |
//! |---|---|
//! | [`logic`] | GameLogic/System/GameLogic.cpp (`update`, `processCommandList`, `processDestroyList`, `registerObject`) |
//! | [`schedule`] | the `m_sleepyUpdates` heap in GameLogic.cpp + `UpdateModule` wake bookkeeping |
//! | [`module`] | GameLogic/Module/UpdateModule.h |
//! | [`object`] | GameLogic/Object.h |
//! | [`body`] | Object/Body/ActiveBody.cpp |
//! | [`ai`] | Object/Update/AIUpdate.cpp + Object/Locomotor.cpp (reduced) |
//! | [`terrain`] | TerrainLogic / `BaseHeightMapRenderObjClass::getHeightMapHeight` |
//! | [`partition`] | Object/PartitionManager.cpp |
//! | [`pathfind`] | GameLogic/AI/AIPathfind.cpp (ground A*, cells, footprints, path optimisation) |
//! | `possession` | docs/POSSESSION.md (feature `possession`, off by default; not part of Zero Hour): `Possess` / `PossessInput` / `Release` driving a unit through its locomotor, turret and weapons |
//! | [`random`] | Core/GameEngine/Source/Common/RandomValue.cpp |
//! | [`crc`] | Common/Xfer/XferCRC.cpp |
//! | [`command`] | `GameMessage` network messages (+ the possession messages with feature `possession`) |
//! | [`weapon`], [`armor`], [`damage`], [`combat`] | Object/Weapon.cpp, WeaponSet.cpp, Armor.cpp, `dealDamageInternal`, AIStates.cpp attack states (reduced) |
//! | [`template`] | the adapter seam for `ThingTemplate` / `LocomotorTemplate` |

pub mod ai;
pub mod assault_transport;
pub mod vector_math;
pub mod ai_guard_retaliate;
pub mod ai_states;
mod guard_area;
pub mod armor;
pub mod assist;
pub mod behaviors;
pub mod body;
pub mod bonus;
pub mod bridge;
pub mod bridge_logic;
pub mod build;
pub mod chinook;
pub mod collide;
pub mod combat;
pub mod command;
pub mod condition;
pub mod contain;
mod contain_exit;
pub mod crc;
pub mod crtmath;
pub mod damage;
pub mod die;
pub mod disabled;
pub mod dock;
pub mod dock_actions;
pub mod dozer;
pub mod energy;
pub mod enter_actions;
pub mod experience;
pub mod garrison;
pub mod hack_internet;
mod horde;
mod id_dump;
#[cfg(test)]
mod production_exit_l156b;
#[cfg(test)]
mod wake_l192;
#[cfg(test)]
mod keepalive_l192;
#[cfg(test)]
mod deploy_style_l195;
pub mod jet_ai;
pub mod jet_states;
pub mod locomotor;
pub mod logic;
pub mod los;
pub mod math;
mod mood;
pub mod module;
pub mod modules2;
pub mod modules3;
pub mod modules4;
pub mod object;
pub(crate) mod object_helpers;
pub(crate) mod sleepy_families;
pub mod ocl;
pub mod orders;
pub mod partition;
mod partition_l175;
mod collide_l181;
mod defect_l193;
mod cash_bounty_l193;
pub mod partition_value;
pub mod pathfind;
pub mod physics;
pub mod player;
pub mod player_orders;
#[cfg(feature = "possession")]
pub mod possession;
pub mod production;
pub mod laser_beam;
pub mod projectile;
pub mod projectile_stream;
pub mod random;
pub mod railroad;
pub mod repair;
pub mod rider;
pub mod schedule;
pub mod science;
pub mod score;
pub mod script;
pub mod shroud;
pub mod skirmish_ai;
pub mod special_power;
pub mod superweapon;
pub mod supply;
pub mod template;
pub mod terrain;
pub mod transform;
pub mod tunnel;
pub mod turret_ai;
pub mod upgrade;
pub mod weapon;
pub mod weapon_orders;
pub mod zone;

pub use combat::{AttackOrder, AttackPhase, AttackTarget, ObjectCombat, ProjectileSnapshot};
pub use laser_beam::{LaserBeam, LaserRollKind, LaserSpec};
pub use projectile_stream::{ProjectileStreamSnapshot, MAX_PROJECTILE_STREAM};
pub use body::DamageInfo;
pub use damage::{DamageType, DeathType};
pub use command::{Command, CombatDropTarget, GameMessage};
#[cfg(feature = "possession")]
pub use command::PossessInput;
pub use condition::{BodyDamageType, ModelConditionFlags};
pub use contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
pub use die::{DieMux, DieModuleTemplate, DieTemplate, FxEvent, SlowDeathTemplate};
pub use disabled::DisabledType;
pub use energy::GameRules;
pub use enter_actions::{EnterAction, EnterKind};
pub use logic::GameLogic;
pub use ocl::{CreateObjectTemplate, OclNugget, OclTemplate};
pub use math::{Coord3D, LOGICFRAMES_PER_SECOND, MAP_HEIGHT_SCALE, MAP_XY_FACTOR};
pub use object::{Object, ObjectId, PlayerIndex};
pub use orders::{GuardMode, GuardTarget};
pub use physics::{Physics, PhysicsTemplate};
pub use player::Player;
pub use production::{Prerequisite, ProductionEntry, ProductionExit, ProductionKind, ProductionState, ProductionTemplate, QuantityModifier};
pub use projectile::{DumbProjectileParams, MissileParams, MissileState, ProjectileBehavior, ProjectileTemplate};
pub use special_power::SpecialPowerTarget;
pub use random::GameRandom;
pub use shroud::{CellShroudStatus, ObjectShroudStatus, Shroud, Sighting};
pub use skirmish_ai::{AiBuildOrder, SkirmishAi, SkirmishAiConfig};
pub use supply::{DockKind, DockTemplate, SupplyTruckTemplate};
pub use template::{GrantUpgradeCreateTemplate, LocomotorKind, LocomotorParams, LocomotorSet, ObjectTemplate, TemplateSource, ZBehavior};
pub use terrain::TerrainLogic;
pub use tunnel::TunnelTracker;
