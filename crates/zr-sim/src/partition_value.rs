//! `PartitionManager`'s cash-value cells and the queries built on them.
//!
//! Ported from `GameLogic/Object/PartitionManager.cpp`:
//!
//! * `PartitionCell::m_cashValue[player]` and its accessors (`getCashValue` `:1430`, `addCashValue`
//!   `:1439`, `removeCashValue` `:1451`).
//! * `PartitionManager::doValueAffect` / `undoValueAffect` (`:4271`, `:4304`): a discrete circle of
//!   cells around the object's cell, each gaining `buildCost * mulVal` for the owner, where `mulVal`
//!   falls linearly from 1 at the centre to 0 at the circle's edge (`hLineAddValue` `:5809`,
//!   `hLineRemoveValue` `:5837`).
//! * `Object::addValue` / `removeValue` (`Object.cpp:4916`, `:4946`) and the per-object
//!   `m_partitionLastValue` (`SightingInfo`) that makes the pair cancel.
//! * `PartitionManager::getNearestGroupWithValue` (`:4880`) with `iterateCellsBreadthFirst`
//!   (`:4499`) and `cellValueProc` (`:5661`), and `getMostValuableLocation` (`:4823`).
//!
//! The engine keeps the map incrementally: `Object::handlePartitionCellMaintenance`
//! (`Object.cpp:4880`) runs `removeValue(); addValue();` from a fixed set of state changes (position
//! cell change, containment, capture, death, construction completion, vision/shroud range change).
//! The port re-derives every object's affect once per frame at the end of the frame
//! ([`crate::logic::GameLogic::update_value_map`]). The result is the same map: `addValue` depends only
//! on the object's live cell (not its sub-cell position: the circle is drawn around the cell centre),
//! its vision range, its build cost, its owner and the under-construction / effectively-dead /
//! shroud-range tests, and the remove/add pair with equal arguments cancels exactly.

use crate::math::{real_to_int_ceil, real_to_int_floor, Coord3D};
use crate::object::{Object, PlayerIndex};
use crate::shroud::discrete_circle_cells;

/// `MAX_PLAYER_COUNT` (`Core/GameEngine/Include/Common/GameCommon.h:108`).
pub const MAX_PLAYER_COUNT: usize = 16;

/// `SightingInfo` as `Object::m_partitionLastValue` (`Object.h`, `PartitionManager.cpp:5864-5935`):
/// where the value circle was drawn, how far it reached, how much it added and for whom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ValueAffect {
    /// `m_where`.
    pub pos: Coord3D,
    /// `m_howFar` — `Object::addValue` stores the object's **vision** range here (`Object.cpp:4935`),
    /// not the shroud clearing range the qualifying test reads.
    pub range: f32,
    /// `m_data` — `friend_getBuildCost()` (`Object.cpp:4932`).
    pub data: u32,
    /// `m_forWhom` — `getControllingPlayer()->getPlayerMask()` (`Object.cpp:4934`).
    pub mask: u32,
}

impl ValueAffect {
    /// `SightingInfo::isInvalid` (`PartitionManager.cpp:5884`): `m_howFar == 0.0f`.
    pub fn is_invalid(&self) -> bool {
        self.range == 0.0
    }
}

/// The partition's cash-value cells: `PartitionCell::m_cashValue` for every cell.
#[derive(Clone, Debug)]
pub struct ValueMap {
    cell_size: f32,
    cell_size_inv: f32,
    count_x: i32,
    count_y: i32,
    cells: Vec<[i32; MAX_PLAYER_COUNT]>,
}

/// `Object::addValue`'s qualifier (`Object.cpp:4916-4943`): a controlling player, not under
/// construction, not effectively dead and a positive shroud clearing range. The affect it would add is
/// the object's cell, **vision** range, build cost and player mask.
pub fn value_affect_for(o: &Object) -> Option<ValueAffect> {
    // `if (!getControllingPlayer()) return;` — every sim object has an owner, and `getControllingPlayer`
    // is that owner (`Object::getControllingPlayer`, Object.cpp:1620).
    // `isEffectivelyDead()` is `m_effectivelyDead || body->isDead()` (`Object::isEffectivelyDead`).
    if o.under_construction || o.is_dying() {
        return None;
    }
    let clearing = crate::shroud::shroud_clearing_range(o);
    if clearing <= 0.0 {
        return None;
    }
    Some(ValueAffect {
        pos: o.position,
        range: o.template.vision_range,
        data: o.template.build_cost.max(0) as u32,
        mask: 1u32 << (u32::from(o.player) & 31),
    })
}

