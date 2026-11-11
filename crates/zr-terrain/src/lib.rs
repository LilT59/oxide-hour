//! Terrain mesh builder: heightfield + the engine's class-packed terrain texture atlas.
//!
//! The original renders each map cell with up to three alpha-blended layers (base tile, blend
//! tile, extra blend tile), each a 32x32-texel quadrant of a 64x64 source tile. Base and blend
//! layer are two passes over the same class-packed atlas with different texture coordinates and
//! per-vertex alpha (`HeightMap.cpp` `updateVB` stores `U/V` and the alpha pass' `UA/VA` +
//! `alpha[4]` in the vertex; `W3DShaderManager.cpp` `TerrainShader2Stage::set` draws pass 0
//! opaque and pass 1 `SRCALPHA`/`INVSRCALPHA`). The extra (3-way) layer is the engine's separate
//! `renderExtraBlendTiles` pass (`HeightMap.cpp:2190-2397`), which runs inside the terrain render,
//! before roads and props; the port composites that cell's three layers into one block of the
//! same atlas and draws its quad opaque with the terrain (a Bevy transparent-phase overlay would
//! paint over the props the engine draws after the terrain).
//!
//! **The atlas is per texture class, as the engine's is.** `WorldHeightMap::updateTileTexturePositions`
//! (`:1429-1568`) packs every texture class once into a `TEXTURE_WIDTH`-wide (2048, `TileData.h:58`)
//! grid of `TILE_PIXEL_EXTENT + TILE_OFFSET` (64 + 8) slots and gives each class' tiles a
//! `positionInTexture`; `getUVForNdx` (`:1573-1618`) then samples a sub-rectangle (the cell's
//! quadrant of its 64x64 tile) of that class block, so cells of one class filter continuously
//! into each other's tiles and wrap inside the class. `TerrainTextureClass::update`
//! (`TerrainTex.cpp:96-203`) fills the tiles and copies the class block's last 4 columns before it
//! and its first 4 after it (and the same 4 rows above/below, `:155-193`): "The tiles are placed
//! with 4 pixel borders around them, so that when the tiles are scaled and bilinearly
//! interpolated, you don't get seams between the tiles." `TILE_OFFSET` is 8, i.e. 4 texels per
//! side (`TerrainTex.h:38`), and the atlas has 3 mip levels only (`MIP_LEVELS_3`,
//! `TerrainTex.cpp:70-74`) box-filtered by `D3DXFilterTexture` (`:198`), so the border covers the
//! coarsest filter footprint. The atlas height is the next power of two of the packed height
//! (`WorldHeightMap::getTerrainTexture`, `:2118-2132`); the extra-blend cells' blocks follow the
//! class blocks inside it. Blend and extra blend tiles are source tiles of the same classes, so
//! their UVs come from the same atlas (`getAlphaUVData`, `:2020-2110`; `getExtraAlphaUVData`,
//! `:1936-2012`).
//!
//! Sources (under `references/GeneralsGameCode/Core/GameEngineDevice/Source/W3DDevice/GameClient/`):
//! `WorldHeightMap.cpp` (`updateTileTexturePositions`, `getUVForNdx`, `getUVForTileIndex`,
//! `getAlphaUVData`, `getExtraAlphaUVData`, `readTexClass`, `readTiles`), `HeightMap.cpp`
//! (`updateVB`, index buffer, `renderExtraBlendTiles`), `BaseHeightMap.cpp` (`doTheLight`) and
//! `TerrainTex.cpp` (class packing, tile copy, borders).

mod tga;

use anyhow::{bail, Context, Result};
use zr_assets::{RgbaImage, Vfs};
use zr_ini::IniDb;
use zr_map::{BlendTileData, HeightMap, MapFile};

/// World units per height cell (`MAP_XY_FACTOR`, `MapObject.h`).
pub const MAP_XY_FACTOR: f32 = 10.0;
/// World units per height byte (`MAP_HEIGHT_SCALE = MAP_XY_FACTOR / 16`).
pub const MAP_HEIGHT_SCALE: f32 = MAP_XY_FACTOR / 16.0;

/// `TILE_PIXEL_EXTENT` (`TileData.h:48`): a source tile is 64x64 texels, a cell one quadrant.
const TILE_PX: usize = 64;
/// `TEXTURE_WIDTH` (`TileData.h:58`): the terrain texture is 2048 texels wide.
const TEXTURE_WIDTH: usize = 2048;
/// `TILE_OFFSET` (`TerrainTex.h:38`): the extra room a class block gets in the packing grid.
const TILE_OFFSET: usize = 8;
/// `TILE_OFFSET / 2`: texels copied around every class block (`TerrainTex.cpp:162-191`).
const BORDER: usize = TILE_OFFSET / 2;
/// Class blocks are packed on a grid of `TILE_PIXEL_EXTENT + TILE_OFFSET` texels.
const SLOT_PX: usize = TILE_PX + TILE_OFFSET;
/// `TEXTURE_WIDTH / (TILE_PIXEL_EXTENT + TILE_OFFSET)`: class slots per atlas row
/// (`WorldHeightMap::updateTileTexturePositions`, `WorldHeightMap.cpp:1433`).
const TILES_PER_ROW: usize = TEXTURE_WIDTH / SLOT_PX;
/// `NUM_SOURCE_TILES` (`WorldHeightMap.h`), the source-tile index space.
const NUM_SOURCE_TILES: usize = 1024;
const INVERTED_MASK: u8 = 0x1;
const FLIPPED_MASK: u8 = 0x2;

/// Terrain triangle mesh and the texture atlas its layers sample.
#[derive(Debug, Clone)]
pub struct TerrainMesh {
    /// Generals world space: X east, Y north, Z up, world units (cell = 10.0).
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Base-layer UVs (`getUVData` -> `getUVForTileIndex`), corner order SW, SE, NE, NW; into
    /// `atlas`, where `(0, 0)` is the top-left of `atlas.pixels` (image row 0).
    pub uvs: Vec<[f32; 2]>,
    /// Blend-layer UVs (`getAlphaUVData` -> `getUVForTileIndex` on the blend tile), same order.
    /// With `blend_alpha` this is the engine's vertex `u2,v2`.
    pub blend_uvs: Vec<[f32; 2]>,
    /// Per-vertex blend alpha (`alpha[4]` of `getAlphaUVData`, the vertex diffuse alpha).
    pub blend_alpha: Vec<u8>,
    /// Triangle list, counter-clockwise when viewed from +Z.
    pub indices: Vec<u32>,
    /// The 3-way blend pass (`HeightMap.cpp` `renderExtraBlendTiles`), if any cell uses one.
    pub extra: Option<ExtraBlend>,
    pub atlas: RgbaImage,
    /// Border cells on each side of the playable area. The playable area is
    /// `playable_min..playable_max` in world units; the border extends to negative coordinates.
    pub border_size: i32,
    pub playable_min: [f32; 2],
    pub playable_max: [f32; 2],
    /// Height-grid size in samples (cells are `grid.0 - 1` by `grid.1 - 1`).
    pub grid: (u32, u32),
    pub stats: TerrainStats,
}

