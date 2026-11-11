//! The exit door of a container: `OpenContain::exitObjectViaDoor` (OpenContain.cpp:1013-1111) and what calls it.
//!
//! * `AIExitState::update` / `AIExitInstantlyState::update` (AIStates.cpp:6480, :6612) reserve the door and exit a
//!   passenger (`ai_exit_state_update`), gated by the container's `isExitBusy` (`TransportContain::isExitBusy`,
//!   TransportContain.cpp:649-655) and `reserveDoorForExit` (`TransportContain::isSpecificRiderFreeToExit`, :570-612).
//! * `exit_object_via_door` is the class dispatch: `GarrisonContain::exitObjectViaDoor` (GarrisonContain.cpp:1498) has
//!   its own burst-from-centre exit; `RailedTransportContain` unloads through its dock (not ported); every other class
//!   takes `OpenContain`'s (TransportContain, TunnelContain, HealContain, MobNexusContain, ... declare no override).
//! * `SpawnBehavior::createSpawn` leaves a tunnel network through the same exit (SpawnBehavior.cpp:727-728).
//!
//! The `OpenContain` exit places the unit at the model's `ExitStart` bone with the container's orientation, pathfinds it to
//! `ExitEnd` (adjusted to a possible destination) and sends it along `aiFollowPath(&exitPath, me, CMD_FROM_AI)`
//! (`AI_FOLLOW_PATH`, the container ignored as an obstacle, AI.h:561). A container with no exit path only puts the unit back
//! in the pathfinder (OpenContain.cpp:1109-1111).

use std::sync::Arc;

use crate::ai_states::{AiStateType, StateReturn};
use crate::contain::{ContainKind, ContainTemplate};
use crate::math::{cos, sin, LOGICFRAMES_PER_SECOND};
use crate::pathfind::PathObject;
use crate::production::model_to_world;
use crate::template::kind_of;
use crate::{Coord3D, GameLogic, ObjectId, ObjectTemplate};

impl GameLogic {
    /// The contain module of `id` (shared with the template).
    fn contain_of(&self, id: ObjectId) -> Option<Arc<ContainTemplate>> {
        self.objects.get(&id)?.template.contain.clone()
    }

    /// Whether a container exits through `OpenContain::exitObjectViaDoor` with no class override, and so may spawn
    /// through it. A tunnel network is the shipped spawner that does (`GLATunnelNetwork`'s `SpawnBehavior`).
    pub(crate) fn has_open_contain_exit(&self, container: ObjectId) -> bool {
        self.contain_of(container).is_some_and(|c| c.kind == ContainKind::Tunnel)
    }

    /// `OpenContain`'s exit bones (OpenContain.cpp:1036-1057): the `ExitStart` and `ExitEnd` of the next exit path, in
    /// world space. With several paths the bones are `ExitStart01`.. and the choice cycles (`m_whichExitPath`). A bone the
    /// model lacks reads as the container's own position (`Object::getSingleLogicalBonePosition`, Object.cpp:6115).
    /// `None` when the container has no exit path (`numberExits == 0`).
    fn open_exit_points(&mut self, container: ObjectId) -> Option<(Coord3D, Coord3D)> {
        let (cpos, cangle) = self.objects.get(&container).map(|o| (o.position, o.angle))?;
        let ct = self.contain_of(container)?;
        let paths = ct.number_of_exit_paths.max(0) as u32;
        if paths == 0 {
            return None;
        }
        let next = self.objects.get(&container).map_or(0, |o| o.behavior.open_exit_next);
        let k = if paths > 1 { next % paths } else { 0 };
        if paths > 1 {
            if let Some(o) = self.objects.get_mut(&container) {
                o.behavior.open_exit_next = (k + 1) % paths;
            }
        }
        let bones = ct.fire_points.exit_bones.get(k as usize).copied().unwrap_or((None, None));
        let to_world = |bone: Option<Coord3D>| bone.map_or(cpos, |b| model_to_world(&cpos, cangle, (b.x, b.y, b.z)));
        Some((to_world(bones.0), to_world(bones.1)))
    }

    /// `SpawnBehavior::createSpawn` for a tunnel container: the unit is created at the exit's start bone and given the
    /// container's orientation (`exitObj->setPosition` / `setOrientation`, OpenContain.cpp:1073-1074), then
    /// `Player::onUnitCreated` (SpawnBehavior.cpp:635), its producer is the container (`:665`) and it leaves through the
    /// exit. Returns the new object's id.
    pub(crate) fn spawn_through_open_contain_exit(&mut self, container: ObjectId, tt: &Arc<ObjectTemplate>) -> ObjectId {
        let Some((cpos, cangle, cplayer)) = self.objects.get(&container).map(|o| (o.position, o.angle, o.player)) else {
            return ObjectId::INVALID;
        };
        let points = self.open_exit_points(container);
        // No exit path: the unit stays where `newObject` put it (the container's position here).
        let start = points.map_or(cpos, |(s, _)| s);
        let sid = self.create_object(tt, start, cangle, cplayer);
        // `Player::onUnitCreated(parent, newSpawn)` (SpawnBehavior.cpp:635), then `newSpawn->setProducer(parent)` (`:665`).
        self.unit_created(container, sid, cplayer);
        if let Some(o) = self.objects.get_mut(&sid) {
            o.producer_id = container;
        }
        // `SpawnBehavior::createSpawn`'s `onEnslave` before the exit (SpawnBehavior.cpp:657-666): the container is the
        // new unit's master.
        self.slaved_on_enslave(sid, container);
        if let Some((_, end)) = points {
            self.open_contain_exit_at(sid, container, end, cangle);
        }
        sid
    }