impl ValueMap {
    /// A grid matching the partition manager's dimensions (`PartitionManager::init`).
    pub fn new(count_x: i32, count_y: i32, cell_size: f32) -> Self {
        let cell_size = if cell_size < 1.0 { 1.0 } else { cell_size };
        Self {
            cell_size,
            cell_size_inv: 1.0 / cell_size,
            count_x,
            count_y,
            cells: vec![[0i32; MAX_PLAYER_COUNT]; (count_x * count_y).max(0) as usize],
        }
    }

    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }
    pub fn cell_counts(&self) -> (i32, i32) {
        (self.count_x, self.count_y)
    }

    /// `PartitionManager::worldToCell` (`PartitionManager.h:1539`): floor of the world position over
    /// the cell size (the sim's world extent starts at 0).
    fn world_to_cell(&self, wx: f32, wy: f32) -> (i32, i32) {
        (real_to_int_floor(wx * self.cell_size_inv), real_to_int_floor(wy * self.cell_size_inv))
    }

    /// Whether two affects draw the same circle with the same value: the pair depends on the object's
    /// **cell** (the circle is drawn around the cell centre, `doValueAffect` `:4273-4276`), its range, its
    /// value and its owner mask — not on its sub-cell position. The engine re-runs the pair when the
    /// object's touched cells change (`Object::onPartitionCellChange`, `Object.cpp:4876`).
    pub fn same_affect(&self, a: &ValueAffect, b: &ValueAffect) -> bool {
        a.range == b.range
            && a.data == b.data
            && a.mask == b.mask
            && self.world_to_cell(a.pos.x, a.pos.y) == self.world_to_cell(b.pos.x, b.pos.y)
    }

    /// `PartitionCell::getCashValue` (`:1430`): the `Int` cell value read as an `UnsignedInt`.
    pub fn cell_value(&self, x: i32, y: i32, player: PlayerIndex) -> u32 {
        if x < 0 || y < 0 || x >= self.count_x || y >= self.count_y {
            return 0;
        }
        let p = usize::from(player);
        if p >= MAX_PLAYER_COUNT {
            return 0;
        }
        self.cells[(y * self.count_x + x) as usize][p] as u32
    }

    /// `PartitionCell::addCashValue` (`:1439`).
    fn add_cash_value(&mut self, x: i32, y: i32, player: usize, value: u32) {
        if x < 0 || y < 0 || x >= self.count_x || y >= self.count_y || player >= MAX_PLAYER_COUNT {
            return;
        }
        let cell = &mut self.cells[(y * self.count_x + x) as usize];
        cell[player] = cell[player].wrapping_add(value as i32);
    }

    /// `PartitionCell::removeCashValue` (`:1451`).
    fn remove_cash_value(&mut self, x: i32, y: i32, player: usize, value: u32) {
        if x < 0 || y < 0 || x >= self.count_x || y >= self.count_y || player >= MAX_PLAYER_COUNT {
            return;
        }
        let cell = &mut self.cells[(y * self.count_x + x) as usize];
        cell[player] = cell[player].wrapping_sub(value as i32);
    }

    /// `PartitionManager::doValueAffect` (`:4271`) / `undoValueAffect` (`:4304`): draw the circle of
    /// `radius` around `pos` and add (or remove) the affect's value on every touched cell, for every
    /// player in the affect's mask.
    pub fn apply(&mut self, a: &ValueAffect, add: bool) {
        let (cell_x, cell_y) = self.world_to_cell(a.pos.x, a.pos.y);
        // `Real fCellCenterX = INT_TO_REAL(cellCenterX)`: the circle is measured in whole cells from the
        // object's cell, not from its sub-cell position.
        let f_center_x = cell_x as f32;
        let f_center_y = cell_y as f32;
        let mut cell_radius = real_to_int_ceil(a.range * self.cell_size_inv);
        if cell_radius < 1 {
            cell_radius = 1;
        }
        // `Real fCellRadius = INT_TO_REAL(cellRadius + 1);` (`:4282`): the falloff denominator is one cell
        // wider than the drawn circle.
        let f_cell_radius = (cell_radius + 1) as f32;
        let players: Vec<usize> = (0..MAX_PLAYER_COUNT).filter(|p| a.mask & (1u32 << p) != 0).collect();
        for (x, y) in discrete_circle_cells(cell_x, cell_y, cell_radius) {
            // `hLineAddValue` skips cells outside the grid (`:5822`).
            if x < 0 || y < 0 || x >= self.count_x || y >= self.count_y {
                continue;
            }
            // `distance = sqrt( pow(x - parms->xCenter, 2) + pow(y - parms->yCenter, 2) )` (`:5825`): the
            // differences are `Real` (float), `pow`/`sqrt` are the double CRT functions, and the result is
            // stored in a `Real distance` — so the double value is rounded to `f32` *before* `mulVal` is
            // computed. `mulVal = 1 - distance / parms->radius` is then entirely float (`:5826`).
            let dx = x as f32 - f_center_x;
            let dy = y as f32 - f_center_y;
            let distance = (f64::from(dx) * f64::from(dx) + f64::from(dy) * f64::from(dy)).sqrt() as f32;
            // `if (mulVal < 0.0f) mulVal = 0.0f; else if (mulVal > 1.0f) mulVal = 1.0f;` (`:5827-5830`).
            let mul = (1.0f32 - distance / f_cell_radius).clamp(0.0, 1.0);
            // `REAL_TO_UNSIGNEDINT(parms->threatOrValue * mulVal)` (`:5832`): a float multiply, then a
            // truncating cast.
            let value = (a.data as f32 * mul) as u32;
            for p in &players {
                if add {
                    self.add_cash_value(x, y, *p, value);
                } else {
                    self.remove_cash_value(x, y, *p, value);
                }
            }
        }
    }

    /// `PartitionManager::getMostValuableLocation` (`:4823`): the first cell (in row-major order) whose
    /// summed value over the allowed players is the greatest. Returns the cell's corner position
    /// (`getCellX() * m_partitionCellSize`), or `None` when no cell was examined.
    pub fn most_valuable_location(&self, allowed_players_mask: u32) -> Option<Coord3D> {
        if allowed_players_mask == 0 {
            return None;
        }
        let mut greatest: i32 = -1;
        let mut max_cell_value: i32 = -1;
        for i in 0..(self.count_x * self.count_y) {
            let cell = &self.cells[i as usize];
            let mut cell_value: i32 = 0;
            for (p, v) in cell.iter().enumerate() {
                if allowed_players_mask & (1u32 << p) != 0 {
                    // `cellValue += m_cells[i].getCashValue(player)` (`:4855`): an `Int` accumulator fed
                    // with the `UnsignedInt` read (the assignment back to `Int` wraps).
                    cell_value = cell_value.wrapping_add(*v);
                }
            }
            if cell_value > max_cell_value {
                max_cell_value = cell_value;
                greatest = i;
            }
        }
        if greatest == -1 || max_cell_value == -1 {
            return None;
        }
        let x = greatest % self.count_x;
        let y = greatest / self.count_x;
        Some(Coord3D::new(x as f32 * self.cell_size, y as f32 * self.cell_size, 0.0))
    }

    /// `PartitionManager::getNearestGroupWithValue` (`:4880`): breadth-first from `source`, return the
    /// world position of the first cell whose summed value over `allowed_players_mask` passes the
    /// comparison.
    ///
    /// `greater_than` is the caller's parameter in the signature, but the engine overwrites it with
    /// `valueRequired` before the search (`:4905`: `parms.greaterThan = valueRequired;`), so the
    /// comparison direction is "greater" exactly when `value_required != 0`. That is a retail bug and
    /// the comparison build has it too; it is kept.
    pub fn nearest_group_with_value(&self, allowed_players_mask: u32, value_required: i32, source: &Coord3D) -> Option<Coord3D> {
        if allowed_players_mask == 0 {
            return None;
        }
        let greater_than = value_required != 0;
        let cell = self.iterate_cells_breadth_first(source, |cell| cell_value_proc(cell, allowed_players_mask, value_required, greater_than))?;
        Some(Coord3D::new(cell.0 as f32 * self.cell_size, cell.1 as f32 * self.cell_size, 0.0))
    }

    /// `PartitionManager::iterateCellsBreadthFirst` (`:4499`): left, up, right, down; the first cell the
    /// procedure accepts wins.
    ///
    /// Deviation: the engine's "bottom" test is `if (curY + 1 > 0)` (`:4555`) — always true — so it
    /// indexes `bitField` and `m_cells` one row past the grid on the last row (out of bounds). The port
    /// bounds the test to the grid; that is a memory-safety fix, not a behaviour change on any map whose
    /// search reaches the last row (where the engine reads uninitialised memory).
    pub fn iterate_cells_breadth_first(&self, pos: &Coord3D, mut proc: impl FnMut(&[i32; MAX_PLAYER_COUNT]) -> bool) -> Option<(i32, i32)> {
        let (cell_x, cell_y) = self.world_to_cell(pos.x, pos.y);
        if cell_x < 0 || cell_y < 0 || cell_x >= self.count_x || cell_y >= self.count_y {
            return None;
        }
        let cell_count = (self.count_x * self.count_y) as usize;
        let mut done = vec![false; cell_count];
        let mut queue = std::collections::VecDeque::new();
        done[(cell_y * self.count_x + cell_x) as usize] = true;
        queue.push_back((cell_x, cell_y));
        while let Some((cur_x, cur_y)) = queue.pop_front() {
            // The neighbours are added before the cell is processed, in the engine's order: left, top,
            // right, bottom.
            for (nx, ny) in [(cur_x - 1, cur_y), (cur_x, cur_y - 1), (cur_x + 1, cur_y), (cur_x, cur_y + 1)] {
                if nx < 0 || ny < 0 || nx >= self.count_x || ny >= self.count_y {
                    continue;
                }
                let i = (ny * self.count_x + nx) as usize;
                if !done[i] {
                    done[i] = true;
                    queue.push_back((nx, ny));
                }
            }
            let cell = &self.cells[(cur_y * self.count_x + cur_x) as usize];
            if proc(cell) {
                return Some((cur_x, cur_y));
            }
        }
        None
    }
}