/// The extra (third) blend layer's quads: `HeightMapRenderObjClass::renderExtraBlendTiles`
/// (`HeightMap.cpp:2190-2397`) draws these after the two-pass terrain, one quad per cell whose
/// `getExtraAlphaUVData` returns a layer, with its own triangulation flip. The engine's pass is
/// part of the terrain render, before roads and props; the port composites the three layers per
/// such cell (the pass' atlas area holds those blocks, see [`build_from_parts`]) and draws the
/// quads opaque with the terrain.
#[derive(Debug, Clone)]
pub struct ExtraBlend {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// UVs into [`TerrainMesh::atlas`]'s extra-block area (the composited cell).
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Build statistics.
#[derive(Debug, Clone, Default)]
pub struct TerrainStats {
    pub cells: usize,
    /// Cells whose base blend layer is present (`getAlphaUVData` returned a tile).
    pub blended_cells: usize,
    pub extra_blended_cells: usize,
    /// Texture classes whose image could not be loaded.
    pub missing_classes: Vec<String>,
    /// Texture classes in the map's blend data (`m_numTextureClasses`).
    pub texture_classes: usize,
    /// Classes the engine's packing grid had no room for (they keep position (0,0) and their
    /// cells sample the atlas corner, as `updateTileTexturePositions` leaves them).
    pub unplaced_classes: usize,
    /// Atlas height in texels (the width is always `TEXTURE_WIDTH`).
    pub atlas_height: usize,
}

/// Builds the terrain mesh of `map`. Texture names resolve through the `Terrain` INI blocks and
/// `art\terrain\` in `vfs`, like `WorldHeightMap::readTexClass`.
pub fn build_terrain(map: &MapFile, vfs: &Vfs, ini: &IniDb) -> Result<TerrainMesh> {
    let hm = map.height_map.as_ref().context("map has no HeightMapData")?;
    let bt = map.blend_tiles.as_ref().context("map has no BlendTileData")?;
    let mut classes = Vec::new();
    for tc in &bt.texture_classes {
        classes.push(load_class(tc, vfs, ini));
    }
    let gd = ini.resolved("GameData", "");
    let flag = |key: &str, default: i32| -> i32 {
        gd.as_ref().and_then(|b| b.value(key)).map_or(default, |v| match v.to_ascii_lowercase().as_str() {
            "yes" | "true" => 1,
            "no" | "false" => 0,
            s => s.parse().unwrap_or(default),
        })
    };
    let opts = Options { adjust_cliff_textures: flag("AdjustCliffTextures", 0) != 0, use_3way_blends: flag("Use3WayTerrainBlends", 1) != 0 };
    build_from_parts(hm, bt, classes, opts)
}

#[derive(Debug, Clone, Copy)]
struct Options {
    /// `TheGlobalData->m_adjustCliffTextures` (GameData.ini `AdjustCliffTextures`).
    adjust_cliff_textures: bool,
    /// `m_use3WayTerrainBlends != 0`.
    use_3way_blends: bool,
}

/// A decoded texture class: `read_width * read_width` tiles stored as one top-down image, packed
/// as a `width * width` block.
struct Class {
    name: String,
    first_tile: i32,
    /// `m_textureClasses[i].numTiles`: the source tiles that belong to the class.
    num_tiles: i32,
    /// Class edge in tiles used for packing (`m_textureClasses[i].width`), 0 when not loaded.
    width: usize,
    /// Tiles read from the image: `readTexClass`'s width, the largest k with k*k <= numTiles.
    read_width: usize,
    /// Top-down `read_width*64` square RGBA image (tile (i, j) at x = i*64, y = (read_width-1-j)*64).
    image: Option<Vec<[u8; 4]>>,
    /// `positionInTexture` in atlas texels; (0, 0) when the class was not placed.
    position: (i32, i32),
    placed: bool,
}

fn load_class(tc: &zr_map::TextureClass, vfs: &Vfs, ini: &IniDb) -> Class {
    let mut c = Class {
        name: tc.name.clone(),
        first_tile: tc.first_tile,
        num_tiles: tc.num_tiles,
        width: usize::try_from(tc.width).unwrap_or(0).min(TILES_PER_ROW),
        read_width: 0,
        image: None,
        position: (0, 0),
        placed: false,
    };
    // TheTerrainTypes->findTerrain(name)->getTexture(), under TERRAIN_TGA_DIR_PATH.
    let Some(texture) = ini.get("Terrain", &tc.name).and_then(|b| b.value("Texture")) else { return c };
    let path = format!("art\\terrain\\{texture}");
    let Some(bytes) = vfs.read(&path) else { return c };
    let Some(img) = tga::decode(&bytes) else { return c };
    // `countTiles`: largest k <= 10 with k*k tiles inside the image; must cover numTiles
    // (`readTexClass`, `WorldHeightMap.cpp:983-1010`).
    let (tw, th) = (img.width / TILE_PX, img.height / TILE_PX);
    let have = (1..=10usize).rev().find(|&k| tw >= k && th >= k).map_or(0, |k| k * k);
    let want = usize::try_from(tc.num_tiles).unwrap_or(0);
    if have < want || want == 0 {
        return c;
    }
    // `readTexClass` derives the tiles-to-read width from numTiles, independent of the packing
    // width the map stored.
    let w = (1..=10usize).rev().find(|&k| want >= k * k).unwrap_or(0);
    let size = w * TILE_PX;
    let mut px = vec![[0u8; 4]; size * size];
    for y in 0..size {
        // file row = size-1-y (the engine's per-tile row inversion); rows past the image are zero.
        let fr = size - 1 - y;
        if fr >= img.height {
            continue;
        }
        for x in 0..size.min(img.width) {
            px[y * size + x] = img.pixels[fr * img.width + x];
        }
    }
    c.read_width = w;
    c.image = Some(px);
    c
}

#[derive(Clone, Copy)]
struct UvResult {
    u: [f32; 4],
    v: [f32; 4],
    /// Return value of `getUVForTileIndex`: custom cliff UVs, so the cell still checks its corner
    /// heights for the triangulation flip (`getAlphaUVData`, `:2097-2106`).
    stretched: bool,
    /// The class block the UV rect lies in (the extra pass' per-cell composite samples it).
    class: Option<usize>,
}

/// The vertex data of one cell: the engine's `U/V`, `UA/VA`, `alpha[4]` and blend flip.
#[derive(Clone)]
struct CellUvs {
    base: UvResult,
    /// Blend layer (`getAlphaUVData`); `None` when the cell has no blend tile.
    blend: Option<UvResult>,
    blend_alpha: [u8; 4],
    /// The cell's triangulation flip (`FLIP_TRIANGLES` in `updateVB`, `HeightMap.cpp:470-480`).
    flip: bool,
    /// Third layer and its own flip (`getExtraAlphaUVData` + `renderExtraBlendTiles`).
    extra: Option<(UvResult, [u8; 4], bool)>,
}

struct Ctx<'a> {
    hm: &'a HeightMap,
    bt: &'a BlendTileData,
    classes: &'a [Class],
    /// base source tile index -> (class, atlas x, atlas y) of the tile's `m_tileLocationInTexture`
    tiles: Vec<Option<(usize, i32, i32)>>,
    opts: Options,
    w: usize,
    /// `m_terrainTexHeight` (the pow2 atlas height) for the engine's V normalisation.
    atlas_height: usize,
}

impl<'a> Ctx<'a> {
    fn new(hm: &'a HeightMap, bt: &'a BlendTileData, classes: &'a [Class], opts: Options, atlas_height: usize) -> Self {
        // `updateTileTexturePositions` resets every source tile's location to (0,0) first, so a
        // class that did not fit the grid keeps it and its cells sample the atlas corner.
        let mut tiles = vec![None; NUM_SOURCE_TILES];
        for (ci, c) in classes.iter().enumerate() {
            if c.image.is_none() {
                continue;
            }
            let pack = if c.width == 0 { c.read_width.max(1) } else { c.width };
            for i in 0..pack {
                for j in 0..pack {
                    let k = i + j * pack;
                    // `readTiles` filled firstTile..firstTile+read_width^2 only.
                    if k >= c.read_width * c.read_width {
                        continue;
                    }
                    let Some(slot) = usize::try_from(c.first_tile as i64 + k as i64).ok().and_then(|i| tiles.get_mut(i)) else {
                        continue;
                    };
                    let loc = if c.placed {
                        (c.position.0 + (i * TILE_PX) as i32, c.position.1 + ((pack - 1 - j) * TILE_PX) as i32)
                    } else {
                        (0, 0)
                    };
                    *slot = Some((ci, loc.0, loc.1));
                }
            }
        }
        Self { hm, bt, classes, tiles, opts, w: hm.width as usize, atlas_height: atlas_height.max(1) }
    }

    fn height(&self, i: usize) -> i32 {
        i32::from(self.hm.data.get(i).copied().unwrap_or(0))
    }

    /// `getUVForNdx` (`WorldHeightMap.cpp:1573-1618`): UV rectangle `(minU, minV, maxU, maxV)`
    /// of tile index `tile_ndx`'s 32x32 quadrant; all zeros when the tile is missing.
    fn uv_for_ndx(&self, tile_ndx: i32) -> ([f32; 4], Option<usize>) {
        let loc = usize::try_from(tile_ndx >> 2).ok().and_then(|b| self.tiles.get(b).copied().flatten());
        let Some((ci, x, y)) = loc else { return ([0.0; 4], None) };
        let (tw, th) = (TEXTURE_WIDTH as f32, self.atlas_height as f32);
        let (mut min_u, mut min_v) = (x as f32 / tw, y as f32 / th);
        let (mut max_u, mut max_v) = ((x as f32 + TILE_PX as f32) / tw, (y as f32 + TILE_PX as f32) / th);
        // Tiles are 64x64 pixels, height grids map to 32x32, so take the proper quadrant.
        let (mid_x, mid_y) = ((min_u + max_u) / 2.0, (min_v + max_v) / 2.0);
        if tile_ndx & 2 != 0 {
            max_v = mid_y;
        } else {
            min_v = mid_y;
        }
        if tile_ndx & 1 != 0 {
            min_u = mid_x;
        } else {
            max_u = mid_x;
        }
        ([min_u, min_v, max_u, max_v], Some(ci))
    }

    /// Class containing base tile `base` by `[firstTile, firstTile+numTiles)` (as the engine searches).
    fn class_of_tile(&self, base: i32) -> Option<usize> {
        let ci = self.tiles.get(usize::try_from(base).ok()?).copied().flatten()?.0;
        Some(ci)
    }

