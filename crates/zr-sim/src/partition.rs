//! Partition manager: uniform grid spatial index.
//!
//! Mirrors the concepts of `Object/PartitionManager.cpp`: a fixed `m_cellSize` grid over the
//! world extents (`worldToCell` floors `(w - lo) * cellSizeInv`), objects registered in every
//! cell their bounding circle touches (`PartitionData`), and range queries over cells.
//!
//! Deterministic iteration: cells are `Vec<ObjectId>` kept sorted; queries gather candidate ids,
//! `sort + dedup`, then order results by `(distance, id)` so the result never depends on
//! insertion history. Stubbed: filters (`PartitionFilter`), shroud/fog, threat/value maps,
//! per-cell terrain heights, line/polygon queries.

use crate::math::{real_to_int_ceil, real_to_int_floor, Coord3D};
use crate::object::ObjectId;
use std::collections::BTreeMap;

/// Default `PartitionCellSize` (GameData.ini ships 40; the engine clamps it to >= 1).
pub const DEFAULT_CELL_SIZE: f32 = 40.0;

/// The COI fill's `REAL_TO_INT_CEIL` is [`real_to_int_ceil`] (`math.rs`, BaseFunctions.h:97): an exact
/// integer `k` gives `k + 1`. That quirk is load-bearing for the COI fill: the China dozer's box (major
/// 17, minor 10, cell 40) gets `numStepsX = 2` and `numStepsY = 2`, and the engine's cells for it at AN1
/// s3 f1840 are `(23,9)(24,9)` — the extra row only exists because `10 * 0.1f` is exactly 1.0f and this
/// "ceil" makes it 2. Checked against the C++ build (`ZR_L175_ADD`).
#[cfg(test)]
pub(crate) fn engine_ceil_for_test(x: f32) -> i32 {
    real_to_int_ceil(x)
}

/// `DistanceCalculationType` (the 2D flavours).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistanceMode {
    /// Centre to centre.
    FromCenter2D,
    /// Centre to the target's bounding circle edge (`FROM_BOUNDINGSPHERE_2D`).
    FromBoundingSphere2D,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    pos: Coord3D,
    radius: f32,
    cells: (i32, i32, i32, i32),
}

#[derive(Clone, Debug)]
pub struct PartitionManager {
    cell_size: f32,
    cell_size_inv: f32,
    lo: (f32, f32),
    count_x: i32,
    count_y: i32,
    cells: Vec<Vec<ObjectId>>,
    entries: BTreeMap<ObjectId, Entry>,
    /// `PartitionData::m_coiArray`: each object's cells from [`coi_cells`](Self::coi_cells), placed by
    /// `PartitionData::updateCellsTouched` when the object is made dirty with a cell update
    /// (`PartitionManager.cpp:2086`), so it can be older than the object's current pose. Contact pairs
    /// (`addPossibleCollisions`, `:1980`) are read from it.
    coi: BTreeMap<ObjectId, Vec<(i32, i32)>>,
    /// `PartitionCell::m_firstCoiInCell`: the objects whose COI covers each cell, in list order. The engine
    /// prepends (`friend_addToCellList`, `PartitionManager.cpp:1148`), so the **last** element is the head and
    /// `addPossibleCollisions` (`:2005`) walks the list from the end. Placement and removal are in
    /// [`set_coi_cells`](Self::set_coi_cells) and [`unregister`](Self::unregister).
    coi_lists: Vec<Vec<ObjectId>>,
}

