//! Per-player shroud / fog-of-war grid.
//!
//! Ported from `Object/PartitionManager.cpp` (`PartitionCell::addLooker` / `removeLooker` /
//! `addShrouder` / `removeShrouder` / `getShroudStatusForPlayer`, `PartitionData::getShroudedStatus`,
//! `PartitionManager::doShroudReveal` / `undoShroudReveal` / `doShroudCover` / `update`) and
//! `Object/Object.cpp` (`Object::look` / `unlook`, `Object::getShroudClearingRange`,
//! `Object::getShroudedStatus`), with the circle rasteriser from `Common/DiscreteCircle.cpp`.
//!
//! Each partition cell keeps a signed `current` reference count and a non-negative `active` shrouder
//! count, per player:
//!
//! * `current > 0` is [`CellShroudStatus::Shrouded`] (never seen).
//! * `current == 0` is [`CellShroudStatus::Fogged`] (seen before, no looker now).
//! * `current < 0` is [`CellShroudStatus::Clear`] (`-current` lookers).
//!
//! `addLooker` is `current = min(current - 1, -1)`; `removeLooker` restores `min(active, 1)` when the
//! last looker leaves, otherwise increments. This is what turns a cleared cell into fog (memory)
//! rather than shroud when it is no longer watched.
//!
//! Simplified from the original:
//!
//! * Spy vision (`m_visionSpiedMask`), stealth/disguise and the ghost-object manager are not
//!   ported. `Object::look`'s non-garrisonable-container rule is reduced to "contained objects do
//!   not look".
//! * The delayed unlook queue uses [`UNLOOK_PERSIST_FRAMES`] (the retail INI's 5 s, 150 frames) so a
//!   moving unit's reveal trails, as in the original; `GlobalData` plumbing is not.
//! * `getShroudedStatus`'s FOGGED-vs-SHROUDED distinction for previously seen immobile enemies is
//!   approximated without per-player ghost memory (the distinction only affects client ghosts,
//!   which are not ported, and both statuses reject targeting).

use crate::math::{real_to_int_ceil, real_to_int_floor, Coord3D};
use crate::object::{Object, ObjectId, PlayerIndex};
use std::collections::{BTreeMap, BTreeSet};

/// `UnlookPersistDuration` as shipped in retail `Data/INI/GameData.ini` (`5000` ms, parsed by
/// `INI::parseDurationUnsignedInt` to 150 logic frames; the `GlobalData.cpp` ctor default of 30 is
/// overridden). How long a removed look keeps a cell clear before it falls back to fog.
pub const UNLOOK_PERSIST_FRAMES: u32 = 150;

/// `PLAYERMASK_ALL` (`GameCommon.h`): every player bit.
pub const PLAYERMASK_ALL: u32 = u32::MAX;

/// `CellShroudStatus` (`GameCommon.h`), in the engine's numeric order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CellShroudStatus {
    Clear = 0,
    Fogged = 1,
    Shrouded = 2,
}

/// `ObjectShroudStatus` (`GameCommon.h`), in the engine's numeric order. Ordered so
/// `status >= ObjectShroudStatus::Fogged` matches the original's comparisons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ObjectShroudStatus {
    Invalid = 0,
    Clear = 1,
    PartialClear = 2,
    Fogged = 3,
    Shrouded = 4,
}

/// One partition cell's shroud counters (`PartitionCell::m_shroudLevel`).
#[derive(Clone, Copy, Debug)]
struct ShroudCell {
    /// `m_currentShroud`: starts at 1 (shrouded); `< 0` = clear, `0` = fogged, `> 0` = shrouded.
    current: i16,
    /// `m_activeShroudLevel`: starts at 0; incremented by shrouders (gap generators).
    active: i16,
}

impl ShroudCell {
    fn new() -> Self {
        Self { current: 1, active: 0 }
    }

    fn status(self) -> CellShroudStatus {
        if self.current > 0 {
            CellShroudStatus::Shrouded
        } else if self.current == 0 {
            CellShroudStatus::Fogged
        } else {
            CellShroudStatus::Clear
        }
    }

