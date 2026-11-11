//! Named-unit and waypoint-path script actions (`ScriptActions.cpp`): `doNamedAttack`,
//! `doNamedDelete`, `doNamedKill`, `doNamedSetHeld`, `doNamedExitAll`, `doNamedMoveToWaypoint`,
//! `doNamedFollowWaypoints`, `doTeamFollowWaypoints` and `doTeamPanic`. The waypoint paths walk the
//! map's link graph (`AIFollowWaypointPathState`, see `ai_states.rs`).

use crate::ai_states::{CommandSource, WalkAt};
use crate::disabled::{DisabledType, FOREVER, NEVER};
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::ObjectId;
use crate::template::LocomotorSet;

use super::ScriptEngine;

/// The average position of the live members (`doTeamFollowWaypoints`' team centre), `None` when empty.
fn team_centre(logic: &GameLogic, ids: &[ObjectId]) -> Option<Coord3D> {
    let (mut x, mut y, mut z, mut n) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for id in ids {
        if let Some(o) = logic.find_object(*id) {
            x += o.position.x;
            y += o.position.y;
            z += o.position.z;
            n += 1.0;
        }
    }
    (n > 0.0).then(|| Coord3D::new(x / n, y / n, z / n))
}

impl ScriptEngine {
    /// `ScriptEngine::getUnitNamed` (ScriptEngine.cpp:5993): the script-named object while it is in the
    /// world (`notifyOfObjectDestruction` removes it from the cache).
    pub(crate) fn live_named(&self, logic: &GameLogic, name: &str) -> Option<ObjectId> {
        self.named_object(name).filter(|id| logic.find_object(*id).is_some_and(|o| !o.destroyed))
    }

    /// A live script-named object that has an AI (`getAIUpdateInterface`).
    fn named_with_ai(&self, logic: &GameLogic, name: &str) -> Option<ObjectId> {
        self.live_named(logic, name).filter(|id| logic.find_object(*id).is_some_and(|o| o.ai_handle.is_some()))
    }

    /// The walk state and location of waypoint `wp`: the position, and whether it has a link to follow.
    /// The graph is read here, not in `GameLogic`, because the engine is taken out of the logic while its
    /// scripts run.
    pub(crate) fn walk_start(&self, wp: i32) -> Option<(WalkAt, Coord3D)> {
        let pos = self.waypoint_by_id.get(&wp)?.pos;
        let open = self.waypoint_links.get(&wp).is_some_and(|l| !l.is_empty());
        Some((WalkAt { waypoint: wp, open }, pos))
    }

    /// `ScriptActions::doNamedAttack` (ScriptActions.cpp:1063): the attacker takes
    /// `chooseLocomotorSet(NORMAL)` and `aiForceAttackObject(victim, NO_MAX_SHOTS_LIMIT)` (`leaveGroup`
    /// has no sim equivalent: the sim keeps no script groups).
    pub(crate) fn named_attack_named(&mut self, logic: &mut GameLogic, attacker: &str, victim: &str) {
        let (Some(a), Some(v)) = (self.named_with_ai(logic, attacker), self.live_named(logic, victim)) else {
            return;
        };
        logic.ai_choose_locomotor_set(a, LocomotorSet::Normal);
        logic.end_attack(a);
        logic.ai_attack_object(a, v, true, false, None);
    }

    /// `ScriptActions::doNamedDelete` (ScriptActions.cpp:2400): `TheGameLogic->destroyObject`.
    pub(crate) fn named_delete(&mut self, logic: &mut GameLogic, unit: &str) {
        if let Some(id) = self.live_named(logic, unit) {
            logic.destroy_object(id);
        }
    }

    /// `ScriptActions::doNamedKill` (ScriptActions.cpp:2553): `Object::kill`.
    pub(crate) fn named_kill(&mut self, logic: &mut GameLogic, unit: &str) {
        if let Some(id) = self.live_named(logic, unit) {
            logic.kill_object(id);
        }
    }

    /// `ScriptActions::doNamedSetHeld` (ScriptActions.cpp:3965): `setDisabledUntil(DISABLED_HELD,
    /// held ? FOREVER : NEVER)`.
    pub(crate) fn named_set_held(&mut self, logic: &mut GameLogic, unit: &str, held: bool) {
        if let Some(id) = self.live_named(logic, unit) {
            logic.set_disabled_until(id, DisabledType::Held, if held { FOREVER } else { NEVER });
        }
    }

