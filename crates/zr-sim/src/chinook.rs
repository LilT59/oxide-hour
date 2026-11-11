//! `ChinookAIUpdate`'s combat drop / rappel state machine
//! (`GameLogic/Object/Update/AIUpdate/ChinookAIUpdate.cpp` + the `AIRappelState` half of
//! `GameLogic/AI/AIStates.cpp`).
//!
//! Ported: `privateCombatDrop` starts `MOVE_TO_COMBAT_DROP`; `ChinookMoveToBldgState` flies the
//! Chinook to a hover height of `building height + MinDropHeight` (or `MinDropHeight` for a ground
//! drop); `ChinookCombatDropState` initialises one rope per `NumRopes`, extends each rope with
//! gravity capped at `RopeDropSpeed` (`RopeFinalHeight` above the ground), drops a
//! `KINDOF_CAN_RAPPEL` passenger per rope every `PerRopeDelayMin..Max` frames (`WaitForRopesToDrop`
//! makes a rope wait until it is fully extended), and finishes when no rope is in use and no
//! passenger remains. Each rappeller descends at `RappelSpeed` (capped at `|Gravity| * 30 * 2.5`)
//! to the ground / building top, kills up to two enemies inside the target building and enters it
//! (`AIRappelState`).
//!
//! Reductions (documented): the client rope drawable / `RopeStart`/`RopeEnd` bone transform is not
//! simulated (client rope drawing is out of scope), so the ropes are seeded from points spread
//! along the Chinook's belly and only the timing/lengths are tracked. The rope `lenMax` uses the
//! hover height instead of the exact `RopeEnd` bone. The rappeller is moved directly rather than
//! through `PhysicsBehavior` (infantry have no physics module in the sim). `findPositionAround`
//! (scattering a rappeller when the target building is full) is reduced to a point south of the
//! building.

use std::collections::BTreeMap;

use crate::logic::GameLogic;
use crate::math::{cos, sin, Coord3D, LOGICFRAMES_PER_SECOND};
use crate::object::ObjectId;
use crate::template::kind_of;

/// `ChinookAIUpdateModuleData` (already per frame / frames).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChinookTemplate {
    /// `NumRopes`.
    pub num_ropes: u32,
    /// `RopeDropSpeed`, units per frame.
    pub rope_drop_speed: f32,
    /// `RappelSpeed`, units per frame.
    pub rappel_speed: f32,
    /// `PerRopeDelayMin` / `PerRopeDelayMax`, frames.
    pub per_rope_delay_min: u32,
    pub per_rope_delay_max: u32,
    /// `MinDropHeight`.
    pub min_drop_height: f32,
    /// `WaitForRopesToDrop`.
    pub wait_for_ropes_to_drop: bool,
    /// `RopeFinalHeight`.
    pub rope_final_height: f32,
}

impl Default for ChinookTemplate {
    fn default() -> Self {
        ChinookTemplate {
            num_ropes: 4,
            rope_drop_speed: 1.0e10,
            rappel_speed: 1.0,
            // The constructor's `0x7fffffff` (effectively never).
            per_rope_delay_min: i32::MAX as u32,
            per_rope_delay_max: i32::MAX as u32,
            min_drop_height: 30.0,
            wait_for_ropes_to_drop: true,
            rope_final_height: 0.0,
        }
    }
}

/// `ChinookAIStateType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChinookPhase {
    /// `MOVE_TO_COMBAT_DROP`.
    MoveToDrop,
    /// `DO_COMBAT_DROP`.
    CombatDrop,
}

/// One rope (`ChinookCombatDropState::RopeInfo`, the sim half).
#[derive(Clone, Debug)]
pub struct RopeState {
    /// World position of the rope's top (the `RopeStart` bone; approximated here).
    pub start: Coord3D,
    /// `ropeLen` / `ropeLenMax`.
    pub len: f32,
    pub len_max: f32,
    /// `ropeSpeed` (units per frame).
    pub speed: f32,
    /// `nextDropTime`.
    pub next_drop_time: u32,
    /// `rappellerIDs`.
    pub rappellers: Vec<ObjectId>,
}