    /// `OpenContain::exitObjectViaDoor` (OpenContain.cpp:1013-1111) for a unit the container holds, after the class
    /// dispatch in [`exit_object_via_door`](Self::exit_object_via_door). The unit is removed from the container
    /// (`removeFromContain`, :1018), then placed at the exit start.
    fn open_contain_exit_via_door(&mut self, container: ObjectId, unit: ObjectId) {
        // `removeFromContain(exitObj)`: the class's `onRemoving` hooks, without placement. A rider that died in its
        // `RiderChangeContain::onRemoving` is gone.
        if !self.contain_remove_unit(container, unit, false) {
            return;
        }
        let Some((_, cangle)) = self.objects.get(&container).map(|o| (o.position, o.angle)) else { return };
        match self.open_exit_points(container) {
            Some((start, end)) => {
                // `exitObj->setPosition(&startPosition)`, `setOrientation(exitAngle)` (:1073-1074). The position
                // change re-registers the unit in the pathfinder (`addObjectToPathfindMap`, :1078).
                if let Some(o) = self.objects.get_mut(&unit) {
                    o.set_orientation(cangle);
                }
                self.place_removed_unit(unit, start);
                self.open_contain_exit_at(unit, container, end, cangle);
            }
            None => {
                // `else { TheAI->pathfinder()->addObjectToPathfindMap(exitObj); }` (:1109-1111): the unit stays put.
                let pos = self.objects.get(&unit).map_or(Coord3D::ZERO, |o| o.position);
                self.place_removed_unit(unit, pos);
            }
        }
    }

    /// The AI part of `exitObjectViaDoor` (OpenContain.cpp:1080-1108) for a unit that is already at its exit start:
    /// the idle-vehicle container step (:1080-1084), `ignoreObstacle(nullptr)` and one second of ignored collisions
    /// (:1085-1089), `adjustToPossibleDestination` on the end (:1096), the exit path `[end, end]` (:1099-1101; the rally
    /// point of `getRallyPoint`, :1094-1096, is nullptr in the retail branch, see the note at the path) followed with the
    /// container ignored as an obstacle (`aiFollowPath`),
    /// `updateGoal`, all with `setAllowToFall(false)` held across the path call (:1100-1110).
    fn open_contain_exit_at(&mut self, unit: ObjectId, container: ObjectId, end: Coord3D, _angle: f32) {
        let has_ai = self.objects.get(&unit).is_some_and(|o| o.ai_handle.is_some());
        if !has_ai {
            return;
        }
        // `if (myAi && myAi->isIdle() && me->isKindOf(KINDOF_VEHICLE))` (:1080-1084): the container's own footprint is
        // taken out of the pathfinder and put back at its position with a goal there.
        let idle_vehicle = self
            .objects
            .get(&container)
            .is_some_and(|o| o.ai_handle.is_some() && o.ai.machine.current == AiStateType::Idle && o.is_kind_of(kind_of::VEHICLE));
        if idle_vehicle {
            if let Some(po) = self.objects.get(&container).and_then(PathObject::for_object) {
                let cpos = self.objects[&container].position;
                self.pathfinder.clear_unit(container);
                self.pathfinder.update_unit_pos(&po, &cpos, false);
                let doing = self.is_doing_ground_movement(container);
                self.pathfinder.update_goal_dispatch(&po, &cpos, doing);
            }
        }
        // `ai->ignoreObstacle(nullptr)`, `ai->setIgnoreCollisionTime(LOGICFRAMES_PER_SECOND)` (:1085-1089).
        self.ai_set_ignore_obstacle(unit, ObjectId::INVALID);
        let until = self.frame + LOGICFRAMES_PER_SECOND;
        if let Some(o) = self.objects.get_mut(&unit) {
            o.ai.ignore_collisions_until = until;
        }
        // `TheAI->pathfinder()->adjustToPossibleDestination(exitObj, ai->getLocomotorSet(), &endPosition)` (:1096).
        let mut dest = end;
        if let Some(po) = self.objects.get(&unit).and_then(PathObject::for_object) {
            let from = self.objects[&unit].position;
            let objects = &self.objects;
            let _ = self.pathfinder.adjust_to_possible_destination(&self.terrain, &po, &from, &mut dest, &|u| {
                objects.get(&ObjectId(u)).map(|o| o.player)
            });
        }
        // `exitPath`: the end point twice, "in case units stack up due to brief flying" (:1099-1101). `if (rallyPoint)
        // exitPath.push_back(*rallyPoint)` (:1101) adds nothing: `OpenContain::getRallyPoint` (OpenContain.cpp:1670-1686)
        // returns only the container's own `m_rallyPoint`, which no shipped caller sets (AIPlayer.cpp:507, :687 are gated
        // on `gotOffset`), else nullptr. The TheSuperHackers branch (the primary exit's rally point, a factory's
        // `production.rally_point`) is `#if !RETAIL_COMPATIBLE_CRC`; the reference build compiles the retail branch,
        // and the policy is recorded in LOG (lane 163, R17-5).
        let path = vec![dest, dest];
        let can_fall = self.objects.get(&unit).and_then(|o| o.physics.as_ref()).map(|p| p.allow_to_fall);
        if can_fall.is_some() {
            self.set_allow_to_fall(unit, false);
        }
        // `ai->aiFollowPath(&exitPath, me, CMD_FROM_AI)` (:1103): `AI_FOLLOW_PATH`, the container is the ignored obstacle.
        self.ai_set_follow_path(unit, path, AiStateType::FollowPath, Some(container), None);
        // `TheAI->pathfinder()->updateGoal(exitObj, &endPosition, layer)` (:1107).
        if let Some(po) = self.objects.get(&unit).and_then(PathObject::for_object) {
            let doing = self.is_doing_ground_movement(unit);
            self.pathfinder.update_goal_dispatch(&po, &dest, doing);
        }
        if let Some(fall) = can_fall {
            self.set_allow_to_fall(unit, fall);
        }
        self.wake_up_now(unit);
    }