/// `cellValueProc` (`:5661`): the cell's summed value over the allowed players against the required
/// value. `val` is an `UnsignedInt` and `valueRequired` an `Int`, so the comparison converts the
/// required value to unsigned (`:5676-5677`).
fn cell_value_proc(cell: &[i32; MAX_PLAYER_COUNT], allowed_players_mask: u32, value_required: i32, greater_than: bool) -> bool {
    let mut val: u32 = 0;
    for (p, v) in cell.iter().enumerate() {
        if allowed_players_mask & (1u32 << p) != 0 {
            val = val.wrapping_add(*v as u32);
        }
    }
    let required = value_required as u32;
    (val > required && greater_than) || (val < required && !greater_than)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn affect(pos: Coord3D, range: f32, data: u32, mask: u32) -> ValueAffect {
        ValueAffect { pos, range, data, mask }
    }

    /// `Object::addValue`'s qualifier (`Object.cpp:4927`): an object still under construction adds nothing,
    /// and the same object's value appears once the construction finishes (its next maintenance).
    /// Revert check: drop the `under_construction` test in `value_affect_for`.
    #[test]
    fn an_object_under_construction_adds_no_value() {
        use crate::template::{kind_of, ObjectTemplate};
        use crate::terrain::TerrainLogic;
        use std::sync::Arc;
        let mut g = crate::logic::GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        g.add_player(1, 0);
        let mut t = ObjectTemplate::new("Building");
        t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.build_cost = 500;
        t.vision_range = 100.0;
        let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 1);
        g.objects.get_mut(&id).unwrap().under_construction = true;
        g.update();
        assert_eq!(g.value_map().cell_value(2, 2, 1), 0, "under construction: no value");
        g.objects.get_mut(&id).unwrap().under_construction = false;
        g.update();
        assert_eq!(g.value_map().cell_value(2, 2, 1), 500, "finished: the build cost is in its cell");
    }

    /// `doValueAffect`/`undoValueAffect` are exact inverses: the add then remove leaves every cell as it
    /// was. Revert check: make `remove_cash_value` subtract `value + 1`.
    #[test]
    fn add_then_remove_restores_the_cells() {
        let mut m = ValueMap::new(20, 20, 40.0);
        let a = affect(Coord3D::new(100.0, 100.0, 0.0), 120.0, 800, 1 << 1);
        m.apply(&a, true);
        let touched: Vec<(i32, i32, u32)> = (0..20)
            .flat_map(|y| (0..20).map(move |x| (x, y)))
            .map(|(x, y)| (x, y, m.cell_value(x, y, 1)))
            .filter(|(_, _, v)| *v > 0)
            .collect();
        assert!(!touched.is_empty(), "the circle touched cells");
        assert!(touched.iter().any(|(x, y, v)| *x == 2 && *y == 2 && *v == 800), "the centre cell has the full value");
        m.apply(&a, false);
        assert!(touched.iter().all(|(x, y, _)| m.cell_value(*x, *y, 1) == 0), "every cell is back to zero");
    }

    /// `hLineAddValue`'s falloff (`:5825-5832`): the cell next to the centre keeps a fraction of the
    /// value, and the value only lands on the owner's cells (`m_forWhom`).
    #[test]
    fn the_value_falls_off_with_the_distance_from_the_centre_cell() {
        let mut m = ValueMap::new(20, 20, 40.0);
        // 4 cells of radius: cell (2,2) is the centre; (6,2) is at the edge (distance 4 / radius 5).
        m.apply(&affect(Coord3D::new(100.0, 100.0, 0.0), 160.0, 1000, 1 << 3), true);
        let centre = m.cell_value(2, 2, 3);
        let next = m.cell_value(3, 2, 3);
        let edge = m.cell_value(6, 2, 3);
        assert_eq!(centre, 1000, "the centre cell keeps the whole value");
        assert!(next > 0 && next < centre, "one cell out is less: {next}");
        assert!(edge < next, "the edge is smaller still: {edge}");
        assert_eq!(m.cell_value(2, 2, 4), 0, "only the owner's cells get the value");
    }

    /// `cellValueProc` (`:5676`) with the engine's `greaterThan = valueRequired` bug (`:4905`): a
    /// required value of 0 means `greaterThan` is false, so the search accepts the first cell *below*
    /// zero — which never happens — and finds nothing; a positive requirement searches for a cell above
    /// it.
    #[test]
    fn the_comparison_direction_follows_the_required_value() {
        let mut m = ValueMap::new(10, 10, 40.0);
        m.apply(&affect(Coord3D::new(100.0, 100.0, 0.0), 40.0, 500, 1 << 1), true);
        let from = Coord3D::new(0.0, 0.0, 0.0);
        assert!(m.nearest_group_with_value(1 << 1, 0, &from).is_none(), "required 0 means 'less than 0'");
        assert!(m.nearest_group_with_value(1 << 1, 100, &from).is_some(), "a cell with 500 passes 100");
        assert!(m.nearest_group_with_value(1 << 1, 900, &from).is_none(), "no cell passes 900");
        assert!(m.nearest_group_with_value(1 << 2, 100, &from).is_none(), "another player's mask sees nothing");
    }

    /// `iterateCellsBreadthFirst` (`:4499`): the search starts at the source cell and walks outwards, so
    /// the nearer of two valued cells wins. Revert check: search row-major instead of breadth-first; the
    /// far circle at x=500 is in an earlier row scan only if the near one is, so seed the near circle on
    /// a later row and the assertion below fails.
    #[test]
    fn the_search_returns_the_nearest_valued_cell() {
        let mut m = ValueMap::new(20, 20, 40.0);
        // Range 39, one cell: `worldToCellDist(39)` = REAL_TO_INT_CEIL(0.975) = 1. An exact 40 would give 2 (the
        // engine's exact-multiple quirk, PartitionManager.h:1548), which this test is not about.
        m.apply(&affect(Coord3D::new(500.0, 100.0, 0.0), 39.0, 500, 1 << 1), true);
        m.apply(&affect(Coord3D::new(300.0, 100.0, 0.0), 39.0, 500, 1 << 1), true);
        // From cell (0,0) the circle around cell (7,2) is nearer than the one around (12,2). The returned
        // position is a cell corner (`getCellX() * m_partitionCellSize`, `:4913-4914`).
        let got = m.nearest_group_with_value(1 << 1, 100, &Coord3D::new(0.0, 0.0, 0.0)).unwrap();
        assert_eq!(got.z, 0.0, "the out location's z is 0");
        assert!(got.x >= 240.0 && got.x < 400.0, "a cell of the near circle (x 6..8): {got:?}");
        assert_eq!(got.x % 40.0, 0.0, "a cell corner, not a centre");
        // From the far side the far circle wins instead.
        let far = m.nearest_group_with_value(1 << 1, 100, &Coord3D::new(760.0, 100.0, 0.0)).unwrap();
        assert!(far.x >= 440.0 && far.x <= 520.0, "a cell of the far circle: {far:?}");
    }
}

/// Lane 176 (wave 20): the falloff's `Real` rounding (R20-7).
#[cfg(test)]
#[path = "partition_value_l176.rs"]
mod l176_tests;
