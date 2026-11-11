//! Terrain logic: ground height queries.
//!
//! Ported from `BaseHeightMapRenderObjClass::getHeightMapHeight` (the function the original's
//! `W3DTerrainLogic::getGroundHeight` ends up calling, using the *logic* height map) in
//! Core/GameEngineDevice/Source/W3DDevice/GameClient/BaseHeightMap.cpp.
//!
//! Layout of a cell (`ix`, `iy` already include the border offset):
//! ```text
//!  3-----2
//!  |    /|
//!  |  /  |
//!  |/    |
//!  0-----1
//! ```
//! The cell is split along the 0-2 diagonal; `fy > fx` selects the upper triangle (0,2,3).
//!
//! Cliff cells (`WorldHeightMap::getCliffState` / `setCellCliffFlagFromHeights`) are carried for the
//! pathfinder: use [`TerrainLogic::set_cliff_state`] with the map's `BlendTileData` bits, otherwise
//! the slope rule (corner height range > 9.8) is applied.
//!
//! Water: the map's water-area polygon triggers (`TerrainLogic::getWaterHandle`, `getWaterHeight`,
//! `isUnderwater` in Core/GameEngine/Source/GameLogic/Map/TerrainLogic.cpp). The legacy water grid
//! mesh (`getWaterGridHeight`, script driven, client side) and `setWaterHeight` are not ported.
//!
//! Stubbed: bridges / layers (`getLayerHeight`), `TerrainLogic::UPDATE`.

use crate::math::{floor, real_to_int_floor, Coord3D, MAP_HEIGHT_SCALE, MAP_XY_FACTOR};
use crate::bridge::{Bridge, LAYER_GROUND};
use zr_map::{HeightMap, PolygonTrigger, PolygonTriggers};

/// `Point_In_Triangle_2D` (`WWMath/tri.h:101`) in 2D: the test point is inside when it is on the same
/// side of all three edges as the third vertex. Points exactly on an edge count as inside
/// (`factors[x] * side_factor < 0.0f` rejects only the far side).
fn point_in_triangle_2d(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), test: (f32, f32)) -> bool {
    let perp = |a: (f32, f32), b: (f32, f32)| a.0 * b.1 - a.1 * b.0;
    let p0p1 = (p1.0 - p0.0, p1.1 - p0.1);
    let p1p2 = (p2.0 - p1.0, p2.1 - p1.1);
    let p2p0 = (p0.0 - p2.0, p0.1 - p2.1);
    let p0p2 = (p2.0 - p0.0, p2.1 - p0.1);
    let orientation = perp(p0p1, p0p2);
    if orientation == 0.0 {
        // Degenerate triangle: `Point_In_Triangle_2D` falls back to segment containment
        // (`tri.h:148-204`) — the point counts when it lies on the line and inside both ends.
        let d01 = p0p1.0 * p0p1.0 + p0p1.1 * p0p1.1;
        let d12 = p1p2.0 * p1p2.0 + p1p2.1 * p1p2.1;
        // `float p2p0dist2 = p1p2.Length2();` (`tri.h:154`): the engine's copy of the second edge's length
        // stands in for the third edge's, and it picks the segment ends with that value (`tri.h:156-185`).
        // Ported as-is (lane 180, R21-9); the end vector below still comes from p2p0 (`tri.h:161`, `:171`).
        let d20 = d12;
        // The start vertex is chosen by index (0, 1 or 2), and the end vector is picked by that index too
        // (`pSpE` is p0p1 for start 0, p1p2 for start 1, p2p0 for start 2, tri.h:161-171). Choosing it by value
        // would take the wrong edge when two vertices coincide.
        let (start_idx, start, start_len_sqr) = if d01 > d12 {
            if d01 > d20 {
                (0usize, p0, d01)
            } else {
                (2, p2, d20)
            }
        } else if d12 > d20 {
            (1, p1, d12)
        } else {
            (2, p2, d20)
        };
        let to_end = [p0p1, p1p2, p2p0][start_idx];
        let to_test = (test.0 - start.0, test.1 - start.1);
        if start_len_sqr != 0.0 {
            if perp(to_end, to_test) != 0.0 {
                return false;
            }
            let test_to_end = (to_test.0 - to_end.0, to_test.1 - to_end.1);
            let test_len_sqr = to_test.0 * to_test.0 + to_test.1 * to_test.1;
            let end_len_sqr = test_to_end.0 * test_to_end.0 + test_to_end.1 * test_to_end.1;
            return test_len_sqr <= start_len_sqr && end_len_sqr <= start_len_sqr;
        }
        let test_len_sqr = to_test.0 * to_test.0 + to_test.1 * to_test.1;
        return test_len_sqr == 0.0;
    }
    let side = if orientation > 0.0 { 1.0 } else { -1.0 };
    let f0 = perp(p0p1, (test.0 - p0.0, test.1 - p0.1));
    if f0 * side < 0.0 {
        return false;
    }
    let f1 = perp(p1p2, (test.0 - p1.0, test.1 - p1.1));
    if f1 * side < 0.0 {
        return false;
    }
    let f2 = perp(p2p0, (test.0 - p2.0, test.1 - p2.1));
    if f2 * side < 0.0 {
        return false;
    }
    true
}