    /// `PartitionCell::addLooker`: `m_currentShroud = min(m_currentShroud - 1, -1)`.
    fn add_looker(&mut self) {
        self.current = self.current.saturating_sub(1).min(-1);
    }

    /// `PartitionCell::removeLooker`.
    fn remove_looker(&mut self) {
        if self.current == -1 {
            self.current = self.active.min(1);
        } else {
            self.current = self.current.saturating_add(1);
        }
    }

    /// `PartitionCell::addShrouder`.
    fn add_shrouder(&mut self) {
        self.active = self.active.saturating_add(1);
        if self.current == 0 {
            self.current = 1;
        }
    }

    /// `PartitionCell::removeShrouder`.
    fn remove_shrouder(&mut self) {
        self.active = self.active.saturating_sub(1);
    }
}

/// One player's grid over the partition cell space.
#[derive(Clone, Debug)]
pub struct ShroudGrid {
    count_x: i32,
    count_y: i32,
    cell_size_inv: f32,
    cells: Vec<ShroudCell>,
    /// `PartitionManager::revealMapForPlayerPermanently`: every cell is considered actively looked
    /// at forever, so shroud generation no longer functions for this player.
    revealed: bool,
}

impl ShroudGrid {
    fn new(count_x: i32, count_y: i32, cell_size: f32) -> Self {
        Self {
            count_x,
            count_y,
            cell_size_inv: 1.0 / cell_size,
            cells: vec![ShroudCell::new(); (count_x * count_y).max(0) as usize],
            revealed: false,
        }
    }

    fn world_to_cell(&self, wx: f32, wy: f32) -> (i32, i32) {
        (real_to_int_floor(wx * self.cell_size_inv), real_to_int_floor(wy * self.cell_size_inv))
    }

    /// The cells an object's bounding circle touches (`PartitionData` COIs), clamped to the grid.
    /// `None` when the circle is entirely off the grid (no COIs, which the original treats as
    /// fully shrouded).
    fn cell_range(&self, pos: &Coord3D, radius: f32) -> Option<(i32, i32, i32, i32)> {
        let (x0, y0) = self.world_to_cell(pos.x - radius, pos.y - radius);
        let (x1, y1) = self.world_to_cell(pos.x + radius, pos.y + radius);
        if x1 < 0 || y1 < 0 || x0 >= self.count_x || y0 >= self.count_y {
            return None;
        }
        Some((x0.max(0), y0.max(0), x1.min(self.count_x - 1), y1.min(self.count_y - 1)))
    }

    /// `PartitionCell::getShroudStatusForPlayer` for a cell that may be off the grid.
    fn status(&self, x: i32, y: i32) -> CellShroudStatus {
        if self.revealed {
            return CellShroudStatus::Clear;
        }
        if x < 0 || y < 0 || x >= self.count_x || y >= self.count_y {
            return CellShroudStatus::Shrouded;
        }
        self.cells[(y * self.count_x + x) as usize].status()
    }

    fn apply_cell(&mut self, x: i32, y: i32, add: bool) {
        if x < 0 || y < 0 || x >= self.count_x || y >= self.count_y {
            return;
        }
        let cell = &mut self.cells[(y * self.count_x + x) as usize];
        if add {
            cell.add_looker();
        } else {
            cell.remove_looker();
        }
    }

    /// `doShroudReveal` / `undoShroudReveal`: draw the discrete circle of `radius` around `pos`.
    fn draw_looker(&mut self, pos: &Coord3D, radius: f32, add: bool) {
        let (cx, cy) = self.world_to_cell(pos.x, pos.y);
        let mut cell_radius = real_to_int_ceil(radius * self.cell_size_inv);
        if cell_radius < 1 {
            cell_radius = 1;
        }
        for (x, y) in discrete_circle_cells(cx, cy, cell_radius) {
            self.apply_cell(x, y, add);
        }
    }
}

/// A look currently applied to the shroud (`SightingInfo`): where, how far and for which players.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sighting {
    pub pos: Coord3D,
    pub range: f32,
    /// Bitmask over player indices.
    pub mask: u32,
}

