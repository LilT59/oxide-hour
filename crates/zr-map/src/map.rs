//! Typed decoding of the map chunks.
//!
//! Sources (all under `references/GeneralsGameCode/Core/GameEngine*/`):
//! - `GameEngineDevice/.../W3DDevice/GameClient/WorldHeightMap.cpp`
//!   (`ParseHeightMapData`, `ParseSizeOnly`, `ParseObjectData`, `ParseWorldDictDataChunk`)
//! - `Include/Common/MapReaderWriterInfo.h` (chunk versions)
//! - `GameLogic/Map/SidesList.cpp` (`ParseSidesDataChunk`)
//! - `GameLogic/Map/TerrainLogic.cpp` (`parseWaypointData`)
//!
//! `PolygonTriggers` is decoded in [`crate::triggers`], the script tree in [`crate::scripts`].
//! Chunks without a typed decoder are only listed in [`MapFile::chunks`]. Decoders of
//! optional data (triggers, scripts, lighting) never fail the whole map: errors are recorded
//! in [`MapFile::decode_errors`].

use crate::chunk::{self, Chunk, ChunkError, ChunkIter, Cursor, Dict, Names, Result};
use crate::scripts::PlayerScripts;
use crate::triggers::PolygonTriggers;

/// A top-level (or nested) chunk seen in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkInfo {
    pub name: String,
    pub version: u16,
    pub size: usize,
}

impl From<&Chunk<'_>> for ChunkInfo {
    fn from(c: &Chunk<'_>) -> Self {
        Self { name: c.name.to_owned(), version: c.version, size: c.data.len() }
    }
}

/// `HeightMapData` (versions 1..=4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeightMap {
    pub version: u16,
    pub width: i32,
    pub height: i32,
    /// Border cells on each side (version >= 3, else 0).
    pub border_size: i32,
    /// Playable boundary rectangles (version >= 4, else one derived from size and border).
    pub boundaries: Vec<(i32, i32)>,
    /// `width * height` height samples, row-major.
    pub data: Vec<u8>,
}

impl HeightMap {
    fn parse(c: &Chunk, names: &Names) -> Result<Self> {
        let mut r = c.cursor(names);
        let (mut width, mut height) = (r.i32()?, r.i32()?);
        let border_size = if c.version >= 3 { r.i32()? } else { 0 };
        let boundaries = if c.version >= 4 {
            let n = r.i32()?;
            let n = usize::try_from(n).map_err(|_| ChunkError::Corrupt("negative boundary count"))?;
            if n > r.left() / 8 {
                return Err(ChunkError::Corrupt("boundary count exceeds chunk"));
            }
            (0..n).map(|_| Ok((r.i32()?, r.i32()?))).collect::<Result<_>>()?
        } else {
            vec![(width - 2 * border_size, height - 2 * border_size)]
        };
        let size = r.i32()?;
        let expected = i64::from(width) * i64::from(height);
        if size <= 0 || i64::from(size) != expected {
            return Err(ChunkError::Corrupt("height data size != width * height"));
        }
        let mut data = r.bytes(size as usize)?.to_vec();
        if c.version == 1 {
            // Version 1 stored cells at half spacing: keep every other sample (ParseSizeOnly).
            let (nw, nh) = ((width + 1) / 2, (height + 1) / 2);
            let (w, nw_u) = (width as usize, nw as usize);
            data = (0..nh as usize).flat_map(|y| (0..nw_u).map(move |x| (y, x))).map(|(y, x)| data[2 * y * w + 2 * x]).collect();
            (width, height) = (nw, nh);
        }
        Ok(Self { version: c.version, width, height, border_size, boundaries, data })
    }

    /// Height sample at cell `(x, y)`.
    pub fn at(&self, x: usize, y: usize) -> Option<u8> {
        if x >= self.width as usize || y >= self.height as usize {
            return None;
        }
        self.data.get(y * self.width as usize + x).copied()
    }
}

/// Texture class entry of `BlendTileData` (`TXTextureClass` on disk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureClass {
    /// Index of the class's first 64x64 source tile.
    pub first_tile: i32,
    pub num_tiles: i32,
    /// Width of the class in tiles (the class is `width * width` tiles).
    pub width: i32,
    /// Terrain type name (`Terrain` INI block), or a texture file name.
    pub name: String,
}