impl PartitionManager {
    /// `PartitionManager::init`: `width`/`height` are the playable world extents.
    pub fn new(width: f32, height: f32, cell_size: f32) -> Self {
        let cell_size = if cell_size < 1.0 { 1.0 } else { cell_size };
        let inv = 1.0 / cell_size;
        let count_x = real_to_int_ceil(width * inv).max(1);
        let count_y = real_to_int_ceil(height * inv).max(1);
        Self {
            cell_size,
            cell_size_inv: inv,
            lo: (0.0, 0.0),
            count_x,
            count_y,
            cells: vec![Vec::new(); (count_x * count_y) as usize],
            entries: BTreeMap::new(),
            coi: BTreeMap::new(),
            coi_lists: vec![Vec::new(); (count_x * count_y) as usize],
        }
    }

    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }
    pub fn cell_counts(&self) -> (i32, i32) {
        (self.count_x, self.count_y)
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `worldToCell` (unclamped).
    pub fn world_to_cell(&self, wx: f32, wy: f32) -> (i32, i32) {
        (real_to_int_floor((wx - self.lo.0) * self.cell_size_inv), real_to_int_floor((wy - self.lo.1) * self.cell_size_inv))
    }

    fn cell_range(&self, pos: &Coord3D, radius: f32) -> (i32, i32, i32, i32) {
        let (x0, y0) = self.world_to_cell(pos.x - radius, pos.y - radius);
        let (x1, y1) = self.world_to_cell(pos.x + radius, pos.y + radius);
        (x0.clamp(0, self.count_x - 1), y0.clamp(0, self.count_y - 1), x1.clamp(0, self.count_x - 1), y1.clamp(0, self.count_y - 1))
    }

    fn insert_cells(&mut self, id: ObjectId, r: (i32, i32, i32, i32)) {
        for y in r.1..=r.3 {
            for x in r.0..=r.2 {
                let cell = &mut self.cells[(y * self.count_x + x) as usize];
                if let Err(i) = cell.binary_search(&id) {
                    cell.insert(i, id);
                }
            }
        }
    }

    fn remove_cells(&mut self, id: ObjectId, r: (i32, i32, i32, i32)) {
        for y in r.1..=r.3 {
            for x in r.0..=r.2 {
                let cell = &mut self.cells[(y * self.count_x + x) as usize];
                if let Ok(i) = cell.binary_search(&id) {
                    cell.remove(i);
                }
            }
        }
    }

    /// `PartitionManager::registerObject`.
    pub fn register(&mut self, id: ObjectId, pos: Coord3D, radius: f32) {
        self.unregister(id);
        let cells = self.cell_range(&pos, radius);
        self.insert_cells(id, cells);
        self.entries.insert(id, Entry { pos, radius, cells });
    }

    /// `PartitionManager::registerObject` for an object created this frame: the entry exists but the
    /// object is in no cell until the next [`update`](Self::update). `PartitionData::attachToObject`
    /// only dirties the new module (`PartitionManager.cpp:2913-2940`), and the cells are placed by
    /// `ThePartitionManager->UPDATE()` at the end of the frame (`GameLogic.cpp:3897`, `PartitionManager.cpp:2796`).
    /// So a spatial query made in the creating frame, such as the dozer's dock search that runs as the
    /// building is created (`DozerAIUpdate.cpp:1925`), does not see the new structure.
    pub fn register_deferred(&mut self, id: ObjectId, pos: Coord3D, radius: f32) {
        self.unregister(id);
        // An empty cell range: `update` removes nothing and inserts the real range.
        self.entries.insert(id, Entry { pos, radius, cells: (0, 0, -1, -1) });
    }

    /// `PartitionManager::unRegisterObject`.
    pub fn unregister(&mut self, id: ObjectId) {
        if let Some(e) = self.entries.remove(&id) {
            self.remove_cells(id, e.cells);
        }
        self.remove_coi(id);
    }

    /// `PartitionData::removeAllTouchedCells` (`PartitionManager.cpp:1735`): take the object out of every cell list
    /// it is in. Removal keeps the order of the others (`friend_removeFromCellList`, `:1159`).
    fn remove_coi(&mut self, id: ObjectId) {
        if let Some(old) = self.coi.remove(&id) {
            for c in old {
                let i = (c.1 * self.count_x + c.0) as usize;
                self.coi_lists[i].retain(|&o| o != id);
            }
        }
    }

    /// `PartitionData::updateCellsTouched`: store the object's COI cells (see [`coi_cells`](Self::coi_cells)).
    /// The old cells are left first (`removeAllTouchedCells`), then each new cell gets the object at its list
    /// head (`addCoverage`, `:1191`, `friend_addToCellList`, `:1148`), in the fill's order.
    pub fn set_coi_cells(&mut self, id: ObjectId, cells: Vec<(i32, i32)>) {
        self.remove_coi(id);
        if cells.is_empty() {
            return;
        }
        for &c in &cells {
            let i = (c.1 * self.count_x + c.0) as usize;
            self.coi_lists[i].push(id);
        }
        self.coi.insert(id, cells);
    }

    /// The COI list of one cell (`PartitionCell::getFirstCoiInCell`, `PartitionManager.h:351`), stored with the head
    /// **last**: iterate it in reverse for the engine's order. Empty for a cell off the grid.
    pub fn coi_list(&self, cell: (i32, i32)) -> &[ObjectId] {
        if cell.0 < 0 || cell.1 < 0 || cell.0 >= self.count_x || cell.1 >= self.count_y {
            return &[];
        }
        &self.coi_lists[(cell.1 * self.count_x + cell.0) as usize]
    }

    /// The object's COI cells as last placed by `updateCellsTouched` (empty when it is in no cell, which the
    /// engine treats as "no possible collision", `PartitionManager::getCellAt` returning null).
    pub fn coi_cells_of(&self, id: ObjectId) -> &[(i32, i32)] {
        self.coi.get(&id).map_or(&[], |v| v.as_slice())
    }

    /// Re-register after a move (`PartitionData::updateCellsTouched`). Cheap when the cell range is unchanged.
    pub fn update(&mut self, id: ObjectId, pos: Coord3D) {
        let Some(e) = self.entries.get(&id).copied() else { return };
        let cells = self.cell_range(&pos, e.radius);
        if cells != e.cells {
            self.remove_cells(id, e.cells);
            self.insert_cells(id, cells);
        }
        self.entries.insert(id, Entry { pos, radius: e.radius, cells });
    }

    /// `PartitionManager::worldToCellDist` (`PartitionManager.h:1546`): the number of cells needed to cover a
    /// world distance (rounding up, with the engine's `REAL_TO_INT_CEIL`).
    pub fn world_to_cell_dist(&self, w: f32) -> i32 {
        real_to_int_ceil(w * self.cell_size_inv)
    }

    /// `PartitionData::updateCellsTouched`'s cell fill (`PartitionManager.cpp:2086-2166`): the cells whose
    /// `CellAndObjectIntersection` list a `PartitionData` enters, which is what `addPossibleCollisions`
    /// (`:1980-2014`) turns into contact-list pairs. `doSmallFill` for `GeometryInfo::getIsSmall`,
    /// `doCircleFill` for a sphere/cylinder (`RETAIL_COMPATIBLE_CRC` is on in the comparison build, so the
    /// retail midpoint algorithm, not `doCircleFillPrecise`), `doRectFill` for a box. Cells outside the grid
    /// are dropped (`PartitionManager::getCellAt` returns null there). In the engine's `m_coiArray` order: each
    /// cell once, at the position of the first sub-sample that reaches it (`addSubPixToCoverage`'s search,
    /// `PartitionManager.cpp:1752-1777`). That order is the order `addPossibleCollisions` (`:2001`) visits them.
    ///
    /// The fill is deliberately coarse: `doRectFill` samples on a half-cell grid anchored at the box's
    /// top-left corner (its `numSteps` count is rounded up but the sample origin is the corner), so a box is
    /// in fewer cells than it covers, and a 40-unit circle is in its centre cell only. That coarseness is
    /// part of the contact-list behaviour (AN5 s3: two dozers 29 units apart are *not* a contact pair).
    pub fn coi_cells(&self, pos: Coord3D, angle: f32, shape: crate::template::GeometryShape, major: f32, minor: f32, is_small: bool) -> Vec<(i32, i32)> {
        let mut out: Vec<(i32, i32)> = Vec::new();
        let add = |out: &mut Vec<(i32, i32)>, x: i32, y: i32| {
            if x >= 0 && y >= 0 && x < self.count_x && y < self.count_y {
                out.push((x, y));
            }
        };
        if is_small {
            // `doSmallFill` (`PartitionManager.cpp:1936-1975`).
            let half = self.cell_size * 0.5;
            let r = if major > half { half } else { major };
            let (cx1, cy1) = self.world_to_cell(pos.x - r, pos.y - r);
            let (cx2, cy2) = self.world_to_cell(pos.x + r, pos.y + r);
            for x in cx1..=cx2 {
                for y in cy1..=cy2 {
                    add(&mut out, x, y);
                }
            }
        } else if shape == crate::template::GeometryShape::Box {
            // `doRectFill` (`PartitionManager.cpp:1782-1837`).
            let c = crate::math::cos(angle);
            let s = crate::math::sin(angle);
            let step = self.cell_size * 0.5;
            let (ydx, ydy) = (s * step, -c * step);
            let (xdx, xdy) = (c * step, s * step);
            let inv_times_two = 2.0 * (1.0 / step);
            let nx = real_to_int_ceil(major * inv_times_two);
            let ny = real_to_int_ceil(minor * inv_times_two);
            let (mut tlx, mut tly) = (pos.x - major * c - minor * s, pos.y + minor * c - major * s);
            for _iy in 0..ny {
                let (mut x, mut y) = (tlx, tly);
                for _ix in 0..nx {
                    let (cx, cy) = self.world_to_cell(x, y);
                    add(&mut out, cx, cy);
                    x += xdx;
                    y += xdy;
                }
                tlx += ydx;
                tly += ydy;
            }
        } else {
            // `doCircleFill`, the retail algorithm (`PartitionManager.cpp:1852-1900`, `RETAIL_COMPATIBLE_CRC`):
            // the loop bound is a reference to the running `y`, so it shrinks as `y` decrements.
            let (ccx, ccy) = self.world_to_cell(pos.x, pos.y);
            let mut cell_radius = self.world_to_cell_dist(major);
            if cell_radius < 1 {
                cell_radius = 1;
            }
            let mut y = cell_radius - 1;
            let mut dec = 3 - 2 * cell_radius;
            let end = cell_radius - 1;
            let mut x = 0;
            loop {
                let bound = if cell_radius < 240 { y } else { end };
                if x > bound {
                    break;
                }
                for (x1, x2, yy) in [(ccx - x, ccx + x, ccy + y), (ccx - x, ccx + x, ccy - y), (ccx - y, ccx + y, ccy + x), (ccx - y, ccx + y, ccy - x)] {
                    for cx in x1..=x2 {
                        add(&mut out, cx, yy);
                    }
                }
                if dec >= 0 {
                    dec += (1 - y) << 2;
                    y -= 1;
                }
                dec += (x << 2) + 6;
                x += 1;
            }
        }
        // First occurrence wins (`addSubPixToCoverage` reuses the cell's COI slot, `:1758-1767`).
        let mut uniq: Vec<(i32, i32)> = Vec::with_capacity(out.len());
        for c in out {
            if !uniq.contains(&c) {
                uniq.push(c);
            }
        }
        uniq
    }

    /// All objects within `range` of `center` (2D), sorted nearest-first, ties broken by id.
    pub fn objects_in_range(&self, center: &Coord3D, range: f32, mode: DistanceMode) -> Vec<ObjectId> {        let max_radius = match mode {
            DistanceMode::FromCenter2D => 0.0,
            DistanceMode::FromBoundingSphere2D => self.entries.values().fold(0.0f32, |m, e| if e.radius > m { e.radius } else { m }),
        };
        let reach = range + max_radius;
        let (x0, y0) = self.world_to_cell(center.x - reach, center.y - reach);
        let (x1, y1) = self.world_to_cell(center.x + reach, center.y + reach);
        let (x0, y0) = (x0.clamp(0, self.count_x - 1), y0.clamp(0, self.count_y - 1));
        let (x1, y1) = (x1.clamp(0, self.count_x - 1), y1.clamp(0, self.count_y - 1));
        let mut cand: Vec<ObjectId> = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                cand.extend_from_slice(&self.cells[(y * self.count_x + x) as usize]);
            }
        }
        cand.sort_unstable();
        cand.dedup();
        let mut out: Vec<(u32, ObjectId)> = Vec::new();
        for id in cand {
            let e = &self.entries[&id];
            let d = e.pos.dist_2d(center);
            let d = match mode {
                DistanceMode::FromCenter2D => d,
                DistanceMode::FromBoundingSphere2D => d - e.radius,
            };
            if d <= range {
                // Non-negative f32 bit patterns order like the values; negative (inside) clamps to 0.
                let key = if d > 0.0 { d.to_bits() } else { 0 };
                out.push((key, id));
            }
        }
        out.sort_unstable();
        out.into_iter().map(|(_, id)| id).collect()
    }

    /// `PartitionManager::calcMinRadius` (FASTER_GCO, PartitionManager.cpp:3160-3216): the fewest whole cells
    /// between the source cell (at the origin) and the cell `(dx, dy)` away, measured between the nearest of
    /// the two cells' four sample points. The C++ mixes `Real` (float) subtractions with a `double` sum and
    /// `sqrtf`, so the same mix is kept here.
    fn gco_min_radius(&self, dx: i32, dy: i32) -> i32 {
        let cs = self.cell_size;
        let half = cs * 0.5;
        let (x, y) = (dx as f32 * cs, dy as f32 * cs);
        let centres = [(-half, -half), (half, -half), (-half, half), (half, half)];
        let others = [(x - half, y - half), (x + half, y - half), (x - half, y + half), (x + half, y + half)];
        let mut min_sq = 1e12f64;
        for c in centres {
            for o in others {
                let ddx = (c.0 - o.0) as f64;
                let ddy = (c.1 - o.1) as f64;
                let d = ddx * ddx + ddy * ddy;
                if min_sq > d {
                    min_sq = d;
                }
            }
        }
        let dist = (min_sq as f32).sqrt() as f64;
        (dist / cs as f64).ceil() as i32
    }

    /// The candidates of `PartitionManager::getClosestObjects` with `FASTER_GCO` (PartitionManager.h:72,
    /// PartitionManager.cpp:3366-3410) for a range of `max_dist`: every object registered in a cell whose minimum
    /// cell distance from the source's cell is at most `min(m_maxGcoRadius, worldToCellDist(max_dist))` cells. The
    /// caller applies the distance test; each object appears once, in id order. Objects not yet placed in cells
    /// (created this frame, see [`register_deferred`](Self::register_deferred)) are not candidates.
    pub fn gco_candidates(&self, center: &Coord3D, max_dist: f32) -> Vec<ObjectId> {
        let (cx, cy) = self.world_to_cell(center.x, center.y);
        // `worldToCellDist(maxDist)` (PartitionManager.h:1546-1549) and `m_maxGcoRadius` (calcRadiusVec).
        let max_radius = real_to_int_ceil(max_dist * self.cell_size_inv);
        let (w, h) = (self.count_x as f64 * self.cell_size as f64, self.count_y as f64 * self.cell_size as f64);
        let max_gco = ((w * w + h * h).sqrt() / self.cell_size as f64).ceil() as i32;
        let limit = max_radius.min(max_gco);
        let mut out: Vec<ObjectId> = Vec::new();
        // A cell two or more cells away on an axis is at least `|dx| - 1` cells off, so only offsets within
        // `limit + 1` on each axis can pass `gco_min_radius(dx, dy) <= limit`.
        for dy in -(limit + 1)..=(limit + 1) {
            for dx in -(limit + 1)..=(limit + 1) {
                if self.gco_min_radius(dx, dy) > limit {
                    continue;
                }
                let (x, y) = (cx + dx, cy + dy);
                if x < 0 || y < 0 || x >= self.count_x || y >= self.count_y {
                    continue;
                }
                out.extend_from_slice(&self.cells[(y * self.count_x + x) as usize]);
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The nearest object to `center` within `range`, if any.
    pub fn closest(&self, center: &Coord3D, range: f32, mode: DistanceMode) -> Option<ObjectId> {
        self.objects_in_range(center, range, mode).first().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pm() -> PartitionManager {
        PartitionManager::new(1000.0, 1000.0, 40.0)
    }

    #[test]
    fn range_query_sorted_and_deterministic() {
        let mut a = pm();
        let mut b = pm();
        let objs = [(3, 100.0, 100.0), (1, 110.0, 100.0), (2, 100.0, 130.0), (4, 400.0, 400.0)];
        for &(i, x, y) in &objs {
            a.register(ObjectId(i), Coord3D::new(x, y, 0.0), 5.0);
        }
        for &(i, x, y) in objs.iter().rev() {
            b.register(ObjectId(i), Coord3D::new(x, y, 0.0), 5.0);
        }
        let c = Coord3D::new(100.0, 100.0, 0.0);
        let ra = a.objects_in_range(&c, 50.0, DistanceMode::FromCenter2D);
        assert_eq!(ra, vec![ObjectId(3), ObjectId(1), ObjectId(2)]);
        assert_eq!(ra, b.objects_in_range(&c, 50.0, DistanceMode::FromCenter2D));
    }

    #[test]
    fn move_and_unregister() {
        let mut p = pm();
        p.register(ObjectId(1), Coord3D::new(10.0, 10.0, 0.0), 1.0);
        p.update(ObjectId(1), Coord3D::new(900.0, 900.0, 0.0));
        let c = Coord3D::new(10.0, 10.0, 0.0);
        assert!(p.objects_in_range(&c, 50.0, DistanceMode::FromCenter2D).is_empty());
        assert_eq!(p.objects_in_range(&Coord3D::new(900.0, 900.0, 0.0), 5.0, DistanceMode::FromCenter2D), vec![ObjectId(1)]);
        p.unregister(ObjectId(1));
        assert!(p.is_empty());
    }

    #[test]
    fn bounding_sphere_mode_includes_large_objects() {
        let mut p = pm();
        p.register(ObjectId(1), Coord3D::new(100.0, 100.0, 0.0), 30.0);
        let c = Coord3D::new(150.0, 100.0, 0.0);
        assert!(p.objects_in_range(&c, 25.0, DistanceMode::FromCenter2D).is_empty());
        assert_eq!(p.objects_in_range(&c, 25.0, DistanceMode::FromBoundingSphere2D), vec![ObjectId(1)]);
    }

    /// `gco_candidates` (the FASTER_GCO cell walk) must return every object whose centre is within the range, for
    /// any range; the distance test on top is the caller's. A far object stays out of a short-range walk.
    #[test]
    fn gco_candidates_cover_every_object_within_the_range() {
        let mut p = pm();
        let mut placed = Vec::new();
        for i in 0..40u32 {
            let pos = Coord3D::new(50.0 + (i % 8) as f32 * 37.0, 50.0 + (i / 8) as f32 * 41.0, 0.0);
            p.register(ObjectId(i + 1), pos, 4.0);
            placed.push((ObjectId(i + 1), pos));
        }
        let c = Coord3D::new(120.0, 100.0, 0.0);
        for range in [10.0f32, 60.0, 130.0, 400.0] {
            let cand = p.gco_candidates(&c, range);
            for (id, pos) in &placed {
                if pos.dist_2d(&c) <= range {
                    assert!(cand.contains(id), "{:?} at {} is within {range}", id, pos.dist_2d(&c));
                }
            }
        }
        p.register(ObjectId(99), Coord3D::new(900.0, 900.0, 0.0), 4.0);
        assert!(!p.gco_candidates(&c, 60.0).contains(&ObjectId(99)));
    }
}