    /// `m_textureClasses[i]`'s region: `(nU, nV, xU, xV)` in atlas UV space.
    fn class_bounds(&self, ci: usize) -> [f32; 4] {
        let c = &self.classes[ci];
        let (tw, th) = (TEXTURE_WIDTH as f32, self.atlas_height as f32);
        let s = (c.width * TILE_PX) as f32;
        [c.position.0 as f32 / tw, c.position.1 as f32 / th, (c.position.0 as f32 + s) / tw, (c.position.1 as f32 + s) / th]
    }

    /// Port of `WorldHeightMap::getUVForTileIndex` (with `DO_OLD_UV`).
    fn uv_for_tile_index(&self, ndx: usize, tile_ndx: i32) -> UvResult {
        let (r, class) = self.uv_for_ndx(tile_ndx);
        let (n_u, n_v, x_u, x_v) = (r[0], r[1], r[2], r[3]);
        let mut u = [n_u, x_u, x_u, n_u];
        let mut v = [x_v, x_v, n_v, n_v];
        let plain = |u, v| UvResult { u, v, stretched: false, class };
        if !self.opts.adjust_cliff_textures || n_u == 0.0 {
            return plain(u, v);
        }
        // Custom cliff UVs from the map.
        let cliff = self.bt.cliff_info_ndxes.get(ndx).copied().unwrap_or(0);
        if cliff != 0 {
            if let Some(info) = self.bt.cliff_info.get(cliff as usize) {
                let (ndx1, ndx2) = (tile_ndx >> 2, info.tile_index >> 2);
                if let Some(ci) = self.class_of_tile(ndx1) {
                    let c = &self.classes[ci];
                    let in_class = |t: i32| t >= c.first_tile && (t as i64) < c.first_tile as i64 + i64::from(c.num_tiles);
                    if in_class(ndx2) {
                        let b = self.class_bounds(ci);
                        // `vFactor = TEXTURE_WIDTH / m_terrainTexHeight` and the class block's
                        // bottom edge (`WorldHeightMap.cpp:1714-1728`).
                        let v_factor = TEXTURE_WIDTH as f32 / self.atlas_height as f32;
                        for k in 0..4 {
                            u[k] = info.uv[k][0] + b[0];
                            v[k] = info.uv[k][1] * v_factor + b[3];
                        }
                        return UvResult { u, v, stretched: info.flip, class };
                    }
                }
            }
        }
        // Old stretch adjustment for steep cells.
        const STRETCH_LIMIT: f32 = 1.5;
        const TILE_LIMIT: f32 = 4.0;
        const TALL_STRETCH_LIMIT: f32 = 2.0;
        const DIAMOND_STRETCH_LIMIT: f32 = 2.4;
        const HEIGHT_SCALE: f32 = MAP_HEIGHT_SCALE / MAP_XY_FACTOR;
        let w = self.w;
        if ndx + w + 1 >= self.hm.data.len() {
            return plain(u, v);
        }
        let (h0, h1, h2, h3) = (self.height(ndx), self.height(ndx + 1), self.height(ndx + w + 1), self.height(ndx + w));
        let min_h = h0.min(h1).min(h2).min(h3);
        let max_h = h0.max(h1).max(h2).max(h3);
        let delta_h = max_h - min_h;
        let below_limit = min_h + (2 * delta_h + 1) / 3;
        let above_limit = min_h + (delta_h + 1) / 3;
        let hs = [h0, h1, h2, h3];
        let below = hs.iter().filter(|&&h| h < below_limit).count();
        let above = hs.iter().filter(|&&h| h > above_limit).count();
        let dh = delta_h as f32 * HEIGHT_SCALE;
        if dh < STRETCH_LIMIT {
            return plain(u, v);
        }
        let Some(ci) = self.class_of_tile(tile_ndx >> 2) else { return plain(u, v) };
        let [_, n_vb, x_ub, x_vb] = self.class_bounds(ci);
        let divisor = (TILE_LIMIT / dh).clamp(1.0, TILE_LIMIT);
        let delta_v = x_vb - n_vb;
        if above != 1 && below != 1 && (above != 2 || below != 2) && dh < DIAMOND_STRETCH_LIMIT {
            return plain(u, v);
        }
        if below == 1 || above > below {
            // one low vertex
            if h0 == min_h {
                v[0] = n_v + delta_v / divisor;
            } else if h1 == min_h {
                v[1] = n_v + delta_v / divisor;
            } else if h2 == min_h {
                v[2] = x_v - delta_v / divisor;
            } else if h3 == min_h {
                v[3] = x_v - delta_v / divisor;
            }
        } else if above == 1 || below > above {
            // one high vertex
            if h0 == max_h {
                v[0] = n_v + delta_v / divisor;
            } else if h1 == max_h {
                v[1] = n_v + delta_v / divisor;
            } else if h2 == max_h {
                v[2] = x_v - delta_v / divisor;
            } else if h3 == max_h {
                v[3] = x_v - delta_v / divisor;
            }
        } else {
            // two up, two down
            if dh < TALL_STRETCH_LIMIT {
                return plain(u, v);
            }
            let len = |d: i32| {
                let d = d as f32 * HEIGHT_SCALE;
                (1.0 + d * d).sqrt()
            };
            let fix = |l: f32| {
                let l = if l < STRETCH_LIMIT { 1.0 } else { l };
                l.min(TILE_LIMIT)
            };
            let dx = fix(len(h3 - h2)) * (x_u - n_u);
            let dy = fix(len(h3 - h0)) * (x_v - n_v);
            u = [n_u, n_u + dx, n_u + dx, n_u];
            v = [n_v + dy, n_v + dy, n_v, n_v];
            let dx = fix(len(h1 - h0)) * (x_u - n_u);
            let dy = fix(len(h2 - h1)) * (x_v - n_v);
            u[1] = u[0] + dx;
            v[1] = v[3] + dy;
        }
        // Keep within the class texture.
        let mut adj_v = 0.0f32;
        for &vv in &v {
            adj_v = adj_v.max(n_vb - vv);
        }
        for vv in &mut v {
            *vv += adj_v;
        }
        let (mut adj_u, mut adj_v) = (0.0f32, 0.0f32);
        for k in 0..4 {
            adj_u = adj_u.max(u[k] - x_ub);
            adj_v = adj_v.max(v[k] - x_vb);
        }
        for k in 0..4 {
            u[k] -= adj_u;
            v[k] -= adj_v;
        }
        UvResult { u, v, stretched: true, class }
    }

    /// Per-corner alpha and "needs flip" for a blend tile record (shared by both blend layers).
    fn alpha_pattern(&self, blend_idx: usize) -> ([u8; 4], bool) {
        let b = &self.bt.blended_tiles[blend_idx];
        let inverted = b.inverted & INVERTED_MASK != 0;
        let forced = b.inverted & FLIPPED_MASK != 0;
        let mut a = [0u8; 4];
        let mut flip = false;
        if b.horiz {
            flip = forced;
            if inverted {
                a[0] = 255;
                a[3] = 255;
            } else {
                a[1] = 255;
                a[2] = 255;
            }
        }
        if b.vert {
            flip = forced;
            if inverted {
                a[0] = 255;
                a[1] = 255;
            } else {
                a[2] = 255;
                a[3] = 255;
            }
        }
        if b.right_diagonal {
            if inverted {
                a[1] = 255;
                if b.long_diagonal {
                    a[0] = 255;
                    a[2] = 255;
                }
            } else {
                flip = true;
                a[2] = 255;
                if b.long_diagonal {
                    a[1] = 255;
                    a[3] = 255;
                }
            }
        }
        if b.left_diagonal {
            if inverted {
                flip = true;
                a[0] = 255;
                if b.long_diagonal {
                    a[1] = 255;
                    a[3] = 255;
                }
            } else {
                a[3] = 255;
                if b.long_diagonal {
                    a[0] = 255;
                    a[2] = 255;
                }
            }
        }
        if b.custom_blend_edge_class >= 0 {
            // The shipped renderer draws custom edge blends with zero vertex alpha (no-op).
            a = [0; 4];
            flip = false;
        }
        (a, flip)
    }