/// `TBlendTileInfo`: how one blend-tile index blends into its cell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlendTileInfo {
    /// Source tile index (tile * 4 + quadrant).
    pub blend_ndx: i32,
    pub horiz: bool,
    pub vert: bool,
    pub right_diagonal: bool,
    pub left_diagonal: bool,
    /// Raw `inverted` byte: bit 0 = `INVERTED_MASK`, bit 1 = `FLIPPED_MASK`.
    pub inverted: u8,
    pub long_diagonal: bool,
    /// Custom edge texture class, `-1` = plain alpha blend.
    pub custom_blend_edge_class: i32,
}

/// `TCliffInfo`: custom UVs for a cliff cell.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CliffInfo {
    pub tile_index: i32,
    /// Four corner UVs `(u, v)` in the engine's convention (see `WorldHeightMap::getUVForTileIndex`).
    pub uv: [[f32; 2]; 4],
    pub flip: bool,
    pub mutant: bool,
}

/// `BlendTileData` (versions 2..=8): per-cell texture indices.
///
/// Ported from `WorldHeightMap::ParseBlendTileData`
/// (`Core/GameEngineDevice/Source/W3DDevice/GameClient/WorldHeightMap.cpp`).
/// All per-cell arrays are `width * height` of the height map, row-major, y up.
#[derive(Debug, Clone, PartialEq)]
pub struct BlendTileData {
    pub version: u16,
    pub width: i32,
    pub height: i32,
    /// Base tile index (tile * 4 + quadrant) per cell.
    pub tile_ndxes: Vec<i16>,
    /// Index into [`Self::blended_tiles`], 0 = none.
    pub blend_tile_ndxes: Vec<i16>,
    /// Third-layer blend index into [`Self::blended_tiles`], 0 = none (version >= 6).
    pub extra_blend_tile_ndxes: Vec<i16>,
    /// Index into [`Self::cliff_info`], 0 = none (version >= 5).
    pub cliff_info_ndxes: Vec<i16>,
    /// Cliff bit per cell, `(width + 7) / 8` bytes per row (version >= 7; else empty).
    pub cell_cliff_state: Vec<u8>,
    pub num_bitmap_tiles: i32,
    pub texture_classes: Vec<TextureClass>,
    pub num_edge_tiles: i32,
    pub edge_texture_classes: Vec<TextureClass>,
    /// Entry 0 is the unused default.
    pub blended_tiles: Vec<BlendTileInfo>,
    /// Entry 0 is the unused default.
    pub cliff_info: Vec<CliffInfo>,
}

/// `FLAG_VAL` sentinel after each blend tile record.
const BLEND_FLAG_VAL: i32 = 0x7ADA0000;