    /// `GarrisonContain::exitObjectViaDoor` (GarrisonContain.cpp:1498-1650), the burst-from-centre disposition that
    /// `GarrisonContain` starts with (`m_evacDisposition = EVAC_BURST_FROM_CENTER`, :536). The evacuate-to-left/right
    /// dispositions need `setEvacDisposition` state the port does not have, so they are not ported. The start is the
    /// container's centre, moved one major radius out of the way of an impassable spot (the cliff bunker case, :1516-1531).
    fn garrison_exit_via_door(&mut self, container: ObjectId, unit: ObjectId) {
        if !self.contain_remove_unit(container, unit, false) {
            return;
        }
        let Some((cpos, cangle, radius)) = self.objects.get(&container).map(|o| (o.position, o.angle, o.template.geometry.major_radius)) else {
            return;
        };
        let enclosing = self.is_enclosing_garrison(container);
        let has_ai = self.objects.get(&unit).is_some_and(|o| o.ai_handle.is_some());
        let mut start = cpos;
        if has_ai {
            if let Some(loco) = self.objects.get(&unit).and_then(|o| o.locomotor()) {
                let surfaces = PathObject::surfaces_for(&loco);
                let valid = |p: &Coord3D| crate::locomotor::valid_movement_terrain(&self.pathfinder, surfaces, p);
                if !valid(&cpos) {
                    // "try front & back" (:1521-1530): one radius back, else one radius forward, else the centre.
                    let back = Coord3D::new(cpos.x - radius * cos(cangle), cpos.y - radius * sin(cangle), cpos.z);
                    start = if valid(&back) {
                        back
                    } else {
                        let front = Coord3D::new(cpos.x + radius * cos(cangle), cpos.y + radius * sin(cangle), cpos.z);
                        if valid(&front) {
                            front
                        } else {
                            cpos
                        }
                    };
                }
            }
        }
        // EVAC_BURST_FROM_CENTER (:1609-1636): an enclosing garrison puts the unit at the start on the ground; the
        // orientation is the container's, and the unit is registered in the pathfinder.
        if let Some(o) = self.objects.get_mut(&unit) {
            o.set_orientation(cangle);
        }
        let here = if enclosing {
            Coord3D::new(start.x, start.y, self.terrain.ground_height(start.x, start.y))
        } else {
            self.objects.get(&unit).map_or(start, |o| o.position)
        };
        self.place_removed_unit(unit, here);
        if has_ai {
            let mut end = start;
            if let Some(po) = self.objects.get(&unit).and_then(PathObject::for_object) {
                let from = self.objects[&unit].position;
                let objects = &self.objects;
                let _ = self.pathfinder.adjust_to_possible_destination(&self.terrain, &po, &from, &mut end, &|u| {
                    objects.get(&ObjectId(u)).map(|o| o.player)
                });
            }
            // `ai->aiFollowPath(&exitPath, getObject(), CMD_FROM_AI)` (:1625).
            self.ai_set_follow_path(unit, vec![end], AiStateType::FollowPath, Some(container), None);
            if let Some(po) = self.objects.get(&unit).and_then(PathObject::for_object) {
                let doing = self.is_doing_ground_movement(unit);
                self.pathfinder.update_goal_dispatch(&po, &end, doing);
            }
            self.wake_up_now(unit);
        }
    }

