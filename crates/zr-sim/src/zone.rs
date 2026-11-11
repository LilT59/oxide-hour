//! Pathfinder zones (`PathfindZoneManager` in `Core/GameEngine/Source/GameLogic/AI/AIPathfind.cpp`).
//!
//! A zone is one contiguous run of the same [`CellType`] (clear, water, cliff, rubble, obstacle), and
//! the engine decides "can anything get from here to there" by comparing the two cells' *effective*
//! zones: the raw zone mapped through the equivalency table of the locomotor's surfaces
//! (`getEffectiveZone`, `AIPathfind.cpp:3272-3300`). Ported here:
//!
//! * `PathfindZoneManager::calculateZones` (`:2661-2997`): the raw per-cell zone (same-type components
//!   inside each `ZONE_BLOCK_SIZE`×`ZONE_BLOCK_SIZE` block, then `resolveZones` and the collapse pass),
//!   then the six equivalency tables built by the global neighbour pass (`applyZone` / `resolveZones` /
//!   `flattenZones`, `:2145-2310`, `:2834-2950`).
//! * `getEffectiveZone` (`:3272-3300`) and `getEffectiveTerrainZone` (`:3264-3268`).
//! * `ZoneBlock` (`:2337-2532`): each block's min/max cell zone and its four equivalence tables
//!   (ground/water, ground/rubble, ground/cliff, crusher), which `PathfindZoneManager::getBlockZone`
//!   (`:3249`) reads. Built by `blockCalculateZones` from the collapsed cell zones, and *not*
//!   rebuilt by `update_zones_for_modify` — the engine leaves them stale until the next
//!   calculation. [`Pathfinder::find_hierarchical_path`](crate::pathfind::Pathfinder::find_hierarchical_path)
//!   runs the engine's `internal_findHierarchicalPath` on top of them.
//! * The neighbour predicates `waterGround` / `groundRubble` / `groundCliff` / `terrain` /
//!   `crusherGround` (`:2145-2230`), quirk for bitwise `CELL_WATER` test included.
//!
//! The tables are **persistent**: they are computed only by [`ZoneManager::calculate_zones`], which
//! the engine calls from `classifyMap` (`AIPathfind.cpp:4814`) and from `processPathfindQueue` on the
//! frame `markZonesDirty` scheduled (`:2641-2652`, `:6029-6032`). Between those, a footprint change
//! marks the cells it touched `UNINITIALIZED_ZONE` and
//! [`ZoneManager::update_zones_for_modify`] gives each of them a same-type neighbour's zone
//! (`:2998-3120`); the equivalency tables are *not* rebuilt, so the cell zones may map through stale
//! tables until the next calculation — exactly what the engine does.
//!
//! Not modelled:
//! * `PathfindLayer` zones — walls and bridges are a grid overlay in the port
//!   ([`Pathfinder::apply_bridges`](crate::pathfind::Pathfinder)), so the layers contribute no zones
//!   (and therefore no `ZoneBlock` bridge flag);
//!
//! The engine's zone numbers are `UnsignedShort`; they are widened to `u32` here (same values below
//! 65536).

use crate::pathfind::{surface, CellType};

/// `PathfindZoneManager::UNINITIALIZED_ZONE`: no zone.
pub const NO_ZONE: u32 = 0;

/// `PathfindZoneManager::ZONE_BLOCK_SIZE` (`PathfindZoneManager.h:48`).
pub const ZONE_BLOCK_SIZE: i32 = 10;

/// `PathfindZoneManager::INITIAL_ZONES` (`PathfindZoneManager.h:47`).
const INITIAL_ZONES: u32 = 256;

/// Table lookup that tolerates a zone number past the allocated tables (`at`), matching the engine's
/// `zone > m_maxZone` guard (`getEffectiveZone`, `:3277`) rather than indexing a shorter vector.
fn at(table: &[u32], zone: u32) -> u32 {
    table.get(zone as usize).copied().unwrap_or(NO_ZONE)
}

/// `resolveZones` (`AIPathfind.cpp:2274`): merge the two zones, keeping the lower number.
fn resolve_zones(src_zone: u32, target_zone: u32, equivalency: &mut [u32], size: u32) {
    let src_zone = at(equivalency, src_zone);
    let target_zone = at(equivalency, target_zone);
    let final_zone = if target_zone < src_zone { at(equivalency, target_zone) } else { at(equivalency, src_zone) };
    for entry in equivalency.iter_mut().take(size as usize) {
        if *entry == target_zone || *entry == src_zone {
            *entry = final_zone;
        }
    }
}

/// `applyZone` (`AIPathfind.cpp:2305`) for the first pass: a cell with no zone yet takes the
/// (equivalent) zone of its same-type neighbour.
fn apply_zone(cell_zone: &mut [u32], equivalency: &mut [u32], target: usize, source: usize, size: u32) {
    let src_zone = at(equivalency, cell_zone[source]);
    let target_zone = at(equivalency, cell_zone[target]);
    if target_zone == NO_ZONE {
        cell_zone[target] = src_zone;
        return;
    }
    if target_zone == src_zone {
        return; // already match.
    }
    resolve_zones(src_zone, target_zone, equivalency, size);
}

/// `applyZone` (`AIPathfind.cpp:2305`) for the global pass, where the equivalency table is indexed by
/// the zone number itself and the cell zones are already collapsed.
fn apply_zone_global(cell_zone: &[u32], equivalency: &mut [u32], target: usize, source: usize, size: u32) {
    let src_zone = at(equivalency, cell_zone[source]);
    let target_zone = at(equivalency, cell_zone[target]);
    if target_zone == NO_ZONE {
        return; // cannot happen after the collapse pass; the engine would write the cell's zone.
    }
    if target_zone == src_zone {
        return;
    }
    resolve_zones(src_zone, target_zone, equivalency, size);
}

/// The six equivalency tables `apply_neighbour_ladder` works on, in its own fixed order.
const T_HIERARCHICAL: usize = 0;
const T_TERRAIN: usize = 1;
const T_CRUSHER: usize = 2;
const T_WATER: usize = 3;
const T_RUBBLE: usize = 4;
const T_CLIFF: usize = 5;