    /// `ScriptActions::doNamedExitAll` (ScriptActions.cpp:1581): `chooseLocomotorSet(NORMAL)` then
    /// `aiEvacuate(FALSE)`, which `privateEvacuate` skips while the unit is subdued (AIUpdate.cpp:3922).
    pub(crate) fn named_exit_all(&mut self, logic: &mut GameLogic, unit: &str) {
        let Some(id) = self.named_with_ai(logic, unit) else { return };
        logic.ai_choose_locomotor_set(id, LocomotorSet::Normal);
        if logic.find_object(id).is_none_or(|o| o.is_disabled_by_type(DisabledType::Subdued)) {
            return;
        }
        let contained = logic.contained_units(id).to_vec();
        logic.exit_passengers(id, &contained);
    }

    /// `ScriptActions::doNamedMoveToWaypoint` (ScriptActions.cpp:435): `chooseLocomotorSet(NORMAL)` and
    /// `aiMoveToPosition` the waypoint, from the script.
    pub(crate) fn named_move_to_waypoint(&mut self, logic: &mut GameLogic, unit: &str, waypoint: &str) {
        let Some(id) = self.live_named(logic, unit) else { return };
        let Some(dest) = self.waypoints.get(waypoint).copied() else { return };
        if logic.find_object(id).is_none_or(|o| o.ai_handle.is_none()) {
            return;
        }
        logic.ai_choose_locomotor_set(id, LocomotorSet::Normal);
        logic.ai_move_to_position_cmd(id, dest, CommandSource::Script);
    }

    /// `ScriptActions::doNamedFollowWaypoints` (ScriptActions.cpp:1643): the waypoint on the path label
    /// closest to the unit, then `chooseLocomotorSet(NORMAL)` and `aiFollowWaypointPath` from it.
    pub(crate) fn named_follow_waypoints(&mut self, logic: &mut GameLogic, unit: &str, label: &str) {
        let Some(id) = self.named_with_ai(logic, unit) else { return };
        let Some(pos) = logic.find_object(id).map(|o| o.position) else { return };
        let Some(wp) = self.closest_waypoint_on_path(&pos, label) else { return };
        let Some((at, at_pos)) = self.walk_start(wp) else { return };
        logic.ai_choose_locomotor_set(id, LocomotorSet::Normal);
        logic.ai_follow_waypoint(id, at, at_pos, CommandSource::Script);
    }

    /// `ScriptActions::doTeamFollowWaypoints` (ScriptActions.cpp:1816): the waypoint on the path label
    /// closest to the team's centre, then every member follows the links from it
    /// (`groupFollowWaypointPath`, no locomotor change). The as-team variant's group speed and offsets
    /// are not modelled (see `ai_follow_waypoint`). `key` is the team instance key.
    pub(crate) fn team_follow_waypoints(&mut self, logic: &mut GameLogic, key: &str, label: &str) {
        if !self.teams.contains_key(key) {
            return;
        }
        let ids = self.team_members(logic, key);
        let Some(center) = team_centre(logic, &ids) else { return };
        let Some(wp) = self.closest_waypoint_on_path(&center, label) else { return };
        let Some((at, at_pos)) = self.walk_start(wp) else { return };
        for id in ids {
            logic.ai_follow_waypoint(id, at, at_pos, CommandSource::Script);
        }
    }

    /// `ScriptActions::doTeamPanic` (ScriptActions.cpp:2524): each member with an AI takes
    /// `chooseLocomotorSet(PANIC)` and `aiPanic` from the waypoint on the path label closest to itself. A
    /// member with no such waypoint ends the action. Members are visited newest first, as the team's
    /// member list is (`Object::setTeam` prepends).
    pub(crate) fn team_panic(&mut self, logic: &mut GameLogic, key: &str, label: &str) {
        if !self.teams.contains_key(key) {
            return;
        }
        for id in self.team_members(logic, key).into_iter().rev() {
            let Some(o) = logic.find_object(id) else { continue };
            if o.ai_handle.is_none() {
                continue;
            }
            let pos = o.position;
            let Some(wp) = self.closest_waypoint_on_path(&pos, label) else { return };
            let Some((at, at_pos)) = self.walk_start(wp) else { return };
            logic.ai_choose_locomotor_set(id, LocomotorSet::Panic);
            logic.ai_panic_waypoint(id, at, at_pos, CommandSource::Script);
        }
    }
}