impl Sighting {
    fn cell_key(&self, cell_size_inv: f32) -> (i32, i32) {
        (real_to_int_floor(self.pos.x * cell_size_inv), real_to_int_floor(self.pos.y * cell_size_inv))
    }
}

/// The per-player shroud grids and the looks that feed them.
#[derive(Clone, Debug)]
pub struct Shroud {
    cell_size: f32,
    count_x: i32,
    count_y: i32,
    grids: BTreeMap<PlayerIndex, ShroudGrid>,
    /// Looks currently applied, by object (`Object::m_partitionLastLook`).
    looks: BTreeMap<ObjectId, Vec<Sighting>>,
    /// Queued undo-look entries `(frame, sighting)` (`m_pendingUndoShroudReveals`), in insertion
    /// order (deterministic).
    pending: Vec<(u32, Sighting)>,
}

impl Shroud {
    /// A grid matching the partition manager's dimensions.
    pub fn new(count_x: i32, count_y: i32, cell_size: f32) -> Self {
        let cell_size = if cell_size < 1.0 { 1.0 } else { cell_size };
        Self { cell_size, count_x, count_y, grids: BTreeMap::new(), looks: BTreeMap::new(), pending: Vec::new() }
    }

    pub fn cell_size(&self) -> f32 {
        self.cell_size
    }
    pub fn cell_counts(&self) -> (i32, i32) {
        (self.count_x, self.count_y)
    }

    /// The cell status for `player` (off-grid cells are shrouded).
    pub fn cell_status(&self, player: PlayerIndex, x: i32, y: i32) -> CellShroudStatus {
        match self.grids.get(&player) {
            Some(g) => g.status(x, y),
            None => CellShroudStatus::Shrouded,
        }
    }

    /// The status of the cell containing a world position.
    pub fn cell_status_at(&self, player: PlayerIndex, pos: &Coord3D) -> CellShroudStatus {
        let Some(g) = self.grids.get(&player) else { return CellShroudStatus::Shrouded };
        let (x, y) = g.world_to_cell(pos.x, pos.y);
        g.status(x, y)
    }

    /// `PartitionData::getShroudedStatus`: classify an object's bounding circle for `player`.
    /// `always_visible` is `KINDOF_ALWAYS_VISIBLE`; `neutral` is the object's relationship to
    /// `player` being neutral; `immobile` is `KINDOF_IMMOBILE`.
    pub fn object_status(&self, player: PlayerIndex, pos: &Coord3D, radius: f32, immobile: bool, neutral: bool, always_visible: bool) -> ObjectShroudStatus {
        if always_visible {
            return ObjectShroudStatus::Clear;
        }
        // A player with no grid yet has an all-shrouded map.
        let Some(grid) = self.grids.get(&player) else { return ObjectShroudStatus::Shrouded };
        let Some((x0, y0, x1, y1)) = grid.cell_range(pos, radius) else {
            return ObjectShroudStatus::Shrouded;
        };
        let (mut shrouded, mut fogged, mut total) = (0i32, 0i32, 0i32);
        for y in y0..=y1 {
            for x in x0..=x1 {
                total += 1;
                match grid.status(x, y) {
                    CellShroudStatus::Shrouded => shrouded += 1,
                    CellShroudStatus::Fogged => fogged += 1,
                    CellShroudStatus::Clear => {}
                }
            }
        }
        if total == 0 || shrouded == total {
            return ObjectShroudStatus::Shrouded;
        }
        if shrouded + fogged == total {
            // Fogged, with the original's enemy/neutral adjustment: mobile enemies (and neutral
            // mobile objects) are always shrouded, never just fogged.
            if !immobile {
                return ObjectShroudStatus::Shrouded;
            }
            let _ = neutral;
            return ObjectShroudStatus::Fogged;
        }
        if shrouded == 0 && fogged == 0 {
            return ObjectShroudStatus::Clear;
        }
        ObjectShroudStatus::PartialClear
    }