/// One water-area polygon trigger (`WaterHandle::m_polygon`), with its bounding box cached.
#[derive(Clone, Debug)]
pub struct WaterArea {
    pub trigger: PolygonTrigger,
    lo: [i32; 2],
    hi: [i32; 2],
}

impl WaterArea {
    pub fn new(trigger: PolygonTrigger) -> Self {
        let (mut lo, mut hi) = ([i32::MAX; 2], [i32::MIN; 2]);
        for p in &trigger.points {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
        Self { trigger, lo, hi }
    }
    /// `getWaterHeight`: z of the first point.
    pub fn height(&self) -> f32 {
        self.trigger.water_height().unwrap_or(0.0)
    }
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.lo[0] && x <= self.hi[0] && y >= self.lo[1] && y <= self.hi[1] && self.trigger.contains(x, y)
    }
}

#[derive(Clone, Debug, Default)]
pub struct TerrainLogic {
    /// `width * height` samples including the border, row-major.
    data: Vec<u8>,
    x_extent: i32,
    y_extent: i32,
    border: i32,
    /// One bit per cell (rows padded to bytes): `m_cellCliffState`.
    cliff: Vec<u8>,
    /// Water-area triggers in map order (`PolygonTrigger` list, water areas only).
    water: Vec<WaterArea>,
    /// Bridges (`TerrainLogic::m_bridgeListHead`), in insertion order.
    bridges: Vec<Bridge>,
}

impl TerrainLogic {
    /// Flat terrain at height 0 of `cells_x * cells_y` playable cells (no border); handy for tests.
    pub fn flat(cells_x: i32, cells_y: i32, sample: u8) -> Self {
        let border = 0;
        // The lookup rejects the outer ring (see `ground_height`), so pad by two cells.
        let (w, h) = (cells_x + 2 * border + 2, cells_y + 2 * border + 2);
        Self { data: vec![sample; (w * h) as usize], x_extent: w, y_extent: h, border, cliff: Vec::new(), water: Vec::new(), bridges: Vec::new() }
    }

