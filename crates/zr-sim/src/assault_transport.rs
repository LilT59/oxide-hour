//! `AssaultTransportAIUpdate`: the troop crawler's AI module (lane 194, wave 23; AN5 s1 f3721).
//!
//! Ported from `GeneralsMD/Code/GameEngine/Source/GameLogic/Object/Update/AIUpdate/AssaultTransportAIUpdate.cpp` and its
//! header. The crawler's DEPLOY weapon (`DamageType = DEPLOY`, `Weapon.cpp:2611-2620`) names the victim as the designated
//! target (`beginAssault`). Each update the transport keeps a list of up to ten members (its contained passengers),
//! orders every healthy, non-new member out of the crawler (`aiExit`, AssaultTransportAIUpdate.cpp:281-286) once a target
//! exists, and sends the outside members at the target. Only then does the transport run its own `AIUpdateInterface::update`.
//!
//! The port's trace of AN5 s1 showed the crawler 263 firing its DEPLOY weapon at f3720 (C++) and the riders 264-266 going to
//! `AI_EXIT` at f3721 (`aisleep` 3738 -> 0). The port had no such module, so the riders stayed in the crawler.
//!
//! Not ported: `m_memberHealing`, `m_state`, `m_framesRemaining` (written, never read in the C++), the centroid code (only
//! read by the commented-out "keep near the troops" block) and `ClearRangeRequiredToContinueAttackMove` (never read).
//! The dead refusal of `isAllowedToRespondToAiCommands` and the subdued container of `privateExit` are modelled by the
//! `aiExit`/`aiEnter` helpers. The mood-sleep and forbid gates of `isAllowedToRespondToAiCommands` are not.

use crate::ai_states::{AiStateType, CommandSource};
use crate::math::Coord3D;
use crate::object::ObjectId;
use crate::GameLogic;

/// `MAX_TRANSPORT_SLOTS` (AssaultTransportAIUpdate.h).
pub const MAX_TRANSPORT_SLOTS: usize = 10;

/// `AssaultTransportAIUpdateModuleData` fields the sim reads.
#[derive(Clone, Debug, PartialEq)]
pub struct AssaultTransportTemplate {
    /// `m_membersGetHealedAtLifeRatio` (`MembersGetHealedAtLifeRatio`, default 0): a member below this health ratio goes
    /// back to its transport to heal instead of attacking (AssaultTransportAIUpdate.cpp:406-418).
    pub members_get_healed_at_life_ratio: f32,
}

impl Default for AssaultTransportTemplate {
    fn default() -> Self {
        Self { members_get_healed_at_life_ratio: 0.0 }
    }
}

/// The transport's per-object state (the members and orders of `AssaultTransportAIUpdate`).
#[derive(Clone, Debug, PartialEq)]
pub struct AssaultTransportState {
    /// `m_memberIDs`. Slots at or past `current_members` keep whatever the C++ left in them (the swap-removal does not clear
    /// the slot it moves from), so the arrays are kept whole.
    member_ids: [ObjectId; MAX_TRANSPORT_SLOTS],
    /// `m_newMember`: a passenger that boarded after the transport's first update does not leave until a new attack order.
    new_member: [bool; MAX_TRANSPORT_SLOTS],
    /// `m_currentMembers`.
    current_members: usize,
    /// `m_attackMoveGoalPos`.
    attack_move_goal: Coord3D,
    /// `m_designatedTarget` (`mutable` in C++: `beginAssault` is const).
    designated_target: ObjectId,
    /// `m_isAttackMove`.
    is_attack_move: bool,
    /// `m_isAttackObject`.
    is_attack_object: bool,
    /// `m_newOccupantsAreNewMembers`.
    new_occupants_are_new_members: bool,
    /// The port's `AssaultTransportAIUpdate::update` returns `UPDATE_SLEEP_FOREVER` after `giveFinalOrders` (the heap never
    /// calls it again). The port's dying path runs every frame, so this latch keeps the final orders to one call.
    final_orders_given: bool,
}