/// One neighbour edge of the global equivalency pass: the `#else` of `RTS_GENERALS &&
/// RETAIL_COMPATIBLE_PATHFINDING` (`AIPathfind.cpp:2847-2873` and `:2900-2926`, the branch this
/// Zero Hour build compiles). Equal cell types join only the hierarchical table; otherwise the
/// terrain and crusher tables each get their own edge, and the exclusive water -> rubble -> cliff
/// ladder runs only when neither of those applied. The Generals-only branch this replaces ran every
/// test independently.
fn apply_neighbour_ladder(
    cell_zone: &[u32],
    tables: &mut [&mut [u32]; 6],
    max_zone: u32,
    ci: usize,
    other: usize,
    target: (CellType, bool),
    source: (CellType, bool),
) {
    let (target_ty, target_fence) = target;
    let (source_ty, source_fence) = source;
    if target_ty == source_ty {
        apply_zone_global(cell_zone, tables[T_HIERARCHICAL], ci, other, max_zone);
        return;
    }
    let mut not_terrain_or_crusher = true;
    if terrain(target_ty, source_ty) {
        apply_zone_global(cell_zone, tables[T_TERRAIN], ci, other, max_zone);
        not_terrain_or_crusher = false;
    }
    if crusher_ground(target_ty, target_fence, source_ty, source_fence) {
        apply_zone_global(cell_zone, tables[T_CRUSHER], ci, other, max_zone);
        not_terrain_or_crusher = false;
    }
    if not_terrain_or_crusher {
        if water_ground(target_ty, source_ty) {
            apply_zone_global(cell_zone, tables[T_WATER], ci, other, max_zone);
        } else if ground_rubble(target_ty, source_ty) {
            apply_zone_global(cell_zone, tables[T_RUBBLE], ci, other, max_zone);
        } else if ground_cliff(target_ty, source_ty) {
            apply_zone_global(cell_zone, tables[T_CLIFF], ci, other, max_zone);
        }
    }
}

/// `flattenZones` (`AIPathfind.cpp:2290`): pull every zone of `zones` onto its hierarchical zone.
fn flatten_zones(zones: &mut [u32], hierarchical: &[u32], size: u32) {
    for i in 0..size as usize {
        let zone1 = zones[i];
        let zone2 = at(hierarchical, zone1);
        let zone1 = at(zones, zone2);
        let zone2 = at(hierarchical, zone1);
        zones[i] = zone2;
    }
    for i in 0..size as usize {
        let zone1 = zones[i];
        let zone2 = at(hierarchical, i as u32);
        if zone1 != zone2 {
            resolve_zones(zone1, zone2, zones, size);
        }
    }
}

/// `waterGround` (`AIPathfind.cpp:2165`): clear next to water. The engine tests the *bit*
/// (`srcType & CELL_WATER`), so rubble (3) and bridge-impassable (5) count as water too — kept as is.
fn water_ground(target: CellType, source: CellType) -> bool {
    let water_bit = CellType::Water as u32;
    (target == CellType::Clear && (source as u32) & water_bit != 0)
        || (source == CellType::Clear && (target as u32) & water_bit != 0)
}

/// `groundRubble` (`:2183`): clear next to rubble.
fn ground_rubble(target: CellType, source: CellType) -> bool {
    (target == CellType::Clear && source == CellType::Rubble) || (source == CellType::Clear && target == CellType::Rubble)
}

/// `groundCliff` (`:2210`): clear next to cliff.
fn ground_cliff(target: CellType, source: CellType) -> bool {
    (target == CellType::Cliff && source == CellType::Clear) || (target == CellType::Clear && source == CellType::Cliff)
}

/// `terrain` (`:2191`): the same terrain once obstacles are read as clear.
fn terrain(target: CellType, source: CellType) -> bool {
    let map = |t: CellType| if t == CellType::Obstacle { CellType::Clear } else { t };
    map(target) == map(source)
}

/// `crusherGround` (`:2200`): clear next to an obstacle *fence*.
fn crusher_ground(target: CellType, target_fence: bool, source: CellType, source_fence: bool) -> bool {
    (target == CellType::Obstacle && target_fence && source == CellType::Clear)
        || (source == CellType::Obstacle && source_fence && target == CellType::Clear)
}

/// `ZoneBlock` (`AIPathfind.cpp:2337-2532`): one `ZONE_BLOCK_SIZE`×`ZONE_BLOCK_SIZE` block's zone
/// equivalency tables, the block-level passability the hierarchical path search reads through
/// `PathfindZoneManager::getBlockZone` (`:3249`).
#[derive(Clone, Debug, Default)]
struct BlockZones {
    /// `m_firstZone`: the lowest cell zone in the block.
    first_zone: u32,
    /// `m_numZones`: one past the highest cell zone minus `m_firstZone`.
    num_zones: u32,
    /// `m_groundCliffZones` / `m_groundWaterZones` / `m_groundRubbleZones` / `m_crusherZones`; empty
    /// when `num_zones < 2` (the engine allocates none then).
    ground_cliff: Vec<u32>,
    ground_water: Vec<u32>,
    ground_rubble: Vec<u32>,
    crusher: Vec<u32>,
}

/// `resolveBlockZones` (`AIPathfind.cpp:2239`): merge two block zones, keeping the lower number.
/// Unlike [`resolve_zones`] the entries are compared directly (the callers already resolved them).
fn resolve_block_zones(src_zone: u32, target_zone: u32, equivalency: &mut [u32]) {
    if target_zone < src_zone {
        for entry in equivalency.iter_mut() {
            if *entry == src_zone {
                *entry = target_zone;
            }
        }
    } else {
        for entry in equivalency.iter_mut() {
            if *entry == target_zone {
                *entry = src_zone;
            }
        }
    }
}

/// `applyBlockZone` (`AIPathfind.cpp:2322`): merge the zones of two neighbouring cells of one block
/// in `equivalency` (entries hold absolute zone numbers, offset by `first_zone`).
fn apply_block_zone(target_zone: u32, source_zone: u32, equivalency: &mut [u32], first_zone: u32) {
    let Some(src) = equivalency.get(source_zone.wrapping_sub(first_zone) as usize).copied() else { return };
    let Some(dst) = equivalency.get(target_zone.wrapping_sub(first_zone) as usize).copied() else { return };
    if dst == src {
        return; // already match.
    }
    resolve_block_zones(src, dst, equivalency);
}

