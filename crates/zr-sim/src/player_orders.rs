//! Player-issued commands a skirmish human can trigger from the control bar, hotkeys or mouse
//! that were not yet ported: force move, waypoints, scatter, overcharge, weapon switch, upgrade
//! cancel and the Chinook combat drop.
//!
//! Ported from `GameLogicDispatch.cpp` (`onDoForcemoveto`, `onAddWaypoint`, `onDoScatter`,
//! `onToggleOvercharge`, `onDoSwitchWeapons`, `onCancelUpgrade`, `onCombatdrop*`),
//! `AIGroup.cpp` (`groupMoveToPosition`, `groupScatter`, `groupToggleOvercharge`,
//! `setWeaponLockForGroup`, `groupCombatDrop`), `AIUpdate.cpp`
//! (`privateFollowPathAppend`) and `ProductionUpdate.cpp` (`cancelUpgrade`).
//!
//! The plain move body lives here too so `MSG_DO_MOVETO` and `MSG_DO_FORCEMOVETO` share it, exactly
//! as `onDoMoveto` and `onDoForcemoveto` both call `AIGroup::groupMoveToPosition(&dest, false, ...)`.

use crate::ai_states::{AiStateType, CommandSource};
use crate::command::CombatDropTarget;
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::{ObjectId, PlayerIndex};
use crate::orders::{group_center, group_move_order, group_unit_dest, ignores_orders};
use crate::pathfind::{PathObject, PATHFIND_CELL_SIZE_F};
use crate::TerrainLogic;

/// `STD_WAYPOINT_CLAMP_MARGIN` (`AIGroup.cpp:1557`).
const STD_WAYPOINT_CLAMP_MARGIN: f32 = PATHFIND_CELL_SIZE_F * 4.0;
/// `STD_AIRCRAFT_EXTRA_MARGIN` (`AIGroup.cpp:1558`).
const STD_AIRCRAFT_EXTRA_MARGIN: f32 = PATHFIND_CELL_SIZE_F * 10.0;
/// `GlobalData::m_groupMoveClickToGatherFactor` default (`GlobalData.cpp:905`). The port does not
/// parse the `GroupMoveClickToGatherAreaFactor` INI key yet, so the engine default stands in for it.
const GROUP_MOVE_CLICK_TO_GATHER_FACTOR: f32 = 1.0;

/// `clampWaypointPosition` (`AIGroup.cpp:1560-1585`): keep `pos` inside the map extent trimmed by
/// `margin` on every side; a point outside it is pulled onto the edge and takes the ground height.
fn clamp_waypoint_position(terrain: &TerrainLogic, pos: &mut Coord3D, margin: f32) {
    let (lo, hi) = terrain.world_extent();
    let (lx, ly, hx, hy) = (lo.x + margin, lo.y + margin, hi.x - margin, hi.y - margin);
    if pos.x >= lx && pos.x <= hx && pos.y >= ly && pos.y <= hy {
        return;
    }
    if pos.x > hx {
        pos.x = hx;
    } else if pos.x < lx {
        pos.x = lx;
    }
    if pos.y > hy {
        pos.y = hy;
    } else if pos.y < ly {
        pos.y = ly;
    }
    pos.z = terrain.ground_height(pos.x, pos.y);
}

/// `ScaleRect2D` (`Line2D.cpp:345-357`): grow or shrink `min`/`max` about their centre by `factor`.
fn scale_rect(min: &mut (f32, f32), max: &mut (f32, f32), factor: f32) {
    let f = factor - 1.0;
    let dw = (max.0 - min.0) * f * 0.5;
    let dh = (max.1 - min.1) * f * 0.5;
    min.0 -= dw;
    min.1 -= dh;
    max.0 += dw;
    max.1 += dh;
}

