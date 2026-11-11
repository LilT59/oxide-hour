//! Ground pathfinder: the cell grid, static obstacles, A* and path optimisation.
//!
//! Ported (scoped) from `Pathfinder` in Core/GameEngine/Source/GameLogic/AI/AIPathfind.cpp and
//! Core/GameEngine/Include/GameLogic/AIPathfind.h (+ `Pathfinder/PathfindCell.h`), following the
//! TheSuperHackers fork with its default `RETAIL_COMPATIBLE_PATHFINDING` behaviour (pinched cells
//! beside a structure become `CELL_IMPASSABLE`).
//!
//! | Original | Here |
//! |---|---|
//! | `Pathfinder::newMap` / `classifyMap` / `classifyMapCell` | [`Pathfinder::new`] (clear / cliff / water, cliff expansion and "pinched" rings) |
//! | `classifyObjectFootprint` / `internal_classifyObjectFootprint` | [`Pathfinder::classify_object_footprint`] |
//! | `PathfindCell::costSoFar` / `costToGoal` | `cost_so_far` / `cost_to_goal` (10 straight, 14 diagonal, +4/8/16 per turn) |
//! | open list (`forwardInsertionSort`, FIFO among equal totals) | a `BTreeSet<(total, sequence, cell)>` with the same order |
//! | `examineNeighboringCells` + `examineCellsCallback` | `examine_neighboring_cells` (including the straight-line pre-pass to the goal) |
//! | `findPath` / `internalFindPath` | [`Pathfinder::find_path`] |
//! | `findClosestPath` | [`Pathfinder::find_closest_path`] (goal unreachable: go to the closest reachable cell) |
//! | `AIUpdateInterface::computePath` | [`Pathfinder::compute_move_path`] |
//! | `buildActualPath` / `prependCells` / `Path::optimize` | [`Path`] (line-of-sight smoothing with the original's diagonal/straight heuristics) |
//! | `iterateCellsAlongLine` (Bresenham), `ClipLine2D`, `getRadiusAndCenter` | same names, snake case |
//!
//! # Walls, zones and hierarchical paths
//!
//! * **Zones / hierarchical pathfinding** (`PathfindZoneManager`, `findHierarchicalPath`): the zone
//!   model lives in [`crate::zone::ZoneManager`], ported from `calculateZones` — the raw same-type
//!   cell zones and the six equivalency tables (`m_hierarchicalZones`, `m_terrainZones`,
//!   `m_groundWaterZones`, `m_groundRubbleZones`, `m_groundCliffZones`, `m_crusherZones`) — with
//!   `getEffectiveZone` / `getEffectiveTerrainZone` answering `clientSafeQuickDoesPathExist`
//!   (`AIPathfind.cpp:8239-8290`, including its `doingTerrainZone` remap for cells inside a
//!   structure). [`Pathfinder::internal_find_path`] repeats the screen before the fine A*
//!   (`:6684-6715`: a wall start/goal with no zone, or two different effective zones, means no path —
//!   the caller then falls back to `findClosestPath`). [`Pathfinder::find_hierarchical_path`] /
//!   [`Pathfinder::find_closest_hierarchical_path`] are the engine's coarse search:
//!   `internal_findHierarchicalPath` over the [`ZONE_BLOCK_SIZE`](crate::zone::ZONE_BLOCK_SIZE) zone
//!   blocks (`processHierarchicalCell`'s four border scans, `ZoneBlock::getEffectiveZone` via
//!   [`crate::zone::ZoneManager::block_zone`]), with `buildHierarchicalPath` marking the route's
//!   blocks plus the 21×21-cell box around the start passable. The fine A* then pays
//!   `100*COST_ORTHOGONAL` for a cell outside the corridor and stops its straight-line pre-pass at
//!   one (`examineCellsCallback`'s `isPassable` test). `find_path`/`find_closest_path` clear the
//!   flags first and `setAllPassable` when the coarse search finds nothing. The coarse search can
//!   only leave a block through a cell that already carries a `PathfindCellInfo`
//!   (`newCell->hasInfo()`, `:7464-7467`) — a unit, a goal, an obstacle, or a cell the search
//!   allocated — so on empty ground it fails and the fine A* is unconstrained; that is the retail
//!   behaviour, not an omission.
//!   The **zone-calculation schedule** is ported as its own piece: `markZonesDirty`
//!   (`AIPathfind.cpp:2641-2652`) sets the next zone-calculation frame, and `processPathfindQueue`
//!   spends that frame re-zoning instead of pathing (`:6018-6022`). Only the engine's `markZonesDirty`
//!   sites schedule (`:4271-4274` fence that changed a cell, `:4389` box, `:4463` sphere/cylinder,
//!   `:9837-9839` a bridge whose destroyed state flipped); `classifyMap` (`:4814`) ends in
//!   `calculateZones` and the wall layer marks nothing.
//! * **Walls** (`LAYER_WALL`): [`Pathfinder::add_wall_piece`] / [`remove_wall_piece`] maintain the
//!   piece list and rebuild [`Pathfinder::wall_extent`] lazily; the wall cells are classified by
//!   the four-corner test (`PathfindLayer::classifyWallMapCell`, retail-compatible `CELL_CLIFF`
//!   edges) and tightened by one cell. A unit on a wall is detected by z
//!   ([`Pathfinder::layer_for_destination`]) and its A* runs on the wall grid (swapped into `cells`
//!   by [`Pathfinder::on_layer`]), with waypoints at the wall height. Bridges remain a ground-grid
//!   deck overlay ([`Pathfinder::apply_bridges`]): the ground under a bridge over passable terrain
//!   stays reachable (pass-under). Reduced: walls have no ground `connectLayer` in the engine
//!   either, so there are no ground<->wall transitions; `checkChangeLayers` for bridge layers is
//!   not ported.
//! * **Dynamic unit avoidance**: `updatePos` / `removePos` position cells, goal cells, the
//!   `CellFlags` word `setPosUnit` / `setGoalUnit` derive for each cell (`AIPathfind.cpp:1420-1531`:
//!   `UNIT_PRESENT_FIXED` means the unit stands on its own goal cell, not that it has stopped),
//!   the `adjustDestination` spiral, and [`Pathfinder::check_for_movement_full`]
//!   (`checkForMovement`: `enemyFixed`, `allyFixedCount`, `allyMoving`, `allyGoal`;
//!   `INFANTRY_MOVES_THROUGH_INFANTRY`; the ally table from `GameLogic`'s relationships). Fixed
//!   allies on a route add `3*COST_DIAGONAL` and mark the cell `blockedByAlly`; the straight-line
//!   pre-pass aborts on either a fixed ally or enemy (the `RETAIL_COMPATIBLE_PATHFINDING` branch),
//!   and `isLinePassable` bails on fixed units so `Path::optimize` does not straighten through them.
//!   [`Pathfinder::move_allies`] then asks the idle fixed allies on the built path to step aside,
//!   computing each one's route with [`Pathfinder::find_move_away_path`]
//!   (`getMoveAwayFromPath`); `GameLogic` applies the queued paths next frame.
//!   [`Pathfinder::patch_path`] is a port of `patchPath`; `AIUpdateInterface::computePath` calls
//!   it (through [`Pathfinder::compute_ai_path`]) when the unit is `isBlockedAndStuck`. Aircraft that
//!   adjust their destination keep a separate goal mark (`setGoalAircraft`,
//!   `update_aircraft_goal`), which a ground mover never reads.
//! * **Group moves**: [`Pathfinder::compute_move_path_group`] passes the group destination through
//!   `adjustDestination`, which tightens the reserved cell toward it (`tightenPath`) and rejects
//!   cells whose route from the group destination is too long (`checkPathCost`); `orders.rs` /
//!   `logic.rs` offset each unit by its position relative to the group centre
//!   (`AIGroup::computeIndividualDestination`).
//! * **Attack paths**: [`Pathfinder::find_attack_path`] (`findAttackPath`) searches for a cell the
//!   weapon can fire on the victim from, with the `isAttackViewBlockedByObstacle` obstacle pass
//!   (the weapon terrain line-of-sight test is not ported). Used by `combat.rs`'s chase.
//!   `findSafePath` is not ported (the sim has no repulsor requests). Aircraft quick paths are
//!   [`Pathfinder::aircraft_path`] (`getAircraftPath`): a two-node path with a `LOCO_WINGS`
//!   destination clip and `KINDOF_AIRCRAFT_PATH_AROUND` detours.
//! * **Async pathfind queue**: [`Pathfinder::queue_for_path`] / [`Pathfinder::pop_queued_path`]
//!   reproduce `queueForPath` / `processPathfindQueue` (`AIPathfind.cpp:5761/5989`), and
//!   `cumulative_cells_allocated` enforces `PATHFIND_CELLS_PER_FRAME`. `GameLogic` services the
//!   queue after the sleepy updates (`AI::update`), in insertion order, one `doPathfind` per
//!   request until the cell budget is spent. `AIUpdateInterface::requestPath` /
//!   `requestAttackPath` / `requestApproachPath` queue; the first path and every repath are serviced
//!   over frames. The fine A*'s neighbour expansion hard-aborts on a fixed enemy
//!   (`AIPathfind.cpp:6332`) like the engine; the AIUpdate collision/repath machinery
//!   ([`crate::ai`]) is what keeps a blocked group moving.
//! * **Other**: `classifyMapCell` marks a cell `CELL_WATER` when any corner is underwater (the
//!   map's water-area polygons). **Fences** (`FenceWidth > 0`, non-`DEFENSIVE_WALL`) classify a thin
//!   oriented strip ([`Pathfinder::classify_fence`]); `KINDOF_MINE` / `PROJECTILE` / `BRIDGE_TOWER`
//!   footprints are excluded. The **dozer exception** in `examineNeighboringCells` is ported
//!   (`AIPathfind.cpp:6384-6399`): a `KINDOF_DOZER` treats an obstacle cell whose owner it is not at
//!   war with as a valid movement position (so it can tunnel out of its own building), with
//!   `dozerHack` keeping the cell out of the diagonal adjacency flags. The obstacle→owner and
//!   enemy tables are refreshed from `GameLogic` (`set_obstacle_owners` / `set_enemies`), because a
//!   `PathfindCell` stores only the obstacle id. Rubble (`BODY_RUBBLE` obstacles) and the "ultra
//!   accurate" last node are not modelled. `checkForAdjust`'s `isHuman` logical-extent test
//!   (`AIPathfind.cpp:5324-5330`) needs no code: the port's grid *is* `m_logicalExtent` and
//!   `get_cell` already rejects every cell outside it (the sim has no `PlayerType`).
//!   The `PathfindCellInfo` pool is the 30000-cell limit per search.
//!
//! All arithmetic is integer or plain `f32`; transcendental functions go through `libm`.

use crate::crc::Crc;
use crate::math::{cos, real_to_int_ceil, real_to_int_floor, sin, sqrt, Coord3D};
use crate::object::ObjectId;
use crate::template::{kind_of, Geometry, GeometryShape, LocomotorParams, ObjectTemplate};
use crate::los::AttackView;
use crate::terrain::TerrainLogic;
use std::collections::{BTreeMap, BTreeSet};

/// `PATHFIND_CELL_SIZE`.
pub const PATHFIND_CELL_SIZE: i32 = 10;
/// `PATHFIND_CELL_SIZE_F`.
pub const PATHFIND_CELL_SIZE_F: f32 = 10.0;
/// `CELL_INFOS_TO_ALLOCATE`: maximum cells one search may touch.
const CELL_INFOS_TO_ALLOCATE: usize = 30000;
/// `PATHFIND_CELLS_PER_FRAME` (`AIPathfind.cpp:119`): the per-frame cell budget of the async
/// pathfind queue.
pub const PATHFIND_CELLS_PER_FRAME: u32 = 5000;
/// `MAX_ADJUSTMENT_CELL_COUNT`: how many cells `adjustDestination`'s spiral may visit.
const MAX_ADJUSTMENT_CELL_COUNT: i32 = 400;
const COST_ORTHOGONAL: i32 = 10;
const COST_DIAGONAL: i32 = 14;
const NONE: u32 = u32::MAX;

/// `PathfindCell::costToHierGoal` (Core/GameEngine/Source/GameLogic/AI/AIPathfind.cpp:2079-2090, shipped
/// copy :1702): `REAL_TO_INT_FLOOR(COST_ORTHOGONAL*sqrt(dx*dx + dy*dy) + 0.5f)` for the cell offset
/// `(dx, dy)`. `dx*dx + dy*dy` is an `Int`, and `sqrt(Int)` is the double overload (the integral overload
/// of `<cmath>` returns `double`; a `float` overload would need a conversion and is not chosen), so the
/// sum is a double, and it narrows to `float` before `fast_float_floor`.
fn cost_to_hier_goal(dx: i32, dy: i32) -> i32 {
    let d = COST_ORTHOGONAL as f64 * ((dx * dx + dy * dy) as f64).sqrt() + 0.5;
    real_to_int_floor(d as f32)
}

/// `Coord2D::length()` (Core/Libraries/Include/Lib/Coord2D.h:41).
fn norm2d(v: (f32, f32)) -> f32 {
    sqrt(v.0 * v.0 + v.1 * v.1)
}

/// `Coord2D::normalize()` (`Coord2D.h:44-52`): a zero vector stays zero.
fn normalize2d(v: (f32, f32)) -> (f32, f32) {
    let len = norm2d(v);
    if len != 0.0 { (v.0 / len, v.1 / len) } else { v }
}

/// `computeNormalRadialOffset` (`AIPathfind.cpp:9538-9566`): the point at `radius` from `obj_pos` along
/// the normal of the line `from`..`to` that points away from the object. The object's z becomes the
/// point's z.
fn compute_normal_radial_offset(from: &Coord3D, insert: &mut Coord3D, to: &Coord3D, obj_pos: &Coord3D, radius: f32) {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let (obj_dx, obj_dy) = (obj_pos.x - from.x, obj_pos.y - from.y);
    let cross = dx * obj_dy - dy * obj_dx;
    let normal = if cross > 0.0 { normalize2d((dy, -dx)) } else { normalize2d((-dy, dx)) };
    *insert = Coord3D::new(obj_pos.x + normal.0 * radius, obj_pos.y + normal.1 * radius, obj_pos.z);
}

/// `LocomotorSurfaceType` bits.
pub mod surface {
    pub const GROUND: u32 = 1 << 0;
    pub const WATER: u32 = 1 << 1;
    pub const CLIFF: u32 = 1 << 2;
    pub const AIR: u32 = 1 << 3;
    pub const RUBBLE: u32 = 1 << 4;
}

/// `PathfindLayerEnum` (`Common/GameType.h`): only the ground grid and the single wall layer are
/// modelled. Bridges are not a separate layer here — a healthy bridge overlays the ground grid's
/// deck ([`Pathfinder::apply_bridges`]) — but walls are, because the cells on top of a wall are a
/// distinct passability surface at the wall height.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PathLayer {
    #[default]
    Ground = 1,
    Wall = 15,
}

/// `PathfindCell::CellType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellType {
    Clear = 0,
    Water = 1,
    Cliff = 2,
    Rubble = 3,
    Obstacle = 4,
    BridgeImpassable = 5,
    Impassable = 6,
}

/// `Pathfinder::validLocomotorSurfacesForCellType`.
fn surfaces_for_cell_type(t: CellType) -> u32 {
    match t {
        CellType::Clear => surface::GROUND | surface::AIR,
        CellType::Water => surface::WATER | surface::AIR,
        CellType::Cliff => surface::CLIFF | surface::AIR,
        CellType::Rubble => surface::RUBBLE | surface::AIR,
        CellType::Obstacle | CellType::BridgeImpassable | CellType::Impassable => surface::AIR,
    }
}

/// `IS_IMPASSABLE`.
fn is_impassable(t: CellType) -> bool {
    matches!(t, CellType::Impassable | CellType::Obstacle | CellType::BridgeImpassable)
}

/// What the pathfinder needs to know about the moving object (`Object` + `LocomotorSet`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PathObject {
    /// `Object::getID()`, so the unit's own cells are skipped and its goal can be reserved.
    /// [`ObjectId::INVALID`] means "anonymous" (no unit cells): all goal/position bookkeeping is
    /// disabled, which is what the raw path tests use.
    pub id: ObjectId,
    /// `LocomotorSet::getValidSurfaces`.
    pub surfaces: u32,
    /// `GeometryInfo::getBoundingCircleRadius`.
    pub bounding_radius: f32,
    /// `Object::getLayer()`: the layer the unit starts on (and, for walls, paths on). Ground for
    /// everything except units already standing on a wall (`LAYER_WALL`).
    pub layer: PathLayer,
    /// `Object::getControllingPlayer`'s index, for the ally/enemy tests in
    /// [`Pathfinder::check_for_movement_full`].
    pub player: crate::object::PlayerIndex,
    /// `KINDOF_INFANTRY`: infantry may walk through other infantry (`INFANTRY_MOVES_THROUGH_INFANTRY`).
    pub infantry: bool,
    /// `Object::getCrusherLevel() > 0`: a crusher may enter cells held by enemy units.
    pub crusher: bool,
    /// `KINDOF_DOZER`: dozers push allies even when their path is not `blockedByAlly`
    /// (`Pathfinder::moveAllies`, `AIPathfind.cpp:10198`).
    pub dozer: bool,
    /// `KINDOF_HARVESTER`: harvesters push allies even when their path is not `blockedByAlly`.
    pub harvester: bool,
    /// `AIUpdateInterface::isAircraftThatAdjustsDestination()` (`AIUpdate.cpp:2408-2429`): the
    /// **current** locomotor's appearance decides — `LOCO_HOVER` and `LOCO_WINGS` adjust, `LOCO_THRUST`
    /// and any other appearance do not, and no locomotor at all does not. `KINDOF_AIRCRAFT` and
    /// `isDoingGroundMovement` are not part of the test, so a taxiing jet (its `SET_TAXIING` wheels
    /// locomotor) is false while the same jet's `SET_NORMAL` wings locomotor is true. Filled from the
    /// locomotor passed to [`for_locomotor`](Self::for_locomotor); [`for_object`](Self::for_object)
    /// passes the live current one.
    pub aircraft_that_adjusts_destination: bool,
    /// `Object::isKindOf(KINDOF_AIRCRAFT)`: `checkForAdjust`'s path gate is skipped for an aircraft
    /// (`AIPathfind.cpp:5334-5338`, `adjustedPathExists = true`) whatever its locomotor's surfaces.
    pub is_aircraft: bool,
    /// `AIUpdateInterface::canPathThroughUnits()`; when set, allies block the path but are not
    /// flagged for `moveAllies` (`AIPathfind.cpp:6377`).
    pub can_path_through_units: bool,
    /// `AIUpdateInterface::getIgnoredObstacleID()`: the obstacle this unit is allowed to enter
    /// (`checkDestination`, `AIPathfind.cpp:5023`; `checkForMovement`, `:5081`). This is *not* the
    /// pathfinder's own `m_ignoreObstacleID` (which `computePath` sets around the search,
    /// `AIUpdate.cpp:1722`/`:1755` and which `validMovementPosition`, `AIPathfind.cpp:4993`, reads).
    pub ignore_obstacle: ObjectId,
    /// `AIUpdateInterface::isAttacking()` (`AIUpdate.cpp:3131`), read by `moveAllies`' ZH vetoes
    /// (`AIPathfind.cpp:10267-10275`). The engine asks the live object; the port's pathfinder is a
    /// separate structure, so `GameLogic` refreshes these three per frame with the unit's occupancy
    /// (`Pathfinder::updatePos`'s loop).
    pub attacking: bool,
    /// `AIUpdateInterface::isBusy()` (`AIUpdate.cpp:3215`: the machine's current state is
    /// `AIBusyState`), for the same veto.
    pub busy: bool,
    /// `Object::testStatus(OBJECT_STATUS_IS_USING_ABILITY)`, for the same veto.
    pub using_ability: bool,
}

impl PathObject {
    /// Surfaces a locomotor can drive on (`LocomotorTemplate::getValidSurfaces`). The adapter fills
    /// the bits from the INI `Surfaces` list; tests that build [`LocomotorParams`] directly get the
    /// appearance default. A climber (`CLIFF` only) paths on cliffs, a boat (`WATER` only) on water.
    pub fn surfaces_for(l: &LocomotorParams) -> u32 {
        let mut s = 0;
        if l.surfaces_ground {
            s |= surface::GROUND;
        }
        if l.surfaces_water {
            s |= surface::WATER;
        }
        if l.surfaces_cliff {
            s |= surface::CLIFF;
        }
        if l.surfaces_air {
            s |= surface::AIR;
        }
        if l.surfaces_rubble {
            s |= surface::RUBBLE;
        }
        s
    }

    pub fn for_template(t: &ObjectTemplate, id: ObjectId, player: crate::object::PlayerIndex) -> Option<Self> {
        let l = t.locomotor.as_ref()?;
        Self::for_locomotor(t, l, id, player)
    }

    /// The pathfinder's view of a live object: the **current** locomotor
    /// (`AIUpdateInterface::getCurLocomotor`, `m_locomotorSet` of the object's `m_curLocomotorSet`) gives
    /// the surfaces and the `isAircraftThatAdjustsDestination` appearance, as `getLocomotorSet()` /
    /// `getCurLocomotor()` do in the engine. [`for_template`](Self::for_template) uses `SET_NORMAL`, which
    /// is wrong for an object on another set (a taxiing jet drives `SET_TAXIING`).
    pub fn for_object(o: &crate::object::Object) -> Option<Self> {
        let l = o.locomotor()?;
        Self::for_locomotor(&o.template, &l, o.id, o.player)
    }

    /// [`for_template`](Self::for_template) with the locomotor the unit's AI uses (a `SET_x` locomotor set,
    /// `AIUpdateInterface::getLocomotorSet`, `AIUpdate.cpp`), for the surfaces.
    pub fn for_locomotor(t: &ObjectTemplate, l: &LocomotorParams, id: ObjectId, player: crate::object::PlayerIndex) -> Option<Self> {
        // `KINDOF_PROJECTILE` objects (flares, bombs) fly and never block ground pathing.
        if t.kind_of & crate::template::kind_of::PROJECTILE != 0 {
            return None;
        }
        Some(Self {
            id,
            surfaces: Self::surfaces_for(l),
            bounding_radius: t.geometry.bounding_circle_radius(),
            layer: PathLayer::Ground,
            player,
            infantry: t.kind_of & crate::template::kind_of::INFANTRY != 0,
            crusher: t.crush.crusher_level > 0,
            dozer: t.kind_of & crate::template::kind_of::DOZER != 0,
            harvester: t.kind_of & crate::template::kind_of::HARVESTER != 0,
            aircraft_that_adjusts_destination: matches!(l.kind, crate::template::LocomotorKind::Hover | crate::template::LocomotorKind::Wings),
            is_aircraft: t.kind_of & crate::template::kind_of::AIRCRAFT != 0,
            can_path_through_units: false,
            ignore_obstacle: ObjectId::INVALID,
            attacking: false,
            busy: false,
            using_ability: false,
        })
    }

    /// `Pathfinder::getRadiusAndCenter`: footprint radius in cells and whether it is centred on a cell.
    pub fn radius_and_center(&self) -> (i32, bool) {
        const MAX_RADIUS: i32 = 2;
        let mut diameter = 2.0 * self.bounding_radius;
        if diameter > PATHFIND_CELL_SIZE_F && diameter < 2.0 * PATHFIND_CELL_SIZE_F {
            diameter = 2.0 * PATHFIND_CELL_SIZE_F;
        }
        let mut radius = real_to_int_floor(diameter / PATHFIND_CELL_SIZE_F + 0.3);
        let mut center = false;
        if radius == 0 {
            radius += 1;
        }
        if radius & 1 != 0 {
            center = true;
        }
        radius /= 2;
        if radius > MAX_RADIUS {
            radius = MAX_RADIUS;
            center = true;
        }
        (radius, center)
    }
}

/// `PathfindCell::CellFlags` (`Core/GameEngine/Include/GameLogic/Pathfinder/PathfindCell.h:53-60`):
/// one flag word derived from the cell's position unit (`setPosUnit`) and goal unit (`setGoalUnit`).
/// `UNIT_PRESENT_FIXED` means the unit standing here is *on its own goal cell* (`posUnit == goalUnit`),
/// not that it has stopped; a stationary unit away from its goal is `UNIT_PRESENT_MOVING`, the
/// engine's name for "present, not on its goal".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CellFlags {
    /// `NO_UNITS`.
    NoUnits = 0x00,
    /// `UNIT_GOAL`: a unit is heading here, nobody is here.
    UnitGoal = 0x01,
    /// `UNIT_PRESENT_MOVING`: a unit is here and no unit owns this cell as its goal.
    UnitPresentMoving = 0x02,
    /// `UNIT_PRESENT_FIXED`: the unit here is on its own goal cell.
    UnitPresentFixed = 0x03,
    /// `UNIT_GOAL_OTHER_MOVING`: a unit is here and another unit owns this cell as its goal.
    UnitGoalOtherMoving = 0x05,
}

/// `PathfindCell` (the static part).
#[derive(Clone, Copy, Debug)]
struct Cell {
    /// Effective type (a healthy bridge over impassable ground makes the cell passable).
    ty: CellType,
    /// The terrain type before any bridge override (`PathfindLayer` vs ground).
    base_ty: CellType,
    pinched: bool,
    /// `m_obstacleID` (0 = none).
    obstacle: u32,
    /// `m_obstacleIsFence`: the obstacle in this cell is a fence (`crusherGround`, `AIPathfind.cpp:2200`).
    fence: bool,
    /// `m_obstacleIsTransparent` (`AIPathfind.cpp:2020`): the obstacle has
    /// `KINDOF_CAN_SEE_THROUGH_STRUCTURE`, so it does not block a line of sight.
    obstacle_transparent: bool,
    /// `m_posUnit`: the mobile unit occupying this cell (`0` = none).
    pos_unit: u32,
    /// `Object::getControllingPlayer()` of `pos_unit`, for the ally/enemy tests.
    pos_player: crate::object::PlayerIndex,
    /// `KINDOF_INFANTRY` of `pos_unit` (`INFANTRY_MOVES_THROUGH_INFANTRY`).
    pos_infantry: bool,
    /// `m_goalUnit`: the unit whose destination footprint includes this cell (`0` = none).
    goal_unit: u32,
    /// `m_flags`, derived from `pos_unit`/`goal_unit` by `setPosUnit`/`setGoalUnit`.
    flags: CellFlags,
    /// `m_aircraftGoal`: an aircraft that adjusts its destination owns this cell as its goal
    /// (`setGoalAircraft`, `AIPathfind.cpp:1466-1490`). Ground units never read it.
    aircraft_goal: bool,
    /// `m_goalAircraftID` (`0` = none).
    goal_aircraft: u32,
    /// The bridge covering this cell (`0` = none); bridge layers are `index + 1`.
    bridge: u32,
    /// The bridge deck height at this cell.
    bridge_height: f32,
    /// The ground under the bridge is impassable, so a route uses the deck (`getLayerHeight`).
    bridge_over_impassable: bool,
}

/// A blank wall-layer cell of a given type (only `ty`, `pinched` and the deck height fields are
/// used by the wall layer).
fn wall_cell(ty: CellType) -> Cell {
    Cell { ty, base_ty: ty, pinched: false, obstacle: 0, fence: false, obstacle_transparent: false, pos_unit: 0, pos_player: 0, pos_infantry: false, goal_unit: 0, flags: CellFlags::NoUnits, aircraft_goal: false, goal_aircraft: 0, bridge: 0, bridge_height: 0.0, bridge_over_impassable: false }
}

/// A unit's occupied footprint in cell coordinates (`getRadiusAndCenter` + the cell it is in).
#[derive(Clone, Copy, Debug)]
struct Footprint {
    cx: i32,
    cy: i32,
    radius: i32,
    center: bool,
    /// `PathObject::player` (the footprint's owner), for the ally/enemy tests.
    player: crate::object::PlayerIndex,
    /// `PathObject::infantry` (`INFANTRY_MOVES_THROUGH_INFANTRY`).
    infantry: bool,
}

/// The cells a footprint covers: `cell-radius ..= cell+radius` (plus one when centered).
fn footprint_bounds(f: Footprint) -> (i32, i32, i32, i32) {
    let above = f.radius + i32::from(f.center);
    (f.cx - f.radius, f.cy - f.radius, f.cx + above, f.cy + above)
}

/// `PathfindCellInfo`: per-search data, allocated on demand.
#[derive(Clone, Copy, Debug)]
struct Info {
    has: bool,
    open: bool,
    closed: bool,
    parent: u32,
    total: u16,
    so_far: u16,
    open_key: (u16, u64),
    /// `PathfindCellInfo::m_blockedByAlly`: a fixed ally sat on this cell when the route reached
    /// it, so the built path is `blockedByAlly` (`AIPathfind.cpp:6377,9118`).
    blocked_by_ally: bool,
}

const NO_INFO: Info = Info { has: false, open: false, closed: false, parent: NONE, total: 0, so_far: 0, open_key: (0, 0), blocked_by_ally: false };

/// A node of a [`Path`] (`PathNode`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathNode {
    pub pos: Coord3D,
    pub can_optimize: bool,
    /// Index of the next node of the optimised path.
    pub next_optimized: Option<usize>,
}

/// `Path`: raw cell-centre nodes plus the optimised chain.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub nodes: Vec<PathNode>,
    /// `Path::m_blockedByAlly`: a fixed ally stood on one of the path's cells, so `moveAllies`
    /// must ask it to step aside (`Path.h`).
    pub blocked_by_ally: bool,
}

impl Path {
    /// Positions of the optimised path, excluding the first node (the unit's own position).
    pub fn waypoints(&self) -> Vec<Coord3D> {
        let mut out = Vec::new();
        let mut cur = self.nodes.first().and_then(|n| n.next_optimized);
        while let Some(i) = cur {
            out.push(self.nodes[i].pos);
            cur = self.nodes[i].next_optimized;
        }
        out
    }

    /// Positions of the optimised chain from the first node (`getFirstNode` -> `getNextOptimized`),
    /// the way `getMoveAwayFromPath` walks `pathToAvoid`.
    pub fn optimized_positions(&self) -> Vec<Coord3D> {
        let mut out = Vec::new();
        if let Some(first) = self.nodes.first() {
            out.push(first.pos);
            let mut cur = first.next_optimized;
            while let Some(i) = cur {
                out.push(self.nodes[i].pos);
                cur = self.nodes[i].next_optimized;
            }
        }
        out
    }
}

/// `PATHFIND_CLOSE_ENOUGH` (`AIPathfind.h:53`).
pub const PATHFIND_CLOSE_ENOUGH: f32 = 1.0;

/// `ClosestPointOnPathInfo` (`AIPathfind.h`): the movement goal along a path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClosestPointOnPathInfo {
    /// `posOnPath`: the position the locomotor should steer toward.
    pub pos_on_path: Coord3D,
    /// `distAlongPath`: the path distance still to cover.
    pub dist_along_path: f32,
    /// Index into [`Path::optimized_positions`] of the node after the closest segment's start
    /// (`closeNode->getNextOptimized()`); the nodes before it are behind the unit.
    pub next_index: usize,
}

impl Path {
    /// `Path::computePointOnPath` (`AIPathfind.cpp`, the ground half): find the closest segment of
    /// the optimised path to `pos`, then pick a goal ahead of it: the next node, or the middle of
    /// the following segment when the line to it is clear, blended toward the closest point the
    /// farther the unit is off the path. The `m_cpop*` result cache is not modelled (it only
    /// avoids recomputation).
    pub fn compute_point_on_path(&self, pf: &Pathfinder, obj: &PathObject, pos: &Coord3D) -> Option<ClosestPointOnPathInfo> {
        // The optimised chain from the first node, with each node's `canOptimize`.
        let mut chain: Vec<(Coord3D, bool)> = Vec::new();
        let first = self.nodes.first()?;
        chain.push((first.pos, first.can_optimize));
        let mut cur = first.next_optimized;
        while let Some(i) = cur {
            chain.push((self.nodes[i].pos, self.nodes[i].can_optimize));
            cur = self.nodes[i].next_optimized;
        }
        if chain.len() < 2 {
            return None;
        }
        // `getNextOptimized(&dir, &len)`: the 2D direction and length from node `a` to node `a + 1`.
        let seg = |a: usize| -> ((f32, f32), f32) {
            let (p, q) = (chain[a].0, chain[a + 1].0);
            let (dx, dy) = (q.x - p.x, q.y - p.y);
            let len = sqrt(dx * dx + dy * dy);
            if len > 0.0 { ((dx / len, dy / len), len) } else { ((0.0, 0.0), 0.0) }
        };

        let mut pos_on_path = chain[chain.len() - 1].0;
        let mut close: Option<usize> = None;
        let mut close_dist_sqr = 99_999_999.9f32;
        let mut total_len = 0.0f32;
        let mut length_along = 0.0f32;
        let mut prev = 0usize;
        for k in 1..chain.len() {
            let ((dnx, dny), seg_len) = seg(prev);
            let prev_pos = chain[prev].0;
            let node_pos = chain[k].0;
            let (tx, ty) = (pos.x - prev_pos.x, pos.y - prev_pos.y);
            let mut along = dnx * tx + dny * ty;
            let point;
            if along < 0.0 {
                along = 0.0;
                point = prev_pos;
            } else if along > seg_len {
                if k + 1 >= chain.len() {
                    along = seg_len;
                    point = node_pos;
                } else {
                    // Beyond the end of this segment: skip it.
                    total_len += seg_len;
                    prev = k;
                    continue;
                }
            } else {
                point = Coord3D::new(prev_pos.x + along * dnx, prev_pos.y + along * dny, 0.0);
            }
            let (ox, oy) = (pos.x - point.x, pos.y - point.y);
            let off_sqr = ox * ox + oy * oy;
            if off_sqr < close_dist_sqr {
                close_dist_sqr = off_sqr;
                close = Some(prev);
                pos_on_path = point;
                length_along = total_len + along;
            }
            total_len += seg_len;
            prev = k;
        }

        // The goal movement position for this agent.
        if let Some(ci) = close.filter(|&c| c + 1 < chain.len()) {
            let ((dnx, dny), seg_len) = seg(ci);
            let next_pos = chain[ci + 1].0;
            let close_pos = chain[ci].0;
            let (tx, ty) = (pos.x - close_pos.x, pos.y - close_pos.y);
            let mut along = dnx * tx + dny * ty;
            if along < 0.0 {
                along = 0.0;
            }
            let to_dist_sqr = tx * tx + ty * ty;
            let off_sq = to_dist_sqr - along * along;
            let offset_dist = if off_sq <= 0.0 { 0.0 } else { sqrt(off_sq) };
            let max_path_error = 3.0 * PATHFIND_CELL_SIZE_F;
            let mut k = offset_dist * (1.0 / max_path_error);
            if k > 1.0 {
                k = 1.0;
            }
            let mut got_pos = false;
            if pf.is_line_passable(obj, pos, &next_pos, false, true) {
                pos_on_path = next_pos;
                got_pos = true;
                let mut try_ahead = along > seg_len * 0.5;
                if !chain[ci + 1].1 {
                    try_ahead = false; // don't go past no-opt nodes.
                }
                let mut very_close = false;
                if seg_len - along < 1.0 {
                    try_ahead = true;
                    very_close = true;
                }
                if try_ahead && ci + 2 < chain.len() {
                    // Try the next segment's middle.
                    let nx = chain[ci + 2].0;
                    let try_pos = Coord3D::new((next_pos.x + nx.x) * 0.5, (next_pos.y + nx.y) * 0.5, next_pos.z);
                    if very_close || pf.is_line_passable(obj, pos, &try_pos, false, true) {
                        got_pos = true;
                        pos_on_path = try_pos;
                    }
                }
            } else if k > 0.5 {
                let try_dist = along + 0.5 * (seg_len - along);
                pos_on_path = Coord3D::new(close_pos.x + try_dist * dnx, close_pos.y + try_dist * dny, close_pos.z);
                if pf.is_line_passable(obj, pos, &pos_on_path, false, true) {
                    k = 0.5;
                    got_pos = true;
                }
            }
            // On the path (k == 0): alongPathDist == segmentLength; far off (k == 1): unchanged.
            along += (1.0 - k) * (seg_len - along);
            if !got_pos {
                if along > seg_len {
                    pos_on_path = next_pos;
                } else {
                    pos_on_path = Coord3D::new(close_pos.x + along * dnx, close_pos.y + along * dny, close_pos.z);
                    let (dx, dy) = ((pos.x - pos_on_path.x).abs(), (pos.y - pos_on_path.y).abs());
                    if dx < 1.0 && dy < 1.0 && ci + 2 < chain.len() {
                        pos_on_path = chain[ci + 2].0;
                    }
                }
            }
        }

        let mut dist_along_path = total_len - length_along;
        let (ddx, ddy) = (pos_on_path.x - pos.x, pos_on_path.y - pos.y);
        let len_delta = sqrt(ddx * ddx + ddy * ddy);
        if len_delta > dist_along_path && dist_along_path > PATHFIND_CLOSE_ENOUGH {
            dist_along_path = len_delta;
        }
        Some(ClosestPointOnPathInfo { pos_on_path, dist_along_path, next_index: close.map_or(chain.len() - 1, |c| c + 1) })
    }
}

/// Per-search parameters derived from the object (`getRadiusAndCenter`, locomotor set).
struct Ctx {
    surfaces: u32,
    radius: i32,
    center: bool,
    is_human: bool,
    /// The moving object, for [`Pathfinder::check_for_movement_cell`]'s ally/enemy tests.
    obj: PathObject,
}

/// The pathfinder (`class Pathfinder`).
#[derive(Clone, Debug)]
pub struct Pathfinder {
    /// Number of cells in x / y (`m_extent.hi + 1`; `m_extent.lo` is 0).
    w: i32,
    h: i32,
    cells: Vec<Cell>,
    info: Vec<Info>,
    touched: Vec<u32>,
    open: BTreeSet<(u16, u64, u32)>,
    seq: u64,
    is_tunneling: bool,
    ignore_obstacle: u32,
    map_ready: bool,
    /// Zone-lite labels per surface mask (0 = not passable); cleared when the map changes.
    zones: crate::zone::ZoneManager,
    /// Position footprint per unit (`m_posUnit` bookkeeping), keyed by object id.
    unit_cells: BTreeMap<u32, Footprint>,
    /// The `PathObject` behind each registered unit, so `moveAllies` can test relationships and
    /// ask an idle ally to step aside (`Pathfinder::moveAllies`).
    unit_objs: BTreeMap<u32, PathObject>,
    /// The world position of each registered unit (`Object::getPosition`), for `getMoveAwayFromPath`.
    unit_pos: BTreeMap<u32, Coord3D>,
    /// Goal footprint per unit (`m_goalUnit` bookkeeping), keyed by object id.
    unit_goals: BTreeMap<u32, Footprint>,
    /// Wall pieces (`m_wallPieces`), keyed by object id (BTreeMap keeps iteration deterministic).
    wall_pieces: BTreeMap<u32, WallPiece>,
    /// `m_wallHeight` (`AIData WallHeight`), used by [`layer_for_destination`](Self::layer_for_destination).
    wall_height: f32,
    /// The wall layer's cells, parallel to `cells`; only cells inside `wall_rect` are meaningful
    /// (everything else is `CELL_IMPASSABLE`). A wall search swaps this into `cells` for the
    /// duration of the search (see [`on_layer`](Self::on_layer)).
    wall: Vec<Cell>,
    /// Inclusive cell bounds `(x0, y0, x1, y1)` of the allocated wall layer, or `None` when unused.
    wall_rect: Option<(i32, i32, i32, i32)>,
    /// A wall piece was added/removed; the wall layer is rebuilt lazily (`ensure_wall_layer`).
    /// `addWallPiece` only appends to `m_wallPieces`; the engine builds the layer once in `newMap`.
    wall_dirty: bool,
    /// The current logic frame (`GameLogic::update` sets it), for the zone schedule below.
    frame: u32,
    /// `PathfindZoneManager::m_nextFrameToCalculateZones` (`AIPathfind.cpp:2537`; `markZonesDirty` :2641-2652 sets
    /// 2 before frame 2, else `min(next, frame + ZONE_UPDATE_FREQUENCY)`; `calculateZones` sets `0xffffffff`,
    /// :2991). `processPathfindQueue` returns early while it is due (:6018-6022). Only
    /// [`mark_zones_dirty`](Self::mark_zones_dirty) schedules, and only at the engine's call sites.
    zone_calc_due: u32,
    /// True while a wall-layer search has the wall grid swapped into `cells`; the hierarchical
    /// corridor and block passable flags are ground-only, so they are skipped then.
    search_on_wall: bool,
    /// `player -> allied players` bitmask, set from `GameLogic`'s relationship table. Used by
    /// [`check_for_movement_full`](Self::check_for_movement_full)'s ally/enemy tests.
    allies: [u32; 32],
    /// `player -> players it is at war with` (`Player::getRelationship(...) == ENEMIES`), for the
    /// dozer exception in [`examine_neighboring_cells`](Self::examine_neighboring_cells)
    /// (`AIPathfind.cpp:6386-6392`). `NEUTRAL` is *not* an enemy, so the neutral player is not set.
    enemies: [u32; 32],
    /// `Object::getControllingPlayer()->getPlayerIndex()` of each obstacle (structure) id. The engine
    /// reads it live off the object (`TheGameLogic->findObjectByID(newCell->getObstacleID())`,
    /// `AIPathfind.cpp:6387`); the sim's pathfinder keeps a copy that `GameLogic` refreshes with the
    /// frame's object list, because a `PathfindCell` itself stores only the obstacle id.
    obstacle_owners: BTreeMap<u32, crate::object::PlayerIndex>,
    /// Lane 188: the ids of the mobile units that are vehicles (`KINDOF_VEHICLE`), refreshed per frame with the unit
    /// table. The attacker's kind decides the strip-back at the end of `findAttackPath` (`AIPathfind.cpp:10887`).
    vehicle_units: std::collections::BTreeSet<u32>,
    /// Lane 188: the ids of the mobile units whose `AIUpdateInterface::isIdle()` is true this frame. The strip-back's
    /// `unitIdle` exemption (`AIPathfind.cpp:10906-10909`) reads it for the unit on each cell.
    idle_units: std::collections::BTreeSet<u32>,
    /// Lane 191: each mobile unit's crush levels and `SquishCollide` module, and whether it is `DISABLED_UNMANNED`
    /// (`Object::canCrushOrSquish`, `Object.cpp:1121-1200`). `checkDestination` asks it whether a unit can move onto a
    /// goal cell that holds a fixed unit. Refreshed per frame with the unit table ([`Pathfinder::set_unit_crush`]).
    crush_units: std::collections::BTreeMap<u32, (crate::modules2::CrushInfo, bool)>,
    /// `m_queuedPathfindRequests` (`AIPathfind.cpp:5761`): objects waiting for a path. FIFO, no
    /// duplicates, serviced in order by [`GameLogic::process_pathfind_queue`].
    queued_path_requests: std::collections::VecDeque<ObjectId>,
    /// `m_cumulativeCellsAllocated`: cells allocated so far in this frame's queue service. The
    /// engine resets it in `processPathfindQueue` and stops servicing once it reaches
    /// `PATHFIND_CELLS_PER_FRAME`.
    cumulative_cells_allocated: u32,
}

/// `TCheckMovementInfo`'s output fields (`checkForMovement`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MovementInfo {
    /// `enemyFixed`: a fixed enemy occupies a footprint cell (blocks unless the mover is a crusher).
    pub enemy_fixed: bool,
    /// `allyFixedCount`: fixed allied units in the footprint (block the fine A* in the engine).
    pub ally_fixed_count: u32,
    /// `allyMoving`: a moving allied unit is in the footprint (only adds cost).
    pub ally_moving: bool,
    /// `allyGoal`: an allied unit's reserved goal is in the footprint.
    pub ally_goal: bool,
}

/// `ZR_L128_PATH` (diagnostic, read-only): trace the AN3 dock workers' path requests around the
/// wave-16 f1547 divergence. Off unless the variable is set; no game state is read or changed.
fn l128_path_trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ZR_L128_PATH").is_some())
}

/// `ZR_HPATH` (diagnostic, read-only): dump each hierarchical path request's corridor and fine path
/// in the same shape as the patched C++ (`zh-cpp-build/src-patched/.../AIPathfind.cpp`). The value is
/// `<first>-<last>` frames, or a single frame; unset means off.
fn hpath_trace(frame: u32) -> bool {
    static RANGE: std::sync::OnceLock<Option<(u32, u32)>> = std::sync::OnceLock::new();
    let range = RANGE.get_or_init(|| {
        let raw = std::env::var("ZR_HPATH").ok()?;
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let (a, b) = match raw.split_once('-') {
            Some((a, b)) => (a.trim().parse().ok()?, b.trim().parse().ok()?),
            None => {
                let f: u32 = raw.parse().ok()?;
                (f, f)
            }
        };
        Some((a, b.max(a)))
    });
    matches!(range, Some((a, b)) if frame >= *a && frame <= *b)
}

/// `ZR_L191_ATKPOP=<first>-<last>[:<objectId>]` (diagnostic, read-only; lane 191): the attack search's pops and
/// its result, in the same shape as the patched C++ (`ZR_L191_ATKPOP` in `AIPathfind.cpp`'s `findAttackPath`).
/// Unset means off; a malformed value is off. A reversed range (`b < a`) is the single frame `a`, which is what the C++
/// `zrL191Active` reads (`f > (b < a ? a : b)`).
fn l191_atkpop(frame: u32, id: u32) -> bool {
    let Ok(raw) = std::env::var("ZR_L191_ATKPOP") else { return false };
    let (range, id_filter) = match raw.split_once(':') {
        Some((r, i)) => (r, i.trim().parse::<u32>().ok()),
        None => (raw.as_str(), None),
    };
    let Some((a, b)) = range.split_once('-') else { return false };
    let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) else { return false };
    // A reversed range (`b < a`) is the single frame `a`, as the C++ `zrL191Active` reads it (`f > (b < a ? a : b)`).
    if frame < a || frame > b.max(a) {
        return false;
    }
    id_filter.is_none_or(|want| want == id)
}

/// The corridor dump: every block whose `ZoneBlock::m_markedPassable` the last hierarchical search
/// set (`PathfindZoneManager::isPassable` at each block's low cell).
fn hpath_dump_corridor(what: &str, id: u32, frame: u32, zones: &crate::zone::ZoneManager) {
    use std::fmt::Write as _;
    let (bx_count, by_count) = zones.block_extent();
    let mut out = String::new();
    for bx in 0..bx_count {
        for by in 0..by_count {
            if zones.is_passable(bx * crate::zone::ZONE_BLOCK_SIZE, by * crate::zone::ZONE_BLOCK_SIZE) {
                let _ = write!(out, " {bx},{by}");
            }
        }
    }
    eprintln!("ZR_HPATH f={frame} req={what} id={id} corridor{out}");
}

/// A path in the trace writer's shape: node count, first node, its first optimised node, last node.
fn hpath_dump_path(what: &str, id: u32, frame: u32, path: Option<&Path>) {
    match path {
        Some(p) if !p.nodes.is_empty() => {
            let first = p.nodes[0].pos;
            let next = p.nodes[0].next_optimized.and_then(|i| p.nodes.get(i)).map_or(first, |n| n.pos);
            let last = p.nodes[p.nodes.len() - 1].pos;
            eprintln!(
                "ZR_HPATH f={frame} req={what} id={id} fine=ok n={} first=({:.3},{:.3}) next=({:.3},{:.3}) last=({:.3},{:.3})",
                p.nodes.len(),
                first.x,
                first.y,
                next.x,
                next.y,
                last.x,
                last.y
            );
        }
        _ => eprintln!("ZR_HPATH f={frame} req={what} id={id} fine=fail"),
    }
}



/// One `KINDOF_WALK_ON_TOP_OF_WALL` object registered with the pathfinder (`PathfindLayer`'s wall
/// piece list): its oriented footprint, used to classify the wall layer's cells.
#[derive(Clone, Copy, Debug)]
struct WallPiece {
    pos: Coord3D,
    angle: f32,
    major: f32,
    minor: f32,
}

/// A `KINDOF_AIRCRAFT_PATH_AROUND` building as `getAircraftPath` queries it: object id, position and
/// `GeometryInfo::getBoundingCircleRadius()`.
pub(crate) type TallBuilding = (u32, Coord3D, f32);

impl Pathfinder {
    /// `Pathfinder::newMap` + `classifyMap`: size the grid from the playable area and classify terrain.
    /// Structures are added afterwards through [`classify_object_footprint`](Self::classify_object_footprint).
    pub fn new(terrain: &TerrainLogic) -> Self {
        let (ww, wh) = terrain.world_size();
        // `Pathfinder::newMap` (AIPathfind.cpp:4691-4697): bounds.hi = REAL_TO_INT_FLOOR(extent.hi / cell) - 1;
        // cells are 0..=hi. The port's terrain extent starts at 0, so `ww` / `wh` are `extent.hi`.
        let hi_x = real_to_int_floor(ww / PATHFIND_CELL_SIZE_F) - 1;
        let hi_y = real_to_int_floor(wh / PATHFIND_CELL_SIZE_F) - 1;
        let (w, h) = ((hi_x + 1).max(0), (hi_y + 1).max(0));
        let n = (w * h) as usize;
        let mut pf = Self {
            w,
            h,
            cells: vec![wall_cell(CellType::Clear); n],
            info: vec![NO_INFO; n],
            touched: Vec::new(),
            open: BTreeSet::new(),
            seq: 0,
            is_tunneling: false,
            ignore_obstacle: 0,
            map_ready: false,
            zones: crate::zone::ZoneManager::default(),
            unit_cells: BTreeMap::new(),
            unit_objs: BTreeMap::new(),
            unit_pos: BTreeMap::new(),
            unit_goals: BTreeMap::new(),
            wall_pieces: BTreeMap::new(),
            wall_height: 0.0,
            wall: vec![wall_cell(CellType::Impassable); n],
            wall_rect: None,
            wall_dirty: false,
            frame: 0,
            // `newMap` -> `classifyMap` -> `calculateZones` leaves no calculation due (`AIPathfind.cpp:4814`,
            // `:2991`). The frame-2 due of a stocked map comes only from the frame-0 footprint marks of newMap's
            // object loop (`:4729-4732` -> `:4389`/`:4463`); a structure-less map never gates.
            zone_calc_due: u32::MAX,
            search_on_wall: false,
            allies: [0u32; 32],
            enemies: [0u32; 32],
            obstacle_owners: BTreeMap::new(),
            vehicle_units: std::collections::BTreeSet::new(),
            idle_units: std::collections::BTreeSet::new(),
            crush_units: std::collections::BTreeMap::new(),
            queued_path_requests: std::collections::VecDeque::new(),
            cumulative_cells_allocated: 0,
        };
        pf.zones.allocate_blocks(w, h);
        pf.classify_map(terrain);
        // The base type is the terrain after cliff expansion, so a bridge overlay (and its removal)
        // restores the real ground classification.
        for c in &mut pf.cells {
            c.base_ty = c.ty;
        }
        pf.apply_bridges(terrain);
        // The engine's `newMap` allocates and classifies the bridge layers *inside* `classifyMap`,
        // so its closing `calculateZones` (`AIPathfind.cpp:4814`) already sees the decks. The port's
        // overlay runs after `classify_map`, so the zones are computed once more here to match
        // (map load only; nothing is scheduled: `calculateZones` clears the due frame, `:2991`).
        pf.calculate_zones();
        pf.map_ready = true;
        pf
    }

    // ---- grid helpers -----------------------------------------------------------------------

    /// Grid size in cells `(x, y)`.
    pub fn extent(&self) -> (i32, i32) {
        (self.w, self.h)
    }

    // ---- async pathfind queue (`queueForPath` / `processPathfindQueue`) -----------------------

    /// `Pathfinder::queueForPath` (`AIPathfind.cpp:5761`): enqueue `id` unless it is already queued.
    /// Returns `true` if the object is now on the queue.
    pub fn queue_for_path(&mut self, id: ObjectId) -> bool {
        if self.queued_path_requests.iter().any(|q| *q == id) {
            return true;
        }
        self.queued_path_requests.push_back(id);
        true
    }

    /// Whether `id` is currently queued (`queueForPath`'s duplicate check).
    pub fn is_queued_for_path(&self, id: ObjectId) -> bool {
        self.queued_path_requests.iter().any(|q| *q == id)
    }

    /// Pop the next queued request (`processPathfindQueue`'s head read).
    pub fn pop_queued_path(&mut self) -> Option<ObjectId> {
        self.queued_path_requests.pop_front()
    }

    pub fn queued_path_count(&self) -> usize {
        self.queued_path_requests.len()
    }

    /// `m_cumulativeCellsAllocated = 0` at the top of `processPathfindQueue`.
    pub fn reset_cell_budget(&mut self) {
        self.cumulative_cells_allocated = 0;
    }

    /// Cells allocated so far this frame (the queue budget check).
    pub fn cells_allocated(&self) -> u32 {
        self.cumulative_cells_allocated
    }
    fn idx(&self, x: i32, y: i32) -> usize {
        (y * self.w + x) as usize
    }
    fn xy(&self, i: usize) -> (i32, i32) {
        (i as i32 % self.w, i as i32 / self.w)
    }
    /// `getCell(LAYER_GROUND, x, y)`.
    fn get_cell(&self, x: i32, y: i32) -> Option<usize> {
        (x >= 0 && x < self.w && y >= 0 && y < self.h).then(|| self.idx(x, y))
    }
    /// `worldToCell`: clamps into the grid.
    fn world_to_cell(&self, p: &Coord3D) -> (i32, i32, bool) {
        let mut x = real_to_int_floor(p.x / PATHFIND_CELL_SIZE_F);
        let mut y = real_to_int_floor(p.y / PATHFIND_CELL_SIZE_F);
        let mut overflow = false;
        if x < 0 {
            overflow = true;
            x = 0;
        }
        if y < 0 {
            overflow = true;
            y = 0;
        }
        if x > self.w - 1 {
            overflow = true;
            x = self.w - 1;
        }
        if y > self.h - 1 {
            overflow = true;
            y = self.h - 1;
        }
        (x, y, overflow)
    }
    /// `getCell(layer, const Coord3D*)`: `None` when off the grid.
    fn cell_at(&self, p: &Coord3D) -> Option<usize> {
        let (x, y, overflow) = self.world_to_cell(p);
        if overflow {
            None
        } else {
            self.get_cell(x, y)
        }
    }

    /// `PathfindCell::isObstaclePresent`/`isObstacleTransparent` (`AIPathfind.cpp:1997`, `:2020`) of the
    /// cell holding `pos`: the obstacle id that would block a line of sight there (`None` when the cell
    /// is not an obstacle). The caller compares it against the ids the engine exempts.
    pub(crate) fn los_cell_obstacle_at(&self, pos: &Coord3D) -> Option<u32> {
        // `getCell(layer, const Coord3D*)` is null when `worldToCell` overflows (`AIPathfind.h:479-481`).
        let ci = self.cell_at(pos)?;
        let c = &self.cells[ci];
        (c.ty == CellType::Obstacle).then_some(c.obstacle)
    }

    /// `Pathfinder::iterateCellsAlongLine` (`AIPathfind.cpp:9350-9385`) for the line-of-sight obstacle
    /// march: the cells in visit order with `(type, obstacle id, obstacle transparent)`. Stops at the
    /// first cell off the grid, as the engine's `return 0` does.
    pub(crate) fn los_cells_along_line(&self, from: &Coord3D, to: &Coord3D) -> Vec<(CellType, u32, bool)> {
        let (mut x, mut y, _) = self.world_to_cell(from);
        let (ex, ey, _) = self.world_to_cell(to);
        let delta_x = (ex - x).abs();
        let delta_y = (ey - y).abs();
        let (mut xinc1, mut xinc2) = if ex >= x { (1, 1) } else { (-1, -1) };
        let (mut yinc1, mut yinc2) = if ey >= y { (1, 1) } else { (-1, -1) };
        let (den, mut num, numadd, numpixels);
        if delta_x >= delta_y {
            xinc1 = 0;
            yinc2 = 0;
            den = delta_x;
            num = delta_x / 2;
            numadd = delta_y;
            numpixels = delta_x;
        } else {
            xinc2 = 0;
            yinc1 = 0;
            den = delta_y;
            num = delta_y / 2;
            numadd = delta_x;
            numpixels = delta_y;
        }
        let mut out = Vec::with_capacity(numpixels.max(0) as usize + 1);
        for _ in 0..=numpixels {
            let Some(ci) = self.get_cell(x, y) else { break };
            let c = &self.cells[ci];
            out.push((c.ty, c.obstacle, c.obstacle_transparent));
            num += numadd;
            if num >= den {
                num -= den;
                x += xinc1;
                y += yinc1;
                // The engine's second callback on a diagonal step (`AIPathfind.cpp:9266-9271`): the cell
                // beside the one just tested is visited too, and an off-grid one ends the walk (`return 0`).
                // `attackBlockedByObstacleCallback` never reads `from`, so only `to` is kept.
                let Some(ci) = self.get_cell(x, y) else { break };
                let c = &self.cells[ci];
                out.push((c.ty, c.obstacle, c.obstacle_transparent));
            }
            x += xinc2;
            y += yinc2;
        }
        out
    }
    /// `getClippedCell`.
    fn clipped_cell_at(&self, p: &Coord3D) -> Option<usize> {
        let (x, y, _) = self.world_to_cell(p);
        self.get_cell(x, y)
    }

    /// Number of grid cells of a type (diagnostics).
    pub fn cell_type_count(&self, ty: CellType) -> usize {
        self.cells.iter().filter(|c| c.ty == ty).count()
    }

    /// Cell type at a world position (`None` off the grid). Public for tests and debug views.
    pub fn cell_type_at(&self, p: &Coord3D) -> Option<CellType> {
        self.cell_at(p).map(|i| self.cells[i].ty)
    }
    /// `PathfindCell::getZone()` at a world position (`None` off the grid). `UNINITIALIZED_ZONE`
    /// (0) means a footprint change touched the cell and `updateZonesForModify` could not give it a
    /// same-type neighbour's zone (`AIPathfind.cpp:2998-3120`). Public for tests and debug views.
    pub fn cell_zone_at(&self, p: &Coord3D) -> Option<u32> {
        self.cell_at(p).map(|i| self.zone_of(i))
    }
    /// Pinched flag at a world position.
    pub fn is_pinched_at(&self, p: &Coord3D) -> bool {
        self.cell_at(p).is_some_and(|i| self.cells[i].pinched)
    }

    /// `setIgnoreObstacleID`.
    pub fn set_ignore_obstacle_id(&mut self, id: ObjectId) {
        self.ignore_obstacle = id.0;
    }

    // ---- classification ----------------------------------------------------------------------

    /// `Pathfinder::classifyMapCell` for a cell with no obstacle.
    fn classify_map_cell(&mut self, terrain: &TerrainLogic, i: i32, j: i32) {
        let ci = self.idx(i, j);
        let has_obstacle = self.cells[ci].ty == CellType::Obstacle;
        let top = j as f32 * PATHFIND_CELL_SIZE_F;
        let bottom = top + PATHFIND_CELL_SIZE_F;
        let left = i as f32 * PATHFIND_CELL_SIZE_F;
        let right = left + PATHFIND_CELL_SIZE_F;
        self.cells[ci].pinched = false;
        let mut ty = CellType::Clear;
        if terrain.is_cliff_cell(left, top) {
            ty = CellType::Cliff;
        }
        // If any corner is underwater this is a water cell.
        if terrain.is_underwater(left, top)
            || terrain.is_underwater(left, bottom)
            || terrain.is_underwater(right, bottom)
            || terrain.is_underwater(right, top)
        {
            ty = CellType::Water;
        }
        if has_obstacle {
            ty = CellType::Obstacle;
        }
        self.cells[ci].base_ty = ty;
        self.set_type(ci, ty);
    }

    /// `PathfindCell::setType`: an occupied cell stays an obstacle.
    fn set_type(&mut self, ci: usize, ty: CellType) {
        self.cells[ci].ty = if self.cells[ci].obstacle != 0 { CellType::Obstacle } else { ty };
    }

    /// `Pathfinder::classifyMap`.
    fn classify_map(&mut self, terrain: &TerrainLogic) {
        for j in 0..self.h {
            for i in 0..self.w {
                self.classify_map_cell(terrain, i, j);
            }
        }
        // Expand all cliff cells one step (mark pinched).
        self.mark_pinched_around_cliffs();
        // Convert pinched to cliff.
        for c in &mut self.cells {
            if c.pinched && c.ty == CellType::Clear {
                c.ty = CellType::Cliff;
            }
        }
        // Add a border of pinched cells to cliffs.
        self.mark_pinched_around_cliffs();
        // `classifyMap` ends in `m_zoneManager.calculateZones(...)` (`AIPathfind.cpp:4814`), which
        // leaves no calculation scheduled (`:2991`); it does not call `markZonesDirty`.
        self.calculate_zones();
    }

    fn mark_pinched_around_cliffs(&mut self) {
        for j in 0..self.h {
            for i in 0..self.w {
                if (self.cells[self.idx(i, j)].ty as u8) & (CellType::Cliff as u8) != 0 {
                    for k in (i - 1)..(i + 2) {
                        if k < 0 || k > self.w - 1 {
                            continue;
                        }
                        for l in (j - 1)..(j + 2) {
                            if l < 0 || l > self.h - 1 {
                                continue;
                            }
                            let ni = self.idx(k, l);
                            if self.cells[ni].ty == CellType::Clear {
                                self.cells[ni].pinched = true;
                            }
                        }
                    }
                }
            }
        }
    }

    // ---- bridges (`PathfindLayer`) -----------------------------------------------------------

    /// Recompute the bridge overlay after a bridge is destroyed/repaired (`Bridge::updateDamageState`,
    /// `TerrainLogic.cpp:840-899` -> `Pathfinder::changeBridgeState`, `AIPathfind.cpp:9834-9840`).
    pub fn refresh_bridges(&mut self, terrain: &TerrainLogic) {
        self.apply_bridges(terrain);
        // `changeBridgeState` schedules a zone calculation only when `PathfindLayer::setDestroyed` reports a real
        // state change (`AIPathfind.cpp:9837-9839`, :3749-3757: equal state returns false and marks nothing).
        // `GameLogic::update_bridge_state` calls this only after the terrain bridge's `destroyed` flag flipped.
        self.mark_zones_dirty();
    }

    /// Overlay every healthy bridge's rectangle: its cells become passable for ground units (a
    /// bridge over water/cliff) and carry the deck height (`PathfindLayer`).
    fn apply_bridges(&mut self, terrain: &TerrainLogic) {
        for c in &mut self.cells {
            c.ty = if c.obstacle != 0 { CellType::Obstacle } else { c.base_ty };
            c.bridge = 0;
            c.bridge_height = 0.0;
            c.bridge_over_impassable = false;
        }
        for (bi, b) in terrain.bridges().iter().enumerate() {
            if b.destroyed {
                continue;
            }
            let (lo, hi) = b.bounds();
            // `PathfindLayer::allocateCells` (AIPathfind.cpp:3511-3519, shipped :3376-3384): origin
            // FLOOR(lo / cell) - 1, last cell CEIL(hi / cell) (the exclusive end is that plus one). The C++
            // layer then classifies each cell by its corners (`classifyLayerMapCell`); the centre test below
            // stays a port difference.
            let x0 = real_to_int_floor(lo[0] / PATHFIND_CELL_SIZE_F) - 1;
            let y0 = real_to_int_floor(lo[1] / PATHFIND_CELL_SIZE_F) - 1;
            let x1 = real_to_int_ceil(hi[0] / PATHFIND_CELL_SIZE_F);
            let y1 = real_to_int_ceil(hi[1] / PATHFIND_CELL_SIZE_F);
            for j in y0..=y1 {
                for i in x0..=x1 {
                    let Some(ci) = self.get_cell(i, j) else { continue };
                    let (cx, cy) = ((i as f32 + 0.5) * PATHFIND_CELL_SIZE_F, (j as f32 + 0.5) * PATHFIND_CELL_SIZE_F);
                    if !b.contains_2d(cx, cy) {
                        continue;
                    }
                    let impassable = matches!(self.cells[ci].base_ty, CellType::Water | CellType::Cliff | CellType::Impassable);
                    self.cells[ci].bridge = bi as u32 + 1;
                    self.cells[ci].bridge_height = b.deck_height(cx, cy);
                    self.cells[ci].bridge_over_impassable = impassable;
                    if impassable {
                        self.cells[ci].ty = CellType::Clear;
                    }
                }
            }
        }
        // No zone bookkeeping here: in the engine the bridge layer's cells are classified inside
        // `classifyMap`, before its closing `calculateZones` (`AIPathfind.cpp:4800-4814`), and
        // `changeBridgeState` only *schedules* a calculation (`:9892-9898`) — the old zones and the
        // ground cells' zone numbers stay until that frame. The port's overlay runs after
        // `classify_map` in [`Pathfinder::new`], which recomputes the zones once to see the decks;
        // [`refresh_bridges`](Self::refresh_bridges) schedules.
    }

    // ---- walls (`Pathfinder::addWallPiece` / `PathfindLayer` wall layer) ---------------------

    /// `AIData WallHeight` (`Pathfinder::m_wallHeight`). A zero wall height disables layer
    /// selection by z (`TerrainLogic::getLayerForDestination`).
    pub fn set_wall_height(&mut self, h: f32) {
        self.wall_height = h;
    }
    /// `Pathfinder::getWallHeight`.
    pub fn wall_height(&self) -> f32 {
        self.wall_height
    }
    /// The number of registered wall pieces (`m_numWallPieces`).
    pub fn wall_piece_count(&self) -> usize {
        self.wall_pieces.len()
    }

    /// `Pathfinder::addWallPiece`: register a `KINDOF_WALK_ON_TOP_OF_WALL` object and rebuild the
    /// wall layer. Called after the object is positioned (its footprint is fixed until removal).
    pub fn add_wall_piece(&mut self, id: ObjectId, geom: &Geometry, pos: &Coord3D, angle: f32) {
        if id == ObjectId::INVALID {
            return;
        }
        let (major, minor) = match geom.shape {
            // `PathfindLayer::isPointOnWall`: a sphere uses the major radius for both axes.
            GeometryShape::Sphere => (geom.major_radius, geom.major_radius),
            _ => (geom.major_radius, geom.minor_radius),
        };
        self.wall_pieces.insert(id.0, WallPiece { pos: *pos, angle, major, minor });
        self.wall_dirty = true;
    }

    /// `Pathfinder::removeWallPiece`. The layer is rebuilt on the next [`ensure_wall_layer`](Self::ensure_wall_layer).
    pub fn remove_wall_piece(&mut self, id: ObjectId) {
        if self.wall_pieces.remove(&id.0).is_some() {
            self.wall_dirty = true;
        }
    }

    /// `GameLogic::update` sets the logic frame before the frame's updates, for the zone schedule.
    pub fn set_frame(&mut self, frame: u32) {
        self.frame = frame;
    }

    /// `PathfindZoneManager::markZonesDirty` (the non-`RTS_GENERALS` branch, AIPathfind.cpp:2641-2652): before
    /// frame 2 the calculation is due at frame 2; later it is due `ZONE_UPDATE_FREQUENCY` (300) frames after the
    /// first dirtying. It only *schedules*: the tables are computed by [`calculate_zones`](Self::calculate_zones)
    /// when `processPathfindQueue` reaches the due frame (`:6029-6032`). Call this only where the engine calls
    /// `markZonesDirty`: the box (:4389) and sphere/cylinder (:4463) footprint arms, a fence that changed a cell
    /// (:4271-4274), and a bridge whose destroyed state actually flipped (:9837-9839). `classifyMap` (:4814),
    /// the wall-layer build (:3545-3630) and the `LAYER_WALL` swaps schedule nothing.
    fn mark_zones_dirty(&mut self) {
        self.zone_calc_due = if self.frame < 2 { 2 } else { self.zone_calc_due.min(self.frame + 300) };
    }

    /// `PathfindZoneManager::needToCalculateZones` (Core/GameEngine/Include/GameLogic/Pathfinder/PathfindZoneManager.h:50).
    pub fn zones_need_calculation(&self) -> bool {
        self.zone_calc_due <= self.frame
    }

    /// `PathfindZoneManager::calculateZones` (`AIPathfind.cpp:2661-2997`): rebuild the per-cell zones and
    /// the six equivalency tables from the current grid, then leave no further calculation due
    /// (`m_nextFrameToCalculateZones = 0xffffffff`, `:2991`).
    pub fn calculate_zones(&mut self) {
        let (w, h) = (self.w, self.h);
        let cells = &self.cells;
        self.zones.calculate_zones(w, h, |i| cells[i].ty, |i| cells[i].fence);
        self.zone_calc_due = u32::MAX;
    }

    /// Rebuild the wall layer if a piece was added or removed. The engine builds it once in
    /// `newMap` (after every `addWallPiece`), not per piece; building it is O(cells × pieces), so
    /// rebuilding 116 times on map load would be quadratic.
    pub fn ensure_wall_layer(&mut self) {
        if self.wall_dirty {
            self.rebuild_wall_layer();
            self.wall_dirty = false;
        }
    }

    /// The union of every piece's 2D bounds, or `None` when there are no pieces.
    fn wall_pieces_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut it = self.wall_pieces.values();
        let first = it.next()?;
        let (mut lx, mut ly, mut hx, mut hy) = self.wall_piece_bounds(first);
        for wp in it {
            let (a, b, c, d) = self.wall_piece_bounds(wp);
            lx = lx.min(a);
            ly = ly.min(b);
            hx = hx.max(c);
            hy = hy.max(d);
        }
        Some((lx, ly, hx, hy))
    }

    /// `GeometryInfo::get2DBounds` for a rotated box/cylinder: the axis-aligned bounds of the
    /// oriented footprint.
    fn wall_piece_bounds(&self, wp: &WallPiece) -> (f32, f32, f32, f32) {
        let c = cos(wp.angle).abs();
        let s = sin(wp.angle).abs();
        let ex = wp.major * c + wp.minor * s;
        let ey = wp.major * s + wp.minor * c;
        (wp.pos.x - ex, wp.pos.y - ey, wp.pos.x + ex, wp.pos.y + ey)
    }

    /// `PathfindLayer::isPointOnWall`: is `pt` inside any wall piece's oriented footprint?
    pub fn is_point_on_wall(&self, pt: &Coord3D) -> bool {
        for wp in self.wall_pieces.values() {
            let c = cos(-wp.angle);
            let s = sin(-wp.angle);
            let ptx = pt.x - wp.pos.x;
            let pty = pt.y - wp.pos.y;
            let nx = (ptx * c - pty * s).abs();
            let ny = (ptx * s + pty * c).abs();
            if nx <= wp.major && ny <= wp.minor {
                return true;
            }
        }
        false
    }

    /// `PathfindLayer::allocateCellsForWallLayer` + `classifyWallCells`: size the wall layer from
    /// the union of the piece footprints, classify every cell by its four corners and close off
    /// pinched (edge) cells.
    fn rebuild_wall_layer(&mut self) {
        let Some((lx, ly, hx, hy)) = self.wall_pieces_bounds() else {
            self.wall_rect = None;
            return;
        };
        for c in &mut self.wall {
            *c = wall_cell(CellType::Impassable);
        }
        // `PathfindLayer::allocateCellsForWallLayer` (Core/GameEngine/Source/GameLogic/AI/AIPathfind.cpp:3567-3577,
        // shipped copy :3435-3445): origin = REAL_TO_INT_FLOOR(lo / cell) - 1 (`PATHFIND_CELL_SIZE/100` is the
        // integer 0, so there is no 0.1 margin); the exclusive end is maxX = REAL_TO_INT_CEIL(hi / cell) + 1,
        // clipped to the extent (`maxX > extent.hi.x` gives hi). The cells are origin..maxX-1 inclusive.
        let origin_x = (real_to_int_floor(lx / PATHFIND_CELL_SIZE_F) - 1).max(0);
        let origin_y = (real_to_int_floor(ly / PATHFIND_CELL_SIZE_F) - 1).max(0);
        let max_x = (real_to_int_ceil(hx / PATHFIND_CELL_SIZE_F) + 1).min(self.w - 1);
        let max_y = (real_to_int_ceil(hy / PATHFIND_CELL_SIZE_F) + 1).min(self.h - 1);
        if max_x <= origin_x || max_y <= origin_y {
            self.wall_rect = None;
            return;
        }
        let (x0, y0, x1, y1) = (origin_x, origin_y, max_x - 1, max_y - 1);
        self.wall_rect = Some((x0, y0, x1, y1));
        self.classify_wall_cells();
        // `allocateCellsForWallLayer` / `classifyWallCells` mark nothing (`AIPathfind.cpp:3545-3630`,
        // :3686-3744); the wall layer has its own cells and the ground grid is unchanged.
    }

    /// `PathfindLayer::classifyWallCells`.
    fn classify_wall_cells(&mut self) {
        let Some((x0, y0, x1, y1)) = self.wall_rect else { return };
        for j in y0..=y1 {
            for i in x0..=x1 {
                let ty = self.classify_wall_map_cell(i, j);
                let ci = self.idx(i, j);
                self.wall[ci] = wall_cell(ty);
                // The wall cell carries the wall height so `adjust_coord_to_cell` returns it.
                if ty == CellType::Clear {
                    self.wall[ci].bridge = 1;
                    self.wall[ci].bridge_height = self.wall_height;
                    self.wall[ci].bridge_over_impassable = true;
                }
            }
        }
        // Tighten up 1 cell (original loops the interior only): a clear cell next to a non-clear
        // cell becomes pinched.
        for j in (y0 + 1)..y1 {
            for i in (x0 + 1)..x1 {
                let mut pinched = false;
                'outer: for k in (i - 1)..(i + 2) {
                    for l in (j - 1)..(j + 2) {
                        if self.wall[self.idx(k, l)].ty != CellType::Clear {
                            pinched = true;
                            break 'outer;
                        }
                    }
                }
                let ci = self.idx(i, j);
                self.wall[ci].pinched = pinched;
            }
        }
        for j in y0..=y1 {
            for i in x0..=x1 {
                let ci = self.idx(i, j);
                if self.wall[ci].pinched && self.wall[ci].ty == CellType::Clear {
                    self.wall[ci].ty = CellType::Cliff;
                    self.wall[ci].bridge = 0;
                    self.wall[ci].bridge_over_impassable = false;
                    self.wall[ci].bridge_height = 0.0;
                }
                self.wall[ci].pinched = false;
            }
        }
    }

    /// `PathfindLayer::classifyWallMapCell`: sample the cell's four corners; four on the wall make
    /// it clear, one to three make it a cliff (the retail-compatible Generals behaviour), none is
    /// impassable.
    fn classify_wall_map_cell(&self, i: i32, j: i32) -> CellType {
        let l = i as f32 * PATHFIND_CELL_SIZE_F;
        let t = j as f32 * PATHFIND_CELL_SIZE_F;
        let r = l + PATHFIND_CELL_SIZE_F;
        let b = t + PATHFIND_CELL_SIZE_F;
        let pts = [Coord3D::new(l, t, 0.0), Coord3D::new(l, b, 0.0), Coord3D::new(r, b, 0.0), Coord3D::new(r, t, 0.0)];
        let count = pts.iter().filter(|p| self.is_point_on_wall(p)).count();
        if count == 4 {
            CellType::Clear
        } else if count != 0 {
            CellType::Cliff
        } else {
            CellType::Impassable
        }
    }

    /// Cell layer bounds `(x0, y0, x1, y1)` for the wall layer, for tests/diagnostics.
    pub fn wall_extent(&self) -> Option<(i32, i32, i32, i32)> {
        self.wall_rect
    }

    /// `ZoneManager::isPassable` for the block containing `pos`, for tests/diagnostics.
    pub fn block_passable_at(&self, pos: &Coord3D) -> bool {
        let (x, y, _) = self.world_to_cell(pos);
        self.zones.is_passable(x, y)
    }

    /// `TerrainLogic::getLayerForDestination`'s wall branch: a position near the wall height and
    /// inside a wall piece is on `LAYER_WALL`, everything else on the ground.
    pub fn layer_for_destination(&self, terrain: &TerrainLogic, pos: &Coord3D) -> PathLayer {
        let ground = terrain.ground_height(pos.x, pos.y);
        let best = (pos.z - ground).abs();
        if best > self.wall_height / 2.0 && self.is_point_on_wall(pos) {
            PathLayer::Wall
        } else {
            PathLayer::Ground
        }
    }

    /// Run `f` with the wall layer swapped in as the active grid when `layer == LAYER_WALL` and a
    /// wall layer exists. The wall cells share the ground grid's dimensions, so every existing
    /// grid accessor works unchanged. No zone bookkeeping: a wall layer's cells all share the
    /// layer's single zone (`PathfindLayer::applyZone`, `AIPathfind.cpp:3749-3757`) and the swap has
    /// no C++ `markZonesDirty` site; [`zone_of`](Self::zone_of) answers that one zone.
    fn on_layer<R>(&mut self, layer: PathLayer, f: impl FnOnce(&mut Self) -> R) -> R {
        self.ensure_wall_layer();
        if layer != PathLayer::Wall || self.wall_rect.is_none() {
            return f(self);
        }
        std::mem::swap(&mut self.cells, &mut self.wall);
        self.search_on_wall = true;
        let r = f(self);
        self.search_on_wall = false;
        std::mem::swap(&mut self.cells, &mut self.wall);
        r
    }

    /// `PathfindCell::getZone()` of the searched grid. On the wall layer every cell has the layer's
    /// one zone (`PathfindLayer::applyZone`, `AIPathfind.cpp:3749-3757`), so a wall-to-wall zone
    /// comparison is always equal; the engine's zone number for the layer is not reproduced, only the
    /// sameness that decides `clientSafeQuickDoesPathExist`. The wall layer has no ground
    /// `connectLayer` (`classifyWallCells`), so an across-layer target is "not connected" on both
    /// sides — `zone_of` then answers the ground grid's zone.
    fn zone_of(&self, ci: usize) -> u32 {
        if self.search_on_wall {
            let (x, y) = self.xy(ci);
            if self.wall_rect.is_some_and(|(x0, y0, x1, y1)| x >= x0 && x <= x1 && y >= y0 && y <= y1) {
                return 1; // the wall layer's own zone: one zone for all of its member cells.
            }
        }
        self.zones.cell_zone(ci)
    }

    /// `Pathfinder::classifyFence` (`AIPathfind.cpp:4172`): a fence is a thin oriented strip (half-length
    /// `FenceWidth/2`, half-thickness one tenth of a cell) rather than a footprint; both C++ branches use this
    /// loop. The engine's non-retail branch re-zones the touched cell bounds (`updateZonesForModify`, :4273);
    /// the port drops its lazy labels instead, and schedules only when a cell changed.
    fn classify_fence(&mut self, id: u32, fence_width: f32, fence_x_offset: f32, pos: &Coord3D, angle: f32, insert: bool) {
        let half_x = fence_width / 2.0;
        let half_y = PATHFIND_CELL_SIZE_F / 10.0;
        let c = cos(angle);
        let s = sin(angle);
        const STEP_SIZE: f32 = PATHFIND_CELL_SIZE_F * 0.5;
        let ydx = s * STEP_SIZE;
        let ydy = -c * STEP_SIZE;
        let xdx = c * STEP_SIZE;
        let xdy = s * STEP_SIZE;
        let steps_x = real_to_int_ceil(2.0 * half_x / STEP_SIZE);
        let steps_y = real_to_int_ceil(2.0 * half_y / STEP_SIZE);
        let mut tl_x = pos.x - fence_x_offset * c - half_y * s;
        let mut tl_y = pos.y + half_y * c - fence_x_offset * s;
        // `IRegion2D cellBounds` (`AIPathfind.cpp:4200-4225`): the fence's own cell, with `hi` left to
        // the `RETAIL_COMPATIBLE_CRC` quirk — `m_classifyFenceZeroInit` (true only while
        // `processDestroyList` runs, `GameLogic.cpp:2556-2558`) gives `(0,0)`, otherwise `(1000000,
        // 1000000)` which `updateZonesForModify` clips to the map's last cell. The port removes
        // footprints at `destroyObject` (mark) time rather than in `~Object` (the destroy list), so
        // the flag is always false here; the resulting `updateZonesForModify` range is documented.
        let pos_cx = real_to_int_floor((pos.x + 0.5) / PATHFIND_CELL_SIZE_F);
        let pos_cy = real_to_int_floor((pos.y + 0.5) / PATHFIND_CELL_SIZE_F);
        let mut cell_bounds = (pos_cx, pos_cy, self.w - 1, self.h - 1);
        let mut did_anything = false;
        for _ in 0..steps_y {
            let (mut x, mut y) = (tl_x, tl_y);
            for _ in 0..steps_x {
                let cx = real_to_int_floor((x + 0.5) / PATHFIND_CELL_SIZE_F);
                let cy = real_to_int_floor((y + 0.5) / PATHFIND_CELL_SIZE_F);
                if cx >= 0 && cy >= 0 && cx < self.w && cy < self.h {
                    let ci = self.idx(cx, cy);
                    let changed = if insert {
                        self.set_type_as_obstacle(ci, id, true, false)
                    } else {
                        self.remove_obstacle(ci, id)
                    };
                    if changed {
                        did_anything = true;
                        // `m_map[cx][cy].setZone(UNINITIALIZED_ZONE)` (`:4253-4262`).
                        self.zones.set_uninitialized(ci);
                    }
                    if cell_bounds.0 > cx {
                        cell_bounds.0 = cx;
                    }
                    if cell_bounds.1 > cy {
                        cell_bounds.1 = cy;
                    }
                    if cell_bounds.2 < cx {
                        cell_bounds.2 = cx;
                    }
                    if cell_bounds.3 < cy {
                        cell_bounds.3 = cy;
                    }
                }
                x += xdx;
                y += xdy;
            }
            tl_x += ydx;
            tl_y += ydy;
        }
        // `if (didAnything) { markZonesDirty(); updateZonesForModify(...); }` (`:4271-4277`; the
        // unconditional :4175 mark is the `RTS_GENERALS && RETAIL_COMPATIBLE_PATHFINDING` branch,
        // which this build does not compile).
        if did_anything {
            self.mark_zones_dirty();
            self.update_zones_for_modify(cell_bounds);
        }
    }

    /// `PathfindZoneManager::updateZonesForModify` over the current grid.
    fn update_zones_for_modify(&mut self, bounds: (i32, i32, i32, i32)) {
        let (w, h) = (self.w, self.h);
        let cells = &self.cells;
        self.zones.update_zones_for_modify(w, h, |i| cells[i].ty, bounds);
    }

    /// `PathfindCell::setTypeAsObstacle` (no rubble; `isFence` records `m_obstacleIsFence`).
    fn set_type_as_obstacle(&mut self, ci: usize, id: u32, is_fence: bool, transparent: bool) -> bool {
        let c = &mut self.cells[ci];
        if c.ty != CellType::Clear && c.ty != CellType::Impassable {
            return false;
        }
        c.ty = CellType::Obstacle;
        c.obstacle = id;
        c.fence = is_fence;
        c.obstacle_transparent = transparent;
        true
    }

    /// `PathfindCell::removeObstacle`.
    fn remove_obstacle(&mut self, ci: usize, id: u32) -> bool {
        let c = &mut self.cells[ci];
        if c.ty == CellType::Rubble {
            c.ty = CellType::Clear;
        }
        if c.obstacle != id {
            return false;
        }
        c.ty = if c.bridge_over_impassable { CellType::Clear } else { c.base_ty };
        c.obstacle = 0;
        c.fence = false;
        c.obstacle_transparent = false;
        true
    }

    /// `Pathfinder::createAWallFromMyFootprint` (`AIPathfind.h:181`): mark the object's footprint as
    /// cells the pathfinder avoids, without `classifyObjectFootprint`'s checks that limit it to
    /// immobile, non-small structures. The engine uses it for a stopped train
    /// (`RailroadBehavior::makeAWallOutOfThisTrain`) and for tensile formations. The object must not
    /// move before [`remove_wall_from_footprint`](Self::remove_wall_from_footprint).
    pub fn create_wall_from_footprint(&mut self, id: ObjectId, geom: &Geometry, pos: &Coord3D, angle: f32) {
        self.internal_classify_object_footprint(id.0, geom, pos, angle, true, false);
    }

    /// `Pathfinder::removeWallFromMyFootprint` (`AIPathfind.h:182`): undo
    /// [`create_wall_from_footprint`](Self::create_wall_from_footprint) for the same object.
    pub fn remove_wall_from_footprint(&mut self, id: ObjectId, geom: &Geometry, pos: &Coord3D, angle: f32) {
        self.internal_classify_object_footprint(id.0, geom, pos, angle, false, false);
    }

    /// `Pathfinder::classifyObjectFootprint`: add (`insert`) or remove a structure's cells.
    ///
    /// Only immobile, non-small structures not hovering above the ground count as obstacles.
    pub fn classify_object_footprint(
        &mut self,
        terrain: &TerrainLogic,
        id: ObjectId,
        template: &ObjectTemplate,
        pos: &Coord3D,
        angle: f32,
        insert: bool,
    ) {
        if template.is_kind_of(kind_of::MINE) {
            return; // don't pathfind around mines (`AIPathfind.cpp:4285`).
        }
        if template.is_bridge {
            return; // Bridge decks are the driving surface, not an obstacle.
        }
        if template.is_kind_of(kind_of::BRIDGE_TOWER) {
            return; // Bridge towers sit on the bridge layer, not the ground grid.
        }
        // `classifyFence` (`AIPathfind.cpp:4297`): a thin oriented strip, not a footprint.
        if template.fence_width > 0.0 && !template.defensive_wall {
            self.classify_fence(id.0, template.fence_width, template.fence_x_offset, pos, angle, insert);
            return;
        }
        if !template.is_kind_of(kind_of::STRUCTURE) {
            return; // Only path around structures.
        }
        if template.locomotor.is_some() {
            return; // mobile aren't obstacles.
        }
        if template.geometry.is_small {
            return;
        }
        if pos.z - terrain.ground_height(pos.x, pos.y) > PATHFIND_CELL_SIZE_F {
            return; // Don't add bounds that are up in the air.
        }
        self.internal_classify_object_footprint(id.0, &template.geometry, pos, angle, insert, template.collide_kinds & crate::template::collide_kind::CAN_SEE_THROUGH_STRUCTURE != 0);
    }

    /// `Pathfinder::internal_classifyObjectFootprint`. `transparent` is the object's
    /// `KINDOF_CAN_SEE_THROUGH_STRUCTURE` (`PathfindCell::setTypeAsObstacle`, `AIPathfind.cpp:2035`).
    fn internal_classify_object_footprint(&mut self, id: u32, geom: &Geometry, pos: &Coord3D, angle: f32, insert: bool, transparent: bool) {
        // IRegion2D cellBounds starts as the cell holding the object.
        let lo_x = real_to_int_floor((pos.x + 0.5) / PATHFIND_CELL_SIZE_F);
        let lo_y = real_to_int_floor((pos.y + 0.5) / PATHFIND_CELL_SIZE_F);
        let mut b = (lo_x, lo_y, lo_x, lo_y); // lo.x, lo.y, hi.x, hi.y

        // Marks / unmarks one cell and grows the bounds. `cx < hi.x` is the original's strict bound.
        let mut touch = |pf: &mut Pathfinder, cx: i32, cy: i32| {
            if cx >= 0 && cy >= 0 && cx < pf.w - 1 && cy < pf.h - 1 {
                let ci = pf.idx(cx, cy);
                let changed = if insert {
                    pf.set_type_as_obstacle(ci, id, false, transparent)
                } else {
                    pf.remove_obstacle(ci, id)
                };
                if changed {
                    // `m_map[cx][cy].setZone(PathfindZoneManager::UNINITIALIZED_ZONE)` when
                    // `setTypeAsObstacle`/`removeObstacle` reports a change (`AIPathfind.cpp:4405-4420`,
                    // `:4500-4518`).
                    pf.zones.set_uninitialized(ci);
                }
                if b.0 > cx {
                    b.0 = cx;
                }
                if b.1 > cy {
                    b.1 = cy;
                }
                if b.2 < cx {
                    b.2 = cx;
                }
                if b.3 < cy {
                    b.3 = cy;
                }
            }
        };

        match geom.shape {
            GeometryShape::Box => {
                // `case GEOMETRY_BOX:` marks the zones dirty before filling (`AIPathfind.cpp:4387-4389`), even
                // when no cell changes.
                self.mark_zones_dirty();
                let half_x = geom.major_radius;
                let half_y = geom.minor_radius;
                let c = cos(angle);
                let s = sin(angle);
                // Smaller than a cell to avoid aliasing problems.
                const STEP_SIZE: f32 = PATHFIND_CELL_SIZE_F * 0.5;
                let ydx = s * STEP_SIZE;
                let ydy = -c * STEP_SIZE;
                let xdx = c * STEP_SIZE;
                let xdy = s * STEP_SIZE;
                let steps_x = real_to_int_ceil(2.0 * half_x / STEP_SIZE);
                let steps_y = real_to_int_ceil(2.0 * half_y / STEP_SIZE);
                let mut tl_x = pos.x - half_x * c - half_y * s;
                let mut tl_y = pos.y + half_y * c - half_x * s;
                for _ in 0..steps_y {
                    let (mut x, mut y) = (tl_x, tl_y);
                    for _ in 0..steps_x {
                        let cx = real_to_int_floor((x + 0.5) / PATHFIND_CELL_SIZE_F);
                        let cy = real_to_int_floor((y + 0.5) / PATHFIND_CELL_SIZE_F);
                        touch(self, cx, cy);
                        x += xdx;
                        y += xdy;
                    }
                    tl_x += ydx;
                    tl_y += ydy;
                }
            }
            GeometryShape::Sphere | GeometryShape::Cylinder => {
                // `case GEOMETRY_SPHERE:` / `case GEOMETRY_CYLINDER:` mark the zones dirty before filling
                // (`AIPathfind.cpp:4460-4463`), even when no cell changes.
                self.mark_zones_dirty();
                // Not quite right, but close enough (as the original says).
                let radius = geom.major_radius;
                let top_left_x = real_to_int_floor(0.5 + (pos.x - radius) / PATHFIND_CELL_SIZE_F) - 1;
                let top_left_y = real_to_int_floor(0.5 + (pos.y - radius) / PATHFIND_CELL_SIZE_F) - 1;
                let mut size = radius / PATHFIND_CELL_SIZE_F;
                let center_x = pos.x / PATHFIND_CELL_SIZE_F;
                let center_y = pos.y / PATHFIND_CELL_SIZE_F;
                size += 0.4;
                let r2 = size * size;
                let bottom_right_x = (top_left_x as f32 + 2.0 * size + 2.0) as i32;
                let bottom_right_y = (top_left_y as f32 + 2.0 * size + 2.0) as i32;
                for j in top_left_y..bottom_right_y {
                    for i in top_left_x..bottom_right_x {
                        let dx = i as f32 + 0.5 - center_x;
                        let dy = j as f32 + 0.5 - center_y;
                        if dx * dx + dy * dy <= r2 {
                            touch(self, i, j);
                        }
                    }
                }
            }
        }

        // `m_zoneManager.updateZonesForModify(m_map, m_layers, cellBounds, m_extent)` (`AIPathfind.cpp:4541-4542`)
        // runs over the touched cells *before* `cellBounds` is grown by 2 (`:4544-4547`).
        self.update_zones_for_modify(b);
        // cellBounds grown by 2 and clipped to the grid.
        let lo_x = (b.0 - 2).max(0);
        let lo_y = (b.1 - 2).max(0);
        let hi_x = (b.2 + 2).min(self.w - 1);
        let hi_y = (b.3 + 2).min(self.h - 1);
        if lo_x > hi_x || lo_y > hi_y {
            return;
        }

        if !insert {
            for j in lo_y..=hi_y {
                for i in lo_x..=hi_x {
                    let ci = self.idx(i, j);
                    if self.cells[ci].ty == CellType::Impassable {
                        self.cells[ci].ty = CellType::Clear;
                    }
                }
            }
        }
        // Check for pinched cells, and close them off.
        for j in lo_y..=hi_y {
            for i in lo_x..=hi_x {
                let ci = self.idx(i, j);
                self.cells[ci].pinched = false;
                if self.cells[ci].ty == CellType::Clear {
                    let mut total = 0;
                    let mut orthogonal = 0;
                    for k in (i - 1)..(i + 2) {
                        if k < 0 || k > self.w - 1 {
                            continue;
                        }
                        for l in (j - 1)..(j + 2) {
                            if l < 0 || l > self.h - 1 {
                                continue;
                            }
                            if k == i && j == l {
                                continue;
                            }
                            if self.cells[self.idx(k, l)].ty == CellType::Clear {
                                total += 1;
                                if k == i || l == j {
                                    orthogonal += 1;
                                }
                            }
                        }
                    }
                    // If the total open cells are < 2 or total cells < 4, we are pinched.
                    if orthogonal < 2 || total < 4 {
                        self.cells[ci].pinched = true;
                    }
                }
            }
        }
        // RETAIL_COMPATIBLE_PATHFINDING: pinched clear cells become impassable.
        for j in lo_y..=hi_y {
            for i in lo_x..=hi_x {
                let ci = self.idx(i, j);
                if self.cells[ci].pinched && self.cells[ci].ty == CellType::Clear {
                    self.cells[ci].ty = CellType::Impassable;
                    self.cells[ci].pinched = false;
                }
            }
        }
        // Expand building bounds 1 cell.
        for j in lo_y..=hi_y {
            for i in lo_x..=hi_x {
                let ci = self.idx(i, j);
                if self.cells[ci].ty == CellType::Clear {
                    let mut adjacent = false;
                    'outer: for k in (i - 1)..(i + 2) {
                        if k < 0 || k > self.w - 1 {
                            continue;
                        }
                        for l in (j - 1)..(j + 2) {
                            if l < 0 || l > self.h - 1 {
                                continue;
                            }
                            if k == i && l == j {
                                continue;
                            }
                            if k != i && l != j {
                                continue;
                            }
                            if self.cells[self.idx(k, l)].ty == CellType::Obstacle {
                                adjacent = true;
                                break 'outer;
                            }
                        }
                    }
                    if adjacent {
                        self.cells[ci].pinched = true;
                    }
                }
            }
        }
    }

    /// Hash of the static grid (cell types and pinched flags), for determinism checks.
    pub fn crc(&self, c: &mut Crc) {
        c.add_i32(self.w);
        c.add_i32(self.h);
        for cell in &self.cells {
            c.add_u32(cell.ty as u32 | (u32::from(cell.pinched) << 8));
            c.add_u32(cell.obstacle);
        }
    }

    // ---- queries -----------------------------------------------------------------------------

    /// `validMovementPosition(isCrusher, surfaces, toCell)` (`AIPathfind.cpp:4985-5006`): an obstacle
    /// cell the search is told to ignore passes, and a *fence* passes for a crusher
    /// (`isCrusher && toCell->isObstacleFence()`); otherwise the cell's surfaces must intersect the
    /// locomotor's. `fromCell` is unused by the engine.
    fn valid_movement(&self, surfaces: u32, to: usize, crusher: bool) -> bool {
        self.valid_movement_cell(surfaces, &self.cells[to], crusher)
    }

    /// [`valid_movement`](Self::valid_movement) over an already-fetched cell.
    fn valid_movement_cell(&self, surfaces: u32, c: &Cell, crusher: bool) -> bool {
        if self.ignore_obstacle != 0 && c.ty == CellType::Obstacle && c.obstacle == self.ignore_obstacle {
            return true;
        }
        if crusher && c.fence {
            return true;
        }
        surfaces_for_cell_type(c.ty) & surfaces != 0
    }

    /// `validMovementPosition(isCrusher, layer, locomotorSet, const Coord3D*)` on the active grid.
    pub fn valid_movement_position(&self, surfaces: u32, pos: &Coord3D, crusher: bool) -> bool {
        let x = (pos.x / PATHFIND_CELL_SIZE_F) as i32;
        let y = (pos.y / PATHFIND_CELL_SIZE_F) as i32;
        self.get_cell(x, y).is_some_and(|c| self.valid_movement(surfaces, c, crusher))
    }

    /// `checkDestination` without the unit/relationship bookkeeping: every cell under the footprint
    /// must exist, be passable for `surfaces` and not be an obstacle or impassable. `ignore_obstacle`
    /// is the *unit's* ignored obstacle (`AIUpdateInterface::getIgnoredObstacleID()`,
    /// `AIPathfind.cpp:5023`), not the pathfinder's search-scoped one.
    fn check_destination(&self, obj: &PathObject, cx: i32, cy: i32, radius: i32, center: bool) -> bool {
        let above = radius + i32::from(center);
        for i in (cx - radius)..(cx + above) {
            for j in (cy - radius)..(cy + above) {
                let Some(ci) = self.get_cell(i, j) else { return false };
                let c = &self.cells[ci];
                // `if (checkForAircraft) { if (!cell->isAircraftGoal()) continue; if
                // (cell->getGoalAircraft() == objID) continue; return false; }` (`AIPathfind.cpp:5030-5038`):
                // for an aircraft that adjusts its destination the goal-cell marks are the *whole* test —
                // the obstacle, impassable and unit tests below are not reached for it. Its goal cells are
                // the ones `updateAircraftGoal` marked (`setGoalAircraft`, `AIPathfind.cpp:1462-1490`, from
                // the marking loop at `:9944-9954`).
                if obj.aircraft_that_adjusts_destination {
                    if !c.aircraft_goal {
                        continue;
                    }
                    if c.goal_aircraft == obj.id.0 {
                        continue;
                    }
                    return false;
                }
                if c.ty == CellType::Obstacle {
                    if obj.ignore_obstacle.0 != 0 && c.obstacle == obj.ignore_obstacle.0 {
                        continue;
                    }
                    return false;
                }
                // `IS_IMPASSABLE` (AIPathfind.cpp:5047): the only cell-type test. C++ has no surface-mask test here:
                // a water, cliff or rubble footprint cell does not block a destination (the path gate of
                // `checkForAdjust` judges the centre cell's surfaces through `validMovementPosition`).
                if is_impassable(c.ty) {
                    return false;
                }
            }
        }
        true
    }

    /// `checkForMovement` (AIPathfind.cpp:5097) reduced to its off-map test: the original returns
    /// false only off the map, then the caller reacts to `enemyFixed` / `allyFixedCount` (enemy
    /// fixed units block; fixed allies merely add cost and clear over the main search, and block
    /// only the straight-line pre-pass). We use [`pos_free`](Self::pos_free) when reserving a goal
    /// instead, and keep A* unit-free so a group ordered through its own ranks cannot strand
    /// itself.
    fn check_for_movement(&self, cx: i32, cy: i32, radius: i32, center: bool) -> bool {
        let above = radius + i32::from(center);
        for i in (cx - radius)..(cx + above) {
            for j in (cy - radius)..(cy + above) {
                if self.get_cell(i, j).is_none() {
                    return false;
                }
            }
        }
        true
    }

    // ---- unit and goal cells (updatePos / updateGoal / adjustDestination) -------------------

    /// Cell coordinates of a world position (`updatePos`): centered units use the cell containing
    /// the point, others the half-cell-adjusted cell.
    fn cell_coords(pos: &Coord3D, center: bool) -> (i32, i32) {
        if center {
            (real_to_int_floor(pos.x / PATHFIND_CELL_SIZE_F), real_to_int_floor(pos.y / PATHFIND_CELL_SIZE_F))
        } else {
            (real_to_int_floor(0.5 + pos.x / PATHFIND_CELL_SIZE_F), real_to_int_floor(0.5 + pos.y / PATHFIND_CELL_SIZE_F))
        }
    }

    /// `Pathfinder::updatePos` (AIPathfind.cpp:10023): move a mobile unit's position footprint.
    /// `_moving` (the caller's `isMoving()`) is not stored: `moveAllies` reads `isMoving` live at each visit
    /// (`AIPathfind.cpp:10262`, [`GameLogic::move_allies`]), so a copy here would be stale. The cell flags are not
    /// derived from it (see [`CellFlags`]).
    pub fn update_unit_pos(&mut self, obj: &PathObject, pos: &Coord3D, _moving: bool) {
        if obj.id == ObjectId::INVALID {
            return;
        }
        let (radius, center) = obj.radius_and_center();
        let (cx, cy) = Self::cell_coords(pos, center);
        let id = obj.id.0;
        self.unit_objs.insert(id, *obj);
        self.unit_pos.insert(id, *pos);
        let f = Footprint { cx, cy, radius, center, player: obj.player, infantry: obj.infantry };
        if self.unit_cells.get(&id).is_some_and(|p| p.cx == cx && p.cy == cy) {
            return; // `if (newCell == curCell) return;` (`:10075-10078`): no setPosUnit, flags unchanged.
        }
        self.clear_pos(id);
        self.unit_cells.insert(id, f);
        self.write_pos(id, f);
    }

    /// `Pathfinder::removeUnitFromPathfindMap` (AIPathfind.cpp:10185): `removePos`, then `removeGoal`.
    pub fn clear_unit(&mut self, id: ObjectId) {
        self.unit_objs.remove(&id.0);
        self.unit_pos.remove(&id.0);
        self.clear_pos(id.0);
        self.remove_goal(id.0);
    }


    /// `PathfindCell::setPosUnit` (`AIPathfind.cpp:1492-1531`): set or clear a cell's position unit
    /// and re-derive its flags. `id == 0` removes the position. Setting a unit on a cell whose current
    /// position unit is already its own goal unit returns without writing ("A unit is already
    /// occupying this cell"). The unit's owner and infantry bit are recorded for the relationship and
    /// `INFANTRY_MOVES_THROUGH_INFANTRY` tests.
    fn set_pos_unit(cell: &mut Cell, id: u32, player: crate::object::PlayerIndex, infantry: bool) {
        if id == 0 {
            if cell.pos_unit != 0 || cell.goal_unit != 0 {
                cell.pos_unit = 0;
                cell.pos_player = 0;
                cell.pos_infantry = false;
                cell.flags = if cell.goal_unit == 0 { CellFlags::NoUnits } else { CellFlags::UnitGoal };
            }
        } else {
            if cell.goal_unit != 0 && cell.goal_unit == cell.pos_unit {
                return;
            }
            cell.pos_unit = id;
            cell.pos_player = player;
            cell.pos_infantry = infantry;
            cell.flags = if id == cell.goal_unit {
                CellFlags::UnitPresentFixed
            } else if cell.goal_unit == 0 {
                CellFlags::UnitPresentMoving
            } else {
                CellFlags::UnitGoalOtherMoving
            };
        }
    }

    /// `PathfindCell::setGoalUnit` (`AIPathfind.cpp:1420-1455`): set or clear a cell's goal unit and
    /// re-derive its flags. Clearing a goal leaves `UNIT_PRESENT_MOVING` when a unit stands here.
    fn set_goal_unit(cell: &mut Cell, id: u32) {
        if id == 0 {
            if cell.pos_unit != 0 || cell.goal_unit != 0 {
                cell.goal_unit = 0;
                cell.flags = if cell.pos_unit == 0 { CellFlags::NoUnits } else { CellFlags::UnitPresentMoving };
            }
        } else {
            cell.goal_unit = id;
            cell.flags = if id == cell.pos_unit {
                CellFlags::UnitPresentFixed
            } else if cell.pos_unit == 0 {
                CellFlags::UnitGoal
            } else {
                CellFlags::UnitGoalOtherMoving
            };
        }
    }

    fn write_pos(&mut self, id: u32, f: Footprint) {
        let (x0, y0, x1, y1) = footprint_bounds(f);
        for i in x0..x1 {
            for j in y0..y1 {
                if let Some(ci) = self.get_cell(i, j) {
                    Self::set_pos_unit(&mut self.cells[ci], id, f.player, f.infantry);
                }
            }
        }
    }

    fn clear_pos(&mut self, id: u32) {
        let Some(f) = self.unit_cells.remove(&id) else { return };
        let (x0, y0, x1, y1) = footprint_bounds(f);
        for i in x0..x1 {
            for j in y0..y1 {
                if let Some(ci) = self.get_cell(i, j) {
                    if self.cells[ci].pos_unit == id {
                        Self::set_pos_unit(&mut self.cells[ci], 0, 0, false);
                    }
                }
            }
        }
    }

    /// `Pathfinder::removeGoal` (AIPathfind.cpp:9963): forget the unit's goal cell and clear the goal-unit marks it made
    /// over the cells of its goal footprint. A zero radius is widened to one for the removal (`:9971-9975`), and only
    /// cells whose goal unit is this unit are cleared (`:9994`). The unit's own cell record is dropped with it.
    pub(crate) fn remove_goal(&mut self, id: u32) {
        let Some(f) = self.unit_goals.remove(&id) else { return };
        let r = f.radius.max(1);
        let above = r + i32::from(f.center);
        for i in (f.cx - r)..(f.cx + above) {
            for j in (f.cy - r)..(f.cy + above) {
                if let Some(ci) = self.get_cell(i, j) {
                    if self.cells[ci].goal_unit == id {
                        Self::set_goal_unit(&mut self.cells[ci], 0);
                    }
                    if self.cells[ci].goal_aircraft == id {
                        Self::set_goal_aircraft(&mut self.cells[ci], 0);
                    }
                }
            }
        }
    }

    /// `PathfindCell::setGoalAircraft` (`AIPathfind.cpp:1466-1490`): the aircraft-goal mark, separate
    /// from the unit goal and independent of `m_flags`.
    fn set_goal_aircraft(cell: &mut Cell, id: u32) {
        if id == 0 {
            if cell.goal_aircraft != 0 {
                cell.goal_aircraft = 0;
                cell.aircraft_goal = false;
            }
        } else {
            cell.goal_aircraft = id;
            cell.aircraft_goal = true;
        }
    }

    /// `Pathfinder::updateAircraftGoal` (`AIPathfind.cpp:9905-9951`): move an aircraft's goal. The
    /// old goal is removed first (`:9909`), a ground mover returns (`:9914`), and only an aircraft
    /// that adjusts its destination marks the cells (`:9917`). The aircraft marks are separate from
    /// the unit-goal marks, so a ground mover's `checkDestination`/`checkForMovement` never see them.
    pub fn update_aircraft_goal(&mut self, obj: &PathObject, new_pos: &Coord3D, doing_ground_movement: bool) {
        if obj.id == ObjectId::INVALID {
            return;
        }
        self.remove_goal(obj.id.0);
        if doing_ground_movement {
            return; // "shouldn't really happen, but just in case."
        }
        if !obj.aircraft_that_adjusts_destination {
            return;
        }
        let (radius, center) = obj.radius_and_center();
        let (ncx, ncy) = Self::cell_coords(new_pos, center);
        if self.unit_goals.get(&obj.id.0).is_some_and(|f| f.cx == ncx && f.cy == ncy) {
            return;
        }
        let f = Footprint { cx: ncx, cy: ncy, radius, center, player: obj.player, infantry: obj.infantry };
        self.unit_goals.insert(obj.id.0, f);
        let (x0, y0, x1, y1) = footprint_bounds(f);
        for i in x0..x1 {
            for j in y0..y1 {
                if let Some(ci) = self.get_cell(i, j) {
                    Self::set_goal_aircraft(&mut self.cells[ci], obj.id.0);
                }
            }
        }
    }

    /// `Pathfinder::updateGoal`'s ground-movement dispatch (`AIPathfind.cpp:9805-9827`): a unit that
    /// is not doing ground movement goes to [`update_aircraft_goal`](Self::update_aircraft_goal)
    /// instead of marking a ground goal. `doing_ground_movement` is
    /// `AIUpdateInterface::isDoingGroundMovement` (`AIUpdate.cpp:2359`), which
    /// [`GameLogic::is_doing_ground_movement`](crate::GameLogic) answers from the live AI's locomotor
    /// (a locomotor whose legal surfaces include air is not a ground mover).
    pub fn update_goal_dispatch(&mut self, obj: &PathObject, new_pos: &Coord3D, doing_ground_movement: bool) {
        if obj.id == ObjectId::INVALID {
            return;
        }
        if doing_ground_movement {
            self.update_goal(obj, new_pos);
        } else {
            self.update_aircraft_goal(obj, new_pos, doing_ground_movement);
        }
    }

    /// `Pathfinder::updateGoal` (AIPathfind.cpp:9807) for a ground mover: the goal cell of `new_pos` for this unit's
    /// footprint (`newCell`, `:9841-9848`). Unchanged cells return early (`:9851`). Otherwise the old goal is removed
    /// (`removeGoal`, `:9862`), the cell is recorded as the unit's goal (`setPathfindGoalCell`, `:9865`), and every cell
    /// of the footprint takes this unit as its goal unit (`setGoalUnit`, `:9882`/`:9894`). `setGoalUnit` overwrites a
    /// goal another unit holds: only the warning differs, so the mark is the unit's own. A cell where this unit also
    /// stands becomes `UNIT_PRESENT_FIXED` (on its own goal). The layer is ground only (the port has no
    /// bridge-layer grid, so `doLayer` is never set). The caller decides ground movement
    /// (`isDoingGroundMovement`); an aircraft goal is `update_aircraft_goal`.
    pub fn update_goal(&mut self, obj: &PathObject, new_pos: &Coord3D) {
        if obj.id == ObjectId::INVALID {
            return;
        }
        let (radius, center) = obj.radius_and_center();
        let (ncx, ncy) = Self::cell_coords(new_pos, center);
        if self.unit_goals.get(&obj.id.0).is_some_and(|f| f.cx == ncx && f.cy == ncy) {
            return;
        }
        self.remove_goal(obj.id.0);
        let f = Footprint { cx: ncx, cy: ncy, radius, center, player: obj.player, infantry: obj.infantry };
        self.unit_goals.insert(obj.id.0, f);
        let (x0, y0, x1, y1) = footprint_bounds(f);
        for i in x0..x1 {
            for j in y0..y1 {
                if let Some(ci) = self.get_cell(i, j) {
                    Self::set_goal_unit(&mut self.cells[ci], obj.id.0);
                }
            }
        }
    }

    /// `Pathfinder::goalPosition` (AIPathfind.cpp:5303): the world position of the unit's goal cell, as
    /// `adjustCoordToCell` gives it for the unit's footprint centring. `None` when the unit has no goal cell (`-1`).
    pub(crate) fn goal_position(&self, terrain: &TerrainLogic, id: ObjectId) -> Option<Coord3D> {
        let f = self.unit_goals.get(&id.0)?;
        if f.cx < 0 || f.cy < 0 {
            return None;
        }
        Some(self.adjust_coord_to_cell(terrain, f.cx, f.cy, f.center))
    }

    /// `Pathfinder`'s goal cell of a unit (`AIUpdateInterface::m_pathfindGoalCell`, set by `updateGoal`,
    /// `AIPathfind.cpp:9865`): `(cx, cy)`, or `None` when the unit has no goal cell (`-1`).
    pub(crate) fn goal_cell(&self, id: ObjectId) -> Option<(i32, i32)> {
        let f = self.unit_goals.get(&id.0)?;
        if f.cx < 0 || f.cy < 0 {
            return None;
        }
        Some((f.cx, f.cy))
    }

    /// `Pathfinder::snapClosestGoalPosition` (AIPathfind.cpp:5235): the goal of a move whose adjust found no cell. The
    /// first step is the centre of `pos`'s own cell. That cell is kept when `checkDestination` passes, else the first
    /// passing cell of the 3x3 around it is taken. Only a unit with no radius goes on to the first 3x3 cell with no goal
    /// unit (other than its own), and then to the first cell not held by a fixed unit. Those two tests read the port's
    /// goal-unit and fixed-position marks, which stand in for `getGoalUnit()` and `getFlags()`.
    pub(crate) fn snap_closest_goal_position(&self, terrain: &TerrainLogic, obj: &PathObject, pos: &Coord3D) -> Coord3D {
        let (radius, center) = obj.radius_and_center();
        let (cx, cy) = Self::cell_coords(pos, center);
        let out = self.adjust_coord_to_cell(terrain, cx, cy, center);
        let owner = |u: u32| self.unit_goals.get(&u).map(|f| f.player);
        let passes = |i: i32, j: i32| {
            self.check_destination(obj, i, j, radius, center)
                && self.check_destination_unit(obj, i, j, radius, center, &owner)
        };
        if passes(cx, cy) {
            return out;
        }
        for i in (cx - 1)..(cx + 2) {
            for j in (cy - 1)..(cy + 2) {
                if passes(i, j) {
                    return self.adjust_coord_to_cell(terrain, i, j, center);
                }
            }
        }
        if radius > 0 {
            return out;
        }
        for i in (cx - 1)..(cx + 2) {
            for j in (cy - 1)..(cy + 2) {
                if let Some(ci) = self.get_cell(i, j) {
                    let g = self.cells[ci].goal_unit;
                    if g == 0 || g == obj.id.0 {
                        return self.adjust_coord_to_cell(terrain, i, j, center);
                    }
                }
            }
        }
        for i in (cx - 1)..(cx + 2) {
            for j in (cy - 1)..(cy + 2) {
                if let Some(ci) = self.get_cell(i, j) {
                    if self.cells[ci].flags != CellFlags::UnitPresentFixed {
                        return self.adjust_coord_to_cell(terrain, i, j, center);
                    }
                }
            }
        }
        out
    }

    /// Set the ally table (`GameLogic`'s relationship matrix): `allies[p]` is the bitmask of
    /// players allied with `p`. Unset players are treated as enemies of everyone but themselves.
    pub fn set_allies(&mut self, allies: [u32; 32]) {
        self.allies = allies;
    }

    /// Is `a` allied with `b`? (The engine's `getRelationship(...) == ALLIES`; a player is always
    /// its own ally.)
    pub fn are_allies(&self, a: crate::object::PlayerIndex, b: crate::object::PlayerIndex) -> bool {
        if a == b {
            return true;
        }
        let (a, b) = (a as usize, b as usize);
        if a >= self.allies.len() || b >= 32 {
            return false;
        }
        self.allies[a] & (1u32 << b) != 0
    }

    /// Set the enemy table (`GameLogic`'s relationship matrix): `enemies[p]` is the bitmask of
    /// players `p` is at war with (`Player::getRelationship(...) == ENEMIES`). Only the dozer
    /// exception reads it (`AIPathfind.cpp:6386-6392`).
    pub fn set_enemies(&mut self, enemies: [u32; 32]) {
        self.enemies = enemies;
    }

    /// Is `a` at war with `b`? `NEUTRAL` is not `ENEMIES`, so two players with no relationship are
    /// not enemies.
    pub fn are_enemies(&self, a: crate::object::PlayerIndex, b: crate::object::PlayerIndex) -> bool {
        if a == b {
            return false;
        }
        let (a, b) = (a as usize, b as usize);
        if a >= self.enemies.len() || b >= 32 {
            return false;
        }
        self.enemies[a] & (1u32 << b) != 0
    }

    /// Replace the obstacle→owner table (see [`Pathfinder::obstacle_owners`]). `GameLogic` calls
    /// this with the frame's live object list, standing in for `TheGameLogic->findObjectByID`
    /// (`AIPathfind.cpp:6387`).
    pub fn set_obstacle_owners(&mut self, owners: impl IntoIterator<Item = (u32, crate::object::PlayerIndex)>) {
        self.obstacle_owners.clear();
        self.obstacle_owners.extend(owners);
    }

    /// Lane 188: replace the vehicle and idle unit tables (see [`Pathfinder::vehicle_units`] and
    /// [`Pathfinder::idle_units`]). `GameLogic` calls it with the frame's unit table.
    pub fn set_unit_kinds(&mut self, vehicles: impl IntoIterator<Item = u32>, idle: impl IntoIterator<Item = u32>) {
        self.vehicle_units.clear();
        self.vehicle_units.extend(vehicles);
        self.idle_units.clear();
        self.idle_units.extend(idle);
    }

    /// Lane 191: replace the crush table (`id`, its `CrushInfo`, and whether it is `DISABLED_UNMANNED`). `GameLogic`
    /// calls it with the frame's unit table, next to [`set_unit_kinds`](Self::set_unit_kinds).
    pub fn set_unit_crush(&mut self, units: impl IntoIterator<Item = (u32, crate::modules2::CrushInfo, bool)>) {
        self.crush_units.clear();
        self.crush_units.extend(units.into_iter().map(|(id, info, unmanned)| (id, (info, unmanned))));
    }

    /// `Object::canCrushOrSquish(unit, TEST_CRUSH_OR_SQUISH)` (`Object.cpp:1121-1200`) for a mover `obj` and a fixed
    /// unit `victim`: asked by `checkDestination` (`AIPathfind.cpp:5082-5086`) and by `checkForMovement` for a fixed
    /// enemy (`:5201-5205`). The ALLIES test is the caller's. The predicate itself is
    /// [`crate::modules2::crush_or_squish`], shared with `GameLogic::can_crush_or_squish`. Both units' data come from
    /// the per-frame crush table, refreshed by `GameLogic`'s pathfinder sync (a mid-frame change to `unmanned` or to a
    /// crush level is seen on the next sync, as the unit table's other per-frame fields are). A unit missing from the
    /// table cannot crush.
    fn can_crush_unit(&self, obj: &PathObject, victim: u32) -> bool {
        let Some(&(crusher, unmanned)) = self.crush_units.get(&obj.id.0) else { return false };
        let Some(&(target, _)) = self.crush_units.get(&victim) else { return false };
        crate::modules2::crush_or_squish(crusher, unmanned, target)
    }

    /// `Pathfinder::checkForMovement` (`AIPathfind.cpp:5097-5221`): the ally/enemy occupancy of the
    /// footprint at `pos`. `consider_transient` is the caller's `TCheckMovementInfo::considerTransient`:
    /// with it, a `UNIT_PRESENT_MOVING` / `UNIT_GOAL_OTHER_MOVING` unit is classified as an ally-fixed
    /// or enemy-fixed one; without it (the A*, the straight-line pre-pass, `isLinePassable` and
    /// `getMoveAwayFromPath` all pass false) only `UNIT_PRESENT_FIXED` units count, while a moving ally
    /// still reports `ally_moving`. Exposed for tests.
    pub fn check_for_movement_full(&self, obj: &PathObject, pos: &Coord3D, consider_transient: bool) -> MovementInfo {
        let (radius, center) = obj.radius_and_center();
        let (cx, cy) = Self::cell_coords(pos, center);
        self.check_for_movement_cell(obj, cx, cy, radius, center, consider_transient)
    }

    /// `checkForMovement` for a footprint anchored at a known cell.
    fn check_for_movement_cell(&self, obj: &PathObject, cx: i32, cy: i32, radius: i32, center: bool, consider_transient: bool) -> MovementInfo {
        let f = Footprint { cx, cy, radius, center, player: obj.player, infantry: obj.infantry };
        let mut info = MovementInfo::default();
        let (x0, y0, x1, y1) = footprint_bounds(f);
        // `checkForMovement` counts each fixed ally once, even though its footprint covers several
        // cells (`allies[maxAlly]`, `maxAlly = 5`).
        let mut fixed_allies = [0u32; 5];
        let mut n_allies = 0usize;
        for i in x0..x1 {
            for j in y0..y1 {
                let Some(ci) = self.get_cell(i, j) else {
                    info.enemy_fixed = true; // off the map, so can't move here
                    continue;
                };
                let c = &self.cells[ci];
                // `flags == UNIT_GOAL || flags == UNIT_GOAL_OTHER_MOVING` -> `allyGoal = true`.
                if c.flags == CellFlags::UnitGoal || c.flags == CellFlags::UnitGoalOtherMoving {
                    info.ally_goal = true;
                } else if c.flags == CellFlags::NoUnits {
                    continue;
                }
                let pos_unit = c.pos_unit;
                if pos_unit == 0 || pos_unit == obj.id.0 {
                    continue;
                }
                // `checkForMovement` (`AIPathfind.cpp:5140,5161`): the ignored obstacle never blocks,
                // so an enter order can path onto its target.
                if pos_unit == obj.ignore_obstacle.0 {
                    continue;
                }
                // The engine looks the position unit up live; a dead unit is skipped.
                let Some(unit) = self.unit_objs.get(&pos_unit) else { continue };
                let mut check = false;
                if c.flags == CellFlags::UnitPresentMoving || c.flags == CellFlags::UnitGoalOtherMoving {
                    if self.are_allies(obj.player, unit.player) {
                        info.ally_moving = true;
                    }
                    if consider_transient {
                        check = true;
                    }
                }
                if c.flags == CellFlags::UnitPresentFixed {
                    check = true;
                }
                if check && pos_unit == obj.ignore_obstacle.0 {
                    check = false; // "Don't check if it's the ignored obstacle."
                }
                if !check {
                    continue;
                }
                // `INFANTRY_MOVES_THROUGH_INFANTRY`: checked before the relationship test.
                if obj.infantry && unit.infantry {
                    continue;
                }
                if self.are_allies(obj.player, unit.player) {
                    if !fixed_allies[..n_allies].contains(&pos_unit) {
                        info.ally_fixed_count += 1;
                        if n_allies < fixed_allies.len() {
                            fixed_allies[n_allies] = pos_unit;
                            n_allies += 1;
                        }
                    }
                } else if !self.can_crush_unit(obj, pos_unit) {
                    // `if (!canCrush) info.enemyFixed = TRUE` (AIPathfind.cpp:5201-5205), with `canCrush =
                    // obj->canCrushOrSquish(unit, TEST_CRUSH_OR_SQUISH)` (lane 191 R27 F4).
                    info.enemy_fixed = true;
                }
            }
        }
        info
    }

    /// `PartitionManager::tryPosition`'s terrain/water/obstacle test (object overlap is the
    /// partition's job): can an object with these surfaces stand on this cell?
    pub fn can_place(&self, surfaces: u32, pos: &Coord3D) -> bool {
        let Some(ci) = self.cell_at(pos) else { return false };
        if self.cells[ci].ty == CellType::Cliff {
            return false;
        }
        self.valid_movement(surfaces, ci, false)
    }

    /// `Pathfinder::adjustDestination` (AIPathfind.cpp:5462), retail-compatible behaviour: pick a
    /// valid final cell near `dest` whose goal units are not allies' (`checkDestination`). It reserves nothing: the
    /// caller's `updateGoal` takes the cell. With a
    /// `group_dest` the cell is tightened toward the group destination (`tightenPath`) and rejected
    /// when the route from the group destination would be too long (`checkPathCost`).
    pub fn adjust_destination(&mut self, terrain: &TerrainLogic, obj: &PathObject, dest: &Coord3D, group_dest: Option<&Coord3D>) -> Option<Coord3D> {
        if obj.id == ObjectId::INVALID {
            return Some(*dest);
        }
        let (radius, center) = obj.radius_and_center();
        let (mut cx, mut cy) = Self::cell_coords(dest, center);
        if let Some(adj) = self.check_for_adjust(terrain, obj, cx, cy, radius, center, dest, group_dest) {
            return Some(adj);
        }
        // Expanding counter-clockwise spiral (AIPathfind.cpp:5510): right, up, left, down; the
        // segment grows after every pair of directions.
        let mut limit = MAX_ADJUSTMENT_CELL_COUNT;
        let mut segment = 1;
        const DIRS: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
        while limit > 0 {
            for (dir, &(dx, dy)) in DIRS.iter().enumerate() {
                for _ in 0..segment {
                    cx += dx;
                    cy += dy;
                    limit -= 1;
                    if let Some(adj) = self.check_for_adjust(terrain, obj, cx, cy, radius, center, dest, group_dest) {
                        return Some(adj);
                    }
                }
                if dir & 1 == 1 {
                    segment += 1;
                }
            }
        }
        if group_dest.is_some() {
            // Didn't work, so just do a simple adjust (AIPathfind.cpp:5537).
            return self.adjust_destination(terrain, obj, dest, None);
        }
        None
    }

    /// `Pathfinder::adjustToPossibleDestination` (AIPathfind.cpp:5672): find a pathable destination near
    /// `dest` for `obj`, writing it back into `dest`. Returns false when no spot is found (or the destination is
    /// off the map). The start cell is the cell `from` clips to (`getClippedCell`); an obstacle start counts as
    /// terrain for the zone test (`startingInObstacle`). The zone test (`checkForPossible`) writes the candidate
    /// centre into `dest` before the footprint test (`checkDestination`), so a rejected candidate still moves
    /// `dest` if its zone matched, as in C++. Ground layer only: the bridge-deck layer is not searched.
    /// `unit_player` maps a goal-unit id to its owner (`TheGameLogic->findObjectByID(...)->getControllingPlayer()`).
    pub fn adjust_to_possible_destination(
        &self,
        terrain: &TerrainLogic,
        obj: &PathObject,
        from: &Coord3D,
        dest: &mut Coord3D,
        unit_player: &dyn Fn(u32) -> Option<crate::object::PlayerIndex>,
    ) -> bool {
        let (radius, center) = obj.radius_and_center();
        let mut adjust = *dest;
        if !center {
            adjust.x += PATHFIND_CELL_SIZE_F / 2.0;
            adjust.y += PATHFIND_CELL_SIZE_F / 2.0;
        }
        let (gx, gy, overflow) = self.world_to_cell(&adjust);
        if overflow {
            return false; // outside of bounds.
        }
        let Some(goal_ci) = self.get_cell(gx, gy) else { return false };
        let (fx, fy, _) = self.world_to_cell(from);
        let Some(parent_ci) = self.get_cell(fx, fy) else { return false };
        let is_crusher = obj.crusher;
        let surfaces = obj.surfaces;
        // `isObstacle` and the start zone (`startingInObstacle`, :5690-5703).
        let is_obstacle = self.cells[parent_ci].ty == CellType::Obstacle;
        let mut zone1 = self.zones.effective_zone(surfaces, is_crusher, self.zone_of(parent_ci));
        if is_obstacle {
            zone1 = self.zones.effective_terrain_zone(zone1);
            zone1 = self.zones.effective_zone(surfaces, is_crusher, zone1);
        }
        let zone2 = self.zones.effective_zone(surfaces, is_crusher, self.zone_of(goal_ci));
        if zone1 == zone2 && self.check_destination_unit(obj, gx, gy, radius, center, unit_player) {
            return true;
        }
        // The spiral: `delta` grows after each pair of runs, as in the C++ `while (limit>0)` body.
        let (mut i, mut j) = (gx, gy);
        let mut limit = MAX_ADJUSTMENT_CELL_COUNT;
        let mut delta = 1;
        while limit > 0 {
            for _ in 0..delta {
                i += 1;
                limit -= 1;
                if self.check_for_possible_unit(is_crusher, zone1, center, surfaces, i, j, dest, is_obstacle, terrain, obj, radius, unit_player) {
                    return true;
                }
            }
            for _ in 0..delta {
                j += 1;
                limit -= 1;
                if self.check_for_possible_unit(is_crusher, zone1, center, surfaces, i, j, dest, is_obstacle, terrain, obj, radius, unit_player) {
                    return true;
                }
            }
            delta += 1;
            for _ in 0..delta {
                i -= 1;
                limit -= 1;
                if self.check_for_possible_unit(is_crusher, zone1, center, surfaces, i, j, dest, is_obstacle, terrain, obj, radius, unit_player) {
                    return true;
                }
            }
            for _ in 0..delta {
                j -= 1;
                limit -= 1;
                if self.check_for_possible_unit(is_crusher, zone1, center, surfaces, i, j, dest, is_obstacle, terrain, obj, radius, unit_player) {
                    return true;
                }
            }
            delta += 1;
        }
        false
    }

    /// `checkForPossible` (AIPathfind.cpp:5647) followed by `checkDestination` (:5013) on the same cell, as the
    /// C++ spiral calls them: the candidate's zone must match `from_zone`, then `dest` takes the candidate's
    /// centre (`adjustCoordToCell`) before the footprint test.
    #[allow(clippy::too_many_arguments)]
    fn check_for_possible_unit(
        &self,
        is_crusher: bool,
        from_zone: u32,
        center: bool,
        surfaces: u32,
        cx: i32,
        cy: i32,
        dest: &mut Coord3D,
        starting_in_obstacle: bool,
        terrain: &TerrainLogic,
        obj: &PathObject,
        radius: i32,
        unit_player: &dyn Fn(u32) -> Option<crate::object::PlayerIndex>,
    ) -> bool {
        let Some(ci) = self.get_cell(cx, cy) else { return false };
        if is_impassable(self.cells[ci].ty) {
            return false;
        }
        let mut zone2 = self.zones.effective_zone(surfaces, is_crusher, self.zone_of(ci));
        if starting_in_obstacle {
            zone2 = self.zones.effective_terrain_zone(zone2);
        }
        if from_zone != zone2 {
            return false;
        }
        *dest = self.adjust_coord_to_cell(terrain, cx, cy, center);
        self.check_destination_unit(obj, cx, cy, radius, center, unit_player)
    }

    /// `checkDestination` (AIPathfind.cpp:5013) for a ground unit: the footprint must be on the map, not blocked by
    /// an obstacle the unit does not ignore, and not impassable. Units on it: the unit's own goal, its ignored
    /// obstacle and unowned goals are skipped; an ally's goal rejects the spot (`:5067-5089`); a `UNIT_PRESENT_FIXED`
    /// unit on a goal cell rejects it unless `canCrushOrSquish` (not modelled: a crusher is treated as unable to
    /// crush). An aircraft that adjusts its destination (`isAircraftThatAdjustsDestination`) instead tests only the
    /// aircraft-goal marks (`:5033-5046`).
    fn check_destination_unit(
        &self,
        obj: &PathObject,
        cx: i32,
        cy: i32,
        radius: i32,
        center: bool,
        unit_player: &dyn Fn(u32) -> Option<crate::object::PlayerIndex>,
    ) -> bool {
        let above = radius + i32::from(center);
        let ignore = obj.ignore_obstacle.0;
        for i in (cx - radius)..(cx + above) {
            for j in (cy - radius)..(cy + above) {
                let Some(ci) = self.get_cell(i, j) else { return false };
                let c = &self.cells[ci];
                // `if (checkForAircraft) { if (!cell->isAircraftGoal()) continue; if (getGoalAircraft()==objID)
                // continue; return false; }` (`AIPathfind.cpp:5033-5046`): before the obstacle test.
                if obj.aircraft_that_adjusts_destination {
                    if !c.aircraft_goal {
                        continue;
                    }
                    if c.goal_aircraft == obj.id.0 {
                        continue;
                    }
                    return false;
                }
                if c.ty == CellType::Obstacle {
                    if ignore != 0 && c.obstacle == ignore {
                        continue;
                    }
                    return false;
                }
                if is_impassable(c.ty) {
                    return false;
                }
                // `getGoalUnit()`: skip our own goal, the ignored obstacle, and cells with no goal.
                let goal = c.goal_unit;
                if goal == obj.id.0 || goal == ignore || goal == 0 {
                    continue;
                }
                let Some(gp) = unit_player(goal) else { continue };
                // `obj->getRelationship(unit) == ALLIES`: a player is allied with itself (`are_allies`).
                if self.are_allies(obj.player, gp) {
                    return false; // "Don't usurp your allies goals."
                }
                if c.flags == CellFlags::UnitPresentFixed && !self.can_crush_unit(obj, goal) {
                    return false; // "Don't move to an occupied cell" unless `canCrushOrSquish` (`AIPathfind.cpp:5082-5086`).
                }
            }
        }
        true
    }

    /// `checkForAdjust`'s read-only half: a valid final cell whose footprint is not reserved by
    /// another unit, without reserving it (`checkDestination` + the goal/position tests).
    fn check_for_adjust_probe(&self, terrain: &TerrainLogic, obj: &PathObject, cx: i32, cy: i32, radius: i32, center: bool) -> Option<Coord3D> {
        let ci = self.get_cell(cx, cy)?;
        if self.cells[ci].ty == CellType::Cliff {
            return None;
        }
        if !self.check_destination(obj, cx, cy, radius, center) {
            return None;
        }
        // `checkDestination`'s unit part (AIPathfind.cpp:5027-5066): the goal units of the footprint, judged by
        // their owners' relationship. The owner is the goal unit's own footprint player.
        if !self.check_destination_unit(obj, cx, cy, radius, center, &|u| self.unit_goals.get(&u).map(|f| f.player)) {
            return None;
        }
        Some(self.adjust_coord_to_cell(terrain, cx, cy, center))
    }

    /// `checkForAdjust` (`AIPathfind.cpp:5314`) without the goal reservation: the probe, the path gate
    /// against `dest`, and the `groupDest` tightening and `checkPathCost` gate. The engine's callers reserve the
    /// accepted cell themselves (`updateGoal`); the tighten callback calls this too (`tightenPathCallback`,
    /// AIPathfind.cpp:8359), which never reserves. `dest` is the destination the path gate tests from.
    #[allow(clippy::too_many_arguments)]
    fn check_for_adjust_full(
        &mut self,
        terrain: &TerrainLogic,
        obj: &PathObject,
        cx: i32,
        cy: i32,
        radius: i32,
        center: bool,
        dest: &Coord3D,
        group_dest: Option<&Coord3D>,
    ) -> Option<Coord3D> {
        let mut adjust_dest = self.check_for_adjust_probe(terrain, obj, cx, cy, radius, center)?;
        // The path gate (AIPathfind.cpp:5334-5343): `if (obj->isKindOf(KINDOF_AIRCRAFT)) adjustedPathExists = true;
        // else { ... }` — an aircraft skips the whole quick-path test, whatever its locomotor's surfaces are. Any other
        // unit needs a path from its position to the original destination, or else from that destination to the adjusted
        // cell, or else from its position to the adjusted cell. The unit's position is the one the pathfinder holds
        // (`unit_pos`).
        //
        // `checkForAdjust`'s `isHuman` logical-extent test (`:5324-5330`) is already enforced by the grid: `get_cell`
        // above rejects every cell outside it, and the port's grid *is* `m_logicalExtent` (`Pathfinder::new` sizes it from
        // `TerrainLogic::getExtent`; the engine's `m_logicalExtent` is the same `getExtent` bounds, `AIPathfind.cpp:6012-6021`,
        // while `m_extent` is the larger `getMaximumPathfindExtent`). No `PlayerType` exists in the sim either, so the
        // test has nothing else to read.
        if !obj.is_aircraft {
            let from = self.unit_pos.get(&obj.id.0).copied().unwrap_or(*dest);
            let path_exists = self.quick_does_path_exist(obj, &from, dest);
            let reachable = if !path_exists && self.quick_does_path_exist(obj, dest, &adjust_dest) {
                true
            } else {
                self.quick_does_path_exist(obj, &from, &adjust_dest)
            };
            if !reachable {
                return None;
            }
        }
        if let Some(group) = group_dest {
            self.tighten_path(terrain, obj, radius, center, &mut adjust_dest, group);
            let cost = self.check_path_cost(terrain, obj, group, &adjust_dest);
            // `Int dx = IABS(groupDest->x-adjustDest.x)` (AIPathfind.cpp:5349-5350): the distances are
            // truncated to whole units before the `1.4f*(dx+dy) < cost` test (:5351).
            let dx = (group.x - adjust_dest.x).abs() as i32;
            let dy = (group.y - adjust_dest.y).abs() as i32;
            let span = (dx + dy) as f32;
            if 1.4 * span < cost as f32 {
                return None;
            }
        }
        Some(adjust_dest)
    }

    /// `checkForAdjust` as the adjust calls it (`adjustDestination` and its spiral, AIPathfind.cpp:5462-5540):
    /// [`check_for_adjust_full`](Self::check_for_adjust_full), then reserve the accepted cell as this unit's goal.
    #[allow(clippy::too_many_arguments)]
    fn check_for_adjust(
        &mut self,
        terrain: &TerrainLogic,
        obj: &PathObject,
        cx: i32,
        cy: i32,
        radius: i32,
        center: bool,
        dest: &Coord3D,
        group_dest: Option<&Coord3D>,
    ) -> Option<Coord3D> {
        // C++ `checkForAdjust` reserves nothing (AIPathfind.cpp:5314-5360): the caller's `updateGoal` does
        // (AIStates.cpp:1667, :1806; AIUpdate.cpp:430-460).
        self.check_for_adjust_full(terrain, obj, cx, cy, radius, center, dest, group_dest)
    }

    /// `Pathfinder::tightenPath` (`AIPathfind.cpp:8387`) with `tightenPathCallback` (`:8359`): every cell along the
    /// line is a full `checkForAdjust(obj, isHuman = true, cell, &pos, nullptr)`, so it passes the path gate and
    /// the logical-extent test (the port's grid is the logical extent, so that test rejects nothing new). `pos`
    /// starts at `newDestPos`, which the caller zeroes (`RETAIL_COMPATIBLE_CRC`, `:8362`, `:8394`): the first
    /// candidate's path gate tests from (0, 0), and each later one from the last accepted cell.
    fn tighten_path(&mut self, terrain: &TerrainLogic, obj: &PathObject, radius: i32, center: bool, from: &mut Coord3D, to: &Coord3D) {
        let (sx, sy, _) = self.world_to_cell(from);
        let (ex, ey, _) = self.world_to_cell(to);
        let mut found = false;
        let mut new_dest = Coord3D::ZERO;
        for (x, y) in self.line_cells((sx, sy), (ex, ey)) {
            if let Some(adj) = self.check_for_adjust_full(terrain, obj, x, y, radius, center, &new_dest, None) {
                found = true;
                new_dest = adj;
            }
        }
        if found {
            *from = new_dest;
        }
    }

    /// `Pathfinder::checkPathCost` (`AIPathfind.cpp:8409`): the cost (in the same units as a
    /// `Coord3D` distance / 10) of the path from the group destination to `raw_to`. A plain bounded
    /// A* (no unit tests, `MAX_CELL_COUNT = 500`). Returns `MAX_COST` when unreachable.
    fn check_path_cost(&mut self, _terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, raw_to: &Coord3D) -> i32 {
        const MAX_COST: i32 = 0x7fff_0000;
        const MAX_CELL_COUNT: i32 = 500;
        if !self.map_ready {
            return 0;
        }
        if obj.id == ObjectId::INVALID {
            return MAX_COST;
        }
        let (radius, center) = obj.radius_and_center();
        let _ = (radius, center);
        let is_crusher = obj.crusher;
        let (Some(goal), Some(parent)) = (self.clipped_cell_at(raw_to), self.cell_at(from)) else { return MAX_COST };
        if !self.allocate_info(goal) {
            return MAX_COST;
        }
        if parent != goal && !self.allocate_info(parent) {
            self.clean_search();
            return MAX_COST;
        }
        if !self.valid_movement(obj.surfaces, parent, is_crusher) {
            self.clean_search();
            return MAX_COST;
        }
        self.start_pathfind(parent, goal);
        self.open.clear();
        self.put_on_open(parent);
        let mut cell_count = 0;
        const DELTA: [(i32, i32); 8] = [(1, 0), (0, 1), (-1, 0), (0, -1), (1, 1), (-1, 1), (-1, -1), (1, -1)];
        const ADJACENT: [usize; 5] = [0, 1, 2, 3, 0];
        let mut neighbor_flags = [false; 8];
        while let Some(p) = self.pop_open() {
            if p == goal {
                let cost = i32::from(self.info[p].total);
                self.clean_search();
                return cost;
            }
            self.info[p].closed = true;
            if cell_count > MAX_CELL_COUNT {
                continue; // stop expanding once the budget is used up (AIPathfind.cpp:8518)
            }
            let (px, py) = self.xy(p);
            for i in 0..8 {
                neighbor_flags[i] = false;
                let (nx, ny) = (px + DELTA[i].0, py + DELTA[i].1);
                let Some(nc) = self.get_cell(nx, ny) else { continue };
                if i >= 4 && !neighbor_flags[ADJACENT[i - 4]] && !neighbor_flags[ADJACENT[i - 3]] {
                    continue;
                }
                // `validMovementPosition(isCrusher, ...)` (`AIPathfind.cpp:8680`): a crusher enters a
                // fence cell, not every obstacle.
                if !self.valid_movement(obj.surfaces, nc, is_crusher) {
                    continue;
                }
                neighbor_flags[i] = true;
                if !self.info[nc].has && !self.allocate_info(nc) {
                    self.clean_search();
                    return MAX_COST;
                }
                cell_count += 1;
                let new_cost = self.cost_so_far(nc, p) as u32;
                let on_list = self.info[nc].open || self.info[nc].closed;
                if on_list && u32::from(self.info[nc].so_far) <= new_cost {
                    continue;
                }
                self.info[nc].parent = p as u32;
                self.info[nc].so_far = new_cost as u16;
                let remaining = self.cost_to_goal(nc, goal);
                self.info[nc].total = (i32::from(self.info[nc].so_far) + remaining) as u16;
                if self.info[nc].closed {
                    self.info[nc].closed = false;
                }
                if self.info[nc].open {
                    self.remove_from_open(nc);
                }
                self.put_on_open(nc);
            }
        }
        self.clean_search();
        MAX_COST
    }

    /// `Pathfinder::clientSafeQuickDoesPathExist` (`AIPathfind.cpp:8239-8290`): the destination must be
    /// a legal movement position and its cell's effective zone must equal the start cell's for this
    /// locomotor set. A cell inside a structure is judged by the *terrain* zone under it (the
    /// `doingTerrainZone` remap, `:8266-8279`): the structure's own zone is not the terrain's.
    ///
    /// The C++ derives a layer per endpoint (`destinationLayer = getLayerForDestination(to)`,
    /// `fromLayer = getLayerForDestination(from)`, `:8249-8256`) and reads `getCell(layer, x, y)` on
    /// each. That is not ported, because the port's layer model makes it unobservable or wrong:
    /// bridge layers are a ground-grid deck overlay here, so the deck's cells already are the ground
    /// cells the engine's `getCell(bridgeLayer, ...)` returns (and their zones are connected the way
    /// the layer's `connectLayer` entry points make them), while a *wall* layer has no ground
    /// `connectLayer` in the engine either (`classifyWallCells` sets none; only bridge layers get
    /// one, `AIPathfind.cpp:3860-3882`), so a goal across the ground/wall boundary is "not
    /// connected" on both sides. The search's grid already comes from the from-endpoint's layer
    /// (`compute_move_path_group` -> `on_layer`).
    ///
    /// The port used to answer `z1 == 0 || z2 == 0 || z1 == z2` over per-surface passable-component
    /// labels, which accepted a goal inside a building because the cell had no label — the AN3 s1
    /// GLA worker's adjust then sent it into the dock warehouse (LOG lane 63/74).
    pub fn quick_does_path_exist(&mut self, obj: &PathObject, from: &Coord3D, to: &Coord3D) -> bool {
        // `validMovementPosition(false, destinationLayer, locomotorSet, to)` (`:8256`): an obstacle cell
        // only passes when it holds the pathfinder's ignored obstacle (`:4985-5006`); the engine passes
        // `isCrusher = false` here.
        if !self.valid_movement_position(obj.surfaces, to, false) {
            return false;
        }
        let (Some(parent), Some(goal)) = (self.clipped_cell_at(from), self.clipped_cell_at(to)) else { return false };
        if self.cells[goal].ty == CellType::Cliff {
            return false; // No goals on cliffs.
        }
        // The zone tables are the ones the last `calculateZones` built: no lazy recomputation here.
        let (parent_cell, goal_cell) = (self.cells[parent].ty, self.cells[goal].ty);
        let parent_zone = self.zone_of(parent);
        let goal_zone = self.zone_of(goal);
        let mut zone1 = self.zones.effective_zone(obj.surfaces, false, parent_zone);
        // `if (parentCell->getType() == CELL_OBSTACLE) doingTerrainZone = true;`
        let mut doing_terrain_zone = parent_cell == CellType::Obstacle;
        if parent_cell == CellType::Obstacle && zone1 == crate::zone::NO_ZONE {
            // `#if !(RTS_GENERALS && RETAIL_COMPATIBLE_PATHFINDING)` (`AIPathfind.cpp:8317-8323`):
            // "We are in a building that just got placed, and zones haven't been updated yet.
            // [8/8/2003] It is better to return a false positive than a false negative. jba."
            return true;
        }
        let mut zone2 = self.zones.effective_zone(obj.surfaces, false, goal_zone);
        if goal_cell == CellType::Obstacle {
            doing_terrain_zone = true;
        }
        if doing_terrain_zone {
            zone1 = self.zones.effective_terrain_zone(parent_zone);
            zone1 = self.zones.effective_zone(obj.surfaces, false, zone1);
            zone1 = self.zones.effective_terrain_zone(zone1);
            zone2 = self.zones.effective_terrain_zone(goal_zone);
            zone2 = self.zones.effective_zone(obj.surfaces, false, zone2);
            zone2 = self.zones.effective_terrain_zone(zone2);
        }
        zone1 == zone2
    }

    // ---- hierarchical path (`findHierarchicalPath` / `findClosestHierarchicalPath`) ---------

    /// `Pathfinder::findHierarchicalPath` (Core `AIPathfind.cpp:7542-7546`): the coarse hierarchical
    /// search from `from` to `to`. Marks the block corridor it finds passable (`setPassable`) and
    /// returns the block route; see [`internal_find_hierarchical_path`](Self::internal_find_hierarchical_path).
    pub fn find_hierarchical_path(&mut self, surfaces: u32, from: &Coord3D, to: &Coord3D) -> Option<Vec<(i32, i32)>> {
        self.internal_find_hierarchical_path(surfaces, false, from, to, false)
    }

    /// `Pathfinder::findClosestHierarchicalPath` (`AIPathfind.cpp:7553-7557`): like
    /// [`find_hierarchical_path`](Self::find_hierarchical_path) but succeeds with the closest
    /// reachable cell when the goal is unreachable.
    pub fn find_closest_hierarchical_path(&mut self, surfaces: u32, from: &Coord3D, to: &Coord3D) -> Option<Vec<(i32, i32)>> {
        self.internal_find_hierarchical_path(surfaces, false, from, to, true)
    }

    /// `Pathfinder::internal_findHierarchicalPath` (Core `AIPathfind.cpp:7565-8140`; shipped copy
    /// `CnC_Generals_Zero_Hour/GeneralsMD/.../AIPathfind.cpp:7458`): the coarse A* over the
    /// `ZONE_BLOCK_SIZE`×`ZONE_BLOCK_SIZE` zone blocks.
    ///
    /// The search state is a cell one step *inside* the neighbouring block (`processHierarchicalCell`'s
    /// `adjNewCell`); the block-border cell is only put on the closed list. A side of the current
    /// block is scanned in the engine's `0,+1,-1,+2,-2,...` order (`AIPathfind.cpp:7930-8051`), and a
    /// border cell already on a list stops that side's scan when it still has the parent's block zone
    /// (`:7946-7949`). `buildHierarchicalPath` then walks the parent chain, marks every block it
    /// touches passable (`prependCells`' `setPassable`, `:9090`/`:9153`) and expands the corridor
    /// around the start (`:6892-6911`; that block is compiled because this Zero Hour build is
    /// `RTS_ZEROHOUR`, not the Generals target, so `!(RTS_GENERALS && RETAIL_COMPATIBLE_PATHFINDING)`
    /// applies).
    fn internal_find_hierarchical_path(&mut self, surfaces: u32, crusher: bool, from: &Coord3D, to: &Coord3D, closest_ok: bool) -> Option<Vec<(i32, i32)>> {
        let trace = hpath_trace(self.frame);
        // The corridor and the block passable flags are a ground-grid optimisation. A wall-layer
        // search swaps the wall grid into `cells`; the engine would walk the wall layer's own cells,
        // which the port models as an overlay, so it keeps the no-corridor answer.
        if self.search_on_wall {
            return None;
        }
        if to.x == 0.0 && to.y == 0.0 {
            return None; // "Attempting pathfind to 0,0, generally a bug."
        }
        if !self.map_ready {
            return None;
        }
        let mut adjust_to = *to;
        let mut clip_from = *from;
        self.clip(&mut clip_from, &mut adjust_to);
        self.is_tunneling = false;
        let goal = self.clipped_cell_at(&adjust_to)?;
        let (gx, gy, _) = self.world_to_cell(&adjust_to);
        if !self.allocate_info(goal) {
            return None;
        }
        let parent = self.clipped_cell_at(&clip_from)?;
        if parent != goal && !self.allocate_info(parent) {
            self.clean_search();
            return None;
        }
        // `zone1`/`zone2` use the locomotor's real surfaces and `crusher` false
        // (`AIPathfind.cpp:7620-7629`).
        let zone1 = self.zones.effective_zone(surfaces, false, self.zone_of(parent));
        let zone2 = self.zones.effective_zone(surfaces, false, self.zone_of(goal));
        if trace {
            eprintln!(
                "ZR_HPATH f={} req=hier from=({:.3},{:.3}) to=({:.3},{:.3}) surface={:#x} crusher={} closest={} zone=({},{})",
                self.frame,
                from.x,
                from.y,
                to.x,
                to.y,
                surfaces,
                crusher as u8,
                closest_ok as u8,
                zone1,
                zone2
            );
        }
        if zone1 != zone2 {
            self.clean_search();
            if trace {
                eprintln!("ZR_HPATH f={} hier=fail zone", self.frame);
            }
            return None;
        }
        self.start_pathfind(parent, goal);
        // `m_openList.reset(parentCell)` (retail, `:7652-7662`): the start cell is the whole list.
        self.open.clear();
        self.put_on_open(parent);
        let zb = crate::zone::ZONE_BLOCK_SIZE;
        // `goalBlockZone` uses the locomotor's surfaces (`:7641`); the loop's `parentZone` and
        // `processHierarchicalCell` use `LOCOMOTORSURFACE_GROUND` (`:7458`, `:7745`).
        let goal_block_zone = self.zones.block_zone(gx, gy, surfaces, crusher);
        let goal_block = (gx.div_euclid(zb), gy.div_euclid(zb));
        let mut closest_cell: Option<usize> = None;
        let mut closest_dist_sqr = f32::MAX;
        let mut cell_count: i32 = 0;
        while let Some(p) = self.pop_open() {
            let (px, py) = self.xy(p);
            let parent_zone = self.zones.block_zone(px, py, surface::GROUND, crusher);
            let (block_x, block_y) = (px.div_euclid(zb), py.div_euclid(zb));
            let reached_goal = parent_zone == goal_block_zone && block_x == goal_block.0 && block_y == goal_block.1;
            if trace {
                eprintln!(
                    "ZR_HPATH f={} hierpop cell=({},{}) block=({},{}) zone={} g={} total={} goal={}",
                    self.frame,
                    px,
                    py,
                    block_x,
                    block_y,
                    parent_zone,
                    self.info[p].so_far,
                    self.info[p].total,
                    reached_goal as u8
                );
            }
            // `:7766-7858` links a bridge layer's cells; the port overlays a healthy bridge's deck
            // on the ground grid, so the popped cell is already the deck cell.
            if reached_goal {
                if p != goal {
                    self.info[goal].parent = p as u32;
                }
                self.is_tunneling = false;
                let route = self.build_hierarchical_path(from, goal);
                self.clean_search();
                if trace {
                    eprintln!("ZR_HPATH f={} hier=ok goalcell=({},{}) route={:?}", self.frame, gx, gy, route);
                }
                return Some(route);
            }
            // The closest cell is tracked before the pop is closed (`:7914-7920`).
            let dx = (gx - px).abs() as f32;
            let dy = (gy - py).abs() as f32;
            let d = dx * dx + dy * dy;
            if d < closest_dist_sqr {
                closest_dist_sqr = d;
                closest_cell = Some(p);
            }
            self.info[p].closed = true;
            // Left, right, top, bottom, in that order (`:7928-8052`). The side guards are the
            // engine's; note `blockX<zoneBlockExtent.x-1` and `blockY<zoneBlockExtent.y-1`.
            for side in 0..4 {
                self.scan_hierarchical_side((block_x, block_y), side, p, goal, parent_zone, crusher, &mut cell_count, trace);
            }
        }
        if closest_ok {
            if let Some(cc) = closest_cell {
                self.is_tunneling = false;
                let route = self.build_hierarchical_path(from, cc);
                self.clean_search();
                if trace {
                    let (cx, cy) = self.xy(cc);
                    eprintln!("ZR_HPATH f={} hier=closest errcell=({},{}) route={:?}", self.frame, cx, cy, route);
                }
                return Some(route);
            }
        }
        self.is_tunneling = false;
        self.clean_search();
        if trace {
            eprintln!("ZR_HPATH f={} hier=fail", self.frame);
        }
        None
    }

    /// One block-border scan of [`internal_find_hierarchical_path`](Self::internal_find_hierarchical_path)
    /// (`AIPathfind.cpp:7928-8052`). `examinedZones` is per side (`numExZones = 0` before the right,
    /// top and bottom scans, `:7930-7934`/`:7960`/`:7992`/`:8024`) and holds at most the
    /// `ZONE_BLOCK_SIZE` scan cells the side has.
    #[allow(clippy::too_many_arguments)]
    fn scan_hierarchical_side(
        &mut self,
        block: (i32, i32),
        side: usize,
        parent_cell: usize,
        goal: usize,
        parent_zone: u32,
        crusher: bool,
        cell_count: &mut i32,
        trace: bool,
    ) {
        let zb = crate::zone::ZONE_BLOCK_SIZE;
        let (bx_count, by_count) = self.zones.block_extent();
        let active = match side {
            0 => block.0 > 0,
            1 => block.0 < bx_count - 1,
            2 => block.1 > 0,
            _ => block.1 < by_count - 1,
        };
        if !active {
            return;
        }
        let delta = [(-1, 0), (1, 0), (0, -1), (0, 1)][side];
        let mut examined_zones = [0u32; crate::zone::ZONE_BLOCK_SIZE as usize];
        let mut num_ex_zones = 0usize;
        for i in 1..=crate::zone::ZONE_BLOCK_SIZE {
            // `Int offset = i>>1; if (i&1) offset = -offset;` (`:7935-7936`): 0, +1, -1, +2, -2, ...
            let mut offset = i >> 1;
            if i & 1 != 0 {
                offset = -offset;
            }
            let scan = match side {
                0 => (block.0 * zb, block.1 * zb + zb / 2 + offset),
                1 => (block.0 * zb + zb - 1, block.1 * zb + zb / 2 + offset),
                2 => (block.0 * zb + zb / 2 + offset, block.1 * zb),
                _ => (block.0 * zb + zb / 2 + offset, block.1 * zb + zb - 1),
            };
            let Some(cell) = self.get_cell(scan.0, scan.1) else { continue };
            if (self.info[cell].open || self.info[cell].closed)
                && parent_zone == self.zones.block_zone(scan.0, scan.1, surface::GROUND, crusher)
            {
                break;
            }
            // `if (isHuman && checkCellOutsideExtents(scanCell)) continue;`: the logical extent is
            // the grid here, and `processHierarchicalCell` repeats the `m_extent` test.
            self.process_hierarchical_cell(scan, delta, parent_cell, goal, parent_zone, &mut examined_zones, &mut num_ex_zones, crusher, cell_count, trace);
        }
    }

    /// `Pathfinder::processHierarchicalCell` (Core `AIPathfind.cpp:7444-7535`; shipped copy `:7346`).
    /// The retail branch is the compiled one (`RTS_GENERALS` is not defined): an uninitialised parent
    /// zone is refused, the border cell must already have path info and must not be on a list, the
    /// parent's block zone must equal the border cell's raw zone, and the step across the border must
    /// keep the *global* zone (`getEffectiveZone(LOCOMOTORSURFACE_GROUND, crusher, ...)`).
    #[allow(clippy::too_many_arguments)]
    fn process_hierarchical_cell(
        &mut self,
        scan: (i32, i32),
        delta: (i32, i32),
        parent_cell: usize,
        goal: usize,
        parent_zone: u32,
        examined_zones: &mut [u32; crate::zone::ZONE_BLOCK_SIZE as usize],
        num_ex_zones: &mut usize,
        crusher: bool,
        cell_count: &mut i32,
        trace: bool,
    ) {
        if scan.0 < 0 || scan.0 >= self.w || scan.1 < 0 || scan.1 >= self.h {
            return;
        }
        if parent_zone == crate::zone::NO_ZONE {
            return; // `#if !(RTS_GENERALS && RETAIL_COMPATIBLE_PATHFINDING)` (`:7453-7457`)
        }
        if parent_zone != self.zones.block_zone(scan.0, scan.1, surface::GROUND, crusher) {
            return;
        }
        let Some(new_cell) = self.get_cell(scan.0, scan.1) else { return };
        if !self.cell_has_info(new_cell) {
            return;
        }
        if self.info[new_cell].open || self.info[new_cell].closed {
            return; // already looked at this one
        }
        if parent_zone != self.zone_of(new_cell) {
            return;
        }
        let adj = (scan.0 + delta.0, scan.1 + delta.1);
        if adj.0 < 0 || adj.0 >= self.w || adj.1 < 0 || adj.1 >= self.h {
            return;
        }
        let Some(adj_cell) = self.get_cell(adj.0, adj.1) else { return };
        if self.info[adj_cell].has && (self.info[adj_cell].open || self.info[adj_cell].closed) {
            return;
        }
        let parent_global_zone = self.zones.effective_zone(surface::GROUND, crusher, parent_zone);
        let new_zone = self.zones.block_zone(adj.0, adj.1, surface::GROUND, crusher);
        let new_global_zone = self.zones.effective_zone(surface::GROUND, crusher, new_zone);
        if new_global_zone != parent_global_zone {
            return; // can't step over
        }
        if examined_zones[..*num_ex_zones].contains(&new_zone) {
            return;
        }
        // `newCell->allocateInfo(scanCell); if (!closed && !open) putOnClosedList` (`:7508-7512`):
        // the border cell is only closed, never queued. A unit/obstacle cell already has a permanent
        // `PathfindCellInfo` in the engine, but that record also carries the search flags, so the
        // port allocates its per-search `Info` too — otherwise the closed flag would outlive the
        // search (`clean_search` only resets touched cells) and the next search's side scan would
        // stop on it.
        if !self.allocate_info(new_cell) {
            return;
        }
        if !self.info[new_cell].closed && !self.info[new_cell].open {
            self.info[new_cell].closed = true;
        }
        if self.allocate_info(adj_cell) {
            *cell_count += 1;
            let (pcx, pcy) = self.xy(parent_cell);
            let (gcx, gcy) = self.xy(goal);
            let mut cur_cost = cost_to_hier_goal(adj.0 - pcx, adj.1 - pcy) as u32;
            let rem_cost = cost_to_hier_goal(adj.0 - gcx, adj.1 - gcy) as u32;
            if self.cells[adj_cell].pinched || self.cells[new_cell].pinched {
                cur_cost += 2 * COST_ORTHOGONAL as u32;
            } else {
                examined_zones[*num_ex_zones] = new_zone;
                *num_ex_zones += 1;
            }
            self.info[adj_cell].so_far = (u32::from(self.info[parent_cell].so_far) + cur_cost) as u16;
            self.info[adj_cell].total = (i32::from(self.info[adj_cell].so_far) + rem_cost as i32) as u16;
            self.info[adj_cell].parent = parent_cell as u32;
            self.put_on_open(adj_cell);
            if trace {
                eprintln!(
                    "ZR_HPATH f={} hierpush scan=({},{}) cell=({},{}) zone={} g={} total={}",
                    self.frame,
                    scan.0,
                    scan.1,
                    adj.0,
                    adj.1,
                    new_zone,
                    self.info[adj_cell].so_far,
                    self.info[adj_cell].total
                );
            }
        }
    }

    /// `Pathfinder::buildHierarchicalPath` (Core `AIPathfind.cpp:6884-6940`; shipped copy
    /// `:6836-6875`). `prependCells` walks the parent chain from the goal cell and marks every cell's
    /// block passable (`:9090`), then the start expansion marks every cell within
    /// `ZONE_BLOCK_SIZE*PATHFIND_CELL_SIZE_F` of the path's first node (`:6892-6911`). Returns the
    /// block chain from the start block to the goal block (consecutive duplicates removed).
    fn build_hierarchical_path(&mut self, from: &Coord3D, goal_cell: usize) -> Vec<(i32, i32)> {
        let zb = crate::zone::ZONE_BLOCK_SIZE;
        let mut route: Vec<(i32, i32)> = Vec::new();
        let mut cell = goal_cell;
        let mut guard = 0usize;
        loop {
            let (x, y) = self.xy(cell);
            self.zones.set_passable(x, y, true);
            let b = (x.div_euclid(zb), y.div_euclid(zb));
            if route.last() != Some(&b) {
                route.push(b);
            }
            let parent = self.info[cell].parent;
            if parent == NONE {
                break;
            }
            cell = parent as usize;
            guard += 1;
            if guard > self.w as usize * self.h as usize {
                break; // a parent cycle would hang the engine; never let one out of the search
            }
        }
        route.reverse();
        // "Expand the hierarchical path around the starting point. This allows the unit to get
        // around friendly units that may be near it." (`:6893-6910`)
        let reach = zb as f32 * PATHFIND_CELL_SIZE_F;
        let (min_x, min_y, _) = self.world_to_cell(&Coord3D::new(from.x - reach, from.y - reach, 0.0));
        let (max_x, max_y, _) = self.world_to_cell(&Coord3D::new(from.x + reach, from.y + reach, 0.0));
        for x in min_x..=max_x {
            for y in min_y..=max_y {
                self.zones.set_passable(x, y, true);
            }
        }
        route
    }

    // ---- search bookkeeping -----------------------------------------------------------------

    /// `PathfindCell::hasInfo` (`PathfindCell.h:151`): the cell holds a `PathfindCellInfo`. Obstacle
    /// and unit cells keep one permanently — `PathfindCell::releaseInfo` returns early for
    /// `m_type == CELL_OBSTACLE || m_flags != NO_UNITS || m_aircraftGoal` (`AIPathfind.cpp:1395`), and
    /// `setTypeAsObstacle`/`setPosUnit`/`setGoalUnit` allocate one — so `hasInfo` is not the same as
    /// "this search allocated the cell". The hierarchical search's `newCell->hasInfo()` gate reads
    /// this. The obstacle clause is the retail allocation mode the port follows: `setObstacle` allocates
    /// the info unless `s_useFixedPathfinding` (`AIPathfind.cpp:1594-1606`), and that flag is false at start
    /// (`AIPathfind.cpp:1081`); it turns on only after the retail-compatible pool fails (lane 191 R27 F9).
    fn cell_has_info(&self, ci: usize) -> bool {
        self.info[ci].has
            || self.cells[ci].ty == CellType::Obstacle
            || self.cells[ci].flags != CellFlags::NoUnits
            || self.cells[ci].aircraft_goal
    }

    /// `PathfindCell::allocateInfo`.
    fn allocate_info(&mut self, ci: usize) -> bool {
        if self.info[ci].has {
            return true;
        }
        if self.touched.len() >= CELL_INFOS_TO_ALLOCATE {
            return false;
        }
        self.info[ci] = Info { has: true, ..NO_INFO };
        self.touched.push(ci as u32);
        self.cumulative_cells_allocated = self.cumulative_cells_allocated.saturating_add(1);
        true
    }

    /// Release every touched cell (`cleanOpenAndClosedLists` + `releaseInfo`).
    fn clean_search(&mut self) {
        self.open.clear();
        for &c in &self.touched {
            self.info[c as usize] = NO_INFO;
        }
        self.touched.clear();
    }

    /// `PathfindCell::costToGoal`.
    fn cost_to_goal(&self, ci: usize, goal: usize) -> i32 {
        let (x, y) = self.xy(ci);
        let (gx, gy) = self.xy(goal);
        let dx = (x - gx).abs();
        let dy = (y - gy).abs();
        if dx > dy {
            COST_ORTHOGONAL * dx + (COST_ORTHOGONAL * dy) / 2
        } else {
            COST_ORTHOGONAL * dy + (COST_ORTHOGONAL * dx) / 2
        }
    }

    /// `PathfindCell::costSoFar( parent )`.
    fn cost_so_far(&self, ci: usize, parent: usize) -> i32 {
        let (x, y) = self.xy(ci);
        let (px, py) = self.xy(parent);
        let prev_dir = (px - x, py - y);
        let mut cost = i32::from(self.info[parent].so_far);
        // diagonal moves cost a bit more than orthogonal ones
        cost += if prev_dir.0 == 0 || prev_dir.1 == 0 { COST_ORTHOGONAL } else { COST_DIAGONAL };
        if self.cells[ci].pinched {
            cost += COST_DIAGONAL;
        }
        // Increase cost of turns.
        let mut num_turns = 0;
        let grand = self.info[parent].parent;
        if grand != NONE {
            let (gx, gy) = self.xy(grand as usize);
            let dir = (gx - px, gy - py);
            if dir != prev_dir {
                let dot = dir.0 * prev_dir.0 + dir.1 * prev_dir.1;
                num_turns = if dot > 0 {
                    4 // 45 degree turn
                } else if dot == 0 {
                    8 // 90 degree turn
                } else {
                    16 // 135 degree turn
                };
            }
        }
        cost + num_turns
    }

    /// `putOnSortedOpenList`: ascending total cost, FIFO among equals.
    fn put_on_open(&mut self, ci: usize) {
        let total = self.info[ci].total;
        let key = (total, self.seq);
        self.seq += 1;
        self.info[ci].open_key = key;
        self.info[ci].open = true;
        self.info[ci].closed = false;
        self.open.insert((key.0, key.1, ci as u32));
    }
    fn remove_from_open(&mut self, ci: usize) {
        let k = self.info[ci].open_key;
        self.open.remove(&(k.0, k.1, ci as u32));
        self.info[ci].open = false;
    }
    fn pop_open(&mut self) -> Option<usize> {
        let first = *self.open.iter().next()?;
        self.open.remove(&first);
        let ci = first.2 as usize;
        self.info[ci].open = false;
        Some(ci)
    }

    /// `PathfindCell::startPathfind`.
    fn start_pathfind(&mut self, ci: usize, goal: usize) {
        let total = self.cost_to_goal(ci, goal);
        self.info[ci] = Info { has: true, total: total as u16, ..NO_INFO };
    }

    /// `ClipLine2D` against the grid; `None` when the line is entirely outside.
    fn clip_line(&self, p1: (i32, i32), p2: (i32, i32)) -> Option<((i32, i32), (i32, i32))> {
        clip_line_2d(p1, p2, (0, 0, self.w - 1, self.h - 1))
    }

    /// `Pathfinder::clip`.
    fn clip(&self, from: &mut Coord3D, to: &mut Coord3D) {
        let from_cell = (real_to_int_floor(from.x / PATHFIND_CELL_SIZE_F), real_to_int_floor(from.y / PATHFIND_CELL_SIZE_F));
        let to_cell = (real_to_int_floor(to.x / PATHFIND_CELL_SIZE_F), real_to_int_floor(to.y / PATHFIND_CELL_SIZE_F));
        if let Some((cf, ct)) = self.clip_line(from_cell, to_cell) {
            if from_cell != cf {
                from.x = cf.0 as f32 * PATHFIND_CELL_SIZE_F + 0.05;
                from.y = cf.1 as f32 * PATHFIND_CELL_SIZE_F + 0.05;
            }
            if to_cell != ct {
                to.x = ct.0 as f32 * PATHFIND_CELL_SIZE_F + 0.05;
                to.y = ct.1 as f32 * PATHFIND_CELL_SIZE_F + 0.05;
            }
        }
    }

    /// The cells `iterateCellsAlongLine` hands to its callback, in order (Bresenham, with the
    /// extra step cell on diagonal moves). Stops early where the original returns on a null cell.
    fn line_cells(&self, start: (i32, i32), end: (i32, i32)) -> Vec<(i32, i32)> {
        let delta_x = (end.0 - start.0).abs();
        let delta_y = (end.1 - start.1).abs();
        let (mut x, mut y) = start;
        let (mut xinc1, mut xinc2) = if end.0 >= start.0 { (1, 1) } else { (-1, -1) };
        let (mut yinc1, mut yinc2) = if end.1 >= start.1 { (1, 1) } else { (-1, -1) };
        let (den, mut num, numadd, numpixels);
        if delta_x >= delta_y {
            xinc1 = 0;
            yinc2 = 0;
            den = delta_x;
            num = delta_x / 2;
            numadd = delta_y;
            numpixels = delta_x;
        } else {
            xinc2 = 0;
            yinc1 = 0;
            den = delta_y;
            num = delta_y / 2;
            numadd = delta_x;
            numpixels = delta_y;
        }
        let mut out = Vec::with_capacity(numpixels as usize + 2);
        for _ in 0..=numpixels {
            if self.get_cell(x, y).is_none() {
                return out;
            }
            out.push((x, y));
            num += numadd;
            if num >= den {
                num -= den;
                x += xinc1;
                y += yinc1;
                if self.get_cell(x, y).is_none() {
                    return out;
                }
                out.push((x, y));
            }
            x += xinc2;
            y += yinc2;
        }
        out
    }

    /// `Pathfinder::isLinePassable` (`AIPathfind.cpp:9727-9750`): a clear line for `obj`, bailing on
    /// an off-map cell, a fixed unit (ally or enemy, `linePassableCallback`), a pinched cell or a
    /// surface the locomotor cannot use. `blocked` is `TCheckMovementInfo::considerTransient`:
    /// `Path::optimize` passes its own `blocked` (`AIPathfind.cpp:507`, false for a normal path and
    /// `m_isBlockedAndStuck` for a stuck one), `Path::computePointOnPath` passes false (`:906`).
    pub fn is_line_passable(&self, obj: &PathObject, start: &Coord3D, end: &Coord3D, blocked: bool, allow_pinched: bool) -> bool {
        let (radius, center) = obj.radius_and_center();
        let (sx, sy, _) = self.world_to_cell(start);
        let (ex, ey, _) = self.world_to_cell(end);
        for (x, y) in self.line_cells((sx, sy), (ex, ey)) {
            let Some(to) = self.get_cell(x, y) else { return true };
            if !self.check_for_movement(x, y, radius, center) {
                return false;
            }
            let info = self.check_for_movement_cell(obj, x, y, radius, center, blocked);
            if info.ally_fixed_count > 0 || info.enemy_fixed {
                return false;
            }
            if !allow_pinched && self.cells[to].pinched {
                return false;
            }
            if !self.valid_movement(obj.surfaces, to, obj.crusher) {
                return false;
            }
        }
        true
    }

    // ---- moveAllies / getMoveAwayFromPath ------------------------------------------------------

    /// `Pathfinder::checkDestination` (`AIPathfind.cpp:5013-5094`), the unit/obstacle/impassable test
    /// `getMoveAwayFromPath`, `patchPath` and `tightenPath`'s callback run. The unit half is shared
    /// with [`check_destination_unit`](Self::check_destination_unit): a cell with no goal unit never
    /// blocks just because a unit stands on it; an ally's goal does; a `UNIT_PRESENT_FIXED` cell
    /// blocks unless the unit can crush it.
    fn engine_check_destination(&self, obj: &PathObject, cx: i32, cy: i32, radius: i32, center: bool) -> bool {
        self.check_destination_unit(obj, cx, cy, radius, center, &|u| self.unit_objs.get(&u).map(|p| p.player))
    }

    /// The scan half of `Pathfinder::moveAllies` (`AIPathfind.cpp:10190-10320`): the units on the mover's new path's
    /// cells, in the engine's visit order, one entry per visit ([`ally_scan`](Self::ally_scan)); `GameLogic::move_allies`
    /// applies `aiMoveAwayFromUnit` to each.
    /// The units standing on the cells along the line from `from` to `dest`, in line order, one entry per cell visit
    /// (`iterateCellsAlongLine` with `moveAlliesDestinationCallback`, `AIPathfind.cpp:6985`, `:7290`: the callback
    /// runs on every cell, with no de-duplication). The caller applies the ally, not-moving and ignored-obstacle tests.
    pub(crate) fn units_along_line(&self, from: &Coord3D, dest: &Coord3D) -> Vec<u32> {
        let (sx, sy, _) = self.world_to_cell(from);
        let (ex, ey, _) = self.world_to_cell(dest);
        let mut out: Vec<u32> = Vec::new();
        for (x, y) in self.line_cells((sx, sy), (ex, ey)) {
            let Some(ci) = self.get_cell(x, y) else { continue };
            let unit_id = self.cells[ci].pos_unit;
            if unit_id != 0 {
                out.push(unit_id);
            }
        }
        out
    }

    pub fn ally_scan(&self, mover: &PathObject, path: &Path) -> Vec<ObjectId> {
        if mover.id == ObjectId::INVALID {
            return Vec::new();
        }
        if !mover.dozer && !mover.harvester && !path.blocked_by_ally {
            return Vec::new(); // Only move units if it is required (AIPathfind.cpp:10197-10200).
        }
        let (radius, center) = mover.radius_and_center();
        let above = radius + i32::from(center);
        // `ObjectID ignoreId = obj->getAIUpdateInterface()->getIgnoredObstacleID()` (`:10342-10345`):
        // the *unit's* ignored obstacle, not the pathfinder's search-scoped one.
        let ignore_id = mover.ignore_obstacle.0;
        let mut visits: Vec<ObjectId> = Vec::new();
        // `for (node = path->getLastNode(); node && node != path->getFirstNode(); node = node->getPrevious())`
        // (`:10347`): from the last node down to, excluding, the first.
        for node in path.nodes.iter().skip(1).rev() {
            let (cx, cy, _) = self.world_to_cell(&node.pos);
            for i in (cx - radius)..(cx + above) {
                for j in (cy - radius)..(cy + above) {
                    let Some(ci) = self.get_cell(i, j) else { continue };
                    let unit_id = self.cells[ci].pos_unit;
                    if unit_id == 0 || unit_id == mover.id.0 || unit_id == ignore_id {
                        continue;
                    }
                    // Every visit is kept, in order, and `GameLogic::move_allies` runs the tests on the live
                    // objects at that visit (the engine calls `aiMoveAwayFromUnit` inside this loop).
                    visits.push(ObjectId(unit_id));
                }
            }
        }
        visits
    }

    /// `Pathfinder::getMoveAwayFromPath` (`AIPathfind.cpp:10291`): the first reachable cell whose
    /// clearance box does not overlap any segment of the path to avoid.
    #[allow(clippy::too_many_arguments)]
    fn find_move_away_path(&mut self, terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, avoid: &[Coord3D], avoid2: &[Coord3D], other_radius: i32, other_center: bool) -> Option<Path> {
        if !self.map_ready {
            return None;
        }
        // `m_zoneManager.setAllPassable()` (`AIPathfind.cpp:10433`): the move-away search is not
        // restricted to the corridor a previous `findPath` marked.
        self.zones.set_all_passable();
        let (radius, center) = obj.radius_and_center();
        // The engine adds PATHFIND_CELL_SIZE_F/2 to `startPos.x` twice (AIPathfind.cpp:10321-10324).
        let mut start_pos = *from;
        if !center {
            start_pos.x += PATHFIND_CELL_SIZE_F * 0.5;
            start_pos.x += PATHFIND_CELL_SIZE_F * 0.5;
        }
        let (sx, sy, _) = self.world_to_cell(&start_pos);
        let parent = self.clipped_cell_at(from)?;
        self.is_tunneling = false;
        if !self.valid_movement(obj.surfaces, parent, obj.crusher) {
            self.is_tunneling = true; // Can't move from here: relax the constraints.
        }
        let minfo = self.check_for_movement_cell(obj, sx, sy, radius, center, false);
        if minfo.enemy_fixed {
            self.is_tunneling = true;
        }
        if !self.allocate_info(parent) {
            return None;
        }
        // `startPathfind(nullptr)`: total 0, no goal heuristic.
        self.info[parent].total = 0;
        self.info[parent].so_far = 0;
        self.info[parent].parent = NONE;
        self.open.clear();
        self.put_on_open(parent);
        let ctx = Self::ctx_for(obj);
        let box_half = radius as f32 * PATHFIND_CELL_SIZE_F - PATHFIND_CELL_SIZE_F / 4.0
            + if center { PATHFIND_CELL_SIZE_F / 2.0 } else { 0.0 }
            + other_radius as f32 * PATHFIND_CELL_SIZE_F
            + if other_center { PATHFIND_CELL_SIZE_F / 2.0 } else { 0.0 };
        while let Some(p) = self.pop_open() {
            let (x, y) = self.xy(p);
            let cell_center = self.adjust_coord_to_cell(terrain, x, y, center);
            let lo = (cell_center.x - box_half, cell_center.y - box_half);
            let hi = (cell_center.x + box_half, cell_center.y + box_half);
            let mut overlap = false;
            for w in avoid.windows(2) {
                if line_in_region(w[0], w[1], lo, hi) {
                    overlap = true;
                    break;
                }
            }
            // `pathToAvoid2` (`AIPathfind.cpp:10544-10556`): the path of the unit this one stepped
            // aside from last time is avoided too.
            if !overlap {
                for w in avoid2.windows(2) {
                    if line_in_region(w[0], w[1], lo, hi) {
                        overlap = true;
                        break;
                    }
                }
            }
            if !overlap && sx == x && sy == y {
                overlap = true; // We didn't move; always move at least one cell.
            }
            if !overlap && self.engine_check_destination(obj, x, y, radius, center) {
                self.is_tunneling = false;
                let path = self.build_actual_path(terrain, obj, from, p, center, false);
                self.clean_search();
                return Some(path);
            }
            self.info[p].closed = true;
            self.examine_neighboring_cells(terrain, p, None, None, &ctx, (sx, sy));
        }
        self.is_tunneling = false;
        self.clean_search();
        None
    }

    // ---- attack path (`findAttackPath`) --------------------------------------------------------

    /// Is the goal position within the weapon's firing envelope (2D, between the two circles)?
    /// `Weapon::isGoalPosWithinAttackRange` (`Weapon.cpp:2357-2400`; `RATIONALIZE_ATTACK_RANGE` and
    /// `ATTACK_RANGE_IS_2D` are defined at `Weapon.cpp:80-81`). `attack_range` is `Weapon::getAttackRange`, which
    /// already has `WeaponTemplate::getAttackRange`'s quarter-cell undersize (`:2452-2456`, `:464-478`); the goal
    /// test undersizes it by a quarter cell again (`:2364`). `min_range` is `getMinimumAttackRange`; the goal test
    /// oversizes it by a quarter cell (`:2394`) and rejects a boundary distance under it (`:2396-2400`; the `-0.5`
    /// belongs to the `#else` branch). Lane 188: the envelope was used unchanged, so a goal 171.3 from the
    /// victim (C++ limit 170) was accepted (AN5 s1 crawler 263, f3138).
    fn goal_in_range(&self, goal: &Coord3D, src_radius: f32, victim_pos: &Coord3D, victim_radius: f32, attack_range: f32, min_range: f32) -> bool {
        let d2 = boundary_dist_sqr(goal, src_radius, victim_pos, victim_radius);
        let range = attack_range - PATHFIND_CELL_SIZE_F * 0.25;
        let min = min_range + PATHFIND_CELL_SIZE_F * 0.25;
        d2 >= min * min && d2 <= range * range
    }

    /// `Pathfinder::findAttackPath` (`AIPathfind.cpp:10709`): a path to a cell the unit can fire on
    /// the victim from. `view` is the attacker's line-of-sight inputs (`GameLogic::attack_view`; `None`
    /// means the view is never blocked). `check_los` is the search's `checkLOS` (`:10844-10850`): false for a
    /// victim significantly above the terrain. The quick check tests the view regardless (`:10750`).
    #[allow(clippy::too_many_arguments)]
    pub fn find_attack_path(
        &mut self,
        terrain: &TerrainLogic,
        obj: &PathObject,
        from: &Coord3D,
        victim_pos: &Coord3D,
        view: Option<&AttackView>,
        attack_range: f32,
        min_range: f32,
        src_radius: f32,
        victim_radius: f32,
        check_los: bool,
    ) -> Option<Path> {
        if !self.map_ready {
            return None;
        }
        let (radius, center) = obj.radius_and_center();

        // Quick check: moving a couple of cells toward the victim may already be enough.
        {
            let cur_pos = *from;
            let mut dx = victim_pos.x - cur_pos.x;
            let mut dy = victim_pos.y - cur_pos.y;
            let len = crate::math::sqrt(dx * dx + dy * dy);
            if len > 0.0 {
                dx = dx / len * PATHFIND_CELL_SIZE_F;
                dy = dy / len * PATHFIND_CELL_SIZE_F;
            } else {
                dx = 0.0;
                dy = 0.0;
            }
            for i in 1..10 {
                let test = Coord3D::new(cur_pos.x + dx * i as f32 * 0.5, cur_pos.y + dy * i as f32 * 0.5, cur_pos.z);
                let (Some(ci), Some((cx, cy, _))) = (self.cell_at(&test), Some(self.world_to_cell(&test))) else { break };
                if !self.valid_movement(obj.surfaces, ci, obj.crusher) {
                    break;
                }
                if !self.engine_check_destination(obj, cx, cy, radius, center) {
                    break;
                }
                if !self.goal_in_range(&test, src_radius, victim_pos, victim_radius, attack_range, min_range) {
                    continue;
                }
                if view.is_some_and(|v| self.attack_view_blocked(terrain, v, &test, victim_pos)) {
                    continue;
                }
                return Some(Path {
                    nodes: vec![
                        PathNode { pos: cur_pos, can_optimize: true, next_optimized: Some(1) },
                        PathNode { pos: test, can_optimize: true, next_optimized: None },
                    ],
                    blocked_by_ally: false,
                });
            }
        }

        // Hierarchical corridor (`findClosestHierarchicalPath`), then the fine A*.
        self.zones.clear_passable_flags();
        if self.internal_find_hierarchical_path(obj.surfaces, obj.crusher, from, victim_pos, true).is_none() {
            self.zones.set_all_passable();
        }

        let attack_distance = attack_range + src_radius + victim_radius + 3.0 * PATHFIND_CELL_SIZE_F;

        let mut obj_pos = *from;
        if center {
            obj_pos.x += PATHFIND_CELL_SIZE_F / 2.0;
            obj_pos.y += PATHFIND_CELL_SIZE_F / 2.0;
        }
        let parent = self.clipped_cell_at(&obj_pos)?;
        let (vx, vy, _) = self.world_to_cell(victim_pos);
        let goal = self.get_cell(vx, vy)?;
        let (fx, fy, _) = self.world_to_cell(from);
        if !self.allocate_info(parent) {
            return None;
        }
        let start_cell = parent;
        if !self.allocate_info(goal) {
            self.clean_search();
            return None;
        }
        self.info[parent].total = 0;
        self.info[parent].so_far = 0;
        self.info[parent].parent = NONE;
        self.open.clear();
        self.put_on_open(parent);
        let ctx = Self::ctx_for(obj);
        // `Int cellCount` and `ATTACK_CELL_LIMIT` (AIPathfind.cpp:10765, :11014-11018): the search stops
        // expanding once 2500 new cells are allocated; the open list then drains and the search fails.
        const ATTACK_CELL_LIMIT: i32 = 2500;
        let mut cell_count: i32 = 0;
        let l191 = l191_atkpop(self.frame, obj.id.0);
        while let Some(p) = self.pop_open() {
            let (x, y) = self.xy(p);
            let cell_center = self.adjust_coord_to_cell(terrain, x, y, center);
            let in_range = self.goal_in_range(&cell_center, src_radius, victim_pos, victim_radius, attack_range, min_range);
            if l191 {
                eprintln!(
                    "ZR_L191_ATKPOP f={} id={} pop={},{} total={} so={} center={:.3},{:.3} in={} count={}",
                    self.frame, obj.id.0, x, y, self.info[p].total, self.info[p].so_far, cell_center.x, cell_center.y,
                    in_range as i32, cell_count
                );
            }
            if in_range && self.engine_check_destination(obj, x, y, radius, center)
            {
                let mut view_blocked = false;
                if check_los {
                    view_blocked = view.is_some_and(|v| self.attack_view_blocked(terrain, v, &cell_center, victim_pos));
                }
                if start_cell == p {
                    view_blocked = true; // never accept the starting cell
                } else {
                    let dx = cell_center.x - obj_pos.x;
                    let dy = cell_center.y - obj_pos.y;
                    if dx * dx + dy * dy < (PATHFIND_CELL_SIZE_F * 0.5) * (PATHFIND_CELL_SIZE_F * 0.5) {
                        view_blocked = true;
                    }
                }
                if !view_blocked {
                    // A vehicle strips back from the goal first (`AIPathfind.cpp:10887-10930`).
                    let end = if self.vehicle_units.contains(&obj.id.0) { self.attack_strip_back(obj, p, radius, center) } else { p };
                    self.is_tunneling = false;
                    let path = self.build_actual_path(terrain, obj, from, end, center, false);
                    if l191 {
                        let (rx, ry) = self.xy(end);
                        let last = path.nodes.last().map_or(Coord3D::new(0.0, 0.0, 0.0), |n| n.pos);
                        eprintln!(
                            "ZR_L191_ATKPOP f={} id={} result={},{} nodes={} last={:.3},{:.3} count={}",
                            self.frame, obj.id.0, rx, ry, path.nodes.len(), last.x, last.y, cell_count
                        );
                    }
                    self.clean_search();
                    return Some(path);
                }
            }
            self.info[p].closed = true;
            if cell_count < ATTACK_CELL_LIMIT {
                // `checkChangeLayers` (ground only here), then the neighbour expansion (AIPathfind.cpp:11014-11018).
                cell_count += self.examine_neighboring_cells(terrain, p, Some(goal), Some(attack_distance), &ctx, (fx, fy));
            }
        }
        self.is_tunneling = false;
        self.clean_search();
        None
    }

    /// Lane 188: the vehicle strip-back at the end of `findAttackPath` (`AIPathfind.cpp:10919-10972`, compiled
    /// because this build is not `RTS_GENERALS`). Walking up to 12 cells from the accepted cell `p` back toward the
    /// start, each cell is tested alone (`radius` 0, centred) until a blocked cell is found, and then with the unit's
    /// full footprint. A blocked cell is off the map, holds a fixed enemy, or holds a fixed or goal ally, unless the
    /// unit on it is idle (`unitIdle`, :10948). The path ends at the parent of the last blocked cell found (or at that
    /// cell when it has no parent).
    fn attack_strip_back(&self, obj: &PathObject, p: usize, radius: i32, center: bool) -> usize {
        let mut last_blocked: Option<usize> = None;
        let mut cur = Some(p);
        let mut use_large_radius = false;
        let mut cell_limit: i32 = 12;
        while let Some(c) = cur {
            cell_limit -= 1;
            if cell_limit < 0 {
                break;
            }
            let (cx, cy) = self.xy(c);
            let (r, ctr) = if use_large_radius { (radius, center) } else { (0, true) };
            let info = self.check_for_movement_cell(obj, cx, cy, r, ctr, false);
            let pos_unit = self.cells[c].pos_unit;
            let unit_idle = pos_unit != 0 && self.idle_units.contains(&pos_unit);
            let blocked_by_allies = !unit_idle && (info.ally_fixed_count > 0 || info.ally_goal);
            if info.enemy_fixed || blocked_by_allies {
                last_blocked = Some(c);
                use_large_radius = true;
            } else {
                use_large_radius = false;
            }
            cur = (self.info[c].parent != NONE).then_some(self.info[c].parent as usize);
        }
        match last_blocked {
            Some(b) => if self.info[b].parent != NONE { self.info[b].parent as usize } else { b },
            None => p,
        }
    }

    // ---- patchPath -----------------------------------------------------------------------------

    /// `Pathfinder::patchPath` (`AIPathfind.cpp:10480`): when a unit becomes blocked, find a short
    /// path from its current position back onto `original`, from the nearest reachable node.
    /// `AIUpdateInterface::computePath` calls this when `isBlockedAndStuck` (see
    /// [`compute_ai_path`](Self::compute_ai_path)); the blocked state is set by
    /// [`GameLogic::process_collision_moving`](crate::GameLogic) (the contact's `onCollide`).
    pub fn patch_path(&mut self, terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, original: &Path, blocked: bool) -> Option<Path> {
        const CELL_LIMIT: i32 = 2000;
        if original.nodes.is_empty() {
            return None;
        }
        // `m_zoneManager.setAllPassable()` (`AIPathfind.cpp:10619`): same as `getMoveAwayFromPath`,
        // the stitch search is not confined to the corridor of the path it patches.
        self.zones.set_all_passable();
        let (radius, center) = obj.radius_and_center();
        let parent = self.clipped_cell_at(from)?;
        self.is_tunneling = false;
        // `patchPath` (`AIPathfind.cpp:10511-10534`): an invalid start (in an obstacle) relaxes the
        // search, and a blocked start cell (fixed unit) does too.
        if !self.valid_movement(obj.surfaces, parent, obj.crusher) {
            self.is_tunneling = true;
        }
        // "determine start cell" (`AIPathfind.cpp:10507-10515`): the shipped code adds
        // `PATHFIND_CELL_SIZE_F*0.5f` to `startPos.x` **twice** (the second add was meant for `y`; the
        // `findMoveAwayFromPath` twin does the same, `:10321-10324`). `startCellNdx` then feeds the path
        // walk's transient window (`:10581-10582`), the neighbour expansion's start-cell ally test
        // (`:6326-6329`, `:6353-6355`) and the start cell's movement info.
        let mut start_pos = *from;
        if !center {
            start_pos.x += PATHFIND_CELL_SIZE_F * 0.5;
            start_pos.x += PATHFIND_CELL_SIZE_F * 0.5;
        }
        let (sx, sy, _) = self.world_to_cell(&start_pos);
        let start_info = self.check_for_movement_cell(obj, sx, sy, radius, center, blocked);
        if start_info.enemy_fixed {
            self.is_tunneling = true;
        }
        if !self.allocate_info(parent) {
            return None;
        }
        self.info[parent].total = 0;
        self.info[parent].so_far = 0;
        self.info[parent].parent = NONE;
        self.open.clear();
        self.put_on_open(parent);

        // Walk the original path from the end: the first occupied node ends the walk; the nearest
        // free node becomes the stitch point. The tail keeps every node from that point onward.
        // A node more than two cells from the start is not tested for transient units
        // (`AIPathfind.cpp:10581-10582`).
        let mut tail_start: Option<usize> = None;
        let mut goal_delta_sqr = f32::MAX;
        for i in (1..original.nodes.len()).rev() {
            let cell = self.world_to_cell(&original.nodes[i].pos);
            let dx = (cell.0 - sx).abs();
            let dy = (cell.1 - sy).abs();
            let consider_transient = blocked && dx <= 2 && dy <= 2;
            let info = self.check_for_movement_cell(obj, cell.0, cell.1, radius, center, consider_transient);
            if !self.check_for_movement(cell.0, cell.1, radius, center) || info.ally_fixed_count > 0 || info.enemy_fixed {
                break;
            }
            let d = original.nodes[i].pos.dist_2d(from);
            if d * d < goal_delta_sqr {
                goal_delta_sqr = d * d;
                tail_start = Some(i);
            }
        }
        let Some(tail_start) = tail_start else { self.clean_search(); return None };
        let Some(goal) = self.clipped_cell_at(&original.nodes[tail_start].pos) else { self.clean_search(); return None };
        if !self.allocate_info(goal) {
            self.clean_search();
            return None;
        }
        let mut match_cells: BTreeSet<(i32, i32)> = BTreeSet::new();
        for node in original.nodes.iter().skip(tail_start) {
            let c = self.world_to_cell(&node.pos);
            match_cells.insert((c.0, c.1));
        }
        let ctx = Self::ctx_for(obj);
        let mut cell_count = 0;
        while let Some(p) = self.pop_open() {
            let (x, y) = self.xy(p);
            if match_cells.contains(&(x, y)) {
                // Stitch: unit position, patched cells (start..match), then the original tail.
                let mut rev: Vec<PathNode> = Vec::new();
                let mut blocked_here = original.blocked_by_ally;
                let mut cell = p;
                while self.info[cell].parent != NONE {
                    let (cx, cy) = self.xy(cell);
                    blocked_here |= self.info[cell].blocked_by_ally;
                    rev.push(PathNode { pos: self.adjust_coord_to_cell(terrain, cx, cy, center), can_optimize: true, next_optimized: None });
                    cell = self.info[cell].parent as usize;
                }
                let mut nodes: Vec<PathNode> = Vec::with_capacity(rev.len() + 1);
                nodes.push(PathNode { pos: *from, can_optimize: true, next_optimized: None });
                nodes.extend(rev.iter().rev().copied());
                nodes.extend(original.nodes.iter().skip(tail_start).copied());
                for i in 0..nodes.len() {
                    nodes[i].next_optimized = if i + 1 < nodes.len() { Some(i + 1) } else { None };
                }
                let mut path = Path { nodes, blocked_by_ally: blocked_here };
                self.optimize(&mut path, obj, blocked);
                self.clean_search();
                return Some(path);
            }
            self.info[p].closed = true;
            if cell_count < CELL_LIMIT {
                cell_count += self.examine_neighboring_cells(terrain, p, None, None, &ctx, (sx, sy));
            }
        }
        self.is_tunneling = false;
        self.clean_search();
        None
    }

    // ---- A* ------------------------------------------------------------------------------------

    /// `examineCellsCallback`; returns `true` to abort the line walk.
    fn examine_cells_callback(&mut self, ctx: &Ctx, from: Option<usize>, to_x: i32, to_y: i32, goal: usize) -> bool {
        if self.is_tunneling {
            return true;
        }
        let (Some(from), Some(to)) = (from, self.get_cell(to_x, to_y)) else { return false };
        if !self.valid_movement(ctx.surfaces, to, ctx.obj.crusher) {
            return true;
        }
        // `examineCellsCallback`: a ground cell outside the hierarchical corridor is rejected.
        if !self.search_on_wall && !self.zones.is_passable(to_x, to_y) {
            return true;
        }
        if self.cells[to].pinched {
            return true;
        }
        if ctx.is_human && (to_x < 0 || to_y < 0 || to_x > self.w - 1 || to_y > self.h - 1) {
            return true;
        }
        if !self.check_for_movement(to_x, to_y, ctx.radius, ctx.center) {
            return true;
        }
        // `examineCellsCallback`: a fixed enemy or fixed ally in the footprint aborts the straight
        // line (`AIPathfind.cpp:6151-6157`, the RETAIL_COMPATIBLE_PATHFINDING branch). The main
        // neighbour expansion adds cost and sets `blockedByAlly` instead of aborting.
        if !self.search_on_wall {
            let info = self.check_for_movement_cell(&ctx.obj, to_x, to_y, ctx.radius, ctx.center, false);
            if info.enemy_fixed || info.ally_fixed_count > 0 {
                return true;
            }
        }
        // 0.5f * COST_ORTHOGONAL
        let new_cost_so_far = u32::from(self.info[from].so_far) + 5;
        if self.cells[to].ty == CellType::Cliff {
            return true;
        }
        if !self.allocate_info(to) {
            return true; // out of cells for pathing
        }
        let cost_remaining = self.cost_to_goal(to, goal);
        let ti = self.info[to];
        if ti.has && (ti.open || ti.closed) && u32::from(ti.so_far) <= new_cost_so_far {
            return false; // already on a list with a cheaper route: keep going
        }
        self.info[to].so_far = new_cost_so_far as u16;
        self.info[to].parent = from as u32;
        self.info[to].total = (u32::from(self.info[to].so_far) as i32 + cost_remaining) as u16;
        if self.info[to].closed {
            self.info[to].closed = false;
        }
        if self.info[to].open {
            self.remove_from_open(to);
        }
        self.put_on_open(to);
        false
    }

    /// `Pathfinder::examineNeighboringCells` (no downhill-only locomotors). `goal` is `None` for the
    /// searches that accept any cell (`getMoveAwayFromPath`, `patchPath`); `attack_distance` is
    /// `Some` only for `findAttackPath` (`AIPathfind.cpp:6207`).
    fn examine_neighboring_cells(&mut self, terrain: &TerrainLogic, parent: usize, goal: Option<usize>, attack_distance: Option<f32>, ctx: &Ctx, start_cell: (i32, i32)) -> i32 {
        let (px, py) = self.xy(parent);
        // The straight-line pre-pass runs only for a normal (non-attack) ground search to a goal
        // (`attackDistance==NO_ATTACK && !isDownhillOnly && goalCell`).
        if !self.is_tunneling {
            if let (Some(goal), None) = (goal, attack_distance) {
                let cells = self.line_cells((px, py), self.xy(goal));
                let mut prev: Option<usize> = None;
                for (x, y) in cells {
                    let to = self.idx(x, y);
                    if self.examine_cells_callback(ctx, prev, x, y, goal) {
                        break;
                    }
                    prev = Some(to);
                }
            }
        }

        let mut cell_count = 0;
        const DELTA: [(i32, i32); 8] = [(1, 0), (0, 1), (-1, 0), (0, -1), (1, 1), (-1, 1), (-1, -1), (1, -1)];
        const FIRST_DIAGONAL: usize = 4;
        const ADJACENT: [usize; 5] = [0, 1, 2, 3, 0];
        let mut neighbor_flags = [false; 8];
        let from_z = terrain.ground_height(px as f32 * PATHFIND_CELL_SIZE_F, py as f32 * PATHFIND_CELL_SIZE_F);

        for i in 0..8 {
            neighbor_flags[i] = false;
            let (nx, ny) = (px + DELTA[i].0, py + DELTA[i].1);
            let Some(nc) = self.get_cell(nx, ny) else { continue };
            // (isHuman && outside the logical extent): the logical extent is the whole grid here.
            let ni = self.info[nc];
            if ni.has && (ni.open || ni.closed) {
                continue;
            }
            if i >= FIRST_DIAGONAL {
                // make sure one of the adjacent sides is open.
                if !neighbor_flags[ADJACENT[i - 4]] && !neighbor_flags[ADJACENT[i - 3]] {
                    continue;
                }
            }
            let mut movement_valid = self.valid_movement(ctx.surfaces, nc, ctx.obj.crusher);
            // `examineNeighboringCells`'s dozer exception (`AIPathfind.cpp:6384-6399`): a cell of an
            // obstacle that is not an enemy of the dozer counts as a valid movement position, so the
            // dozer can tunnel out of (or into) its own building. `dozerHack` keeps it out of the
            // diagonal adjacency flags, and the cell still costs `100*COST_ORTHOGONAL` below. The
            // C++ test is `obj->getRelationship(obstacle) == ENEMIES` (`Object.cpp:1593` ->
            // `Team::getRelationship` -> `Player::getRelationship`); the port has no team or player
            // relation overrides and no undetected-defector status, so the per-player enemy table is
            // that same test.
            let mut dozer_hack = false;
            if !movement_valid && ctx.obj.dozer && self.cells[nc].ty == CellType::Obstacle {
                let obstacle = self.cells[nc].obstacle;
                if obstacle != 0 && self.obstacle_owners.get(&obstacle).is_some_and(|&owner| !self.are_enemies(ctx.obj.player, owner)) {
                    movement_valid = true;
                    dozer_hack = true;
                }
            }
            if !movement_valid && !self.is_tunneling {
                continue;
            }
            if !dozer_hack {
                neighbor_flags[i] = true;
            }

            // `checkForMovement`: the off-map test and `enemyFixed` (`AIPathfind.cpp:6332`). A
            // fixed enemy in the footprint aborts the neighbour expansion (the A* routes around it
            // or falls back to the closest reachable cell). Allied fixed units are handled below
            // (cost + `blockedByAlly`).
            let move_info = self.check_for_movement_cell(&ctx.obj, nx, ny, ctx.radius, ctx.center, false);
            if !self.check_for_movement(nx, ny, ctx.radius, ctx.center) || move_info.enemy_fixed {
                if !self.is_tunneling {
                    continue;
                }
                movement_valid = false;
            }
            if movement_valid && !self.cells[nc].pinched {
                // Only turn off tunneling after the check for movement.
                self.is_tunneling = false;
            }
            if !self.info[nc].has {
                // `if (!newCell->hasInfo()) { allocateInfo; cellCount++ }` (AIPathfind.cpp:6344-6349): a unit, goal or
                // obstacle cell keeps its PathfindCellInfo, so only a cell without one is counted (lane 191: the
                // attack search's ATTACK_CELL_LIMIT counted those cells, and stopped where C++ carries on).
                let had_info = self.cell_has_info(nc);
                if !self.allocate_info(nc) {
                    return cell_count; // out of cells for pathing
                }
                if !had_info {
                    cell_count += 1;
                }
            }

            let mut new_cost = self.cost_so_far(nc, parent) as u32;
            if move_info.ally_moving {
                // `if (info.allyMoving && dx<10 && dy<10)` (`AIPathfind.cpp:6544-6552`): the distance is
                // to the *search's start cell* (`startCellNdx`, `worldToCell(from)`), not to the parent
                // being expanded — the port used the parent, which is always one cell away, so every
                // cell near a moving ally paid the 3*COST_DIAGONAL penalty (AN1 s1 f2837, lane 170).
                let dx = (nx - start_cell.0).abs();
                let dy = (ny - start_cell.1).abs();
                if dx < 10 && dy < 10 {
                    new_cost = new_cost.wrapping_add((3 * COST_DIAGONAL) as u32);
                }
            }
            if self.cells[nc].ty == CellType::Cliff && !self.cells[nc].pinched {
                let to_z = terrain.ground_height(nx as f32 * PATHFIND_CELL_SIZE_F, ny as f32 * PATHFIND_CELL_SIZE_F);
                let d = from_z - to_z;
                let d = if d < 0.0 { -d } else { d };
                if d < PATHFIND_CELL_SIZE_F {
                    new_cost = new_cost.wrapping_add((7 * COST_DIAGONAL) as u32);
                }
            } else if self.cells[nc].pinched {
                new_cost = new_cost.wrapping_add(COST_ORTHOGONAL as u32);
            }

            // `RETAIL_COMPATIBLE_PATHFINDING`: fixed allies add `3*COST_DIAGONAL` per ally and mark
            // the cell `blockedByAlly` unless the unit may path through units.
            let mut blocked_by_ally = false;
            if move_info.ally_fixed_count > 0 {
                new_cost = new_cost.wrapping_add(3 * COST_DIAGONAL as u32 * move_info.ally_fixed_count);
                if !ctx.obj.can_path_through_units {
                    blocked_by_ally = true;
                }
            }

            let mut cost_remaining = match (goal, attack_distance) {
                (Some(goal), None) => self.cost_to_goal(nc, goal),
                (Some(goal), Some(attack_distance)) => {
                    let (gx, gy) = self.xy(goal);
                    let dx = nx - gx;
                    let dy = ny - gy;
                    // `costRemaining = COST_ORTHOGONAL*sqrt(dx*dx + dy*dy)` (AIPathfind.cpp:6410 of the Core fork,
                    // shipped copy :6324): `dx`, `dy` are Int, so `sqrt` is the double overload and the product
                    // is truncated into the Int. `attackDistance` is an Int (:6231), truncated from the weapon's
                    // Real attack distance (:10840, then `+= 3*PATHFIND_CELL_SIZE`), so `attackDistance/2` is an
                    // integer halving: truncate first, then halve.
                    let d2 = (dx * dx + dy * dy) as f64;
                    let ad = attack_distance as i32;
                    let mut c = (COST_ORTHOGONAL as f64 * d2.sqrt()) as i32 - ad / 2;
                    if c < 0 {
                        c = 0;
                    }
                    if move_info.ally_goal {
                        if ctx.obj.surfaces & surface::AIR == 0 && ctx.obj.infantry {
                            new_cost = new_cost.wrapping_add(COST_ORTHOGONAL as u32);
                        } else {
                            new_cost = new_cost.wrapping_add((3 * COST_ORTHOGONAL) as u32);
                        }
                    }
                    c
                }
                (None, _) => 0,
            };
            // A ground cell outside the hierarchical corridor costs `100*COST_ORTHOGONAL` extra
            // (`AIPathfind.cpp:6310-6314`, `:6445-6447`); the corridor is what steers the fine A*
            // after the straight-line pre-pass bails (`:6167`). `getMoveAwayFromPath`/`patchPath`
            // lift the corridor with `setAllPassable` (`:10433`, `:10619`).
            if !self.search_on_wall && !self.zones.is_passable(nx, ny) {
                new_cost = new_cost.wrapping_add((100 * COST_ORTHOGONAL) as u32);
            }
            if self.cells[nc].ty == CellType::Obstacle {
                new_cost = new_cost.wrapping_add((100 * COST_ORTHOGONAL) as u32);
            }
            if self.is_tunneling && !self.valid_movement(ctx.surfaces, nc, ctx.obj.crusher) {
                new_cost = new_cost.wrapping_add((10 * COST_ORTHOGONAL) as u32);
            }

            self.info[nc].so_far = new_cost as u16;
            self.info[nc].parent = parent as u32;
            self.info[nc].blocked_by_ally = blocked_by_ally;
            if self.is_tunneling {
                cost_remaining = 0; // find the closest valid cell.
            }
            self.info[nc].total = (i32::from(self.info[nc].so_far) + cost_remaining) as u16;

            if self.info[nc].closed {
                self.info[nc].closed = false;
            }
            if self.info[nc].open {
                self.remove_from_open(nc);
            }
            self.put_on_open(nc);
        }
        cell_count
    }

    fn ctx_for(obj: &PathObject) -> Ctx {
        let (radius, center) = obj.radius_and_center();
        Ctx { surfaces: obj.surfaces, radius, center, is_human: true, obj: *obj }
    }

    /// `Pathfinder::findPath`: a short, valid path or `None` (goal unreachable or blocked). A unit
    /// whose `layer` is `LAYER_WALL` searches the wall layer.
    pub fn find_path(&mut self, terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, raw_to: &Coord3D) -> Option<Path> {
        self.on_layer(obj.layer, |pf| pf.ground_find_path(terrain, obj, from, raw_to))
    }

    /// The ground-layer body of [`find_path`](Self::find_path): quick check, hierarchical corridor,
    /// then the fine A*.
    fn ground_find_path(&mut self, terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, raw_to: &Coord3D) -> Option<Path> {
        if !self.quick_does_path_exist(obj, from, raw_to) {
            return None;
        }
        let trace = hpath_trace(self.frame);
        if trace {
            eprintln!(
                "ZR_HPATH f={} req=findPath id={} from=({:.3},{:.3}) to=({:.3},{:.3})",
                self.frame, obj.id.0, from.x, from.y, raw_to.x, raw_to.y
            );
        }
        // `Pathfinder::findPath`: `clearPassableFlags()` then the hierarchical path; a successful
        // hierarchical path leaves the corridor's passable flags set, otherwise every block is
        // passable (no restriction) (`AIPathfind.cpp:6455-6461`).
        self.zones.clear_passable_flags();
        let hier = self.internal_find_hierarchical_path(obj.surfaces, false, from, raw_to, false);
        if hier.is_none() {
            self.zones.set_all_passable();
        }
        if trace {
            eprintln!("ZR_HPATH f={} id={} hier={}", self.frame, obj.id.0, if hier.is_some() { "ok" } else { "fail" });
            hpath_dump_corridor("findPath", obj.id.0, self.frame, &self.zones);
        }
        let path = self.internal_find_path(terrain, obj, from, raw_to);
        if trace {
            hpath_dump_path("findPath", obj.id.0, self.frame, path.as_ref());
        }
        path
    }

    /// `Pathfinder::internalFindPath`.
    fn internal_find_path(&mut self, terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, raw_to: &Coord3D) -> Option<Path> {
        let ctx = Self::ctx_for(obj);
        if raw_to.x == 0.0 && raw_to.y == 0.0 {
            return None; // "Attempting pathfind to 0,0, generally a bug."
        }
        if !self.map_ready {
            return None;
        }
        let mut adjust_to = *raw_to;
        let mut clip_from = *from;
        self.clip(&mut clip_from, &mut adjust_to);
        // `startCellNdx` (`AIPathfind.cpp:6811-6812`): the cell of the clipped start position, used by
        // the moving-ally distance test in `examineNeighboringCells`.
        let (fx, fy, _) = self.world_to_cell(&clip_from);
        if !ctx.center {
            adjust_to.x += PATHFIND_CELL_SIZE_F / 2.0;
            adjust_to.y += PATHFIND_CELL_SIZE_F / 2.0;
        }
        self.is_tunneling = false;

        let goal = self.cell_at(&adjust_to)?;
        let (gx, gy, _) = self.world_to_cell(&adjust_to);
        // `checkDestination(obj, ...)` (`AIPathfind.cpp:6640-6643`): the full unit test, not just the
        // cell surfaces. The `UNIT_PRESENT_FIXED`/goal-owner rules decide a blocked goal here.
        if !self.check_destination_unit(&ctx.obj, gx, gy, ctx.radius, ctx.center, &|u| self.unit_objs.get(&u).map(|p| p.player)) {
            return None;
        }
        let mut parent = self.clipped_cell_at(&clip_from)?;
        if !self.allocate_info(goal) {
            return None;
        }
        if parent != goal && !self.allocate_info(parent) {
            self.clean_search();
            return None;
        }
        // A tunneling pathfind starts from within an obstacle and ignores obstacle cells until it
        // reaches a cell that is no longer one.
        self.is_tunneling = self.cells[parent].ty == CellType::Obstacle;

        // The zone screen (`AIPathfind.cpp:6684-6715`): if the start and the goal are not in the same
        // effective zone for this locomotor, no route exists and the search is skipped. A footprint
        // change can leave cells `UNINITIALIZED_ZONE` (0), which reads as "unknown" — a wall-layer
        // search refuses those outright (`:6686-6694`). When the goal is the obstacle the search was
        // told to ignore, or the start is inside an obstacle (tunneling), both zones are remapped
        // through the terrain table first ("Use terrain zones instead of building zones, since we are
        // moving into or out of a building").
        let mut zone1 = self.zones.effective_zone(ctx.surfaces, obj.crusher, self.zone_of(parent));
        let mut zone2 = self.zones.effective_zone(ctx.surfaces, obj.crusher, self.zone_of(goal));
        if self.search_on_wall && (zone1 == 0 || zone2 == 0) {
            self.clean_search();
            return None;
        }
        let goal_is_ignored_obstacle =
            self.cells[goal].ty == CellType::Obstacle && self.ignore_obstacle != 0 && self.cells[goal].obstacle == self.ignore_obstacle;
        if goal_is_ignored_obstacle || self.is_tunneling {
            zone2 = self.zones.effective_terrain_zone(zone2);
            zone2 = self.zones.effective_zone(ctx.surfaces, obj.crusher, zone2);
            zone1 = self.zones.effective_terrain_zone(zone1);
            zone1 = self.zones.effective_zone(ctx.surfaces, obj.crusher, zone1);
        }
        if zone1 != zone2 {
            self.clean_search();
            return None;
        }

        // Sanity check - if destination is invalid, can't path there.
        if !self.valid_movement_position(obj.surfaces, &adjust_to, obj.crusher) {
            self.is_tunneling = false;
            self.clean_search();
            return None;
        }
        // If the source is invalid, we have to cheat.
        if !self.valid_movement_position(obj.surfaces, from, obj.crusher) {
            self.is_tunneling = true;
        }

        self.start_pathfind(parent, goal);
        self.open.clear();
        self.put_on_open(parent);

        while let Some(p) = self.pop_open() {
            parent = p;
            if parent == goal {
                self.is_tunneling = false;
                let path = self.build_actual_path(terrain, obj, from, goal, ctx.center, false);
                self.clean_search();
                return Some(path);
            }
            self.info[parent].closed = true;
            self.examine_neighboring_cells(terrain, parent, Some(goal), None, &ctx, (fx, fy));
        }
        // failure - goal cannot be reached
        self.is_tunneling = false;
        self.clean_search();
        None
    }

    /// `Pathfinder::findClosestPath` (blocked = false, pathCostMultiplier = 0): succeeds even when
    /// the goal is unreachable, by heading for the closest reachable cell. `raw_to` is updated to
    /// that cell's centre in the latter case. `blocked` is the caller's `m_isBlockedAndStuck`, which
    /// decides whether a transient unit at the start or along the path counts as fixed
    /// (`AIPathfind.cpp:8759`, `Path::optimize`).
    pub fn find_closest_path(&mut self, terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, raw_to: &mut Coord3D, blocked: bool) -> Option<Path> {
        self.on_layer(obj.layer, |pf| pf.find_closest_path_ground(terrain, obj, from, raw_to, blocked))
    }

    /// The ground-layer body of [`find_closest_path`](Self::find_closest_path).
    fn find_closest_path_ground(&mut self, terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, raw_to: &mut Coord3D, blocked: bool) -> Option<Path> {
        let ctx = Self::ctx_for(obj);
        let path_cost_multiplier = 0.0f32;
        if obj.surfaces == 0 || !self.map_ready {
            return None;
        }
        self.is_tunneling = false;
        let mut adjust_to = *raw_to;
        if !ctx.center {
            adjust_to.x += PATHFIND_CELL_SIZE_F / 2.0;
            adjust_to.y += PATHFIND_CELL_SIZE_F / 2.0;
        }
        let mut clip_from = *from;
        self.clip(&mut clip_from, &mut adjust_to);
        let goal = self.clipped_cell_at(&adjust_to)?;
        let (sx, sy, _) = self.world_to_cell(from);
        let mut parent = self.clipped_cell_at(&clip_from)?;
        if !self.valid_movement(obj.surfaces, parent, obj.crusher) {
            self.is_tunneling = true; // can't move from the current location: relax the constraints
        }
        if !self.valid_movement(obj.surfaces, parent, obj.crusher) {
            self.is_tunneling = true; // can't move from the current location: relax the constraints
        }
        // `TCheckMovementInfo startInfo; startInfo.considerTransient = blocked;` (`AIPathfind.cpp:8752-8760`):
        // a fixed unit in the start footprint (or the blocked caller's transient allies) relaxes the search.
        let start_info = self.check_for_movement_cell(obj, sx, sy, ctx.radius, ctx.center, blocked);
        if start_info.enemy_fixed {
            self.is_tunneling = true;
        }
        // `findClosestPath`: establish the hierarchical corridor (closest-ok); if none, every block
        // is passable. A path that starts stuck cannot be optimised, so no corridor is used.
        let trace = hpath_trace(self.frame);
        if trace {
            eprintln!(
                "ZR_HPATH f={} req=findClosestPath id={} from=({:.3},{:.3}) to=({:.3},{:.3}) blocked={}",
                self.frame, obj.id.0, from.x, from.y, adjust_to.x, adjust_to.y, blocked as u8
            );
        }
        let got_hierarchical_path = if self.is_tunneling {
            self.zones.set_all_passable();
            false
        } else {
            self.zones.clear_passable_flags();
            let found = self.internal_find_hierarchical_path(obj.surfaces, false, from, &adjust_to, true).is_some();
            if !found {
                self.zones.set_all_passable();
            }
            found
        };
        if trace {
            eprintln!("ZR_HPATH f={} id={} hier={}", self.frame, obj.id.0, if got_hierarchical_path { "ok" } else { "fail" });
            hpath_dump_corridor("findClosestPath", obj.id.0, self.frame, &self.zones);
        }
        let started_stuck = self.is_tunneling;

        if !self.allocate_info(goal) {
            return None;
        }
        if parent != goal && !self.allocate_info(parent) {
            self.clean_search();
            return None;
        }
        self.start_pathfind(parent, goal);

        let mut closest_cell: Option<usize> = None;
        let mut closest_distance_sqr = f32::MAX;
        let mut closest_dist_screen_sqr = f32::MAX;
        self.open.clear();
        self.put_on_open(parent);
        let mut count: i32 = 0;
        let mut found_goal = false;
        let (goal_x, goal_y) = self.xy(goal);
        let cost_to_distance_factor_sqr = (1.0f32 / 10.0) * (1.0f32 / 10.0);

        while let Some(p) = self.pop_open() {
            parent = p;
            if parent == goal {
                // success - found a path to the goal; if it is not a valid destination accept the closest cell.
                // `if (closesetCell!=nullptr && !canPathThroughUnits && !checkDestination(...))`
                // (`AIPathfind.cpp:8837-8844`): a unit that may path through units (the dock machine sets
                // it) keeps the goal even when another unit is on it. `goalOnObstacle` (the ignored
                // airfield obstacle, `:8794-8815`) is not modelled.
                let valid_dest = ctx.obj.can_path_through_units
                    || {
                        let (x, y) = self.xy(parent);
                        self.check_destination_unit(&ctx.obj, x, y, ctx.radius, ctx.center, &|u| self.unit_objs.get(&u).map(|p| p.player))
                    };
                if closest_cell.is_some() && !valid_dest {
                    found_goal = true;
                    // Continue processing the open list to find a possibly closer cell.
                    continue;
                }
                self.is_tunneling = false;
                let path = self.build_actual_path(terrain, obj, from, goal, ctx.center, blocked);
                self.clean_search();
                if trace {
                    hpath_dump_path("findClosestPath", obj.id.0, self.frame, Some(&path));
                }
                return Some(path);
            }
            self.info[parent].closed = true;
            let (x, y) = self.xy(parent);
            if !self.is_tunneling
                && self.check_destination_unit(&ctx.obj, x, y, ctx.radius, ctx.center, &|u| self.unit_objs.get(&u).map(|p| p.player))
                && (!started_stuck || self.valid_movement(obj.surfaces, parent, obj.crusher))
            {
                let dx = (goal_x - x).abs() as f32;
                let dy = (goal_y - y).abs() as f32;
                let mut dist_sqr = dx * dx + dy * dy;
                if dist_sqr < closest_dist_screen_sqr {
                    closest_dist_screen_sqr = dist_sqr;
                }
                let so_far = u32::from(self.info[parent].so_far) as f32;
                dist_sqr += (so_far * (so_far * cost_to_distance_factor_sqr)) * path_cost_multiplier;
                if dist_sqr < closest_distance_sqr {
                    closest_cell = Some(parent);
                    closest_distance_sqr = dist_sqr;
                }
            }
            let dx = (goal_x - x).abs() as f32;
            let dy = (goal_y - y).abs() as f32;
            let dist_sqr = dx * dx + dy * dy;
            // If we are 2x farther than the closest location already found, don't continue.
            if dist_sqr > closest_dist_screen_sqr * 4.0 {
                let mut skip = false;
                if !got_hierarchical_path {
                    skip = true;
                }
                if count > 2000 {
                    skip = true;
                }
                if closest_dist_screen_sqr < 10.0 * 10.0 * PATHFIND_CELL_SIZE_F {
                    skip = true;
                }
                if skip {
                    continue;
                }
            }
            if !found_goal {
                count += self.examine_neighboring_cells(terrain, parent, Some(goal), None, &ctx, (sx, sy));
            }
        }

        if let Some(cc) = closest_cell {
            // success - found a path to near the goal
            self.is_tunneling = false;
            let (x, y) = self.xy(cc);
            raw_to.x = x as f32 * PATHFIND_CELL_SIZE_F + PATHFIND_CELL_SIZE_F / 2.0;
            raw_to.y = y as f32 * PATHFIND_CELL_SIZE_F + PATHFIND_CELL_SIZE_F / 2.0;
            let path = self.build_actual_path(terrain, obj, from, cc, ctx.center, blocked);
            self.clean_search();
            if trace {
                hpath_dump_path("findClosestPath", obj.id.0, self.frame, Some(&path));
            }
            return Some(path);
        }
        self.is_tunneling = false;
        self.clean_search();
        if trace {
            hpath_dump_path("findClosestPath", obj.id.0, self.frame, None);
        }
        None
    }

    /// `AIUpdateInterface::computePath` for ground units: the waypoints a `MoveTo` order produces.
    ///
    /// Off-map start and goal give the straight line (`computeQuickPath`); airborne locomotors
    /// fly straight (aircraft paths are not ported); a blocked or unreachable goal falls back to
    /// the closest reachable cell; no path at all gives an empty list (the unit stays put).
    pub fn compute_move_path(&mut self, terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, to: &Coord3D) -> Vec<Coord3D> {
        self.compute_move_path_group(terrain, obj, from, to, None)
    }

    /// [`compute_move_path`](Self::compute_move_path) with the group destination that drives
    /// `checkDestination`'s tightening (`AIGroup::computeIndividualDestination` ->
    /// `adjustDestination(..., groupDest)`). `None` is a single-unit move.
    pub fn compute_move_path_group(&mut self, terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, to: &Coord3D, group_dest: Option<Coord3D>) -> Vec<Coord3D> {
        // The wall layer is built lazily, so it must exist before deriving the start layer.
        self.ensure_wall_layer();
        // `AIUpdateInterface::computePath` starts on `obj->getLayer()`, which `updateLayer` sets from
        // the object's z. The sim does not keep an object layer, so derive it here the same way.
        let layer = if self.wall_rect.is_some() { self.layer_for_destination(terrain, from) } else { PathLayer::Ground };
        self.on_layer(layer, |pf| pf.compute_move_path_ground(terrain, obj, from, to, group_dest))
    }

    /// The ground-layer body of [`compute_move_path`](Self::compute_move_path).
    fn compute_move_path_ground(&mut self, terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, to: &Coord3D, group_dest: Option<Coord3D>) -> Vec<Coord3D> {
        if obj.surfaces & surface::AIR != 0 && obj.surfaces & surface::GROUND == 0 {
            return vec![*to];
        }
        let (ww, wh) = terrain.world_size();
        let in_region = |p: &Coord3D| p.x >= 0.0 && p.x <= ww && p.y >= 0.0 && p.y <= wh;
        if !in_region(to) && !in_region(from) {
            return vec![*to]; // starting off the map: just build a path
        }
        // `AIUpdateInterface::computePath` -> `adjustDestination`: reserve a valid, non-overlapping
        // goal cell for this unit before finding the route. In the engine the adjust runs in the AI
        // state *before* `computePath` sets the ignored obstacle (`AIStates.cpp:1661-1664`,
        // `AIUpdate.cpp:1722`), i.e. with the pathfinder's field as the previous request left it.
        let mut dest = self.adjust_destination(terrain, obj, to, group_dest.as_ref()).unwrap_or(*to);
        self.ignore_obstacle = 0;
        let mut path = None;
        if self.valid_movement_position(obj.surfaces, &dest, obj.crusher) {
            // The wrapped `find_path` would swap the layer again; call the ground body directly,
            // which runs the hierarchical corridor then the fine A* like `findPath`.
            path = self.ground_find_path(terrain, obj, from, &dest);
        }
        if path.is_none() {
            path = self.find_closest_path_ground(terrain, obj, from, &mut dest, false);
        }
        match path {
            Some(p) => {
                // `AIUpdateInterface::computePath`: after a path is found, tell allies to move.
                // `moveAllies` for a computed path runs in `GameLogic` when the path is installed (`store_ai_path`).
                p.waypoints()
            }
            None => Vec::new(),
        }
    }

    /// `computePath`'s straight-line test (`AIUpdate.cpp:1709-1711`): `isLinePassable(obj, surfaces, layer, pos,
    /// destination, false, true)` on the unit's layer, with the unit's ignored obstacle set the way `computePath` sets
    /// it (`:1703`). A passable line makes a quick path, and it returns before the search clears the pathfinder's
    /// ignored obstacle (`:1736`), so that value stays set, as in C++.
    pub fn line_passable_for_path(&mut self, terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, to: &Coord3D, ignore: ObjectId) -> bool {
        let mut obj = *obj;
        obj.ignore_obstacle = ignore;
        self.ensure_wall_layer();
        let layer = if self.wall_rect.is_some() { self.layer_for_destination(terrain, from) } else { PathLayer::Ground };
        self.on_layer(layer, |pf| {
            pf.ignore_obstacle = ignore.0;
            pf.is_line_passable(&obj, from, to, false, true)
        })
    }

    /// `AIUpdateInterface::computePath`'s ground branch for the async pathfind queue. Returns the
    /// raw [`Path`] so the caller can keep it for a later `patchPath`. `blocked_and_stuck` picks
    /// `patchPath` (stitching back onto `original`) over a fresh `findPath`
    /// (`AIUpdate.cpp:1721-1728`); `ignore_obstacle` is `computePath`'s `setIgnoreObstacleID`
    /// (`AIUpdate.cpp:1703`, set to the attack victim by `doPathfind`'s fallback). `adjusts` is
    /// `AIInternalMoveToState::getAdjustsDestination()`: the state calls `adjustDestination` in
    /// `onEnter` only when it is set (`AIStates.cpp:1661`), so an enter move onto an occupied target
    /// (`setAdjustsDestination(false)`, `:6269`) must keep the raw goal. `None` means no ground path
    /// (quick/off-map paths are the caller's job).
    #[allow(clippy::too_many_arguments)]
    pub fn compute_ai_path(
        &mut self,
        terrain: &TerrainLogic,
        obj: &PathObject,
        from: &Coord3D,
        to: &Coord3D,
        group_dest: Option<Coord3D>,
        blocked_and_stuck: bool,
        original: Option<&Path>,
        ignore_obstacle: ObjectId,
        adjusts: bool,
    ) -> Option<Path> {
        if obj.surfaces & surface::AIR != 0 && obj.surfaces & surface::GROUND == 0 {
            return None;
        }
        // `computePath` runs with the object's own ignored obstacle (`AIUpdateInterface::getIgnoredObstacleID()`,
        // passed in as `ignore_obstacle`), which `checkDestination`/`checkForMovement` read (`AIPathfind.cpp:5023`,
        // `:5081`). It is a different value from the pathfinder's search-scoped `m_ignoreObstacleID`, which
        // `computePath` sets around the search (`AIUpdate.cpp:1722`/`:1755`) and `validMovementPosition` reads.
        let mut obj = *obj;
        obj.ignore_obstacle = ignore_obstacle;
        let obj = &obj;
        // ZR_L128_PATH (diagnostic, read-only): the dock workers' path requests around the AN3 f1547 divergence.
        let l128 = l128_path_trace_on() && matches!(obj.id.0, 238..=240) && (1500..=1600).contains(&self.frame);
        if l128 {
            eprintln!(
                "ZR_L128_PATH f={} id={} from=({:.4},{:.4}) to=({:.4},{:.4}) group={:?} blocked={} original={} ignore={} adjusts={}",
                self.frame,
                obj.id.0,
                from.x,
                from.y,
                to.x,
                to.y,
                group_dest,
                blocked_and_stuck,
                original.is_some(),
                ignore_obstacle.0,
                adjusts
            );
        }

        let (ww, wh) = terrain.world_size();
        let in_region = |p: &Coord3D| p.x >= 0.0 && p.x <= ww && p.y >= 0.0 && p.y <= wh;
        if !in_region(to) && !in_region(from) {
            return None;
        }
        self.ensure_wall_layer();
        let layer = if self.wall_rect.is_some() { self.layer_for_destination(terrain, from) } else { PathLayer::Ground };
        self.on_layer(layer, |pf| {
            // `AIUpdateInterface::computePath` sets the ignored obstacle just before `findPath`/`patchPath`
            // (`AIUpdate.cpp:1722`) and clears it at the end (`:1755`), so the destination adjust that the AI
            // state runs before requesting the path (`AIInternalMoveToState::onEnter` -> `adjustDestination`,
            // `AIStates.cpp:1661-1664`) sees the value the previous request left behind. That value gates
            // `validMovementPosition`'s "ignore this obstacle" pass (`AIPathfind.cpp:4993`), i.e. whether a
            // goal cell inside a building is a legal movement position at all.
            let mut dest = if adjusts {
                pf.adjust_destination(terrain, obj, to, group_dest.as_ref()).unwrap_or(*to)
            } else {
                *to
            };
            pf.ignore_obstacle = ignore_obstacle.0;
            // `Path *theNewPath = nullptr;` (`AIUpdate.cpp:1705`). `computePath` destroyed the unit's old
            // path at its top unless it is blocked and stuck (`:1662-1664`), so `m_path == nullptr` for
            // every later test in this function except that case.
            let mut path = None;
            if blocked_and_stuck {
                // `AIUpdate.cpp:1720-1723`: stuck -> `patchPath` back onto the old path. The engine does
                // *not* fall through to `findPath` in this arm.
                if let Some(orig) = original {
                    path = pf.patch_path(terrain, obj, from, orig, blocked_and_stuck);
                }
            } else if pf.valid_movement_position(obj.surfaces, &dest, obj.crusher) {
                // `AIUpdate.cpp:1726`: `pathServices->findPath(...)`, which builds the hierarchical
                // corridor and then runs the fine A* inside it (`AIPathfind.cpp:6444-6469`).
                path = pf.ground_find_path(terrain, obj, from, &dest);
            }
            // `if (theNewPath==nullptr && m_path==nullptr) { ... findClosestPath ... }` (`AIUpdate.cpp:1728-1733`):
            // a blocked-and-stuck unit that still holds its old path keeps it (the caller then stops and
            // waits, AIUpdate.cpp:1765-1779); every other failure searches for the closest reachable cell.
            let keeps_old_path = blocked_and_stuck && original.is_some();
            if path.is_none() && !keeps_old_path {
                path = pf.find_closest_path_ground(terrain, obj, from, &mut dest, blocked_and_stuck);
            }
            if l128 {
                match &path {
                    Some(p) => eprintln!(
                        "ZR_L128_PATH f={} id={} dest=({:.4},{:.4}) wps={:?}",
                        pf.frame,
                        obj.id.0,
                        dest.x,
                        dest.y,
                        p.waypoints().iter().map(|w| (w.x, w.y)).collect::<Vec<_>>()
                    ),
                    None => eprintln!("ZR_L128_PATH f={} id={} dest=({:.4},{:.4}) wps=none", pf.frame, obj.id.0, dest.x, dest.y),
                }
            }
            pf.ignore_obstacle = 0; // `computePath` clears it (`AIUpdate.cpp:1736`).
            path
        })
    }

    /// `Pathfinder::getMoveAwayFromPath` (`AIPathfind.cpp:10291`) for the AI's
    /// `privateMoveAwayFromUnit`: a route whose clearance box avoids the other unit's path.
    #[allow(clippy::too_many_arguments)]
    pub fn move_away_from_unit(&mut self, terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, avoid: &[Coord3D], avoid2: &[Coord3D], other_radius: i32, other_center: bool) -> Option<Path> {
        self.find_move_away_path(terrain, obj, from, avoid, avoid2, other_radius, other_center)
    }

    // ---- path construction ---------------------------------------------------------------------

    /// `Pathfinder::getAircraftPath` (`AIPathfind.cpp:5918-5970`): "Just jogs around tall buildings
    /// marked with `KINDOF_AIRCRAFT_PATH_AROUND`." A real two-node `Path` — the object's position (at
    /// the destination's z, `:5945-5946`), then the destination — with the destination circle-clipped
    /// for a `LOCO_WINGS` aircraft (`circleClipsTallBuilding`, `:5930-5942`) and detour nodes inserted
    /// around every tall building the segment crosses (`segmentIntersectsTallBuilding`, `:5947-5969`).
    /// Then every node's `nextOptimized` points at the next one (`:5963-5968`).
    ///
    /// `closest_tall` is `ThePartitionManager->getClosestObject(pos, radius, FROM_BOUNDINGSPHERE_2D,
    /// KINDOF_AIRCRAFT_PATH_AROUND)` (`:9633`, `:9638`) as `(id, position, bounding_circle_radius)`;
    /// `tall_by_id` is the `TheGameLogic->findObjectByID(cell->getObstacleID())` lookup of
    /// `segmentIntersectsBuildingCallback` (`:9310-9327`) with the same tuple. `avoid` is
    /// `AIUpdateInterface::getBuildingToNotPathAround()` (`:5926`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn aircraft_path(
        &self,
        from: &Coord3D,
        to_z: f32,
        to: &Coord3D,
        check_clips: bool,
        avoid: ObjectId,
        closest_tall: &dyn Fn(&Coord3D) -> Option<TallBuilding>,
        tall_by_id: &dyn Fn(u32) -> Option<(Coord3D, f32)>,
    ) -> Path {
        // `Real radius = 100; Coord3D adjDest = *to; if (checkClips) circleClipsTallBuilding(...)`
        // (`:5938-5942`).
        let mut adj_dest = *to;
        if check_clips {
            Self::circle_clips_tall_building(from, to, 100.0, closest_tall, &mut adj_dest);
        }
        let node = |pos: Coord3D| PathNode { pos, can_optimize: false, next_optimized: None };
        let mut nodes = vec![node(Coord3D::new(from.x, from.y, to_z)), node(adj_dest)];
        // `Int limit = 20; ... limit--; if (limit<0) break;` (`:5949`, `:5960-5961`).
        let mut limit = 20;
        let mut cur = 0usize;
        while cur + 1 < nodes.len() {
            let from_pos = nodes[cur].pos;
            let mut next_pos = nodes[cur + 1].pos;
            let (mut insert1, mut insert2, mut insert3) = (Coord3D::ZERO, Coord3D::ZERO, Coord3D::ZERO);
            let found = self.segment_intersects_tall_building(&from_pos, &mut next_pos, avoid, tall_by_id, &mut insert1, &mut insert2, &mut insert3);
            if found {
                nodes[cur + 1].pos = next_pos;
                // `curNode->append(newNode3); curNode->append(newNode2); curNode->append(newNode1);` each
                // inserts immediately after `curNode` (`PathNode::append`, `:179-188`), so the list becomes
                // `cur, n1, n2, n3, next`. `curNode = newNode2; curNode = curNode->getNext();` then
                // continues from `n3` (`:5952-5957`).
                nodes.insert(cur + 1, node(insert3));
                nodes.insert(cur + 1, node(insert2));
                nodes.insert(cur + 1, node(insert1));
                cur += 3;
            } else {
                cur += 1;
            }
            limit -= 1;
            if limit < 0 {
                break;
            }
        }
        // `for (curNode = first; curNode && curNode->getNext();) curNode->setNextOptimized(curNode->getNext());`
        // then `markOptimized()` (`:5963-5969`): the optimized chain is the whole node list.
        for i in 0..nodes.len().saturating_sub(1) {
            nodes[i].next_optimized = Some(i + 1);
        }
        Path { nodes, blocked_by_ally: false }
    }

    /// `Pathfinder::circleClipsTallBuilding` (`AIPathfind.cpp:9628-9645`): if a `KINDOF_AIRCRAFT_PATH_AROUND`
    /// building is within `circle_radius` of `to` (bounding-sphere distance), push `adjust_to` out along the
    /// normal from the line `from`..`to` at `circle_radius + building_radius + 2*PATHFIND_CELL_SIZE_F`
    /// (`computeNormalRadialOffset`, `:9538-9566`). A second building near the clipped point shifts it once
    /// more. The `ignoreBuilding` parameter of the original is unused inside it.
    #[allow(clippy::too_many_arguments)]
    fn circle_clips_tall_building(
        from: &Coord3D,
        to: &Coord3D,
        circle_radius: f32,
        closest_tall: &dyn Fn(&Coord3D) -> Option<TallBuilding>,
        adjust_to: &mut Coord3D,
    ) -> bool {
        let Some((tall_id, tall_pos, tall_radius)) = closest_tall(to) else { return false };
        let radius = tall_radius + 2.0 * PATHFIND_CELL_SIZE_F;
        compute_normal_radial_offset(from, adjust_to, to, &tall_pos, circle_radius + radius);
        if let Some((other_id, other_pos, other_radius)) = closest_tall(adjust_to) {
            if other_id != tall_id {
                let radius = other_radius + 2.0 * PATHFIND_CELL_SIZE_F;
                let tmp = *adjust_to;
                compute_normal_radial_offset(from, adjust_to, &tmp, &other_pos, circle_radius + radius);
            }
        }
        true
    }

    /// `Pathfinder::segmentIntersectsTallBuilding` (`AIPathfind.cpp:9570-9624`): walk the cells of the
    /// segment `from`..`to`; the first `CELL_OBSTACLE` whose object is `KINDOF_AIRCRAFT_PATH_AROUND` (and is
    /// not `avoid`) makes it tall (`segmentIntersectsBuildingCallback`, `:9310-9327`). Two passes, as in the
    /// original: when the segment's far end is inside the building's radius the node's position is pushed
    /// out to the radius and the scan repeats (`:9587-9603`, which is why `to` is mutable); otherwise three
    /// detour points are computed around the building (`:9605-9619`) and returned.
    #[allow(clippy::too_many_arguments)]
    fn segment_intersects_tall_building(
        &self,
        from: &Coord3D,
        to: &mut Coord3D,
        avoid: ObjectId,
        tall_by_id: &dyn Fn(u32) -> Option<(Coord3D, f32)>,
        insert1: &mut Coord3D,
        insert2: &mut Coord3D,
        insert3: &mut Coord3D,
    ) -> bool {
        for _ in 0..2 {
            let from_pos = *from;
            let to_pos = *to;
            let (sx, sy, _) = self.world_to_cell(&from_pos);
            let (ex, ey, _) = self.world_to_cell(&to_pos);
            let mut found: Option<(u32, Coord3D, f32)> = None;
            for (x, y) in self.line_cells((sx, sy), (ex, ey)) {
                let Some(ci) = self.get_cell(x, y) else { break };
                let c = &self.cells[ci];
                if c.ty != CellType::Obstacle || c.obstacle == 0 || c.obstacle == avoid.0 {
                    continue;
                }
                if let Some((pos, radius)) = tall_by_id(c.obstacle) {
                    found = Some((c.obstacle, pos, radius));
                    break;
                }
            }
            let Some((_, bldg_pos, bldg_radius)) = found else { return false };
            let radius = bldg_radius + 2.0 * PATHFIND_CELL_SIZE_F;
            let mut delta = (to_pos.x - bldg_pos.x, to_pos.y - bldg_pos.y);
            if norm2d(delta) <= radius * 0.98 {
                if norm2d(delta) < 0.1 {
                    delta.0 = 1.0;
                }
                delta = normalize2d(delta);
                to.x = bldg_pos.x + delta.0 * radius;
                to.y = bldg_pos.y + delta.1 * radius;
                continue;
            }
            let mut from_pos = from_pos;
            let mut delta = (from_pos.x - bldg_pos.x, from_pos.y - bldg_pos.y);
            if norm2d(delta) <= radius * 0.98 {
                if norm2d(delta) < 0.1 {
                    delta.0 = 1.0;
                }
                delta = normalize2d(delta);
                from_pos.x = bldg_pos.x + delta.0 * radius;
                from_pos.y = bldg_pos.y + delta.1 * radius;
            }
            compute_normal_radial_offset(&from_pos, insert2, &to_pos, &bldg_pos, radius);
            compute_normal_radial_offset(&from_pos, insert1, insert2, &bldg_pos, radius);
            compute_normal_radial_offset(insert2, insert3, &to_pos, &bldg_pos, radius);
            return true;
        }
        false
    }

    /// `adjustCoordToCell`: a waypoint on a bridge over impassable ground sits at the deck height.
    /// `Pathfinder::snapPosition` (`Core/.../AIPathfind.cpp:5216`): the centre of the cell `pos` lies in,
    /// with the unit's footprint offset applied when it is not centred.
    pub(crate) fn snap_position(&self, terrain: &TerrainLogic, obj: &PathObject, pos: &Coord3D) -> Coord3D {
        let (_, center) = obj.radius_and_center();
        let mut adjust = *pos;
        if !center {
            adjust.x += PATHFIND_CELL_SIZE_F / 2.0;
            adjust.y += PATHFIND_CELL_SIZE_F / 2.0;
        }
        let (cx, cy, _) = self.world_to_cell(&adjust);
        self.adjust_coord_to_cell(terrain, cx, cy, center)
    }


    fn adjust_coord_to_cell(&self, terrain: &TerrainLogic, cx: i32, cy: i32, center: bool) -> Coord3D {
        let (x, y) = if center {
            ((cx as f32 + 0.5) * PATHFIND_CELL_SIZE_F, (cy as f32 + 0.5) * PATHFIND_CELL_SIZE_F)
        } else {
            // `(Real)cellX + 0.05` is a double expression in the original; float here is within 1e-6.
            ((cx as f32 + 0.05) * PATHFIND_CELL_SIZE_F, (cy as f32 + 0.05) * PATHFIND_CELL_SIZE_F)
        };
        let z = match self.get_cell(cx, cy) {
            Some(ci) if self.cells[ci].bridge != 0 && self.cells[ci].bridge_over_impassable => self.cells[ci].bridge_height,
            _ => terrain.ground_height(x, y),
        };
        Coord3D::new(x, y, z)
    }

    /// `buildActualPath` + `prependCells`, then `Path::optimize`.
    fn build_actual_path(&mut self, terrain: &TerrainLogic, obj: &PathObject, from: &Coord3D, goal_cell: usize, center: bool, blocked: bool) -> Path {
        let mut goal_cell = goal_cell;
        let gp = self.info[goal_cell].parent;
        if self.cells[goal_cell].pinched && gp != NONE && !self.cells[gp as usize].pinched {
            goal_cell = gp as usize;
        }

        // Nodes are collected goal -> start and reversed at the end ("prepend").
        let mut rev: Vec<PathNode> = Vec::new();
        let goal_cell_null = self.info[goal_cell].parent == NONE;
        let mut blocked_by_ally = self.info[goal_cell].blocked_by_ally;
        let mut cell = goal_cell;
        let mut prev_cell: Option<usize> = None;
        while self.info[cell].parent != NONE {
            blocked_by_ally |= self.info[cell].blocked_by_ally;
            let (x, y) = self.xy(cell);
            let pos = self.adjust_coord_to_cell(terrain, x, y, center);
            let mut can_optimize = true;
            if self.cells[cell].ty == CellType::Cliff {
                if let Some(pc) = prev_cell {
                    if self.cells[pc].ty != CellType::Cliff {
                        if let Some(first) = rev.last_mut() {
                            first.can_optimize = false;
                        }
                    }
                }
            } else if let Some(pc) = prev_cell {
                if self.cells[pc].ty == CellType::Cliff {
                    can_optimize = false;
                }
            }
            rev.push(PathNode { pos, can_optimize, next_optimized: None });
            let parent = self.info[cell].parent as usize;
            if let Some(pc) = prev_cell {
                self.info[pc].parent = NONE; // prevCell->clearParentCell()
            }
            prev_cell = Some(cell);
            cell = parent;
        }
        if goal_cell_null {
            // Very short path.
            let (x, y) = self.xy(cell);
            blocked_by_ally |= self.info[cell].blocked_by_ally;
            rev.push(PathNode { pos: self.adjust_coord_to_cell(terrain, x, y, center), can_optimize: true, next_optimized: None });
        }
        // Put the actual start position first, so the path begins at the unit's feet.
        if rev.last().is_none_or(|n| from.x != n.pos.x || from.y != n.pos.y) {
            rev.push(PathNode { pos: *from, can_optimize: true, next_optimized: None });
        }
        rev.reverse();
        let mut path = Path { nodes: rev, blocked_by_ally };
        self.optimize(&mut path, obj, blocked);
        path
    }

    /// `Path::optimize`: keep the farthest node with a clear line of sight from each anchor.
    fn optimize(&self, path: &mut Path, obj: &PathObject, blocked: bool) {
        let n = path.nodes.len();
        if n == 0 {
            return;
        }
        let last = n - 1;
        let mut anchor = 0usize;
        while anchor != last {
            let mut optimized_segment = false;
            // Find the farthest candidate: stop at the last node or at a node that must not be skipped.
            let mut node = anchor + 1;
            while node + 1 < n {
                if !path.nodes[node].can_optimize {
                    break;
                }
                node += 1;
            }
            while node != anchor {
                let a_pos = path.nodes[anchor].pos;
                let n_pos = path.nodes[node].pos;
                let mut is_passable = self.is_line_passable(obj, &a_pos, &n_pos, blocked, false);
                if let Some(c) = self.cell_at(&n_pos) {
                    if self.cells[c].ty == CellType::Cliff && !self.cells[c].pinched {
                        is_passable = true;
                    }
                }
                // Horizontal, diagonal and vertical steps are passable.
                if !is_passable {
                    let mut dx = (n_pos.x - a_pos.x) as i32;
                    let mut dy = (n_pos.y - a_pos.y) as i32;
                    let mut might_be_passable = false;
                    if dx.abs() == PATHFIND_CELL_SIZE && dy.abs() == PATHFIND_CELL_SIZE {
                        is_passable = true;
                    }
                    let step = |t: usize| -> (i32, i32) {
                        let (p, q) = (path.nodes[t].pos, path.nodes[t + 1].pos);
                        ((q.x - p.x) as i32, (q.y - p.y) as i32)
                    };
                    if dx == 0 {
                        might_be_passable = true;
                        let mut t = node;
                        while t > anchor + 1 {
                            t -= 1;
                            dx = step(t).0;
                            if dx != 0 {
                                might_be_passable = false;
                            }
                        }
                    }
                    if dy == 0 {
                        might_be_passable = true;
                        let mut t = node;
                        while t > anchor + 1 {
                            t -= 1;
                            dy = step(t).1;
                            if dy != 0 {
                                might_be_passable = false;
                            }
                        }
                    }
                    if dx == dy {
                        might_be_passable = true;
                        let mut t = node;
                        while t > anchor + 1 {
                            t -= 1;
                            let s = step(t);
                            dx = s.0;
                            dy = s.1;
                            if dy != dx {
                                might_be_passable = false;
                            }
                        }
                    }
                    if dx == -dy {
                        might_be_passable = true;
                        let mut t = node;
                        while t > anchor + 1 {
                            t -= 1;
                            let s = step(t);
                            dx = s.0;
                            dy = s.1;
                            if dy != -dx {
                                might_be_passable = false;
                            }
                        }
                    }
                    if might_be_passable {
                        is_passable = true;
                    }
                }
                if is_passable {
                    // anchor can directly see this node, make it next in the optimized path
                    path.nodes[anchor].next_optimized = Some(node);
                    anchor = node;
                    optimized_segment = true;
                    break;
                }
                node -= 1;
            }
            if !optimized_segment {
                // no clear LOS between the anchor node and the very next node
                path.nodes[anchor].next_optimized = Some(anchor + 1);
                anchor += 1;
            }
        }
    }
}

/// 2D distance between two bounding circles (`PartitionManager::getGoalDistanceSquared`).
fn boundary_dist_sqr(a: &Coord3D, ra: f32, b: &Coord3D, rb: f32) -> f32 {
    let d = a.dist_2d(b) - ra - rb;
    if d <= 0.0 {
        0.0
    } else {
        d * d
    }
}

/// `LineInRegion` (`TerrainLogic.cpp:464`): does the segment `p1 -> p2` intersect the axis-aligned
/// region `[lo_x, hi_x] x [lo_y, hi_y]`? The float twin of [`clip_line_2d`], used by
/// `getMoveAwayFromPath` to test a cell's clearance box against a path segment.
pub fn line_in_region(p1: Coord3D, p2: Coord3D, lo: (f32, f32), hi: (f32, f32)) -> bool {
    const LEFT: i32 = 0x01;
    const RIGHT: i32 = 0x02;
    const BOTTOM: i32 = 0x04;
    const TOP: i32 = 0x08;
    let (clip_left, clip_top, clip_right, clip_bottom) = (lo.0, lo.1, hi.0, hi.1);
    let (mut x1, mut y1, mut x2, mut y2) = (p1.x, p1.y, p2.x, p2.y);
    let code = |x: f32, y: f32| {
        let mut c = 0;
        if x < clip_left {
            c = LEFT;
        } else if x > clip_right {
            c = RIGHT;
        }
        if y < clip_top {
            c |= TOP;
        } else if y > clip_bottom {
            c |= BOTTOM;
        }
        c
    };
    let code1 = code(x1, y1);
    let code2 = code(x2, y2);
    if code1 | code2 == 0 {
        return true;
    }
    if code1 & code2 != 0 {
        return false;
    }
    if code1 != 0 {
        if code1 & TOP != 0 {
            let diff = y2 - y1;
            if diff == 0.0 {
                return false;
            }
            x1 += (x2 - x1) * (clip_top - y1) / diff;
            y1 = clip_top;
        } else if code1 & BOTTOM != 0 {
            let diff = y2 - y1;
            if diff == 0.0 {
                return false;
            }
            x1 += (x2 - x1) * (clip_bottom - y1) / diff;
            y1 = clip_bottom;
        }
        if x1 > clip_right {
            let diff = x2 - x1;
            if diff == 0.0 {
                return false;
            }
            y1 += (y2 - y1) * (clip_right - x1) / diff;
            x1 = clip_right;
        } else if x1 < clip_left {
            let diff = x2 - x1;
            if diff == 0.0 {
                return false;
            }
            y1 += (y2 - y1) * (clip_left - x1) / diff;
            x1 = clip_left;
        }
    }
    if code2 != 0 {
        if code2 & TOP != 0 {
            let diff = y2 - y1;
            if diff == 0.0 {
                return false;
            }
            x2 += (x2 - x1) * (clip_top - y2) / diff;
            y2 = clip_top;
        } else if code2 & BOTTOM != 0 {
            let diff = y2 - y1;
            if diff == 0.0 {
                return false;
            }
            x2 += (x2 - x1) * (clip_bottom - y2) / diff;
            y2 = clip_bottom;
        }
        if x2 > clip_right {
            let diff = x2 - x1;
            if diff == 0.0 {
                return false;
            }
            y2 += (y2 - y1) * (clip_right - x2) / diff;
            x2 = clip_right;
        } else if x2 < clip_left {
            let diff = x2 - x1;
            if diff == 0.0 {
                return false;
            }
            y2 += (y2 - y1) * (clip_left - x2) / diff;
            x2 = clip_left;
        }
    }
    x1 >= clip_left && x1 <= clip_right && y1 >= clip_top && y1 <= clip_bottom && x2 >= clip_left && x2 <= clip_right && y2 >= clip_top && y2 <= clip_bottom
}

/// `ClipLine2D` (Core/GameEngine/Source/GameClient/Line2D.cpp): integer Cohen-Sutherland clip of
/// `p1 -> p2` to `(lo_x, lo_y, hi_x, hi_y)`. `None` when the line is not visible.
pub fn clip_line_2d(p1: (i32, i32), p2: (i32, i32), region: (i32, i32, i32, i32)) -> Option<((i32, i32), (i32, i32))> {
    const LEFT: i32 = 0x01;
    const RIGHT: i32 = 0x02;
    const BOTTOM: i32 = 0x04;
    const TOP: i32 = 0x08;
    let (clip_left, clip_top, clip_right, clip_bottom) = region;
    let (mut x1, mut y1) = p1;
    let (mut x2, mut y2) = p2;
    let code = |x: i32, y: i32| {
        let mut c = 0;
        if x < clip_left {
            c = LEFT;
        } else if x > clip_right {
            c = RIGHT;
        }
        if y < clip_top {
            c |= TOP;
        } else if y > clip_bottom {
            c |= BOTTOM;
        }
        c
    };
    let code1 = code(x1, y1);
    let code2 = code(x2, y2);
    if code1 | code2 == 0 {
        return Some((p1, p2));
    }
    if code1 & code2 != 0 {
        return None;
    }
    if code1 != 0 {
        if code1 & TOP != 0 {
            let diff = y2 - y1;
            if diff == 0 {
                return None;
            }
            x1 += (x2 - x1).wrapping_mul(clip_top - y1) / diff;
            y1 = clip_top;
        } else if code1 & BOTTOM != 0 {
            let diff = y2 - y1;
            if diff == 0 {
                return None;
            }
            x1 += (x2 - x1).wrapping_mul(clip_bottom - y1) / diff;
            y1 = clip_bottom;
        }
        if x1 > clip_right {
            let diff = x2 - x1;
            if diff == 0 {
                return None;
            }
            y1 += (y2 - y1).wrapping_mul(clip_right - x1) / diff;
            x1 = clip_right;
        } else if x1 < clip_left {
            let diff = x2 - x1;
            if diff == 0 {
                return None;
            }
            y1 += (y2 - y1).wrapping_mul(clip_left - x1) / diff;
            x1 = clip_left;
        }
    }
    if code2 != 0 {
        if code2 & TOP != 0 {
            let diff = y2 - y1;
            if diff == 0 {
                return None;
            }
            x2 += (x2 - x1).wrapping_mul(clip_top - y2) / diff;
            y2 = clip_top;
        } else if code2 & BOTTOM != 0 {
            let diff = y2 - y1;
            if diff == 0 {
                return None;
            }
            x2 += (x2 - x1).wrapping_mul(clip_bottom - y2) / diff;
            y2 = clip_bottom;
        }
        if x2 > clip_right {
            let diff = x2 - x1;
            if diff == 0 {
                return None;
            }
            y2 += (y2 - y1).wrapping_mul(clip_right - x2) / diff;
            x2 = clip_right;
        } else if x2 < clip_left {
            let diff = x2 - x1;
            if diff == 0 {
                return None;
            }
            y2 += (y2 - y1).wrapping_mul(clip_left - x2) / diff;
            x2 = clip_left;
        }
    }
    let visible = x1 >= clip_left && x1 <= clip_right && y1 >= clip_top && y1 <= clip_bottom && x2 >= clip_left && x2 <= clip_right && y2 >= clip_top && y2 <= clip_bottom;
    visible.then_some(((x1, y1), (x2, y2)))
}

#[cfg(test)]
#[path = "pathfind_l161.rs"]
mod l161_tests;

/// Lane 176 (wave 20): `patchPath`'s doubly-adjusted start cell (R20-8).
#[cfg(test)]
#[path = "pathfind_l176.rs"]
mod l176_tests;

/// Lane 179 (R21-1): the line-of-sight march's diagonal-step cell (`iterateCellsAlongLine`).
#[cfg(test)]
#[path = "pathfind_l179.rs"]
mod l179_tests;

/// Lane 180 (R21-5, item 2): the shared attack view inside `findAttackPath`.
#[cfg(test)]
#[path = "pathfind_l180.rs"]
mod l180_tests;

/// Lane 188 (AN5 s1 crawler 263): the attack goal's undersized range and oversized minimum.
#[cfg(test)]
#[path = "pathfind_l188.rs"]
mod l188_tests;

/// Lane 191 (AN5 s1 crawler 263): a crusher may stand on a fixed squishable unit's cell, and a unit's cell does not count
/// toward the search limit (`pathfind_l191.rs`).
#[cfg(test)]
#[path = "pathfind_l191.rs"]
mod l191_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::LocomotorKind;

    fn tank_obj() -> PathObject {
        PathObject { id: ObjectId::INVALID, surfaces: surface::GROUND, bounding_radius: 6.0, layer: crate::pathfind::PathLayer::Ground, player: 0, infantry: false, crusher: false, dozer: false, harvester: false, aircraft_that_adjusts_destination: false, is_aircraft: false, can_path_through_units: false, ignore_obstacle: ObjectId::INVALID, attacking: false, busy: false, using_ability: false }
    }

    /// 60x60 playable cells of flat terrain with an optional raised wall.
    fn terrain_with(wall: impl Fn(usize, usize) -> u8) -> TerrainLogic {
        let (w, h) = (62usize, 62usize);
        let mut data = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                data[y * w + x] = wall(x, y);
            }
        }
        TerrainLogic::from_samples(w as i32, h as i32, 0, data)
    }

    fn pos(x: f32, y: f32) -> Coord3D {
        Coord3D::new(x, y, 0.0)
    }

    fn dist(a: &Coord3D, b: &Coord3D) -> f32 {
        a.dist_2d(b)
    }

    fn path_len(from: &Coord3D, wps: &[Coord3D]) -> f32 {
        let mut prev = *from;
        let mut total = 0.0;
        for w in wps {
            total += dist(&prev, w);
            prev = *w;
        }
        total
    }

    /// `internal_findHierarchicalPath`'s `newCell->hasInfo()` gate (`AIPathfind.cpp:7464-7467`): the
    /// coarse search can only leave a block through a border cell that already holds a
    /// `PathfindCellInfo` — a unit, a goal, an obstacle, or a cell this search allocated. On bare
    /// ground no cell has one, so the search fails and `findPath` lifts the corridor
    /// (`setAllPassable`). A unit standing on the border cell (9,5) gives it one: the search steps to
    /// (10,5) in block (1,0), which is the goal block, and returns the two-block route.
    /// Revert check: using `info[ci].has` instead of `cell_has_info` makes the second part fail.
    #[test]
    fn a_hierarchical_path_can_only_leave_a_block_through_an_allocated_border_cell() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let from = pos(55.0, 55.0);
        let to = pos(155.0, 55.0);
        assert!(
            pf.find_hierarchical_path(surface::GROUND, &from, &to).is_none(),
            "no cell on bare ground has a PathfindCellInfo, so the coarse search cannot move"
        );
        // A unit on the block's right-border cell (9,5): `setPosUnit` allocates its info and the
        // border cell is clear ground of the parent's zone, so the scan may step across.
        let unit = unit(1);
        pf.update_unit_pos(&unit, &pos(95.0, 55.0), false);
        pf.zones.clear_passable_flags(); // `findPath`'s `clearPassableFlags` before the search
        let route = pf.find_hierarchical_path(surface::GROUND, &from, &to).expect("a two-block corridor");
        assert_eq!(route, vec![(0, 0), (1, 0)]);
        assert!(pf.block_passable_at(&pos(55.0, 55.0)));
        assert!(pf.block_passable_at(&pos(155.0, 55.0)));
        // The fine A* finds the route under the corridor gate.
        let wps = pf.compute_move_path(&t, &tank_obj(), &from, &to);
        assert!(dist(wps.last().unwrap(), &to) < 8.0, "{wps:?}");
    }

    /// `buildHierarchicalPath`'s start expansion (`AIPathfind.cpp:6892-6911`, compiled in this Zero
    /// Hour build): every block touched by the 21×21-cell box around the path's first node becomes
    /// passable, so a unit can get around friendly units near its start. Block (1,1) is inside that
    /// box ((55,55) ± 100 units covers cells 0..15 both ways) but not on the two-block route.
    /// Revert check: dropping the expansion loop leaves block (1,1) impassable.
    #[test]
    fn the_hierarchical_corridor_expands_around_the_start() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let unit = unit(1);
        pf.update_unit_pos(&unit, &pos(95.0, 55.0), false);
        pf.zones.clear_passable_flags();
        pf.find_hierarchical_path(surface::GROUND, &pos(55.0, 55.0), &pos(155.0, 55.0)).expect("a corridor");
        assert!(pf.block_passable_at(&pos(55.0, 55.0)), "the start block");
        assert!(pf.block_passable_at(&pos(155.0, 55.0)), "the goal block");
        assert!(pf.block_passable_at(&pos(55.0, 155.0)), "block (0,1) is inside the start box");
        assert!(pf.block_passable_at(&pos(155.0, 155.0)), "block (1,1) is inside the start box");
        // A block well outside both the route and the box stays closed.
        assert!(!pf.block_passable_at(&pos(455.0, 455.0)), "block (4,4) is outside the corridor");
    }

    /// `processHierarchicalCell` closes the block-border cell (`newCell->allocateInfo(scanCell);
    /// if (!closed && !open) putOnClosedList`, `AIPathfind.cpp:7508-7512`). In the engine that record
    /// is the same `PathfindCellInfo` a unit/obstacle cell keeps permanently, and the search's
    /// cleanup clears the flag (`releaseClosedList`). The port keeps the search state in a separate
    /// `Info`, so it has to allocate one for the border cell too: otherwise the `closed` flag is left
    /// behind and the *next* search's side scan stops on it (`:7469-7470`), and the same route can no
    /// longer be found. Revert check: dropping the `allocate_info(new_cell)` call makes the second
    /// search fail.
    #[test]
    fn a_border_cell_closed_by_one_search_does_not_stop_the_next() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let unit = unit(1);
        pf.update_unit_pos(&unit, &pos(95.0, 55.0), false);
        let from = pos(55.0, 55.0);
        let to = pos(155.0, 55.0);
        pf.zones.clear_passable_flags();
        let first = pf.find_hierarchical_path(surface::GROUND, &from, &to).expect("a two-block corridor");
        pf.zones.clear_passable_flags();
        let second = pf.find_hierarchical_path(surface::GROUND, &from, &to).expect("the closed border cell is cleared");
        assert_eq!(second, first);
        assert_eq!(second, vec![(0, 0), (1, 0)]);
    }

    /// `internal_findHierarchicalPath`'s zone screen (`AIPathfind.cpp:7620-7629`): a start and a goal
    /// whose effective zones differ give no hierarchical path at all — even for the "closest" variant
    /// (`findClosestHierarchicalPath` is the same function with `closestOK`). `findPath`/
    /// `findClosestPath` then lift the corridor and let the fine A* decide.
    #[test]
    fn a_hierarchical_path_refuses_a_goal_in_another_zone() {
        // A plateau island sealed by a cliff ring cannot be reached on the ground.
        let t = terrain_with(|x, y| if (20..30).contains(&x) && (20..30).contains(&y) { 60 } else { 0 });
        let mut pf = Pathfinder::new(&t);
        assert!(pf.find_hierarchical_path(surface::GROUND, &pos(55.0, 255.0), &pos(255.0, 255.0)).is_none());
        assert!(
            pf.find_closest_hierarchical_path(surface::GROUND, &pos(55.0, 255.0), &pos(255.0, 255.0)).is_none(),
            "the zone screen runs before the closest-cell arm"
        );
        // `findClosestPath` still returns a path: with no corridor every block is passable.
        let mut raw_to = pos(255.0, 255.0);
        let path = pf.find_closest_path(&t, &tank_obj(), &pos(55.0, 255.0), &mut raw_to, false);
        assert!(path.is_some(), "the fine search takes over");
    }

    #[test]
    fn open_ground_path_is_a_straight_line() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let wps = pf.compute_move_path(&t, &tank_obj(), &pos(55.0, 55.0), &pos(455.0, 355.0));
        assert_eq!(wps.len(), 1, "{wps:?}");
        assert!(dist(&wps[0], &pos(455.0, 355.0)) < 8.0);
    }

    #[test]
    fn cliff_classification_uses_slope_rule_and_expands() {
        // A step of 32 samples (20 units) at x = 30 makes cell column 29 a cliff.
        let t = terrain_with(|x, _| if x >= 30 { 32 } else { 0 });
        let pf = Pathfinder::new(&t);
        assert_eq!(pf.cell_type_at(&pos(295.0, 100.0)), Some(CellType::Cliff));
        // Neighbours of cliff cells become cliffs too (first expansion), then a pinched ring.
        assert_eq!(pf.cell_type_at(&pos(285.0, 100.0)), Some(CellType::Cliff));
        assert_eq!(pf.cell_type_at(&pos(305.0, 100.0)), Some(CellType::Cliff));
        assert_eq!(pf.cell_type_at(&pos(275.0, 100.0)), Some(CellType::Clear));
        assert!(pf.is_pinched_at(&pos(275.0, 100.0)));
        assert!(!pf.is_pinched_at(&pos(100.0, 100.0)));
    }

    #[test]
    fn path_goes_around_a_cliff_block() {
        // Plateau 16..24 x 8..40 (cells), 30 sample steps => cliff ring.
        let t = terrain_with(|x, y| if (16..24).contains(&x) && (8..40).contains(&y) { 40 } else { 0 });
        let mut pf = Pathfinder::new(&t);
        let from = pos(105.0, 205.0);
        let to = pos(325.0, 205.0);
        let wps = pf.compute_move_path(&t, &tank_obj(), &from, &to);
        assert!(wps.len() >= 2, "must bend around the block: {wps:?}");
        assert!(dist(wps.last().unwrap(), &to) < 8.0);
        // No waypoint on a cliff and no leg crossing the plateau column.
        let mut prev = from;
        for w in &wps {
            assert_ne!(pf.cell_type_at(w), Some(CellType::Cliff), "{w:?}");
            if (prev.x < 160.0 && w.x > 240.0) || (prev.x > 240.0 && w.x < 160.0) {
                assert!(w.y < 70.0 || w.y > 410.0 || prev.y < 70.0 || prev.y > 410.0);
            }
            prev = *w;
        }
        assert!(path_len(&from, &wps) > dist(&from, &to) + 100.0);
    }

    fn building() -> ObjectTemplate {
        let mut t = ObjectTemplate::new("Block");
        t.kind_of = kind_of::STRUCTURE;
        t.geometry = Geometry { shape: GeometryShape::Box, major_radius: 40.0, minor_radius: 40.0, height: 20.0, is_small: false };
        t
    }

    #[test]
    fn path_goes_around_a_structure_footprint() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let b = building();
        pf.classify_object_footprint(&t, ObjectId(7), &b, &pos(300.0, 200.0), 0.0, true);
        assert_eq!(pf.cell_type_at(&pos(300.0, 200.0)), Some(CellType::Obstacle));
        assert_eq!(pf.cell_type_at(&pos(222.0, 200.0)), Some(CellType::Clear));
        let from = pos(105.0, 205.0);
        let to = pos(505.0, 205.0);
        let wps = pf.compute_move_path(&t, &tank_obj(), &from, &to);
        assert!(wps.len() >= 2, "{wps:?}");
        for w in &wps {
            assert_ne!(pf.cell_type_at(w), Some(CellType::Obstacle));
        }
        // The path clears the 80x80 block.
        let mut prev = from;
        for w in &wps {
            let steps = 40;
            for s in 0..=steps {
                let f = s as f32 / steps as f32;
                let (x, y) = (prev.x + (w.x - prev.x) * f, prev.y + (w.y - prev.y) * f);
                assert!(!(x > 262.0 && x < 338.0 && y > 162.0 && y < 238.0), "({x},{y}) inside block");
            }
            prev = *w;
        }
        // Removing the structure restores the straight line.
        pf.classify_object_footprint(&t, ObjectId(7), &b, &pos(300.0, 200.0), 0.0, false);
        assert_eq!(pf.cell_type_at(&pos(300.0, 200.0)), Some(CellType::Clear));
        let wps = pf.compute_move_path(&t, &tank_obj(), &from, &to);
        assert_eq!(wps.len(), 1, "{wps:?}");
    }

    #[test]
    fn mobile_small_and_airborne_things_are_not_obstacles() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let mut mobile = building();
        mobile.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
        pf.classify_object_footprint(&t, ObjectId(1), &mobile, &pos(300.0, 200.0), 0.0, true);
        let mut small = building();
        small.geometry.is_small = true;
        pf.classify_object_footprint(&t, ObjectId(2), &small, &pos(300.0, 200.0), 0.0, true);
        let mut not_structure = building();
        not_structure.kind_of = kind_of::VEHICLE;
        pf.classify_object_footprint(&t, ObjectId(3), &not_structure, &pos(300.0, 200.0), 0.0, true);
        pf.classify_object_footprint(&t, ObjectId(4), &building(), &Coord3D::new(300.0, 200.0, 50.0), 0.0, true);
        assert_eq!(pf.cell_type_at(&pos(300.0, 200.0)), Some(CellType::Clear));
    }

    #[test]
    fn unreachable_goal_moves_to_closest_reachable_cell() {
        // Plateau island 20..30 x 20..30 (cells): its interior is clear but sealed by cliffs.
        let t = terrain_with(|x, y| if (20..30).contains(&x) && (20..30).contains(&y) { 60 } else { 0 });
        let mut pf = Pathfinder::new(&t);
        let from = pos(55.0, 255.0);
        let to = pos(255.0, 255.0); // centre of the island
        let obj = tank_obj();
        assert_eq!(pf.cell_type_at(&to), Some(CellType::Clear));
        assert!(!pf.quick_does_path_exist(&obj, &from, &to));
        assert!(pf.find_path(&t, &obj, &from, &to).is_none());
        let wps = pf.compute_move_path(&t, &obj, &from, &to);
        assert!(!wps.is_empty());
        let end = *wps.last().unwrap();
        assert_ne!(pf.cell_type_at(&end), Some(CellType::Cliff));
        // It stops at the island's shore (outside the cliff ring, which spans cells 17..=31). Like the
        // original, a pinched closest cell retreats to its unpinched parent, so the end is one cell off.
        assert!(end.x > 100.0 && end.x < 340.0 && end.y > 100.0 && end.y < 340.0, "{end:?}");
        let outside_ring = end.x < 170.0 || end.x > 320.0 || end.y < 170.0 || end.y > 320.0;
        assert!(outside_ring, "{end:?}");
        assert!(dist(&end, &to) < dist(&from, &to));
    }

    #[test]
    fn goal_inside_an_obstacle_goes_next_to_it() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        pf.classify_object_footprint(&t, ObjectId(7), &building(), &pos(300.0, 200.0), 0.0, true);
        let from = pos(55.0, 205.0);
        let wps = pf.compute_move_path(&t, &tank_obj(), &from, &pos(300.0, 200.0));
        let end = *wps.last().unwrap();
        assert_ne!(pf.cell_type_at(&end), Some(CellType::Obstacle));
        assert!(end.x < 262.0, "approaches from the west: {end:?}");
        assert!(dist(&end, &pos(300.0, 200.0)) < 70.0, "{end:?}");
    }

    #[test]
    fn paths_are_deterministic_and_instances_agree() {
        let t = terrain_with(|x, y| if (16..24).contains(&x) && (8..40).contains(&y) { 40 } else { 0 });
        let mk = || {
            let mut pf = Pathfinder::new(&t);
            pf.classify_object_footprint(&t, ObjectId(7), &building(), &pos(400.0, 300.0), 0.7, true);
            pf
        };
        let (mut a, mut b) = (mk(), mk());
        let obj = tank_obj();
        let q = [(pos(55.0, 55.0), pos(555.0, 505.0)), (pos(105.0, 205.0), pos(325.0, 205.0)), (pos(555.0, 55.0), pos(75.0, 555.0))];
        for (f, g) in q {
            let p1 = a.compute_move_path(&t, &obj, &f, &g);
            let p2 = a.compute_move_path(&t, &obj, &f, &g);
            let p3 = b.compute_move_path(&t, &obj, &f, &g);
            assert!(!p1.is_empty());
            assert_eq!(p1, p2);
            assert_eq!(p1, p3);
        }
        let (mut ca, mut cb) = (Crc::new(), Crc::new());
        a.crc(&mut ca);
        b.crc(&mut cb);
        assert_eq!(ca.get(), cb.get());
    }

    #[test]
    fn cost_function_matches_the_original_constants() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        // Straight run then a diagonal: costs 10 each, 14 for the diagonal, plus 4 for the 45 degree turn.
        let (a, b, c) = (pf.idx(5, 5), pf.idx(6, 5), pf.idx(7, 5));
        let d = pf.idx(8, 6);
        assert!(pf.allocate_info(a) && pf.allocate_info(b) && pf.allocate_info(c) && pf.allocate_info(d));
        pf.info[b].so_far = pf.cost_so_far(b, a) as u16;
        pf.info[b].parent = a as u32;
        assert_eq!(pf.info[b].so_far, 10);
        pf.info[c].so_far = pf.cost_so_far(c, b) as u16;
        pf.info[c].parent = b as u32;
        assert_eq!(pf.info[c].so_far, 20);
        assert_eq!(pf.cost_so_far(d, c), 20 + 14 + 4);
        assert_eq!(pf.cost_to_goal(a, d), 30 + 10 / 2);
    }

    #[test]
    fn open_list_is_fifo_among_equal_costs() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let cells = [pf.idx(1, 1), pf.idx(2, 1), pf.idx(3, 1), pf.idx(4, 1)];
        for (i, &c) in cells.iter().enumerate() {
            pf.allocate_info(c);
            pf.info[c].total = if i == 3 { 5 } else { 9 };
            pf.put_on_open(c);
        }
        assert_eq!(pf.pop_open(), Some(cells[3]));
        assert_eq!(pf.pop_open(), Some(cells[0]));
        assert_eq!(pf.pop_open(), Some(cells[1]));
        assert_eq!(pf.pop_open(), Some(cells[2]));
    }

    /// The non-`RTS_GENERALS` `REAL_TO_INT_CEIL` / `REAL_TO_INT_FLOOR` (`BaseFunctions.h:81-98`): an
    /// exact integer rounds up one under `fast_float_ceil`, and an exact negative integer rounds down
    /// one under `fast_float_floor`. The box footprint's step count (`AIPathfind.cpp:4194`) depends on
    /// the first: `2 * 60 / 5 = 24` takes 25 steps. The extra sample marks cell (107,33) as an obstacle
    /// of the AmericaCommandCenter footprint; that makes (107,32) pinched, so the dock search's
    /// straight-line pass stops there (`examineCellsCallback`, `AIPathfind.cpp:6147`), as in C++.
    #[test]
    fn real_to_int_rounds_like_the_fast_float_helpers() {
        assert_eq!(real_to_int_ceil(24.0), 25);
        assert_eq!(real_to_int_ceil(10.0), 11);
        assert_eq!(real_to_int_ceil(24.5), 25);
        assert_eq!(real_to_int_floor(-2.0), -3);
        assert_eq!(real_to_int_floor(-2.5), -3);
        assert_eq!(real_to_int_floor(2.7), 2);
        // The edges: zero, negative exact integers and the documented `floor(-1e-8) == 0` quirk.
        assert_eq!(real_to_int_ceil(0.0), 0);
        assert_eq!(real_to_int_ceil(-2.0), -2);
        assert_eq!(real_to_int_ceil(-2.5), -2);
        assert_eq!(real_to_int_floor(0.0), 0);
        assert_eq!(real_to_int_floor(-1e-8), 0);
    }

    /// `costToHierGoal` (AIPathfind.cpp:2079-2090): the rounded Euclidean distance in cost units. The octile
    /// form the port used before gives 15 for (1, 1), where the engine gives 14.
    #[test]
    fn cost_to_hier_goal_rounds_the_euclidean_distance() {
        assert_eq!(cost_to_hier_goal(0, 0), 0);
        assert_eq!(cost_to_hier_goal(1, 0), 10);
        assert_eq!(cost_to_hier_goal(1, 1), 14);
        assert_eq!(cost_to_hier_goal(3, 4), 50);
        assert_eq!(cost_to_hier_goal(10, 10), 141);
    }

    /// The TestHQ footprint (an 80-unit box at angle 0, `Pathfinder::internal_classifyObjectFootprint`
    /// box loop, AIPathfind.cpp:4404-4420 of the Core fork): `REAL_TO_INT_CEIL(2*40/5)` is 17 steps, so
    /// the 17th sample column x = 340 marks cell (34, 24) at the top row. With the plain `ceil` (16 steps)
    /// that cell stays clear, so this fails if the box loop's step count is reverted.
    #[test]
    fn a_box_footprint_takes_the_retail_step_count() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        pf.classify_object_footprint(&t, ObjectId(7), &building(), &pos(300.0, 200.0), 0.0, true);
        assert_eq!(pf.cell_type_at(&pos(345.0, 245.0)), Some(CellType::Obstacle), "the 17th step column");
        assert_eq!(pf.cell_type_at(&pos(355.0, 245.0)), Some(CellType::Clear));
    }

    /// `PathfindLayer::allocateCellsForWallLayer` (AIPathfind.cpp:3567-3577): the pad is
    /// `REAL_TO_INT_FLOOR(lo / cell) - 1` with `PATHFIND_CELL_SIZE/100` the integer 0, so a wall piece with
    /// lo.x = 100 starts at cell 9 (the 0.1 margin of the old port gave 8).
    #[test]
    fn the_wall_layer_pad_has_no_tenth_of_a_cell_margin() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        pf.add_wall_piece(ObjectId(3), &wall_piece(), &pos(200.0, 100.0), 0.0);
        pf.ensure_wall_layer();
        assert_eq!(pf.wall_extent().map(|r| r.0), Some(9));
    }

    #[test]
    fn clip_line_clips_to_the_grid() {
        assert_eq!(clip_line_2d((1, 1), (5, 5), (0, 0, 9, 9)), Some(((1, 1), (5, 5))));
        assert_eq!(clip_line_2d((-5, 5), (5, 5), (0, 0, 9, 9)), Some(((0, 5), (5, 5))));
        assert_eq!(clip_line_2d((-5, -5), (-1, 9), (0, 0, 9, 9)), None);
    }

    #[test]
    fn off_map_start_and_goal_go_straight() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let wps = pf.compute_move_path(&t, &tank_obj(), &pos(-500.0, -500.0), &pos(-100.0, -100.0));
        assert_eq!(wps, vec![pos(-100.0, -100.0)]);
    }

    #[test]
    fn radius_and_center_follow_the_original_table() {
        let r = |radius: f32| PathObject { id: ObjectId::INVALID, surfaces: surface::GROUND, bounding_radius: radius, layer: PathLayer::Ground, player: 0, infantry: false, crusher: false, dozer: false, harvester: false, aircraft_that_adjusts_destination: false, is_aircraft: false, can_path_through_units: false, ignore_obstacle: ObjectId::INVALID, attacking: false, busy: false, using_ability: false }.radius_and_center();
        assert_eq!(r(2.0), (0, true)); // diameter 4 -> 1 cell
        assert_eq!(r(6.0), (1, false)); // diameter 12 -> bumped to 20 -> 2 cells
        assert_eq!(r(15.0), (1, true)); // diameter 30 -> 3 cells
        assert_eq!(r(100.0), (2, true)); // capped
    }

    fn water_strip(x0: i32, x1: i32) -> zr_map::PolygonTrigger {
        zr_map::PolygonTrigger {
            name: "strip".into(),
            layer: String::new(),
            id: 7,
            is_water: true,
            is_river: false,
            river_start: 0,
            points: vec![[x0, -10, 10], [x1, -10, 10], [x1, 700, 10], [x0, 700, 10]],
        }
    }

    #[test]
    fn water_strip_blocks_ground_but_not_hover() {
        let mut t = terrain_with(|_, _| 0);
        t.add_water_area(water_strip(300, 340));
        let mut pf = Pathfinder::new(&t);
        assert_eq!(pf.cell_type_at(&pos(320.0, 200.0)), Some(CellType::Water));
        assert_eq!(pf.cell_type_at(&pos(100.0, 200.0)), Some(CellType::Clear));
        let goal = pos(500.0, 200.0);
        // Ground: the goal is on an unreachable bank, so the path stops on the near side.
        let wps = pf.compute_move_path(&t, &tank_obj(), &pos(100.0, 200.0), &goal);
        assert!(wps.iter().all(|w| w.x < 300.0), "ground unit must not cross: {wps:?}");
        // Hover: ground + water surfaces, crosses straight to the goal.
        let hover = PathObject { id: ObjectId::INVALID, surfaces: surface::GROUND | surface::WATER, bounding_radius: 6.0, layer: PathLayer::Ground, player: 0, infantry: false, crusher: false, dozer: false, harvester: false, aircraft_that_adjusts_destination: false, is_aircraft: false, can_path_through_units: false, ignore_obstacle: ObjectId::INVALID, attacking: false, busy: false, using_ability: false };
        let wps = pf.compute_move_path(&t, &hover, &pos(100.0, 200.0), &goal);
        assert!(wps.last().is_some_and(|w| dist(w, &goal) < 8.0), "{wps:?}");
        assert!(wps.iter().any(|w| w.x > 340.0));
    }

    #[test]
    fn surfaces_mask_selects_cliff_and_water() {
        // `LocomotorTemplate::getValidSurfaces`: a climber walks cliffs, a hover unit ground+water,
        // a tread vehicle ground only (the adapter overrides these from the INI `Surfaces` list).
        let climber = LocomotorParams::from_ini(LocomotorKind::Climber, 30.0, 60.0, 120.0, 120.0);
        assert_eq!(PathObject::surfaces_for(&climber), surface::CLIFF);
        let hover = LocomotorParams::from_ini(LocomotorKind::Hover, 30.0, 60.0, 120.0, 120.0);
        assert_eq!(PathObject::surfaces_for(&hover), surface::GROUND | surface::WATER);
        let treads = LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0);
        assert_eq!(PathObject::surfaces_for(&treads), surface::GROUND);
    }

    #[test]
    fn water_below_the_ground_is_dry() {
        // Ground at sample 100 (= 31 units) is above a 10-unit water surface: no water cells.
        let mut t = terrain_with(|_, _| 100);
        t.add_water_area(water_strip(300, 340));
        let pf = Pathfinder::new(&t);
        assert_eq!(pf.cell_type_at(&pos(320.0, 200.0)), Some(CellType::Clear));
    }

    fn unit(id: u32) -> PathObject {
        PathObject { id: ObjectId(id), surfaces: surface::GROUND, bounding_radius: 8.0, layer: PathLayer::Ground, player: 0, infantry: false, crusher: false, dozer: false, harvester: false, aircraft_that_adjusts_destination: false, is_aircraft: false, can_path_through_units: false, ignore_obstacle: ObjectId::INVALID, attacking: false, busy: false, using_ability: false }
    }

    /// `adjustToPossibleDestination` (AIPathfind.cpp:5672): a destination whose zone matches the start and whose footprint
    /// is clear is returned as given (the first `checkDestination`, no `checkForPossible` write). Revert check: none
    /// needed beyond the second test; this pins the early return.
    #[test]
    fn adjust_to_possible_destination_keeps_a_pathable_destination() {
        let t = terrain_with(|_, _| 0);
        let pf = Pathfinder::new(&t);
        let mut dest = pos(300.0, 300.0);
        assert!(pf.adjust_to_possible_destination(&t, &unit(1), &pos(300.0, 300.0), &mut dest, &|_| None));
        assert_eq!(dest, pos(300.0, 300.0));
    }

    /// `adjustToPossibleDestination`: a destination on a building's footprint is moved to a legal cell of the start's
    /// zone (`checkForPossible` writes the candidate centre, `checkDestination` accepts it). Revert check: make
    /// `check_for_possible_unit` skip its `*dest = ...` write (or return false) and the destination stays inside the
    /// building, so the test fails.
    #[test]
    fn adjust_to_possible_destination_moves_a_blocked_destination_to_a_pathable_cell() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        pf.classify_object_footprint(&t, ObjectId(7), &building(), &pos(300.0, 300.0), 0.0, true);
        assert!(!pf.can_place(surface::GROUND, &pos(300.0, 300.0)), "the footprint is blocked");
        let mut dest = pos(300.0, 300.0);
        assert!(pf.adjust_to_possible_destination(&t, &unit(1), &pos(100.0, 100.0), &mut dest, &|_| None));
        assert!(pf.can_place(surface::GROUND, &dest), "a legal spot: {dest:?}");
        assert!(dest.dist_2d(&pos(300.0, 300.0)) > 1.0, "the destination moved: {dest:?}");
    }

    /// `snapClosestGoalPosition` (AIPathfind.cpp:5235): when the unit's own cell is another ally's goal, the snap takes the
    /// first passing cell of the 3x3 around it. Revert check: return the own cell's centre at once (skip the 3x3 loop)
    /// and the snapped goal stays on the held cell.
    #[test]
    fn a_snapped_goal_moves_off_a_cell_an_ally_holds() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        // A small unit (radius 0, one cell): an 8-unit unit's two-cell footprint overlaps every neighbour's, and
        // `snapClosestGoalPosition` keeps the own cell for a unit with a radius (AIPathfind.cpp:5282).
        let mut holder = unit(1);
        holder.bounding_radius = 0.5;
        let mut other = unit(2);
        other.bounding_radius = 0.5;
        let pos_held = pos(205.0, 205.0);
        pf.update_goal(&holder, &pos_held);
        let held = pf.goal_position(&t, holder.id).expect("the holder's goal cell");
        let snapped = pf.snap_closest_goal_position(&t, &other, &pos_held);
        assert!(snapped.dist_2d(&held) > 5.0, "the snap left the held cell {held:?}: {snapped:?}");
    }

    #[test]
    fn adjust_destination_spreads_goals_and_reserves_them() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let dest = pos(300.0, 300.0);
        let a = unit(1);
        let b = unit(2);
        // `adjustDestination` only reads the goal cells; the unit's `updateGoal` (AIStates.cpp:1667) takes the cell.
        let pa = pf.adjust_destination(&t, &a, &dest, None).unwrap();
        pf.update_goal(&a, &pa);
        let pb = pf.adjust_destination(&t, &b, &dest, None).unwrap();
        pf.update_goal(&b, &pb);
        assert!(pa.dist_2d(&pb) > 15.0, "goals must not stack: {pa:?} / {pb:?}");
        // Re-adjusting a unit keeps its own goal cell (its own goal is never a blocker, `checkDestination`).
        assert_eq!(pf.adjust_destination(&t, &a, &dest, None).unwrap(), pa);
        // Releasing a's goal (`removeUnitFromPathfindMap`, AIPathfind.cpp:10185) lets b keep its cell, and a fresh unit
        // can take the center.
        pf.clear_unit(a.id);
        let c = unit(3);
        let pc = pf.adjust_destination(&t, &c, &dest, None).unwrap();
        pf.update_goal(&c, &pc);
        assert!(pc.dist_2d(&pb) > 15.0, "{pc:?} / {pb:?}");
    }

    /// `validMovementPosition(isCrusher, ...)` (`AIPathfind.cpp:4985-5006`): `isCrusher &&
    /// toCell->isObstacleFence()` passes — a crusher may enter a *fence* cell, not a structure cell
    /// (the port's `checkPathCost` used to let a crusher into every obstacle).
    #[test]
    fn a_crusher_may_enter_a_fence_cell_but_not_a_structure_cell() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let mut fence = building();
        fence.fence_width = 40.0;
        pf.classify_object_footprint(&t, ObjectId(9), &fence, &pos(200.0, 200.0), 0.0, true);
        pf.classify_object_footprint(&t, ObjectId(7), &building(), &pos(400.0, 200.0), 0.0, true);
        let (fence_cell, structure_cell) = (pos(200.0, 200.0), pos(400.0, 200.0));
        assert_eq!(pf.cell_type_at(&fence_cell), Some(CellType::Obstacle));
        assert_eq!(pf.cell_type_at(&structure_cell), Some(CellType::Obstacle));
        assert!(!pf.valid_movement_position(surface::GROUND, &fence_cell, false), "not for a non-crusher");
        assert!(pf.valid_movement_position(surface::GROUND, &fence_cell, true), "a crusher drives over a fence");
        assert!(!pf.valid_movement_position(surface::GROUND, &structure_cell, true), "but not into a structure");
    }

    #[test]
    fn a_goal_inside_a_structure_is_judged_by_the_terrain_under_it() {
        // `clientSafeQuickDoesPathExist` (`AIPathfind.cpp:8239-8290`): an obstacle cell is only a
        // movement position when it is the *pathfinder's* ignored obstacle, and then the zone test uses
        // the terrain zone under the structure (`doingTerrainZone`, `:8266-8279`).
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        pf.classify_object_footprint(&t, ObjectId(7), &building(), &pos(300.0, 200.0), 0.0, true);
        let mut obj = unit(1);
        let from = pos(100.0, 200.0);
        let inside = pos(300.0, 200.0);
        assert_eq!(pf.cell_type_at(&inside), Some(CellType::Obstacle));
        assert!(!pf.quick_does_path_exist(&obj, &from, &inside), "an obstacle cell is not a movement position");
        // Ignoring the structure lets the cell through. Its cells are UNINITIALIZED_ZONE until the
        // next calculation (a frame-0 footprint change is due at frame 2, `markZonesDirty`), so the
        // terrain-zone remap of zone 0 is not the ground's zone yet.
        obj.ignore_obstacle = ObjectId(7);
        pf.set_ignore_obstacle_id(ObjectId(7));
        assert!(!pf.quick_does_path_exist(&obj, &from, &inside), "the placed structure has no zone yet");
        pf.calculate_zones();
        assert!(pf.quick_does_path_exist(&obj, &from, &inside), "after the calculation the terrain under it is connected");
        pf.set_ignore_obstacle_id(ObjectId::INVALID);

        // A unit standing *inside* a structure has no zone of its own for its locomotor; the engine
        // compares the terrain zone under it. A cliff between it and the goal still stops it — the port's
        // old `z1 == 0 || z2 == 0` shortcut answered "connected" here (LOG lane 63/74).
        let t = terrain_with(|x, _| if x >= 30 { 32 } else { 0 }); // cliff column 29
        let mut pf = Pathfinder::new(&t);
        pf.classify_object_footprint(&t, ObjectId(7), &building(), &pos(400.0, 100.0), 0.0, true);
        pf.calculate_zones();
        let obj = unit(1);
        assert_eq!(pf.cell_type_at(&pos(400.0, 100.0)), Some(CellType::Obstacle));
        assert!(
            !pf.quick_does_path_exist(&obj, &pos(400.0, 100.0), &pos(100.0, 100.0)),
            "the far side of the cliff is a different terrain zone"
        );
        assert!(
            pf.quick_does_path_exist(&obj, &pos(400.0, 100.0), &pos(500.0, 100.0)),
            "its own side of the cliff is the same terrain zone"
        );
    }

    /// `updateZonesForModify` (`AIPathfind.cpp:2998-3120`, called from the footprint arms
    /// `:4541-4548`) gives each touched `UNINITIALIZED_ZONE` cell a same-type neighbour's zone at
    /// once, so a structure that is *removed* leaves its cells connected to the ground before the
    /// next `calculateZones`.
    #[test]
    fn a_removed_structure_rezones_its_cells_from_the_ground() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        pf.classify_object_footprint(&t, ObjectId(7), &building(), &pos(300.0, 200.0), 0.0, true);
        pf.calculate_zones(); // the structure's cells now carry the structure's own zones
        let obj = unit(1);
        let cell = pos(300.0, 200.0);
        let from = pos(100.0, 200.0);
        assert_eq!(pf.cell_type_at(&cell), Some(CellType::Obstacle));
        assert!(!pf.quick_does_path_exist(&obj, &from, &cell), "the structure is not the ground's zone");
        pf.classify_object_footprint(&t, ObjectId(7), &building(), &pos(300.0, 200.0), 0.0, false);
        assert_eq!(pf.cell_type_at(&cell), Some(CellType::Clear));
        // No `calculate_zones` in between: `updateZonesForModify` already gave the cells the ground's
        // zone from their same-type neighbours.
        assert!(pf.quick_does_path_exist(&obj, &from, &cell), "the cleared cells took the ground's zone");
    }

    /// The quick path's `UNINITIALIZED_ZONE` arm (`#if !(RTS_GENERALS && RETAIL_COMPATIBLE_PATHFINDING)`,
    /// `AIPathfind.cpp:8317-8323`): a unit standing inside a structure that was just placed has no
    /// zone yet, and the engine answers "connected" — "better to return a false positive than a
    /// false negative" — instead of comparing a zone 0 with the goal's.
    #[test]
    fn a_unit_inside_a_just_placed_structure_returns_a_false_positive() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        pf.classify_object_footprint(&t, ObjectId(7), &building(), &pos(300.0, 200.0), 0.0, true);
        let obj = unit(1);
        let inside = pos(300.0, 200.0);
        assert_eq!(pf.cell_type_at(&inside), Some(CellType::Obstacle));
        assert!(
            pf.quick_does_path_exist(&obj, &inside, &pos(100.0, 200.0)),
            "an UNINITIALIZED parent zone answers true"
        );
        // The same frame with the zones calculated: the unit's zone is the structure's own zone,
        // which the terrain remap turns into the ground under it, so land is connected and a cliff
        // cell on the far side is not.
        pf.calculate_zones();
        assert!(
            pf.quick_does_path_exist(&obj, &inside, &pos(100.0, 200.0)),
            "the terrain under the structure is the same ground zone"
        );
        let t2 = terrain_with(|x, _| if x >= 30 { 32 } else { 0 });
        let mut pf = Pathfinder::new(&t2);
        pf.classify_object_footprint(&t2, ObjectId(7), &building(), &pos(400.0, 100.0), 0.0, true);
        pf.calculate_zones();
        assert!(
            !pf.quick_does_path_exist(&obj, &pos(400.0, 100.0), &pos(100.0, 100.0)),
            "a cliff in between is a different zone"
        );
    }

    #[test]
    fn a_move_order_onto_a_structure_adjusts_to_a_cell_beside_it() {
        // The AN3 seed 1 GLA worker's dock order (LOG lane 63/74): its AI ignores the dock warehouse, so
        // `checkDestination` lets the warehouse's cells through (`AIPathfind.cpp:5023`), but the adjust's
        // path gate runs with the *pathfinder's* field unset (`AIStates.cpp:1661-1664` before
        // `AIUpdate.cpp:1722`) and `validMovementPosition` rejects them (`AIPathfind.cpp:4993`).
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let warehouse = ObjectId(7);
        pf.classify_object_footprint(&t, warehouse, &building(), &pos(300.0, 200.0), 0.0, true);
        let mut obj = unit(1);
        obj.ignore_obstacle = warehouse; // `AIUpdateInterface::getIgnoredObstacleID()`
        obj.bounding_radius = 6.0; // footprint radius 1, not centred: the worker's shape
        let from = pos(100.0, 200.0);
        pf.update_unit_pos(&obj, &from, false);
        let path = pf
            .compute_ai_path(&t, &obj, &from, &pos(300.0, 200.0), None, false, None, warehouse, true)
            .expect("a path beside the structure");
        let last = *path.waypoints().last().unwrap();
        assert!(
            dist(&last, &pos(300.0, 200.0)) > 25.0,
            "the adjust stepped out of the warehouse instead of keeping its cell: {last:?}"
        );
        // The port used to set the pathfinder's field before the adjust; then the *same* request keeps the
        // warehouse cell, because `validMovementPosition` accepts an obstacle it is told to ignore (`:4993`).
        pf.set_ignore_obstacle_id(warehouse);
        let kept = pf.adjust_destination(&t, &obj, &pos(300.0, 200.0), None).expect("accepted");
        pf.set_ignore_obstacle_id(ObjectId::INVALID);
        assert_eq!(kept, pos(300.5, 200.5), "with the field set, the warehouse cell is a movement position");
    }

    /// `compute_ai_path` is `AIUpdateInterface::computePath` -> `pathServices->findPath`
    /// (`AIUpdate.cpp:1726`), which first clears the stale corridor and rebuilds it
    /// (`AIPathfind.cpp:6455-6461`) — it must not run the fine A* inside whatever corridor the
    /// previous request left. That stale corridor was the AN3 s1 worker-246 divergence (f2028: the
    /// port's fine path took the 9-node diagonal instead of the C++ 15-node route). On bare ground no
    /// cell carries a `PathfindCellInfo`, so the coarse search fails and `findPath` lifts the
    /// corridor (`setAllPassable`): after the request every block is passable.
    /// Revert check: replacing the corridor arm with `internal_find_path` leaves the one-block
    /// corridor in place and both `block_passable_at` assertions fail.
    #[test]
    fn a_compute_path_rebuilds_the_corridor_instead_of_using_the_last_requests() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let mover = tank_obj();
        pf.zones.clear_passable_flags();
        pf.zones.set_passable(300, 300, true); // a stale corridor around block (30,30)
        assert!(!pf.block_passable_at(&pos(100.0, 100.0)), "block (1,1) starts outside it");
        let path = pf.compute_ai_path(&t, &mover, &pos(100.0, 100.0), &pos(500.0, 400.0), None, false, None, ObjectId::INVALID, false);
        assert!(path.is_some(), "the fine A* still finds a route");
        assert!(pf.block_passable_at(&pos(100.0, 100.0)), "the failed coarse search lifted the corridor");
        assert!(pf.block_passable_at(&pos(500.0, 400.0)));
    }

    #[test]
    fn a_units_own_ignored_obstacle_is_a_valid_adjust_cell() {
        // The AN3 seed 1 dozer's dock order (LOG lane 74): the stash is the unit's ignored obstacle, so a
        // footprint that overlaps one of its cells still passes `checkDestination` (`AIPathfind.cpp:5023`).
        // That ignore id is the unit's, not the pathfinder's search-scoped one (`:4993`).
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let stash = ObjectId(7);
        // A 30x30 stash at (280,200): cells 26..29 x 18..21.
        let mut stash_box = building();
        stash_box.geometry = Geometry { shape: GeometryShape::Box, major_radius: 15.0, minor_radius: 15.0, height: 20.0, is_small: false };
        pf.classify_object_footprint(&t, stash, &stash_box, &pos(280.0, 200.0), 0.0, true);
        assert_eq!(pf.cell_type_at(&pos(295.0, 200.0)), Some(CellType::Obstacle));
        // The goal cell (30,20) is clear, but a radius-1 footprint there reaches the stash cell (29,20).
        let dest = pos(300.0, 200.0);
        let mut obj = unit(1);
        obj.bounding_radius = 6.0;
        obj.ignore_obstacle = stash;
        let adj = pf.adjust_destination(&t, &obj, &dest, None).expect("adjust accepted");
        assert_eq!(adj, pos(300.5, 200.5), "the centre cell (30,20) is kept");
        // Without the unit's ignore id the same cell is rejected and the spiral moves out of the stash.
        let mut other = obj;
        other.id = ObjectId(2);
        other.ignore_obstacle = ObjectId::INVALID;
        let adj = pf.adjust_destination(&t, &other, &dest, None).expect("a cell outside the stash");
        assert_ne!(adj, pos(300.5, 200.5), "a footprint over another unit's obstacle is not a destination");
    }

    #[test]
    fn placement_rejects_water_cliff_and_obstacle() {
        let mut t = terrain_with(|x, _| if x >= 30 { 32 } else { 0 }); // cliff column 29
        t.add_water_area(water_strip(200, 240));
        let mut pf = Pathfinder::new(&t);
        pf.classify_object_footprint(&t, ObjectId(7), &building(), &pos(400.0, 200.0), 0.0, true);
        assert!(!pf.can_place(surface::GROUND, &pos(295.0, 100.0)), "cliff");
        assert!(!pf.can_place(surface::GROUND, &pos(220.0, 100.0)), "water");
        assert!(!pf.can_place(surface::GROUND, &pos(400.0, 200.0)), "obstacle");
        assert!(pf.can_place(surface::GROUND, &pos(100.0, 300.0)), "clear ground");
        // A hover unit may sit on the water.
        assert!(pf.can_place(surface::GROUND | surface::WATER, &pos(220.0, 100.0)));
    }

    // ---- unit avoidance (`checkForMovement`) ------------------------------------------------

    #[test]
    fn check_for_movement_sees_enemy_and_ally_fixed_units() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let mover = PathObject { id: ObjectId(1), surfaces: surface::GROUND, bounding_radius: 6.0, layer: PathLayer::Ground, player: 0, infantry: false, crusher: false, dozer: false, harvester: false, aircraft_that_adjusts_destination: false, is_aircraft: false, can_path_through_units: false, ignore_obstacle: ObjectId::INVALID, attacking: false, busy: false, using_ability: false };
        let mut other = mover;
        other.id = ObjectId(2);
        other.player = 1;
        // A stationary unit on its own goal cell is `UNIT_PRESENT_FIXED` (`setGoalUnit` after
        // `updateGoal`, `AIPathfind.cpp:1420-1455`, `:9851-9894`), which is what an idle unit that
        // finished an adjusting move keeps.
        pf.update_unit_pos(&other, &pos(300.0, 300.0), false);
        pf.update_goal(&other, &pos(300.0, 300.0));
        // A fixed unit of an unrelated player blocks as an enemy.
        let info = pf.check_for_movement_full(&mover, &pos(300.0, 300.0), false);
        assert!(info.enemy_fixed);
        assert_eq!(info.ally_fixed_count, 0);
        // Once allied it counts as an ally, not an enemy.
        let mut allies = [0u32; 32];
        allies[0] |= 1 << 1;
        allies[1] |= 1 << 0;
        pf.set_allies(allies);
        assert!(pf.are_allies(0, 1) && pf.are_allies(1, 0));
        let info = pf.check_for_movement_full(&mover, &pos(300.0, 300.0), false);
        assert!(!info.enemy_fixed);
        assert_eq!(info.ally_fixed_count, 1);
        // `if (!canCrush) info.enemyFixed = TRUE` (AIPathfind.cpp:5201-5205), `canCrush = obj->canCrushOrSquish(unit,
        // TEST_CRUSH_OR_SQUISH)` (Object.cpp:1121-1200). The enemy branch is reached only once the players are not allied.
        // A crusher level 2 against a crushable-0 victim crushes it (not enemy-fixed); against a crushable-2 victim it
        // does not (enemy-fixed); a squishable victim is crushed whatever its level (lane 191 R27 F4, revert: the old
        // `!obj.crusher` test fails the second case).
        pf.set_allies([0u32; 32]);
        let mut crusher = mover;
        crusher.crusher = true;
        let mover_info = crate::modules2::CrushInfo { crusher_level: 2, crushable_level: 0, squish_collide: false };
        pf.set_unit_crush([(1, mover_info, false), (2, crate::modules2::CrushInfo { crusher_level: 0, crushable_level: 0, squish_collide: false }, false)]);
        assert!(!pf.check_for_movement_full(&crusher, &pos(300.0, 300.0), false).enemy_fixed, "a crusher crushes a crushable-0 enemy");
        pf.set_unit_crush([(1, mover_info, false), (2, crate::modules2::CrushInfo { crusher_level: 0, crushable_level: 2, squish_collide: false }, false)]);
        assert!(pf.check_for_movement_full(&crusher, &pos(300.0, 300.0), false).enemy_fixed, "a level-2 crusher cannot crush a crushable-2 enemy");
        pf.set_unit_crush([(1, mover_info, false), (2, crate::modules2::CrushInfo { crusher_level: 0, crushable_level: 2, squish_collide: true }, false)]);
        assert!(!pf.check_for_movement_full(&crusher, &pos(300.0, 300.0), false).enemy_fixed, "a squishable enemy is crushed");
        // Back to the allied case for the rest of the test.
        let mut allies = [0u32; 32];
        allies[0] |= 1 << 1;
        allies[1] |= 1 << 0;
        pf.set_allies(allies);
        // A unit away from its goal is `UNIT_PRESENT_MOVING`: transient, so the A* (`considerTransient`
        // false) only reports the ally, while a caller that considers transient units counts it fixed.
        pf.update_goal(&other, &pos(500.0, 500.0));
        let info = pf.check_for_movement_full(&mover, &pos(300.0, 300.0), false);
        assert!(info.ally_moving && info.ally_fixed_count == 0, "{info:?}");
        let info = pf.check_for_movement_full(&mover, &pos(300.0, 300.0), true);
        assert!(info.ally_moving && info.ally_fixed_count == 1, "{info:?}");
    }

    #[test]
    fn infantry_moves_through_infantry() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let inf_a = PathObject { id: ObjectId(1), surfaces: surface::GROUND, bounding_radius: 6.0, layer: PathLayer::Ground, player: 0, infantry: true, crusher: false, dozer: false, harvester: false, aircraft_that_adjusts_destination: false, is_aircraft: false, can_path_through_units: false, ignore_obstacle: ObjectId::INVALID, attacking: false, busy: false, using_ability: false };
        let mut inf_b = inf_a;
        inf_b.id = ObjectId(2);
        inf_b.player = 1;
        pf.update_unit_pos(&inf_b, &pos(300.0, 300.0), false);
        pf.update_goal(&inf_b, &pos(300.0, 300.0));
        let info = pf.check_for_movement_full(&inf_a, &pos(300.0, 300.0), false);
        assert!(!info.enemy_fixed && info.ally_fixed_count == 0, "{info:?}");
        // A vehicle is still blocked by the fixed enemy infantry.
        let mut tank = inf_a;
        tank.infantry = false;
        assert!(pf.check_for_movement_full(&tank, &pos(300.0, 300.0), false).enemy_fixed);
    }

    /// `moveAllies` walks the path from its **last** node down to, excluding, the **first**
    /// (`AIPathfind.cpp:10347`: `for (node = path->getLastNode(); node && node != path->getFirstNode();
    /// node = node->getPrevious())`). An ally standing on the destination node is pushed; one on the
    /// start node is not.
    #[test]
    fn move_allies_scans_from_the_last_node_down_to_the_first() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let mut allies = [0u32; 32];
        allies[0] |= 1 << 0;
        pf.set_allies(allies);
        let mover = unit(1);
        let at_goal = unit(2);
        let at_start = unit(3);
        pf.update_unit_pos(&at_goal, &pos(500.0, 100.0), false);
        pf.update_unit_pos(&at_start, &pos(100.0, 100.0), false);
        let path = Path {
            nodes: vec![
                PathNode { pos: pos(100.0, 100.0), can_optimize: true, next_optimized: Some(1) },
                PathNode { pos: pos(300.0, 100.0), can_optimize: true, next_optimized: Some(2) },
                PathNode { pos: pos(500.0, 100.0), can_optimize: true, next_optimized: None },
            ],
            blocked_by_ally: true,
        };
        // One visit per cell of the unit's square around each scanned node (`aiMoveAwayFromUnit` runs per visit).
        let visits = pf.ally_scan(&mover, &path);
        assert!(!visits.is_empty() && visits.iter().all(|v| *v == ObjectId(2)), "the destination node is scanned: {visits:?}");
    }

    /// The ZH vetoes of `moveAllies` (`#if !(RTS_GENERALS && RETAIL_COMPATIBLE_PATHFINDING)`,
    /// `AIPathfind.cpp:10267-10275`): an attacking ally, one using an ability and a busy one are
    /// left alone. The ignore test is the *mover's* `getIgnoredObstacleID()` (`:10342-10345`), not
    /// the pathfinder's search-scoped field.
    #[test]
    fn move_allies_keeps_attacking_busy_and_ability_using_allies_in_place() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let mut allies = [0u32; 32];
        allies[0] |= 1 << 0;
        pf.set_allies(allies);
        let mover = unit(1);
        let plain = unit(2);
        let mut attacking = unit(3);
        attacking.attacking = true;
        let mut busy = unit(4);
        busy.busy = true;
        let mut ability = unit(5);
        ability.using_ability = true;
        pf.update_unit_pos(&plain, &pos(200.0, 100.0), false);
        pf.update_unit_pos(&attacking, &pos(300.0, 100.0), false);
        pf.update_unit_pos(&busy, &pos(400.0, 100.0), false);
        pf.update_unit_pos(&ability, &pos(500.0, 100.0), false);
        let path = Path {
            nodes: vec![
                PathNode { pos: pos(100.0, 100.0), can_optimize: true, next_optimized: Some(1) },
                PathNode { pos: pos(200.0, 100.0), can_optimize: true, next_optimized: Some(2) },
                PathNode { pos: pos(300.0, 100.0), can_optimize: true, next_optimized: Some(3) },
                PathNode { pos: pos(400.0, 100.0), can_optimize: true, next_optimized: Some(4) },
                PathNode { pos: pos(500.0, 100.0), can_optimize: true, next_optimized: None },
            ],
            blocked_by_ally: true,
        };
        // The scan lists every visit, last node first; the vetoes are applied per visit by `GameLogic::move_allies`.
        // Per cell, last node first: each unit's visits are contiguous and in node order (the vetoes are GameLogic's).
        let mut visits = pf.ally_scan(&mover, &path);
        visits.dedup();
        assert_eq!(visits, vec![ObjectId(5), ObjectId(4), ObjectId(3), ObjectId(2)], "the units in the engine's order");

        // The mover's ignored obstacle is not a visit even though the pathfinder's own field is clear.
        let mut mover = mover;
        mover.ignore_obstacle = ObjectId(2);
        pf.set_ignore_obstacle_id(ObjectId::INVALID);
        let visits = pf.ally_scan(&mover, &path);
        assert!(!visits.contains(&ObjectId(2)), "the mover's ignored obstacle is not a visit");
    }

    /// The fine A* pays `100*COST_ORTHOGONAL` for a ground cell outside the hierarchical corridor
    /// (`AIPathfind.cpp:6310-6314`, `:6445-6447`), which is what makes the corridor steer the route
    /// once the straight-line pre-pass has bailed at the first non-passable cell (`:6167`).
    /// `getMoveAwayFromPath`/`patchPath` lift it again with `setAllPassable` (`:10433`, `:10619`).
    #[test]
    fn a_fine_search_follows_the_corridor_when_it_is_cheaper() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let unit = tank_obj();
        let from = pos(105.0, 105.0);
        let to = pos(505.0, 505.0);
        // An L-shaped corridor: down block column x=1 (cells 10..19), then right along block row
        // y=5 (cells y 50..59). The start cell (10,10) is in block (1,1), the goal (50,50) in (5,5).
        pf.zones.clear_passable_flags();
        for by in 1..=5 {
            pf.zones.set_passable(10, by * 10, true);
        }
        for bx in 1..=5 {
            pf.zones.set_passable(bx * 10, 50, true);
        }
        // The raw A* route (before `optimize` straightens the line of sight, which knows nothing
        // about the corridor) follows the corridor: down first, then right.
        let down_then_right = |p: &Path| p.nodes.iter().any(|n| n.pos.x < 300.0 && n.pos.y > 450.0);
        let path = pf.internal_find_path(&t, &unit, &from, &to).expect("a corridor path");
        assert!(
            down_then_right(&path),
            "the route runs down the corridor instead of straight across: {:?}",
            path.nodes.iter().map(|n| (n.pos.x, n.pos.y)).collect::<Vec<_>>()
        );
        // With every block passable the direct diagonal wins (`setAllPassable`'s effect).
        pf.zones.set_all_passable();
        let direct = pf.internal_find_path(&t, &unit, &from, &to).expect("a direct path");
        assert!(!down_then_right(&direct), "the lifted corridor gives the straight diagonal: {:?}", direct.nodes.iter().map(|n| (n.pos.x, n.pos.y)).collect::<Vec<_>>());
    }

    /// `getMoveAwayFromPath` checks `pathToAvoid2` as well as `pathToAvoid` (`AIPathfind.cpp:10544-10556`):
    /// the path of the unit this one stepped aside from *last* time is avoided too. With no second
    /// path a non-centred unit accepts its own cell (the engine's "we didn't move" test compares the
    /// half-cell-offset start cell with the popped cell); a second path through that cell pushes the
    /// goal one cell further.
    #[test]
    fn the_second_avoid_path_is_checked_too() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let mover = unit(1);
        let from = pos(300.0, 100.0);
        let through = [pos(300.0, 100.0), pos(320.0, 100.0)];
        let first = pf.move_away_from_unit(&t, &mover, &from, &[], &[], 0, false).expect("a step-aside");
        let g1 = *first.waypoints().last().unwrap();
        let second = pf.move_away_from_unit(&t, &mover, &from, &[], &through, 0, false).expect("a step-aside");
        let g2 = *second.waypoints().last().unwrap();
        assert!(g2.dist_2d(&from) > g1.dist_2d(&from) + 5.0, "the second avoid path moved the goal: {g1:?} -> {g2:?}");
    }

    /// `getMoveAwayFromPath` and `patchPath` both lift the corridor with
    /// `m_zoneManager.setAllPassable()` (`AIPathfind.cpp:10433`, `:10619`), so the block flags a
    /// previous `findPath` marked do not restrict either search.
    #[test]
    fn the_move_away_and_patch_searches_lift_the_corridor() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let unit = unit(1);
        // One passable block, everything else cleared, as a `findPath` corridor leaves the flags.
        let off_corridor = |pf: &Pathfinder| pf.block_passable_at(&pos(100.0, 500.0));
        pf.zones.clear_passable_flags();
        pf.zones.set_passable(300, 100, true);
        assert!(!off_corridor(&pf), "the block is outside the corridor");
        pf.move_away_from_unit(&t, &unit, &pos(300.0, 300.0), &[pos(900.0, 900.0), pos(900.0, 920.0)], &[], 0, false);
        assert!(off_corridor(&pf), "the move-away search lifts the corridor");
        // The same for the stitch search.
        pf.zones.clear_passable_flags();
        pf.zones.set_passable(300, 100, true);
        let original = Path {
            nodes: vec![
                PathNode { pos: pos(300.0, 300.0), can_optimize: true, next_optimized: Some(1) },
                PathNode { pos: pos(500.0, 300.0), can_optimize: true, next_optimized: None },
            ],
            blocked_by_ally: false,
        };
        pf.patch_path(&t, &unit, &pos(100.0, 300.0), &original, true);
        assert!(off_corridor(&pf), "the patch search lifts the corridor");
    }

    // ---- walls (`LAYER_WALL`) ---------------------------------------------------------------

    /// A wall piece spanning x 100..300, y 80..120: four cell rows, of which the inner two stay
    /// clear after the one-cell edge tightening (a two-cell wall is entirely edge, as in the
    /// engine).
    fn wall_piece() -> Geometry {
        Geometry { shape: GeometryShape::Box, major_radius: 100.0, minor_radius: 20.0, height: 40.0, is_small: false }
    }

    fn wall_unit(_x: f32, _y: f32, _z: f32) -> PathObject {
        PathObject { id: ObjectId::INVALID, surfaces: surface::GROUND, bounding_radius: 6.0, layer: PathLayer::Wall, player: 0, infantry: false, crusher: false, dozer: false, harvester: false, aircraft_that_adjusts_destination: false, is_aircraft: false, can_path_through_units: false, ignore_obstacle: ObjectId::INVALID, attacking: false, busy: false, using_ability: false }
    }

    #[test]
    fn wall_piece_classifies_a_walkable_layer() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        pf.set_wall_height(30.0);
        assert!(pf.wall_extent().is_none());
        pf.add_wall_piece(ObjectId(3), &wall_piece(), &pos(200.0, 100.0), 0.0);
        pf.ensure_wall_layer();
        assert_eq!(pf.wall_piece_count(), 1);
        assert!(pf.is_point_on_wall(&pos(200.0, 100.0)));
        assert!(pf.is_point_on_wall(&pos(100.0, 90.0)));
        assert!(!pf.is_point_on_wall(&pos(200.0, 130.0)));
        // The layer covers the wall plus a one-cell pad.
        let (x0, y0, x1, y1) = pf.wall_extent().unwrap();
        assert!(x0 <= 9 && x1 >= 30 && y0 <= 8 && y1 >= 11, "{:?}", (x0, y0, x1, y1));
        // Cells y = 9, 10 are fully covered (clear); everything else is not walkable.
        assert_eq!(pf.layer_for_destination(&t, &Coord3D::new(200.0, 95.0, 31.0)), PathLayer::Wall);
        assert_eq!(pf.layer_for_destination(&t, &Coord3D::new(200.0, 95.0, 0.0)), PathLayer::Ground);

        // A wall unit paths straight along the wall at the wall height.
        let wps = pf.compute_move_path(&t, &wall_unit(150.0, 100.0, 30.0), &Coord3D::new(150.0, 100.0, 30.0), &Coord3D::new(250.0, 100.0, 30.0));
        assert!(!wps.is_empty());
        assert!(dist(wps.last().unwrap(), &Coord3D::new(250.0, 100.0, 30.0)) < 8.0, "{wps:?}");
        for w in &wps {
            assert!((w.z - 30.0).abs() < 0.01, "waypoint on the wall height: {w:?}");
            assert!(pf.is_point_on_wall(w), "waypoint stays on the wall: {w:?}");
        }

        // Removing the piece disables the layer.
        pf.remove_wall_piece(ObjectId(3));
        pf.ensure_wall_layer();
        assert_eq!(pf.wall_piece_count(), 0);
        assert!(pf.wall_extent().is_none());
        assert!(!pf.is_point_on_wall(&pos(200.0, 100.0)));
    }

    #[test]
    fn wall_is_an_obstacle_to_ground_units() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        pf.set_wall_height(30.0);
        // The wall piece is also a structure on the ground grid (`classifyObjectFootprint`).
        let mut geom = wall_piece();
        geom.height = 40.0;
        let mut tmpl = ObjectTemplate::new("WallPiece");
        tmpl.kind_of = kind_of::STRUCTURE | kind_of::WALK_ON_TOP_OF_WALL;
        tmpl.geometry = geom;
        pf.add_wall_piece(ObjectId(3), &geom, &pos(200.0, 100.0), 0.0);
        pf.classify_object_footprint(&t, ObjectId(3), &tmpl, &pos(200.0, 100.0), 0.0, true);
        assert_eq!(pf.cell_type_at(&pos(200.0, 100.0)), Some(CellType::Obstacle));
        // A ground unit going from below to above must go around the wall (or stop short of it).
        let from = pos(200.0, 40.0);
        let to = pos(200.0, 160.0);
        let wps = pf.compute_move_path(&t, &tank_obj(), &from, &to);
        assert!(!wps.is_empty());
        let mut prev = from;
        for w in &wps {
            let steps = 40;
            for s in 0..=steps {
                let f = s as f32 / steps as f32;
                let (x, y) = (prev.x + (w.x - prev.x) * f, prev.y + (w.y - prev.y) * f);
                assert!(!(x > 110.0 && x < 290.0 && y > 80.0 && y < 120.0), "({x},{y}) inside the wall");
            }
            prev = *w;
        }
    }

    // ---- the zone-calculation schedule (`PathfindZoneManager::markZonesDirty`) -----------------

    /// `Pathfinder::newMap` -> `classifyMap` -> `calculateZones` (`AIPathfind.cpp:4726`, :4814, :2991) leaves no
    /// calculation due. The frame-2 calculation of a stocked map comes only from newMap's frame-0 object loop
    /// (:4729-4732), whose box/sphere/cylinder footprints call `markZonesDirty` (:4389, :4463). A map with no such
    /// footprint never gates the path queue at frame 2.
    #[test]
    fn a_structure_less_map_does_not_schedule_a_zone_calculation() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        pf.set_frame(2);
        assert!(!pf.zones_need_calculation(), "newMap's classifyMap computed the zones; nothing is due");
        // A structure footprint at frame 0 is what schedules frame 2.
        pf.set_frame(0);
        pf.classify_object_footprint(&t, ObjectId(7), &building(), &pos(300.0, 200.0), 0.0, true);
        pf.set_frame(2);
        assert!(pf.zones_need_calculation(), "the frame-0 footprint mark schedules frame 2");
        pf.calculate_zones();
        assert!(!pf.zones_need_calculation(), "calculateZones clears the schedule");
    }

    /// `classifyFence` marks the zones dirty only when a cell's obstacle state changed (`didAnything`,
    /// AIPathfind.cpp:4247-4274 uses `if (didAnything)`); re-classifying a fence into cells it already occupies
    /// schedules nothing. (The unconditional mark at :4175 belongs to the `RTS_GENERALS &&
    /// RETAIL_COMPATIBLE_PATHFINDING` branch, which this build does not compile.)
    #[test]
    fn a_fence_with_no_cell_change_does_not_schedule_a_zone_calculation() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let mut fence = building();
        fence.fence_width = 40.0;
        // The first insert changes cells, so it schedules; the removal changes them back.
        pf.classify_object_footprint(&t, ObjectId(9), &fence, &pos(300.0, 200.0), 0.0, true);
        pf.set_frame(2);
        assert!(pf.zones_need_calculation(), "a fence that marked cells schedules a calculation");
        pf.calculate_zones();
        // Same fence again: every cell already carries this obstacle, so `setTypeAsObstacle` returns false.
        pf.classify_object_footprint(&t, ObjectId(9), &fence, &pos(300.0, 200.0), 0.0, true);
        pf.set_frame(302); // past `frame + ZONE_UPDATE_FREQUENCY`
        assert!(!pf.zones_need_calculation(), "no cell changed, so no calculation is scheduled");
        pf.set_frame(2);
        pf.classify_object_footprint(&t, ObjectId(9), &fence, &pos(300.0, 200.0), 0.0, false);
        pf.set_frame(302);
        assert!(pf.zones_need_calculation(), "the removal clears cells, so it schedules at frame 2 + 300");
    }

    /// The wall layer (`allocateCellsForWallLayer` / `classifyWallCells`, AIPathfind.cpp:3545-3630, :3686-3744)
    /// and the `LAYER_WALL` grid swap have no `markZonesDirty` site, so neither a rebuild nor a wall search
    /// schedules a zone calculation.
    #[test]
    fn a_wall_layer_search_does_not_schedule_a_zone_calculation() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        pf.set_wall_height(30.0);
        pf.add_wall_piece(ObjectId(3), &wall_piece(), &pos(200.0, 100.0), 0.0);
        pf.ensure_wall_layer();
        pf.set_frame(2);
        assert!(!pf.zones_need_calculation(), "building the wall layer marks nothing");
        let wps = pf.compute_move_path(&t, &wall_unit(150.0, 100.0, 30.0), &Coord3D::new(150.0, 100.0, 30.0), &Coord3D::new(250.0, 100.0, 30.0));
        assert!(!wps.is_empty(), "{wps:?}");
        pf.set_frame(302); // past `frame + ZONE_UPDATE_FREQUENCY`
        assert!(!pf.zones_need_calculation(), "the LAYER_WALL swap marks nothing");
    }

    /// `changeBridgeState` only *marks* the zones dirty (`AIPathfind.cpp:9892-9898`); the tables are
    /// rebuilt by `processPathfindQueue` on the due frame (`:6029-6032`). A destroyed bridge therefore
    /// still reads as connected in `clientSafeQuickDoesPathExist` until that frame — it is not
    /// re-zoned immediately.
    #[test]
    fn a_destroyed_bridge_only_schedules_a_zone_calculation() {
        let mut t = terrain_with(|_, _| 0);
        t.add_water_area(water_strip(300, 340));
        t.add_bridge(crate::bridge::Bridge::new(Coord3D::new(280.0, 300.0, 10.0), Coord3D::new(360.0, 300.0, 10.0), 40.0, 2));
        let mut pf = Pathfinder::new(&t);
        let obj = tank_obj();
        let from = pos(100.0, 200.0);
        let to = pos(500.0, 200.0);
        assert!(pf.quick_does_path_exist(&obj, &from, &to), "the deck connects the two banks");
        pf.set_frame(10);
        t.bridges_mut()[0].destroyed = true;
        pf.refresh_bridges(&t);
        assert!(
            pf.quick_does_path_exist(&obj, &from, &to),
            "the zones are still the ones from before the destruction (`markZonesDirty` only schedules)"
        );
        pf.set_frame(309);
        assert!(!pf.zones_need_calculation(), "due at frame 10 + ZONE_UPDATE_FREQUENCY");
        pf.set_frame(310);
        assert!(pf.zones_need_calculation(), "the due frame is when `processPathfindQueue` re-zones");
        pf.calculate_zones();
        assert!(!pf.zones_need_calculation(), "and `calculateZones` clears the schedule");
        assert!(!pf.quick_does_path_exist(&obj, &from, &to), "after that the destroyed deck disconnects them");
    }

    /// A tall building for `getAircraftPath`'s queries: a `KINDOF_AIRCRAFT_PATH_AROUND` structure whose
    /// cells are classified as obstacles.
    fn tall_building(pf: &mut Pathfinder, t: &TerrainLogic, id: u32, at: (f32, f32)) -> (u32, Coord3D, f32) {
        let mut b = ObjectTemplate::new("TallBuilding");
        b.kind_of |= kind_of::STRUCTURE | kind_of::IMMOBILE;
        b.geometry.major_radius = 10.0;
        b.geometry.minor_radius = 10.0;
        b.geometry.height = 80.0;
        b.aircraft_path_around = true;
        let pos = Coord3D::new(at.0, at.1, 0.0);
        pf.classify_object_footprint(t, ObjectId(id), &b, &pos, 0.0, true);
        (id, pos, b.geometry.bounding_circle_radius())
    }

    /// `Pathfinder::circleClipsTallBuilding` (`AIPathfind.cpp:9628-9645`) through `getAircraftPath`'s
    /// `checkClips` arm (`:5936-5942`): a `LOCO_WINGS` aircraft's destination within 100 units of a
    /// `KINDOF_AIRCRAFT_PATH_AROUND` building is pushed out along the normal from the flight line at
    /// `100 + boundingRadius + 2*PATHFIND_CELL_SIZE`, and the path keeps the object's position (at the
    /// destination's z) as its first node. A hover does not clip (`checkClips` false): same path, raw goal.
    #[test]
    fn an_aircraft_path_clips_a_wings_destination_around_a_tall_building() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let (id, bpos, bradius) = tall_building(&mut pf, &t, 7, (300.0, 100.0));
        let tall = [(id, bpos, bradius)];
        let closest = |p: &Coord3D| -> Option<(u32, Coord3D, f32)> {
            tall.iter().filter(|b| p.dist_2d(&b.1) - b.2 <= 100.0).map(|b| (b.0, b.1, b.2)).next()
        };
        let by_id = |oid: u32| -> Option<(Coord3D, f32)> { tall.iter().find(|b| b.0 == oid).map(|b| (b.1, b.2)) };
        let from = pos(50.0, 100.0);
        let to = pos(250.0, 100.0);
        let path = pf.aircraft_path(&from, 12.0, &to, /*check_clips=*/ true, ObjectId::INVALID, &closest, &by_id);
        assert_eq!(path.nodes.len(), 2, "start + clipped destination");
        assert_eq!(path.nodes[0].pos, Coord3D::new(50.0, 100.0, 12.0), "the start node is the object's position at the goal's z");
        // `computeNormalRadialOffset(from, adjustTo, to, building, 100 + 10 + 20)`: cross product 0 picks
        // `(-dy, dx)` = +y, so the destination moves to (300, 100 + 130).
        assert_eq!((path.nodes[1].pos.x, path.nodes[1].pos.y), (300.0, 230.0), "the goal is pushed out of the building's circle");
        assert_eq!(path.nodes[1].pos.z, to.z, "the clip keeps the destination's z");
        assert_eq!(path.waypoints(), vec![path.nodes[1].pos], "the optimised chain is the whole node list");
        assert_eq!(path.nodes[0].next_optimized, Some(1));
        assert_eq!(path.nodes[1].next_optimized, None);

        // A hover (`checkClips` false) keeps the raw destination: no clip for `LOCO_HOVER`.
        let hover = pf.aircraft_path(&from, 0.0, &to, /*check_clips=*/ false, ObjectId::INVALID, &closest, &by_id);
        assert_eq!((hover.nodes[1].pos.x, hover.nodes[1].pos.y), (to.x, to.y), "a hover flies to the goal itself");
    }

    /// `Pathfinder::segmentIntersectsTallBuilding` (`AIPathfind.cpp:9570-9624`) through `getAircraftPath`
    /// (`:5947-5969`): a segment crossing a `KINDOF_AIRCRAFT_PATH_AROUND` building gets three detour nodes
    /// (`computeNormalRadialOffset`, `:9538-9566`) at the building's radius, inserted between the segment's
    /// ends in `n1, n2, n3` order. The ignored building (`getBuildingToNotPathAround`, `:5926`) is skipped.
    #[test]
    fn an_aircraft_path_jogs_around_a_tall_building_on_its_line() {
        let t = terrain_with(|_, _| 0);
        let mut pf = Pathfinder::new(&t);
        let (id, bpos, bradius) = tall_building(&mut pf, &t, 7, (150.0, 100.0));
        let tall = [(id, bpos, bradius)];
        let closest = |_p: &Coord3D| -> Option<(u32, Coord3D, f32)> { None };
        let by_id = |oid: u32| -> Option<(Coord3D, f32)> { tall.iter().find(|b| b.0 == oid).map(|b| (b.1, b.2)) };
        let from = pos(50.0, 100.0);
        let to = pos(250.0, 100.0);
        let path = pf.aircraft_path(&from, 0.0, &to, /*check_clips=*/ false, ObjectId::INVALID, &closest, &by_id);
        assert_eq!(path.nodes.len(), 5, "start, three detour nodes, destination: {:?}", path.nodes.iter().map(|n| n.pos).collect::<Vec<_>>());
        assert_eq!(path.nodes.last().map(|n| n.pos), Some(to), "the destination stays the last node");
        // radius = 10 + 2*10 = 30; the line is +x through the building's centre, so the first normal is +y.
        assert_eq!((path.nodes[2].pos.x, path.nodes[2].pos.y), (150.0, 130.0), "the detour's middle point is at the building's radius");
        assert!(path.nodes[1].pos.dist_2d(&bpos) > 29.9 && path.nodes[1].pos.dist_2d(&bpos) < 30.1, "the first detour node is on the circle");
        assert!(path.nodes[3].pos.dist_2d(&bpos) > 29.9 && path.nodes[3].pos.dist_2d(&bpos) < 30.1, "the third one too");
        assert!(path.nodes[1].pos.y > 100.0 && path.nodes[3].pos.y > 100.0, "both go around the same side");
        // Ignoring the building (the Chinook's combat-drop target) removes the detour.
        let ignored = pf.aircraft_path(&from, 0.0, &to, false, ObjectId(id), &closest, &by_id);
        assert_eq!(ignored.nodes.len(), 2, "`getBuildingToNotPathAround` skips its own building");
    }

    /// `checkForAdjust`'s path gate (`AIPathfind.cpp:5334-5343`): `if (obj->isKindOf(KINDOF_AIRCRAFT))
    /// adjustedPathExists = true; else { ... clientSafeQuickDoesPathExist ... }`. The gate is the object's *kind*,
    /// not its locomotor's surfaces: a taxiing jet (a ground locomotor on a `KINDOF_AIRCRAFT`) skips it where the
    /// same surfaces on a non-aircraft take it. Here the unit stands on the east bank of a river and its goal is
    /// in the water, so the spiral's first clear cell is on the *west* bank: the aircraft accepts it, while the
    /// ground unit refuses it (no quick path from the goal or from its position) and stays on its own side.
    #[test]
    fn the_adjust_path_gate_is_the_aircraft_kind_not_the_surfaces() {
        let mut t = terrain_with(|_, _| 0);
        t.add_water_area(water_strip(300, 340));
        let mut pf = Pathfinder::new(&t);
        let from = pos(400.0, 300.0);
        let goal = pos(305.0, 300.0); // inside the river
        let mut jet = tank_obj();
        jet.id = ObjectId(1);
        pf.update_unit_pos(&jet, &from, false);
        assert!(!jet.is_aircraft);
        let ground = pf.adjust_destination(&t, &jet, &goal, None).expect("its own bank has cells");
        assert!(ground.x >= 350.0, "the ground unit's adjusted cell is back on its own bank: {ground:?}");
        jet.is_aircraft = true;
        // Lane 161 (wave 19): `checkDestination` has no surface test (AIPathfind.cpp:5009-5097), so the spiral's first
        // cell is the non-centred start cell (goal + half a cell, :5479-5482 -> cell (31, 30)), a river cell, and the
        // aircraft's gate is skipped (:5334-5336): it takes that cell. The old (290.5, 320.5) came from the removed
        // surface test.
        assert_eq!(
            pf.adjust_destination(&t, &jet, &goal, None).map(|p| (p.x, p.y)),
            Some((310.5, 300.5)),
            "the aircraft skips the gate and takes the first cell its footprint can stand on (a river cell is not refused)"
        );
    }
}