impl BlockZones {
    /// `ZoneBlock::blockCalculateZones` (`AIPathfind.cpp:2374`): size the tables from the block's cell
    /// zones and build the four combiners from its internal cell-type edges. `cell(i)` is the cell
    /// type of grid index `i` (row-major), `fence(i)` its `isObstacleFence()`.
    fn calculate(
        w: i32,
        lo: (i32, i32),
        hi: (i32, i32),
        zone_of: impl Fn(usize) -> u32,
        ty_of: impl Fn(usize) -> CellType,
        fence_of: impl Fn(usize) -> bool,
    ) -> Self {
        let idx = |x: i32, y: i32| (y * w + x) as usize;
        let mut min_zone = zone_of(idx(lo.0, lo.1));
        let mut max_zone = min_zone;
        for y in lo.1..=hi.1 {
            for x in lo.0..=hi.0 {
                let z = zone_of(idx(x, y));
                if min_zone > z {
                    min_zone = z;
                }
                if max_zone < z {
                    max_zone = z;
                }
            }
        }
        let num_zones = 1 + max_zone - min_zone;
        let mut block = Self { first_zone: min_zone, num_zones, ..Default::default() };
        if num_zones == 1 {
            return block; // all zones are equivalent.
        }
        // `allocateZones` (`:2510`): a power-of-two table, entries `i + m_firstZone`.
        let mut allocated = 4u32;
        while allocated <= num_zones {
            allocated *= 2;
        }
        let identity: Vec<u32> = (0..allocated).map(|i| i + min_zone).collect();
        block.ground_cliff = identity.clone();
        block.ground_water = identity.clone();
        block.ground_rubble = identity.clone();
        block.crusher = identity;
        for y in lo.1..=hi.1 {
            for x in lo.0..=hi.0 {
                let here = idx(x, y);
                let (ty_here, fence_here) = (ty_of(here), fence_of(here));
                if x > lo.0 {
                    let other = idx(x - 1, y);
                    if zone_of(here) != zone_of(other) {
                        let (ty_other, fence_other) = (ty_of(other), fence_of(other));
                        if water_ground(ty_here, ty_other) {
                            apply_block_zone(zone_of(here), zone_of(other), &mut block.ground_water, min_zone);
                        }
                        if ground_rubble(ty_here, ty_other) {
                            apply_block_zone(zone_of(here), zone_of(other), &mut block.ground_rubble, min_zone);
                        }
                        if ground_cliff(ty_here, ty_other) {
                            apply_block_zone(zone_of(here), zone_of(other), &mut block.ground_cliff, min_zone);
                        }
                        if crusher_ground(ty_here, fence_here, ty_other, fence_other) {
                            apply_block_zone(zone_of(here), zone_of(other), &mut block.crusher, min_zone);
                        }
                    }
                }
                if y > lo.1 {
                    let other = idx(x, y - 1);
                    if zone_of(here) != zone_of(other) {
                        let (ty_other, fence_other) = (ty_of(other), fence_of(other));
                        if water_ground(ty_here, ty_other) {
                            apply_block_zone(zone_of(here), zone_of(other), &mut block.ground_water, min_zone);
                        }
                        if ground_rubble(ty_here, ty_other) {
                            apply_block_zone(zone_of(here), zone_of(other), &mut block.ground_rubble, min_zone);
                        }
                        if ground_cliff(ty_here, ty_other) {
                            apply_block_zone(zone_of(here), zone_of(other), &mut block.ground_cliff, min_zone);
                        }
                        if crusher_ground(ty_here, fence_here, ty_other, fence_other) {
                            apply_block_zone(zone_of(here), zone_of(other), &mut block.crusher, min_zone);
                        }
                    }
                }
            }
        }
        block
    }

    /// `ZoneBlock::getEffectiveZone` (`AIPathfind.cpp:2444`): the block zone a locomotor with
    /// `surfaces` sees for a cell of this block whose cell zone is `zone`.
    fn effective_zone(&self, surfaces: u32, crusher: bool, zone: u32) -> u32 {
        // `#if !(RTS_GENERALS && RETAIL_COMPATIBLE_PATHFINDING)`: this build keeps the early-out
        // (`RTS_ZEROHOUR`, not the Generals target), so an uninitialized cell zone stays uninitialized.
        if zone == NO_ZONE {
            return zone;
        }
        if surfaces & surface::AIR != 0 {
            return 1; // air is all zone 1.
        }
        if surfaces & surface::GROUND != 0 && surfaces & surface::WATER != 0 && surfaces & surface::CLIFF != 0 {
            // Locomotors can go on ground, water & cliff, so all is zone 1.
            return 1;
        }
        if self.num_zones < 2 {
            return self.first_zone; // if we only got 1 zone, it's all the same zone.
        }
        if zone < self.first_zone || zone >= self.first_zone + self.num_zones {
            return self.first_zone;
        }
        let mut zone = zone - self.first_zone;
        if crusher {
            zone = at(&self.crusher, zone);
            zone = zone.saturating_sub(self.first_zone);
        }
        if surfaces & surface::GROUND != 0 && surfaces & surface::CLIFF != 0 {
            return at(&self.ground_cliff, zone);
        }
        if surfaces & surface::GROUND != 0 && surfaces & surface::WATER != 0 {
            return at(&self.ground_water, zone);
        }
        if surfaces & surface::GROUND != 0 && surfaces & surface::RUBBLE != 0 {
            return at(&self.ground_rubble, zone);
        }
        // CLIFF && WATER: the engine DEBUG_CRASHes ("not supported yet") and falls through.
        zone + self.first_zone
    }
}

/// `PathfindZoneManager`'s zone data for one grid snapshot.
#[derive(Clone, Debug)]
struct ZoneState {
    /// The grid width and height (cells), for the block bookkeeping.
    w: i32,
    h: i32,
    /// `m_maxZone`: one past the highest zone number.
    max_zone: u32,
    /// `PathfindCell::m_zone`: the collapsed zone of every cell.
    cell_zone: Vec<u32>,
    /// `m_hierarchicalZones`: same-type connectivity (and bridges); the fallback table.
    hierarchical: Vec<u32>,
    /// `m_terrainZones`: obstacles read as the terrain under them.
    terrain: Vec<u32>,
    /// `m_groundWaterZones`: clear ⇄ water edges.
    ground_water: Vec<u32>,
    /// `m_groundRubbleZones`: clear ⇄ rubble edges.
    ground_rubble: Vec<u32>,
    /// `m_groundCliffZones`: clear ⇄ cliff edges.
    ground_cliff: Vec<u32>,
    /// `m_crusherZones`: clear ⇄ obstacle-fence edges.
    crusher: Vec<u32>,
    /// The per-block zone tables (`m_zoneBlocks`), row-major over `(w+9)/10 × (h+9)/10` blocks.
    blocks: Vec<BlockZones>,
    /// `m_zoneBlockExtent` (in blocks).
    block_extent: (i32, i32),
}

impl ZoneState {
    /// The tables of a map that has never been calculated (every cell `UNINITIALIZED_ZONE`).
    fn empty() -> Self {
        Self {
            w: 0,
            h: 0,
            max_zone: 1,
            cell_zone: Vec::new(),
            hierarchical: vec![0],
            terrain: vec![0],
            ground_water: vec![0],
            ground_rubble: vec![0],
            ground_cliff: vec![0],
            crusher: vec![0],
            blocks: Vec::new(),
            block_extent: (0, 0),
        }
    }

    /// `PathfindZoneManager::getEffectiveZone` (`AIPathfind.cpp:3272-3300`): the zone a locomotor with
    /// `surfaces` sees, after the table for its surface combination.
    fn effective_zone(&self, surfaces: u32, crusher: bool, zone: u32) -> u32 {
        // `if (zone>m_maxZone) { DEBUG_CRASH(("Invalid zone")); return (0); }`.
        if zone > self.max_zone {
            return NO_ZONE;
        }
        if surfaces & surface::AIR != 0 {
            return 1; // air is all zone 1.
        }
        if surfaces & surface::GROUND != 0 && surfaces & surface::WATER != 0 && surfaces & surface::CLIFF != 0 {
            // Locomotors can go on ground, water & cliff, so all is zone 1.
            return 1;
        }
        let mut zone = zone;
        if crusher {
            zone = at(&self.crusher, zone);
        }
        if surfaces & surface::GROUND != 0 && surfaces & surface::CLIFF != 0 {
            return at(&self.ground_cliff, zone);
        }
        if surfaces & surface::GROUND != 0 && surfaces & surface::WATER != 0 {
            return at(&self.ground_water, zone);
        }
        if surfaces & surface::GROUND != 0 && surfaces & surface::RUBBLE != 0 {
            return at(&self.ground_rubble, zone);
        }
        // CLIFF && WATER: the engine DEBUG_CRASHes ("not supported yet") and falls through.
        at(&self.hierarchical, zone)
    }