/// `getHelicopterOffset` (`AIGroup.cpp:1862-1889`): the `idx`-th helicopter's place on the rings of
/// circles around the tighten point.
fn helicopter_offset(pos: &mut Coord3D, idx: i32) {
    if idx == 0 {
        return;
    }
    const CIRCLE: f32 = 2.0 * std::f32::consts::PI;
    let assumed_heli_diameter = 70.0f32;
    let mut radius = assumed_heli_diameter;
    let mut circumference = radius * CIRCLE;
    let mut angle = 0.0f32;
    let mut angle_between = assumed_heli_diameter / circumference * CIRCLE;
    for _ in 1..idx {
        angle += angle_between;
        if angle > CIRCLE {
            radius += assumed_heli_diameter;
            circumference = radius * CIRCLE;
            angle_between = assumed_heli_diameter / circumference * CIRCLE;
            angle -= CIRCLE;
        }
    }
    let t = *pos;
    pos.x = t.x + crate::math::sin(angle) * radius;
    pos.y = t.y + crate::math::cos(angle) * radius;
}

impl GameLogic {
    /// `AIUpdateInterface::setGoalPositionClipped` (`AIUpdate.cpp:351-380`): a player's goal is kept inside the map
    /// extent trimmed by half a partition cell (`m_partitionCellSize * 0.5`). An aircraft significantly above the
    /// terrain keeps the larger of that and its preferred height. Other sources (AI, script) are not clipped.
    pub(crate) fn set_goal_position_clipped(&self, id: ObjectId, pos: Coord3D, source: CommandSource) -> Coord3D {
        if source != CommandSource::Player {
            return pos;
        }
        let Some(o) = self.objects.get(&id) else { return pos };
        let mut fudge = self.partition.cell_size() * 0.5;
        if o.is_kind_of(crate::template::kind_of::AIRCRAFT) && self.significantly_above_terrain(id) {
            if let Some(loco) = o.locomotor() {
                fudge = fudge.max(loco.preferred_height);
            }
        }
        // The C++ tests the low edge first, then the high one; with the low edge below the high one that is a clamp.
        let (lo, hi) = self.terrain.world_extent();
        Coord3D::new((pos.x.max(lo.x + fudge)).min(hi.x - fudge), (pos.y.max(lo.y + fudge)).min(hi.y - fudge), pos.z)
    }

    /// `MSG_DO_MOVETO` / `MSG_DO_FORCEMOVETO` -> `AIGroup::groupMoveToPosition(&dest, false, CMD_FROM_PLAYER)`
    /// (`GameLogicDispatch.cpp:1309,1323`). A script move is `CMD_FROM_SCRIPT`. `group_path` false is
    /// the direct `aiMoveToPosition` a scatter uses (`AIGroup.cpp:1855`), which has no group offset.
    pub(crate) fn command_move_to_from(&mut self, player: PlayerIndex, ids: &[ObjectId], pos: Coord3D, source: CommandSource, group_path: bool) {
        if group_path {
            self.group_move_to_position(player, ids, pos, false, source);
            return;
        }
        for &id in ids {
            let Some(obj) = self.objects.get(&id) else { continue };
            if obj.destroyed || obj.player != player || obj.ai_handle.is_none() || ignores_orders(obj) {
                continue;
            }
            // `privateMoveToPosition` returns when `!isMobile()` (`AIUpdate.cpp:2889-2893`): any disable type
            // or `KINDOF_IMMOBILE` drops the order and keeps the old state.
            // `privateMoveToPosition` returns when `!isMobile()` (`AIUpdate.cpp:2889-2893`): any disable type
            // or `KINDOF_IMMOBILE` drops the order and keeps the old state.
            if !self.is_mobile(id) {
                continue;
            }
            // `privateMoveToPosition`'s normal (non-AI/player) branch (`AIUpdate.cpp:2912`) clears
            // before setting the goal. A jet owns its `aiDoCommand` override.
            if !self.is_jet(id) {
                self.ai_clear(id);
            }
            let goal = self.set_goal_position_clipped(id, pos, source);
            if let Some(o) = self.objects.get_mut(&id) {
                o.combat.attack = None; // a move order cancels the attack
                o.combat.attack_move = false;
                o.combat.guard = None;
                o.combat.hunt = false;
                o.ai.machine.goal_position = goal;
                o.ai.machine.goal_object = ObjectId::INVALID;
                o.ai.machine.group_dest = None;
                o.ai.machine.last_command_source = source;
            }
            self.ai_command_set_state(id, AiStateType::MoveTo);
        }
    }

