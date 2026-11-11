//! Script-created units and reinforcement teams: `ScriptActions::createUnitOnTeamAt`
//! (ScriptActions.cpp:1164) and `ScriptActions::doCreateReinforcements` (ScriptActions.cpp:499).
//!
//! Not modelled: the parachute drop of a `DeliverPayloadAIUpdate` transport (`putInContainerTemplate`
//! and `deliverPayloadViaModuleData`), and the team's group movement lock-step (`AIGroup` is not used).

use crate::command::Command;
use crate::ai_states::CommandSource;
use crate::disabled::DisabledType;
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::ObjectId;
use crate::template::{kind_of, LocomotorSet};

use super::team::partition_fast;
use super::ScriptEngine;

impl ScriptEngine {
    /// The team's live members, newest first: `Team::prependTo_TeamMemberList` puts each new member at
    /// the head (`Object.cpp:898`), and the script loops iterate from there. `Team::members` keeps
    /// that order, so the list is used as is.
    fn team_members_newest(&mut self, logic: &GameLogic, key: &str) -> Vec<ObjectId> {
        self.team_members(logic, key)
    }

    /// `ScriptActions::createUnitOnTeamAt` (ScriptActions.cpp:1164): a unit of `obj_type` joins `team`
    /// (a team with no instance is created and activated) at `waypoint`, under `unit_name` (empty for an
    /// unnamed unit). A live unit already named `unit_name` makes the action fail.
    pub(crate) fn create_unit_on_team_at(
        &mut self,
        logic: &mut GameLogic,
        unit_name: &str,
        obj_type: &str,
        team: &str,
        waypoint: &str,
    ) {
        let existing = if unit_name.is_empty() { None } else { self.named_object(unit_name) };
        // `ScriptActions.cpp:1168`: a unit named `unit_name` that is not `isEffectivelyDead()` blocks
        // the action (the port's `is_dying()` is health 0 or the die pipeline).
        if existing.and_then(|id| logic.find_object(id)).is_some_and(|o| !o.is_dying() && !o.destroyed) {
            return;
        }
        let resolved = self.resolve_team(team);
        let key = if self.teams.contains_key(&resolved) {
            resolved
        } else {
            // `TheTeamFactory->createTeam`: an inactive instance, then `setActive`.
            let Some(k) = self.create_team(&resolved) else { return };
            self.on_team_created(logic, &k);
            k
        };
        let Some(template) = logic.find_template(obj_type) else { return };
        // `setPosition(waypoint)` only when the waypoint exists; the new object is at the origin otherwise.
        let pos = self.waypoints.get(waypoint).copied().unwrap_or(Coord3D::ZERO);
        // `TheThingFactory->newObject(thingTemplate, theTeam)` (`ScriptActions.cpp:1195`). The engine is
        // handed in so the new unit's owner and team membership are set at creation (the same call
        // reads the team again below).
        let id = logic.create_object_in_team_with(&template, pos, 0.0, &key, self);
        if !unit_name.is_empty() {
            self.add_named_object(unit_name, id);
        }
    }