/// The Chinook combat-drop state for one object, exposed to the viewer.
#[derive(Clone, Debug)]
pub struct ChinookState {
    pub phase: ChinookPhase,
    /// `getMachineGoalPosition` (the drop position, x/y of the target and the hover z).
    pub target_pos: Coord3D,
    /// `getMachineGoalObject` (the building to drop into, if any).
    pub target_obj: Option<ObjectId>,
    /// The hover height the Chinook flies to.
    pub dest_z: f32,
    pub ropes: Vec<RopeState>,
    /// The drop finished (`DO_COMBAT_DROP` returned `STATE_SUCCESS`).
    pub done: bool,
}

/// A rappeller descending a rope (`AIRappelState`).
#[derive(Clone, Debug)]
pub struct RappelState {
    /// `m_destZ`: the ground (or the building's top).
    pub dest_z: f32,
    /// `m_rappelRate` (a negative per-frame speed).
    pub rate: f32,
    /// `getMachineGoalObject`.
    pub target_obj: Option<ObjectId>,
}

impl GameLogic {
    /// The viewer/script trigger for a combat drop (`AIUpdateInterface::privateCombatDrop`).
    /// `target_obj` is the building to drop into (`None` for a ground drop).
    pub fn start_combat_drop(&mut self, chinook: ObjectId, target_pos: Coord3D, target_obj: Option<ObjectId>) -> bool {
        let Some((min_drop_height, bounding_radius)) = self.objects.get(&chinook).and_then(|o| {
            let t = o.template.chinook?;
            if o.destroyed || o.is_dying() || o.template.contain.is_none() {
                return None;
            }
            Some((t.min_drop_height, o.template.geometry.bounding_circle_radius()))
        }) else {
            return false;
        };
        // `privateCombatDrop` (ChinookAIUpdate.cpp:1266-1274): a drop at a position (no building) goes to the first free
        // spot near it, so the Chinook does not drop onto a structure's footprint or into the fog. The search starts at a
        // random angle (`startAngle` default, PartitionManager.cpp:3996-3998) and reaches 100 bounding radii.
        // The commanded z stays the goal's z (`setGoalPositionClipped(pos)`, ChinookAIUpdate.cpp:1212-1222); the drop's
        // altitude is `m_destZ`, carried by `drop_move_z` (R24-6).
        let commanded_z = target_pos.z;
        let target_pos = if target_obj.is_none() {
            let opts = crate::dock::FindPositionOptions { max_radius: bounding_radius * 100.0, ..Default::default() };
            self.find_position_around(&target_pos, &opts).unwrap_or(target_pos)
        } else {
            target_pos
        };
        // The drop replaces a supply dock sequence in progress (halting it cancels the reservation).
        if self.objects.get(&chinook).is_some_and(|o| o.ai.machine.current == crate::ai_states::AiStateType::Dock) {
            self.ai_idle(chinook, crate::ai_states::CommandSource::Script);
        }
        // `ChinookMoveToBldgState::onEnter` (ChinookAIUpdate.cpp:726-754). `m_oldPreferredHeight` is the locomotor's current
        // preferred height. A live structure sets `max height + MinDropHeight`, never below the old height (:737-746); a
        // ground drop keeps the old height (:749-752). `m_destZ` is the ground at the destination plus that height (:754).
        let old_pref = self.objects.get(&chinook).map_or(0.0, |o| o.locomotor().map_or(0.0, |l| l.preferred_height));
        let bldg = target_obj
            .and_then(|b| self.objects.get(&b))
            .filter(|o| !o.is_dying() && !o.effectively_dead && o.is_kind_of(kind_of::STRUCTURE));
        let (dest_xy, new_pref) = match bldg {
            Some(b) => {
                let height = b.template.geometry.max_height_above_position() + min_drop_height;
                (b.position, if height < old_pref { old_pref } else { height })
            }
            None => (target_pos, old_pref),
        };
        let dest_z = self.terrain.ground_height(dest_xy.x, dest_xy.y) + new_pref;
        // The goal keeps the commanded z (R24-6, `setGoalPositionClipped(pos)`, ChinookAIUpdate.cpp:1212-1222); the move's own
        // height is `dest_z` (`m_destZ`, :754). The Chinook state's `target_pos` keeps the drop height (a port field).
        let goal = Coord3D::new(dest_xy.x, dest_xy.y, commanded_z);
        let state_target = Coord3D::new(dest_xy.x, dest_xy.y, dest_z);
        // `setMyState(MOVE_TO_COMBAT_DROP, target, &localPos, cmdSource)` (ChinookAIUpdate.cpp:1219-1226, :1275): the drop is
        // the main state. The machine is cleared, the goal object and position are set, and the state is entered, so a
        // Chinook busy with another move does not keep that move in charge (no temporary, R23-6).
        self.ai_clear(chinook);
        if let Some(o) = self.objects.get_mut(&chinook) {
            o.ai.machine.goal_object = target_obj.unwrap_or(ObjectId::INVALID);
            o.ai.machine.goal_position = goal;
            o.ai.machine.last_command_source = crate::ai_states::CommandSource::Ai;
            // `ChinookMoveToBldgState::onEnter`'s own settings, made before the enter so the move sees them: the ultra-accurate
            // flight (:731), the new preferred height (:748) and the height the move waits for (:766-776).
            o.loco_rt.ultra_accurate = true;
            o.preferred_height_override = Some(new_pref);
            o.ai.machine.drop_move_z = Some(dest_z);
            o.ai.speed = 0.0;
            o.supply_target = None;
            o.boxes = 0;
        }
        self.ai_command_set_state(chinook, crate::ai_states::AiStateType::MoveTo);
        self.chinooks.insert(
            chinook,
            ChinookState { phase: ChinookPhase::MoveToDrop, target_pos: state_target, target_obj, dest_z, ropes: Vec::new(), done: false },
        );
        self.wake_up_now(chinook);
        true
    }