    /// The cell's vertex data: `getAlphaUVData` (`WorldHeightMap.cpp:2020-2110`) for the blend
    /// layer and flip, `getExtraAlphaUVData` (`:1936-2012`) plus `renderExtraBlendTiles`'
    /// height check (`HeightMap.cpp:2242-2253`) for the third layer.
    fn cell(&self, ndx: usize) -> CellUvs {
        let bt = self.bt;
        let nb = bt.blended_tiles.len();
        let tile = i32::from(bt.tile_ndxes[ndx]);
        let base = self.uv_for_tile_index(ndx, tile);

        // getAlphaUVData.
        let blend_idx = (bt.blend_tile_ndxes[ndx].max(0) as usize).min(nb - 1);
        let (mut need_flip, blend, blend_alpha);
        if blend_idx == 0 {
            // No blend tile: the alpha pass still carries the base tile's UVs, alpha 0.
            need_flip = false;
            blend = None;
            blend_alpha = [0u8; 4];
        } else {
            let info = &bt.blended_tiles[blend_idx];
            let r = self.uv_for_tile_index(ndx, info.blend_ndx);
            let (a, f) = self.alpha_pattern(blend_idx);
            need_flip = f;
            blend = Some(r);
            blend_alpha = a;
        }
        let stretched = if blend_idx == 0 { base.stretched } else { blend.as_ref().is_some_and(|r| r.stretched) };
        if stretched {
            need_flip = self.flip_from_heights(ndx);
        }
        let flip = need_flip;

        // getExtraAlphaUVData (only when 3-way blends are enabled, as `ParseBlendTileData` clears
        // the array otherwise) and the extra pass' own flip.
        let mut extra = None;
        let extra_idx = (bt.extra_blend_tile_ndxes[ndx].max(0) as usize).min(nb - 1);
        if extra_idx != 0 && self.opts.use_3way_blends {
            let info = &bt.blended_tiles[extra_idx];
            let r = self.uv_for_tile_index(ndx, info.blend_ndx);
            let (a, mut f) = self.alpha_pattern(extra_idx);
            // `if (cliffState && abs(p0-p2) > abs(p1-p3)) flipState = TRUE;`
            if r.stretched && self.flip_from_heights(ndx) {
                f = true;
            }
            extra = Some((r, a, f));
        }
        CellUvs { base, blend, blend_alpha, flip, extra }
    }

    /// `abs(p0-p2) > abs(p1-p3)` over the cell's corner heights (`getAlphaUVData`, `:2097-2106`).
    fn flip_from_heights(&self, ndx: usize) -> bool {
        let (p0, p1, p2, p3) = (
            self.height(ndx),
            self.height(ndx + 1),
            self.height(ndx + self.w + 1),
            self.height(ndx + self.w),
        );
        (p0 - p2).abs() > (p1 - p3).abs()
    }

    /// Bilinear sample of a class block in the built atlas at atlas UV `(u, v)`, wrapping inside
    /// the class (the block's 4-texel border is `TerrainTex.cpp:162-191`'s periodic copy).
    fn sample_class(&self, atlas: &RgbaImage, class: Option<usize>, u: f32, v: f32) -> [f32; 4] {
        let Some(ci) = class else { return [0.0; 4] };
        let c = &self.classes[ci];
        if !c.placed || c.width == 0 {
            return [0.0; 4];
        }
        let size = (c.width * TILE_PX) as f32;
        let x = (u * TEXTURE_WIDTH as f32 - c.position.0 as f32).rem_euclid(size);
        let y = (v * self.atlas_height as f32 - c.position.1 as f32).rem_euclid(size);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let at = |xi: f32, yi: f32| -> [f32; 4] {
            let (px, py) = (c.position.0 + (xi as i32).rem_euclid(size as i32), c.position.1 + (yi as i32).rem_euclid(size as i32));
            let p = atlas.pixel(px as u32, py as u32);
            [f32::from(p[0]), f32::from(p[1]), f32::from(p[2]), f32::from(p[3])]
        };
        let (a, b, c2, d) = (at(x0, y0), at(x0 + 1.0, y0), at(x0, y0 + 1.0), at(x0 + 1.0, y0 + 1.0));
        let mut out = [0.0; 4];
        for k in 0..4 {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bot = c2[k] + (d[k] - c2[k]) * fx;
            out[k] = top + (bot - top) * fy;
        }
        out
    }