impl BlendTileData {
    fn parse(c: &Chunk, names: &Names, width: i32, height: i32) -> Result<Self> {
        if c.version == 1 {
            return Err(ChunkError::Corrupt("BlendTileData version 1 (5-unit cells) unsupported"));
        }
        let mut r = c.cursor(names);
        let len = r.i32()?;
        let cells = usize::try_from(i64::from(width) * i64::from(height))
            .map_err(|_| ChunkError::Corrupt("bad height map size"))?;
        if usize::try_from(len).ok() != Some(cells) {
            return Err(ChunkError::Corrupt("BlendTileData size != height map size"));
        }
        let shorts = |r: &mut Cursor| -> Result<Vec<i16>> {
            let b = r.bytes(cells.checked_mul(2).ok_or(ChunkError::Corrupt("overflow"))?)?;
            Ok(b.chunks_exact(2).map(|p| i16::from_le_bytes([p[0], p[1]])).collect())
        };
        let tile_ndxes = shorts(&mut r)?;
        let blend_tile_ndxes = shorts(&mut r)?;
        let extra_blend_tile_ndxes = if c.version >= 6 { shorts(&mut r)? } else { vec![0; cells] };
        let cliff_info_ndxes = if c.version >= 5 { shorts(&mut r)? } else { vec![0; cells] };
        let stride = (width as usize).div_ceil(8);
        let mut cell_cliff_state = Vec::new();
        if c.version >= 7 {
            if c.version == 7 {
                // Writer bug kept for compatibility: rows were saved `(w + 1) / 8` bytes wide.
                let bw = (width as usize + 1) / 8;
                let raw = r.bytes(bw * height as usize)?;
                cell_cliff_state = vec![0; stride * height as usize];
                for j in 0..height as usize {
                    for i in 0..bw.min(stride) {
                        cell_cliff_state[j * stride + i] = raw[j * bw + i];
                    }
                }
            } else {
                cell_cliff_state = r.bytes(stride * height as usize)?.to_vec();
            }
        }
        let num_bitmap_tiles = r.i32()?;
        let num_blended = r.i32()?;
        let num_cliff = if c.version >= 5 { r.i32()? } else { 1 };
        let tex_class = |r: &mut Cursor, legacy: bool| -> Result<TextureClass> {
            let first_tile = r.i32()?;
            let num_tiles = r.i32()?;
            let width = r.i32()?;
            if legacy {
                r.i32()?;
            }
            Ok(TextureClass { first_tile, num_tiles, width, name: r.ascii()? })
        };
        let n = count(&mut r, 18)?;
        let texture_classes = (0..n).map(|_| tex_class(&mut r, true)).collect::<Result<Vec<_>>>()?;
        let (mut num_edge_tiles, mut edge_texture_classes) = (0, Vec::new());
        if c.version >= 4 {
            num_edge_tiles = r.i32()?;
            let n = count(&mut r, 14)?;
            edge_texture_classes = (0..n).map(|_| tex_class(&mut r, false)).collect::<Result<Vec<_>>>()?;
        }
        let num_blended = usize::try_from(num_blended).map_err(|_| ChunkError::Corrupt("negative blend count"))?;
        if num_blended > r.left() / 10 + 1 {
            return Err(ChunkError::Corrupt("blend tile count exceeds chunk"));
        }
        let mut blended_tiles = vec![BlendTileInfo { custom_blend_edge_class: -1, ..Default::default() }];
        for _ in 1..num_blended {
            let blend_ndx = r.i32()?;
            let (horiz, vert, right_diagonal, left_diagonal) = (r.bool()?, r.bool()?, r.bool()?, r.bool()?);
            let inverted = r.u8()?;
            let long_diagonal = if c.version >= 3 { r.bool()? } else { false };
            let custom_blend_edge_class = if c.version >= 4 { r.i32()? } else { -1 };
            if r.i32()? != BLEND_FLAG_VAL {
                return Err(ChunkError::Corrupt("bad blend tile sentinel"));
            }
            blended_tiles.push(BlendTileInfo {
                blend_ndx,
                horiz,
                vert,
                right_diagonal,
                left_diagonal,
                inverted,
                long_diagonal,
                custom_blend_edge_class,
            });
        }
        let mut cliff_info = vec![CliffInfo::default()];
        if c.version >= 5 {
            let num_cliff = usize::try_from(num_cliff).map_err(|_| ChunkError::Corrupt("negative cliff count"))?;
            if num_cliff > r.left() / 38 + 1 {
                return Err(ChunkError::Corrupt("cliff info count exceeds chunk"));
            }
            for _ in 1..num_cliff {
                let tile_index = r.i32()?;
                let mut uv = [[0.0; 2]; 4];
                for p in &mut uv {
                    *p = [r.f32()?, r.f32()?];
                }
                cliff_info.push(CliffInfo { tile_index, uv, flip: r.bool()?, mutant: r.bool()? });
            }
        }
        Ok(Self {
            version: c.version,
            width,
            height,
            tile_ndxes,
            blend_tile_ndxes,
            extra_blend_tile_ndxes,
            cliff_info_ndxes,
            cell_cliff_state,
            num_bitmap_tiles,
            texture_classes,
            num_edge_tiles,
            edge_texture_classes,
            blended_tiles,
            cliff_info,
        })
    }
}

/// One `Object` chunk inside `ObjectsList` (versions 1..=3; dict from version 2).
#[derive(Debug, Clone, PartialEq)]
pub struct MapObject {
    pub version: u16,
    pub pos: [f32; 3],
    pub angle: f32,
    pub flags: i32,
    pub template: String,
    pub props: Dict,
}

impl MapObject {
    fn parse(c: &Chunk, names: &Names) -> Result<Self> {
        let mut r = c.cursor(names);
        let mut pos = [r.f32()?, r.f32()?, r.f32()?];
        if c.version <= 2 {
            pos[2] = 0.0;
        }
        let angle = r.f32()?;
        let flags = r.i32()?;
        let template = r.ascii()?;
        let props = if c.version >= 2 { r.dict()? } else { Dict::default() };
        Ok(Self { version: c.version, pos, angle, flags, template, props })
    }
}