    /// The current combat-drop state of a Chinook, for the viewer.
    pub fn chinook_state(&self, id: ObjectId) -> Option<&ChinookState> {
        self.chinooks.get(&id)
    }

    /// The rappeller state of a descending passenger, for the viewer.
    pub fn rappel_state(&self, id: ObjectId) -> Option<&RappelState> {
        self.rappels.get(&id)
    }

    /// How many passengers are currently descending a rope, for the viewer.
    pub fn rappelling_count(&self) -> usize {
        self.rappels.len()
    }

    pub(crate) fn update_one_chinook(&mut self, id: ObjectId) {
        let Some(mut st) = self.chinooks.remove(&id) else { return };
        let Some(alive) = self.objects.get(&id).map(|o| !o.destroyed && !o.is_dying()) else {
            return;
        };
        // `ChinookAIUpdate` is an `AIUpdateInterface`: its first update is one frame after creation
        // (`AIUpdate.cpp:299-301`).
        if self.objects.get(&id).is_some_and(|o| o.ai.wake_frame > self.frame) {
            self.chinooks.insert(id, st);
            return;
        }
        if !alive {
            // The Chinook died mid-drop; its rappellers keep descending (`update_rappels`).
            return;
        }
        match st.phase {
            ChinookPhase::MoveToDrop => {
                // `MOVE_TO_COMBAT_DROP` (ChinookAIUpdate.cpp:853): the move's success starts `DO_COMBAT_DROP`, its failure
                // (or an interrupt that ends the move without a result) goes to `AI_IDLE`, and the drop is over.
                let (result, moving_drop) = match self.objects.get_mut(&id) {
                    Some(o) => (o.ai.machine.drop_move_result.take(), o.ai.machine.current == crate::ai_states::AiStateType::MoveTo),
                    None => (None, false),
                };
                match result {
                    Some(true) => self.begin_combat_drop(id, &mut st),
                    Some(false) => self.cancel_combat_drop(id, &mut st),
                    None if !moving_drop => self.cancel_combat_drop(id, &mut st),
                    None => {}
                }
            }
            ChinookPhase::CombatDrop => self.step_combat_drop(id, &mut st),
        }
        if !st.done {
            self.chinooks.insert(id, st);
        }
    }