    /// Composites a cell's up to three layers into an `res + 2*BORDER` block for the extra-blend
    /// pass' opaque replacement quad. The engine draws the same three layers as passes; the block
    /// rows go from the cell's north edge (row 0) to its south edge, and the 4-texel border holds
    /// the same layers sampled just outside the cell.
    fn composite_cell(&self, atlas: &RgbaImage, cell: &CellUvs, res: usize) -> Vec<u8> {
        let pool = res + 2 * BORDER;
        let mut out = vec![255u8; pool * pool * 4];
        let (b, r) = (BORDER as f32, res as f32);
        let mut layers: Vec<(UvResult, [u8; 4], bool)> = vec![(cell.base, [255; 4], cell.flip)];
        if let Some(blend) = cell.blend {
            layers.push((blend, cell.blend_alpha, cell.flip));
        }
        if let Some((uv, alpha, flip)) = cell.extra {
            layers.push((uv, alpha, flip));
        }
        for row in 0..pool {
            let t = 1.0 - (row as f32 + 0.5 - b) / r;
            for col in 0..pool {
                let s = (col as f32 + 0.5 - b) / r;
                let mut px = [0f32; 3];
                for (li, (uv, alpha, flip)) in layers.iter().enumerate() {
                    let wts = corner_weights(s, t, *flip);
                    let (mut u, mut v, mut a) = (0.0, 0.0, 0.0);
                    for (k, wk) in wts.iter().enumerate() {
                        u += wk * uv.u[k];
                        v += wk * uv.v[k];
                        a += wk * f32::from(alpha[k]);
                    }
                    let tex = self.sample_class(atlas, uv.class, u, v);
                    // Linear continuation outside the cell can leave [0, 255]; the layers are
                    // composited with a clamped alpha.
                    let a = a.clamp(0.0, 255.0);
                    let alpha = if li == 0 { 1.0 } else { a / 255.0 * tex[3] / 255.0 };
                    for k in 0..3 {
                        px[k] = px[k] * (1.0 - alpha) + tex[k] * alpha;
                    }
                }
                let o = (row * pool + col) * 4;
                for k in 0..3 {
                    out[o + k] = px[k].round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        out
    }
}

/// Barycentric weights of corners 0..4 `(v0 = SW, v1 = SE, v2 = NE, v3 = NW)` at cell-space
/// `(s, t)` for the triangulation chosen by `flip` (diagonal v0-v2, or v1-v3 when flipped).
fn corner_weights(s: f32, t: f32, flip: bool) -> [f32; 4] {
    if !flip {
        if t <= s {
            [1.0 - s, s - t, t, 0.0]
        } else {
            [1.0 - t, 0.0, s, t - s]
        }
    } else if s + t <= 1.0 {
        [1.0 - s - t, s, 0.0, t]
    } else {
        [0.0, 1.0 - t, s + t - 1.0, 1.0 - s]
    }
}

fn build_from_parts(hm: &HeightMap, bt: &BlendTileData, mut classes: Vec<Class>, opts: Options) -> Result<TerrainMesh> {
    let (w, h) = (hm.width as usize, hm.height as usize);
    if w < 2 || h < 2 || hm.data.len() != w * h {
        bail!("degenerate height map {w}x{h}");
    }
    if bt.width != hm.width || bt.height != hm.height || bt.tile_ndxes.len() != w * h {
        bail!("BlendTileData size does not match the height map");
    }

    // 1. Pack the texture classes (`WorldHeightMap::updateTileTexturePositions`, `:1429-1568`)
    // into a `TEXTURE_WIDTH`-wide atlas whose height is the next power of two (`:2118-2132`).
    let (class_content, class_height, unplaced) = pack_classes(&mut classes, TILES_PER_ROW);

    // 2. The extra-blend cells' composite blocks follow the class blocks, still inside the
    // power-of-two height when they fit (they do for every shipped map we measured). The engine's
    // extra pass is part of the terrain render (`HeightMap.cpp:2076-2078`), before roads and props,
    // so its quads are opaque terrain here instead of an alpha-blended overlay (a Bevy
    // transparent-phase overlay would paint over the ground props the engine draws after it).
    let extra_res = 32usize;
    let extra_slot = extra_res + 2 * BORDER;
    let extra_cols = (TEXTURE_WIDTH / extra_slot).max(1);
    let extra_count = if opts.use_3way_blends && bt.blended_tiles.len() > 1 {
        bt.extra_blend_tile_ndxes.iter().filter(|&&n| n > 0).count()
    } else {
        0
    };
    let extra_height = extra_count.div_ceil(extra_cols) * extra_slot;
    let mut pow2 = 1;
    while pow2 < class_content + extra_height {
        pow2 *= 2;
    }
    let atlas_height = pow2.max(class_height);
    let ctx = Ctx::new(hm, bt, &classes, opts, atlas_height);
    let mut atlas = build_atlas(&classes, atlas_height);

    // 3. Vertex data, as `updateVB` builds it.
    let normals_grid = grid_normals(hm);
    let border = hm.border_size;
    let (cw, ch) = (w - 1, h - 1);
    let mut positions = Vec::with_capacity(cw * ch * 4);
    let mut normals = Vec::with_capacity(cw * ch * 4);
    let mut uvs = Vec::with_capacity(cw * ch * 4);
    let mut blend_uvs = Vec::with_capacity(cw * ch * 4);
    let mut blend_alpha = Vec::with_capacity(cw * ch * 4);
    let mut indices = Vec::with_capacity(cw * ch * 6);
    let mut extra = ExtraBlend { positions: Vec::new(), normals: Vec::new(), uvs: Vec::new(), indices: Vec::new() };
    let mut extra_slot_index = 0usize;
    let mut stats = TerrainStats {
        cells: cw * ch,
        texture_classes: classes.len(),
        unplaced_classes: unplaced,
        atlas_height,
        ..Default::default()
    };
    stats.missing_classes = classes.iter().filter(|c| c.image.is_none()).map(|c| c.name.clone()).collect();
    for cy in 0..ch {
        for cx in 0..cw {
            let ndx = cy * w + cx;
            let cell = ctx.cell(ndx);
            stats.blended_cells += usize::from(cell.blend.is_some());
            stats.extra_blended_cells += usize::from(cell.extra.is_some());
            let base = positions.len() as u32;
            let corners = [(0, 0), (1, 0), (1, 1), (0, 1)];
            for (k, (dx, dy)) in corners.into_iter().enumerate() {
                let (gx, gy) = (cx + dx, cy + dy);
                positions.push([
                    (gx as f32 - border as f32) * MAP_XY_FACTOR,
                    (gy as f32 - border as f32) * MAP_XY_FACTOR,
                    f32::from(hm.data[gy * w + gx]) * MAP_HEIGHT_SCALE,
                ]);
                normals.push(normals_grid[gy * w + gx]);
                uvs.push([cell.base.u[k], cell.base.v[k]]);
                // With no blend tile the alpha pass carries the base tile's UVs and alpha 0
                // (`getAlphaUVData`, `:2031-2034`).
                let b = cell.blend.unwrap_or(cell.base);
                blend_uvs.push([b.u[k], b.v[k]]);
                blend_alpha.push(cell.blend_alpha[k]);
            }
            // HeightMap.cpp index buffer: (0,2,3) (0,1,2); the flipped cell rotates its vertices,
            // which picks the other diagonal (the port swaps the indices instead).
            if cell.flip {
                indices.extend([base, base + 1, base + 3, base + 1, base + 2, base + 3]);
            } else {
                indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
            }

            // `renderExtraBlendTiles' quad: the cell's composited block, drawn opaque.
            if let Some((_, _, flip)) = cell.extra {
                let (ox, oy) = ((extra_slot_index % extra_cols) * extra_slot, class_content + (extra_slot_index / extra_cols) * extra_slot);
                let block = ctx.composite_cell(&atlas, &cell, extra_res);
                for y in 0..extra_slot {
                    for x in 0..extra_slot {
                        let o = (y * extra_slot + x) * 4;
                        atlas.set_pixel((ox + x) as u32, (oy + y) as u32, [block[o], block[o + 1], block[o + 2], 255]);
                    }
                }
                let eb = extra.positions.len() as u32;
                let (x0, y0) = ((ox + BORDER) as f32 / TEXTURE_WIDTH as f32, (oy + BORDER) as f32 / atlas_height as f32);
                let (du, dv) = (extra_res as f32 / TEXTURE_WIDTH as f32, extra_res as f32 / atlas_height as f32);
                // Corner order SW, SE, NE, NW: v grows downward in the atlas image.
                let corner_uvs = [(x0, y0 + dv), (x0 + du, y0 + dv), (x0 + du, y0), (x0, y0)];
                for (k, (dx, dy)) in corners.into_iter().enumerate() {
                    let (gx, gy) = (cx + dx, cy + dy);
                    extra.positions.push([
                        (gx as f32 - border as f32) * MAP_XY_FACTOR,
                        (gy as f32 - border as f32) * MAP_XY_FACTOR,
                        f32::from(hm.data[gy * w + gx]) * MAP_HEIGHT_SCALE,
                    ]);
                    extra.normals.push(normals_grid[gy * w + gx]);
                    extra.uvs.push([corner_uvs[k].0, corner_uvs[k].1]);
                }
                if flip {
                    extra.indices.extend([eb + 1, eb + 3, eb, eb + 1, eb + 2, eb + 3]);
                } else {
                    extra.indices.extend([eb, eb + 2, eb + 3, eb, eb + 1, eb + 2]);
                }
                extra_slot_index += 1;
            }
        }
    }

    let bounds = |n: usize| ((n as i32 - 2 * border).max(0) as f32) * MAP_XY_FACTOR;
    Ok(TerrainMesh {
        positions,
        normals,
        uvs,
        blend_uvs,
        blend_alpha,
        indices,
        extra: (!extra.indices.is_empty()).then_some(extra),
        atlas,
        border_size: border,
        playable_min: [0.0, 0.0],
        playable_max: [bounds(w), bounds(h)],
        grid: (w as u32, h as u32),
        stats,
    })
}

/// `WorldHeightMap::updateTileTexturePositions` (`WorldHeightMap.cpp:1429-1568`) for the normal
/// (not edge) tiles: the largest classes are placed first, each at the first free grid cell of a
/// row that has room for its whole block (the engine tests only that one cell per row), on a
/// `tiles_per_row * tiles_per_row` grid of `TILE_PIXEL_EXTENT + TILE_OFFSET` slots.
/// Returns `(packed content height, pow2 atlas height, classes that did not fit)`
/// (`getTerrainTexture`, `:2118-2132`); unplaced classes keep position (0,0).
fn pack_classes(classes: &mut [Class], tiles_per_row: usize) -> (usize, usize, usize) {
    let mut available = vec![true; tiles_per_row * tiles_per_row];
    let mut max_height = 0usize;
    let mut unplaced = 0;
    for tile_width in (1..=tiles_per_row).rev() {
        for c in classes.iter_mut() {
            if c.width != tile_width || c.image.is_none() || tile_width == 0 {
                continue;
            }
            let mut found = false;
            let (mut at_row, mut at_col) = (0usize, 0usize);
            for row in 0..tiles_per_row - tile_width + 1 {
                for col in 0..tiles_per_row - tile_width + 1 {
                    if !available[row * tiles_per_row + col] {
                        continue;
                    }
                    let mut open = true;
                    for i in 0..tile_width {
                        for j in 0..tile_width {
                            if !available[(row + j) * tiles_per_row + (col + i)] {
                                open = false;
                            }
                        }
                    }
                    if open {
                        found = true;
                        at_row = row;
                        at_col = col;
                    }
                    // The engine's search breaks after the first free cell of the row, whether or
                    // not the block fits there; a class that does not fit moves to the next row.
                    break;
                }
                if found {
                    break;
                }
            }
            if !found {
                c.position = (0, 0);
                c.placed = false;
                unplaced += 1;
                continue;
            }
            let x = BORDER + at_col * SLOT_PX;
            let y = BORDER + at_row * SLOT_PX;
            c.position = (x as i32, y as i32);
            c.placed = true;
            max_height = max_height.max(y + tile_width * TILE_PX + BORDER);
            for i in 0..tile_width {
                for j in 0..tile_width {
                    available[(at_row + j) * tiles_per_row + (at_col + i)] = false;
                }
            }
        }
    }
    let mut pow2 = 1;
    while pow2 < max_height {
        pow2 *= 2;
    }
    // The engine's atlas height (`WorldHeightMap::getTerrainTexture`, `:2118-2132`); the extra
    // blend blocks start at the packed content height, inside that power of two when they fit.
    (max_height.max(1), pow2.max(1), unplaced)
}

/// The terrain texture: each class' tiles pasted at their `positionInTexture`, then
/// `TerrainTextureClass::update`'s 4-texel border copies around every class block
/// (`TerrainTex.cpp:132-193`), in the engine's order (columns first, then rows including the
/// column strips so the corners wrap too). The block is periodic over the class, so the border
/// is the class' own content just outside it.
fn build_atlas(classes: &[Class], atlas_height: usize) -> RgbaImage {
    let mut atlas = RgbaImage::new(TEXTURE_WIDTH as u32, atlas_height as u32);
    let inside = |x: i32, y: i32| -> bool {
        x >= 0 && y >= 0 && x < TEXTURE_WIDTH as i32 && y < atlas_height as i32
    };
    let get = |a: &RgbaImage, x: i32, y: i32| -> [u8; 4] {
        if inside(x, y) { a.pixel(x as u32, y as u32) } else { [0; 4] }
    };
    // Tile copy (`TerrainTex.cpp:132-154`): tile k = i + j*width of the class block is tile k of
    // the read image, placed at (i*64, (width-1-j)*64) - i.e. the top-down class image.
    for c in classes {
        if !c.placed {
            continue;
        }
        let Some(img) = &c.image else { continue };
        let (read, pack) = (c.read_width, c.width);
        for k in 0..read * read {
            let (si, sj) = (k % read, k / read);
            let (di, dj) = (k % pack, k / pack);
            let (sx, sy) = ((si * TILE_PX) as i32, ((read - 1 - sj) * TILE_PX) as i32);
            let (dx, dy) = (c.position.0 + (di * TILE_PX) as i32, c.position.1 + ((pack - 1 - dj) * TILE_PX) as i32);
            for y in 0..TILE_PX {
                for x in 0..TILE_PX {
                    if !inside(dx + x as i32, dy + y as i32) {
                        continue;
                    }
                    let stride = read * TILE_PX;
                    let pixel = img[(sy as usize + y) * stride + sx as usize + x];
                    atlas.set_pixel((dx as usize + x) as u32, (dy as usize + y) as u32, pixel);
                }
            }
        }
    }
    for c in classes {
        if !c.placed {
            continue;
        }
        let (ox, oy) = c.position;
        let width = (c.width * TILE_PX) as i32;
        // Duplicate 4 columns of pixels before and after (`TerrainTex.cpp:162-175`).
        for j in 0..width {
            let row = oy + j;
            for k in 0..BORDER as i32 {
                let dst = ox - BORDER as i32 + k;
                if inside(dst, row) {
                    atlas.set_pixel(dst as u32, row as u32, get(&atlas, ox + width - BORDER as i32 + k, row));
                }
                let dst = ox + width + k;
                if inside(dst, row) {
                    atlas.set_pixel(dst as u32, row as u32, get(&atlas, ox + k, row));
                }
            }
        }
        // Duplicate 4 rows of pixels before and after (`TerrainTex.cpp:177-191`), including the
        // column strips just written (`width + 8` texels starting at `origin.x - 4`).
        for j in 0..BORDER as i32 {
            let (src, dst) = (oy + width - 1 - j, oy - 1 - j);
            let strip: Vec<[u8; 4]> = (0..width + 2 * BORDER as i32).map(|k| get(&atlas, ox - BORDER as i32 + k, src)).collect();
            for (k, pixel) in strip.into_iter().enumerate() {
                let (x, y) = (ox - BORDER as i32 + k as i32, dst);
                if inside(x, y) {
                    atlas.set_pixel(x as u32, y as u32, pixel);
                }
            }
            let (src, dst) = (oy + j, oy + width + j);
            let strip: Vec<[u8; 4]> = (0..width + 2 * BORDER as i32).map(|k| get(&atlas, ox - BORDER as i32 + k, src)).collect();
            for (k, pixel) in strip.into_iter().enumerate() {
                let (x, y) = (ox - BORDER as i32 + k as i32, dst);
                if inside(x, y) {
                    atlas.set_pixel(x as u32, y as u32, pixel);
                }
            }
        }
    }
    atlas
}

/// Smooth per-sample normals (`HeightMapRenderObjClass::updateVB`: central differences over
/// `2 * MAP_XY_FACTOR`, clamped at the edges).
fn grid_normals(hm: &HeightMap) -> Vec<[f32; 3]> {
    let (w, h) = (hm.width as usize, hm.height as usize);
    let z = |x: usize, y: usize| f32::from(hm.data[y * w + x]) * MAP_HEIGHT_SCALE;
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let (xm, xp) = (x.saturating_sub(1), (x + 1).min(w - 1));
            let (ym, yp) = (y.saturating_sub(1), (y + 1).min(h - 1));
            let l2r = [2.0 * MAP_XY_FACTOR, 0.0, z(xp, y) - z(xm, y)];
            let n2f = [0.0, 2.0 * MAP_XY_FACTOR, z(x, yp) - z(x, ym)];
            let n = [
                l2r[1] * n2f[2] - l2r[2] * n2f[1],
                l2r[2] * n2f[0] - l2r[0] * n2f[2],
                l2r[0] * n2f[1] - l2r[1] * n2f[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
            out.push([n[0] / len, n[1] / len, n[2] / len]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use zr_map::{BlendTileInfo, CliffInfo};

    /// A class whose tiles are flat colours; `colors` is indexed `i + j*width` with j counted
    /// from the bottom (file rows), like `load_class`.
    fn solid_class(first: i32, width: usize, colors: &[[u8; 4]]) -> Class {
        let size = width * TILE_PX;
        let mut px = vec![[0u8; 4]; size * size];
        for j in 0..width {
            for i in 0..width {
                let col = colors[i + j * width];
                for y in 0..TILE_PX {
                    for x in 0..TILE_PX {
                        px[((width - 1 - j) * TILE_PX + y) * size + i * TILE_PX + x] = col;
                    }
                }
            }
        }
        Class {
            name: "t".into(),
            first_tile: first,
            num_tiles: (width * width) as i32,
            width,
            read_width: width,
            image: Some(px),
            position: (0, 0),
            placed: false,
        }
    }

    /// A class whose single tile is a gradient `[x*4, y*4, 0, 255]` (`width` tiles wide, the
    /// gradient runs over the whole class image).
    fn gradient_class(first: i32, width: usize) -> Class {
        let size = width * TILE_PX;
        let mut image = vec![[0u8; 4]; size * size];
        for y in 0..size {
            for x in 0..size {
                image[y * size + x] = [(x % 64) as u8 * 4, (y % 64) as u8 * 4, ((x / 64) * 100) as u8, 255];
            }
        }
        Class {
            name: "grad".into(),
            first_tile: first,
            num_tiles: (width * width) as i32,
            width,
            read_width: width,
            image: Some(image),
            position: (0, 0),
            placed: false,
        }
    }

    fn map(w: i32, h: i32, heights: Vec<u8>, tiles: Vec<i16>) -> (HeightMap, BlendTileData) {
        let n = (w * h) as usize;
        let hm = HeightMap { version: 4, width: w, height: h, border_size: 1, boundaries: vec![(w - 2, h - 2)], data: heights };
        let bt = BlendTileData {
            version: 8,
            width: w,
            height: h,
            tile_ndxes: tiles,
            blend_tile_ndxes: vec![0; n],
            extra_blend_tile_ndxes: vec![0; n],
            cliff_info_ndxes: vec![0; n],
            cell_cliff_state: vec![],
            num_bitmap_tiles: 4,
            texture_classes: vec![],
            num_edge_tiles: 0,
            edge_texture_classes: vec![],
            blended_tiles: vec![BlendTileInfo { custom_blend_edge_class: -1, ..Default::default() }],
            cliff_info: vec![CliffInfo::default()],
        };
        (hm, bt)
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const OPTS: Options = Options { adjust_cliff_textures: true, use_3way_blends: true };

    /// The cell's triangulation flip, read back from the mesh's index list.
    fn cell_flip(m: &TerrainMesh, cell: usize) -> bool {
        let base = (cell * 4) as u32;
        m.indices[cell * 6 + 1] != base + 1
    }

    fn interpolate(uvs: &[[f32; 2]], s: f32, t: f32) -> (f32, f32) {
        let u = uvs[0][0] + (uvs[1][0] - uvs[0][0]) * s;
        let v = uvs[0][1] + (uvs[3][1] - uvs[0][1]) * t;
        (u, v)
    }

    fn texel(m: &TerrainMesh, u: f32, v: f32) -> [u8; 4] {
        let x = ((u * m.atlas.width as f32).floor() as i64).clamp(0, i64::from(m.atlas.width) - 1) as u32;
        let y = ((v * m.atlas.height as f32).floor() as i64).clamp(0, i64::from(m.atlas.height) - 1) as u32;
        m.atlas.pixel(x, y)
    }

    /// What the terrain shader draws at cell-space `(s, t)`: base * light, then the blend layer
    /// `SRCALPHA`/`INVSRCALPHA` with alpha = blend texel alpha * interpolated vertex alpha
    /// (`W3DShaderManager.cpp` `TerrainShader2Stage::set` pass 1).
    fn shade_cell(m: &TerrainMesh, cell: usize, s: f32, t: f32) -> [u8; 4] {
        let (bu, bv) = interpolate(&m.uvs[cell * 4..cell * 4 + 4], s, t);
        let base = texel(m, bu, bv);
        let flip = cell_flip(m, cell);
        let w = corner_weights(s, t, flip);
        let (eu, ev) = interpolate(&m.blend_uvs[cell * 4..cell * 4 + 4], s, t);
        let blend = texel(m, eu, ev);
        let a: f32 = (0..4).map(|k| w[k] * f32::from(m.blend_alpha[cell * 4 + k])).sum::<f32>() / 255.0;
        let a = a * f32::from(blend[3]) / 255.0;
        let mix = |x: u8, y: u8| (f32::from(x) * (1.0 - a) + f32::from(y) * a).round() as u8;
        [mix(base[0], blend[0]), mix(base[1], blend[1]), mix(base[2], blend[2]), 255]
    }

    #[test]
    fn flat_class_mesh_is_ccw_and_samples_its_block() {
        let classes = vec![solid_class(0, 1, &[RED])];
        let (hm, bt) = map(3, 3, vec![16; 9], vec![0; 9]);
        let m = build_from_parts(&hm, &bt, classes, OPTS).unwrap();
        assert_eq!(m.stats.cells, 4);
        assert_eq!(m.stats.texture_classes, 1);
        // One 64x64 class block at (4,4) plus its 4-texel borders: 72 -> pow2 128.
        assert_eq!(m.stats.atlas_height, 128);
        assert_eq!((m.atlas.width, m.atlas.height), (TEXTURE_WIDTH as u32, 128));
        assert_eq!(m.positions.len(), 16);
        assert_eq!(m.indices.len(), 24);
        assert_eq!(m.positions[0], [-10.0, -10.0, 10.0]); // border 1, height 16 * 0.625
        assert_eq!(shade_cell(&m, 0, 0.5, 0.5), RED);
        for tri in m.indices.chunks(3) {
            let [a, b, c] = [tri[0], tri[1], tri[2]].map(|i| m.positions[i as usize]);
            let cross = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            assert!(cross > 0.0);
        }
        assert!(m.normals.iter().all(|n| (n[2] - 1.0).abs() < 1e-5));
        assert_eq!(m.playable_max, [10.0, 10.0]);
    }

    #[test]
    fn quadrants_and_tile_orientation() {
        // 2x2-tile class: tile index = i + j*2; j counts rows from the bottom of the image.
        let classes = vec![solid_class(0, 2, &[[1, 0, 0, 255], [2, 0, 0, 255], [3, 0, 0, 255], [4, 0, 0, 255]])];
        // cell uses tile 3 (i=1, j=1) quadrant 0
        let (hm, bt) = map(2, 2, vec![0; 4], vec![3 << 2; 4]);
        let m = build_from_parts(&hm, &bt, classes, OPTS).unwrap();
        assert_eq!(shade_cell(&m, 0, 0.5, 0.5), [4, 0, 0, 255]);
    }

    #[test]
    fn classes_are_packed_largest_first_on_the_engine_grid() {
        // Widths in map order: 4, 2, 6, 4, 2, 6, 4, 4. `updateTileTexturePositions` walks
        // tileWidth from 28 down, so the two 6-wide classes go first, then the four 4-wide ones,
        // then the 2-wide ones, each at the first free grid cell of a row. Every class image is a
        // gradient tagged with its index in blue, so the exact block origin is checkable.
        let widths = [4usize, 2, 6, 4, 2, 6, 4, 4];
        let classes: Vec<Class> = widths
            .iter()
            .enumerate()
            .map(|(k, &w)| {
                let mut c = gradient_class(0, w);
                for p in c.image.as_mut().unwrap() {
                    p[2] = (k as u8 + 1) * 16;
                }
                c
            })
            .collect();
        let (hm, bt) = map(2, 2, vec![0; 4], vec![0; 4]);
        let m = build_from_parts(&hm, &bt, classes, OPTS).unwrap();
        // Classes are placed on 28x28 *slots* of 72 texels; a width-6 class reserves six slot rows
        // even though its block is only 384 texels tall, so the 2-wide classes have to go to slot
        // row 4, at the first free columns of that row (12 and 14). `expected[k]` is the position
        // of `classes[k]`, i.e. of map-order class k.
        let expected = [
            (4 + 12 * 72, 4),   // 0: width 4
            (4 + 12 * 72, 292), // 1: width 2 (slot row 4)
            (4, 4),             // 2: width 6
            (4 + 16 * 72, 4),   // 3: width 4
            (4 + 14 * 72, 292), // 4: width 2 (slot row 4)
            (4 + 6 * 72, 4),    // 5: width 6
            (4 + 20 * 72, 4),   // 6: width 4
            (4 + 24 * 72, 4),   // 7: width 4
        ];
        for (k, &(x, y)) in expected.iter().enumerate() {
            // The block starts exactly at `positionInTexture` (no extra offset): its (0, 0) is the
            // gradient's first texel, tagged with the class index.
            assert_eq!(m.atlas.pixel(x as u32, y as u32), [0, 0, (k as u8 + 1) * 16, 255], "class {k} origin");
            assert_eq!(m.atlas.pixel(x as u32 + 5, y as u32 + 5), [20, 20, (k as u8 + 1) * 16, 255], "class {k} block");
        }
        // The tallest block (a 6-wide class at y=4: 4 + 384 + 4 = 392) rounds up to 512.
        assert_eq!(m.stats.atlas_height, 512);
        assert_eq!(m.stats.unplaced_classes, 0);
    }

    #[test]
    fn a_class_that_does_not_fit_keeps_position_zero() {
        // On a 4x4 slot grid, 16 one-tile classes fill it; the 17th has no room.
        let mut classes: Vec<Class> = (0..16).map(|k| solid_class(k * 4, 1, &[[k as u8; 4]])).collect();
        classes.push(solid_class(16 * 4, 1, &[[200; 4]]));
        let (content, height, unplaced) = pack_classes(&mut classes, 4);
        assert_eq!(unplaced, 1);
        assert_eq!(classes[16].position, (0, 0));
        assert!(!classes[16].placed);
        // Four rows of one-tile blocks: 4 + 3*72 + 64 + 4 = 288 -> pow2 512.
        assert_eq!((content, height), (288, 512));
        assert_eq!(classes[15].position, (4 + 3 * 72, 4 + 3 * 72));
    }

    #[test]
    fn class_borders_wrap_the_class_content() {
        // `TerrainTex.cpp:162-191`: the 4 columns before a class block are its last 4 columns, the
        // 4 after its first 4; rows are the same, so the block is periodic over the class.
        let classes = vec![gradient_class(0, 2)];
        let (hm, bt) = map(2, 2, vec![0; 4], vec![0; 4]);
        let m = build_from_parts(&hm, &bt, classes, OPTS).unwrap();
        let px = |x: usize, y: usize| m.atlas.pixel(x as u32, y as u32);
        let size = 2 * TILE_PX; // 128: the class block
        let (ox, oy) = (BORDER, BORDER);
        let img = |x: usize, y: usize| m.atlas.pixel((ox + x) as u32, (oy + y) as u32);
        // Columns: block row 10, x = 0..4 (left of the block) is its x = 124..128.
        for k in 0..BORDER {
            assert_eq!(px(k, oy + 10), img(size - BORDER + k, 10), "left border {k}");
            assert_eq!(px(ox + size + k, oy + 10), img(k, 10), "right border {k}");
        }
        // Rows: the 4 rows above are the block's last 4 rows (bottom-up), the 4 below its first 4.
        for k in 0..BORDER {
            assert_eq!(px(ox + 10, k), img(10, size - BORDER + k), "top border {k}");
            assert_eq!(px(ox + 10, oy + size + k), img(10, k), "bottom border {k}");
        }
        // The corner regions are the wrapped row/column strips too (the engine copies width+8).
        assert_eq!(px(0, 0), img(size - BORDER, size - BORDER), "NW corner");
        assert_eq!(px(ox + size + BORDER - 1, oy + size + BORDER - 1), img(BORDER - 1, BORDER - 1), "SE corner");
        // Atlas height: 4 + 128 + 4 = 136 -> 256.
        assert_eq!(m.stats.atlas_height, 256);
    }

    #[test]
    fn cell_uvs_are_the_tile_quadrant_and_their_footprint_stays_in_the_class_border() {
        // Two adjacent cells of two different classes: each cell's UV rect is the quadrant of its
        // own source tile, normalised by TEXTURE_WIDTH x atlas height, and the 4-texel filter
        // footprint around it never leaves its own class block's border - so a class boundary
        // shows each class up to the geometric edge, as the engine's atlas does.
        let classes = vec![solid_class(0, 2, &[RED; 4]), solid_class(4, 2, &[GREEN; 4])];
        let (hm, bt) = map(3, 2, vec![0; 6], vec![1, 4 << 2, 0, 0, 0, 0]);
        let m = build_from_parts(&hm, &bt, classes, OPTS).unwrap();
        assert_eq!(m.stats.atlas_height, 256); // 4 + 128 + 4 = 136 -> 256
        let positions: [(i32, i32); 2] = [(4, 4), (4 + 2 * 72, 4)]; // class A and class B, both width 2
        let width_px = 2 * TILE_PX;
        // Cell 0 (class A, tile 0 quadrant 1 = east half), cell 1 (class B, tile 4 quadrant 0 =
        // west half). Read back the quad's texel rectangle: tile 0 of a 2-wide class sits at
        // y = 4 + (2-1-0)*64 = 68, and quadrant bit 1 clear takes the bottom half (y 100..132).
        let texels = |cell: usize| {
            let u = &m.uvs[cell * 4..cell * 4 + 4];
            let (x0, x1) = (u[0][0] * TEXTURE_WIDTH as f32, u[1][0] * TEXTURE_WIDTH as f32);
            let (y0, y1) = (u[3][1] * m.atlas.height as f32, u[0][1] * m.atlas.height as f32);
            [x0 as i32, y0 as i32, x1 as i32, y1 as i32]
        };
        assert_eq!(texels(0), [36, 100, 68, 132]); // A at (4,4): east half x 36..68, bottom half y 100..132
        assert_eq!(texels(1), [148, 100, 180, 132]); // B at (148,4): west half x 148..180
        for (cell, (ox, oy)) in [(0, positions[0]), (1, positions[1])] {
            let [x0, y0, x1, y1] = texels(cell);
            let b = BORDER as i32;
            let w = width_px as i32;
            assert!(x0 - b >= ox - b && x1 + b <= ox + w + b, "cell {cell} U footprint {:?}", (x0, x1));
            assert!(y0 - b >= oy - b && y1 + b <= oy + w + b, "cell {cell} V footprint {:?}", (y0, y1));
        }
        // The atlas is 2048 wide and the two class blocks never overlap.
        assert_eq!(shade_cell(&m, 0, 0.5, 0.5), RED);
        assert_eq!(shade_cell(&m, 1, 0.5, 0.5), GREEN);
    }

    #[test]
    fn blend_layer_samples_the_blend_tile_from_the_same_atlas() {
        // Base tile from class A, blend tile from class B; both live in the same atlas and the
        // blend pass samples B's quadrant with the per-vertex alpha (`getAlphaUVData`).
        let classes = vec![solid_class(0, 1, &[RED]), solid_class(1, 1, &[GREEN])];
        let (hm, mut bt) = map(2, 2, vec![0; 4], vec![0; 4]);
        // inverted horizontal: alpha on corners 0 (SW) and 3 (NW), the west side.
        bt.blended_tiles.push(BlendTileInfo { blend_ndx: 1 << 2, horiz: true, inverted: INVERTED_MASK, custom_blend_edge_class: -1, ..Default::default() });
        bt.blend_tile_ndxes = vec![1; 4];
        let m = build_from_parts(&hm, &bt, classes, OPTS).unwrap();
        assert_eq!(m.blend_alpha, vec![255, 0, 0, 255]);
        // Base UVs = class A's tile 0 quadrant 0 at (4,4); blend UVs = class B's tile 1 at (76,4).
        let u = &m.uvs[0..4];
        assert_eq!([u[3][0] * TEXTURE_WIDTH as f32, u[1][0] * TEXTURE_WIDTH as f32], [4.0, 36.0]);
        let b = &m.blend_uvs[0..4];
        assert_eq!([b[3][0] * TEXTURE_WIDTH as f32, b[1][0] * TEXTURE_WIDTH as f32], [76.0, 108.0]);
        let west = shade_cell(&m, 0, 0.1, 0.5);
        let east = shade_cell(&m, 0, 0.9, 0.5);
        assert!(west[1] > 200 && west[0] < 60, "{west:?}");
        assert!(east[0] > 200 && east[1] < 60, "{east:?}");
        assert_eq!(m.stats.blended_cells, 1);
    }

    #[test]
    fn extra_blend_layer_becomes_its_own_opaque_quad_list() {
        // Third layer: `getExtraAlphaUVData` + `renderExtraBlendTiles`; an uninverted right
        // diagonal forces its own flip (`needFlip = TRUE`, `:1977-1979`) and the index order
        // (1,3,0),(1,2,3). The port composites the cell into the atlas' extra-block area and draws
        // it opaque, so the quad's UVs point at that block, not at the extra tile.
        let classes = vec![solid_class(0, 1, &[RED]), solid_class(1, 1, &[GREEN])];
        let (hm, mut bt) = map(2, 2, vec![0; 4], vec![0; 4]);
        bt.blended_tiles.push(BlendTileInfo { blend_ndx: 1 << 2, right_diagonal: true, custom_blend_edge_class: -1, ..Default::default() });
        bt.extra_blend_tile_ndxes = vec![1; 4];
        let m = build_from_parts(&hm, &bt, classes, OPTS).unwrap();
        assert_eq!(m.stats.extra_blended_cells, 1);
        // Class block height 72; one 40-texel extra slot below it -> 112 -> atlas height 128.
        assert_eq!(m.stats.atlas_height, 128);
        let extra = m.extra.as_ref().unwrap();
        assert_eq!((extra.positions.len(), extra.indices.len()), (4, 6));
        assert_eq!(extra.indices, vec![1, 3, 0, 1, 2, 3]);
        // The cell's composite: the extra tile (green) covers the east half, the base (red) the
        // rest; the sample at the cell's east side is a green/red mix.
        let (x0, y0) = (extra.uvs[3][0] * TEXTURE_WIDTH as f32, extra.uvs[3][1] * 128.0);
        assert_eq!((x0, y0), ((BORDER) as f32, (72 + BORDER) as f32));
        let texel = |s: f32, t: f32| {
            let x = x0 + s * 32.0;
            let y = y0 + (1.0 - t) * 32.0;
            m.atlas.pixel(x as u32, y as u32)
        };
        assert_eq!(texel(0.2, 0.5), RED);
        let east = texel(0.9, 0.5);
        assert!(east[1] > 20 && east[0] > 20 && east[1] < 200 && east[0] < 240, "{east:?}");
        assert_eq!(m.atlas.pixel(x0 as u32, y0 as u32), [255, 0, 0, 255], "block core is opaque base");
        // With 3-way blends off the extra array is not stored (as `ParseBlendTileData` clears it).
        let off = Options { use_3way_blends: false, ..OPTS };
        let m2 = build_from_parts(&hm, &bt, vec![solid_class(0, 1, &[RED]), solid_class(1, 1, &[GREEN])], off).unwrap();
        assert!(m2.extra.is_none());
    }

    #[test]
    fn cliff_info_uvs_use_the_atlas_height_factor() {
        // Custom cliff UVs: `U = u + class.x/TEXTURE_WIDTH`, `V = v*(TEXTURE_WIDTH/atlasHeight) +
        // (class.y + classWidth*64)/atlasHeight` (`WorldHeightMap.cpp:1714-1728`).
        let classes = vec![solid_class(0, 1, &[RED])];
        let (hm, mut bt) = map(2, 2, vec![0; 4], vec![0; 4]);
        bt.cliff_info_ndxes = vec![1; 4];
        bt.cliff_info.push(CliffInfo { tile_index: 0, uv: [[0.05, -0.1], [0.1, -0.1], [0.1, -0.02], [0.05, -0.02]], flip: false, mutant: false });
        let m = build_from_parts(&hm, &bt, classes, OPTS).unwrap();
        let h = m.atlas.height as f32; // 128
        let u = &m.uvs[0..4];
        let n_u = BORDER as f32 / TEXTURE_WIDTH as f32;
        let x_v = (BORDER as f32 + TILE_PX as f32) / h;
        for (k, uv) in u.iter().enumerate().take(4) {
            assert!((uv[0] - (bt.cliff_info[1].uv[k][0] + n_u)).abs() < 1e-6, "u[{k}]");
            assert!((uv[1] - (bt.cliff_info[1].uv[k][1] * TEXTURE_WIDTH as f32 / h + x_v)).abs() < 1e-6, "v[{k}]");
        }
    }

    #[test]
    fn custom_edge_blend_is_invisible() {
        let classes = vec![solid_class(0, 2, &[RED, GREEN, RED, RED])];
        let (hm, mut bt) = map(2, 2, vec![0; 4], vec![0; 4]);
        bt.blended_tiles.push(BlendTileInfo { blend_ndx: 1 << 2, horiz: true, custom_blend_edge_class: 0, ..Default::default() });
        bt.blend_tile_ndxes = vec![1; 4];
        let m = build_from_parts(&hm, &bt, classes, OPTS).unwrap();
        assert_eq!(shade_cell(&m, 0, 0.9, 0.5), RED);
    }

    #[test]
    fn missing_texture_and_bad_sizes() {
        let classes = vec![Class {
            name: "gone".into(),
            first_tile: 0,
            num_tiles: 1,
            width: 1,
            read_width: 0,
            image: None,
            position: (0, 0),
            placed: false,
        }];
        let (hm, bt) = map(2, 2, vec![0; 4], vec![0; 4]);
        let m = build_from_parts(&hm, &bt, classes, OPTS).unwrap();
        assert_eq!(m.stats.missing_classes, vec!["gone".to_string()]);
        // `getUVForNdx` returns zeros for a missing source tile (`WorldHeightMap.cpp:1576-1580`).
        assert!(m.uvs.iter().all(|uv| *uv == [0.0, 0.0]));
        assert_eq!(m.stats.atlas_height, 1);
        let (hm2, mut bt2) = map(2, 2, vec![0; 4], vec![0; 4]);
        bt2.tile_ndxes.pop();
        assert!(build_from_parts(&hm2, &bt2, vec![], OPTS).is_err());
    }

    #[test]
    fn weights_sum_to_one_and_hit_corners() {
        for flip in [false, true] {
            for &(s, t) in &[(0.2f32, 0.7f32), (0.9, 0.1), (0.5, 0.5), (0.01, 0.01)] {
                let w = corner_weights(s, t, flip);
                assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6);
                assert!(w.iter().all(|&x| x >= -1e-6));
                let (px, py) = (w[1] + w[2], w[2] + w[3]);
                assert!((px - s).abs() < 1e-6 && (py - t).abs() < 1e-6);
            }
        }
    }
}