    /// The ally mask of `player`: `player` itself plus every player in `players` whose relationship
    /// to `player` is allied. The owner always sees its own look, even before any `Player` is
    /// registered (the sim's bare tests do not call `add_player`).
    pub fn ally_mask(player: PlayerIndex, players: &[PlayerIndex], allied: impl Fn(PlayerIndex, PlayerIndex) -> bool) -> u32 {
        let mut mask = player_bit(player);
        for &p in players {
            if p != player && allied(p, player) {
                mask |= player_bit(p);
            }
        }
        mask
    }

    /// `PartitionManager::addLooker` for every player in `mask`.
    pub fn add_looker(&mut self, pos: &Coord3D, range: f32, mask: u32) {
        for p in mask_players(mask) {
            self.grid_for(p).draw_looker(pos, range, true);
        }
    }

    /// `PartitionManager::undoShroudReveal`.
    pub fn remove_looker(&mut self, pos: &Coord3D, range: f32, mask: u32) {
        for p in mask_players(mask) {
            self.grid_for(p).draw_looker(pos, range, false);
        }
    }

    /// `PartitionManager::revealMapForPlayerPermanently`: mark the player's whole grid clear and
    /// stop shroud generation for it (called when the player is defeated, `VictoryConditions.cpp:207`).
    pub fn reveal_map_permanently(&mut self, player: PlayerIndex) {
        self.grid_for(player).revealed = true;
    }

    /// `PartitionManager::revealMapForPlayer` (`PartitionManager.cpp:3047`): add and remove a looker
    /// on every cell. Each cell's passive shroud is cleared, but shroud generation keeps working, so
    /// a never-seen cell (`current == 1`) becomes fogged (`current == 0`) rather than clear. This is
    /// the start-of-game reveal `GameLogic::startNewGame` applies to every occupied slot when
    /// `UseShroud` is off (`GameLogic.cpp:1814`, retail `Multiplayer.ini` `UseShroud = No`).
    pub fn reveal_map(&mut self, player: PlayerIndex) {
        for cell in &mut self.grid_for(player).cells {
            cell.add_looker();
            cell.remove_looker();
        }
    }

    /// `PartitionManager::doShroudCover` (shrouders / gap generators): add a cover circle.
    pub fn add_shrouder(&mut self, pos: &Coord3D, range: f32, mask: u32) {
        self.draw_shrouder(pos, range, mask, true);
    }

    /// `PartitionManager::doShroudCover` undo.
    pub fn remove_shrouder(&mut self, pos: &Coord3D, range: f32, mask: u32) {
        self.draw_shrouder(pos, range, mask, false);
    }

    fn draw_shrouder(&mut self, pos: &Coord3D, range: f32, mask: u32, add: bool) {
        let inv = 1.0 / self.cell_size;
        let (cx, cy) = (real_to_int_floor(pos.x * inv), real_to_int_floor(pos.y * inv));
        let cell_radius = real_to_int_ceil(range * inv).max(1);
        let cells = discrete_circle_cells(cx, cy, cell_radius);
        for p in mask_players(mask) {
            let g = self.grid_for(p);
            for &(x, y) in &cells {
                if x >= 0 && y >= 0 && x < g.count_x && y < g.count_y {
                    let cell = &mut g.cells[(y * g.count_x + x) as usize];
                    if add {
                        cell.add_shrouder();
                    } else {
                        cell.remove_shrouder();
                    }
                }
            }
        }
    }

    fn grid_for(&mut self, player: PlayerIndex) -> &mut ShroudGrid {
        let (cx, cy, cs) = (self.count_x, self.count_y, self.cell_size);
        self.grids.entry(player).or_insert_with(|| ShroudGrid::new(cx, cy, cs))
    }