    /// The combat drop's move failed or was interrupted: the state machine is `AI_IDLE` and the drop never starts. The
    /// move's exit restores the normal preferred height (`ChinookMoveToBldgState::onExit`, ChinookAIUpdate.cpp:770-782).
    fn cancel_combat_drop(&mut self, id: ObjectId, st: &mut ChinookState) {
        self.end_drop_move_settings(id);
        st.done = true;
    }

    /// `ChinookMoveToBldgState::onExit` (ChinookAIUpdate.cpp:770-782): the move's exit restores the normal preferred height and
    /// ends the ultra-accurate flight. It runs on success and on failure, before `DO_COMBAT_DROP`'s enter (:853).
    pub(crate) fn end_drop_move_settings(&mut self, id: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.preferred_height_override = None;
            o.loco_rt.ultra_accurate = false;
        }
    }

    /// `ChinookCombatDropState::onEnter`: hold, lose the supplies, seed the ropes.
    fn begin_combat_drop(&mut self, id: ObjectId, st: &mut ChinookState) {
        // The move succeeded, so its exit runs first (ChinookAIUpdate.cpp:770-782, :853).
        self.end_drop_move_settings(id);
        let Some((pos, angle, t, radius)) = self.objects.get(&id).map(|o| (o.position, o.angle, o.template.chinook, o.template.geometry.bounding_circle_radius())) else {
            return;
        };
        let Some(t) = t else { return };
        self.set_disabled(id, crate::disabled::DisabledType::Held);
        let now = self.frame;
        let ground = self.terrain.ground_height(pos.x, pos.y);
        if let Some(o) = self.objects.get_mut(&id) {
            o.boxes = 0;
            o.ai.stop();
        }
        let n = t.num_ropes.max(1);
        let mut ropes = Vec::with_capacity(n as usize);
        for i in 0..n {
            // Approximated `RopeStart` bones: spread along the Chinook's belly.
            let off = if n <= 1 { 0.0 } else { (i as f32 / (n - 1) as f32 - 0.5) * radius * 1.2 };
            let (s, c) = (sin(angle), cos(angle));
            let start = Coord3D::new(pos.x - off * s, pos.y + off * c, pos.z);
            let len_max = (start.z - ground - t.rope_final_height).max(0.0);
            let next_drop_time = now.saturating_add(self.rng.value(t.per_rope_delay_min as i32, t.per_rope_delay_max as i32).max(0) as u32).saturating_sub(t.per_rope_delay_min);
            ropes.push(RopeState { start, len: 1.0, len_max, speed: 0.0, next_drop_time, rappellers: Vec::new() });
        }
        st.ropes = ropes;
        st.phase = ChinookPhase::CombatDrop;
    }

    /// `ChinookCombatDropState::update`: extend the ropes, drop rappellers and detect completion.
    fn step_combat_drop(&mut self, id: ObjectId, st: &mut ChinookState) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.chinook) else { return };
        let now = self.frame;
        let gravity = self.rules.gravity.abs();
        // `removeDoneRappellers`.
        for rope in &mut st.ropes {
            rope.rappellers.retain(|&rid| {
                self.objects.get(&rid).is_some_and(|o| {
                    !o.is_dying()
                        && o.contained_by.is_none()
                        && o.position.z > self.terrain.ground_height(o.position.x, o.position.y) + 0.01
                })
            });
        }
        let mut num_ropes_in_use = 0;
        for i in 0..st.ropes.len() {
            if st.ropes[i].len < st.ropes[i].len_max {
                st.ropes[i].speed += gravity;
                if st.ropes[i].speed > t.rope_drop_speed {
                    st.ropes[i].speed = t.rope_drop_speed;
                }
                st.ropes[i].len += st.ropes[i].speed;
                if t.wait_for_ropes_to_drop {
                    // Can't use this rope until it has dropped all the way.
                    st.ropes[i].next_drop_time = st.ropes[i].next_drop_time.saturating_add(1);
                    if !st.ropes[i].rappellers.is_empty() {
                        num_ropes_in_use += 1;
                    }
                    continue;
                }
            }
            if now >= st.ropes[i].next_drop_time {
                if let Some(rider) = self.potential_rappeller(id) {
                    let start = st.ropes[i].start;
                    self.drop_rappeller(id, rider, start, st.target_obj, t.rappel_speed, gravity);
                    st.ropes[i].rappellers.push(rider);
                    st.ropes[i].next_drop_time = now.saturating_add(self.rng.value(t.per_rope_delay_min as i32, t.per_rope_delay_max as i32).max(0) as u32);
                }
            }
            if !st.ropes[i].rappellers.is_empty() {
                num_ropes_in_use += 1;
            }
        }
        if num_ropes_in_use == 0 && self.potential_rappeller(id).is_none() {
            self.clear_disabled(id, crate::disabled::DisabledType::Held);
            // `ChinookTakeoffOrLandingState::onExit` restores the normal preferred height.
            if let Some(o) = self.objects.get_mut(&id) {
                o.preferred_height_override = None;
            }
            st.done = true;
        }
    }

    /// `getPotentialRappeller`: the first contained `KINDOF_CAN_RAPPEL` passenger.
    fn potential_rappeller(&self, chinook: ObjectId) -> Option<ObjectId> {
        let o = self.objects.get(&chinook)?;
        o.contained.iter().copied().find(|r| self.objects.get(r).is_some_and(|u| u.is_kind_of(kind_of::CAN_RAPPEL)))
    }

    /// `exitObjectViaDoor(rappeller, DOOR_1)` + `setTransformMatrix(dropStartMtx)` + `aiRappelInto`.
    #[allow(clippy::too_many_arguments)]
    fn drop_rappeller(&mut self, chinook: ObjectId, rider: ObjectId, start: Coord3D, target_obj: Option<ObjectId>, rappel_speed: f32, gravity: f32) {
        self.remove_from_contain(chinook, rider);
        if let Some(o) = self.objects.get_mut(&rider) {
            o.set_position(start);
            o.ai.stop();
        }
        let dest_z = self.rappel_dest_z(rider, target_obj);
        // `AIRappelState::onEnter`: `m_rappelRate = -min(desiredSpeed, |gravity| * 30 * 2.5)`.
        let rate = rappel_speed.min(gravity * LOGICFRAMES_PER_SECOND as f32 * 2.5).max(0.0);
        self.rappels.insert(rider, RappelState { dest_z, rate, target_obj });
    }

    /// The z a rappeller descends to (`AIRappelState::onEnter`).
    fn rappel_dest_z(&self, rider: ObjectId, target_obj: Option<ObjectId>) -> f32 {
        let pos = self.objects.get(&rider).map_or(Coord3D::ZERO, |o| o.position);
        let ground = self.terrain.ground_height(pos.x, pos.y);
        match target_obj.and_then(|b| self.objects.get(&b)) {
            Some(b) if !b.is_dying() && b.template.kind_of & kind_of::STRUCTURE != 0 => ground + b.template.geometry.height,
            _ => ground,
        }
    }

    /// `AIRappelState::update` for every descending rappeller, in id order.
    pub(crate) fn update_rappels(&mut self) {
        let ids: Vec<ObjectId> = self.rappels.keys().copied().collect();
        for id in ids {
            let Some(st) = self.rappels.get(&id).cloned() else { continue };
            let Some(o) = self.objects.get(&id) else {
                self.rappels.remove(&id);
                continue;
            };
            if o.destroyed || o.is_dying() {
                self.rappels.remove(&id);
                continue;
            }
            let mut pos = o.position;
            pos.z -= st.rate;
            if pos.z <= st.dest_z {
                pos.z = st.dest_z;
                if let Some(o) = self.objects.get_mut(&id) {
                    o.set_position(pos);
                }
                self.rappel_land(id, st.target_obj);
                self.rappels.remove(&id);
            } else if let Some(o) = self.objects.get_mut(&id) {
                o.set_position(pos);
            }
        }
    }

    /// The `AIRappelState::update` arrival branch: kill up to two enemies in the target building,
    /// then enter it (or land on the ground).
    fn rappel_land(&mut self, id: ObjectId, target_obj: Option<ObjectId>) {
        let Some(bldg) = target_obj else { return };
        let is_bldg = self.objects.get(&bldg).is_some_and(|b| !b.is_dying() && b.template.kind_of & kind_of::STRUCTURE != 0);
        if !is_bldg {
            return;
        }
        let killed = self.kill_enemies_in_container(id, bldg, 2);
        if killed >= 2 {
            // `obj->kill()` after killing two defenders.
            self.destroy_object(id);
            return;
        }
        if self.is_valid_container_for(bldg, id, true) {
            self.force_enter_contain(bldg, id);
            return;
        }
        // The building is full: scatter to the ground nearby (reduced `findPositionAround`).
        let (bpos, br, bangle) = match self.objects.get(&bldg) {
            Some(b) => (b.position, b.template.geometry.bounding_circle_radius(), b.angle),
            None => return,
        };
        let ur = self.objects.get(&id).map_or(0.0, |u| u.template.geometry.bounding_circle_radius());
        let offset = ur.min(br);
        let angle = self.rng.value_real(crate::math::PI, crate::math::TWO_PI);
        let x = bpos.x + offset * cos(angle);
        let y = bpos.y + offset * sin(angle);
        let z = self.terrain.ground_height(x, y);
        if let Some(o) = self.objects.get_mut(&id) {
            o.set_position(Coord3D::new(x, y, z));
            o.set_orientation(bangle);
        }
        // `AIRappelState::update`'s scatter (AIStates.cpp, the full-building branch): `findPositionAround` from the start
        // point, searching from 1.5*PI (down) out to 200, and `aiFollowPath` on the spot it finds, ignoring the building.
        let start = Coord3D::new(x, y, z);
        let opts = crate::dock::FindPositionOptions { max_radius: 200.0, start_angle: Some(1.5 * crate::math::PI), ..Default::default() };
        if let Some(spot) = self.find_position_around(&start, &opts) {
            self.ai_set_follow_path(id, vec![spot], crate::ai_states::AiStateType::FollowPath, Some(bldg), None);
        }
        self.wake_up_now(id);
    }

    /// `killEnemiesInContainer`: remove and kill up to `max_to_kill` enemies inside `bldg`.
    fn kill_enemies_in_container(&mut self, killer: ObjectId, bldg: ObjectId, max_to_kill: i32) -> i32 {
        let killer_player = self.objects.get(&killer).map_or(0, |o| o.player);
        let mut killed = 0;
        while killed < max_to_kill {
            let enemy = self
                .contained_units(bldg)
                .iter()
                .copied()
                .find(|&e| self.objects.get(&e).is_some_and(|o| !o.is_dying() && self.relationship(killer_player, o.player) == crate::damage::Relationship::Enemies));
            let Some(enemy) = enemy else { break };
            self.remove_from_contain(bldg, enemy);
            // `killer->scoreTheKill(enemy)` before `enemy->kill()` (AIStates.cpp:452, `killEnemiesInContainer`).
            self.object_score_the_kill(enemy, killer);
            self.destroy_object(enemy);
            killed += 1;
        }
        killed
    }
}