    /// `AIGroup::groupMoveToPosition` (`AIGroup.cpp:1591-1792`), used by `MSG_DO_MOVETO`,
    /// `MSG_DO_FORCEMOVETO` and `MSG_ADD_WAYPOINT` (`add_waypoint`, `GameLogicDispatch.cpp:1337`).
    /// Not modelled: the infantry and vehicle column moves (`friend_computeGroundPath`,
    /// `friend_moveInfantryToPos`, `friend_moveVehicleToPos`, which need `Pathfinder::findGroundPath`)
    /// and the formation path. `isFormation` is therefore always false: no member has a formation id.
    fn group_move_to_position(&mut self, player: PlayerIndex, ids: &[ObjectId], pos_in: Coord3D, add_waypoint: bool, source: CommandSource) {
        let mut pos = pos_in;
        let members: Vec<ObjectId> = ids
            .iter()
            .copied()
            .filter(|id| self.objects.get(id).is_some_and(|o| !o.destroyed && o.player == player))
            .collect();

        // `getMinMaxAndCenter` (`AIGroup.cpp:380-431`): the bounds of the members that are not
        // `DISABLED_HELD` riders and have an AI.
        let mut bounds = self.group_bounds(&members);
        // `tightenGroup` (`AIGroup.cpp:1622-1630`): a player click inside the scaled group bounds.
        let mut tighten_group = false;
        if source == CommandSource::Player && GROUP_MOVE_CLICK_TO_GATHER_FACTOR > 0.0 {
            if let Some((min, max)) = bounds.as_mut() {
                scale_rect(min, max, GROUP_MOVE_CLICK_TO_GATHER_FACTOR);
                tighten_group = pos.x >= min.0 && pos.x <= max.0 && pos.y >= min.1 && pos.y <= max.1;
            }
        }

        // The helicopter and aircraft margins (`AIGroup.cpp:1632-1653`), over every member.
        let mut extra_margin = 0.0f32;
        for &id in &members {
            let Some(o) = self.objects.get(&id) else { continue };
            if o.template.is_collide_kind_of(crate::template::collide_kind::PRODUCED_AT_HELIPAD) {
                extra_margin = extra_margin.max(o.template.geometry.major_radius);
            } else if o.is_kind_of(crate::template::kind_of::AIRCRAFT) {
                // A fixed-wing aircraft that is airborne keeps its spread (`AIGroup.cpp:1645-1648`).
                if o.ai_handle.is_some() && !self.is_doing_ground_movement(id) {
                    tighten_group = false;
                }
                extra_margin = extra_margin.max(STD_AIRCRAFT_EXTRA_MARGIN);
            }
        }
        // `Int margin = STD_WAYPOINT_CLAMP_MARGIN + extraMargin;` truncates to an integer.
        let margin = (STD_WAYPOINT_CLAMP_MARGIN + extra_margin) as i32 as f32;
        clamp_waypoint_position(&self.terrain, &mut pos, margin);

        // `groupTightenToPosition` (`AIGroup.cpp:1661-1673`); the cell count uses `max.x` twice.
        if tighten_group && !add_waypoint {
            if let Some((min, max)) = bounds {
                let dx = ((max.0 - min.0) / PATHFIND_CELL_SIZE_F) as i32;
                let dy = ((max.0 - min.0) / PATHFIND_CELL_SIZE_F) as i32;
                if dx * dy < 2000 {
                    self.group_tighten_to_position(player, ids, pos, source);
                    return;
                }
            }
        }

        // The near-to-far loop (`AIGroup.cpp:1682-1791`). The first unit is the group centre, and
        // `computeIndividualDestination` gives each unit its offset from it, at the group destination
        // `pos` (`group_dest`), and the layer height there (`:1757`).
        let (order, center) = group_move_order(self, player, ids, &pos);
        // `groupMoveToPosition`'s loop calls `removeGoal` on each member it will move, after the `DISABLED_HELD`,
        // `KINDOF_IMMOBILE`, no-AI and column filters (`AIGroup.cpp:1687-1716`), so a member the order skips keeps
        // its goal cell. `group_move_order` returns exactly that filtered, sorted list.
        for &id in &order {
            self.pathfind_remove_goal(id);
        }
        let Some(center) = center else { return };
        for id in order {
            let Some(obj) = self.objects.get(&id) else { continue };
            let from = obj.position;
            let r = obj.template.geometry.bounding_circle_radius();
            let dest = group_unit_dest(&self.terrain, &center, &pos, &from, r);
            let dest = self.group_unit_adjusted_dest(id, dest, pos);
            let group_dest = Some(pos);
            // The stealth rule (`AIGroup.cpp:1759-1781`) runs before `aiMoveToPosition`, in the same loop.
            if source == CommandSource::Player {
                self.group_stealth_mood_delay(id);
            }
            if add_waypoint {
                // `ai->aiFollowPathAppend(&dest, cmdSource)` (`AIGroup.cpp:1789`).
                self.ai_follow_path_append(id, dest, group_dest);
                continue;
            }
            // `aiMoveToPosition` -> `privateMoveToPosition`'s `!isMobile()` return (`AIUpdate.cpp:2889-2893`).
            if !self.is_mobile(id) {
                continue;
            }
            // `privateMoveToPosition`'s normal (non-AI/player) branch (`AIUpdate.cpp:2912`) clears
            // before setting the goal. A jet owns its `aiDoCommand` override.
            if !self.is_jet(id) {
                self.ai_clear(id);
            }
            let goal = self.set_goal_position_clipped(id, dest, source);
            if let Some(o) = self.objects.get_mut(&id) {
                o.combat.attack = None; // a move order cancels the attack
                o.combat.attack_move = false;
                o.combat.guard = None;
                o.combat.hunt = false;
                o.ai.machine.goal_position = goal;
                o.ai.machine.goal_object = ObjectId::INVALID;
                o.ai.machine.group_dest = group_dest;
                o.ai.machine.last_command_source = source;
            }
            self.ai_command_set_state(id, AiStateType::MoveTo);
        }
    }

