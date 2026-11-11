//! Order commands the viewer needs that were not yet ported: `AttackMove`, `Guard` and `Sell`.
//!
//! Ported from `GameLogicDispatch.cpp` (`MSG_DO_ATTACKMOVETO`, `MSG_DO_GUARD_*`,
//! `MSG_SELL`), `AI/AIStates.cpp` (`AIAttackMoveState`, `AIGuardState`) and
//! `Common/System/BuildAssistant.cpp` (`sellObject`). `AttackMove` / `Guard` keep the unit's
//! auto-acquire running while it moves (`update_idle` in [`crate::combat`] now scans when either is
//! set); `Guard` also returns to the guard point. `Sell` refunds `GlobalData::m_sellPercentage`
//! (0.5 in retail) of the build cost, evacuates the contents and removes the building.

use crate::ai_states::CommandSource;
use crate::combat::AttackTarget;
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::{ObjectId, PlayerIndex};
use crate::weapon::WeaponLockType;
use crate::TerrainLogic;

/// `PRIMARY_WEAPON` (`WeaponSet.h`): weapon slot 0.
const PRIMARY_WEAPON: usize = 0;

/// What a guard order protects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GuardTarget {
    Point(Coord3D),
    Object(ObjectId),
}

/// `GUI_COMMAND_GUARD*` behaviour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GuardMode {
    /// `GUARD`: pursue targets out to the guard range.
    #[default]
    Normal,
    /// `GUARD_WITHOUT_PURSUIT`: only fire at targets already in weapon range.
    WithoutPursuit,
    /// `GUARD_FLYING_UNITS_ONLY`: only acquire airborne targets.
    FlyingUnitsOnly,
}

/// `GlobalData::m_sellPercentage` default (`GlobalData.cpp`).
pub const SELL_PERCENTAGE: f32 = 0.5;

/// Possession's order filter: a possessed unit ignores move, stop and attack orders (docs/POSSESSION.md). The
/// engine's group loops have no such check (`AIGroup.cpp:1687-1699`, `:2327`, `:2545`, `:2605`), so the filter is a
/// possession-only addition: only the `possession` feature sets `ai.suspended`, and the plain build never reads it.
pub(crate) fn ignores_orders(o: &crate::object::Object) -> bool {
    cfg!(feature = "possession") && o.ai.suspended
}

/// `AIGroup::groupMoveToPosition`'s member order and centre (`AIGroup.cpp:1683-1745`). Every member is
/// inserted into a `SimpleObjectIterator` with `dx*dx+dy*dy` to the destination (the move-priority
/// `adjust` is `#if 0`'d, `:1715-1730`) and the list is sorted `ITER_SORTED_NEAR_TO_FAR`
/// (`:1734`): "Move the ones nearest the goal first. Reduces collision problems later." `insert`
/// prepends to the clump list (`SimpleObjectIterator.cpp:77-90`) and `sort` is a stable mergesort
/// (`:150-230`), so equal distances keep the *reverse* member order. `center` is the first unit's
/// position (`:1737-1743`), which `computeIndividualDestination` (`:514-551`) uses as the group
/// centre — not the average `AIGroup::getMinMaxAndCenter` computes for the formation/tighten paths.
///
/// The member filter is the engine's move-loop filter (`AIGroup.cpp:1687-1699`: `DISABLED_HELD`,
/// `KINDOF_IMMOBILE` and `getAI() == nullptr`), plus the owner check and the possession filter. The owner
/// check is the engine's `GameLogicDispatch.cpp:386-400`, which drops the non-owned units of a selection
/// (`AIGroup::removeAnyObjectsNotOwnedByPlayer`, `AIGroup.cpp:295-318`) before any group order runs. The
/// infantry/vehicle column moves (`didInfantry` / `didVehicles`) are not modelled (ROADMAP).
pub(crate) fn group_move_order(logic: &GameLogic, player: PlayerIndex, ids: &[ObjectId], dest: &Coord3D) -> (Vec<ObjectId>, Option<Coord3D>) {
    let mut members: Vec<(ObjectId, f32)> = Vec::new();
    for &id in ids.iter().rev() {
        let Some(o) = logic.find_object(id) else { continue };
        if o.destroyed
            || o.player != player
            || o.ai_handle.is_none()
            || ignores_orders(o)
            || o.is_disabled_by_type(crate::disabled::DisabledType::Held)
            || o.is_kind_of(crate::template::kind_of::IMMOBILE)
        {
            continue;
        }
        let (dx, dy) = (o.position.x - dest.x, o.position.y - dest.y);
        members.push((id, dx * dx + dy * dy));
    }
    members.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let center = members.first().and_then(|(id, _)| logic.find_object(*id)).map(|o| o.position);
    (members.into_iter().map(|(id, _)| id).collect(), center)
}