    /// `ScriptActions::doCreateReinforcements` (ScriptActions.cpp:499). The prototype's units appear at its
    /// reinforcement origin (`teamReinforcementOrigin`, else `waypoint`), in columns, with the transport
    /// first. With `teamStartsFull` the units are loaded into the team's transports by the partition
    /// solver. Each unit left over goes into a transport with room (a new transport when the last is
    /// full) or walks to `waypoint`. A team with no transport is activated and walks to `waypoint` from
    /// its origin; a transport team is activated when its transport finishes unloading.
    pub(crate) fn create_reinforcements(&mut self, logic: &mut GameLogic, team: &str, waypoint: &str) {
        let proto_name = team.to_ascii_lowercase();
        let Some(proto) = self.team_protos.get(&proto_name).cloned() else { return };
        let Some(destination) = self.waypoints.get(waypoint).copied() else { return };
        // `m_startReinforceWaypoint`: where the units appear; they walk to `destination` when it differs.
        let mut origin = destination;
        let mut need_move = false;
        if let Some(start) = self.waypoints.get(&proto.start_reinforce_waypoint).copied() {
            origin = start;
            need_move = origin.x != destination.x || origin.y != destination.y;
        }
        // `TeamFactory::createInactiveTeam` (a singleton reuses its instance).
        let Some(key) = self.create_team(&proto_name) else { return };
        let Some(owner) = self.team_player(&key) else { return };

        // The transport first (`newObject(transportTemplate, theTeam)` at the origin, facing 0).
        let transport_template = if proto.transport.is_empty() { None } else { logic.find_template(&proto.transport) };
        let mut transport: Option<ObjectId> = None;
        if let Some(t) = &transport_template {
            let id = logic.create_object_in_team_with(t, origin, 0.0, &key, self);
            transport = Some(id);
        }

        // The units: each slot's `maxUnits` copies in a row (`2.25 * radius` apart), and the next slot
        // starts `2 * radius` further along y (ScriptActions.cpp:521-560).
        for slot in &proto.units {
            let mut pos = origin;
            if let Some(unit) = logic.find_template(&slot.template) {
                let radius = unit.geometry.major_radius;
                let mut made = false;
                for j in 0..slot.max.max(0) {
                    let x = origin.x + 2.25 * (j as f32) * radius;
                    let z = logic.terrain().ground_height(x, origin.y);
                    logic.create_object_in_team_with(&unit, Coord3D::new(x, origin.y, z), 0.0, &key, self);
                    made = true;
                }
                if made {
                    pos.y += 2.0 * radius;
                }
            }
            origin.y = pos.y;
        }

        origin = destination;
        if proto.starts_full {
            // `PartitionSolver` (PREFER_FAST_SOLUTION) over the team's units and its other transports
            // (ScriptActions.cpp:579-642): every solved pair is `addToContain`ed.
            let mut units: Vec<(ObjectId, u32)> = Vec::new();
            let mut spaces: Vec<(ObjectId, u32)> = Vec::new();
            for id in self.team_members_newest(logic, &key) {
                if Some(id) == transport {
                    continue;
                }
                let Some(o) = logic.find_object(id) else { continue };
                if o.is_kind_of(kind_of::TRANSPORT) {
                    if let Some(c) = o.template.contain.as_ref() {
                        let max = if c.capacity < 0 { u32::MAX } else { c.capacity as u32 };
                        spaces.push((id, max));
                    }
                } else {
                    // A unit with no slot count takes "lots" of room (`slots = 0x7fffff`).
                    let slots = if o.template.transport_slot_count == 0 { 0x7fffff } else { o.template.transport_slot_count };
                    units.push((id, slots));
                }
            }
            for (unit, trans) in partition_fast(&units, &spaces) {
                logic.add_to_contain(trans, unit);
            }
        }

        // The leftover units go into the transport (ScriptActions.cpp:644-697): a unit it can take joins
        // it, starting a new transport when the current one is full; a unit that does not fit walks to
        // the destination unless it is held. Transports and contained units are skipped.
        if let (Some(mut tr), Some(tt)) = (transport, transport_template.clone()) {
            if logic.find_object(tr).is_some_and(|o| o.template.contain.is_some()) {
                let transport_name = tt.name.clone();
                let mut transport_count = 1i32;
                for id in self.team_members_newest(logic, &key) {
                    let Some(o) = logic.find_object(id) else { continue };
                    if o.template.name == transport_name || o.contained_by.is_some() {
                        continue;
                    }
                    if logic.is_valid_container_for(tr, id, false) {
                        if !logic.is_valid_container_for(tr, id, true) {
                            // Full: a new transport, `pos.x = origin.x + transportCount * radius`.
                            let radius = logic.find_object(tr).map_or(0.0, |t| t.template.geometry.major_radius);
                            let x = origin.x + transport_count as f32 * radius;
                            let at = Coord3D::new(x, origin.y, logic.terrain().ground_height(x, origin.y));
                            let next = logic.create_object_in_team_with(&tt, at, 0.0, &key, self);
                            transport_count += 1;
                            tr = next;
                            transport = Some(next);
                        }
                        logic.add_to_contain(tr, id);
                    } else if o.ai_handle.is_some() && !o.is_disabled_by_type(DisabledType::Held) {
                        logic.ai_choose_locomotor_set(id, LocomotorSet::Normal);
                        logic.ai_move_to_position_cmd(id, destination, CommandSource::Script);
                    }
                }
            }
        }

        // The team dispatch (ScriptActions.cpp:774-800). With a transport, each transport unloads (or drops
        // its payload) at the destination and the other members walk there unless held; the team is
        // activated later by the transport's `MoveAndEvacuate` (`ai_move_and_evacuate_update`). Without one,
        // the team is activated now and walks to the destination as a group when it has to.
        match transport {
            Some(_) => {
                let transport_name = transport_template.as_ref().map(|t| t.name.clone()).unwrap_or_default();
                for id in self.team_members_newest(logic, &key) {
                    let Some(o) = logic.find_object(id) else { continue };
                    let has_ai = o.ai_handle.is_some();
                    if o.template.name == transport_name {
                        if has_ai {
                            logic.ai_choose_locomotor_set(id, LocomotorSet::Normal);
                            if proto.transports_exit {
                                logic.ai_move_and_evacuate_and_exit(id, destination);
                            } else {
                                logic.ai_move_and_evacuate(id, destination);
                            }
                        }
                    } else if has_ai && !o.is_disabled_by_type(DisabledType::Held) {
                        logic.ai_choose_locomotor_set(id, LocomotorSet::Normal);
                        logic.ai_move_to_position_cmd(id, destination, CommandSource::Script);
                    }
                }
            }
            None => {
                // `theTeam->setActive()`.
                if !self.teams.get(&key).is_some_and(|t| t.created) {
                    self.on_team_created(logic, &key);
                }
                if need_move {
                    let ids = self.team_members(logic, &key);
                    logic.submit(owner, Command::MoveTo { ids, pos: destination });
                }
            }
        }
    }
}