    /// The `getMinMaxAndCenter` bounds (`AIGroup.cpp:380-431`) of the members that are not held riders
    /// and have an AI; `None` when there is none.
    fn group_bounds(&self, members: &[ObjectId]) -> Option<((f32, f32), (f32, f32))> {
        let mut min = (1e10f32, 1e10f32);
        let mut max = (-1e10f32, -1e10f32);
        let mut count = 0;
        for &id in members {
            let Some(o) = self.objects.get(&id) else { continue };
            if o.is_disabled_by_type(crate::disabled::DisabledType::Held) || o.ai_handle.is_none() {
                continue;
            }
            min = (min.0.min(o.position.x), min.1.min(o.position.y));
            max = (max.0.max(o.position.x), max.1.max(o.position.y));
            count += 1;
        }
        (count > 0).then_some((min, max))
    }

    /// `AIGroup::groupTightenToPosition` (`AIGroup.cpp:1895-1974`): every member takes the shortest
    /// path to `pos`, nearest first, with the helicopters on their own offsets. The `addWaypoint`
    /// branch is not reached (the tighten path needs `!addWaypoint`).
    fn group_tighten_to_position(&mut self, player: PlayerIndex, ids: &[ObjectId], pos: Coord3D, source: CommandSource) {
        // The member filter is the move loop's (`AIGroup.cpp:1917-1931`): `DISABLED_HELD`,
        // `KINDOF_IMMOBILE` and `getAI() == nullptr`, sorted `ITER_SORTED_NEAR_TO_FAR`.
        let (order, _) = group_move_order(self, player, ids, &pos);
        let mut heli_idx = 0;
        for &id in &order {
            let is_heli = self.objects.get(&id).is_some_and(|o| o.template.is_collide_kind_of(crate::template::collide_kind::PRODUCED_AT_HELIPAD));
            if is_heli {
                let mut heli_offs = pos;
                helicopter_offset(&mut heli_offs, heli_idx);
                heli_idx += 1;
                self.ai_tighten_to_position(id, heli_offs, CommandSource::Ai);
            } else {
                self.ai_tighten_to_position(id, pos, source);
            }
        }
        // `TheAI->pathfinder()->updatePos(theUnit, &unitPos)` (`AIGroup.cpp:1969-1973`).
        for &id in &order {
            if !self.is_doing_ground_movement(id) {
                continue;
            }
            let Some(o) = self.objects.get(&id) else { continue };
            let Some(po) = PathObject::for_object(o) else { continue };
            let p = o.position;
            self.pathfinder.update_unit_pos(&po, &p, false);
        }
    }