    /// `OpenContain::exitObjectViaDoor` through its class dispatch: a garrison has its own exit, a railed transport
    /// unloads through its dock (`RailedTransportContain::exitObjectViaDoor`, not ported: it keeps the port's placement), and
    /// every other container takes the `OpenContain` exit.
    pub(crate) fn exit_object_via_door(&mut self, container: ObjectId, unit: ObjectId) {
        match self.contain_of(container).map(|c| c.kind) {
            Some(ContainKind::Garrison) => self.garrison_exit_via_door(container, unit),
            Some(ContainKind::RailedTransport) => self.remove_from_contain(container, unit),
            _ => self.open_contain_exit_via_door(container, unit),
        }
    }

    /// `TransportContain::isExitBusy` (TransportContain.cpp:649-655): a transport is busy until `m_frameExitNotBusy`.
    /// `DelayExitInAir` (`isDelayExitInAir && isAboveTerrain`) is not ported (the sim's contain template has no field).
    /// The other containers answer FALSE (`OpenContain`'s default).
    pub(crate) fn container_exit_busy(&self, container: ObjectId) -> bool {
        let Some(o) = self.objects.get(&container) else { return false };
        match o.template.contain.as_ref() {
            // `RiderChangeContain::isExitBusy` returns FALSE (`RiderChangeContain.cpp:414-418`, "return FALSE; //return
            // TransportContain::isExitBusy();"): a bike's riders do not wait out the ExitDelay window.
            Some(c) if c.is_transport() && c.kind != ContainKind::RiderChange => self.frame < o.behavior.frame_exit_not_busy,
            _ => false,
        }
    }

    /// `reserveDoorForExit` (`AIExitState::update`, AIStates.cpp:6509): a transport asks whether this rider may leave
    /// (`TransportContain::isSpecificRiderFreeToExit`); every other container's door is always `DOOR_1`.
    fn reserve_exit_door(&self, container: ObjectId, rider: ObjectId) -> bool {
        match self.contain_of(container) {
            Some(c) if c.is_transport() => self.is_specific_rider_free_to_exit(container, rider),
            _ => true,
        }
    }

    /// `TransportContain::isSpecificRiderFreeToExit` (TransportContain.cpp:570-612) under `RETAIL_COMPATIBLE_CRC` (1, the
    /// reference build): an airborne container may always unload, otherwise the rider needs an AI and a current locomotor
    /// that can stand on the container's cell. The "container is itself contained, not free to exit" test is
    /// `#if !RETAIL_COMPATIBLE_CRC` (TransportContain.cpp:579-585) and is not applied (R17-5). `ai->getAiFreeToExit`
    /// (`Chinook`, `DeliverPayload` overrides) is not ported.
    fn is_specific_rider_free_to_exit(&self, container: ObjectId, rider: ObjectId) -> bool {
        if self.is_airborne_locomotor(container) {
            return true;
        }
        let Some(r) = self.objects.get(&rider) else { return false };
        if r.ai_handle.is_none() {
            return false;
        }
        let Some(loco) = r.locomotor() else { return false };
        let Some(c) = self.objects.get(&container) else { return false };
        crate::locomotor::valid_movement_terrain(&self.pathfinder, PathObject::surfaces_for(&loco), &c.position)
    }