/// Starting-building entry of a side's build list.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildListEntry {
    pub building_name: String,
    pub template: String,
    pub pos: [f32; 3],
    pub angle: f32,
    pub initially_built: bool,
    pub num_rebuilds: i32,
    /// Version >= 3 fields: script, health, whiner, unsellable, repairable.
    pub script: String,
    pub health: i32,
    pub whiner: bool,
    pub unsellable: bool,
    pub repairable: bool,
}

/// A player ("side") with its build list.
#[derive(Debug, Clone, PartialEq)]
pub struct Side {
    pub dict: Dict,
    pub build_list: Vec<BuildListEntry>,
}

/// `SidesList` chunk: sides, teams (version >= 2) and trailing nested chunks (`PlayerScriptsList`).
#[derive(Debug, Clone, PartialEq)]
pub struct Sides {
    pub version: u16,
    pub sides: Vec<Side>,
    pub teams: Vec<Dict>,
    pub nested: Vec<ChunkInfo>,
    /// Nested `PlayerScriptsList` (one `ScriptList` per side), if present and decodable.
    pub scripts: Option<PlayerScripts>,
    /// Why `PlayerScriptsList` could not be decoded (the sides themselves still are).
    pub script_error: Option<ChunkError>,
}

/// Rejects counts that cannot possibly fit in the remaining bytes (each item >= `min` bytes).
pub(crate) fn count(r: &mut Cursor, min: usize) -> Result<usize> {
    let n = usize::try_from(r.i32()?).map_err(|_| ChunkError::Corrupt("negative count"))?;
    if n > r.left() / min {
        return Err(ChunkError::Corrupt("count exceeds chunk"));
    }
    Ok(n)
}

impl Sides {
    fn parse(c: &Chunk, names: &Names) -> Result<Self> {
        let mut r = c.cursor(names);
        let mut sides = Vec::new();
        for _ in 0..count(&mut r, 6)? {
            let dict = r.dict()?;
            let mut build_list = Vec::new();
            for _ in 0..count(&mut r, 4)? {
                let building_name = r.ascii()?;
                let template = r.ascii()?;
                let pos = [r.f32()?, r.f32()?, r.f32()?];
                let angle = r.f32()?;
                let initially_built = r.bool()?;
                let num_rebuilds = r.i32()?;
                let mut e = BuildListEntry {
                    building_name,
                    template,
                    pos,
                    angle,
                    initially_built,
                    num_rebuilds,
                    script: String::new(),
                    health: 0,
                    whiner: false,
                    unsellable: false,
                    repairable: false,
                };
                if c.version >= 3 {
                    e.script = r.ascii()?;
                    e.health = r.i32()?;
                    e.whiner = r.bool()?;
                    e.unsellable = r.bool()?;
                    e.repairable = r.bool()?;
                }
                build_list.push(e);
            }
            sides.push(Side { dict, build_list });
        }
        let mut teams = Vec::new();
        if c.version >= 2 {
            for _ in 0..count(&mut r, 2)? {
                teams.push(r.dict()?);
            }
        }
        let (mut nested, mut scripts, mut script_error) = (Vec::new(), None, None);
        for child in r.chunks() {
            let child = child?;
            nested.push(ChunkInfo::from(&child));
            if child.name == "PlayerScriptsList" {
                // Lenient: a malformed script tree must not make the sides unusable.
                match PlayerScripts::parse(&child, names) {
                    Ok(s) => scripts = Some(s),
                    Err(e) => script_error = Some(e),
                }
            }
        }
        Ok(Self { version: c.version, sides, teams, nested, scripts, script_error })
    }
}

/// One global light: ambient + diffuse colour (0..1 floats, may exceed 1) and the direction
/// the light travels (`lightPos` in the engine; unit vector in practice, Z up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlobalLight {
    pub ambient: [f32; 3],
    pub diffuse: [f32; 3],
    pub direction: [f32; 3],
}

impl Default for GlobalLight {
    /// `initLightValues` in `ParseLightingDataChunk`: black, pointing straight down.
    fn default() -> Self {
        Self { ambient: [0.0; 3], diffuse: [0.0; 3], direction: [0.0, 0.0, -1.0] }
    }
}