    /// Terrain from raw height samples (`width * height`, row-major, including `border` cells on
    /// each side). Cliff flags come from the slope rule. Handy for tests and examples.
    pub fn from_samples(width: i32, height: i32, border: i32, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), (width.max(0) * height.max(0)) as usize);
        let mut t = Self { data, x_extent: width, y_extent: height, border, cliff: Vec::new(), water: Vec::new(), bridges: Vec::new() };
        t.init_cliff_flags_from_heights();
        t
    }

    /// Build from a parsed map height map (`WorldHeightMap`).
    pub fn from_height_map(hm: &HeightMap) -> Self {
        let mut t = Self { data: hm.data.clone(), x_extent: hm.width, y_extent: hm.height, border: hm.border_size, cliff: Vec::new(), water: Vec::new(), bridges: Vec::new() };
        t.init_cliff_flags_from_heights();
        t
    }

    fn cliff_stride(&self) -> usize {
        ((self.x_extent + 7) / 8).max(0) as usize
    }

    /// Replace the cliff bits with the map's own (`BlendTileData::cell_cliff_state`, version >= 7):
    /// `(x_extent + 7) / 8` bytes per row. Ignored when the length does not match.
    pub fn set_cliff_state(&mut self, bits: Vec<u8>) {
        if bits.len() == self.cliff_stride() * self.y_extent.max(0) as usize {
            self.cliff = bits;
        }
    }

    /// `WorldHeightMap::getCliffState`.
    pub fn cliff_state(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || y >= self.y_extent || x >= self.x_extent || self.cliff.is_empty() {
            return false;
        }
        self.cliff[y as usize * self.cliff_stride() + (x >> 3) as usize] & (1 << (x & 7)) != 0
    }

    /// `WorldHeightMap::initCliffFlagsFromHeights` / `setCellCliffFlagFromHeights`:
    /// a cell is a cliff when its four corner heights span more than `PATHFIND_CLIFF_SLOPE_LIMIT_F`.
    pub fn init_cliff_flags_from_heights(&mut self) {
        const PATHFIND_CLIFF_SLOPE_LIMIT_F: f32 = 9.8;
        let stride = self.cliff_stride();
        let mut bits = vec![0u8; stride * self.y_extent.max(0) as usize];
        for y in 0..(self.y_extent - 1).max(0) {
            for x in 0..(self.x_extent - 1).max(0) {
                let h = |xx: i32, yy: i32| f32::from(self.data[(xx + yy * self.x_extent) as usize]) * MAP_HEIGHT_SCALE;
                let (h1, h2, h3, h4) = (h(x, y), h(x + 1, y), h(x, y + 1), h(x + 1, y + 1));
                let mut min_z = h1;
                if min_z > h2 {
                    min_z = h2;
                }
                if min_z > h3 {
                    min_z = h3;
                }
                if min_z > h4 {
                    min_z = h4;
                }
                let mut max_z = h1;
                if max_z < h2 {
                    max_z = h2;
                }
                if max_z < h3 {
                    max_z = h3;
                }
                if max_z < h4 {
                    max_z = h4;
                }
                if max_z - min_z > PATHFIND_CLIFF_SLOPE_LIMIT_F {
                    bits[y as usize * stride + (x >> 3) as usize] |= 1 << (x & 7);
                }
            }
        }
        self.cliff = bits;
    }

    /// `BaseHeightMapRenderObjClass::isCliffCell` (BaseHeightMap.cpp): the cell containing the point.
    pub fn is_cliff_cell(&self, x: f32, y: f32) -> bool {
        if self.data.is_empty() {
            return false;
        }
        let mut ix = (x / MAP_XY_FACTOR) as i32 + self.border;
        let mut iy = (y / MAP_XY_FACTOR) as i32 + self.border;
        if ix < 0 {
            ix = 0;
        }
        if iy < 0 {
            iy = 0;
        }
        if ix >= self.x_extent - 1 {
            ix = self.x_extent - 2;
        }
        if iy >= self.y_extent - 1 {
            iy = self.y_extent - 2;
        }
        self.cliff_state(ix, iy)
    }

    /// Take the water areas out of the map's polygon triggers (`PolygonTrigger::isWaterArea`).
    /// Triggers with fewer than 2 points are dropped (the engine deletes them on load).
    pub fn set_water_from_triggers(&mut self, triggers: &PolygonTriggers) {
        self.water = triggers.water_areas().filter(|t| t.is_valid()).cloned().map(WaterArea::new).collect();
    }

    /// Add one water area (tests, scripts).
    pub fn add_water_area(&mut self, trigger: PolygonTrigger) {
        self.water.push(WaterArea::new(trigger));
    }

    /// The water areas, in map order.
    pub fn water_areas(&self) -> &[WaterArea] {
        &self.water
    }

    /// `TerrainLogic::getWaterHandle`: among water areas containing the point, the one whose first
    /// point has the highest z (`>=`, so later areas win ties; the running maximum starts at 0, so an
    /// area below z = 0 is never found). Returns an index into [`water_areas`](Self::water_areas).
    pub fn water_handle(&self, x: f32, y: f32) -> Option<usize> {
        let (ix, iy) = (floor(x + 0.5) as i32, floor(y + 0.5) as i32);
        let mut best = None;
        let mut water_z = 0.0f32;
        for (i, w) in self.water.iter().enumerate() {
            if w.contains(ix, iy) && w.height() >= water_z {
                water_z = w.height();
                best = Some(i);
            }
        }
        best
    }

    /// `TerrainLogic::getWaterHeight(handle)`.
    pub fn water_height(&self, handle: usize) -> f32 {
        self.water.get(handle).map_or(0.0, WaterArea::height)
    }

    /// Water surface height at a point, if it lies in a water area.
    pub fn water_height_at(&self, x: f32, y: f32) -> Option<f32> {
        self.water_handle(x, y).map(|h| self.water_height(h))
    }

    /// `TerrainLogic::isUnderwater(x, y, &waterZ, &terrainZ)`: `(underwater, water z, terrain z)`.
    /// The water z is `None` when there is no water at the point.
    pub fn underwater_info(&self, x: f32, y: f32) -> (bool, Option<f32>, f32) {
        let ground = self.ground_height(x, y);
        match self.water_height_at(x, y) {
            None => (false, None, ground),
            Some(wz) => (ground < wz, Some(wz), ground),
        }
    }

    /// `TerrainLogic::isUnderwater(x, y)`: water here and the ground below its surface.
    pub fn is_underwater(&self, x: f32, y: f32) -> bool {
        !self.water.is_empty() && self.underwater_info(x, y).0
    }

    pub fn x_extent(&self) -> i32 {
        self.x_extent
    }
    pub fn y_extent(&self) -> i32 {
        self.y_extent
    }
    pub fn border_size(&self) -> i32 {
        self.border
    }

    /// Playable world extents `(width, height)`: `(extent - 2*border) * MAP_XY_FACTOR`.
    pub fn world_size(&self) -> (f32, f32) {
        let w = (self.x_extent - 2 * self.border).max(0) as f32 * MAP_XY_FACTOR;
        let h = (self.y_extent - 2 * self.border).max(0) as f32 * MAP_XY_FACTOR;
        (w, h)
    }

    /// `getClipHeight`: clamps the index into the map.
    fn clip_height(&self, x: i32, y: i32) -> u8 {
        if self.data.is_empty() {
            return 0;
        }
        let x = x.clamp(0, self.x_extent - 1);
        let y = y.clamp(0, self.y_extent - 1);
        self.data.get((x + y * self.x_extent) as usize).copied().unwrap_or(0)
    }

    /// `TerrainLogic::getGroundHeight(x, y)`.
    pub fn ground_height(&self, x: f32, y: f32) -> f32 {
        if self.data.is_empty() {
            return 0.0;
        }
        let inv = 1.0 / MAP_XY_FACTOR;
        let xdiv = x * inv;
        let ydiv = y * inv;
        let ixf = floor(xdiv);
        let iyf = floor(ydiv);
        let fx = xdiv - ixf;
        let fy = ydiv - iyf;
        let ix = ixf as i32 + self.border;
        let iy = iyf as i32 + self.border;

        // The original checks extent-3 because smoothed normals read one more row/column.
        if ix > self.x_extent - 3 || iy > self.y_extent - 3 || iy < 1 || ix < 1 {
            return f32::from(self.clip_height(ix, iy)) * MAP_HEIGHT_SCALE;
        }
        let idx = (ix + iy * self.x_extent) as usize;
        let xe = self.x_extent as usize;
        let p0 = f32::from(self.data[idx]);
        let p2 = f32::from(self.data[idx + xe + 1]);
        if fy > fx {
            let p3 = f32::from(self.data[idx + xe]);
            (p3 + (1.0 - fy) * (p0 - p3) + fx * (p2 - p3)) * MAP_HEIGHT_SCALE
        } else {
            let p1 = f32::from(self.data[idx + 1]);
            (p1 + fy * (p2 - p1) + (1.0 - fx) * (p0 - p1)) * MAP_HEIGHT_SCALE
        }
    }

    /// Ground height at a position (`Coord3D` flavour).
    pub fn height_at(&self, p: &Coord3D) -> f32 {
        self.ground_height(p.x, p.y)
    }

    /// `WorldHeightMap::getHeight` (a raw height sample, border included, no interpolation and no
    /// clamping): `BaseHeightMapRenderObjClass::isClearLineOfSight` reads the samples directly
    /// (`BaseHeightMap.cpp:1113-1125`). `None` off the grid.
    pub(crate) fn raw_sample(&self, x: i32, y: i32) -> Option<u8> {
        if x < 0 || y < 0 || x >= self.x_extent || y >= self.y_extent {
            return None;
        }
        self.data.get((x + y * self.x_extent) as usize).copied()
    }

    /// `BaseHeightMapRenderObjClass::getMaxHeight` (`BaseHeightMap.cpp:1838`): the highest sample of the
    /// map in world units, the early-out of the line-of-sight march.
    pub(crate) fn max_sample(&self) -> u8 {
        self.data.iter().copied().max().unwrap_or(0)
    }

    /// `W3DTerrainVisual::setRawMapHeight` (`W3DTerrainVisual.cpp:897`): lower one height sample. The
    /// caller's grid coordinates are relative to the playable area; the map's border is added here, and
    /// the sample is only ever lowered (`setRawMapHeight`'s `if (getHeight(x,y) > height)`). The index
    /// check is `WorldHeightMap::getHeight`/`setRawHeight`'s flat `0 <= ndx < m_dataSize` on
    /// `y*m_width + x`, so a grid x outside the row range aliases the neighbouring row just as the C++
    /// does it.
    pub fn set_raw_height(&mut self, gx: i32, gy: i32, height: u8) {
        let x = gx + self.border;
        let y = gy + self.border;
        let ndx = y as i64 * self.x_extent as i64 + x as i64;
        if ndx >= 0 && (ndx as usize) < self.data.len() && self.data[ndx as usize] > height {
            self.data[ndx as usize] = height;
        }
    }

    /// `TerrainLogic::flattenTerrain` (`TerrainLogic.cpp:2619`): lower the height samples under a
    /// structure's footprint (and one cell around it) to the average ground height of the grid points
    /// it covers, then leave the caller to re-read `getGroundHeight` for the object's z. Small
    /// geometry is skipped; a box uses its two radii and the object's angle, a sphere/cylinder its
    /// major radius. `getGroundHeight` is evaluated as each grid point is visited, so every building
    /// flattens on top of the previous ones (the operation is not undone when a building dies).
    pub fn flatten_terrain(&mut self, pos: &Coord3D, angle: f32, geometry: &crate::template::Geometry) {
        use crate::template::{Geometry, GeometryShape};
        if geometry.is_small || self.data.is_empty() {
            return;
        }
        // The grid points to visit, as `(i, j)` world-cell coordinates, plus the raw height to stamp.
        match geometry.shape {
            GeometryShape::Box => {
                let Geometry { major_radius: half_x, minor_radius: half_y, .. } = *geometry;
                let (c, s) = (crate::math::cos(angle), crate::math::sin(angle));
                let top_left = (pos.x - half_x * c - half_y * s, pos.y + half_y * c - half_x * s);
                let top_right = (pos.x + half_x * c - half_y * s, pos.y + half_y * c + half_x * s);
                let bottom_right = (pos.x + half_x * c + half_y * s, pos.y - half_y * c + half_x * s);
                let bottom_left = (pos.x - half_x * c + half_y * s, pos.y - half_y * c - half_x * s);
                let min_x = top_left.0.min(top_right.0).min(bottom_right.0).min(bottom_left.0);
                let max_x = top_left.0.max(top_right.0).max(bottom_right.0).max(bottom_left.0);
                let min_y = top_left.1.min(top_right.1).min(bottom_right.1).min(bottom_left.1);
                let max_y = top_left.1.max(top_right.1).max(bottom_right.1).max(bottom_left.1);
                // `iMin.x = REAL_TO_INT_FLOOR(minX/MAP_XY_FACTOR)` and the three others (`TerrainLogic.cpp:2663-2666`).
                let i_min = real_to_int_floor(min_x / MAP_XY_FACTOR);
                let i_max = real_to_int_floor(max_x / MAP_XY_FACTOR);
                // `RETAIL_COMPATIBLE_CRC`: retail scans from row 0 (`TerrainLogic.cpp:2670`).
                let j_min = real_to_int_floor(min_y / MAP_XY_FACTOR).max(0);
                let j_max = real_to_int_floor(max_y / MAP_XY_FACTOR);
                let inside = |x: f32, y: f32| {
                    point_in_triangle_2d(top_left, top_right, bottom_left, (x, y))
                        || point_in_triangle_2d(top_right, bottom_right, bottom_left, (x, y))
                };
                self.flatten_grid(i_min, i_max, j_min, j_max, inside, Some(*pos));
            }
            GeometryShape::Sphere | GeometryShape::Cylinder => {
                let radius = geometry.major_radius;
                let radius_sqr = radius * radius;
                // `iMin.x = REAL_TO_INT_FLOOR((pos->x-radius)/MAP_XY_FACTOR)` and the three others (`TerrainLogic.cpp:2758-2761`).
                let i_min = real_to_int_floor((pos.x - radius) / MAP_XY_FACTOR);
                let i_max = real_to_int_floor((pos.x + radius) / MAP_XY_FACTOR);
                let j_min = real_to_int_floor((pos.y - radius) / MAP_XY_FACTOR).max(0);
                let j_max = real_to_int_floor((pos.y + radius) / MAP_XY_FACTOR);
                let inside = |x: f32, y: f32| {
                    let (dx, dy) = (x - pos.x, y - pos.y);
                    dx * dx + dy * dy < radius_sqr
                };
                self.flatten_grid(i_min, i_max, j_min, j_max, inside, None);
            }
        }
    }

    /// The shared scan of `flattenTerrain`: average `getGroundHeight` over the matching grid points,
    /// round it to a raw sample, then stamp the 3x3 block around each matching point. `centre_clamp`
    /// is the box case's "`setRawMapHeight` only lowers" guard: the raw height may not exceed the
    /// building origin's (`TerrainLogic.cpp:2705-2708`; the sphere/cylinder case has no such guard).
    fn flatten_grid(
        &mut self,
        i_min: i32,
        i_max: i32,
        j_min: i32,
        j_max: i32,
        inside: impl Fn(f32, f32) -> bool,
        centre_clamp: Option<Coord3D>,
    ) {
        let mut total = 0.0f32;
        let mut samples = 0i32;
        for i in i_min..=i_max {
            for j in j_min..=j_max {
                let (x, y) = (i as f32 * MAP_XY_FACTOR, j as f32 * MAP_XY_FACTOR);
                if inside(x, y) {
                    total += self.ground_height(x, y);
                    samples += 1;
                }
            }
        }
        if samples == 0 {
            return;
        }
        let avg = total / samples as f32;
        // `Int rawDataHeight = REAL_TO_INT_FLOOR(0.5f + avgHeight/MAP_HEIGHT_SCALE)` (`TerrainLogic.cpp:2694`).
        let mut raw = real_to_int_floor(0.5 + avg / MAP_HEIGHT_SCALE);
        if let Some(pos) = centre_clamp {
            // `Int centerHeight = REAL_TO_INT_FLOOR(getGroundHeight(pos->x, pos->y)/MAP_HEIGHT_SCALE)` (:2698).
            let centre = real_to_int_floor(self.ground_height(pos.x, pos.y) / MAP_HEIGHT_SCALE);
            if raw > centre {
                raw = centre;
            }
        }
        let raw = raw.clamp(0, 255) as u8;
        for i in i_min..=i_max {
            for j in j_min..=j_max {
                let (x, y) = (i as f32 * MAP_XY_FACTOR, j as f32 * MAP_XY_FACTOR);
                if !inside(x, y) {
                    continue;
                }
                for (di, dj) in [
                    (0, 0),
                    (-1, 0),
                    (1, 0),
                    (0, -1),
                    (0, 1),
                    (-1, -1),
                    (1, 1),
                    (1, -1),
                    (-1, 1),
                ] {
                    self.set_raw_height(i + di, j + dj, raw);
                }
            }
        }
    }

    /// The playable map extent in world units (`TerrainLogic::getExtent`), as `(lo, hi)`.
    /// Used by `findClosestEdgePoint` / `findFarthestEdgePoint` (the Spectre deployment).
    pub fn world_extent(&self) -> (Coord3D, Coord3D) {
        let w = (self.x_extent - 2 * self.border).max(1) as f32 * MAP_XY_FACTOR;
        let h = (self.y_extent - 2 * self.border).max(1) as f32 * MAP_XY_FACTOR;
        (Coord3D::new(0.0, 0.0, 0.0), Coord3D::new(w, h, 0.0))
    }

    /// `TerrainLogic::getExtentIncludingBorder` (W3DTerrainLogic.cpp:231-242): from `-border` on each axis to
    /// `(m_mapDX - border)`, in world units, as `(lo, hi)`. The map's sample grid holds the border on both sides, so
    /// the upper edge is `x_extent - border` cells.
    pub fn extent_including_border(&self) -> (Coord3D, Coord3D) {
        let b = self.border as f32 * MAP_XY_FACTOR;
        let hi_x = self.x_extent as f32 * MAP_XY_FACTOR - b;
        let hi_y = self.y_extent as f32 * MAP_XY_FACTOR - b;
        (Coord3D::new(-b, -b, 0.0), Coord3D::new(hi_x, hi_y, 0.0))
    }

    // ---- bridges (`TerrainLogic` `Bridge` / `PathfindLayer`) -------------------------------

    /// Add a bridge (`TerrainLogic::addBridgeToLogic`). The pathfinder is told separately.
    pub fn add_bridge(&mut self, bridge: Bridge) {
        self.bridges.push(bridge);
    }
    pub fn set_bridges(&mut self, bridges: Vec<Bridge>) {
        self.bridges = bridges;
    }
    pub fn bridges(&self) -> &[Bridge] {
        &self.bridges
    }
    pub fn bridges_mut(&mut self) -> &mut Vec<Bridge> {
        &mut self.bridges
    }

    /// `TerrainLogic::findBridgeAt` (any damage state).
    pub fn find_bridge_at(&self, x: f32, y: f32) -> Option<&Bridge> {
        self.bridges.iter().find(|b| b.contains_2d(x, y))
    }
    /// `TerrainLogic::findBridgeLayerAt`.
    pub fn find_bridge_layer_at(&self, x: f32, y: f32, layer: u32) -> Option<&Bridge> {
        if layer == LAYER_GROUND {
            return None;
        }
        self.bridges.iter().find(|b| b.layer == layer && b.contains_2d(x, y))
    }

    /// `TerrainLogic::getLayerHeight`: the surface height at `(x, y)` on `layer`.
    pub fn layer_height(&self, x: f32, y: f32, layer: u32) -> f32 {
        if layer != LAYER_GROUND {
            if let Some(b) = self.find_bridge_layer_at(x, y, layer) {
                return b.deck_height(x, y);
            }
        }
        self.ground_height(x, y)
    }

    /// `TerrainLogic::getLayerForDestination`: the layer whose height is closest to `pos.z`.
    pub fn layer_for_destination(&self, pos: &Coord3D) -> u32 {
        let mut best_layer = LAYER_GROUND;
        let mut best = (pos.z - self.ground_height(pos.x, pos.y)).abs();
        for b in &self.bridges {
            if b.contains_2d(pos.x, pos.y) {
                let d = (pos.z - b.deck_height(pos.x, pos.y)).abs();
                if d < best {
                    best = d;
                    best_layer = b.layer;
                }
            }
        }
        best_layer
    }

    /// `TerrainLogic::getHighestLayerForDestination` (healthy bridges only).
    pub fn highest_layer_for_destination(&self, pos: &Coord3D) -> u32 {
        let mut best_layer = LAYER_GROUND;
        let mut best = pos.z - self.ground_height(pos.x, pos.y); // deliberately not abs
        for b in &self.bridges {
            if b.destroyed {
                continue;
            }
            if b.contains_2d(pos.x, pos.y) {
                let delta = pos.z - b.deck_height(pos.x, pos.y);
                if delta >= 0.0 && delta.abs() < best.abs() {
                    best = delta;
                    best_layer = b.layer;
                }
            }
        }
        best_layer
    }

    /// The height an object at `(x, y, current_z)` should sit at: the closest of the ground and any
    /// healthy bridge deck (`getLayerForDestination` + `getLayerHeight`).
    pub fn surface_height(&self, x: f32, y: f32, current_z: f32) -> f32 {
        let mut best = self.ground_height(x, y);
        let mut best_d = (current_z - best).abs();
        for b in &self.bridges {
            if b.destroyed {
                continue;
            }
            if b.contains_2d(x, y) {
                let h = b.deck_height(x, y);
                let d = (current_z - h).abs();
                if d < best_d {
                    best = h;
                    best_d = d;
                }
            }
        }
        best
    }

    /// `TerrainLogic::objectInteractsWithBridgeLayer`.
    pub fn object_interacts_with_bridge_layer(&self, pos: &Coord3D, radius: f32, layer: u32) -> bool {
        if layer == LAYER_GROUND {
            return false;
        }
        let Some(b) = self.bridges.iter().find(|b| b.layer == layer) else { return false };
        let near = b.contains_2d(pos.x, pos.y) || b.is_cell_on_end(pos.x, pos.y, radius + crate::pathfind::PATHFIND_CELL_SIZE_F * 0.5);
        if !near {
            return false;
        }
        if (pos.z - b.deck_height(pos.x, pos.y)).abs() > crate::bridge::LAYER_Z_CLOSE_ENOUGH {
            return false;
        }
        !b.destroyed
    }

    /// `TerrainLogic::objectInteractsWithBridgeEnd`.
    pub fn object_interacts_with_bridge_end(&self, pos: &Coord3D, radius: f32, layer: u32) -> bool {
        if layer == LAYER_GROUND {
            return false;
        }
        let Some(b) = self.bridges.iter().find(|b| b.layer == layer) else { return false };
        if !b.is_cell_on_end(pos.x, pos.y, radius + crate::pathfind::PATHFIND_CELL_SIZE_F * 0.5) {
            return false;
        }
        (pos.z - b.deck_height(pos.x, pos.y)).abs() <= crate::bridge::LAYER_Z_CLOSE_ENOUGH
    }

    pub fn crc(&self, c: &mut crate::crc::Crc) {
        c.add_i32(self.x_extent);
        c.add_i32(self.y_extent);
        c.add_i32(self.border);
        c.add_bytes(&self.data);
        c.add_bytes(&self.cliff);
        // Water areas (only when present, so water-free terrain keeps its previous CRC).
        if !self.water.is_empty() {
            c.add_u32(self.water.len() as u32);
            for w in &self.water {
                c.add_i32(w.trigger.id);
                c.add_u32(w.trigger.points.len() as u32);
                for p in &w.trigger.points {
                    c.add_i32(p[0]);
                    c.add_i32(p[1]);
                    c.add_i32(p[2]);
                }
            }
        }
        // Bridges (only when present, so bridge-free terrain keeps its previous CRC).
        if !self.bridges.is_empty() {
            c.add_u32(self.bridges.len() as u32);
            for b in &self.bridges {
                b.crc(c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp() -> TerrainLogic {
        // 8x8, height = 16 * x (so 1 cell step = 16 samples = 10 world units of rise)
        let mut data = vec![0u8; 64];
        for y in 0..8 {
            for x in 0..8 {
                data[y * 8 + x] = (x * 16) as u8;
            }
        }
        TerrainLogic { data, x_extent: 8, y_extent: 8, border: 0, cliff: Vec::new(), water: Vec::new(), bridges: Vec::new() }
    }

    #[test]
    fn exact_at_grid_points() {
        let t = ramp();
        assert_eq!(t.ground_height(20.0, 20.0), 32.0 * MAP_HEIGHT_SCALE);
        assert_eq!(t.ground_height(30.0, 40.0), 48.0 * MAP_HEIGHT_SCALE);
    }

    #[test]
    fn linear_across_cells() {
        let t = ramp();
        let h = t.ground_height(25.0, 25.0);
        assert!((h - 40.0 * MAP_HEIGHT_SCALE).abs() < 1e-4);
    }

    #[test]
    fn triangle_split_matches_diagonal() {
        // Single non-planar quad: p0=0, p1=0, p2=100, p3=0, at cell (2,2).
        let mut t = ramp();
        t.data.iter_mut().for_each(|v| *v = 0);
        t.data[2 + 3 * 8 + 1] = 100; // p2 at (3,3)
        // fx=fy exactly on diagonal belongs to the lower triangle (fy > fx is false).
        let on_diag = t.ground_height(25.0, 25.0);
        assert!((on_diag - 50.0 * MAP_HEIGHT_SCALE).abs() < 1e-3);
        // below the diagonal (fx > fy) height = fy*(p2-p1) = fy*100
        let below = t.ground_height(28.0, 22.0);
        assert!((below - 20.0 * MAP_HEIGHT_SCALE).abs() < 1e-3);
        // above the diagonal (fy > fx) height = fx*(p2-p3) = fx*100
        let above = t.ground_height(22.0, 28.0);
        assert!((above - 20.0 * MAP_HEIGHT_SCALE).abs() < 1e-3);
    }

    #[test]
    fn off_map_clips() {
        let t = ramp();
        assert_eq!(t.ground_height(-500.0, 20.0), 0.0);
        assert_eq!(t.ground_height(9999.0, 20.0), 112.0 * MAP_HEIGHT_SCALE);
    }

    fn lake(z: i32) -> PolygonTrigger {
        PolygonTrigger {
            name: "lake".into(),
            layer: String::new(),
            id: 1,
            is_water: true,
            is_river: false,
            river_start: 0,
            points: vec![[20, 20, z], [60, 20, z], [60, 60, z], [20, 60, z]],
        }
    }

    #[test]
    fn water_height_and_underwater() {
        let mut t = ramp(); // ground height 16 * x samples
        assert!(!t.is_underwater(30.0, 30.0));
        t.add_water_area(lake(10));
        assert_eq!(t.water_height_at(30.0, 30.0), Some(10.0));
        assert_eq!(t.water_height_at(5.0, 30.0), None);
        assert!(t.ground_height(30.0, 30.0) > 10.0);
        assert!(!t.is_underwater(30.0, 30.0), "ground above the surface");
        t.add_water_area(lake(40));
        assert!(t.is_underwater(30.0, 30.0), "highest water wins");
        assert_eq!(t.underwater_info(30.0, 30.0).1, Some(40.0));
        assert!(!t.is_underwater(70.0, 30.0), "outside the polygon");
        let (mut a, mut b) = (crate::crc::Crc::new(), crate::crc::Crc::new());
        t.crc(&mut a);
        ramp().crc(&mut b);
        assert_ne!(a.get(), b.get(), "water is part of the CRC");
    }

    #[test]
    fn negative_water_is_never_found() {
        let mut t = ramp();
        t.add_water_area(lake(-5));
        assert_eq!(t.water_handle(30.0, 30.0), None);
    }

    /// `TerrainLogic::flattenTerrain` (`TerrainLogic.cpp:2619`) on a box: the average of
    /// `getGroundHeight` over the covered *grid points* (multiples of `MAP_XY_FACTOR`; the box at
    /// (150, 200) with half extents 10 x 5 spans y 195..205, so only the row y = 200 contributes),
    /// rounded by `floor(0.5 + avg/MAP_HEIGHT_SCALE)`, then stamped into the 3x3 block around every
    /// covered point (only lowering). The bumps at (15,20), (16,20) and (17,19) make the three covered
    /// points 18.75, 25, 25: avg 22.9167 -> raw sample 37 -> 23.125. (17,19) is one cell outside the
    /// box and is stamped by (16,20)'s 3x3 block; (22,19) is out of reach.
    #[test]
    fn flatten_terrain_averages_the_footprint_and_stamps_3x3() {
        let (w, h) = (42usize, 42usize);
        let mut data = vec![30u8; w * h];
        for (x, y) in [(15usize, 20usize), (16, 20), (17, 19)] {
            data[x + y * w] = 40;
        }
        let mut t = TerrainLogic::from_samples(w as i32, h as i32, 0, data);
        assert_eq!(t.ground_height(150.0, 200.0), 25.0);
        let geometry = crate::template::Geometry {
            shape: crate::template::GeometryShape::Box,
            major_radius: 10.0,
            minor_radius: 5.0,
            height: 10.0,
            is_small: false,
        };
        t.flatten_terrain(&Coord3D::new(150.0, 200.0, 25.0), 0.0, &geometry);
        assert_eq!(t.ground_height(150.0, 200.0), 23.125, "the footprint is the averaged height");
        assert_eq!(t.ground_height(170.0, 190.0), 23.125, "the neighbouring bump is stamped down");
        assert_eq!(t.ground_height(220.0, 190.0), 18.75, "a cell further out is untouched");
        assert_eq!(t.ground_height(140.0, 190.0), 18.75, "a lower neighbour is not raised");

        // A small (inactive) geometry does not flatten at all.
        let mut small = geometry;
        small.is_small = true;
        let mut t2 = TerrainLogic::from_samples(w as i32, h as i32, 0, {
            let mut d = vec![30u8; w * h];
            d[15 + 20 * w] = 40;
            d
        });
        t2.flatten_terrain(&Coord3D::new(150.0, 200.0, 25.0), 0.0, &small);
        assert_eq!(t2.ground_height(150.0, 200.0), 25.0, "small geometry is skipped");
    }
}

/// Lane 180 (R21-9): the collinear triangle's third-edge length (`tri.h:154`).
#[cfg(test)]
#[path = "terrain_l180.rs"]
mod l180_tests;

/// Lane 183 (R22-3): the degenerate triangle's end edge follows the start index (`terrain_l183.rs`).
#[cfg(test)]
#[path = "terrain_l183.rs"]
mod l183_tests;