impl Default for AssaultTransportState {
    /// `AssaultTransportAIUpdate::AssaultTransportAIUpdate` (the constructor's `reset()` over the whole array).
    fn default() -> Self {
        Self {
            member_ids: [ObjectId::INVALID; MAX_TRANSPORT_SLOTS],
            new_member: [false; MAX_TRANSPORT_SLOTS],
            current_members: 0,
            attack_move_goal: Coord3D::ZERO,
            designated_target: ObjectId::INVALID,
            is_attack_move: false,
            is_attack_object: false,
            new_occupants_are_new_members: false,
            final_orders_given: false,
        }
    }
}

impl AssaultTransportState {
    /// `AssaultTransportAIUpdate::reset` (AssaultTransportAIUpdate.cpp:59-77). Only the first `current_members` slots are
    /// cleared, as in the C++.
    fn reset(&mut self) {
        for i in 0..self.current_members {
            self.member_ids[i] = ObjectId::INVALID;
            self.new_member[i] = false;
        }
        self.current_members = 0;
        self.attack_move_goal = Coord3D::ZERO;
        self.designated_target = ObjectId::INVALID;
        self.is_attack_move = false;
        self.is_attack_object = false;
        self.new_occupants_are_new_members = false;
    }
}

/// The AI commands `AssaultTransportAIUpdate::aiDoCommand` inspects (AssaultTransportAIUpdate.cpp:83-115).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum AssaultCommand {
    /// `AICMD_ATTACKMOVE_TO_POSITION` with its `m_pos`.
    AttackMoveToPosition(Coord3D),
    AttackObject,
    Idle,
    /// Any other command: the default branch resets the transport.
    Other,
}

impl GameLogic {
    /// The transport's state, if the object has an `AssaultTransportAIUpdate`.
    fn assault_state(&self, id: ObjectId) -> Option<AssaultTransportState> {
        let o = self.objects.get(&id)?;
        o.template.assault_transport.as_ref()?;
        Some(o.ai.assault.clone())
    }