/// The `GameLogic` maps for the Chinook combat drop (hashed only when non-empty).
pub(crate) type ChinookMap = BTreeMap<ObjectId, ChinookState>;
pub(crate) type RappelMap = BTreeMap<ObjectId, RappelState>;

/// Hash the combat-drop state (see [`crate::logic::GameLogic::crc`]).
pub(crate) fn crc_chinooks(chinooks: &ChinookMap, rappels: &RappelMap, c: &mut crate::crc::Crc) {
    if !chinooks.is_empty() {
        c.add_u32(chinooks.len() as u32);
        for (id, st) in chinooks {
            c.add_u32(id.0);
            c.add_u32(st.phase as u32);
            c.add_coord(&st.target_pos);
            c.add_u32(st.target_obj.map_or(u32::MAX, |x| x.0));
            c.add_f32(st.dest_z);
            c.add_bool(st.done);
            c.add_u32(st.ropes.len() as u32);
            for rope in &st.ropes {
                c.add_coord(&rope.start);
                c.add_f32(rope.len);
                c.add_f32(rope.len_max);
                c.add_f32(rope.speed);
                c.add_u32(rope.next_drop_time);
                c.add_u32(rope.rappellers.len() as u32);
                for r in &rope.rappellers {
                    c.add_u32(r.0);
                }
            }
        }
    }
    if !rappels.is_empty() {
        c.add_u32(rappels.len() as u32);
        for (id, st) in rappels {
            c.add_u32(id.0);
            c.add_f32(st.dest_z);
            c.add_f32(st.rate);
            c.add_u32(st.target_obj.map_or(u32::MAX, |x| x.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contain::{ContainTemplate, GarrisonFirePoints};
    use crate::template::{kind_of, LocomotorKind, LocomotorParams, ZBehavior};
    use crate::{Coord3D, GameRules, ObjectTemplate, TerrainLogic};
    use std::sync::Arc;

    fn transport_contain() -> ContainTemplate {
        ContainTemplate {
            kind: crate::contain::ContainKind::Transport,
            capacity: 8,
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
            tunnel_frames_for_full_heal: 1.0,
            is_enclosing_container: false,
            fire_points: GarrisonFirePoints::default(),
            passengers_in_turret: false,
            weapon_bonus_passed_to_passengers: false,
            immune_to_clear_building_attacks: false,
        }
    }

    fn chinook_template() -> ObjectTemplate {
        let mut t = ObjectTemplate::new("Chinook");
        t.kind_of = kind_of::VEHICLE | kind_of::AIRCRAFT | kind_of::TRANSPORT | kind_of::SELECTABLE;
        t.geometry.major_radius = 20.0;
        t.geometry.height = 12.0;
        t.max_health = 300.0;
        t.initial_health = 300.0;
        let mut loco = LocomotorParams::from_ini(LocomotorKind::Hover, 120.0, 60.0, 180.0, 120.0);
        loco.behavior_z = ZBehavior::SurfaceRelativeHeight;
        t.locomotor = Some(loco);
        t.contain = Some(Arc::new(transport_contain()));
        t.chinook = Some(ChinookTemplate {
            num_ropes: 2,
            rope_drop_speed: 1000.0,
            rappel_speed: 1.0,
            per_rope_delay_min: 0,
            per_rope_delay_max: 0,
            min_drop_height: 30.0,
            wait_for_ropes_to_drop: true,
            rope_final_height: 0.0,
        });
        t
    }

    fn rappeller_template() -> ObjectTemplate {
        let mut t = ObjectTemplate::new("Ranger");
        t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE | kind_of::CAN_RAPPEL;
        t.geometry.major_radius = 4.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.transport_slot_count = 1;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
        t
    }

    #[test]
    fn combat_drop_flies_hovers_and_rappels_everyone_down() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        g.set_rules(GameRules { gravity: -1.0, ..GameRules::default() });
        let ch = g.create_object(&Arc::new(chinook_template()), Coord3D::new(100.0, 100.0, 30.0), 0.0, 0);
        let r1 = g.create_object(&Arc::new(rappeller_template()), Coord3D::new(100.0, 100.0, 30.0), 0.0, 0);
        let r2 = g.create_object(&Arc::new(rappeller_template()), Coord3D::new(100.0, 100.0, 30.0), 0.0, 0);
        g.force_enter_contain(ch, r1);
        g.force_enter_contain(ch, r2);
        assert_eq!(g.contained_units(ch).len(), 2);
        let target = Coord3D::new(400.0, 400.0, 0.0);
        assert!(g.start_combat_drop(ch, target, None), "the Chinook has the module and a transport");
        assert_eq!(g.chinook_state(ch).unwrap().phase, ChinookPhase::MoveToDrop);
        // Fly to the hover point, then drop both riders and land them.
        g.run_frames(600);
        assert!(g.chinook_state(ch).is_none() || g.chinook_state(ch).unwrap().done, "the drop finished");
        for r in [r1, r2] {
            let o = g.find_object(r).expect("rappeller survives a ground drop");
            assert!(o.contained_by.is_none(), "rappeller left the Chinook");
            let ground = g.terrain().ground_height(o.position.x, o.position.y);
            assert!((o.position.z - ground).abs() < 0.5, "rappeller reached the ground ({})", o.position.z);
        }
        assert_eq!(g.rappels.len(), 0);
        assert!(g.find_object(ch).is_some(), "the Chinook survives");
    }

    #[test]
    fn combat_drop_into_a_building_kills_defenders_and_enters() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        g.set_rules(GameRules { gravity: -1.0, ..GameRules::default() });
        g.set_enemies(0, 1);
        let ch = g.create_object(&Arc::new(chinook_template()), Coord3D::new(100.0, 100.0, 30.0), 0.0, 0);
        let r1 = g.create_object(&Arc::new(rappeller_template()), Coord3D::new(100.0, 100.0, 30.0), 0.0, 0);
        g.force_enter_contain(ch, r1);
        // A garrisonable enemy building with one defender.
        let mut b = ObjectTemplate::new("Bunker");
        b.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        b.geometry.major_radius = 20.0;
        b.geometry.height = 15.0;
        b.max_health = 1000.0;
        b.initial_health = 1000.0;
        let mut c = transport_contain();
        c.kind = crate::contain::ContainKind::Garrison;
        c.is_enclosing_container = true;
        c.capacity = 5;
        b.contain = Some(Arc::new(c));
        let bldg = g.create_object(&Arc::new(b), Coord3D::new(400.0, 400.0, 0.0), 0.0, 1);
        let e1 = g.create_object(&Arc::new(rappeller_template()), Coord3D::new(400.0, 400.0, 0.0), 0.0, 1);
        g.force_enter_contain(bldg, e1);
        assert!(g.start_combat_drop(ch, Coord3D::new(400.0, 400.0, 0.0), Some(bldg)));
        g.run_frames(600);
        assert!(g.find_object(e1).is_none(), "the defender was killed");
        assert_eq!(g.container_of(r1), Some(bldg), "the rappeller entered the building");
    }

    /// `killEnemiesInContainer` (AIStates.cpp:434): each kill is `killer->scoreTheKill(enemy)` before `enemy->kill()`,
    /// so the killer's owner counts the kills and the defender's owner the losses.
    #[test]
    fn combat_drop_kills_are_scored() {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.set_enemies(0, 1);
        let ch = g.create_object(&Arc::new(chinook_template()), Coord3D::new(100.0, 100.0, 30.0), 0.0, 0);
        let mut b = ObjectTemplate::new("Bunker");
        b.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        b.geometry.major_radius = 20.0;
        b.geometry.height = 15.0;
        b.max_health = 1000.0;
        b.initial_health = 1000.0;
        let mut c = transport_contain();
        c.kind = crate::contain::ContainKind::Garrison;
        c.is_enclosing_container = true;
        c.capacity = 5;
        b.contain = Some(Arc::new(c));
        let bldg = g.create_object(&Arc::new(b), Coord3D::new(400.0, 400.0, 0.0), 0.0, 1);
        let mut d = ObjectTemplate::new("Defender");
        d.kind_of = kind_of::INFANTRY | kind_of::SCORE;
        d.max_health = 100.0;
        d.initial_health = 100.0;
        let defender = Arc::new(d);
        for _ in 0..2 {
            let e = g.create_object(&defender, Coord3D::new(400.0, 400.0, 0.0), 0.0, 1);
            g.force_enter_contain(bldg, e);
        }
        assert_eq!(g.kill_enemies_in_container(ch, bldg, 5), 2, "both defenders are killed");
        assert_eq!(g.score_keeper(0).unwrap().total_units_destroyed(), 2, "the killer's owner counts the kills");
        assert_eq!(g.score_keeper(1).unwrap().total_units_lost(), 2, "the defender's owner counts the losses");
    }
}
