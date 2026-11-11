//! Glue between typed template data (`zr-templates`), the deterministic simulation (`zr-sim`)
//! and the map (`zr-map`): the layer clients (viewer, possession) talk to.
//!
//! * [`adapter`]: `ThingTemplate` -> `zr_sim::ObjectTemplate` (`TemplateSource` over `Templates`).
//! * [`data`]: [`GameData`] (templates, `PlayerTemplate`s, skirmish starting cash), loaded once.
//! * [`game`]: [`Game::new_skirmish`] (a port of `GameLogic::startNewGame`) and the read/command API.

pub mod adapter;
pub mod bridges;
pub mod data;
pub mod firepoints;
pub mod game;
pub mod dock_bones;
pub mod launch_bones;
pub mod parking_bones;
pub mod replay;
pub mod snapshot;
pub mod spawn_bones;
pub mod trace;

pub use adapter::{
    convert_armor, convert_combat, convert_locomotor, convert_thing, convert_weapon, kind_of_bits, CombatCache, TemplateStore,
};
pub use data::{GameData, PlayerTemplate};
pub use game::{AiDifficulty, Game, MapLoadStats, ObjectView, PlayerInfo, PlayerKind, PlayerSetup, Spawned, Waypoint, store_template_source};