    /// Writes the state back. The caller has checked the module exists.
    fn set_assault_state(&mut self, id: ObjectId, state: AssaultTransportState) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.assault = state;
        }
    }

    /// Whether the object carries an `AssaultTransportAIUpdate`.
    pub(crate) fn has_assault_transport(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.template.assault_transport.is_some())
    }

    /// `AssaultTransportAIUpdate::beginAssault` (AssaultTransportAIUpdate.cpp:122-128), called by a DEPLOY weapon's fire
    /// (`Weapon::privateFireWeapon`, Weapon.cpp:2611-2620) with the victim. A missing victim does nothing.
    pub(crate) fn assault_begin(&mut self, transport: ObjectId, victim: ObjectId) {
        if !self.has_assault_transport(transport) || !self.objects.contains_key(&victim) {
            return;
        }
        if let Some(o) = self.objects.get_mut(&transport) {
            o.ai.assault.designated_target = victim;
        }
    }

    /// `AssaultTransportAIUpdate::aiDoCommand` (AssaultTransportAIUpdate.cpp:83-115) for a command that does not come from
    /// the AI: the order resets the transport (an idle order first recalls its outside members). Called from the port's
    /// idle, attack-object, attack-move and move-to-position/object command entries, before their own checks, as the C++
    /// override runs before `AIUpdateInterface::aiDoCommand`. The default branch (every other command, :109-115) is the
    /// player's command dispatch (`process_command_list`) for the orders it names, and evacuate (`command_evacuate`) with its
    /// own source. The AI's commands from other systems that the port does not dispatch here are not reset.
    pub(crate) fn assault_command(&mut self, id: ObjectId, cmd: AssaultCommand, source: CommandSource) {
        if source == CommandSource::Ai || !self.has_assault_transport(id) {
            return;
        }
        let Some(mut s) = self.assault_state(id) else { return };
        match cmd {
            AssaultCommand::AttackMoveToPosition(pos) => {
                s.reset();
                s.attack_move_goal = pos;
                s.is_attack_move = true;
            }
            AssaultCommand::AttackObject => {
                s.reset();
                s.is_attack_object = true;
            }
            AssaultCommand::Idle => {
                // `m_designatedTarget = INVALID_ID; retrieveMembers(); reset();` (AssaultTransportAIUpdate.cpp:101-106).
                s.designated_target = ObjectId::INVALID;
                self.set_assault_state(id, s);
                self.assault_retrieve_members(id);
                if let Some(mut s) = self.assault_state(id) {
                    s.reset();
                    self.set_assault_state(id, s);
                }
                return;
            }
            AssaultCommand::Other => s.reset(),
        }
        self.set_assault_state(id, s);
    }

    /// The health test of `isMemberWounded` (AssaultTransportAIUpdate.cpp:406-416): health / max health below the
    /// transport's `MembersGetHealedAtLifeRatio`.
    fn assault_member_wounded(&self, transport: ObjectId, member: ObjectId) -> bool {
        let ratio = self.objects.get(&transport).and_then(|o| o.template.assault_transport.as_ref()).map_or(0.0, |t| t.members_get_healed_at_life_ratio);
        self.objects.get(&member).and_then(|o| o.body.as_ref()).is_some_and(|b| b.health() / b.max_health() < ratio)
    }

    /// `isMemberHealthy` (AssaultTransportAIUpdate.cpp:422-431): health equals max health.
    fn assault_member_healthy(&self, member: ObjectId) -> bool {
        self.objects.get(&member).and_then(|o| o.body.as_ref()).is_some_and(|b| b.health() == b.max_health())
    }

    /// `AssaultTransportAIUpdate::isAttackPointless` (AssaultTransportAIUpdate.cpp:381-402): the transport is attacking and
    /// none of its members can (all are new).
    fn assault_attack_pointless(&self, id: ObjectId, s: &AssaultTransportState) -> bool {
        if !self.is_ai_attacking(id) {
            return false;
        }
        !s.new_member[..s.current_members].iter().any(|new| !new)
    }

    /// `AssaultTransportAIUpdate::retrieveMembers` (AssaultTransportAIUpdate.cpp:436-452): an outside member goes back
    /// into the transport (`aiEnter`, unless it is already entering).
    pub(crate) fn assault_retrieve_members(&mut self, id: ObjectId) {
        let Some(s) = self.assault_state(id) else { return };
        for &m in &s.member_ids[..s.current_members] {
            let Some(mo) = self.objects.get(&m) else { continue };
            if mo.ai_handle.is_none() || mo.contained_by.is_some() {
                continue;
            }
            if mo.ai.machine.current != AiStateType::Enter {
                self.assault_ai_enter(m, id, CommandSource::Ai);
            }
        }
    }

    /// `AssaultTransportAIUpdate::giveFinalOrders` (AssaultTransportAIUpdate.cpp:458-480): the transport's last order goes
    /// to its members (`CMD_FROM_PLAYER`), and no member may chase after it.
    pub(crate) fn assault_give_final_orders(&mut self, id: ObjectId) {
        let Some(mut s) = self.assault_state(id) else { return };
        for i in 0..s.current_members {
            let m = s.member_ids[i];
            if self.objects.get(&m).is_none_or(|o| o.ai_handle.is_none()) {
                continue;
            }
            let designated_exists = self.objects.contains_key(&s.designated_target);
            if s.is_attack_object && designated_exists {
                self.ai_attack_object_from(m, s.designated_target, false, false, None, CommandSource::Player);
            } else if s.is_attack_move {
                self.ai_attack_move_to_position_from(m, s.attack_move_goal, CommandSource::Player);
            }
            if let Some(o) = self.objects.get_mut(&m) {
                o.ai.allowed_to_chase = false;
            }
        }
        s.final_orders_given = true;
        self.set_assault_state(id, s);
    }

    /// `AssaultTransportAIUpdate::update`'s dead branch (AssaultTransportAIUpdate.cpp:150-154): a dead transport gives its
    /// final orders once and never runs its AI again. Returns `true` when the transport is dead (the caller skips the rest).
    pub(crate) fn assault_dead_update(&mut self, id: ObjectId) -> bool {
        let Some(s) = self.assault_state(id) else { return false };
        if !self.objects.get(&id).is_some_and(|o| o.effectively_dead) {
            return false;
        }
        if !s.final_orders_given {
            self.assault_give_final_orders(id);
        }
        true
    }

    /// The member bookkeeping at the start of `AssaultTransportAIUpdate::update` (AssaultTransportAIUpdate.cpp:157-248):
    /// dead or ordered-away members leave the list (the last one moves into the gap, and the loop does not revisit it),
    /// then every passenger not yet listed joins, up to the ten slots.
    fn assault_refresh_members(&mut self, id: ObjectId) {
        let Some(mut s) = self.assault_state(id) else { return };
        let mut i = 0;
        while i < s.current_members {
            let m = s.member_ids[i];
            let member = self.objects.get(&m);
            // `!member || member->isEffectivelyDead() || ai->getLastCommandSource() != CMD_FROM_AI` (a member without an AI
            // is dropped; the C++ dereferences a null AI there).
            let gone = match member {
                None => true,
                Some(mo) => mo.effectively_dead || mo.ai_handle.is_none() || mo.ai.machine.last_command_source != CommandSource::Ai,
            };
            if gone {
                let last = s.current_members - 1;
                if last > i {
                    s.member_ids[i] = s.member_ids[last];
                    s.new_member[i] = s.new_member[last];
                } else {
                    s.member_ids[i] = ObjectId::INVALID;
                    s.new_member[i] = false;
                }
                if let Some(mo) = self.objects.get_mut(&m) {
                    if mo.ai_handle.is_some() {
                        // The ex-member is no longer under transport control (AssaultTransportAIUpdate.cpp:185).
                        mo.ai.allowed_to_chase = false;
                    }
                }
                s.current_members -= 1;
            }
            i += 1;
        }

        // The passengers, in the container's order (`getContainedItemsList`).
        let has_contain = self.objects.get(&id).is_some_and(|o| o.template.contain.is_some());
        if has_contain {
            let passengers: Vec<ObjectId> = self.contained_units(id).to_vec();
            for p in passengers {
                if s.member_ids[..s.current_members].contains(&p) {
                    continue;
                }
                if s.current_members < MAX_TRANSPORT_SLOTS {
                    let slot = s.current_members;
                    s.member_ids[slot] = p;
                    if let Some(po) = self.objects.get_mut(&p) {
                        if po.ai_handle.is_some() {
                            // Members of an assault transport may chase the designated target (AssaultTransportAIUpdate.cpp:231).
                            po.ai.allowed_to_chase = true;
                        }
                    }
                    if s.new_occupants_are_new_members {
                        s.new_member[slot] = true;
                    }
                    s.current_members += 1;
                }
            }
            s.new_occupants_are_new_members = true;
        }
        self.set_assault_state(id, s);
    }

    /// `AssaultTransportAIUpdate::update` before `AIUpdateInterface::update` (AssaultTransportAIUpdate.cpp:156-374). Returns
    /// `true` when the C++ returns without the base update (`isAttackPointless` -> `aiIdle`, :251-254).
    pub(crate) fn assault_update(&mut self, id: ObjectId) -> bool {
        if !self.has_assault_transport(id) {
            return false;
        }
        self.assault_refresh_members(id);
        let Some(s) = self.assault_state(id) else { return false };
        if self.assault_attack_pointless(id, &s) {
            self.ai_idle(id, CommandSource::Ai);
            return true;
        }

        // The designated target, unless it is dead (`findObjectByID` and `isEffectivelyDead`, :264-266).
        let target = self.objects.get(&s.designated_target).filter(|o| !o.effectively_dead).map(|o| o.id);
        if let Some(target) = target {
            for i in 0..s.current_members {
                let m = s.member_ids[i];
                let Some(mo) = self.objects.get(&m) else { continue };
                if mo.ai_handle.is_none() {
                    continue;
                }
                let contained = mo.contained_by.is_some();
                let healthy = self.assault_member_healthy(m);
                let wounded = self.assault_member_wounded(id, m);
                // A healthy contained member that is not new is ordered out (AssaultTransportAIUpdate.cpp:281-286). The
                // `contained` test below is the one taken before the order, as in the C++.
                if contained && healthy && !s.new_member[i] {
                    self.assault_ai_exit(m, id, CommandSource::Ai);
                }
                if !contained {
                    if wounded {
                        // Wounded members go back to be healed (:287-296).
                        if self.objects.get(&m).is_some_and(|o| o.ai.machine.current != AiStateType::Enter) {
                            self.assault_ai_enter(m, id, CommandSource::Ai);
                        }
                    } else if self.objects.get(&m).is_some_and(|o| !o.is_ai_moving() && o.ai.machine.goal_object != target) {
                        // An outside, idle member attacks the designated target (:302-310).
                        self.ai_attack_object_from(m, target, false, false, None, CommandSource::Ai);
                    }
                }
            }
        } else if s.is_attack_move && self.objects.get(&id).is_some_and(|o| o.ai.machine.current != AiStateType::AttackMoveTo) {
            // Keep moving to the attack-move area (:317-321).
            self.ai_attack_move_to_position_from(id, s.attack_move_goal, CommandSource::Ai);
        } else if s.is_attack_object {
            // The target is gone: order the members back in (:324-326).
            self.assault_retrieve_members(id);
        }
        false
    }

    /// `AIUpdateInterface::aiEnter` -> `privateEnter` (AIUpdate.cpp:3776-3795) for a member: the unit enters `container`
    /// when `canEnterObject` allows it (`DONT_CHECK_CAPACITY`).
    fn assault_ai_enter(&mut self, rider: ObjectId, container: ObjectId, source: CommandSource) {
        // `aiEnter` is `aiDoCommand`, which returns first when `isAllowedToRespondToAiCommands` refuses: "the dead don't listen
        // very well" (AIUpdate.cpp:2592-2598, :2627-2628). The mood-sleep veto is not ported (module doc).
        if self.objects.get(&rider).is_none_or(|o| o.effectively_dead) {
            return;
        }
        if !self.is_mobile(rider) || !self.can_enter_object(rider, container, false) {
            return;
        }
        self.ai_clear(rider);
        if let Some(o) = self.objects.get_mut(&rider) {
            o.combat.attack = None;
            o.ai.machine.goal_object = container;
            o.ai.machine.last_command_source = source;
        }
        self.ai_set_state(rider, AiStateType::Enter);
    }

    /// `AIUpdateInterface::aiExit` -> `privateExit` (AIUpdate.cpp:3832-3850) for a passenger: `AI_EXIT` with the container as
    /// the goal. The containment test of `privateExit` (AIUpdate.cpp:3838-3843) is under `#if !RETAIL_COMPATIBLE_CRC`, which
    /// is `1` in retail (GameDefines.h), so no check runs here. A subdued container does not let go (AIUpdate.cpp:3847-3848).
    fn assault_ai_exit(&mut self, rider: ObjectId, container: ObjectId, source: CommandSource) {
        // `aiDoCommand`'s `isAllowedToRespondToAiCommands` first (AIUpdate.cpp:2627), as in `assault_ai_enter`.
        if self.objects.get(&rider).is_none_or(|o| o.effectively_dead) {
            return;
        }
        if self.objects.get(&container).is_some_and(|c| c.is_disabled_by_type(crate::disabled::DisabledType::Subdued)) {
            return;
        }
        self.ai_clear(rider);
        if let Some(o) = self.objects.get_mut(&rider) {
            o.ai.machine.goal_object = container;
            o.ai.machine.last_command_source = source;
        }
        self.ai_set_state(rider, AiStateType::Exit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::DamageType;
    use crate::combat::AttackTarget;
    use crate::command::Command;
    use crate::contain::{ContainKind, ContainTemplate, GarrisonFirePoints};
    use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
    use crate::terrain::TerrainLogic;
    use crate::weapon::{CombatTemplate, WeaponSetTemplate, WeaponTemplate};
    use std::sync::Arc;

    /// A troop-crawler-like transport: a vehicle with an `AssaultTransportAIUpdate`, a transport container of `capacity`
    /// and a DEPLOY weapon of 175 range (TroopCrawlerAssault, `Weapon.ini`).
    fn crawler_template(capacity: i32) -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new("TroopCrawler");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK | kind_of::TRANSPORT;
        t.geometry.major_radius = 11.0;
        t.max_health = 240.0;
        t.initial_health = 240.0;
        t.vision_range = 175.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 40.0, 120.0, 1000.0, 1000.0));
        t.contain = Some(Arc::new(ContainTemplate {
            kind: ContainKind::Transport,
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
            is_enclosing_container: false,
            fire_points: GarrisonFirePoints::default(),
            passengers_in_turret: false,
            weapon_bonus_passed_to_passengers: false,
            immune_to_clear_building_attacks: false,
        }));
        t.assault_transport = Some(AssaultTransportTemplate::default());
        let mut w = WeaponTemplate::new("TroopCrawlerAssault");
        w.primary_damage = 0.00001;
        w.damage_type = DamageType::Deploy;
        w.attack_range = 175.0;
        w.min_delay_between_shots = 30;
        w.max_delay_between_shots = 30;
        let mut set = WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(w));
        t.combat = CombatTemplate { weapon_set: Some(set), ..CombatTemplate::default() };
        Arc::new(t)
    }

    /// A rifleman: infantry with an AI module (a locomotor) and 100 health.
    fn rider_template() -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new("Rebel");
        t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
        t.geometry.major_radius = 4.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.transport_slot_count = 1;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 60.0, 120.0, 180.0));
        Arc::new(t)
    }

    /// A transport with `riders` already inside before its first update (so they are not new), and a player-0 transport
    /// with the first update run. The riders are added directly (`OpenContain::addToContain`), as the heal test does.
    fn boarded(riders: usize) -> (GameLogic, ObjectId, Vec<ObjectId>) {
        let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
        let c = g.create_object(&crawler_template(8), Coord3D::new(1000.0, 1000.0, 0.0), 0.0, 0);
        let mut ids = Vec::new();
        for k in 0..riders {
            let r = g.create_object(&rider_template(), Coord3D::new(1020.0 + k as f32, 1000.0, 0.0), 0.0, 0);
            g.add_to_contain(c, r);
            ids.push(r);
        }
        g.run_frames(1);
        assert!(ids.iter().all(|&r| g.is_contained(r)), "the riders are inside");
        (g, c, ids)
    }

    /// `beginAssault` (Weapon.cpp:2611-2620) records the designated target, and the riders then leave the transport.
    /// AN5 s1 f3720-3721: the DEPLOY fire sets the target and the next update orders the healthy riders out
    /// (AssaultTransportAIUpdate.cpp:281-286). Revert check: with the `assault_update` hook removed the riders stay inside.
    #[test]
    fn a_deploy_shot_orders_the_healthy_riders_out() {
        let (mut g, c, riders) = boarded(3);
        let victim = g.create_object(&rider_template(), Coord3D::new(1080.0, 1000.0, 0.0), 0.0, 1);
        g.assault_begin(c, victim);
        g.run_frames(1);
        for &r in &riders {
            let exiting = g.objects[&r].ai.machine.current == AiStateType::Exit || !g.is_contained(r);
            assert!(exiting, "rider {r:?} is ordered out on the update after the designated target is set");
        }
        g.run_frames(40);
        assert!(riders.iter().all(|&r| !g.is_contained(r)), "the riders have left the transport");
    }

    /// A rider that boarded after the transport's first update is new and stays in until a new attack order
    /// (AssaultTransportAIUpdate.cpp:242, :281). Revert check: ignoring `m_newMember` lets the new rider out.
    #[test]
    fn a_new_rider_stays_inside_until_a_new_order() {
        let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
        let c = g.create_object(&crawler_template(8), Coord3D::new(1000.0, 1000.0, 0.0), 0.0, 0);
        g.run_frames(2); // the transport's first update: the list is empty, and new occupants will be new
        let r = g.create_object(&rider_template(), Coord3D::new(1020.0, 1000.0, 0.0), 0.0, 0);
        g.submit(0, Command::Enter { ids: vec![r], container: c });
        g.run_frames(40);
        assert!(g.is_contained(r), "the rider boarded");
        let victim = g.create_object(&rider_template(), Coord3D::new(1080.0, 1000.0, 0.0), 0.0, 1);
        g.assault_begin(c, victim);
        g.run_frames(3);
        assert!(g.is_contained(r), "a new member is not ordered out");
        assert!(g.objects[&c].ai.assault.new_member[0], "the rider is recorded as new");
    }

    /// A passenger that joins the list may chase its designated target, and one that leaves it may not
    /// (AssaultTransportAIUpdate.cpp:231, :185). Revert check: without the flag writes the chase flag stays false.
    #[test]
    fn members_may_chase_while_they_are_members() {
        let (mut g, c, riders) = boarded(2);
        g.run_frames(2);
        assert_eq!(g.objects[&c].ai.assault.current_members, 2, "both riders are listed");
        assert!(riders.iter().all(|&r| g.objects[&r].ai.allowed_to_chase), "members are allowed to chase");
        // A rider that leaves the crawler while its player has ordered it drops off the list on the next update
        // (`getLastCommandSource() != CMD_FROM_AI`, AssaultTransportAIUpdate.cpp:162-170, :185).
        g.objects.get_mut(&riders[0]).unwrap().ai.machine.last_command_source = CommandSource::Player;
        g.remove_from_contain(c, riders[0]);
        g.run_frames(1);
        assert!(!g.objects[&riders[0]].ai.allowed_to_chase, "the ex-member may not chase");
        assert!(g.objects[&riders[1]].ai.allowed_to_chase, "the member stays allowed to chase");
        assert_eq!(g.objects[&c].ai.assault.current_members, 1, "the list drops the ex-member");
    }

    /// A passenger ordered by its player but still inside is removed and listed again in the same update: the add loop
    /// lists every passenger missing from the array (AssaultTransportAIUpdate.cpp:194-248), and it boarded before the first
    /// update's new-occupant flag, so this is a new member only if it boarded later. Revert check: skipping the add loop
    /// leaves the list at one member.
    #[test]
    fn a_contained_passenger_is_listed_again_as_a_new_member() {
        let (mut g, c, riders) = boarded(2);
        g.run_frames(2);
        g.objects.get_mut(&riders[0]).unwrap().ai.machine.last_command_source = CommandSource::Player;
        g.run_frames(1);
        assert_eq!(g.objects[&c].ai.assault.current_members, 2, "the still-contained rider is listed again");
        assert!(g.objects[&c].ai.assault.new_member[1], "the re-listed rider is new, since occupants are new after the first update");
    }

    /// `AssaultTransportAIUpdate::aiDoCommand` (AssaultTransportAIUpdate.cpp:83-115): a player's idle order calls the members
    /// back in before the reset. Revert check: without the idle hook the outside member is not recalled.
    #[test]
    fn a_player_idle_order_recalls_outside_members() {
        let (mut g, c, riders) = boarded(1);
        let victim = g.create_object(&rider_template(), Coord3D::new(1080.0, 1000.0, 0.0), 0.0, 1);
        g.assault_begin(c, victim);
        g.run_frames(3);
        assert!(!g.is_contained(riders[0]), "the rider is out");
        g.ai_idle(c, CommandSource::Player);
        assert_eq!(g.objects[&riders[0]].ai.machine.current, AiStateType::Enter, "the outside rider is sent back in");
        assert_eq!(g.objects[&c].ai.assault.current_members, 0, "the transport is reset");
    }

    /// `AssaultTransportAIUpdate::update` returns `UPDATE_SLEEP_NONE` while alive (AssaultTransportAIUpdate.cpp:376-377), so the
    /// stopped crawler (no goal, nothing to do) is still updated each frame, where a plain AI would sleep forever at rest
    /// (AIUpdate.cpp:2305-2314). Revert check: without the transport term in `ai_update_sleep` the sleep is FOREVER.
    #[test]
    fn a_stopped_transport_is_never_asleep() {
        let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
        let c = g.create_object(&crawler_template(8), Coord3D::new(1000.0, 1000.0, 0.0), 0.0, 0);
        g.run_frames(5);
        assert_eq!(g.ai_update_sleep(c), crate::module::UPDATE_SLEEP_NONE, "the transport asks for every frame");
    }

    /// `AssaultTransportAIUpdate::aiDoCommand`'s default branch (AssaultTransportAIUpdate.cpp:112-115) resets the transport for
    /// an evacuate (AICMD_EVACUATE) and for an attack-position order (AICMD_ATTACK_POSITION), which have no case of their own. Revert
    /// check: without the hooks in `command_evacuate` and `ai_attack_position_order_from` the member list keeps its rider.
    #[test]
    fn an_evacuate_or_attack_position_order_resets_the_transport() {
        let (mut g, c, _riders) = boarded(1);
        g.run_frames(2);
        assert_eq!(g.objects[&c].ai.assault.current_members, 1, "the rider is listed");
        g.command_evacuate(0, c, CommandSource::Player);
        assert_eq!(g.objects[&c].ai.assault.current_members, 0, "an evacuate resets the transport");

        let (mut g, c, _riders) = boarded(1);
        g.run_frames(2);
        g.ai_attack_position_order_from(c, Coord3D::new(1080.0, 1000.0, 0.0), false, CommandSource::Player);
        assert_eq!(g.objects[&c].ai.assault.current_members, 0, "an attack-position order resets the transport");
    }

    /// `aiDoCommand`'s default branch (AssaultTransportAIUpdate.cpp:109-115) resets a transport for every player AI order it
    /// receives, including an Enter, which has no branch of its own in the port. The reset runs in the command dispatch
    /// (`process_command_list`), with the order's source. Revert check: remove the dispatch hook in logic.rs; the rider stays listed.
    #[test]
    fn a_player_enter_order_resets_the_transport() {
        let (mut g, c, _riders) = boarded(1);
        g.run_frames(2);
        assert_eq!(g.objects[&c].ai.assault.current_members, 1, "the rider is listed");
        let other = g.create_object(&crawler_template(8), Coord3D::new(1400.0, 1000.0, 0.0), 0.0, 0);
        g.submit(0, Command::Enter { ids: vec![c], container: other });
        // Only the dispatch: the transport's own update would list the still-contained rider again (the new-member rule).
        g.process_command_list();
        assert_eq!(g.objects[&c].ai.assault.current_members, 0, "a player's enter order resets the transport");
    }

    /// `Weapon::privateFireWeapon`'s DAMAGE_DEPLOY case (Weapon.cpp:2611-2620) runs on the shot: the crawler's DEPLOY fire
    /// names its victim as the designated target (AssaultTransportAIUpdate.cpp:122-128). Revert check: without the hook in
    /// `fire_weapon` the designated target stays invalid.
    #[test]
    fn a_deploy_shot_designates_its_victim() {
        let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
        let c = g.create_object(&crawler_template(8), Coord3D::new(1000.0, 1000.0, 0.0), 0.0, 0);
        let victim = g.create_object(&rider_template(), Coord3D::new(1080.0, 1000.0, 0.0), 0.0, 1);
        g.fire_weapon(c, 0, AttackTarget::Object(victim));
        assert_eq!(g.objects[&c].ai.assault.designated_target, victim, "the victim is the designated target");
    }

    /// `AIAttackApproachTargetState::onEnter` (AIStates.cpp:2613): a human player's AI-ordered approach is refused unless the
    /// unit may chase, and an assault transport's member may (`isAllowedToChase`, AssaultTransportAIUpdate.cpp:231).
    /// Revert check: without `allowed_to_chase` in `approach_refused` the member is refused.
    #[test]
    fn a_transport_member_may_chase_for_a_human_player() {
        let mut g = GameLogic::new(TerrainLogic::flat(300, 300, 0), 1);
        let rider = g.create_object(&rider_template(), Coord3D::new(1000.0, 1000.0, 0.0), 0.0, 0);
        let victim = g.create_object(&rider_template(), Coord3D::new(1080.0, 1000.0, 0.0), 0.0, 1);
        assert!(g.approach_refused(rider, AttackTarget::Object(victim), true, false), "a human's AI-ordered approach is refused");
        g.objects.get_mut(&rider).unwrap().ai.allowed_to_chase = true;
        assert!(!g.approach_refused(rider, AttackTarget::Object(victim), true, false), "a member may chase");
    }
}