    /// `PathfindZoneManager::getEffectiveTerrainZone` (`:3264`): `m_hierarchicalZones[m_terrainZones[zone]]`.
    fn effective_terrain_zone(&self, zone: u32) -> u32 {
        at(&self.hierarchical, at(&self.terrain, zone))
    }
}

/// `PathfindZoneManager::calculateZones` (`AIPathfind.cpp:2661-2997`) over one grid.
///
/// `ty(i)` is `PathfindCell::getType()` of cell `i` (row-major), `is_fence(i)` its
/// `isObstacleFence()`.
fn calculate_zones(w: i32, h: i32, ty: impl Fn(usize) -> CellType, is_fence: impl Fn(usize) -> bool) -> ZoneState {
    let empty = ZoneState {
        w: 0,
        h: 0,
        max_zone: 1,
        cell_zone: Vec::new(),
        hierarchical: vec![0],
        terrain: vec![0],
        ground_water: vec![0],
        ground_rubble: vec![0],
        ground_cliff: vec![0],
        crusher: vec![0],
        blocks: Vec::new(),
        block_extent: (0, 0),
    };
    if w <= 0 || h <= 0 {
        return empty;
    }
    let n = (w as usize) * (h as usize);
    let mut cell_zone = vec![NO_ZONE; n];
    // `zoneEquivalency[maxZones]` (`:2686`): a union-find over raw zone numbers. The engine's array is
    // a fixed 24000 entries on the stack; here it is sized to the grid (a zone per cell at worst), so a
    // large map cannot overrun it.
    let mut equivalency: Vec<u32> = (0..=(n as u32 + 1)).collect();
    let mut max_zone = 1u32; // `m_maxZone = 1` (zone 0 is used as a flag).

    let x_count = (w + ZONE_BLOCK_SIZE - 1) / ZONE_BLOCK_SIZE;
    let y_count = (h + ZONE_BLOCK_SIZE - 1) / ZONE_BLOCK_SIZE;
    for x_block in 0..x_count {
        for y_block in 0..y_count {
            let lo_x = x_block * ZONE_BLOCK_SIZE;
            let lo_y = y_block * ZONE_BLOCK_SIZE;
            let hi_x = (lo_x + ZONE_BLOCK_SIZE - 1).min(w - 1); // bounds are inclusive
            let hi_y = (lo_y + ZONE_BLOCK_SIZE - 1).min(h - 1);
            for j in lo_y..=hi_y {
                for i in lo_x..=hi_x {
                    let ci = (j * w + i) as usize;
                    let ty_here = ty(ci);
                    cell_zone[ci] = NO_ZONE;
                    if i > lo_x && ty_here == ty(ci - 1) {
                        apply_zone(&mut cell_zone, &mut equivalency, ci, ci - 1, max_zone);
                    }
                    if j > lo_y && ty_here == ty(ci - w as usize) {
                        apply_zone(&mut cell_zone, &mut equivalency, ci, ci - w as usize, max_zone);
                    }
                    if cell_zone[ci] == NO_ZONE {
                        cell_zone[ci] = max_zone;
                        max_zone += 1;
                    }
                }
            }
        }
    }

    // Collapse the zones into a 1,2,3... sequence, removing collapsed zones (`:2775-2790`).
    let total_zones = max_zone;
    max_zone = 1;
    let mut collapsed = vec![NO_ZONE; total_zones as usize];
    collapsed[0] = NO_ZONE;
    for i in 1..total_zones as usize {
        let zone = at(&equivalency, i as u32);
        if zone == i as u32 {
            collapsed[i] = max_zone;
            max_zone += 1;
        } else {
            collapsed[i] = collapsed[zone as usize];
        }
    }
    for zone in cell_zone.iter_mut() {
        *zone = collapsed[*zone as usize];
    }

    // The engine now gives every `PathfindLayer` its own zone and resolves the cells that connect to it
    // (`:2793-2811`). Bridge and wall layers are a grid overlay in the port, so there is nothing to do.

    // `m_zoneBlocks[x][y].blockCalculateZones` (`:2789-2806`): the per-block zone equivalency tables,
    // built from the collapsed cell zones before the global equivalency pass below. Row-major in y so
    // the index is `y_block * x_count + x_block`, as `PathfindZoneManager::getBlockZone` reads it.
    let x_count_b = (w + ZONE_BLOCK_SIZE - 1) / ZONE_BLOCK_SIZE;
    let y_count_b = (h + ZONE_BLOCK_SIZE - 1) / ZONE_BLOCK_SIZE;
    let mut blocks = Vec::with_capacity((x_count_b * y_count_b) as usize);
    for y_block in 0..y_count_b {
        for x_block in 0..x_count_b {
            let lo = (x_block * ZONE_BLOCK_SIZE, y_block * ZONE_BLOCK_SIZE);
            let hi = ((lo.0 + ZONE_BLOCK_SIZE - 1).min(w - 1), (lo.1 + ZONE_BLOCK_SIZE - 1).min(h - 1));
            blocks.push(BlockZones::calculate(w, lo, hi, |i| cell_zone[i], &ty, &is_fence));
        }
    }

    // `allocateZones` (`:2518-2546`): a power-of-two table size, identity entries.
    let mut allocated = INITIAL_ZONES;
    while allocated <= max_zone {
        allocated *= 2;
    }
    let identity: Vec<u32> = (0..allocated).collect();
    let mut hierarchical = identity.clone();
    let mut terrain_zones = identity.clone();
    let mut ground_water = identity.clone();
    let mut ground_rubble_zones = identity.clone();
    let mut ground_cliff_zones = identity.clone();
    let mut crusher_zones = identity;

    // Determine water/ground, ground/rubble, ground/cliff, terrain, crusher and hierarchical
    // equivalencies (`:2834-2950`). The compiled branch is the `#else` of `RTS_GENERALS &&
    // RETAIL_COMPATIBLE_PATHFINDING` (`RTS_GENERALS` is only defined for the Generals target,
    // `Generals/Code/CMakeLists.txt:12-18`; this is a `RTS_ZEROHOUR` build), i.e. the ladder: equal
    // types -> hierarchical only, else terrain and crusher each get an edge, and only when neither
    // applied does the exclusive water -> rubble -> cliff ladder run.
    let mut tables: [&mut [u32]; 6] = [
        &mut hierarchical,
        &mut terrain_zones,
        &mut crusher_zones,
        &mut ground_water,
        &mut ground_rubble_zones,
        &mut ground_cliff_zones,
    ];
    for j in 0..h {
        for i in 0..w {
            let ci = (j * w + i) as usize;
            if i > 0 && cell_zone[ci] != cell_zone[ci - 1] {
                let other = ci - 1;
                apply_neighbour_ladder(&cell_zone, &mut tables, max_zone, ci, other, (ty(ci), is_fence(ci)), (ty(other), is_fence(other)));
            }
            if j > 0 && cell_zone[ci] != cell_zone[ci - w as usize] {
                let other = ci - w as usize;
                apply_neighbour_ladder(&cell_zone, &mut tables, max_zone, ci, other, (ty(ci), is_fence(ci)), (ty(other), is_fence(other)));
            }
        }
    }

    // FLATTEN HIERARCHICAL ZONES (`:2961-2965`), then the five combiners onto it (`:2967-2971`).
    for i in 1..max_zone as usize {
        hierarchical[i] = at(&hierarchical, hierarchical[i]);
    }
    flatten_zones(&mut ground_cliff_zones, &hierarchical, max_zone);
    flatten_zones(&mut ground_water, &hierarchical, max_zone);
    flatten_zones(&mut ground_rubble_zones, &hierarchical, max_zone);
    flatten_zones(&mut terrain_zones, &hierarchical, max_zone);
    flatten_zones(&mut crusher_zones, &hierarchical, max_zone);

    ZoneState {
        w,
        h,
        max_zone,
        cell_zone,
        hierarchical,
        terrain: terrain_zones,
        ground_water,
        ground_rubble: ground_rubble_zones,
        ground_cliff: ground_cliff_zones,
        crusher: crusher_zones,
        blocks,
        block_extent: (x_count_b, y_count_b),
    }
}

