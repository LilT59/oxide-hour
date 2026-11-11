//! Zero Hour `.map` reader.
//!
//! A map is a "CkMp" DataChunk file ([`chunk`]); [`map::MapFile`] decodes the
//! well-known chunks into typed data and keeps every top-level chunk listed.
//! [`triggers`] holds polygon triggers (water areas, rivers), [`scripts`] the script tree.

pub mod chunk;
pub mod map;
pub mod script_names;
pub mod scripts;
pub mod triggers;

pub use chunk::{Chunk, ChunkError, ChunkIter, Cursor, Dict, DictValue, Names};
pub use map::{BlendTileData, BlendTileInfo, BuildListEntry, ChunkInfo, CliffInfo, DecodeError, GlobalLight, GlobalLighting, HeightMap, MapFile, MapObject, Side, Sides, TextureClass};
pub use scripts::{
    parse_scripts_file, qualify_team_dict, Action, Condition, OrCondition, ParamValue, Parameter,
    PlayerScripts, Script, ScriptGroup, ScriptList, ScriptPlayer, ScriptStats, ScriptsFile, THIS_TEAM,
};
pub use triggers::{PolygonTrigger, PolygonTriggers};