    /// Advance the shroud by one frame: flush due unlooks, then reconcile each object's look with
    /// `desired` (`Object::look` / `unlook`). `desired` is keyed by object and already filtered to
    /// live, non-contained objects with a positive clearing range; `now` is the logic frame.
    pub fn update(&mut self, now: u32, desired: &BTreeMap<ObjectId, Vec<Sighting>>) {
        self.flush_pending(now);

        let mut ids: BTreeSet<ObjectId> = BTreeSet::new();
        ids.extend(self.looks.keys().copied());
        ids.extend(desired.keys().copied());

        for id in ids {
            let old = self.looks.get(&id).cloned();
            let new = desired.get(&id).cloned();
            if old.is_none() && new.is_none() {
                continue;
            }
            if let (Some(old), Some(new)) = (&old, &new) {
                if same_look(old, new, self.cell_size) {
                    continue;
                }
            }
            // `Object::unlook`: a changed/removed look is queued for a delayed undo, so the reveal
            // trails for `UnlookPersistDuration` after the unit moves on.
            if let Some(old) = old {
                for s in old {
                    self.pending.push((now.saturating_add(UNLOOK_PERSIST_FRAMES), s));
                }
            }
            // `Object::look`: the new look applies immediately.
            match new {
                Some(new) => {
                    for s in &new {
                        self.add_looker(&s.pos, s.range, s.mask);
                    }
                    self.looks.insert(id, new);
                }
                None => {
                    self.looks.remove(&id);
                }
            }
        }
    }

    fn flush_pending(&mut self, now: u32) {
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].0 <= now {
                let (_, s) = self.pending.remove(i);
                self.remove_looker(&s.pos, s.range, s.mask);
            } else {
                i += 1;
            }
        }
    }

    /// Number of queued undo-look entries (for tests / diagnostics).
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub(crate) fn crc(&self, c: &mut crate::crc::Crc) {
        c.add_f32(self.cell_size);
        c.add_u32(self.grids.len() as u32);
        for (p, g) in &self.grids {
            c.add_u32(u32::from(*p));
            c.add_u32(g.count_x as u32);
            c.add_u32(g.count_y as u32);
            c.add_bool(g.revealed);
            c.add_u32(g.cells.len() as u32);
            for cell in &g.cells {
                c.add_i32(i32::from(cell.current));
                c.add_i32(i32::from(cell.active));
            }
        }
        // The undo queue affects future grid state, so hash it in insertion order.
        c.add_u32(self.pending.len() as u32);
        for (frame, s) in &self.pending {
            c.add_u32(*frame);
            c.add_coord(&s.pos);
            c.add_f32(s.range);
            c.add_u32(s.mask);
        }
    }
}

impl crate::logic::GameLogic {
    /// `PartitionManager::revealMapForPlayer` for `player` (see [`Shroud::reveal_map`]).
    pub fn reveal_map_for_player(&mut self, player: PlayerIndex) {
        self.shroud.reveal_map(player);
    }

    /// `PartitionManager::revealMapForPlayerPermanently`: observer slots (`GameLogic.cpp:1792`,
    /// `1809`) see the whole map and generate no shroud.
    pub fn reveal_map_permanently_for_player(&mut self, player: PlayerIndex) {
        self.shroud.reveal_map_permanently(player);
    }
}

fn same_look(a: &[Sighting], b: &[Sighting], cell_size: f32) -> bool {
    let inv = 1.0 / cell_size;
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| x.mask == y.mask && x.range == y.range && x.cell_key(inv) == y.cell_key(inv))
}

/// The bit for `player`, or 0 for indices the 32-bit mask cannot hold.
fn player_bit(player: PlayerIndex) -> u32 {
    if u32::from(player) < 32 {
        1u32 << player
    } else {
        0
    }
}

/// Players whose bit is set in `mask`, low to high.
fn mask_players(mask: u32) -> impl Iterator<Item = PlayerIndex> {
    (0..32u8).filter(move |p| mask & player_bit(*p) != 0)
}