/// The pathfinder's zone bookkeeping: the lazily recomputed [`ZoneState`] plus the per-search block
/// passable flags used to restrict the fine A* to the hierarchical corridor.
#[derive(Default, Clone, Debug)]
pub struct ZoneManager {
    /// `PathfindCell::m_zone` plus the six equivalency tables (`m_maxZone`): the engine's
    /// `PathfindZoneManager` data, kept between calculations (`calculateZones` is the only writer).
    state: ZoneState,
    /// Block grid size `(blocks_x, blocks_y)` (`m_zoneBlockExtent`).
    block_extent: (i32, i32),
    /// `ZoneBlock::m_markedPassable`: one flag per block, in row-major block order.
    passable: Vec<bool>,
}

impl Default for ZoneState {
    fn default() -> Self {
        Self::empty()
    }
}

impl ZoneManager {
    /// `PathfindZoneManager::calculateZones` (`AIPathfind.cpp:2661-2997`): recompute the per-cell
    /// zones and all six equivalency tables from the current cell grid.
    pub fn calculate_zones(&mut self, w: i32, h: i32, ty: impl Fn(usize) -> CellType, is_fence: impl Fn(usize) -> bool) {
        self.state = calculate_zones(w, h, ty, is_fence);
    }

    /// `PathfindCell::setZone(PathfindZoneManager::UNINITIALIZED_ZONE)` (`AIPathfind.cpp:4253`,
    /// `:4440`, `:4446`, `:4512`, `:4517`): a cell a footprint change touched loses its zone until
    /// `updateZonesForModify` or the next `calculateZones` gives it one again.
    pub fn set_uninitialized(&mut self, cell: usize) {
        if let Some(z) = self.state.cell_zone.get_mut(cell) {
            *z = NO_ZONE;
        }
    }

    /// `PathfindZoneManager::updateZonesForModify` (`AIPathfind.cpp:2998-3120`): re-zone the
    /// `UNINITIALIZED_ZONE` cells of the modified `structureBounds` from a same-type neighbour
    /// inside their `ZONE_BLOCK_SIZE` block, forward sweep then backward. A cell with no same-type
    /// neighbour that already has a zone stays `UNINITIALIZED_ZONE` (0) until the next calculation;
    /// the equivalency tables are left as they are.
    pub fn update_zones_for_modify(
        &mut self,
        w: i32,
        h: i32,
        ty: impl Fn(usize) -> CellType,
        bounds: (i32, i32, i32, i32),
    ) {
        let n = (w as usize) * (h as usize);
        if w <= 0 || h <= 0 || self.state.cell_zone.len() != n {
            return;
        }
        let (lo_x, lo_y, mut hi_x, mut hi_y) = bounds;
        // `bounds.hi.x++; bounds.hi.y++; bounds.hi.updateMin(globalBounds.hi)` (`:3009-3011`).
        hi_x = (hi_x + 1).min(w - 1);
        hi_y = (hi_y + 1).min(h - 1);
        // The engine's `m_extent.lo` is the grid origin (0); a footprint can start off the map, and
        // the port clamps instead of reading outside the cell array.
        let lo_x = lo_x.max(0);
        let lo_y = lo_y.max(0);
        if lo_x > hi_x || lo_y > hi_y {
            return;
        }
        let x_count = (w + ZONE_BLOCK_SIZE - 1) / ZONE_BLOCK_SIZE;
        let y_count = (h + ZONE_BLOCK_SIZE - 1) / ZONE_BLOCK_SIZE;
        let wusize = w as usize;
        for xb in 0..x_count {
            for yb in 0..y_count {
                // `blockBounds` = the block ∩ `bounds` (`:3017-3024`).
                let b_lo_x = (xb * ZONE_BLOCK_SIZE).max(lo_x);
                let b_lo_y = (yb * ZONE_BLOCK_SIZE).max(lo_y);
                let b_hi_x = ((xb + 1) * ZONE_BLOCK_SIZE - 1).min(hi_x);
                let b_hi_y = ((yb + 1) * ZONE_BLOCK_SIZE - 1).min(hi_y);
                if b_lo_x > b_hi_x || b_lo_y > b_hi_y {
                    continue;
                }
                // Forward sweep: left, up, then (up-right && right).
                for j in b_lo_y..=b_hi_y {
                    for i in b_lo_x..=b_hi_x {
                        let ci = (j as usize) * wusize + i as usize;
                        if self.state.cell_zone[ci] != NO_ZONE {
                            continue;
                        }
                        let t = ty(ci);
                        let mut zone = NO_ZONE;
                        if i > b_lo_x && ty(ci - 1) == t {
                            zone = self.state.cell_zone[ci - 1];
                        }
                        if zone == NO_ZONE && j > b_lo_y && ty(ci - wusize) == t {
                            zone = self.state.cell_zone[ci - wusize];
                        }
                        if zone == NO_ZONE
                            && j > b_lo_y
                            && i < b_hi_x
                            && ty(ci + 1 - wusize) == t
                            && ty(ci + 1) == t
                        {
                            zone = self.state.cell_zone[ci + 1 - wusize];
                        }
                        self.state.cell_zone[ci] = zone;
                    }
                }
                // Backward sweep: right, down, then (down-right && right).
                for j in (b_lo_y..=b_hi_y).rev() {
                    for i in (b_lo_x..=b_hi_x).rev() {
                        let ci = (j as usize) * wusize + i as usize;
                        if self.state.cell_zone[ci] != NO_ZONE {
                            continue;
                        }
                        let t = ty(ci);
                        let mut zone = NO_ZONE;
                        if i < b_hi_x && ty(ci + 1) == t {
                            zone = self.state.cell_zone[ci + 1];
                        }
                        if zone == NO_ZONE && j < b_hi_y && ty(ci + wusize) == t {
                            zone = self.state.cell_zone[ci + wusize];
                        }
                        if zone == NO_ZONE
                            && j < b_hi_y
                            && i < b_hi_x
                            && ty(ci + 1 + wusize) == t
                            && ty(ci + 1) == t
                        {
                            zone = self.state.cell_zone[ci + 1 + wusize];
                        }
                        self.state.cell_zone[ci] = zone;
                    }
                }
            }
        }
    }