/// `AIGroup::getCenter` (`AIGroup.cpp:325`) as an average, kept for the orders whose engine path does
/// not use the group-move centre (`groupScatter`, `createFormation`, `groupTightenToPosition`).
pub(crate) fn group_center(logic: &GameLogic, ids: &[ObjectId]) -> Option<Coord3D> {
    let (mut n, mut x, mut y, mut z) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for &id in ids {
        if let Some(o) = logic.find_object(id) {
            // `AIGroup::getCenter` ignores `DISABLED_HELD` riders (`AIGroup.cpp:335`).
            if o.destroyed || o.is_disabled_by_type(crate::disabled::DisabledType::Held) {
                continue;
            }
            n += 1.0;
            x += o.position.x;
            y += o.position.y;
            z += o.position.z;
        }
    }
    (n > 0.0).then(|| Coord3D::new(x / n, y / n, z / n))
}

/// `AIGroup::getCenter` (`AIGroup.cpp:325-380`): the average position of the members that have an AI and
/// are not `DISABLED_HELD`; when none of them has an AI (a team of buildings), the average of every non-held
/// member. `None` when there is nothing to average.
pub(crate) fn ai_group_center(logic: &GameLogic, ids: &[ObjectId]) -> Option<Coord3D> {
    let (mut x, mut y, mut z) = (0.0f32, 0.0f32, 0.0f32);
    let mut count = 0.0f32;
    for &id in ids {
        let Some(o) = logic.find_object(id) else { continue };
        if o.is_disabled_by_type(crate::disabled::DisabledType::Held) {
            continue; // don't bother counting riders in the center calculation.
        }
        if o.ai_handle.is_none() {
            continue;
        }
        x += o.position.x;
        y += o.position.y;
        z += o.position.z;
        count += 1.0;
    }
    if count == 0.0 && !ids.is_empty() {
        for &id in ids {
            let Some(o) = logic.find_object(id) else { continue };
            if o.is_disabled_by_type(crate::disabled::DisabledType::Held) {
                continue;
            }
            x += o.position.x;
            y += o.position.y;
            z += o.position.z;
            count += 1.0;
        }
    }
    (count > 0.0).then(|| Coord3D::new(x / count, y / count, z / count))
}

/// `AIGroup::computeIndividualDestination` (`AIGroup.cpp:514-551`), the non-formation path: offset the
/// destination by the unit's position relative to the group centre, clamped to six bounding radii,
/// and take the height of the destination's layer (`:534-540`). The `adjustDestination` /
/// `updateGoal` pass (`:541-549`) happens when the path is computed (`compute_move_path_group`).
pub(crate) fn group_unit_dest(terrain: &TerrainLogic, center: &Coord3D, group_dest: &Coord3D, pos: &Coord3D, radius: f32) -> Coord3D {
    let (mut vx, mut vy) = (pos.x - center.x, pos.y - center.y);
    let len = crate::math::sqrt(vx * vx + vy * vy);
    if len > 0.0 {
        let l = len.min(6.0 * radius);
        vx = vx / len * l;
        vy = vy / len * l;
    }
    let (x, y) = (group_dest.x + vx, group_dest.y + vy);
    let layer = terrain.layer_for_destination(group_dest);
    Coord3D::new(x, y, terrain.layer_height(x, y, layer))
}