    /// `AIExitState::update` (AIStates.cpp:6480-6523) and `AIExitInstantlyState::update` (:6612-6619): the exit state of a
    /// passenger. The plain exit waits while the container is busy, reserves its door and exits; it stays in the state
    /// (and waits) until the exit has moved the unit into `AI_FOLLOW_PATH`. The instant exit has already left in its
    /// `onEnter`, so its update only reports whether the state changed.
    pub(crate) fn ai_exit_state_update(&mut self, unit: ObjectId, container: ObjectId) -> StateReturn {
        if self.objects.get(&unit).is_some_and(|o| o.ai.machine.current == AiStateType::ExitInstantly) {
            return if self.objects.get(&unit).is_some_and(|o| o.ai.machine.current != AiStateType::ExitInstantly) {
                StateReturn::Continue
            } else {
                StateReturn::Success
            };
        }
        // `Object* goal = getMachineGoalObject(); if (goal) ... else return STATE_FAILURE` (AIStates.cpp:6484-6490). A
        // dead or destroyed container still exits its passenger: `if (goal->isEffectivelyDead()) return STATE_FAILURE;`
        // is `#if !RETAIL_COMPATIBLE_CRC` (AIStates.cpp:6500-6504) and the reference build compiles the retail branch (R17-5).
        if !self.objects.contains_key(&container) {
            return StateReturn::Failure;
        }
        if self.container_exit_busy(container) {
            return StateReturn::Continue;
        }
        if !self.reserve_exit_door(container, unit) {
            return StateReturn::Failure;
        }
        self.exit_object_via_door(container, unit);
        if self.objects.get(&unit).is_some_and(|o| o.ai.machine.current != AiStateType::Exit) {
            StateReturn::Continue
        } else {
            StateReturn::Success
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contain::GarrisonFirePoints;
    use crate::modules2::SpawnTemplate;
    use crate::template::{LocomotorKind, LocomotorParams, SimpleTemplateStore};
    use crate::{Command, GameRules, TerrainLogic};

    /// `GLATunnelNetwork` (`UBUndTunn`, one exit path): `ExitStart` (3.5225, -0.0779, -0.5001) and `ExitEnd` (34.4925,
    /// -0.0132, 0.0) in model space, the install's bind pose. The C++ defenders leave at (1509.7949, 1958.1681, 18.2499)
    /// with the tunnel at (1512.3408, 1960.6038, 18.75) (`zr-refs` AH7 s1 f1906).
    fn tunnel_template(spawn_of: Option<&str>) -> ObjectTemplate {
        let mut t = container_template("GLATunnelNetwork", ContainKind::Tunnel, -1, 1.0);
        if let Some(name) = spawn_of {
            t.spawn = Some(SpawnTemplate {
                spawn_number: 2,
                spawn_replace_delay: 9999,
                one_shot: false,
                spawn_template_names: vec![name.to_string()],
                spawned_require_spawner: false,
                initial_burst: 0,
                aggregate_health: false,
            });
        }
        t
    }

    /// A container with one exit path whose bones are the tunnel's, with the given contain kind and capacity.
    fn container_template(name: &str, kind: ContainKind, capacity: i32, exit_delay_frames: f32) -> ObjectTemplate {
        let _ = exit_delay_frames;
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
        t.geometry.major_radius = 20.0;
        t.max_health = 1000.0;
        t.initial_health = 1000.0;
        t.transport_slot_count = 0;
        t.contain = Some(Arc::new(ContainTemplate {
            kind,
            capacity,
            allow_inside_kind_of: None,
            forbid_inside_kind_of: 0,
            allow_allies_inside: true,
            allow_enemies_inside: true,
            allow_neutral_inside: true,
            passengers_allowed_to_fire: false,
            damage_percentage_to_units: 0.0,
            number_of_exit_paths: 1,
            exit_delay: 0,
            door_open_time: 1,
            scatter_nearby_on_exit: true,
            orient_like_container_on_exit: false,
            reset_mood_check_on_exit: false,
            health_regen: 0.0,
            do_i_heal_objects: false,
            heal_frames: 0.0,
            initial_payload: Vec::new(),
            tunnel_frames_for_full_heal: 30.0,
            is_enclosing_container: kind == ContainKind::Garrison,
            fire_points: GarrisonFirePoints {
                exit_bones: vec![(Some(Coord3D::new(3.5225, -0.0779, -0.5001)), Some(Coord3D::new(34.4925, -0.0132, 0.0)))],
                ..GarrisonFirePoints::default()
            },
            passengers_in_turret: false,
            weapon_bonus_passed_to_passengers: false,
            immune_to_clear_building_attacks: false,
        }));
        t
    }

    fn infantry_template(name: &str) -> ObjectTemplate {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
        t.geometry.major_radius = 4.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.transport_slot_count = 1;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
        t
    }

    /// Two defenders leave a tunnel facing `angle`: both start at the rotated `ExitStart` bone and walk the two-point
    /// exit path to the rotated `ExitEnd` bone in `AI_FOLLOW_PATH` (C++ f1906: `ai=FollowPath`, both at one point).
    /// Revert check: without the spawn branch (`modules2.rs::spawn_create_spawn`) the defenders are placed on the 12-unit ring.
    #[test]
    fn tunnel_spawns_leave_at_the_exit_bone_and_follow_the_exit_path() {
        let mut store = SimpleTemplateStore::default();
        let defender = store.add(infantry_template("GLAInfantryTunnelDefender"));
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        g.set_template_source(Arc::new(store));
        let angle = -2.361f32;
        let tunnel_pos = Coord3D::new(1512.3408, 1960.6038, 18.75);
        let mut t = tunnel_template(Some(&defender.name));
        t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::SELECTABLE;
        // A tunnel built in play: its first update finds both entries past (`currentTime > replacementTime`,
        // SpawnBehavior.cpp:249), so both defenders leave at once. A tunnel placed at the very start would trickle its
        // second defender in one SPAWN_UPDATE_RATE later (the entry for index 1 is not past yet, SpawnBehavior.cpp:230-233).
        g.run_frames(2);
        let tunnel = g.create_object(&Arc::new(t), tunnel_pos, angle, 0);
        let mut spawns = Vec::new();
        for _ in 0..40 {
            g.run_frames(1);
            spawns = g.objects.values().filter(|o| o.template.name == defender.name).map(|o| o.id).collect();
            if spawns.len() == 2 {
                break;
            }
        }
        assert_eq!(spawns.len(), 2, "the first batch of two defenders");
        let start = model_to_world(&tunnel_pos, angle, (3.5225, -0.0779, -0.5001));
        let end = model_to_world(&tunnel_pos, angle, (34.4925, -0.0132, 0.0));
        for &s in &spawns {
            let o = &g.objects[&s];
            assert_eq!(o.producer_id, tunnel, "`newSpawn->setProducer(parent)`");
            assert!(o.position.dist_2d(&start) < 1.0, "left at the exit bone: {:?} vs {:?}", o.position, start);
            assert_eq!(o.ai.machine.current, AiStateType::FollowPath, "AI_FOLLOW_PATH, not idle");
            // `adjustToPossibleDestination` snaps `ExitEnd` to a pathfinding cell (`AIPathfind.cpp:5649`).
            let goal = o.ai.machine.goal_path.last().copied().expect("exit path");
            assert!(goal.dist_2d(&end) < 10.0, "walks to ExitEnd: goal {:?} vs {:?}", goal, end);
        }
        assert!(g.objects[&spawns[0]].position.dist_2d(&g.objects[&spawns[1]].position) < 1.0);
    }

    /// `SpawnBehavior::createSpawn`'s `onEnslave` (SpawnBehavior.cpp:660-663) runs `SlavedUpdate::startSlavedEffects`
    /// (SlavedUpdate.cpp:703-716) for a spawn with a `SlavedUpdate`: its first draw is the pinned point's direction,
    /// `GameLogicRandomValue( 0, 2*PI )` (the integer range 0..6). Lane 159 (AH7 s1 draw 1699). Two identical
    /// worlds, one with a slaved defender: the slaved spawn leaves the stream exactly one draw further on. Revert
    /// check: without the call in `spawn_through_open_contain_exit` the two streams are equal.
    #[test]
    fn a_slaved_tunnel_spawn_draws_its_pinned_point_direction_once() {
        let spawn_with = |slaved: bool| {
            let mut store = SimpleTemplateStore::default();
            let mut d = infantry_template("GLAInfantryTunnelDefender");
            if slaved {
                d.slaved = Some(crate::modules2::SlavedTemplate {
                    guard_max_range: 0,
                    guard_wander_range: 0,
                    attack_range: 0,
                    repair_rate_per_second: 0.0,
                    stay_on_same_layer_as_master: false,
                });
            }
            let defender = store.add(d);
            let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
            g.set_template_source(Arc::new(store));
            let tunnel = g.create_object(&Arc::new(tunnel_template(None)), Coord3D::new(1000.0, 1000.0, 0.0), 0.0, 0);
            let before = g.rng_mut().clone();
            let sid = g.spawn_through_open_contain_exit(tunnel, &defender);
            assert_ne!(sid, ObjectId::INVALID, "a spawn was made");
            (before, g.rng_mut().clone())
        };
        let (before, plain) = spawn_with(false);
        let (before_slaved, slaved) = spawn_with(true);
        assert_eq!(before, before_slaved, "the two worlds start from the same stream");
        let mut expected = plain.clone();
        let _ = expected.value(0, 6);
        assert_eq!(slaved, expected, "the slaved spawn draws `GameLogicRandomValue(0, 2*PI)` once more than the plain one");
    }

    /// A tunnel passenger that is ordered out (`AIExitState`) leaves at the exit bone and walks the exit path. Revert check:
    /// with the old placement the passenger stands on the 12-unit-plus ring around the tunnel instead of the bone.
    #[test]
    fn a_tunnel_passenger_leaves_at_the_exit_bone() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        g.set_rules(GameRules { max_tunnel_capacity: 2, ..GameRules::default() });
        let tunnel_pos = Coord3D::new(1000.0, 1000.0, 0.0);
        let tunnel = g.create_object(&Arc::new(tunnel_template(None)), tunnel_pos, 0.0, 0);
        let u = g.create_object(&Arc::new(infantry_template("GLAInfantryRebel")), Coord3D::new(1025.0, 1000.0, 0.0), 0.0, 0);
        g.submit(0, Command::Enter { ids: vec![u], container: tunnel });
        g.run_frames(60);
        assert!(g.is_contained(u), "the passenger boarded");
        g.command_exit(0, tunnel, &[u]);
        g.run_frames(2);
        assert!(!g.is_contained(u), "the passenger is out");
        let start = model_to_world(&tunnel_pos, 0.0, (3.5225, -0.0779, -0.5001));
        let o = &g.objects[&u];
        assert!(o.position.dist_2d(&start) < 3.0, "at the exit bone {:?} vs {:?}", o.position, start);
        assert_eq!(o.ai.machine.current, AiStateType::FollowPath);
    }

    /// A garrison's burst exit puts an enclosed unit at the building's centre on the ground (GarrisonContain.cpp:1609-1636),
    /// not on a ring around it. Revert check: the old ring places it about 25 units out.
    #[test]
    fn a_garrison_burst_exit_starts_at_the_centre() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        let center = Coord3D::new(1000.0, 1000.0, 0.0);
        let garrison = g.create_object(&Arc::new(container_template("GLAGarrison", ContainKind::Garrison, 5, 0.0)), center, 0.0, 0);
        let u = g.create_object(&Arc::new(infantry_template("GLAInfantryRebel")), Coord3D::new(1025.0, 1000.0, 0.0), 0.0, 0);
        g.submit(0, Command::Enter { ids: vec![u], container: garrison });
        g.run_frames(60);
        assert!(g.is_contained(u), "the unit garrisoned");
        g.command_exit(0, garrison, &[u]);
        g.run_frames(2);
        assert!(!g.is_contained(u), "the unit is out");
        let o = &g.objects[&u];
        assert!(o.position.dist_2d(&center) < 3.0, "burst from the centre: {:?}", o.position);
        assert_eq!(o.ai.machine.current, AiStateType::FollowPath);
    }