/// `GlobalLighting` chunk (versions 1..=3): four times of day (morning, afternoon, evening,
/// night), each with up to 3 terrain lights and 3 object lights.
///
/// Ported from `WorldHeightMap::ParseLightingDataChunk` (`WorldHeightMap.cpp`).
#[derive(Debug, Clone, PartialEq)]
pub struct GlobalLighting {
    pub version: u16,
    /// `TimeOfDay` enum: 1 morning, 2 afternoon, 3 evening, 4 night.
    pub time_of_day: i32,
    /// `[time of day - 1][light]` for terrain (only light 0 exists before version 3).
    pub terrain: [[GlobalLight; 3]; 4],
    /// `[time of day - 1][light]` for objects (only light 0 exists before version 2).
    pub objects: [[GlobalLight; 3]; 4],
    /// Shadow colour (ARGB), if the chunk carries one.
    pub shadow_color: Option<u32>,
}

impl GlobalLighting {
    fn parse(c: &Chunk, names: &Names) -> Result<Self> {
        let mut r = c.cursor(names);
        let time_of_day = r.i32()?;
        let mut terrain = [[GlobalLight::default(); 3]; 4];
        let mut objects = terrain;
        let light = |r: &mut Cursor| -> Result<GlobalLight> {
            let mut v = [0.0f32; 9];
            for x in &mut v {
                *x = r.f32()?;
            }
            Ok(GlobalLight { ambient: [v[0], v[1], v[2]], diffuse: [v[3], v[4], v[5]], direction: [v[6], v[7], v[8]] })
        };
        for (t, o) in terrain.iter_mut().zip(objects.iter_mut()) {
            t[0] = light(&mut r)?;
            o[0] = light(&mut r)?;
            if c.version >= 2 {
                for slot in &mut o[1..] {
                    *slot = light(&mut r)?;
                }
            }
            if c.version >= 3 {
                for slot in &mut t[1..] {
                    *slot = light(&mut r)?;
                }
            }
        }
        let shadow_color = if r.left() >= 4 { Some(r.u32()?) } else { None };
        Ok(Self { version: c.version, time_of_day, terrain, objects, shadow_color })
    }

    /// Index into the per-time-of-day arrays for the map's current time of day (clamped).
    pub fn tod_index(&self) -> usize {
        (self.time_of_day.clamp(1, 4) - 1) as usize
    }
}

/// A non-fatal decode failure of an optional chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    pub chunk: String,
    pub error: ChunkError,
}

/// A decoded map file.
#[derive(Debug, Clone, Default)]
pub struct MapFile {
    /// Every top-level chunk in file order.
    pub chunks: Vec<ChunkInfo>,
    pub height_map: Option<HeightMap>,
    /// `BlendTileData` (needs `HeightMapData` for its dimensions).
    pub blend_tiles: Option<BlendTileData>,
    /// `WorldInfo`: a single dict (weather, music, ...).
    pub world_info: Option<Dict>,
    /// `ObjectsList` children.
    pub objects: Vec<MapObject>,
    pub sides: Option<Sides>,
    /// `GlobalLighting`: per time-of-day terrain and object lights.
    pub lighting: Option<GlobalLighting>,
    /// `WaypointsList`: pairs of waypoint ids.
    pub waypoint_links: Vec<(i32, i32)>,
    /// `PolygonTriggers`: water areas, rivers and script areas.
    pub triggers: Option<PolygonTriggers>,
    /// A top-level `PlayerScriptsList` (normally the scripts live inside `SidesList`).
    pub top_level_scripts: Option<PlayerScripts>,
    /// Optional chunks that failed to decode (`triggers`, scripts, `lighting`).
    pub decode_errors: Vec<DecodeError>,
}