impl GameLogic {
    /// `MSG_DO_ATTACKMOVETO`: move to `pos`, auto-acquiring targets on the way
    /// (`AIAttackMoveState`). `AIGroup::groupAttackMoveToPosition` (`AIGroup.cpp:2327-2340`) hands the
    /// clicked point to every member unchanged: there is no per-member group offset.
    ///
    /// `source` is the `CommandSourceType` of the order: `CMD_FROM_PLAYER` for the UI dispatch, and
    /// `CMD_FROM_SCRIPT` for `ScriptActions`' team orders. It reaches
    /// `setGoalPositionClipped` (which clips a *player's* target to the map's edge margin,
    /// `AIUpdate.cpp:351-380`) and `setLastCommandSource` (`:3608`), which the attack-move state saves as
    /// its own `m_commandSrc` (`AIStates.cpp:3590`).
    pub(crate) fn command_attack_move(&mut self, player: PlayerIndex, ids: &[ObjectId], pos: Coord3D, source: CommandSource) {
        for &id in ids {
            let Some(obj) = self.objects.get(&id) else { continue };
            if obj.destroyed || obj.player != player || obj.ai_handle.is_none() || ignores_orders(obj) {
                continue;
            }
            // `privateAttackMoveToPosition` (`AIUpdate.cpp:3595-3598`) returns when `!isMobile()`, and the non-attacker
            // `privateMoveToPosition` returns the same way (`:2889-2893`): the order is dropped and the old state kept.
            if !self.is_mobile(id) {
                continue;
            }
            // `setGoalPositionClipped(pos, cmdSource)` (`AIUpdate.cpp:3604`, `:351-380`).
            let dest = self.set_goal_position_clipped(id, pos, source);
            // `AIGroup::groupAttackMoveToPosition` (`AIGroup.cpp:2327`): a member that cannot attack
            // (no weapon, or a passenger whose container forbids firing) gets a plain
            // `aiMoveToPosition` instead.
            if !self.is_able_to_attack(id) || self.objects.get(&id).is_some_and(|o| o.combat.weapons.is_empty()) {
                // `aiMoveToPosition` -> `privateMoveToPosition`'s normal branch
                // (`AIUpdate.cpp:2912`) clears before setting the goal.
                if !self.is_jet(id) {
                    self.ai_clear(id);
                }
                if let Some(o) = self.objects.get_mut(&id) {
                    o.combat.attack = None;
                    o.combat.attack_move = false;
                    o.combat.guard = None;
                    o.combat.hunt = false;
                    o.ai.machine.goal_position = dest;
                    o.ai.machine.goal_object = ObjectId::INVALID;
                    o.ai.machine.group_dest = None;
                    o.ai.machine.last_command_source = source;
                }
                self.ai_command_set_state(id, crate::ai_states::AiStateType::MoveTo);
                continue;
            }
            // `privateAttackMoveToPosition` (`AIUpdate.cpp:3606`) clears the machine before it sets the state, so the
            // old state's `onExit` still sees its attack (`end_attack`) before the new orders overwrite the fields.
            if !self.is_jet(id) {
                self.ai_clear(id);
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.combat.attack = None;
                o.combat.attack_move = true;
                o.combat.guard = None;
                o.combat.hunt = false;
                o.ai.machine.goal_position = dest;
                o.ai.machine.goal_object = ObjectId::INVALID;
                o.ai.machine.group_dest = None;
                o.ai.machine.attack_move_resume = false;
                o.ai.machine.attack_return = crate::ai_states::AiStateType::AttackMoveTo;
                o.ai.machine.last_command_source = source;
            }
            self.ai_command_set_state(id, crate::ai_states::AiStateType::AttackMoveTo);
        }
    }

    /// `AIGroup::groupHunt` / `AIHuntState`: acquire the closest enemy anywhere (ignoring fog).
    pub fn command_hunt(&mut self, player: PlayerIndex, ids: &[ObjectId]) {
        for &id in ids {
            let Some(obj) = self.objects.get(&id) else { continue };
            if obj.destroyed || obj.player != player || obj.ai_handle.is_none() || ignores_orders(obj) {
                continue;
            }
            // `privateHunt` (`AIUpdate.cpp:3662`) clears the machine before it sets `AI_HUNT`: the old state's exit runs first.
            if !self.is_jet(id) {
                self.ai_clear(id);
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.combat.attack = None;
                o.combat.attack_move = false;
                o.combat.guard = None;
                o.combat.hunt = true;
                o.combat.hunt_area = None;
                o.ai.machine.attack_return = crate::ai_states::AiStateType::Hunt;
                o.ai.machine.last_command_source = crate::ai_states::CommandSource::Player;
            }
            self.ai_command_set_state(id, crate::ai_states::AiStateType::Hunt);
        }
    }

    /// `AIGroup::groupAttackArea` / `AIAttackAreaState`: hunt within the area around `center`
    /// (`AIAttackAreaState::update`, `AIStates.cpp:7372`, reads the polygon, not a goal). `groupAttackArea`
    /// (`AIGroup.cpp:2605`) gives every member the same
    /// area, so there is no group offset.
    pub fn command_attack_area(&mut self, player: PlayerIndex, ids: &[ObjectId], center: Coord3D, radius: f32) {
        for &id in ids {
            let Some(obj) = self.objects.get(&id) else { continue };
            if obj.destroyed || obj.player != player || obj.ai_handle.is_none() || ignores_orders(obj) {
                continue;
            }
            // `privateAttackArea` (`AIUpdate.cpp:3671-3680`) clears the machine before it sets `AI_ATTACK_AREA`, and
            // sets no goal position: `AIAttackAreaState` works from the area polygon (`getAreaToGuard`, `AIStates.cpp:7392`).
            if !self.is_jet(id) {
                self.ai_clear(id);
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.combat.attack = None;
                o.combat.attack_move = true;
                o.combat.guard = None;
                o.combat.hunt = true;
                o.combat.hunt_area = Some((center, radius));
                o.ai.machine.goal_object = ObjectId::INVALID;
                o.ai.machine.group_dest = None;
                o.ai.machine.attack_return = crate::ai_states::AiStateType::AttackArea;
                o.ai.machine.last_command_source = crate::ai_states::CommandSource::Player;
            }
            self.ai_command_set_state(id, crate::ai_states::AiStateType::AttackArea);
        }
    }