/// `DiscreteCircle` (`Common/DiscreteCircle.cpp`): the set of cells in the filled circle of
/// `radius` around `(cx, cy)`, generated with Bresenham and mirrored across the centre row.
/// Returned sorted and de-duplicated so each cell is touched exactly once.
pub fn discrete_circle_cells(cx: i32, cy: i32, radius: i32) -> Vec<(i32, i32)> {
    let radius = radius.max(0);
    let mut edges: Vec<(i32, i32, i32)> = Vec::new(); // (y, xStart, xEnd), top half
    let mut x = 0i32;
    let mut y = radius;
    let mut d = (1 - radius) << 1;
    while y >= 0 {
        edges.push((cy + y, cx - x, cx + x));
        if d + y > 0 {
            y -= 1;
            d -= (y << 1) - 1;
        }
        if x > d {
            x += 1;
            d += (x << 1) + 1;
        }
    }
    // `removeDuplicates`: drop a row when it repeats the previous row's y.
    let mut deduped: Vec<(i32, i32, i32)> = Vec::with_capacity(edges.len());
    for e in edges {
        if deduped.last().map(|p| p.0) != Some(e.0) {
            deduped.push(e);
        }
    }
    let mut cells: BTreeSet<(i32, i32)> = BTreeSet::new();
    for (yp, xs, xe) in deduped {
        for xx in xs..=xe {
            cells.insert((xx, yp));
            if yp != cy {
                cells.insert((xx, 2 * cy - yp));
            }
        }
    }
    cells.into_iter().collect()
}