    /// `PathfindCell::getZone()` of a cell.
    pub fn cell_zone(&self, cell: usize) -> u32 {
        self.state.cell_zone.get(cell).copied().unwrap_or(NO_ZONE)
    }

    /// The raw cell zone from its grid coordinates (`PathfindCell::getZone`), `NO_ZONE` off the grid.
    pub fn cell_zone_at(&self, cell_x: i32, cell_y: i32) -> u32 {
        if cell_x < 0 || cell_y < 0 || cell_x >= self.state.w || cell_y >= self.state.h {
            return NO_ZONE;
        }
        self.cell_zone((cell_y * self.state.w + cell_x) as usize)
    }

    /// `PathfindZoneManager::getBlockZone` (`AIPathfind.cpp:3249`): the `ZoneBlock` zone of the block
    /// holding cell `(cell_x, cell_y)`, for a locomotor with `surfaces` (`crusher` false for the
    /// hierarchical path search). `NO_ZONE` when the block is off the block grid or the block zone is
    /// past `m_maxZone` (`if (zone > m_maxZone) return UNINITIALIZED_ZONE;`).
    pub fn block_zone(&self, cell_x: i32, cell_y: i32, surfaces: u32, crusher: bool) -> u32 {
        let (bx, by) = (cell_x.div_euclid(ZONE_BLOCK_SIZE), cell_y.div_euclid(ZONE_BLOCK_SIZE));
        let (bx_count, by_count) = self.state.block_extent;
        if bx < 0 || by < 0 || bx >= bx_count || by >= by_count {
            return NO_ZONE;
        }
        let block = &self.state.blocks[(by * bx_count + bx) as usize];
        let zone = block.effective_zone(surfaces, crusher, self.cell_zone_at(cell_x, cell_y));
        if zone > self.state.max_zone {
            return NO_ZONE;
        }
        zone
    }

    /// `PathfindZoneManager::getEffectiveZone` (`AIPathfind.cpp:3272`).
    pub fn effective_zone(&self, surfaces: u32, crusher: bool, zone: u32) -> u32 {
        self.state.effective_zone(surfaces, crusher, zone)
    }

    /// `PathfindZoneManager::getEffectiveTerrainZone` (`AIPathfind.cpp:3264`).
    pub fn effective_terrain_zone(&self, zone: u32) -> u32 {
        self.state.effective_terrain_zone(zone)
    }

    /// `m_maxZone` (one past the highest zone number), for tests and diagnostics.
    pub fn max_zone(&self) -> u32 {
        self.state.max_zone
    }

    /// The zones the tables are allocated for (`m_zonesAllocated`), for tests and diagnostics.
    pub fn zones_allocated(&self) -> usize {
        self.state.hierarchical.len()
    }

    // ---- block passable flags (`PathfindZoneManager::clearPassableFlags` et al.) -------------

    /// `PathfindZoneManager::allocateBlocks`: size the block grid from the cell grid.
    pub fn allocate_blocks(&mut self, width: i32, height: i32) {
        let bx = (width + ZONE_BLOCK_SIZE - 1) / ZONE_BLOCK_SIZE;
        let by = (height + ZONE_BLOCK_SIZE - 1) / ZONE_BLOCK_SIZE;
        if (bx, by) == self.block_extent && self.passable.len() == (bx.max(0) * by.max(0)) as usize {
            return;
        }
        self.block_extent = (bx.max(0), by.max(0));
        self.passable = vec![true; (bx.max(0) * by.max(0)) as usize];
    }

    /// `m_zoneBlockExtent` (in blocks).
    pub fn block_extent(&self) -> (i32, i32) {
        self.block_extent
    }

    /// `PathfindZoneManager::clearPassableFlags`.
    pub fn clear_passable_flags(&mut self) {
        for p in &mut self.passable {
            *p = false;
        }
    }

    /// `PathfindZoneManager::setAllPassable`.
    pub fn set_all_passable(&mut self) {
        for p in &mut self.passable {
            *p = true;
        }
    }

    /// `PathfindZoneManager::setPassable`: set the flag of the block containing the cell.
    pub fn set_passable(&mut self, cell_x: i32, cell_y: i32, passable: bool) {
        let Some(i) = self.block_index(cell_x, cell_y) else { return };
        self.passable[i] = passable;
    }

    /// `PathfindZoneManager::isPassable`.
    pub fn is_passable(&self, cell_x: i32, cell_y: i32) -> bool {
        self.block_index(cell_x, cell_y).is_some_and(|i| self.passable[i])
    }

    /// `PathfindZoneManager::clipIsPassable`: out-of-range cells are not passable (used by the
    /// neighbour-corner check).
    pub fn clip_is_passable(&self, cell_x: i32, cell_y: i32) -> bool {
        self.is_passable(cell_x, cell_y)
    }