    /// `AIUpdateInterface::privateTightenToPosition` (`AIUpdate.cpp:3167-3176`): clear the machine (the
    /// old state's `onExit` runs first), set the goal through `setGoalPositionClipped`, and enter
    /// `AI_MOVE_AND_TIGHTEN`. The helicopter offsets are `CMD_FROM_AI` and so are not clipped (`AIGroup.cpp:1918-1928`).
    fn ai_tighten_to_position(&mut self, id: ObjectId, pos: Coord3D, source: CommandSource) {
        if !self.is_mobile(id) {
            return;
        }
        self.ai_clear(id);
        let goal = self.set_goal_position_clipped(id, pos, source);
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack = None;
            o.combat.attack_move = false;
            o.combat.guard = None;
            o.combat.hunt = false;
            o.ai.machine.goal_position = goal;
            o.ai.machine.goal_object = ObjectId::INVALID;
            o.ai.machine.group_dest = None;
            o.ai.machine.last_command_source = source;
        }
        self.ai_command_set_state(id, AiStateType::MoveAndTighten);
    }

    /// `MSG_ADD_WAYPOINT` -> `AIGroup::groupMoveToPosition(&dest, addWaypoint = true, ...)`
    /// (`AIGroup.cpp:1591`) -> `AIUpdateInterface::aiFollowPathAppend` (`AIUpdate.cpp:3358`): append
    /// `pos` to the group's current move path.
    pub(crate) fn command_add_waypoint(&mut self, player: PlayerIndex, ids: &[ObjectId], pos: Coord3D) {
        self.group_move_to_position(player, ids, pos, true, CommandSource::Player);
    }

    /// `AIUpdateInterface::privateFollowPathAppend` (`AIUpdate.cpp:3358`): a dynamic waypoint.
    /// The ground half of `AIGroup::computeIndividualDestination` (AIGroup.cpp:546-548): a ground mover's offset
    /// destination is adjusted against the group destination (`adjustDestination(obj, ..., dest, groupDest)`), and the
    /// adjusted cell becomes the unit's goal (`updateGoal`). The formation case passes no group destination; the
    /// port's group orders are not formations here.
    fn group_unit_adjusted_dest(&mut self, id: ObjectId, dest: Coord3D, group: Coord3D) -> Coord3D {
        if !self.is_doing_ground_movement(id) {
            return dest;
        }
        let Some(po) = self.path_object(id) else { return dest };
        let adjusted = self.pathfinder.adjust_destination(&self.terrain, &po, &dest, Some(&group)).unwrap_or(dest);
        self.pathfind_update_goal(id, &adjusted);
        adjusted
    }

    fn ai_follow_path_append(&mut self, id: ObjectId, pos: Coord3D, group_dest: Option<Coord3D>) {
        if !self.is_mobile(id) {
            return;
        }
        let (in_follow, path_len, effectively_moving, goal) = match self.objects.get(&id) {
            Some(o) => (
                o.ai.machine.current == AiStateType::FollowPath,
                o.ai.machine.goal_path.len(),
                // `isMoving() || isWaitingForPath()`; the sim's `is_moving` already folds in the
                // wait flag.
                o.is_ai_moving() || o.ai.waiting_for_path,
                o.ai.machine.goal_position,
            ),
            None => return,
        };
        if in_follow && path_len > 0 && effectively_moving {
            // We already have a path, so simply add the point to the end of it.
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.goal_path.push(pos);
                o.ai.machine.last_command_source = CommandSource::Player;
            }
            return;
        }
        let mut path = Vec::new();
        if effectively_moving {
            // Moving to a point already: add our waypoint after that point and convert it to a
            // waypoint command.
            path.push(goal);
        }
        path.push(pos);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.group_dest = group_dest;
        }
        // `privateFollowPath` (`AIUpdate.cpp:3390`) clears the machine before setting the goal path
        // (and sets the command source to `AI`); the player issued this one, so restore it.
        self.ai_follow_path(id, path, false);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.last_command_source = CommandSource::Player;
        }
    }

    /// `MSG_DO_SCATTER` -> `AIGroup::groupScatter` (`AIGroup.cpp:1801`): every mobile member moves
    /// four bounding radii outward from the group centre, far members first.
    pub(crate) fn command_scatter(&mut self, player: PlayerIndex, ids: &[ObjectId]) {
        // `getMinMaxAndCenter`'s centre is the member average (`AIGroup.cpp:380`).
        let Some(center) = group_center(self, ids) else { return };
        // `SimpleObjectIterator::sort(ITER_SORTED_FAR_TO_NEAR)`: farthest first, ties by id so the
        // order is deterministic.
        let mut order: Vec<(f32, ObjectId)> = Vec::new();
        for &id in ids {
            let Some(o) = self.objects.get(&id) else { continue };
            if o.destroyed || o.player != player || o.ai_handle.is_none() || ignores_orders(o) {
                continue;
            }
            // `groupScatter` skips `DISABLED_HELD` riders and immobile members.
            if o.is_disabled_by_type(crate::disabled::DisabledType::Held) || !self.is_mobile(id) {
                continue;
            }
            let dx = o.position.x - center.x;
            let dy = o.position.y - center.y;
            order.push((dx * dx + dy * dy, id));
        }
        order.sort_by(|a, b| {
            b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.0.cmp(&b.1.0))
        });
        let (mut cx, cy) = (center.x, center.y);
        for (_, id) in order {
            let (from, r) = match self.objects.get(&id) {
                Some(o) => (o.position, o.template.geometry.bounding_circle_radius()),
                None => continue,
            };
            // `center.x -= 0.01f` accumulates each iteration, so the outward ray skews a little.
            cx -= 0.01;
            let (mut dx, mut dy) = (from.x - cx, from.y - cy);
            let len = crate::math::sqrt(dx * dx + dy * dy);
            if len > 0.0 {
                dx /= len;
                dy /= len;
            }
            let dest = Coord3D::new(from.x + dx * 4.0 * r, from.y + dy * 4.0 * r, from.z);
            // `AIGroup::groupScatter` -> `ai->aiMoveToPosition(&dest, cmdSource)` (`AIGroup.cpp:1849`): a
            // direct move, not `groupMoveToPosition`, so the stealth rule does not apply.
            self.command_move_to_from(player, &[id], dest, CommandSource::Player, false);
        }
    }

    /// `MSG_TOGGLE_OVERCHARGE` -> `AIGroup::groupToggleOvercharge` (`AIGroup.cpp:2862`): toggle the
    /// `OverchargeBehavior` of every member that has one (`OverchargeBehavior::toggle` ->
    /// `enable(!active)`).
    pub(crate) fn command_toggle_overcharge(&mut self, _player: PlayerIndex, ids: &[ObjectId]) {
        for &id in ids {
            let active = match self.objects.get(&id) {
                Some(o) if o.template.overcharge.is_some() => o.behavior.overcharge,
                _ => continue,
            };
            self.set_overcharge(id, !active);
        }
    }

    /// `MSG_SWITCH_WEAPONS` -> `AIGroup::setWeaponLockForGroup(slot, LOCKED_PERMANENTLY)`
    /// (`GameLogicDispatch.cpp:1061`, `AIGroup.cpp:3071`, `WeaponSet::setWeaponLock`,
    /// `WeaponSet.cpp:1078`): lock each unit to `slot` until another switch or a weapon-set change.
    pub(crate) fn command_switch_weapons(&mut self, player: PlayerIndex, ids: &[ObjectId], slot: usize) {
        for &id in ids {
            // The selection is the player's (`GameLogicDispatch.cpp:386-400`); `setWeaponLock` fails for a slot with no weapon.
            if self.objects.get(&id).is_some_and(|o| !o.destroyed && o.player == player) {
                self.set_weapon_lock(id, slot, crate::weapon::WeaponLockType::Permanently);
            }
        }
    }

    /// `MSG_CANCEL_UPGRADE` -> `GameLogicDispatch.cpp:1780` -> `ProductionUpdate::cancelUpgrade`
    /// (`ProductionUpdate.cpp:326`): refund the upgrade cost and drop its queue entry. The producer
    /// must be controlled by the player and have a `ProductionUpdate`.
    pub(crate) fn command_cancel_upgrade(&mut self, player: PlayerIndex, building: ObjectId, upgrade: &str) {
        let Some(o) = self.objects.get(&building) else { return };
        if o.destroyed || o.player != player || o.production.is_none() {
            return;
        }
        let index = self.objects.get(&building).and_then(|o| o.production.as_ref()).and_then(|st| {
            st.queue.iter().position(|e| {
                matches!(&e.kind, crate::production::ProductionKind::Upgrade(n) if n == upgrade)
            })
        });
        let Some(index) = index else { return };
        // `calcCostToBuild` is the cost withdrawn when the upgrade was queued.
        let refund = self.upgrade_info.get(upgrade).map_or(0, |i| i.cost);
        let removed = self
            .objects
            .get_mut(&building)
            .and_then(|o| o.production.as_mut())
            .map(|st| st.queue.remove(index));
        if removed.is_some() {
            if let Some(p) = self.players.get_mut(&player) {
                p.deposit(refund);
            }
        }
    }

    /// `MSG_COMBATDROP_AT_OBJECT` / `MSG_COMBATDROP_AT_LOCATION` -> `AIGroup::groupCombatDrop`
    /// (`GameLogicDispatch.cpp:993,1017`, `AIGroup.cpp:2937`) -> `AIUpdateInterface::aiCombatDrop`.
    /// Only the Chinook (`ChinookAIUpdate`) implements it; `start_combat_drop` bails for the rest,
    /// where the engine's default is a debug-only fallback (`AIUpdate.cpp:3813`).
    pub(crate) fn command_combat_drop(&mut self, _player: PlayerIndex, ids: &[ObjectId], target: CombatDropTarget) {
        for &id in ids {
            let (pos, obj) = match target {
                CombatDropTarget::Position(p) => (p, None),
                CombatDropTarget::Object(o) => match self.objects.get(&o) {
                    Some(t) => (t.position, Some(o)),
                    None => continue,
                },
            };
            self.start_combat_drop(id, pos, obj);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `getHelicopterOffset` (`AIGroup.cpp:1862-1889`): the first helicopter keeps the point, the
    /// second is 70 units out at angle 0 (the ring spacing is one radian at 70 units), and the
    /// third is at angle 1 rad. Revert check: a helicopter tightens onto the point itself.
    #[test]
    fn helicopter_tighten_offsets_follow_the_engine_rings() {
        let base = Coord3D::new(100.0, 100.0, 0.0);
        let mut first = base;
        helicopter_offset(&mut first, 0);
        assert_eq!(first, base, "the first helicopter keeps the point");
        let mut second = base;
        helicopter_offset(&mut second, 1);
        assert!((second.x - 100.0).abs() < 1e-3 && (second.y - 170.0).abs() < 1e-3, "second {second:?}");
        let mut third = base;
        helicopter_offset(&mut third, 2);
        assert!((third.x - (100.0 + 70.0 * 1.0f32.sin())).abs() < 1e-2, "third {third:?}");
        assert!((third.y - (100.0 + 70.0 * 1.0f32.cos())).abs() < 1e-2, "third {third:?}");
    }

    fn tank() -> std::sync::Arc<crate::ObjectTemplate> {
        let mut t = crate::ObjectTemplate::new("Tank");
        t.kind_of = crate::template::kind_of::VEHICLE | crate::template::kind_of::SELECTABLE;
        t.geometry.major_radius = 8.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.locomotor = Some(crate::template::LocomotorParams::from_ini(crate::template::LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
        std::sync::Arc::new(t)
    }

    /// `AIUpdateInterface::setGoalPositionClipped` (`AIUpdate.cpp:351-380`): a player's move goal is kept
    /// half a partition cell inside the map edge (`m_partitionCellSize * 0.5`, 20 here); a script goal is
    /// not clipped (`CMD_FROM_SCRIPT`). Revert check: the goal stays at the clicked point, 500 past the edge.
    #[test]
    fn a_player_move_goal_is_clipped_to_the_map_edge() {
        use crate::ai_states::CommandSource;
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let id = g.create_object(&tank(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let (_, hi) = g.terrain.world_extent();
        let fudge = g.partition.cell_size() * 0.5;
        let far = Coord3D::new(hi.x + 500.0, 100.0, 0.0);
        g.command_move_to_from(0, &[id], far, CommandSource::Player, false);
        let goal = g.find_object(id).unwrap().ai.machine.goal_position;
        assert!((goal.x - (hi.x - fudge)).abs() < 1e-3, "player goal {goal:?} clipped to the edge less {fudge}");
        g.command_move_to_from(0, &[id], far, CommandSource::Script, false);
        assert_eq!(g.find_object(id).unwrap().ai.machine.goal_position.x, far.x, "a script goal is not clipped");
    }

    /// `AIGroup::groupMoveToPosition` gives each member its own offset from the centre (`AIGroup.cpp:514-551`), and
    /// `privateMoveToPosition` clips a player's goal (`AIUpdate.cpp:2889-2916`). The click is 40 inside the edge
    /// (the group clamp), the centre is the member nearest it, and the other member lies 25 further east, so its
    /// goal would be 15 past the clip line (hi-20). Revert check: the goal is stored unclipped (hi-15).
    #[test]
    fn a_group_members_player_goal_is_clipped_to_the_map_edge() {
        use crate::ai_states::CommandSource;
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let (_, hi) = g.terrain.world_extent();
        let fudge = g.partition.cell_size() * 0.5;
        let centre = g.create_object(&tank(), Coord3D::new(hi.x - 30.0, 100.0, 0.0), 0.0, 0);
        let east = g.create_object(&tank(), Coord3D::new(hi.x - 5.0, 100.0, 0.0), 0.0, 0);
        let click = Coord3D::new(hi.x - 40.0, 100.0, 0.0);
        g.command_move_to_from(0, &[centre, east], click, CommandSource::Player, true);
        let goal = |id| g.find_object(id).unwrap().ai.machine.goal_position;
        // Traced by hand as in orders.rs: the click (3980,100) is the non-centred cell (398,10), adjusted to (3980.5,100.5),
        // then the degenerate tightening keeps (3980.5,110.5) (AIPathfind.cpp:8387, :9196).
        assert_eq!((goal(centre).x, goal(centre).y), (3980.5, 110.5), "the centre goes to the click's cell: {:?}", goal(centre));
        assert!((goal(east).x - (hi.x - fudge)).abs() < 1e-3, "offset goal {:?} clipped", goal(east));
    }

    /// The possession filter (docs/POSSESSION.md: a possessed unit ignores move orders) is a possession-only
    /// addition: `AIGroup::groupMoveToPosition` has no such check (`AIGroup.cpp:1687-1699`). Without the
    /// `possession` feature a suspended member is still ordered; with it, the member is left alone. Revert check
    /// (default build): reinstating the `ai.suspended` filter outside the feature leaves the member Idle.
    #[test]
    fn a_suspended_member_takes_a_group_move_only_without_possession() {
        use crate::ai_states::{AiStateType, CommandSource};
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let a = g.create_object(&tank(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let b = g.create_object(&tank(), Coord3D::new(200.0, 100.0, 0.0), 0.0, 0);
        g.objects.get_mut(&a).unwrap().ai.suspended = true;
        g.command_move_to_from(0, &[a, b], Coord3D::new(300.0, 100.0, 0.0), CommandSource::Player, true);
        let state = g.find_object(a).unwrap().ai.machine.current;
        assert_eq!(state == AiStateType::MoveTo, !cfg!(feature = "possession"), "suspended member state {state:?}");
    }
}