    /// `MSG_DO_GUARD_*`: hold `target`, engaging enemies within the guard range (`AIGuardState`).
    /// `AIGroup::groupGuardPosition` (`AIGroup.cpp:2545-2559`) gives every member the same point.
    pub(crate) fn command_guard(&mut self, player: PlayerIndex, ids: &[ObjectId], target: GuardTarget, mode: GuardMode) {
        for &id in ids {
            let Some(obj) = self.objects.get(&id) else { continue };
            if obj.destroyed || obj.player != player || obj.ai_handle.is_none() || ignores_orders(obj) {
                continue;
            }
            self.ai_set_guard(id, target, mode, crate::ai_states::CommandSource::Player);
        }
    }

    /// Is `id` a live object of `player` (a member of the player's selection, `AIGroup::removeAnyObjectsNotOwnedByPlayer`)?
    fn is_owned_live(&self, id: ObjectId, player: PlayerIndex) -> bool {
        self.objects.get(&id).is_some_and(|o| !o.destroyed && o.player == player)
    }

    /// `AIGroup::releaseWeaponLockForGroup(LOCKED_TEMPORARILY)` (`AIGroup.cpp:3083`), which the dispatch runs before
    /// a group order: `onDoAttackmoveto`, `onDoForcemoveto`, `onDoMoveto`, `onAddWaypoint`, `onEnter`, `onEvacuate`,
    /// `onInternetHack`, `onDoAttackObject`, `onDoForceAttackObject`, `onDoForceAttackGround`
    /// (`GameLogicDispatch.cpp:1295,1308,1322,1336,1481,1535,1564,1709,1726,1755,1760`). Each owned, live member
    /// of the selection drops its temporary lock. Only player messages pass through the dispatch: a script order
    /// (`CMD_FROM_SCRIPT`) releases nothing.
    pub(crate) fn release_group_temporary_locks(&mut self, player: PlayerIndex, source: CommandSource, ids: &[ObjectId]) {
        if source != CommandSource::Player {
            return;
        }
        for &id in ids {
            if self.is_owned_live(id, player) {
                self.release_weapon_lock(id, WeaponLockType::Temporarily);
            }
        }
    }

    /// `AIGroup::isIdle` (`AIGroup.cpp:3156-3180`): every member with an AI is idle or effectively dead. One busy
    /// member ends it. The members are the owned, live units of the selection.
    pub(crate) fn group_is_idle(&self, player: PlayerIndex, ids: &[ObjectId]) -> bool {
        ids.iter().filter(|&&id| self.is_owned_live(id, player)).all(|&id| {
            let Some(o) = self.objects.get(&id) else { return true };
            o.ai_handle.is_none() || o.effectively_dead || self.ai_is_idle(o)
        })
    }

    /// `GameLogic::onDoForceAttackGround` (`GameLogicDispatch.cpp:1734-1761`). A group that is not idle attacks the
    /// position with its primary weapon: `setWeaponLockForGroup(PRIMARY_WEAPON, LOCKED_TEMPORARILY)`, the attack,
    /// then the release (`AIGroup.cpp:3071,3083`). An idle group releases its temporary locks and attacks. The
    /// idleness is read before either release.
    pub(crate) fn command_force_attack_ground(&mut self, player: PlayerIndex, source: CommandSource, ids: &[ObjectId], pos: Coord3D) {
        if source != CommandSource::Player {
            self.command_attack(player, ids, AttackTarget::Ground(pos));
            return;
        }
        let needs_primary = !self.group_is_idle(player, ids);
        if needs_primary {
            for &id in ids {
                if self.is_owned_live(id, player) {
                    self.set_weapon_lock(id, PRIMARY_WEAPON, WeaponLockType::Temporarily);
                }
            }
            self.command_attack(player, ids, AttackTarget::Ground(pos));
            self.release_group_temporary_locks(player, source, ids);
        } else {
            self.release_group_temporary_locks(player, source, ids);
            self.command_attack(player, ids, AttackTarget::Ground(pos));
        }
    }