/// The sim's view of an object's shroud contribution, computed by [`GameLogic::update_shroud`].
///
/// `Object::getShroudClearingRange` (`Object.cpp:5237-5265`): the object's live `m_shroudClearingRange` (the
/// template's `ShroudClearingRange`, or `VisionRange` when that is -1, changed by `DynamicShroudClearingRangeUpdate`'s
/// `setShroudClearingRange`), except that a structure under construction only sees its own bounding circle.
pub(crate) fn shroud_clearing_range(obj: &Object) -> f32 {
    if obj.under_construction {
        return obj.template.geometry.bounding_circle_radius();
    }
    obj.shroud_clearing_range
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circle_is_symmetric_and_has_expected_extents() {
        let cells = discrete_circle_cells(0, 0, 2);
        assert!(cells.contains(&(0, 0)));
        assert!(cells.contains(&(2, 0)));
        assert!(cells.contains(&(-2, 0)));
        assert!(cells.contains(&(0, 2)));
        assert!(cells.contains(&(0, -2)));
        assert!(!cells.contains(&(3, 0)));
        // Mirror symmetry about both axes.
        for &(x, y) in &cells {
            assert!(cells.contains(&(-x, y)));
            assert!(cells.contains(&(x, -y)));
            assert!(cells.contains(&(-x, -y)));
        }
    }

    #[test]
    fn cell_counters_follow_original_semantics() {
        let mut g = ShroudGrid::new(8, 8, 40.0);
        assert_eq!(g.status(0, 0), CellShroudStatus::Shrouded);
        g.draw_looker(&Coord3D::new(20.0, 20.0, 0.0), 20.0, true);
        assert_eq!(g.status(0, 0), CellShroudStatus::Clear);
        // Removing the only looker leaves fog, not shroud (memory).
        g.draw_looker(&Coord3D::new(20.0, 20.0, 0.0), 20.0, false);
        assert_eq!(g.status(0, 0), CellShroudStatus::Fogged);
    }

    #[test]
    fn shrouder_overrides_fog_but_not_clear() {
        let mut g = ShroudGrid::new(4, 4, 40.0);
        let pos = Coord3D::new(20.0, 20.0, 0.0);
        // Clear a cell, then remove the looker: fogged.
        g.draw_looker(&pos, 20.0, true);
        g.draw_looker(&pos, 20.0, false);
        assert_eq!(g.status(0, 0), CellShroudStatus::Fogged);
        // A shrouder over a fogged cell makes it shrouded again; removing the shrouder only drops
        // the active level, so the cell stays shrouded until a looker clears it again
        // (`PartitionCell::removeShrouder` never restores fog).
        g.cells[0].add_shrouder();
        assert_eq!(g.status(0, 0), CellShroudStatus::Shrouded);
        g.cells[0].remove_shrouder();
        assert_eq!(g.status(0, 0), CellShroudStatus::Shrouded);
        // A looker clears it again; a shrouder over a cleared cell does not fog it.
        g.draw_looker(&pos, 20.0, true);
        g.cells[0].add_shrouder();
        assert_eq!(g.status(0, 0), CellShroudStatus::Clear);
    }

    #[test]
    fn object_status_uses_touched_cells() {
        let mut s = Shroud::new(8, 8, 40.0);
        let pos = Coord3D::new(20.0, 20.0, 0.0);
        // Nothing seen yet: shrouded.
        assert_eq!(s.object_status(1, &pos, 1.0, false, false, false), ObjectShroudStatus::Shrouded);
        // A looker for player 1 clears the cell.
        s.add_looker(&pos, 40.0, 1 << 1);
        assert_eq!(s.object_status(1, &pos, 1.0, false, false, false), ObjectShroudStatus::Clear);
        // Player 2 sees nothing.
        assert_eq!(s.object_status(2, &pos, 1.0, false, false, false), ObjectShroudStatus::Shrouded);
        // A mobile object in fog is shrouded; an immobile one is fogged.
        s.remove_looker(&pos, 40.0, 1 << 1);
        assert_eq!(s.object_status(1, &pos, 1.0, false, false, false), ObjectShroudStatus::Shrouded);
        assert_eq!(s.object_status(1, &pos, 1.0, true, false, false), ObjectShroudStatus::Fogged);
    }

    /// `revealMapForPlayer` (skirmish start, `UseShroud = No`): every cell becomes fog, not shroud,
    /// and shroud generation still works afterwards (`revealMapForPlayerPermanently` would not).
    #[test]
    fn start_reveal_fogs_the_whole_map_and_keeps_generation() {
        let mut s = Shroud::new(8, 8, 40.0);
        assert_eq!(s.cell_status(1, 3, 3), CellShroudStatus::Shrouded);
        s.reveal_map(1);
        assert_eq!(s.cell_status(1, 0, 0), CellShroudStatus::Fogged);
        assert_eq!(s.cell_status(1, 7, 7), CellShroudStatus::Fogged);
        // Only the revealed player; another player still starts shrouded.
        assert_eq!(s.cell_status(2, 3, 3), CellShroudStatus::Shrouded);
        // A unit's look clears its cells and they fall back to fog once it leaves.
        let pos = Coord3D::new(100.0, 100.0, 0.0);
        s.add_looker(&pos, 40.0, 1 << 1);
        assert_eq!(s.cell_status_at(1, &pos), CellShroudStatus::Clear);
        s.remove_looker(&pos, 40.0, 1 << 1);
        assert_eq!(s.cell_status_at(1, &pos), CellShroudStatus::Fogged);
        // Permanent reveal (observers) is clear everywhere.
        s.reveal_map_permanently(2);
        assert_eq!(s.cell_status(2, 0, 0), CellShroudStatus::Clear);
    }

    #[test]
    fn look_trails_after_move_then_fades() {
        let mut s = Shroud::new(8, 8, 40.0);
        let id = ObjectId(1);
        let here = Coord3D::new(20.0, 20.0, 0.0);
        let mut desired = BTreeMap::new();
        desired.insert(id, vec![Sighting { pos: here, range: 20.0, mask: 1 << 1 }]);
        s.update(0, &desired);
        assert_eq!(s.cell_status(1, 0, 0), CellShroudStatus::Clear);
        // Move far away: the old look is queued, not removed immediately.
        let there = Coord3D::new(300.0, 300.0, 0.0);
        desired.insert(id, vec![Sighting { pos: there, range: 20.0, mask: 1 << 1 }]);
        s.update(1, &desired);
        assert_eq!(s.cell_status(1, 0, 0), CellShroudStatus::Clear);
        // After the persist window it becomes fog.
        s.update(1 + UNLOOK_PERSIST_FRAMES, &desired);
        assert_eq!(s.cell_status(1, 0, 0), CellShroudStatus::Fogged);
        assert_eq!(s.cell_status(1, 7, 7), CellShroudStatus::Clear);
    }
}