    /// A container with a production exit leaves its passengers along `[end, end]` only (OpenContain.cpp:1094-1101): under
    /// `RETAIL_COMPATIBLE_CRC` (1), `OpenContain::getRallyPoint` (OpenContain.cpp:1670-1686) returns the container's own
    /// `m_rallyPoint` or nullptr, so the primary exit's rally point (the factory's `production.rally_point`) is not a node.
    /// Revert check: restore the TheSuperHackers primary-exit branch (`production.rally_point` pushed as the third node) and
    /// the path has three nodes.
    #[test]
    fn a_passenger_leaves_a_factory_style_container_without_its_primary_exit_rally_point() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        let mut ct = container_template("GLAOpenWithExit", ContainKind::Open, 3, 0.0);
        ct.production = Some(crate::production::ProductionTemplate::default());
        let c = g.create_object(&Arc::new(ct), Coord3D::new(1000.0, 1000.0, 0.0), 0.0, 0);
        let u = g.create_object(&Arc::new(infantry_template("GLAInfantryRebel")), Coord3D::new(1025.0, 1000.0, 0.0), 0.0, 0);
        g.submit(0, Command::Enter { ids: vec![u], container: c });
        g.run_frames(60);
        assert!(g.is_contained(u), "the passenger boarded");
        let rally = Coord3D::new(1100.0, 1100.0, 0.0);
        g.objects.get_mut(&c).unwrap().production.as_mut().expect("a production state").rally_point = Some(rally);
        g.exit_object_via_door(c, u);
        let path = &g.objects[&u].ai.machine.goal_path;
        assert_eq!(path.len(), 2, "end, end (no rally node under the retail branch): {path:?}");
        assert!(!path.contains(&rally), "the factory's rally point is not on the exit path: {path:?}");
    }

    /// `AIExitState::update` (AIStates.cpp:6480-6523) under the retail branch: `if (goal->isEffectivelyDead()) return
    /// STATE_FAILURE;` is `#if !RETAIL_COMPATIBLE_CRC` (AIStates.cpp:6500-6504), so a passenger of a dead container still
    /// leaves through the exit. Revert check: restore the dead-goal test and the state fails with the passenger still in.
    #[test]
    fn a_dead_containers_passenger_still_leaves_through_the_exit() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        let c = g.create_object(&Arc::new(container_template("GLAOpenDying", ContainKind::Open, 3, 0.0)), Coord3D::new(1000.0, 1000.0, 0.0), 0.0, 0);
        let u = g.create_object(&Arc::new(infantry_template("GLAInfantryRebel")), Coord3D::new(1025.0, 1000.0, 0.0), 0.0, 0);
        g.submit(0, Command::Enter { ids: vec![u], container: c });
        g.run_frames(60);
        assert!(g.is_contained(u), "the passenger boarded");
        g.objects.get_mut(&c).unwrap().effectively_dead = true;
        let r = g.ai_exit_state_update(u, c);
        assert_ne!(r, StateReturn::Failure, "the exit continues for a dead container");
        assert!(!g.is_contained(u), "the passenger is out");
    }

    /// `TransportContain::isSpecificRiderFreeToExit` (TransportContain.cpp:570-612) under the retail branch: the
    /// "container is itself contained, so not free to exit" rule is `#if !RETAIL_COMPATIBLE_CRC` (:579-585). A rider of a
    /// transport that is inside another container is free to leave the transport when the ground rules allow it. Revert
    /// check: restore the contained test and the rider is not free to exit.
    #[test]
    fn a_rider_of_a_contained_transport_is_free_to_exit() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        let t = g.create_object(&Arc::new(container_template("GLATransport", ContainKind::Transport, 3, 0.0)), Coord3D::new(1000.0, 1000.0, 0.0), 0.0, 0);
        let u = g.create_object(&Arc::new(infantry_template("GLAInfantryRebel")), Coord3D::new(1025.0, 1000.0, 0.0), 0.0, 0);
        g.submit(0, Command::Enter { ids: vec![u], container: t });
        g.run_frames(60);
        assert!(g.is_contained(u), "the passenger boarded");
        g.objects.get_mut(&t).unwrap().contained_by = Some(ObjectId(999));
        assert!(g.is_specific_rider_free_to_exit(t, u), "the retail branch does not block a contained transport");
    }

    /// `HealContain::update` (HealContain.cpp:113-118) ejects a healed passenger through the exit: it leaves at the exit
    /// bone on `AI_FOLLOW_PATH`. Revert check: with the old `remove_from_contain` the passenger keeps the ring placement.
    #[test]
    fn a_healed_passenger_leaves_through_the_exit_bone() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        let mut ct = container_template("GLAHealBunker", ContainKind::Heal, 2, 0.0);
        if let Some(c) = ct.contain.as_mut().and_then(Arc::get_mut) {
            c.heal_frames = 1.0;
        }
        let center = Coord3D::new(1000.0, 1000.0, 0.0);
        let c = g.create_object(&Arc::new(ct), center, 0.0, 0);
        let u = g.create_object(&Arc::new(infantry_template("GLAInfantryRebel")), Coord3D::new(1025.0, 1000.0, 0.0), 0.0, 0);
        // Boarding is not what this test covers, so the unit is added directly (`OpenContain::addToContain`).
        g.add_to_contain(c, u);
        // Fully healed on the first update, which ejects it (`doHeal` then `reserveDoorForExit` + `exitObjectViaDoor`).
        g.run_frames(3);
        assert!(!g.is_contained(u), "healed and ejected");
        let start = model_to_world(&center, 0.0, (3.5225, -0.0779, -0.5001));
        assert!(g.objects[&u].position.dist_2d(&start) < 1.0, "at the exit bone: {:?}", g.objects[&u].position);
        assert_eq!(g.objects[&u].ai.machine.current, AiStateType::FollowPath);
    }

    /// A transport releases one passenger per `ExitDelay` (`m_frameExitNotBusy`, TransportContain.cpp:410, :649-655): the
    /// second rider waits for the busy window the first one opened. Revert check: without the busy gate both riders leave on
    /// the same frame.
    #[test]
    fn a_transport_releases_one_passenger_per_exit_delay() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        let mut tt = container_template("GLATransport", ContainKind::Transport, 3, 0.0);
        if let Some(c) = tt.contain.as_mut().and_then(Arc::get_mut) {
            c.exit_delay = 30;
        }
        let transport = g.create_object(&Arc::new(tt), Coord3D::new(1000.0, 1000.0, 0.0), 0.0, 0);
        let a = g.create_object(&Arc::new(infantry_template("GLAInfantryRebel")), Coord3D::new(1025.0, 1000.0, 0.0), 0.0, 0);
        let b = g.create_object(&Arc::new(infantry_template("GLAInfantryRebel")), Coord3D::new(1025.0, 1010.0, 0.0), 0.0, 0);
        g.submit(0, Command::Enter { ids: vec![a, b], container: transport });
        g.run_frames(90);
        assert!(g.is_contained(a) && g.is_contained(b), "both boarded");
        g.command_exit(0, transport, &[a, b]);
        g.run_frames(3);
        let out = [a, b].iter().filter(|&&u| !g.is_contained(u)).count();
        assert_eq!(out, 1, "one leaves at once, the other waits for the busy window");
        let busy_until = g.objects[&transport].behavior.frame_exit_not_busy;
        assert!(busy_until > g.frame(), "the transport is busy until {busy_until}");
        g.run_frames(busy_until - g.frame() + 2);
        assert!(!g.is_contained(a) && !g.is_contained(b), "the second leaves when the window closes");
    }

    /// `TransportContain::onRemoving` (TransportContain.cpp:386-391): a rider leaving a transport that is above the ground is
    /// marked as allowed to fall (R17-6). `exitObjectViaDoor` reads the flag before its path call and restores it after
    /// (OpenContain.cpp:1069-1110), so the rider falls after the exit. Revert check: remove the allow-to-fall step from
    /// `contain_remove_unit` and the flag stays false.
    #[test]
    fn a_rider_of_an_airborne_transport_is_allowed_to_fall_on_removal() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        let t = g.create_object(&Arc::new(container_template("GLATransport", ContainKind::Transport, 3, 0.0)), Coord3D::new(1000.0, 1000.0, 0.0), 0.0, 0);
        let mut rt = infantry_template("GLAInfantryRebel");
        rt.physics = Some(crate::physics::PhysicsTemplate { mass: 10.0, ..Default::default() });
        let u = g.create_object(&Arc::new(rt), Coord3D::new(1025.0, 1000.0, 0.0), 0.0, 0);
        g.submit(0, Command::Enter { ids: vec![u], container: t });
        g.run_frames(60);
        assert!(g.is_contained(u), "the passenger boarded");
        g.objects.get_mut(&t).unwrap().position.z = 20.0;
        assert!(g.contain_remove_unit(t, u, false), "the rider is released");
        assert!(g.objects[&u].physics.as_ref().unwrap().allow_to_fall, "the rider may fall from an airborne transport");
    }

    /// `RiderChangeContain::isExitBusy` returns FALSE (RiderChangeContain.cpp:414-418), so a bike's rider does not wait out
    /// the ExitDelay window that `TransportContain::onRemoving` opens (R17-10). Revert check: drop the RiderChange exclusion
    /// from `container_exit_busy` and the bike reads as busy while its window is open.
    #[test]
    fn a_rider_change_bike_is_never_exit_busy() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        let mut tt = container_template("GLABike", ContainKind::RiderChange, 1, 0.0);
        if let Some(c) = tt.contain.as_mut().and_then(Arc::get_mut) {
            c.exit_delay = 30;
        }
        let bike = g.create_object(&Arc::new(tt), Coord3D::new(1000.0, 1000.0, 0.0), 0.0, 0);
        let until = g.frame() + 30;
        g.objects.get_mut(&bike).unwrap().behavior.frame_exit_not_busy = until;
        assert!(!g.container_exit_busy(bike), "a rider change bike is never busy");
    }
}