    /// `BuildAssistant::sellObject`: refund `SELL_PERCENTAGE` of the build cost, evacuate the
    /// contents and remove the building. (The engine's deconstruction `SlowDeath` timer is not
    /// modelled; the refund is immediate.)
    pub(crate) fn command_sell(&mut self, player: PlayerIndex, building: ObjectId) {
        let Some(o) = self.objects.get(&building) else { return };
        if o.destroyed || o.player != player || !o.is_kind_of(crate::template::kind_of::STRUCTURE) {
            return;
        }
        // `BuildAssistant::sellObject`'s refund is `calcCostToBuild(player)` (BuildAssistant.cpp:249).
        let refund = (self.calc_cost_to_build(player, &o.template) as f32 * SELL_PERCENTAGE) as u32;
        // `BuildAssistant::sellObject` (BuildAssistant.cpp:1580): the queue is refunded when the sale starts,
        // before the contents and the building go (`cancelAndRefundAllProduction`), units and upgrades alike.
        self.refund_all_production(building);
        if let Some(p) = self.players.get_mut(&player) {
            p.deposit(refund);
        }
        // `TunnelContain::onSelling`: only the last tunnel kicks its passengers out.
        if self.is_tunnel_container(building) {
            self.on_tunnel_selling(building);
        } else {
            self.command_evacuate(player, building, crate::ai_states::CommandSource::Player);
        }
        self.destroy_object(building);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai_states::AiStateType;
    use crate::template::{kind_of, LocomotorKind, LocomotorParams};
    use crate::TerrainLogic;
    use std::sync::Arc;

    fn unit() -> Arc<crate::ObjectTemplate> {
        let mut t = crate::ObjectTemplate::new("Tank");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 8.0;
        t.geometry.height = 10.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.vision_range = 200.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
        Arc::new(t)
    }

    fn building() -> Arc<crate::ObjectTemplate> {
        let mut t = crate::ObjectTemplate::new("Bunker");
        t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        t.geometry.major_radius = 20.0;
        t.geometry.height = 20.0;
        t.max_health = 500.0;
        t.initial_health = 500.0;
        t.build_cost = 200;
        Arc::new(t)
    }

    /// `AIGroup::groupMoveToPosition` (`AIGroup.cpp:1734-1743`): the members are sorted
    /// `ITER_SORTED_NEAR_TO_FAR` from the destination and the first one's position is the group
    /// centre that `computeIndividualDestination` (`:514-551`) measures the offsets from — not the
    /// average of the members. Revert check: the old average centre gives the nearest unit 548 and the
    /// middle one 500, and `group_move_order` does not exist.
    #[test]
    fn a_group_move_is_ordered_near_to_far_from_the_nearest_units_position() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let near = g.create_object(&unit(), Coord3D::new(300.0, 100.0, 0.0), 0.0, 0);
        let mid = g.create_object(&unit(), Coord3D::new(200.0, 100.0, 0.0), 0.0, 0);
        let far = g.create_object(&unit(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let dest = Coord3D::new(500.0, 100.0, 0.0);
        // The selection order is deliberately not the near-to-far order.
        let (order, center) = group_move_order(&g, 0, &[far, mid, near], &dest);
        assert_eq!(order, vec![near, mid, far], "nearest the goal first");
        let c = center.expect("a member's position");
        assert_eq!((c.x, c.y), (300.0, 100.0), "the centre is the first unit's position, not the average");

        g.command_move_to_from(0, &[far, mid, near], dest, crate::ai_states::CommandSource::Player, true);
        let goal = |id| g.find_object(id).unwrap().ai.machine.goal_position;
        // `computeIndividualDestination` adjusts each offset point to a free goal cell (AIGroup.cpp:546-548), so the
        // nearest unit's goal is the free cell at or beside the click, not the click itself.
        assert_eq!((goal(near).x, goal(near).y), (500.5, 110.5), "the nearest unit goes to the click's cell: {:?}", goal(near));
        // Offsets from (300,100), clamped to 6 bounding radii (radius 8 -> 48).
        // The offset points are adjusted to goal cells (AIGroup.cpp:546-548): each lies within a cell of its offset
        // point, and the two do not stack on one cell.
        // Traced by hand against the C++ (computeIndividualDestination passes the click as groupDest, AIGroup.cpp:546-548;
        // checkForAdjust's tightenPath, AIPathfind.cpp:8387, walks the line from each candidate to the click's cell (50,10)
        // and keeps the last cell whose footprint is free). The near unit (50,10) tightens to (50,11) = (500.5,110.5). The
        // mid unit's candidate (45,10) walks (45..48,10) and stops before the near unit's footprint at (49,10): (480.5,100.5).
        // The far unit's walk stops before the mid unit's footprint at (47,10): (460.5,100.5). checkPathCost passes both.
        assert_eq!((goal(mid).x, goal(mid).y), (480.5, 100.5), "mid {:?}", goal(mid));
        assert_eq!((goal(far).x, goal(far).y), (460.5, 100.5), "far {:?}", goal(far));
        assert!(goal(mid).dist_2d(&goal(far)) > 5.0, "the offset goals do not stack: {:?} / {:?}", goal(mid), goal(far));
        assert_eq!(g.find_object(far).unwrap().ai.machine.group_dest, Some(dest), "the click is the group goal");
    }

    #[test]
    fn attack_move_sets_flag_and_path() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        let id = g.create_object(&unit(), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
        g.command_attack_move(0, &[id], Coord3D::new(60.0, 10.0, 0.0), CommandSource::Player);
        let o = g.find_object(id).unwrap();
        // `AIGroup::groupAttackMoveToPosition`: a unit that cannot attack (this one has no weapon)
        // gets a plain move.
        assert!(!o.combat.attack_move);
        assert!(o.ai.is_moving());
        let goal = o.ai.goal().expect("has a goal");
        // The click at y = 10 is inside the map but nearer the edge than half a partition cell (20), so
        // `setGoalPositionClipped` (`AIUpdate.cpp:351-380`) pulls the goal to y = 20.
        assert!((goal.x - 60.0).abs() < 2.0 && (goal.y - 20.0).abs() < 2.0, "goal {goal:?} near (60,20)");
    }

    #[test]
    fn guard_sets_point_and_clears_attack_move() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        let id = g.create_object(&unit(), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
        g.command_attack_move(0, &[id], Coord3D::new(60.0, 10.0, 0.0), CommandSource::Player);
        g.command_guard(0, &[id], GuardTarget::Point(Coord3D::new(20.0, 20.0, 0.0)), GuardMode::WithoutPursuit);
        let o = g.find_object(id).unwrap();
        assert!(!o.combat.attack_move);
        assert_eq!(o.combat.guard, Some(GuardTarget::Point(Coord3D::new(20.0, 20.0, 0.0))));
        assert_eq!(o.combat.guard_mode, GuardMode::WithoutPursuit);
        let goal = o.ai.goal().expect("has a goal");
        assert!((goal.x - 20.0).abs() < 2.0 && (goal.y - 20.0).abs() < 2.0, "goal {goal:?} near (20,20)");
    }

    #[test]
    fn sell_refunds_half_and_removes() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        g.add_player(0, 0);
        let id = g.create_object(&building(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        g.command_sell(0, id);
        assert!(g.find_object(id).is_none(), "building removed");
        assert_eq!(g.player(0).unwrap().money, (200.0 * SELL_PERCENTAGE) as u32);
    }

    /// `AIGroup::groupAttackMoveToPosition` and `groupGuardPosition` (`AIGroup.cpp:2327-2340`,
    /// `:2545-2559`) hand the clicked point to every member unchanged. Revert check: the old
    /// `group_unit_dest` offset moves each unit off the click (the nearest unit by about 48 units).
    #[test]
    fn attack_move_and_guard_give_every_member_the_clicked_point() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let a = g.create_object(&unit(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let b = g.create_object(&unit(), Coord3D::new(300.0, 100.0, 0.0), 0.0, 0);
        let p = Coord3D::new(200.0, 200.0, 0.0);
        g.command_attack_move(0, &[a, b], p, CommandSource::Player);
        for id in [a, b] {
            let goal = g.find_object(id).unwrap().ai.machine.goal_position;
            assert!((goal.x - p.x).abs() < 2.0 && (goal.y - p.y).abs() < 2.0, "attack-move goal {goal:?} for {id:?}");
        }
        g.command_guard(0, &[a, b], GuardTarget::Point(p), GuardMode::Normal);
        for id in [a, b] {
            assert_eq!(g.find_object(id).unwrap().combat.guard, Some(GuardTarget::Point(p)), "guard point for {id:?}");
        }
    }

    /// `AIGroup::computeIndividualDestination` (`AIGroup.cpp:534-540`): the destination takes the
    /// height of its layer, `TheTerrainLogic->getLayerHeight(dest.x, dest.y, layer)`. Revert check:
    /// the destination keeps the clicked point's z (0), and the flat map's ground is above it.
    #[test]
    fn a_group_move_destination_takes_the_layer_height() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 50), 1);
        g.add_player(0, 1000);
        let near = g.create_object(&unit(), Coord3D::new(300.0, 100.0, 0.0), 0.0, 0);
        let far = g.create_object(&unit(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let dest = Coord3D::new(500.0, 100.0, 0.0);
        let want = g.terrain.layer_height(500.0, 100.0, crate::bridge::LAYER_GROUND);
        assert!(want > 0.0, "the flat map has ground above zero");
        g.command_move_to_from(0, &[far, near], dest, crate::ai_states::CommandSource::Player, true);
        let z = g.find_object(near).unwrap().ai.machine.goal_position.z;
        assert!((z - want).abs() < 1e-3, "goal z {z} should be the layer height {want}");
    }

    /// `AIGroup::groupMoveToPosition`'s member filter skips `KINDOF_IMMOBILE` (`AIGroup.cpp:1692`).
    /// The immobile unit has a locomotor, so it is an AI object that only the kind check excludes.
    /// Revert check: the immobile unit is in the order.
    #[test]
    fn a_group_move_skips_immobile_members() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let mobile = g.create_object(&unit(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let mut t = crate::ObjectTemplate::new("Turret");
        t.kind_of = kind_of::VEHICLE | kind_of::IMMOBILE | kind_of::SELECTABLE;
        t.geometry.major_radius = 8.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
        let immobile = g.create_object(&Arc::new(t), Coord3D::new(120.0, 100.0, 0.0), 0.0, 0);
        assert!(g.find_object(immobile).unwrap().ai_handle.is_some(), "the immobile unit is an AI object");
        let (order, _) = group_move_order(&g, 0, &[mobile, immobile], &Coord3D::new(500.0, 100.0, 0.0));
        assert_eq!(order, vec![mobile]);
    }

    /// `AIGroup::groupMoveToPosition` clamps the destination into the map trimmed by
    /// `STD_WAYPOINT_CLAMP_MARGIN` (40, `AIGroup.cpp:1557,1655-1656,1560-1585`). Revert check: the
    /// unclamped point (500 units past the east edge) is the goal.
    #[test]
    fn a_group_move_is_clamped_inside_the_map_margin() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let id = g.create_object(&unit(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let (_, hi) = g.terrain.world_extent();
        g.command_move_to_from(0, &[id], Coord3D::new(hi.x + 500.0, 100.0, 0.0), crate::ai_states::CommandSource::Player, true);
        let goal = g.find_object(id).unwrap().ai.machine.goal_position;
        // Traced by hand: the clamped point (hi-40 = 3980,100) is the non-centred cell (398,10), whose adjust is (3980.5,100.5);
        // the tightening walks (398,10)->(398,11) (the degenerate line) and keeps (3980.5,110.5). Inside the margin: the
        // half cell past the clamp line is the cell-centre adjust of the non-centred footprint (AIPathfind.cpp:5328).
        assert_eq!((goal.x, goal.y), (3980.5, 110.5), "goal {goal:?} inside the margin (edge {})", hi.x);
        assert!(goal.x <= hi.x - 40.0 + 5.0, "goal {goal:?} inside the margin (edge {})", hi.x);
        assert!(goal.x >= hi.x - 60.0, "goal {goal:?} is pulled onto the margin, not far inside");
    }

    /// `AIGroup::groupMoveToPosition`'s `tightenGroup` (`AIGroup.cpp:1622-1673`): a player click
    /// inside the group's bounds sends each member to `AI_MOVE_AND_TIGHTEN` at the click
    /// (`groupTightenToPosition`, `AIUpdate.cpp:3167`). Revert check: the members enter `MoveTo`
    /// with their offsets from the centre.
    #[test]
    fn a_click_inside_the_group_tightens_it() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let a = g.create_object(&unit(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let b = g.create_object(&unit(), Coord3D::new(200.0, 100.0, 0.0), 0.0, 0);
        g.command_move_to_from(0, &[a, b], Coord3D::new(150.0, 100.0, 0.0), crate::ai_states::CommandSource::Player, true);
        for id in [a, b] {
            let o = g.find_object(id).unwrap();
            assert_eq!(o.ai.machine.current, AiStateType::MoveAndTighten, "tightened {id:?}");
            assert!((o.ai.machine.goal_position.x - 150.0).abs() < 2.0, "tighten goal {:?}", o.ai.machine.goal_position);
        }
    }

    /// `privateHunt` (`AIUpdate.cpp:3662`) runs `StateMachine::clear` before it sets `AI_HUNT`, so the attack the unit
    /// was making exits first: `AIAttackState::onExit` (`AIStates.cpp:5714`) takes the attack down (the port's
    /// `end_attack` stops the chase's path). Revert check: the order cleared the attack before the exit ran, so the
    /// exit found nothing to end and the chase's move goal survived.
    #[test]
    fn a_hunt_order_exits_the_attack_before_it_takes_over() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        g.add_player(0, 0);
        let id = g.create_object(&unit(), Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
        {
            // The chase is an approach sub-state (`AttackPhase::Approach`), a move (AIStates.cpp:2804).
            let o = g.objects.get_mut(&id).unwrap();
            let mut order = crate::combat::AttackOrder::new(crate::combat::AttackTarget::Ground(Coord3D::new(50.0, 10.0, 0.0)));
            order.phase = crate::combat::AttackPhase::Approach;
            o.combat.attack = Some(order);
            o.ai.machine.current = AiStateType::AttackPosition;
            o.ai.move_goal = Some(Coord3D::new(50.0, 10.0, 0.0));
        }
        g.command_hunt(0, &[id]);
        let o = g.find_object(id).unwrap();
        assert!(o.combat.attack.is_none(), "the attack is gone");
        assert!(o.ai.movement_complete, "the chase's approach exits, ending its move (friend_endingMove)");
        assert_eq!(o.ai.move_goal, None, "the chase's goal goes with its sub-state");
        assert!(o.combat.hunt, "the hunt order is in force");
    }

    /// A unit with a primary and a secondary weapon (the `combat.rs` gunner fixture, two slots).
    fn armed() -> Arc<crate::ObjectTemplate> {
        use crate::weapon::{WeaponSetTemplate, WeaponTemplate};
        let mut t = crate::ObjectTemplate::new("Gunner");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.geometry.major_radius = 8.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.vision_range = 200.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
        let mut w = WeaponTemplate::new("Hit");
        w.primary_damage = 1.0;
        w.attack_range = 100.0;
        let mut set = WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(w.clone()));
        set.weapons[1] = Some(Arc::new(w));
        t.combat.weapon_set = Some(set);
        Arc::new(t)
    }

    /// `GameLogicDispatch`'s `releaseWeaponLockForGroup(LOCKED_TEMPORARILY)` runs before a player's move
    /// (`onDoMoveto`, `GameLogicDispatch.cpp:1322`), so a special attack's temporary lock does not survive the move.
    /// A script move (`CMD_FROM_SCRIPT`) never passes through the dispatch and keeps it. Revert check: without the
    /// release in the `MoveTo` arm the player move leaves the lock on slot 1.
    #[test]
    fn a_player_move_releases_the_temporary_lock_and_a_script_move_does_not() {
        use crate::ai_states::CommandSource;
        use crate::weapon::WeaponLockType;
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let a = g.create_object(&armed(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let dest = Coord3D::new(200.0, 100.0, 0.0);
        g.set_weapon_lock(a, 1, WeaponLockType::Temporarily);
        g.submit_from(0, CommandSource::Script, crate::command::Command::MoveTo { ids: vec![a], pos: dest });
        g.process_command_list();
        assert_eq!(g.find_object(a).unwrap().combat.weapons.temporary_lock, Some(1), "a script order keeps the lock");
        g.submit_from(0, CommandSource::Player, crate::command::Command::MoveTo { ids: vec![a], pos: dest });
        g.process_command_list();
        assert_eq!(g.find_object(a).unwrap().combat.weapons.temporary_lock, None, "the player's move released it");
    }

    /// `onDoAttackObject` releases the group only when the enemy is still there (`GameLogicDispatch.cpp:1703-1709`):
    /// an attack on a gone target does nothing, the lock stays. Revert check: an unconditional release clears it.
    #[test]
    fn an_attack_on_a_gone_target_keeps_the_temporary_lock() {
        use crate::ai_states::CommandSource;
        use crate::weapon::WeaponLockType;
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let a = g.create_object(&armed(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        g.set_weapon_lock(a, 1, WeaponLockType::Temporarily);
        g.submit_from(0, CommandSource::Player, crate::command::Command::Attack { ids: vec![a], target: ObjectId(9999) });
        g.process_command_list();
        assert_eq!(g.find_object(a).unwrap().combat.weapons.temporary_lock, Some(1), "no enemy, no release");
    }

    /// `onDoForceAttackGround` (`GameLogicDispatch.cpp:1734-1761`) releases the temporary lock in both branches: a busy
    /// group sets the primary weapon's lock for the attack and then releases, an idle group releases first. Revert
    /// check: dropping the release at the end of the busy branch leaves slot 1 locked.
    #[test]
    fn a_force_attack_on_the_ground_releases_the_temporary_lock_when_busy_and_idle() {
        use crate::ai_states::AiStateType;
        use crate::weapon::WeaponLockType;
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let busy = g.create_object(&armed(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let idle = g.create_object(&armed(), Coord3D::new(100.0, 200.0, 0.0), 0.0, 0);
        let target = Coord3D::new(150.0, 150.0, 0.0);
        g.objects.get_mut(&busy).unwrap().ai.machine.current = AiStateType::MoveTo;
        for id in [busy, idle] {
            g.set_weapon_lock(id, 1, WeaponLockType::Temporarily);
        }
        g.command_force_attack_ground(0, crate::ai_states::CommandSource::Player, &[busy], target);
        assert_eq!(g.find_object(busy).unwrap().combat.weapons.temporary_lock, None, "the busy group released its lock");
        g.command_force_attack_ground(0, crate::ai_states::CommandSource::Player, &[idle], target);
        assert_eq!(g.find_object(idle).unwrap().combat.weapons.temporary_lock, None, "the idle group released its lock");
    }

    /// `MSG_SWITCH_WEAPONS` -> `setWeaponLockForGroup(slot, LOCKED_PERMANENTLY)` (`GameLogicDispatch.cpp:1067`,
    /// `WeaponSet.cpp:1078`): the permanent lock replaces a temporary one. Revert check: assigning the permanent slot
    /// directly leaves the temporary lock on slot 0.
    #[test]
    fn switching_weapons_replaces_a_temporary_lock_with_the_permanent_one() {
        use crate::weapon::WeaponLockType;
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let a = g.create_object(&armed(), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        g.set_weapon_lock(a, 0, WeaponLockType::Temporarily);
        g.command_switch_weapons(0, &[a], 1);
        let w = &g.find_object(a).unwrap().combat.weapons;
        assert_eq!((w.permanent_lock, w.temporary_lock), (Some(1), None), "the permanent lock holds slot 1 alone");
    }

    #[test]
    fn sell_ignores_other_players() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        g.add_player(0, 0);
        g.add_player(1, 0);
        let id = g.create_object(&building(), Coord3D::new(50.0, 50.0, 0.0), 0.0, 1);
        g.command_sell(0, id);
        assert!(g.find_object(id).is_some());
        assert_eq!(g.player(0).unwrap().money, 0);
    }
}