    fn block_index(&self, cell_x: i32, cell_y: i32) -> Option<usize> {
        let (bx, by) = (cell_x.div_euclid(ZONE_BLOCK_SIZE), cell_y.div_euclid(ZONE_BLOCK_SIZE));
        if bx < 0 || by < 0 || bx >= self.block_extent.0 || by >= self.block_extent.1 {
            return None;
        }
        Some((by * self.block_extent.0 + bx) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid of `w`×`h` cells, filled by `f(x, y)`.
    fn grid(w: i32, h: i32, f: impl Fn(i32, i32) -> CellType) -> (Vec<CellType>, usize) {
        let mut cells = Vec::new();
        for y in 0..h {
            for x in 0..w {
                cells.push(f(x, y));
            }
        }
        (cells, w as usize * h as usize)
    }

    fn ensure(zm: &mut ZoneManager, w: i32, h: i32, cells: &[CellType]) {
        zm.calculate_zones(w, h, |i| cells[i], |_| false);
    }

    #[test]
    fn a_ground_locomotor_is_stopped_by_water_and_an_amphibious_one_is_not() {
        // Left half clear, right half water: one straight shoreline at x = 5.
        let (cells, _) = grid(10, 4, |x, _| if x < 5 { CellType::Clear } else { CellType::Water });
        let mut zm = ZoneManager::default();
        ensure(&mut zm, 10, 4, &cells);
        let (land, sea) = (0usize, 9usize);
        let ground = surface::GROUND | surface::RUBBLE;
        let amphibious = surface::GROUND | surface::WATER;
        let z_land = zm.effective_zone(ground, false, zm.cell_zone(land));
        let z_sea = zm.effective_zone(ground, false, zm.cell_zone(sea));
        assert_ne!(z_land, z_sea, "a ground locomotor cannot cross the shoreline");
        assert_eq!(
            zm.effective_zone(amphibious, false, zm.cell_zone(land)),
            zm.effective_zone(amphibious, false, zm.cell_zone(sea)),
            "an amphibious locomotor walks the clear ⇄ water edge (m_groundWaterZones)"
        );
    }

    /// The compiled equivalency ladder (`#else` of `RTS_GENERALS && RETAIL_COMPATIBLE_PATHFINDING`,
    /// `AIPathfind.cpp:2847-2873`): a clear cell next to rubble matches `waterGround` (the
    /// `srcType & CELL_WATER` bit quirk, `:2165`), so the *water* table takes the edge and the
    /// exclusive water -> rubble -> cliff ladder never reaches the rubble arm. A ground/rubble
    /// locomotor therefore does not cross rubble. The Generals-only branch (every predicate applied
    /// independently) would also join the rubble table and let it through.
    #[test]
    fn the_water_table_takes_the_clear_rubble_edge_not_the_rubble_table() {
        // A clear field with a one-cell rubble wall at x = 5; rubble splits the clear component.
        let (cells, _) = grid(11, 3, |x, _| if x == 5 { CellType::Rubble } else { CellType::Clear });
        let mut zm = ZoneManager::default();
        ensure(&mut zm, 11, 3, &cells);
        let (left, right, rub) = (0usize, 10usize, 5usize);
        let ground_only = surface::GROUND;
        let with_rubble = surface::GROUND | surface::RUBBLE;
        let with_water = surface::GROUND | surface::WATER;
        assert_ne!(
            zm.effective_zone(ground_only, false, zm.cell_zone(left)),
            zm.effective_zone(ground_only, false, zm.cell_zone(right)),
            "a ground-only locomotor is split by rubble"
        );
        assert_eq!(
            zm.effective_zone(with_water, false, zm.cell_zone(left)),
            zm.effective_zone(with_water, false, zm.cell_zone(rub)),
            "the water combiner joins clear and rubble (the `srcType & CELL_WATER` quirk)"
        );
        assert_ne!(
            zm.effective_zone(with_rubble, false, zm.cell_zone(left)),
            zm.effective_zone(with_rubble, false, zm.cell_zone(rub)),
            "the ladder gave the edge to water, so the rubble table stays identity"
        );
    }

    /// The ladder's first two arms: an obstacle next to clear joins the terrain table (obstacles read
    /// as the ground under them) and, when the obstacle is a *fence*, the crusher table too
    /// (`AIPathfind.cpp:2847-2873`). A crusher (`effective_zone`'s `crusher` argument,
    /// `AIUpdate.cpp:1775`'s `getCrusherLevel() > 0`) therefore sees a fence cell as its own zone; a
    /// plain structure cell stays a different zone for it.
    #[test]
    fn a_fence_joins_the_terrain_and_crusher_tables_but_a_structure_only_the_terrain_one() {
        let (cells, _) = grid(7, 7, |x, y| if x == 3 && y == 3 { CellType::Obstacle } else { CellType::Clear });
        let (w, h) = (7i32, 7i32);
        let mut zm = ZoneManager::default();
        // The obstacle cell is a fence.
        zm.calculate_zones(w, h, |i| cells[i], |i| i == (3 * 7 + 3) as usize);
        let (clear, obst) = (0usize, (3 * 7 + 3) as usize);
        let ground = surface::GROUND;
        assert_eq!(
            zm.effective_zone(ground, true, zm.cell_zone(obst)),
            zm.effective_zone(ground, false, zm.cell_zone(clear)),
            "the crusher table (m_crusherZones) joins the fence with the clear cells"
        );
        // Without the fence flag the crusher table gets no edge.
        let mut zm = ZoneManager::default();
        ensure(&mut zm, w, h, &cells);
        assert_ne!(
            zm.effective_zone(ground, true, zm.cell_zone(obst)),
            zm.effective_zone(ground, false, zm.cell_zone(clear)),
            "a non-fence obstacle joins only the terrain table"
        );
    }

    /// `ZoneBlock::blockCalculateZones` (`AIPathfind.cpp:2374-2439`) applies *all four* neighbour
    /// predicates to a block's internal cell-type edges (unlike the global ladder, which stops at the
    /// first). A block holding clear and rubble cells therefore has a ground/rubble equivalency in
    /// `m_groundRubbleZones`, so a ground/rubble locomotor reads one block zone across the edge —
    /// while a ground-only locomotor still sees the two cell zones. Revert check: dropping the
    /// `ground_rubble(...)` edge from `BlockZones::calculate` makes the first assertion fail.
    #[test]
    fn a_block_zone_applies_every_combiner_edge() {
        let (cells, _) = grid(10, 10, |x, _| if x < 5 { CellType::Clear } else { CellType::Rubble });
        let mut zm = ZoneManager::default();
        ensure(&mut zm, 10, 10, &cells);
        let ground_only = surface::GROUND;
        let with_rubble = surface::GROUND | surface::RUBBLE;
        assert_eq!(
            zm.block_zone(0, 0, with_rubble, false),
            zm.block_zone(9, 0, with_rubble, false),
            "m_groundRubbleZones joins the block's clear and rubble cells"
        );
        assert_ne!(
            zm.block_zone(0, 0, ground_only, false),
            zm.block_zone(9, 0, ground_only, false),
            "a ground-only locomotor reads the two cell zones"
        );
    }

    /// `PathfindZoneManager::getBlockZone` (`AIPathfind.cpp:3249`): a block whose cells all share one
    /// zone reports that zone (`ZoneBlock::getEffectiveZone`'s `if (m_numZones<2) return m_firstZone`),
    /// and a cell off the block grid has no block zone. Revert check: without the block-grid bounds
    /// guard in `ZoneManager::block_zone` the off-grid lookups panic.
    #[test]
    fn a_single_zone_block_reports_its_zone_and_off_grid_has_none() {
        let (cells, _) = grid(10, 10, |_, _| CellType::Clear);
        let mut zm = ZoneManager::default();
        ensure(&mut zm, 10, 10, &cells);
        let zone = zm.cell_zone(0);
        assert_eq!(zm.block_zone(0, 0, surface::GROUND, false), zone);
        assert_eq!(zm.block_zone(9, 9, surface::GROUND, false), zone);
        assert_eq!(zm.block_zone(10, 0, surface::GROUND, false), NO_ZONE, "block (1,0) does not exist");
        assert_eq!(zm.block_zone(-1, 0, surface::GROUND, false), NO_ZONE);
    }

    #[test]
    fn a_structure_cell_keeps_its_own_zone_but_its_terrain_zone_is_the_ground_under_it() {
        // A 3×3 obstacle block inside a clear field: the structure's cells are one zone of their own.
        let (cells, _) = grid(9, 9, |x, y| {
            if (3..6).contains(&x) && (3..6).contains(&y) {
                CellType::Obstacle
            } else {
                CellType::Clear
            }
        });
        let mut zm = ZoneManager::default();
        ensure(&mut zm, 9, 9, &cells);
        let (outside, inside) = (0usize, (4 * 9 + 4) as usize);
        let ground = surface::GROUND | surface::RUBBLE;
        let z_out = zm.effective_zone(ground, false, zm.cell_zone(outside));
        let z_in = zm.effective_zone(ground, false, zm.cell_zone(inside));
        assert_ne!(z_in, z_out, "the structure is not the ground's zone for a ground locomotor");
        assert_eq!(
            zm.effective_terrain_zone(zm.cell_zone(inside)),
            z_out,
            "m_terrainZones reads an obstacle cell as the terrain under it"
        );
    }

    #[test]
    fn equal_cell_types_are_joined_across_a_zone_block_border() {
        // The raw zones are only built inside a 10×10 block; the global hierarchical pass joins equal
        // types across the border (x = 9 | x = 10), which is what makes the coarse route complete.
        let (cells, _) = grid(20, 20, |_, _| CellType::Clear);
        let mut zm = ZoneManager::default();
        ensure(&mut zm, 20, 20, &cells);
        let ground = surface::GROUND;
        let a = zm.effective_zone(ground, false, zm.cell_zone(0));
        let b = zm.effective_zone(ground, false, zm.cell_zone(19));
        assert_eq!(a, b, "clear ground is one zone across the block border");
        // A cliff column at x = 10 is its own zone for a ground-only locomotor.
        let (cells, _) = grid(20, 20, |x, _| if x == 10 { CellType::Cliff } else { CellType::Clear });
        let mut zm = ZoneManager::default();
        ensure(&mut zm, 20, 20, &cells);
        let cliff = zm.effective_zone(ground, false, zm.cell_zone(10));
        assert_ne!(a, cliff, "cliff cells are a different zone");
    }

    #[test]
    fn every_cell_has_a_zone_and_the_tables_are_allocated() {
        let (cells, _) = grid(30, 30, |x, y| match (x / 7 + y / 5) % 4 {
            0 => CellType::Clear,
            1 => CellType::Water,
            2 => CellType::Cliff,
            _ => CellType::Rubble,
        });
        let mut zm = ZoneManager::default();
        ensure(&mut zm, 30, 30, &cells);
        assert!(zm.max_zone() > 1);
        assert!(zm.zones_allocated() > zm.max_zone() as usize, "the engine rounds the tables up to a power of two");
        for c in 0..cells.len() {
            assert_ne!(zm.cell_zone(c), NO_ZONE, "every cell is given a zone by calculateZones");
        }
        // Air is all zone 1.
        assert_eq!(zm.effective_zone(surface::AIR, false, zm.cell_zone(0)), 1);
    }

    /// The tables are persistent: only `calculate_zones` rebuilds them, so a cell that a footprint
    /// change marked `UNINITIALIZED_ZONE` keeps zone 0 until then (the engine's
    /// `PathfindCell::setZone(UNINITIALIZED_ZONE)`), and only a new calculation sees new cell types.
    #[test]
    fn a_calculation_is_the_only_thing_that_rezones() {
        let (w, h) = (4i32, 4i32);
        let mut cells = vec![CellType::Clear; (w * h) as usize];
        let mut zm = ZoneManager::default();
        zm.calculate_zones(w, h, |i| cells[i], |_| false);
        let before = zm.effective_zone(surface::GROUND, false, zm.cell_zone(0));
        assert_eq!(before, zm.effective_zone(surface::GROUND, false, zm.cell_zone(15)));
        // A water wall (the whole second row) splits the grid in two, but the tables are stale until
        // a calculation runs.
        for c in cells.iter_mut().take(8).skip(4) {
            *c = CellType::Water;
        }
        assert_eq!(zm.effective_zone(surface::GROUND, false, zm.cell_zone(0)), before, "no lazy relabel");
        zm.calculate_zones(w, h, |i| cells[i], |_| false);
        assert_ne!(
            zm.effective_zone(surface::GROUND, false, zm.cell_zone(0)),
            zm.effective_zone(surface::GROUND, false, zm.cell_zone(15)),
            "water disconnects the two halves"
        );
    }

    /// `updateZonesForModify` (`AIPathfind.cpp:2998-3120`): an `UNINITIALIZED_ZONE` cell takes a
    /// same-type neighbour's zone inside its block; a cell with no same-type neighbour that already
    /// has one stays 0 until `calculateZones`.
    #[test]
    fn update_zones_for_modify_rezones_from_same_type_neighbours() {
        // A clear field with a 3x3 structure block at 3..5: the structure's cells are obstacles.
        let (cells, _) = grid(9, 9, |x, y| {
            if (3..6).contains(&x) && (3..6).contains(&y) {
                CellType::Obstacle
            } else {
                CellType::Clear
            }
        });
        let mut zm = ZoneManager::default();
        ensure(&mut zm, 9, 9, &cells);
        let clear_zone = zm.cell_zone(0);
        let obst_zone = zm.cell_zone(4 * 9 + 4);
        assert_ne!(clear_zone, obst_zone, "the structure is its own zone");
        // A new obstacle placed on clear ground: its cell was touched and is UNINITIALIZED.
        let mut new_cells = cells.clone();
        new_cells[7 * 9 + 7] = CellType::Obstacle;
        zm.set_uninitialized(7 * 9 + 7);
        assert_eq!(zm.cell_zone(7 * 9 + 7), NO_ZONE);
        zm.update_zones_for_modify(9, 9, |i| new_cells[i], (7, 7, 7, 7));
        // A lone obstacle has no same-type neighbour, so it stays uninitialized ...
        assert_eq!(zm.cell_zone(7 * 9 + 7), NO_ZONE, "an unassignable cell stays 0");
        // ... while a clear cell next to it keeps its own zone through the sweep.
        zm.set_uninitialized(7 * 9 + 6);
        zm.update_zones_for_modify(9, 9, |i| new_cells[i], (6, 7, 7, 7));
        assert_eq!(zm.cell_zone(7 * 9 + 6), clear_zone, "the left clear neighbour gives its zone back");
    }
}