impl MapFile {
    /// Parses a decompressed `.map` image. Never panics; malformed data yields `Err`.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let (names, body) = chunk::open(data)?;
        let mut m = MapFile::default();
        let mut blend: Option<Chunk> = None;
        for c in ChunkIter::new(body, &names) {
            let c = c?;
            m.chunks.push(ChunkInfo::from(&c));
            match c.name {
                "HeightMapData" => m.height_map = Some(HeightMap::parse(&c, &names)?),
                "BlendTileData" => blend = Some(c),
                "WorldInfo" => m.world_info = Some(c.cursor(&names).dict()?),
                "ObjectsList" => {
                    for o in c.children(&names) {
                        let o = o?;
                        if o.name == "Object" {
                            m.objects.push(MapObject::parse(&o, &names)?);
                        }
                    }
                }
                "SidesList" => m.sides = Some(Sides::parse(&c, &names)?),
                // Lenient: a malformed lighting chunk must not make the map unloadable.
                "GlobalLighting" => match GlobalLighting::parse(&c, &names) {
                    Ok(l) => m.lighting = Some(l),
                    Err(error) => m.decode_errors.push(DecodeError { chunk: c.name.to_owned(), error }),
                },
                "PolygonTriggers" => match PolygonTriggers::parse(&c, &names) {
                    Ok(t) => m.triggers = Some(t),
                    Err(error) => m.decode_errors.push(DecodeError { chunk: c.name.to_owned(), error }),
                },
                "PlayerScriptsList" => match PlayerScripts::parse(&c, &names) {
                    Ok(s) => m.top_level_scripts = Some(s),
                    Err(error) => m.decode_errors.push(DecodeError { chunk: c.name.to_owned(), error }),
                },
                "WaypointsList" => {
                    let mut r = c.cursor(&names);
                    for _ in 0..count(&mut r, 8)? {
                        m.waypoint_links.push((r.i32()?, r.i32()?));
                    }
                }
                _ => {}
            }
        }
        if let Some(error) = m.sides.as_mut().and_then(|s| s.script_error.take()) {
            m.decode_errors.push(DecodeError { chunk: "PlayerScriptsList".into(), error });
        }
        if let (Some(c), Some(h)) = (blend, &m.height_map) {
            m.blend_tiles = Some(BlendTileData::parse(&c, &names, h.width, h.height)?);
        }
        Ok(m)
    }

    /// The script tree: nested in `SidesList` for shipped maps, else a top-level chunk.
    pub fn scripts(&self) -> Option<&PlayerScripts> {
        self.sides.as_ref().and_then(|s| s.scripts.as_ref()).or(self.top_level_scripts.as_ref())
    }

    /// Names of the top-level chunks, in order.
    pub fn chunk_names(&self) -> impl Iterator<Item = &str> {
        self.chunks.iter().map(|c| c.name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::test_util::*;
    use crate::chunk::DictValue;

    const NAMES: &[(&str, u32)] = &[
        ("HeightMapData", 1),
        ("ObjectsList", 2),
        ("Object", 3),
        ("WorldInfo", 4),
        ("WaypointsList", 5),
        ("SidesList", 6),
        ("Foo", 7),
        ("k", 8),
    ];

    fn hm(version: u16, w: i32, h: i32, tail_ver4: bool) -> Vec<u8> {
        let mut d = i32s(&[w, h]);
        if version >= 3 {
            d.extend(i32s(&[2]));
        }
        if version >= 4 {
            d.extend(i32s(&[1, w - 4, h - 4]));
        }
        let _ = tail_ver4;
        d.extend(i32s(&[w * h]));
        d.extend((0..w * h).map(|i| i as u8));
        chunk(1, version, &d)
    }

    fn obj(ver: u16, z: f32, name: &str, dict: bool) -> Vec<u8> {
        let mut d: Vec<u8> = [1.0f32, 2.0, z, 0.5].iter().flat_map(|f| f.to_le_bytes()).collect();
        d.extend(i32s(&[6]));
        d.extend(ascii(name));
        if dict {
            d.extend(1u16.to_le_bytes());
            d.extend((8u32 << 8 | 1).to_le_bytes());
            d.extend(42i32.to_le_bytes());
        }
        chunk(3, ver, &d)
    }

    #[test]
    fn heightmap_versions() {
        let m = MapFile::parse(&file(NAMES, &hm(4, 6, 5, true))).unwrap();
        let h = m.height_map.unwrap();
        assert_eq!((h.width, h.height, h.border_size), (6, 5, 2));
        assert_eq!(h.boundaries, vec![(2, 1)]);
        assert_eq!(h.at(1, 1), Some(7));
        assert_eq!(h.at(6, 0), None);

        let h = MapFile::parse(&file(NAMES, &hm(2, 4, 4, false))).unwrap().height_map.unwrap();
        assert_eq!((h.border_size, h.boundaries.clone()), (0, vec![(4, 4)]));

        // v1 halves the grid by keeping every other sample
        let h = MapFile::parse(&file(NAMES, &hm(1, 4, 4, false))).unwrap().height_map.unwrap();
        assert_eq!((h.width, h.height, h.data.clone()), (2, 2, vec![0, 2, 8, 10]));
    }

    #[test]
    fn heightmap_errors() {
        let mut bad = hm(2, 4, 4, false);
        bad.truncate(bad.len() - 3);
        // chunk header size now lies about the data
        assert!(MapFile::parse(&file(NAMES, &bad)).is_err());
        let mut d = i32s(&[4, 4, 5]);
        d.extend([0; 5]);
        assert!(MapFile::parse(&file(NAMES, &chunk(1, 2, &d))).is_err());
        let d = i32s(&[4, 4, 0, 1_000_000]);
        assert!(MapFile::parse(&file(NAMES, &chunk(1, 4, &d))).is_err());
    }

    #[test]
    fn objects_world_waypoints_sides() {
        let mut list = obj(3, 9.0, "Tree", true);
        list.extend(obj(1, 9.0, "Old", false));
        let mut body = chunk(2, 1, &list);
        body.extend(chunk(7, 1, &[1, 2, 3]));
        let mut w = 1u16.to_le_bytes().to_vec();
        w.extend((8u32 << 8 | 3).to_le_bytes());
        w.extend(ascii("x"));
        body.extend(chunk(4, 1, &w));
        body.extend(chunk(5, 1, &i32s(&[2, 1, 2, 2, 3])));
        // SidesList v3: 1 side (empty dict, 1 build entry), 1 empty team, nested Foo chunk
        let mut s = i32s(&[1]);
        s.extend(0u16.to_le_bytes());
        s.extend(i32s(&[1]));
        s.extend(ascii("b"));
        s.extend(ascii("T"));
        s.extend([0.0f32, 1.0, 2.0, 3.0].iter().flat_map(|f| f.to_le_bytes()));
        s.push(1);
        s.extend(i32s(&[3]));
        s.extend(ascii("scr"));
        s.extend(i32s(&[100]));
        s.extend([1, 0, 1]);
        s.extend(i32s(&[1]));
        s.extend(0u16.to_le_bytes());
        s.extend(chunk(7, 1, &[9]));
        body.extend(chunk(6, 3, &s));

        let m = MapFile::parse(&file(NAMES, &body)).unwrap();
        assert_eq!(m.chunk_names().collect::<Vec<_>>(), ["ObjectsList", "Foo", "WorldInfo", "WaypointsList", "SidesList"]);
        assert_eq!(m.objects.len(), 2);
        assert_eq!(m.objects[0].pos, [1.0, 2.0, 9.0]);
        assert_eq!((m.objects[0].flags, m.objects[0].template.as_str()), (6, "Tree"));
        assert_eq!(m.objects[0].props.int("k"), Some(42));
        assert_eq!(m.objects[1].pos[2], 0.0);
        assert!(m.objects[1].props.is_empty());
        assert_eq!(m.world_info.unwrap().get("k"), Some(&DictValue::Ascii("x".into())));
        assert_eq!(m.waypoint_links, [(1, 2), (2, 3)]);
        let s = m.sides.unwrap();
        assert_eq!((s.sides.len(), s.teams.len(), s.nested.len()), (1, 1, 1));
        let e = &s.sides[0].build_list[0];
        assert_eq!((e.building_name.as_str(), e.health, e.whiner, e.unsellable, e.repairable), ("b", 100, true, false, true));
    }

    #[test]
    fn blend_tile_data() {
        // 4x4 height map (v2, no border) + BlendTileData v8 with 1 blend tile and 1 cliff entry.
        let names: &[(&str, u32)] = &[("HeightMapData", 1), ("BlendTileData", 9)];
        let mut b = i32s(&[16]);
        for fill in [3i16, 1, 2, 4] {
            // tiles, blend, extra, cliff
            b.extend((0..16).flat_map(|_| fill.to_le_bytes()));
        }
        b.extend([0b0101u8; 4]); // cliff bits: stride 1 byte * 4 rows
        b.extend(i32s(&[8, 2, 2])); // bitmap tiles, blended tiles, cliff infos
        b.extend(i32s(&[1])); // one texture class
        b.extend(i32s(&[0, 4, 2, 0]));
        b.extend(ascii("Snow"));
        b.extend(i32s(&[0, 0])); // edge tiles, edge classes
        b.extend(i32s(&[7])); // blend 1: blendNdx
        b.extend([1, 0, 0, 1, 3, 1]); // horiz, vert, rdiag, ldiag, inverted, long
        b.extend(i32s(&[-1, 0x7ADA0000]));
        b.extend(i32s(&[5])); // cliff 1: tile index
        b.extend((0..8).flat_map(|i| (i as f32).to_le_bytes()));
        b.extend([1, 0]);
        let mut body = hm(2, 4, 4, false);
        body.extend(chunk(9, 8, &b));
        let m = MapFile::parse(&file(names, &body)).unwrap();
        let t = m.blend_tiles.unwrap();
        assert_eq!((t.version, t.tile_ndxes[5], t.blend_tile_ndxes[0], t.extra_blend_tile_ndxes[1], t.cliff_info_ndxes[2]), (8, 3, 1, 2, 4));
        assert_eq!(t.cell_cliff_state, vec![5; 4]);
        assert_eq!(t.texture_classes[0], TextureClass { first_tile: 0, num_tiles: 4, width: 2, name: "Snow".into() });
        assert_eq!(t.blended_tiles.len(), 2);
        let bl = t.blended_tiles[1];
        assert_eq!((bl.blend_ndx, bl.horiz, bl.left_diagonal, bl.inverted, bl.long_diagonal, bl.custom_blend_edge_class), (7, true, true, 3, true, -1));
        assert_eq!((t.cliff_info[1].tile_index, t.cliff_info[1].uv[3], t.cliff_info[1].flip, t.cliff_info[1].mutant), (5, [6.0, 7.0], true, false));
        // every truncation fails cleanly
        let full = file(names, &body);
        for n in 0..full.len() {
            let _ = MapFile::parse(&full[..n]);
        }
        // wrong size / bad sentinel
        let mut bad = body.clone();
        let at = bad.len() - 38 - 4;
        bad[at] ^= 0xff;
        assert!(MapFile::parse(&file(names, &bad)).is_err());
    }

    #[test]
    fn global_lighting() {
        let names: &[(&str, u32)] = &[("GlobalLighting", 1)];
        // v3: tod, then per tod: terrain[0], objects[0], objects[1..3], terrain[1..3]; trailing shadow colour.
        let mut d = i32s(&[2]);
        for tod in 0..4 {
            for light in 0..6 {
                let v = (tod * 10 + light) as f32;
                d.extend([v, v + 0.1, v + 0.2, 1.0, 1.0, 1.0, 0.0, 0.0, -1.0].iter().flat_map(|f: &f32| f.to_le_bytes()));
            }
        }
        d.extend(0xffa8a0e0u32.to_le_bytes());
        let m = MapFile::parse(&file(names, &chunk(1, 3, &d))).unwrap();
        let l = m.lighting.unwrap();
        assert_eq!((l.time_of_day, l.tod_index(), l.shadow_color), (2, 1, Some(0xffa8a0e0)));
        assert_eq!(l.terrain[1][0].ambient[0], 10.0); // tod 1, light 0 = terrain[0]
        assert_eq!(l.objects[1][0].ambient[0], 11.0);
        assert_eq!(l.objects[1][2].ambient[0], 13.0);
        assert_eq!(l.terrain[1][1].ambient[0], 14.0);
        assert_eq!(l.terrain[3][2].ambient[0], 35.0);
        // truncated chunk is dropped, not fatal
        let m = MapFile::parse(&file(names, &chunk(1, 3, &d[..40]))).unwrap();
        assert!(m.lighting.is_none());
    }

    #[test]
    fn truncated_never_panics() {
        let mut body = chunk(2, 1, &obj(3, 0.0, "Tree", true));
        body.extend(hm(4, 6, 5, true));
        body.extend(chunk(5, 1, &i32s(&[2, 1, 2, 2, 3])));
        let full = file(NAMES, &body);
        for n in 0..full.len() {
            let _ = MapFile::parse(&full[..n]);
        }
        // hostile counts
        let _ = MapFile::parse(&file(NAMES, &chunk(5, 1, &i32s(&[i32::MAX]))));
        let _ = MapFile::parse(&file(NAMES, &chunk(6, 3, &i32s(&[i32::MAX]))));
    }
}
