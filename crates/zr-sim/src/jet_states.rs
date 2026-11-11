//! `JetAIUpdate` and its state machines (`Object/Update/AIUpdate/JetAIUpdate.cpp`): the airfield
//! cycle of the Raptor / Aurora / Stealth Fighter / MiG (`NeedsRunway = Yes`, `JetAIStateMachine`)
//! and the helicopter takeoff / landing of the Comanche (`NeedsRunway = No`, `HeliAIStateMachine`).
//!
//! The jet states are ported as [`AiStateType`] variants dispatched from `ai_states.rs`:
//!
//! | C++ state (`JetAIStateType`) | here |
//! |---|---|
//! | `TAKING_OFF_AWAIT_CLEARANCE` / `LANDING_AWAIT_CLEARANCE` (`JetAwaitingRunwayState`, or `SuccessState` for helis) | `JetTakingOffAwaitClearance` / `JetLandingAwaitClearance` |
//! | `TAXI_TO_TAKEOFF` / `TAXI_FROM_LANDING` / `TAXI_FROM_HANGAR` (`JetOrHeliTaxiState`) | `JetTaxiToTakeoff` / `JetTaxiFromLanding` / `JetTaxiFromHangar` |
//! | `PAUSE_BEFORE_TAKEOFF` (`JetPauseBeforeTakeoffState`) | `JetPauseBeforeTakeoff` |
//! | `TAKING_OFF` / `LANDING` (`JetTakeoffOrLandingState`) | `JetTakingOff` / `JetLanding` |
//! | `TAKING_OFF` / `LANDING` of a helicopter (`HeliTakeoffOrLandingState`) | `HeliTakingOff` / `HeliLanding` |
//! | `ORIENT_FOR_PARKING_PLACE` (`JetOrHeliParkOrientState`) | `JetOrientForParking` |
//! | `RELOAD_AMMO` (`JetOrHeliReloadAmmoState`) | `JetReloadAmmo` |
//! | `RETURNING_FOR_LANDING` (`JetOrHeliReturnForLandingState`) | `JetReturningForLanding` |
//! | `RETURN_TO_DEAD_AIRFIELD` / `CIRCLING_DEAD_AIRFIELD` | `JetReturnToDeadAirfield` / `JetCirclingDeadAirfield` |
//!
//! Commands reach a jet through [`GameLogic::ai_command_set_state`], the sim's `aiDoCommand`
//! (`JetAIUpdate::aiDoCommand`, `JetAIUpdate.cpp:2572`): a parked jet takes off first and keeps the
//! order pending, a jet in the middle of a takeoff or landing stores the order, an airborne jet
//! told to idle goes home.
//!
//! Reductions: the `Locomotor` runtime flags are per object, not per locomotor instance (they are
//! reset on every locomotor-set change, which is when the engine builds a new instance); the carrier
//! `FlightDeck` branches, `OBJECT_STATUS_DECK_HEIGHT_OFFSET`, the lock-on / sneaky-target missile
//! dodge, sounds and the client `MinHeight` drawable lift are not ported.

use crate::ai_states::{AiStateType, CommandSource, StateReturn};
use crate::jet_ai::{jet_flag, JetCommand};
use crate::logic::GameLogic;
use crate::math::{angle_diff, atan2, cos, floor, sin, sqrt, Coord3D, LOGICFRAMES_PER_SECOND, PI};
use crate::object::{ObjectId, LOCO_BIGNUM as BIGNUM};
use crate::template::{collide_kind, LocomotorSet};
use crate::weapon::WeaponStatus;
/// `JetOrHeliCirclingDeadAirfieldState::HOW_OFTEN_TO_CHECK`.
const HOW_OFTEN_TO_CHECK: i32 = LOGICFRAMES_PER_SECOND as i32;

/// What kind of `AICommandType` a command-initiated state change stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CmdKind {
    Idle,
    /// `AICMD_BUSY`: the "don't need (or want) to take off for these" list also holds it
    /// (`JetAIUpdate.cpp:2595-2601`), so a parked jet's `aiBusy` reaches `privateBusy` like any other
    /// unit's (`AIUpdate.cpp:4054-4060`).
    Busy,
    /// An offensive / guard order (`AICMD_ATTACKMOVE_TO_POSITION` ... `AICMD_HUNT`).
    Attack,
    /// `AICMD_GUARD_*` / `AICMD_HUNT` / `AICMD_GUARD_RETALIATE`: also set `ALLOW_INTERRUPT_AND_RESUME_OF_CUR_STATE_FOR_RELOAD`.
    Guard,
    Enter,
    /// Everything else (`AICMD_MOVE_TO_POSITION`, `AICMD_FOLLOW_PATH`, ...): "the default case".
    Other,
}

fn classify(state: AiStateType) -> CmdKind {
    match state {
        AiStateType::Idle => CmdKind::Idle,
        AiStateType::Busy => CmdKind::Busy,
        AiStateType::Guard | AiStateType::GuardRetaliate | AiStateType::Hunt => CmdKind::Guard,
        AiStateType::AttackMoveTo
        | AiStateType::AttackArea
        | AiStateType::AttackObject
        | AiStateType::AttackPosition
        | AiStateType::ForceAttackObject
        | AiStateType::AttackAndFollowObject => CmdKind::Attack,
        AiStateType::Enter => CmdKind::Enter,
        _ => CmdKind::Other,
    }
}

impl AiStateType {
    /// Is this one of the `JetAIStateType` states (`id >= JETAISTATETYPE_FIRST`)?
    pub(crate) fn is_jet_state(self) -> bool {
        matches!(
            self,
            AiStateType::JetTakingOffAwaitClearance
                | AiStateType::JetTaxiToTakeoff
                | AiStateType::JetPauseBeforeTakeoff
                | AiStateType::JetTakingOff
                | AiStateType::JetLandingAwaitClearance
                | AiStateType::JetLanding
                | AiStateType::JetTaxiFromLanding
                | AiStateType::JetTaxiFromHangar
                | AiStateType::JetOrientForParking
                | AiStateType::JetReloadAmmo
                | AiStateType::JetReturningForLanding
                | AiStateType::JetReturnToDeadAirfield
                | AiStateType::JetCirclingDeadAirfield
                | AiStateType::HeliTakingOff
                | AiStateType::HeliLanding
        )
    }
}

impl GameLogic {
    // ---- small accessors ------------------------------------------------------------------------

    fn jf(&self, id: ObjectId, f: u16) -> bool {
        self.objects.get(&id).is_some_and(|o| o.jet.flag(f))
    }

    fn jset(&mut self, id: ObjectId, f: u16, v: bool) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.jet.set_flag(f, v);
        }
    }

    pub(crate) fn is_jet(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.template.jet.is_some())
    }

    /// Whether a private command (`privateGuardArea`, `privateGuardPosition`, `privateMoveToObject`, ...) runs
    /// its own machine clear for this unit. A jet's `aiDoCommand` override (`JetAIUpdate.cpp:2572-2680`) only
    /// reaches the base `AIUpdateInterface::aiDoCommand` when the jet may fly (`ALLOW_AIR_LOCO`); a grounded jet
    /// takes the default branch instead, which clears the machine itself (`JetAIUpdate.cpp:2653-2655`, see
    /// `jet_intercept`). `JetAIUpdate.h:134-137` overrides none of the private guard or move functions.
    pub(crate) fn jet_base_command_clears(&self, id: ObjectId) -> bool {
        !self.is_jet(id) || self.jf(id, jet_flag::ALLOW_AIR_LOCO)
    }

    fn jet_producer(&self, id: ObjectId) -> ObjectId {
        self.objects.get(&id).map_or(ObjectId::INVALID, |o| o.producer_id)
    }

    fn jet_needs_runway(&self, id: ObjectId) -> bool {
        self.objects.get(&id).and_then(|o| o.template.jet.as_ref()).is_none_or(|j| j.needs_runway)
    }

    fn jet_parking_offset(&self, id: ObjectId) -> f32 {
        self.objects.get(&id).and_then(|o| o.template.jet.as_ref()).map_or(0.0, |j| j.parking_offset)
    }

    fn is_produced_at_helipad(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.template.is_collide_kind_of(collide_kind::PRODUCED_AT_HELIPAD))
    }

    /// `AIUpdateInterface::isIdle`: the state machine is in `AI_IDLE`.
    fn jet_machine_is_idle(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.ai.machine.current == AiStateType::Idle && o.ai.machine.temporary.is_none())
    }

    fn ai_current_state(&self, id: ObjectId) -> AiStateType {
        self.objects.get(&id).map_or(AiStateType::Idle, |o| o.ai.machine.current)
    }

    fn ai_set_last_command_source(&mut self, id: ObjectId, src: CommandSource) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.last_command_source = src;
        }
    }

    /// `Object::isAirborneTarget` (`Object.h:211`): the `OBJECT_STATUS_AIRBORNE_TARGET` bit `doLocomotor` sets after
    /// each frame's movement (`AIUpdate.cpp:2294-2300`). `JetAIUpdate::aiDoCommand` reads it (`JetAIUpdate.cpp:2599`).
    fn jet_is_airborne_target(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.status_bits & crate::object::status::AIRBORNE_TARGET != 0)
    }

    /// `WeaponSet::isOutOfAmmo`: every weapon (none counts) reads `OUT_OF_AMMO`.
    pub(crate) fn is_out_of_ammo(&self, id: ObjectId) -> bool {
        let now = self.frame;
        self.objects.get(&id).is_none_or(|o| o.combat.weapons.weapons.iter().flatten().all(|w| w.status(now) == WeaponStatus::OutOfAmmo))
    }

    /// `JetAIUpdate::isOutOfSpecialReloadAmmo` (`JetAIUpdate.cpp:101`): there is a
    /// `RETURN_TO_BASE_TO_RELOAD` weapon and all such weapons are out of ammo.
    pub(crate) fn jet_is_out_of_special_reload_ammo(&self, id: ObjectId) -> bool {
        let now = self.frame;
        let Some(o) = self.objects.get(&id) else { return false };
        let mut specials = 0;
        let mut out = 0;
        for w in o.combat.weapons.weapons.iter().flatten() {
            if !w.template.returns_to_base_to_reload {
                continue;
            }
            specials += 1;
            if w.status(now) == WeaponStatus::OutOfAmmo {
                out += 1;
            }
        }
        specials > 0 && out == specials
    }

    // ---- locomotor control ---------------------------------------------------------------------

    /// `Locomotor::setUsePreciseZPos` / `setUltraAccurate` / `setAllowInvalidPosition` on the current
    /// locomotor.
    fn jet_loco_flags(&mut self, id: ObjectId, precise_z: bool, ultra: bool, allow_invalid: bool) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.loco_rt.precise_z = precise_z;
            o.loco_rt.ultra_accurate = ultra;
            o.loco_rt.allow_invalid_position = allow_invalid;
        }
    }

    /// `JetAIUpdate::chooseLocomotorSet` (`JetAIUpdate.cpp:2213`) + `AIUpdateInterface::chooseLocomotorSet`.
    pub(crate) fn jet_choose_locomotor_set(&mut self, id: ObjectId, wst: LocomotorSet) -> bool {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.jet.clone()) else { return false };
        let mut wst = wst;
        if !self.jf(id, jet_flag::ALLOW_AIR_LOCO) {
            wst = LocomotorSet::Taxiing;
        } else if self.objects.get(&id).is_some_and(|o| o.jet.attack_loco_expire_frame != 0) {
            wst = t.attacking_loco;
        } else if self.jf(id, jet_flag::USE_SPECIAL_RETURN_LOCO) {
            wst = t.returning_loco;
        }
        self.ai_choose_locomotor_set(id, wst)
    }

    /// `AIUpdateInterface::chooseLocomotorSet`: switch to `wst` (`SET_NORMAL` becomes
    /// `SET_NORMAL_UPGRADED` once the locomotor upgrade is in). A real switch builds fresh `Locomotor`
    /// instances, so the runtime flags reset.
    pub(crate) fn ai_choose_locomotor_set(&mut self, id: ObjectId, wst: LocomotorSet) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        let mut wst = wst;
        if wst == LocomotorSet::Normal && o.loco_upgraded {
            wst = LocomotorSet::NormalUpgraded;
        }
        if wst == o.locomotor_set {
            return true;
        }
        let has = o.template.locomotor_sets.contains_key(&wst) || (wst == LocomotorSet::Normal && o.template.locomotor.is_some());
        if !has {
            return false;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.locomotor_set = wst;
        }
        // `chooseLocomotorSetExplicit` rebuilds the `Locomotor`s: their ctor resets and RNG draws (`AIUpdate.cpp:830-841`).
        self.rebuild_locomotors(id, wst);
        self.apply_locomotor_physics_options(id);
        true
    }

    /// `AIUpdateInterface::setLocomotorGoalNone` (the locomotor maintains its position).
    fn ai_set_locomotor_goal_none(&mut self, id: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.path.clear();
            o.ai.raw_path = None;
            o.ai.move_goal = None;
            o.ai.waiting_for_path = false;
            o.ai.set_locomotor_goal_none();
        }
    }

    /// `AIUpdateInterface::setLocomotorGoalPositionExplicit` (`AIUpdate.cpp:2461-2466`).
    pub(crate) fn ai_set_locomotor_goal_explicit(&mut self, id: ObjectId, pos: Coord3D) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.path.clear();
            o.ai.raw_path = None;
            o.ai.move_goal = None;
            o.ai.waiting_for_path = false;
            o.ai.set_locomotor_goal_explicit(pos);
        }
    }

    /// `AIUpdateInterface::setLocomotorGoalOrientation` (`AIUpdate.cpp:2478-2483`): only `.x` is
    /// written; `y`/`z` keep whatever the last explicit goal left there.
    pub(crate) fn ai_set_locomotor_goal_orientation(&mut self, id: ObjectId, angle: f32) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.path.clear();
            o.ai.raw_path = None;
            o.ai.move_goal = None;
            o.ai.waiting_for_path = false;
            o.ai.set_locomotor_goal_orientation(angle);
        }
    }

    /// `JetAIUpdate::setLocomotorGoalNone` (`JetAIUpdate.cpp:2232`): while taking off or landing in
    /// the air (and not allowed to circle) "none" means "keep flying straight".
    fn jet_set_locomotor_goal_none(&mut self, id: ObjectId) {
        if (self.jf(id, jet_flag::TAKEOFF_IN_PROGRESS) || self.jf(id, jet_flag::LANDING_IN_PROGRESS))
            && self.jf(id, jet_flag::ALLOW_AIR_LOCO)
            && !self.jf(id, jet_flag::ALLOW_CIRCLING)
        {
            let Some(o) = self.objects.get(&id) else { return };
            let desired = Coord3D::new(o.position.x + cos(o.angle) * 1000.0, o.position.y + sin(o.angle) * 1000.0, o.position.z);
            self.ai_set_locomotor_goal_explicit(id, desired);
        } else {
            self.ai_set_locomotor_goal_none(id);
        }
    }

    // ---- creation / deletion --------------------------------------------------------------------

    /// `JetAIUpdate::onObjectCreated` (`JetAIUpdate.cpp:1939`).
    pub(crate) fn jet_on_created(&mut self, id: ObjectId) {
        if !self.is_jet(id) {
            return;
        }
        self.jset(id, jet_flag::ALLOW_AIR_LOCO, false);
        // `chooseLocomotorSet(LOCOMOTORSET_TAXIING)` (JetAIUpdate.cpp:1943 -> AIUpdate.cpp:830) builds one `Locomotor`
        // per taxi entry after the base construction, each drawing its wander state (Locomotor.cpp:673-675). That is
        // the switch's own draw (`rebuild_locomotors`); the NORMAL draws were made before the object was inserted.
        self.jet_choose_locomotor_set(id, LocomotorSet::Taxiing);
    }

    /// Possession (an addition, `docs/POSSESSION.md`): the player flies the aircraft by hand, so a jet
    /// that is parked or taxiing is made airborne-capable on the spot (the airfield cycle's
    /// takeoff / taxi flags are dropped, the normal locomotor is chosen). Its parking space is kept
    /// (`KeepsParkingSpaceWhenAirborne`); on release the AI takes over again like any airborne jet.
    #[cfg(feature = "possession")]
    pub(crate) fn jet_begin_possession(&mut self, id: ObjectId) {
        if !self.is_jet(id) {
            return;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.jet.last_command = None;
            o.jet.flags &= !(jet_flag::HAS_PENDING_COMMAND
                | jet_flag::TAKEOFF_IN_PROGRESS
                | jet_flag::LANDING_IN_PROGRESS
                | jet_flag::TAXI_IN_PROGRESS
                | jet_flag::ALLOW_CIRCLING
                | jet_flag::ALLOW_INTERRUPT_AND_RESUME_OF_CUR_STATE_FOR_RELOAD);
            o.jet.set_flag(jet_flag::ALLOW_AIR_LOCO, true);
            o.loco_rt = crate::object::LocoRuntime::default();
        }
        // Leave any airfield-cycle state (the exit hooks release runways / reset the flags).
        if self.objects.get(&id).is_some_and(|o| o.ai.machine.current.is_jet_state()) {
            self.ai_set_state(id, AiStateType::Idle);
        }
        self.jet_choose_locomotor_set(id, LocomotorSet::Normal);
    }

    /// `JetAIUpdate::onDelete`: give the parking space back.
    pub(crate) fn jet_on_delete(&mut self, id: ObjectId) {
        if !self.is_jet(id) {
            return;
        }
        let producer = self.jet_producer(id);
        if let Some(af) = self.jet_getpp(producer) {
            self.pp_release_space(af, id);
        }
    }

    /// `JetAIUpdate::getProducerLocation` (`JetAIUpdate.cpp:1956`).
    fn jet_get_producer_location(&mut self, id: ObjectId) {
        if self.jf(id, jet_flag::HAS_PRODUCER_LOCATION) {
            return;
        }
        let producer = self.jet_producer(id);
        let loc = match self.find_object(producer) {
            Some(a) => a.position,
            None => match self.objects.get(&id) {
                Some(o) => o.position,
                None => return,
            },
        };
        if let Some(o) = self.objects.get_mut(&id) {
            o.jet.producer_location = loc;
        }
        // "if we aren't allowed to fly, then we should be parked (or at least taxiing), which
        // implies we have a parking place reserved. If we don't, it's probably because we were
        // directly spawned via script (or directly placed on the map). So ... quietly enable flight."
        let parked = self.jet_getpp(producer).is_some_and(|af| self.pp_has_reserved_space(af, id));
        if !parked {
            self.jset(id, jet_flag::ALLOW_AIR_LOCO, true);
            self.jet_choose_locomotor_set(id, LocomotorSet::Normal);
        } else {
            self.jset(id, jet_flag::ALLOW_AIR_LOCO, false);
            self.jet_choose_locomotor_set(id, LocomotorSet::Taxiing);
        }
        self.jset(id, jet_flag::HAS_PRODUCER_LOCATION, true);
    }

    /// `findSuitableAirfield` (`JetAIUpdate.cpp:165`): the closest allied, finished, living airfield
    /// that has a parking space for `jet` (reserving it, as the `PartitionFilterHasParkingPlace` does).
    fn jet_find_suitable_airfield(&mut self, jet: ObjectId) -> Option<ObjectId> {
        let (pos, player) = self.objects.get(&jet).map(|o| (o.position, o.player))?;
        let mut cands: Vec<(f32, ObjectId)> = self
            .objects
            .values()
            .filter(|o| {
                !o.destroyed
                    && !o.is_dying()
                    && !o.under_construction
                    && o.template.is_collide_kind_of(collide_kind::FS_AIRFIELD)
                    && o.parking.is_some()
                    && self.relationship(player, o.player) == crate::damage::Relationship::Allies
            })
            .map(|o| (o.position.dist_sqr_2d(&pos), o.id))
            .collect();
        cands.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        cands.into_iter().map(|(_, af)| af).find(|&af| self.jet_getpp(af).is_some() && self.pp_reserve_space(af, jet, 0.0).is_some())
    }

    // ---- JetAIUpdate::update ---------------------------------------------------------------------

    /// `JetAIUpdate::update` (`JetAIUpdate.cpp:1991`), up to the `AIUpdateInterface::update()` call
    /// (the caller runs the state machine and the locomotor afterwards).
    pub(crate) fn jet_ai_update(&mut self, id: ObjectId) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.jet.clone()) else { return };
        self.jet_get_producer_location(id);
        let now = self.frame;
        let producer = self.jet_producer(id);
        let pp = self.jet_getpp(producer);
        let is_helipad = self.is_produced_at_helipad(id);

        let idle = self.jet_machine_is_idle(id);
        let reloading = self.ai_current_state(id) == AiStateType::JetReloadAmmo;
        // "srj sez: not 100% sure on this. calling RELOAD_AMMO "idle" allows us to get healed while
        // reloading".
        if idle || reloading {
            if let Some(af) = pp {
                let allow_air = self.jf(id, jet_flag::ALLOW_AIR_LOCO);
                let pending = self.jf(id, jet_flag::HAS_PENDING_COMMAND);
                let full = self.objects.get(&id).and_then(|o| o.body.as_ref()).is_some_and(|b| b.health() == b.max_health());
                if !allow_air && !pending && is_helipad && full {
                    // "we're completely healed, so take off again"
                    self.pp_set_healee(af, id, false);
                    self.jset(id, jet_flag::ALLOW_AIR_LOCO, true);
                    self.ai_set_last_command_source(id, CommandSource::Ai);
                    self.ai_set_state(id, AiStateType::JetTakingOffAwaitClearance);
                    // TheSuperHackers @bugfix: "Move healed helicopter to rally point if present."
                    let rally = self.objects.get(&af).and_then(|o| o.parking.as_ref()).and_then(|rt| rt.heli_rally);
                    if let Some(r) = rally {
                        self.jet_ai_move_to_position(id, r);
                    }
                } else {
                    self.pp_set_healee(af, id, !allow_air);
                }
            }

            // "note that we might still have weapons with ammo, but still be forced to return to reload."
            let allow_air = self.jf(id, jet_flag::ALLOW_AIR_LOCO);
            let state = self.ai_current_state(id);
            if self.jet_is_out_of_special_reload_ammo(id) && allow_air {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.jet.return_to_base_frame = 0;
                }
                self.jset(id, jet_flag::USE_SPECIAL_RETURN_LOCO, true);
                self.ai_set_last_command_source(id, CommandSource::Ai);
                self.ai_set_state(id, AiStateType::JetReturningForLanding);
            } else if self.jf(id, jet_flag::HAS_PENDING_COMMAND) && state != AiStateType::JetReloadAmmo {
                // "if we are reloading ammo, wait will we are done before processing the pending command."
                if let Some(o) = self.objects.get_mut(&id) {
                    o.jet.return_to_base_frame = 0;
                }
                self.jet_replay_pending(id);
            } else if self.objects.get(&id).is_some_and(|o| o.jet.return_to_base_frame != 0 && now >= o.jet.return_to_base_frame) && allow_air {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.jet.return_to_base_frame = 0;
                }
                self.jset(id, jet_flag::USE_SPECIAL_RETURN_LOCO, false);
                self.ai_set_last_command_source(id, CommandSource::Ai);
                self.ai_set_state(id, AiStateType::JetReturningForLanding);
            } else if self.objects.get(&id).is_some_and(|o| o.jet.return_to_base_frame == 0) && t.return_to_base_idle_time > 0 && allow_air {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.jet.return_to_base_frame = now + t.return_to_base_idle_time;
                }
            }
        } else {
            if let Some(af) = pp {
                self.pp_set_healee(af, id, false);
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.jet.return_to_base_frame = 0;
            }
            if self.jf(id, jet_flag::ALLOW_INTERRUPT_AND_RESUME_OF_CUR_STATE_FOR_RELOAD)
                && self.jet_is_out_of_special_reload_ammo(id)
                && self.jf(id, jet_flag::ALLOW_AIR_LOCO)
            {
                self.jset(id, jet_flag::USE_SPECIAL_RETURN_LOCO, true);
                self.jset(id, jet_flag::HAS_PENDING_COMMAND, true);
                self.jset(id, jet_flag::ALLOW_INTERRUPT_AND_RESUME_OF_CUR_STATE_FOR_RELOAD, false);
                self.ai_set_last_command_source(id, CommandSource::Ai);
                self.ai_set_state(id, AiStateType::JetReturningForLanding);
            }
        }

        // `OBJECT_STATUS_IS_ATTACKING` and the attack-locomotor persistence.
        let attacking = self.objects.get(&id).is_some_and(|o| o.combat.attack.is_some());
        if attacking {
            if let Some(o) = self.objects.get_mut(&id) {
                o.jet.attack_loco_expire_frame = now + t.attack_loco_persist_time;
                o.jet.attackers_miss_expire_frame = now + t.attackers_miss_persist_time;
            }
        } else {
            let (loco_exp, miss_exp) = self.objects.get(&id).map_or((0, 0), |o| (o.jet.attack_loco_expire_frame, o.jet.attackers_miss_expire_frame));
            if loco_exp != 0 && now >= loco_exp {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.jet.attack_loco_expire_frame = 0;
                }
                // TheSuperHackers @bugfix: "Reset locomotor when attack locomotor timer expires."
                self.jet_choose_locomotor_set(id, LocomotorSet::Normal);
            }
            if miss_exp != 0 && now >= miss_exp {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.jet.attackers_miss_expire_frame = 0;
                }
            }
        }

        if self.objects.get(&id).is_some_and(|o| o.jet.attack_loco_expire_frame != 0) {
            self.jet_choose_locomotor_set(id, t.attacking_loco);
        } else if self.jf(id, jet_flag::USE_SPECIAL_RETURN_LOCO) {
            self.jet_choose_locomotor_set(id, t.returning_loco);
        }
    }

    /// `JetAIUpdate::notifyVictimIsDead`: with runways, "land when the target is gone".
    pub(crate) fn jet_notify_victim_is_dead(&mut self, id: ObjectId) {
        if !self.jet_needs_runway(id) || !self.is_jet(id) {
            return;
        }
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&id) {
            o.jet.return_to_base_frame = now;
        }
    }

    // ---- commands -------------------------------------------------------------------------------

    /// The sim's `aiDoCommand` for a command that sets `state`: jets intercept it
    /// (`JetAIUpdate::aiDoCommand`), everything else just sets the state.
    pub(crate) fn ai_command_set_state(&mut self, id: ObjectId, state: AiStateType) {
        if self.jet_intercept(id, state) {
            return;
        }
        self.ai_set_state(id, state);
    }

    /// Snapshot what the command handler wrote before calling `setState` (`m_mostRecentCommand.store`).
    fn jet_snapshot(&self, id: ObjectId, state: AiStateType) -> Option<JetCommand> {
        let o = self.objects.get(&id)?;
        Some(JetCommand {
            state,
            machine: o.ai.machine.clone(),
            attack: o.combat.attack,
            force_attack: o.combat.force_attack,
            attack_move: o.combat.attack_move,
            guard: o.combat.guard,
            guard_mode: o.combat.guard_mode,
            hunt: o.combat.hunt,
            hunt_area: o.combat.hunt_area,
        })
    }

    /// Park the order: remember it and forget the combat orders the handler set, so nothing acts on
    /// them while the jet takes off (`HAS_PENDING_COMMAND`).
    fn jet_store_pending(&mut self, id: ObjectId, cmd: JetCommand) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.jet.last_command = Some(Box::new(cmd));
            o.jet.set_flag(jet_flag::HAS_PENDING_COMMAND, true);
            o.combat.attack = None;
            o.combat.force_attack = false;
            o.combat.attack_move = false;
            o.combat.guard = None;
            o.combat.hunt = false;
            o.combat.hunt_area = None;
        }
    }

    /// `aiDoCommand(&m_mostRecentCommand.reconstitute(...))` for the pending command.
    fn jet_replay_pending(&mut self, id: ObjectId) {
        let Some(cmd) = self.objects.get_mut(&id).and_then(|o| o.jet.last_command.take()) else {
            self.jset(id, jet_flag::HAS_PENDING_COMMAND, false);
            return;
        };
        self.jset(id, jet_flag::HAS_PENDING_COMMAND, false);
        let cmd = *cmd;
        if let Some(o) = self.objects.get_mut(&id) {
            let (cur, locked) = (o.ai.machine.current, o.ai.machine.locked);
            o.ai.machine = cmd.machine;
            o.ai.machine.current = cur;
            o.ai.machine.locked = locked;
            o.ai.machine.temporary = None;
            o.ai.machine.sleep_till = 0;
            o.combat.attack = cmd.attack;
            o.combat.force_attack = cmd.force_attack;
            o.combat.attack_move = cmd.attack_move;
            o.combat.guard = cmd.guard;
            o.combat.guard_mode = cmd.guard_mode;
            o.combat.hunt = cmd.hunt;
            o.combat.hunt_area = cmd.hunt_area;
        }
        self.ai_command_set_state(id, cmd.state);
    }

    /// `JetAIUpdate::aiDoCommand` (`JetAIUpdate.cpp:2572`). Returns `true` when the jet consumed the
    /// command (took off first, stored it, or went home instead).
    fn jet_intercept(&mut self, id: ObjectId, state: AiStateType) -> bool {
        if !self.is_jet(id) {
            return false;
        }
        // "call this from aiDoCommand as well as update, because this can be called before update ever is"
        self.jet_get_producer_location(id);
        // `isAllowedToRespondToAiCommands`: "the dead don't listen very well".
        if self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return true;
        }
        let kind = classify(state);
        let Some(snapshot) = self.jet_snapshot(id, state) else { return true };
        // "note that we always store this, even if nothing will be "pending"."
        if let Some(o) = self.objects.get_mut(&id) {
            o.jet.last_command = Some(Box::new(snapshot.clone()));
        }

        if self.jf(id, jet_flag::TAKEOFF_IN_PROGRESS) || self.jf(id, jet_flag::LANDING_IN_PROGRESS) {
            // "have to wait for takeoff or landing to complete, just store the sucker"
            self.jet_store_pending(id, snapshot);
            return true;
        }
        if kind == CmdKind::Idle && self.ai_current_state(id) == AiStateType::JetReloadAmmo {
            // "uber-special-case... if we are told to idle, but are reloading ammo, ignore it for now"
            self.jet_store_pending(id, snapshot);
            return true;
        }
        if kind == CmdKind::Idle && self.jet_is_airborne_target(id) && !self.is_produced_at_helipad(id) {
            self.ai_set_last_command_source(id, CommandSource::Ai);
            self.ai_set_state(id, AiStateType::JetReturningForLanding);
            return true;
        }
        if !self.jf(id, jet_flag::ALLOW_AIR_LOCO) {
            match kind {
                // "don't need (or want) to take off for these" (`JetAIUpdate.cpp:2595-2601`: `AICMD_IDLE`,
                // `AICMD_BUSY`, `AICMD_FOLLOW_EXITPRODUCTION_PATH`): fall through to
                // `AIUpdateInterface::aiDoCommand`, whose `privateBusy` clears the machine.
                CmdKind::Idle | CmdKind::Busy => {}
                CmdKind::Attack | CmdKind::Guard | CmdKind::Enter | CmdKind::Other => {
                    if matches!(kind, CmdKind::Attack | CmdKind::Guard) && self.jet_is_out_of_special_reload_ammo(id) {
                        // TheSuperHackers @tweak: "Parked jets now defer offensive commands when out of ammo."
                        self.jet_store_pending(id, snapshot);
                        return true;
                    }
                    if kind == CmdKind::Enter {
                        // "if we're already parked at the airfield in question, just ignore."
                        let obj = self.objects.get(&id).map_or(ObjectId::INVALID, |o| o.ai.machine.goal_object);
                        if self.jet_is_parked_at(id, obj) {
                            return true;
                        }
                    }
                    // "nuke any existing pending cmd" (`JetAIUpdate.cpp:2653-2655`: store, then `getStateMachine()->clear()`
                    // before the setState; the clear runs the current state's exit and drops the goal).
                    self.jet_store_pending(id, snapshot);
                    self.ai_clear(id);
                    self.ai_set_last_command_source(id, CommandSource::Ai);
                    self.ai_set_state(id, AiStateType::JetTakingOffAwaitClearance);
                    return true;
                }
            }
        }
        self.jset(id, jet_flag::ALLOW_INTERRUPT_AND_RESUME_OF_CUR_STATE_FOR_RELOAD, kind == CmdKind::Guard);
        self.jset(id, jet_flag::HAS_PENDING_COMMAND, false);
        if kind == CmdKind::Enter {
            // `JetAIUpdate::privateEnter`: "we are already landing. just ignore it."
            let airfield = self.objects.get(&id).map_or(ObjectId::INVALID, |o| o.ai.machine.goal_object);
            if !self.jf(id, jet_flag::LANDING_IN_PROGRESS) {
                self.jet_do_landing_command(id, airfield);
            }
            return true;
        }
        false
    }

    /// `JetAIUpdate::isParkedAt` (`JetAIUpdate.cpp:2554`).
    fn jet_is_parked_at(&self, id: ObjectId, obj: ObjectId) -> bool {
        if self.jf(id, jet_flag::ALLOW_AIR_LOCO) || self.is_produced_at_helipad(id) || obj == ObjectId::INVALID {
            return false;
        }
        self.jet_getpp(self.jet_producer(id)).is_some_and(|af| af == obj)
    }

    /// `JetAIUpdate::doLandingCommand` (`JetAIUpdate.cpp:2471`): land at `airfield` and rearm.
    fn jet_do_landing_command(&mut self, id: ObjectId, airfield: ObjectId) {
        let Some(a) = self.find_object(airfield) else { return };
        if a.parking.is_none() {
            return;
        }
        let apos = a.position;
        let radius = a.template.geometry.bounding_circle_radius() * 10.0;
        let helipad = self.is_produced_at_helipad(id);
        if helipad {
            // "m_landingPosForHelipadStuff = *airfield->getPosition(); ... options.maxRadius =
            // airfield->getGeometryInfo().getBoundingCircleRadius() * 10.0f;
            // if (findPositionAround(&m_landingPosForHelipadStuff, &options, &tmp)) m_landing... = tmp"
            let options = crate::dock::FindPositionOptions { max_radius: radius, ..Default::default() };
            let spot = self.find_position_around(&apos, &options).unwrap_or(apos);
            if let Some(o) = self.objects.get_mut(&id) {
                o.jet.landing_pos_helipad = spot;
            }
        }
        let offset = self.jet_parking_offset(id);
        let reserved = helipad || self.pp_reserve_space(airfield, id, offset).is_some();
        if reserved {
            // "if we had a space at another airfield, release it"
            let old = self.jet_getpp(self.jet_producer(id));
            if let Some(old) = old {
                if old != airfield {
                    self.pp_release_space(old, id);
                }
            }
            self.set_producer(id, airfield);
            self.jset(id, jet_flag::USE_SPECIAL_RETURN_LOCO, false);
            self.jset(id, jet_flag::ALLOW_INTERRUPT_AND_RESUME_OF_CUR_STATE_FOR_RELOAD, false);
            self.ai_set_last_command_source(id, CommandSource::Ai);
            self.ai_set_state(id, AiStateType::JetReturningForLanding);
        }
    }

    /// `MSG_DO_ENTER` / `MSG_DO_GET_REPAIRED` for a jet targeting an airfield: land there. Returns
    /// `false` when `airfield` is not an airfield the jet may land at.
    pub(crate) fn jet_command_enter(&mut self, player: crate::object::PlayerIndex, id: ObjectId, airfield: ObjectId) -> bool {
        if !self.is_jet(id) {
            return false;
        }
        let ok = match (self.objects.get(&id), self.find_object(airfield)) {
            (Some(j), Some(a)) => {
                j.player == player
                    && !j.ai.suspended
                    && a.parking.is_some()
                    && !a.is_dying()
                    && !a.under_construction
                    && a.template.is_collide_kind_of(collide_kind::FS_AIRFIELD)
                    && self.relationship(j.player, a.player) == crate::damage::Relationship::Allies
            }
            _ => false,
        };
        if !ok {
            return false;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack = None;
            o.ai.machine.goal_object = airfield;
            o.ai.machine.last_command_source = CommandSource::Player;
        }
        self.ai_command_set_state(id, AiStateType::Enter);
        true
    }

    /// `aiMoveToPosition(pos, CMD_FROM_AI)`.
    pub(crate) fn jet_ai_move_to_position(&mut self, id: ObjectId, pos: Coord3D) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.attack = None;
            o.combat.attack_move = false;
            o.combat.guard = None;
            o.combat.hunt = false;
            o.ai.machine.goal_position = pos;
            o.ai.machine.goal_object = ObjectId::INVALID;
            o.ai.machine.group_dest = None;
            o.ai.machine.last_command_source = CommandSource::Ai;
        }
        self.ai_command_set_state(id, AiStateType::MoveTo);
    }

    /// `aiFollowExitProductionPath` for a jet (`JetAIUpdate::privateFollowPath(.., exitProduction)`).
    pub(crate) fn jet_follow_exit_production_path(&mut self, id: ObjectId, airfield: ObjectId) {
        if !self.is_jet(id) {
            return;
        }
        self.jet_get_producer_location(id);
        self.jset(id, jet_flag::ALLOW_INTERRUPT_AND_RESUME_OF_CUR_STATE_FOR_RELOAD, false);
        self.jset(id, jet_flag::HAS_PENDING_COMMAND, false);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.ignore_obstacle_id = airfield;
        }
        self.ai_set_last_command_source(id, CommandSource::Ai);
        if self.is_produced_at_helipad(id) {
            self.ai_set_state(id, AiStateType::JetTakingOffAwaitClearance);
        } else {
            self.ai_set_state(id, AiStateType::JetTaxiFromHangar);
        }
    }

    /// `JetAIUpdate::isReloading`.
    pub fn jet_is_reloading(&self, id: ObjectId) -> bool {
        self.ai_current_state(id) == AiStateType::JetReloadAmmo
    }

    // ---- state machine glue ---------------------------------------------------------------------

    /// The `defineState(id, state, success, failure)` table of `JetAIStateMachine` /
    /// `HeliAIStateMachine` (`JetAIUpdate.cpp:1738`, `:1775`).
    pub(crate) fn jet_transition(&self, id: ObjectId, state: AiStateType, success: bool) -> AiStateType {
        use AiStateType::*;
        let runway = self.jet_needs_runway(id);
        let idle = Idle;
        if runway {
            match (state, success) {
                (JetReturningForLanding, true) => JetLandingAwaitClearance,
                (JetReturningForLanding, false) => JetReturnToDeadAirfield,
                (JetTakingOffAwaitClearance, true) => JetTaxiToTakeoff,
                (JetTaxiToTakeoff, true) => JetPauseBeforeTakeoff,
                (JetPauseBeforeTakeoff, true) => JetTakingOff,
                (JetLandingAwaitClearance, true) => JetLanding,
                (JetLanding, true) => JetTaxiFromLanding,
                (JetTaxiFromLanding, true) | (JetTaxiFromHangar, true) => JetOrientForParking,
                (JetOrientForParking, true) => JetReloadAmmo,
                (JetReturnToDeadAirfield, true) => JetCirclingDeadAirfield,
                (JetReturnToDeadAirfield, false) => JetReturnToDeadAirfield,
                _ => idle,
            }
        } else {
            match (state, success) {
                (JetReturningForLanding, true) => JetLandingAwaitClearance,
                (JetReturningForLanding, false) => JetReturnToDeadAirfield,
                (JetTakingOffAwaitClearance, true) => HeliTakingOff,
                (JetLandingAwaitClearance, true) => JetOrientForParking,
                (JetOrientForParking, true) => HeliLanding,
                // TheSuperHackers: LANDING / RELOAD_AMMO failure -> TAKING_OFF.
                (HeliLanding, true) => JetReloadAmmo,
                (HeliLanding, false) => HeliTakingOff,
                (JetReloadAmmo, false) => HeliTakingOff,
                (JetReturnToDeadAirfield, true) => JetCirclingDeadAirfield,
                (JetReturnToDeadAirfield, false) => JetReturnToDeadAirfield,
                _ => idle,
            }
        }
    }

    pub(crate) fn jet_state_enter(&mut self, id: ObjectId, state: AiStateType) -> StateReturn {
        match state {
            AiStateType::JetTakingOffAwaitClearance => self.jet_awaiting_runway_enter(id, false),
            AiStateType::JetLandingAwaitClearance => self.jet_awaiting_runway_enter(id, true),
            AiStateType::JetTaxiToTakeoff => self.jet_taxi_enter(id, Taxi::FromParking),
            AiStateType::JetTaxiFromLanding => self.jet_taxi_enter(id, Taxi::ToParking),
            AiStateType::JetTaxiFromHangar => self.jet_taxi_enter(id, Taxi::FromHangar),
            AiStateType::JetPauseBeforeTakeoff => self.jet_pause_enter(id),
            AiStateType::JetTakingOff => self.jet_takeoff_landing_enter(id, false),
            AiStateType::JetLanding => self.jet_takeoff_landing_enter(id, true),
            AiStateType::HeliTakingOff => self.heli_takeoff_landing_enter(id, false),
            AiStateType::HeliLanding => self.heli_takeoff_landing_enter(id, true),
            AiStateType::JetOrientForParking => self.jet_park_orient_enter(id),
            AiStateType::JetReloadAmmo => self.jet_reload_enter(id),
            AiStateType::JetReturningForLanding => self.jet_return_for_landing_enter(id),
            AiStateType::JetReturnToDeadAirfield => self.jet_return_to_dead_airfield_enter(id),
            AiStateType::JetCirclingDeadAirfield => self.jet_circling_enter(id),
            _ => StateReturn::Success,
        }
    }

    pub(crate) fn jet_state_update(&mut self, id: ObjectId, state: AiStateType) -> StateReturn {
        match state {
            AiStateType::JetTakingOffAwaitClearance => self.jet_awaiting_runway_update(id, false),
            AiStateType::JetLandingAwaitClearance => self.jet_awaiting_runway_update(id, true),
            AiStateType::JetTaxiToTakeoff => self.jet_taxi_update(id, Taxi::FromParking),
            AiStateType::JetTaxiFromLanding => self.jet_taxi_update(id, Taxi::ToParking),
            AiStateType::JetTaxiFromHangar => self.jet_taxi_update(id, Taxi::FromHangar),
            AiStateType::JetPauseBeforeTakeoff => self.jet_pause_update(id),
            AiStateType::JetTakingOff => self.jet_takeoff_landing_update(id, false),
            AiStateType::JetLanding => self.jet_takeoff_landing_update(id, true),
            AiStateType::HeliTakingOff => self.heli_takeoff_landing_update(id, false),
            AiStateType::HeliLanding => self.heli_takeoff_landing_update(id, true),
            AiStateType::JetOrientForParking => self.jet_park_orient_update(id),
            AiStateType::JetReloadAmmo => self.jet_reload_update(id),
            AiStateType::JetReturningForLanding | AiStateType::JetReturnToDeadAirfield => self.ai_internal_move_update_pub(id),
            AiStateType::JetCirclingDeadAirfield => self.jet_circling_update(id),
            _ => StateReturn::Success,
        }
    }

    pub(crate) fn jet_state_exit(&mut self, id: ObjectId, state: AiStateType) {
        match state {
            AiStateType::JetTakingOffAwaitClearance | AiStateType::JetLandingAwaitClearance => {
                self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, false);
                self.jset(id, jet_flag::LANDING_IN_PROGRESS, false);
                self.jset(id, jet_flag::ALLOW_CIRCLING, false);
            }
            AiStateType::JetTaxiToTakeoff | AiStateType::JetTaxiFromLanding | AiStateType::JetTaxiFromHangar => self.jet_taxi_exit(id),
            AiStateType::JetPauseBeforeTakeoff => {
                self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, false);
                self.jset(id, jet_flag::LANDING_IN_PROGRESS, false);
            }
            AiStateType::JetTakingOff => self.jet_takeoff_landing_exit(id, false),
            AiStateType::JetLanding => self.jet_takeoff_landing_exit(id, true),
            AiStateType::HeliTakingOff => self.heli_takeoff_landing_exit(id, false),
            AiStateType::HeliLanding => self.heli_takeoff_landing_exit(id, true),
            AiStateType::JetOrientForParking => {
                self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, false);
                self.jset(id, jet_flag::LANDING_IN_PROGRESS, false);
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.ignore_obstacle_id = ObjectId::INVALID;
                }
            }
            AiStateType::JetReloadAmmo => {
                self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, false);
                self.jset(id, jet_flag::LANDING_IN_PROGRESS, false);
            }
            _ => {}
        }
    }

    // ---- JetAwaitingRunwayState ---------------------------------------------------------------------

    fn jet_awaiting_runway_enter(&mut self, id: ObjectId, landing: bool) -> StateReturn {
        if !self.jet_needs_runway(id) {
            return StateReturn::Success; // the helicopters' `SuccessState`
        }
        self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, !landing);
        self.jset(id, jet_flag::LANDING_IN_PROGRESS, landing);
        self.jset(id, jet_flag::ALLOW_CIRCLING, true);
        StateReturn::Continue
    }

    fn jet_awaiting_runway_update(&mut self, id: ObjectId, landing: bool) -> StateReturn {
        if self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return StateReturn::Failure;
        }
        let Some(af) = self.jet_getpp(self.jet_producer(id)) else {
            return StateReturn::Failure; // TheSuperHackers: `!RETAIL_COMPATIBLE_CRC`
        };
        // "gotta reserve a space in order to reserve a runway"
        let offset = self.jet_parking_offset(id);
        if self.pp_reserve_space(af, id, offset).is_none() {
            return StateReturn::Failure;
        }
        if self.pp_reserve_runway(af, id, landing) {
            return StateReturn::Success;
        }
        // "can't get a runway? gotta wait."
        self.jet_set_locomotor_goal_none(id);
        StateReturn::Continue
    }

    // ---- JetOrHeliTaxiState -----------------------------------------------------------------------

    fn jet_taxi_enter(&mut self, id: ObjectId, mode: Taxi) -> StateReturn {
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.can_path_through_units = true;
        }
        self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, mode != Taxi::ToParking);
        self.jset(id, jet_flag::LANDING_IN_PROGRESS, mode == Taxi::ToParking);
        self.jset(id, jet_flag::TAXI_IN_PROGRESS, true);
        if mode == Taxi::ToParking {
            // "Instantly reload flares."
            self.reload_countermeasures(id);
        }
        self.jset(id, jet_flag::ALLOW_AIR_LOCO, false);
        self.jet_choose_locomotor_set(id, LocomotorSet::Taxiing);

        let Some(af) = self.jet_getpp(self.jet_producer(id)) else {
            return StateReturn::Failure; // TheSuperHackers: `!RETAIL_COMPATIBLE_CRC`
        };
        let offset = self.jet_parking_offset(id);
        let Some(info) = self.pp_reserve_space(af, id, offset) else {
            return StateReturn::Failure; // full?
        };

        let mut intermed: Option<Coord3D> = None;
        let orient = atan2(info.runway_prep.y - info.parking_space.y, info.runway_prep.x - info.parking_space.x);
        if angle_diff(orient, info.parking_orientation).abs() > PI / 128.0 {
            let z = (info.parking_space.z + info.runway_prep.z) * 0.5;
            intermed = intersect_infinite_line_2d(
                info.parking_space.x,
                info.parking_space.y,
                info.parking_orientation,
                info.runway_prep.x,
                info.runway_prep.y,
                info.parking_orientation + PI / 2.0,
            )
            .map(|(x, y)| Coord3D::new(x, y, z));
        }

        // `movePath`: the jet's own position first (not followed), then the nodes.
        let mut nodes: Vec<Coord3D> = Vec::new();
        match mode {
            Taxi::ToParking => {
                nodes.push(info.runway_prep);
                if let Some(p) = intermed {
                    nodes.push(p);
                }
                nodes.push(info.parking_space);
            }
            Taxi::FromParking => {
                if let Some(p) = intermed {
                    nodes.push(p);
                }
                nodes.push(info.runway_prep);
                nodes.push(info.runway_start);
            }
            Taxi::FromHangar => nodes.push(info.parking_space),
        }

        let producer = self.jet_producer(id);
        self.jet_loco_flags(id, true, true, true);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.path = nodes;
            o.ai.raw_path = None;
            o.ai.flight_goal = None; // "movePath->prependNode(&pos)": the path starts where the jet is
            o.ai.ignore_obstacle_id = producer;
            // `setAdjustsDestination(false)` ('precision is necessary') before
            // `AIMoveOutOfTheWayState::onEnter` (`JetAIUpdate.cpp:634`). That state sets it back to
            // `true` (`AIStates.cpp:2150`), but `AIInternalMoveToState::onEnter` then turns it off for
            // the ultra-accurate locomotor set just above (`AIStates.cpp:1629-1632`), so the taxi never
            // adjusts its nodes or keeps a goal cell.
            o.ai.adjusts_destination = false;
        }
        // `AIMoveOutOfTheWayState::onEnter`: continue the path that was just set (and
        // `setAdjustsDestination(true)`).
        let ret = self.ai_move_out_of_way_enter_pub(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.is_final_goal = false;
        }
        ret
    }

    fn jet_taxi_update(&mut self, id: ObjectId, _mode: Taxi) -> StateReturn {
        if self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return StateReturn::Failure;
        }
        // `calcBestParkingAssignment` is only non-trivial for the carrier's `FlightDeckBehavior`.
        // `AIMoveOutOfTheWayState::update`.
        self.ai_internal_move_update_pub(id)
    }

    fn jet_taxi_exit(&mut self, id: ObjectId) {
        self.jet_loco_flags(id, false, false, false);
        self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, false);
        self.jset(id, jet_flag::LANDING_IN_PROGRESS, false);
        self.jset(id, jet_flag::TAXI_IN_PROGRESS, false);
        // `AIMoveOutOfTheWayState::onExit` (`AIStates.cpp:2185-2194`): the base exit (already run by
        // `ai_state_exit`), then `destroyPath(); setCanPathThroughUnits(false); clearMoveOutOfWay();`.
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.can_path_through_units = false;
            o.ai.path.clear();
            o.ai.raw_path = None;
            o.ai.move_goal = None;
            o.ai.move_out_of_way = [ObjectId::INVALID, ObjectId::INVALID];
        }
    }

    // ---- JetPauseBeforeTakeoffState ------------------------------------------------------------------

    fn jet_pause_enter(&mut self, id: ObjectId) -> StateReturn {
        self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, true);
        self.jset(id, jet_flag::LANDING_IN_PROGRESS, false);
        if let Some(o) = self.objects.get_mut(&id) {
            o.jet.when_takeoff = 0;
            o.jet.when_transfer = 0;
            o.jet.waited_for_taxi_id = ObjectId::INVALID;
        }
        let Some(af) = self.jet_getpp(self.jet_producer(id)) else {
            return StateReturn::Success; // "no airfield? just skip this step."
        };
        let offset = self.jet_parking_offset(id);
        let Some(info) = self.pp_reserve_space(af, id, offset) else {
            return StateReturn::Success; // "full?"
        };
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.goal_position = info.runway_end;
            o.ai.machine.goal_object = ObjectId::INVALID;
        }
        StateReturn::Continue
    }

    /// `findJetToWaitFor` (TheSuperHackers, `JetAIUpdate.cpp:1336`): a jet on another runway that is
    /// still taxiing to takeoff.
    fn jet_find_jet_to_wait_for(&mut self, id: ObjectId) -> Option<ObjectId> {
        let af = self.jet_getpp(self.jet_producer(id))?;
        let this_runway = self.pp_runway_index(af, id);
        for runway in 0..self.pp_runway_count(af) {
            if Some(runway) == this_runway {
                continue;
            }
            let other = self.pp_runway_reservation(af, runway);
            if other == ObjectId::INVALID || self.find_object(other).is_none() {
                continue;
            }
            if self.objects.get(&other).is_some_and(|o| o.template.jet.is_some() && o.ai.machine.current == AiStateType::JetTaxiToTakeoff) {
                return Some(other);
            }
        }
        None
    }

    fn jet_pause_update(&mut self, id: ObjectId) -> StateReturn {
        if self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return StateReturn::Failure;
        }
        // "always call this." `AIFaceState::update` toward the runway end.
        let super_status = self.jet_face_update(id);
        if let Some(other) = self.jet_find_jet_to_wait_for(id) {
            let waited = self.objects.get(&id).map_or(ObjectId::INVALID, |o| o.jet.waited_for_taxi_id);
            if waited == ObjectId::INVALID {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.jet.waited_for_taxi_id = other;
                }
            }
            let waited = self.objects.get(&id).map_or(ObjectId::INVALID, |o| o.jet.waited_for_taxi_id);
            if waited == other {
                return StateReturn::Continue; // "Wait for the other Jet to get ready."
            }
        }
        let now = self.frame;
        let takeoff_pause = self.objects.get(&id).and_then(|o| o.template.jet.as_ref()).map_or(0, |j| j.takeoff_pause);
        let (when_takeoff, when_transfer, waited) = self.objects.get(&id).map_or((0, 0, ObjectId::INVALID), |o| (o.jet.when_takeoff, o.jet.when_transfer, o.jet.waited_for_taxi_id));
        if when_takeoff == 0 {
            // "The other Jet (if any) is ready. Prepare runway transfer and takeoff."
            let transfer;
            let waited_new;
            if waited == ObjectId::INVALID {
                waited_new = id;
                transfer = now + 1;
            } else {
                waited_new = waited;
                transfer = now + 2; // "2 seems odd, but is correct"
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.jet.waited_for_taxi_id = waited_new;
                o.jet.when_transfer = transfer;
                o.jet.when_takeoff = transfer.max(now + takeoff_pause);
            }
            // `friend_enableAfterburners(true)` is a client model condition.
        } else {
            if now >= when_transfer {
                // "once we start the final wait, release the runways for guys behind us"
                if let Some(af) = self.jet_getpp(self.jet_producer(id)) {
                    self.pp_transfer_runway_to_next(af, id);
                }
                if let Some(o) = self.objects.get_mut(&id) {
                    o.jet.when_transfer = u32::MAX; // `~0u`
                }
            }
            if now >= when_takeoff {
                return super_status; // "Ready to take off!"
            }
        }
        StateReturn::Continue
    }

    /// `AIFaceState::update` (`AIStates.cpp:7483`) for the jet's pause: face the goal position,
    /// turning in place (`setLocomotorGoalOrientation`) when the locomotor has no minimum speed.
    fn jet_face_update(&mut self, id: ObjectId) -> StateReturn {
        let Some(o) = self.objects.get(&id) else { return StateReturn::Failure };
        let pos = o.ai.machine.goal_position;
        let rel = angle_diff(atan2(pos.y - o.position.y, pos.x - o.position.x), o.angle);
        if rel.abs() < crate::combat::AIM_REL_THRESH {
            return StateReturn::Success;
        }
        let can_turn_in_place = o.locomotor().is_some_and(|l| l.min_speed == 0.0);
        let angle = o.angle;
        if can_turn_in_place {
            self.ai_set_locomotor_goal_orientation(id, angle + rel);
        } else {
            self.ai_set_locomotor_goal_explicit(id, pos);
        }
        StateReturn::Continue
    }

    // ---- JetTakeoffOrLandingState (an AIFollowPathState) -------------------------------------------

    fn jet_takeoff_landing_enter(&mut self, id: ObjectId, landing: bool) -> StateReturn {
        if self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return StateReturn::Failure;
        }
        self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, !landing);
        self.jset(id, jet_flag::LANDING_IN_PROGRESS, landing);
        self.jset(id, jet_flag::ALLOW_AIR_LOCO, true);
        self.jet_choose_locomotor_set(id, LocomotorSet::Normal);
        match self.objects.get_mut(&id) {
            Some(o) => o.loco_rt.max_lift = BIGNUM, // `loco->setMaxLift(BIGNUM)`
            None => return StateReturn::Failure,
        }
        // `m_maxLift = loco->getMaxLift(bdt)` / `m_maxSpeed = loco->getMaxSpeedForCondition(bdt)` (`JetAIUpdate.cpp:737-738`).
        let l = self.locomotor_now(id);
        let (lift, speed, min_speed) = (l.map_or(0.0, |l| l.lift), l.map_or(0.0, |l| l.max_speed), l.map_or(0.0, |l| l.min_speed));
        if let Some(o) = self.objects.get_mut(&id) {
            o.jet.saved_max_lift = lift;
            o.jet.saved_max_speed = speed;
            if landing {
                o.loco_rt.max_speed = min_speed; // `loco->setMaxSpeed(loco->getMinSpeed())`
            } else {
                o.loco_rt.max_lift = 0.0; // `loco->setMaxLift(0)`
            }
        }
        let producer = self.jet_producer(id);
        self.jet_loco_flags(id, true, true, false);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.ignore_obstacle_id = producer;
        }
        let Some(af) = self.jet_getpp(producer) else {
            return StateReturn::Failure; // TheSuperHackers: `!RETAIL_COMPATIBLE_CRC`
        };
        let offset = self.jet_parking_offset(id);
        let Some(mut info) = self.pp_reserve_space(af, id, offset) else {
            return StateReturn::Failure; // "it's full."
        };
        // "only check this for landing; we might have already given up the reservation to the guy behind us for takeoff"
        if landing && !self.pp_reserve_runway(af, id, true) {
            return StateReturn::Failure;
        }
        let path: Vec<Coord3D> = if landing {
            // `CIRCLE_FOR_LANDING` is not defined: start at the approach point, land the way we took off, reversed.
            vec![info.runway_approach, info.runway_end, info.runway_start]
        } else {
            info.runway_end.z = info.runway_approach.z;
            vec![info.runway_end, info.runway_exit]
        };
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.goal_path = path;
        }
        self.ai_follow_path_enter_pub(id)
    }

    fn jet_takeoff_landing_update(&mut self, id: ObjectId, landing: bool) -> StateReturn {
        if self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return StateReturn::Failure;
        }
        if landing {
            if let Some(o) = self.objects.get_mut(&id) {
                o.loco_rt.max_lift = BIGNUM;
            }
        } else {
            let producer = self.jet_producer(id);
            if let Some(af) = self.jet_getpp(producer) {
                self.pp_transfer_runway_to_next(af, id);
                // "Calculate the distance of the jet from the end of the runway as a ratio from the
                // start. As it approaches the end of the runway, the plane will gain more lift..."
                if let Some(info) = self.pp_calc_info(af, id) {
                    let pos = self.objects.get(&id).map_or(Coord3D::ZERO, |o| o.position);
                    let v = Coord3D::new(info.runway_end.x - pos.x, info.runway_end.y - pos.y, info.runway_end.z - pos.z);
                    let dist = sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
                    let mut ratio = 1.0 - (dist / info.runway_takeoff_dist);
                    ratio *= ratio; // "dampen it...."
                    ratio = ratio.clamp(0.0, 1.0);
                    let saved = self.objects.get(&id).map_or(0.0, |o| o.jet.saved_max_lift);
                    if let Some(o) = self.objects.get_mut(&id) {
                        o.loco_rt.max_lift = saved * ratio;
                    }
                }
            }
        }
        self.ai_follow_path_update_pub(id)
    }

    fn jet_takeoff_landing_exit(&mut self, id: ObjectId, landing: bool) {
        // `AIFollowPathState::onExit` first.
        self.ai_follow_path_exit_pub(id);
        self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, false);
        self.jset(id, jet_flag::LANDING_IN_PROGRESS, false);
        // "Paranoia checks"; "don't restore lift if dead -- this may fight with JetSlowDeathBehavior!"
        let dead = self.objects.get(&id).is_none_or(|o| o.is_dying());
        if let Some(o) = self.objects.get_mut(&id) {
            o.loco_rt.precise_z = false;
            o.loco_rt.ultra_accurate = false;
            if !dead {
                o.loco_rt.max_lift = BIGNUM;
            }
            o.ai.ignore_obstacle_id = ObjectId::INVALID;
        }
        let producer = self.jet_producer(id);
        let keeps = self.objects.get(&id).and_then(|o| o.template.jet.as_ref()).is_none_or(|j| j.keeps_parking_space_when_airborne);
        if let Some(af) = self.jet_getpp(producer) {
            if !landing && !keeps {
                self.pp_release_space(af, id);
            }
            self.pp_release_runway(af, id);
        }
    }

    // ---- HeliTakeoffOrLandingState ------------------------------------------------------------------

    fn heli_takeoff_landing_enter(&mut self, id: ObjectId, landing: bool) -> StateReturn {
        self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, !landing);
        self.jset(id, jet_flag::LANDING_IN_PROGRESS, landing);
        self.jset(id, jet_flag::ALLOW_AIR_LOCO, true);
        self.jet_choose_locomotor_set(id, LocomotorSet::Normal);
        let producer = self.jet_producer(id);
        self.jet_loco_flags(id, true, true, false);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.ignore_obstacle_id = producer;
        }
        let Some(af) = self.jet_getpp(producer) else {
            return StateReturn::Failure; // TheSuperHackers: `!RETAIL_COMPATIBLE_CRC`
        };
        let (approach_height, deck) = self.objects.get(&af).and_then(|o| o.template.parking_place).map_or((0.0, 0.0), |p| (p.approach_height, p.landing_deck_height_offset));
        let (parking_loc, parking_orient, approach);
        if self.is_produced_at_helipad(id) {
            let (pos, ang, landing_pos) = match self.objects.get(&id) {
                Some(o) => (o.position, o.angle, o.jet.landing_pos_helipad),
                None => return StateReturn::Failure,
            };
            if landing {
                parking_loc = landing_pos;
                parking_orient = ang;
            } else {
                parking_orient = ang;
                parking_loc = pos;
            }
            let mut a = parking_loc;
            a.z += approach_height + deck;
            approach = a;
        } else {
            let offset = self.jet_parking_offset(id);
            let Some(info) = self.pp_reserve_space(af, id, offset) else {
                return StateReturn::Failure;
            };
            parking_loc = info.parking_space;
            parking_orient = info.parking_orientation;
            let mut a = parking_loc;
            a.z += info.runway_approach.z - info.runway_end.z;
            approach = a;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            if landing {
                o.jet.heli_path = [approach, parking_loc];
            } else {
                o.jet.heli_path = [parking_loc, approach];
            }
            o.jet.heli_index = 0;
            o.jet.heli_parking_loc = parking_loc;
            o.jet.heli_parking_orientation = parking_orient;
        }
        StateReturn::Continue
    }

    fn heli_takeoff_landing_update(&mut self, id: ObjectId, _landing: bool) -> StateReturn {
        if self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return StateReturn::Failure;
        }
        let Some((idx, goal, pos)) = self.objects.get(&id).map(|o| (usize::from(o.jet.heli_index), o.jet.heli_path[usize::from(o.jet.heli_index.min(1))], o.position)) else {
            return StateReturn::Failure;
        };
        // `adjustDestination` / `updateGoal` (helipad takeoff or any takeoff) only claim pathfinder cells.
        self.ai_set_locomotor_goal_explicit(id, goal);
        const THRESH: f32 = 3.0;
        let (dx, dy, dz) = (pos.x - goal.x, pos.y - goal.y, pos.z - goal.z);
        let mut idx = idx;
        if dx * dx + dy * dy + dz * dz <= THRESH * THRESH {
            idx += 1;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.jet.heli_index = idx.min(255) as u8;
        }
        if idx >= 2 {
            return StateReturn::Success;
        }
        StateReturn::Continue
    }

    fn heli_takeoff_landing_exit(&mut self, id: ObjectId, landing: bool) {
        self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, false);
        self.jset(id, jet_flag::LANDING_IN_PROGRESS, false);
        let dead = self.objects.get(&id).is_none_or(|o| o.is_dying());
        if let Some(o) = self.objects.get_mut(&id) {
            o.loco_rt.precise_z = false;
            o.loco_rt.ultra_accurate = false;
            if !dead {
                o.loco_rt.max_lift = BIGNUM;
            }
            o.ai.ignore_obstacle_id = ObjectId::INVALID;
        }
        let producer = self.jet_producer(id);
        let keeps = self.objects.get(&id).and_then(|o| o.template.jet.as_ref()).is_none_or(|j| j.keeps_parking_space_when_airborne);
        if landing {
            self.jset(id, jet_flag::ALLOW_AIR_LOCO, false);
            self.jet_choose_locomotor_set(id, LocomotorSet::Taxiing);
        } else if let Some(af) = self.jet_getpp(producer) {
            if !keeps {
                self.pp_release_space(af, id);
            }
        }
    }

    // ---- JetOrHeliParkOrientState ---------------------------------------------------------------------

    fn jet_park_orient_enter(&mut self, id: ObjectId) -> StateReturn {
        if self.is_produced_at_helipad(id) {
            return StateReturn::Success;
        }
        self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, false);
        self.jset(id, jet_flag::LANDING_IN_PROGRESS, true);
        let producer = self.jet_producer(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.ignore_obstacle_id = producer;
        }
        StateReturn::Continue
    }

    fn jet_park_orient_update(&mut self, id: ObjectId) -> StateReturn {
        if self.objects.get(&id).is_none_or(|o| o.is_dying()) {
            return StateReturn::Failure;
        }
        let Some(af) = self.jet_getpp(self.jet_producer(id)) else { return StateReturn::Failure };
        let offset = self.jet_parking_offset(id);
        let Some(info) = self.pp_reserve_space(af, id, offset) else { return StateReturn::Failure };
        const THRESH: f32 = 0.001;
        let angle = self.objects.get(&id).map_or(0.0, |o| o.angle);
        if angle_diff(angle, info.parking_orientation).abs() <= THRESH {
            return StateReturn::Success;
        }
        // "magically position it correctly."
        self.scrub_velocity_2d(id, 0.0);
        if let Some(o) = self.objects.get_mut(&id) {
            let mut hover = info.parking_space;
            hover.z = o.position.z;
            o.set_position(hover);
        }
        self.ai_set_locomotor_goal_orientation(id, info.parking_orientation);
        StateReturn::Continue
    }

    // ---- JetOrHeliReloadAmmoState ------------------------------------------------------------------------

    fn jet_reload_enter(&mut self, id: ObjectId) -> StateReturn {
        self.jset(id, jet_flag::TAKEOFF_IN_PROGRESS, false);
        self.jset(id, jet_flag::LANDING_IN_PROGRESS, false);
        self.jset(id, jet_flag::USE_SPECIAL_RETURN_LOCO, false);
        let now = self.frame;
        let mut reload_time = 0u32;
        if let Some(o) = self.objects.get(&id) {
            for w in o.combat.weapons.weapons.iter().flatten() {
                let remaining = w.ammo_in_clip();
                let clip = w.template.clip_size;
                // `Weapon::getClipReloadTime(jet)`: `floor(m_clipReloadTime / bonus.getField(RATE_OF_FIRE))`.
                let rof = self.combined_weapon_bonus(id, &w.template).get(crate::bonus::WeaponBonusField::RateOfFire);
                let mut rt = floor(w.template.clip_reload_time as f32 / if rof > 0.0 { rof } else { 1.0 }) as i32;
                if clip > 0 {
                    // "bias by amount empty."
                    let needed = clip - remaining;
                    rt = (rt * needed) / clip;
                }
                if rt > reload_time as i32 {
                    reload_time = rt as u32;
                }
            }
        }
        if reload_time < 1 {
            reload_time = 1;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.jet.reload_time = reload_time;
            o.jet.reload_done_frame = reload_time + now;
        }
        StateReturn::Continue
    }

    fn jet_reload_update(&mut self, id: ObjectId) -> StateReturn {
        if self.jet_getpp(self.jet_producer(id)).is_none() {
            return StateReturn::Failure; // TheSuperHackers: `!RETAIL_COMPATIBLE_CRC`
        }
        let now = self.frame;
        let (reload_time, done_frame) = self.objects.get(&id).map_or((1, 0), |o| (o.jet.reload_time, o.jet.reload_done_frame));
        let mut all_done = true;
        if let Some(o) = self.objects.get_mut(&id) {
            for w in o.combat.weapons.weapons.iter_mut().flatten() {
                if now >= done_frame {
                    w.set_clip_percent_full(1.0, false, now);
                } else {
                    w.set_clip_percent_full((reload_time - (done_frame - now)) as f32 / reload_time as f32, false, now);
                }
                if w.ammo_in_clip() != w.template.clip_size {
                    all_done = false;
                }
            }
        }
        if all_done {
            StateReturn::Success
        } else {
            StateReturn::Continue
        }
    }

    // ---- JetOrHeliReturnForLandingState (an AIInternalMoveToState) -----------------------------------

    fn jet_return_for_landing_enter(&mut self, id: ObjectId) -> StateReturn {
        let producer = self.jet_producer(id);
        let mut af = self.jet_getpp(producer);
        if af.is_none() {
            // "nuke the producer id, since it's dead"
            self.set_producer(id, ObjectId::INVALID);
            match self.jet_find_suitable_airfield(id) {
                Some(a) if self.jet_getpp(a).is_some() => {
                    self.set_producer(id, a);
                    af = Some(a);
                }
                _ => return StateReturn::Failure,
            }
        }
        let af = af.expect("set above");
        let goal = if self.is_produced_at_helipad(id) {
            self.objects.get(&id).map_or(Coord3D::ZERO, |o| o.jet.landing_pos_helipad)
        } else {
            let offset = self.jet_parking_offset(id);
            let Some(info) = self.pp_reserve_space(af, id, offset) else { return StateReturn::Failure };
            if self.jet_needs_runway(id) { info.runway_approach } else { info.parking_space }
        };
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.goal_position = goal;
            o.ai.machine.goal_object = ObjectId::INVALID;
        }
        // `setAdjustsDestination(false)`: "precision is necessary".
        self.ai_internal_move_enter_pub(id, goal, false)
    }

    // ---- dead airfield -------------------------------------------------------------------------------------

    fn jet_return_to_dead_airfield_enter(&mut self, id: ObjectId) -> StateReturn {
        let goal = self.objects.get(&id).map_or(Coord3D::ZERO, |o| o.jet.producer_location);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.goal_position = goal;
            o.ai.machine.goal_object = ObjectId::INVALID;
        }
        self.ai_internal_move_enter_pub(id, goal, true)
    }

    fn jet_circling_enter(&mut self, id: ObjectId) -> StateReturn {
        // "obscure case: if the jet wasn't spawned, but just placed directly on the map, it might not
        // have an owning airfield, and it might be trying to return simply due to being idle, not out
        // of ammo. so check and don't die in that case, but just punt back out to idle."
        if !self.jet_is_out_of_special_reload_ammo(id) && self.jet_producer(id) == ObjectId::INVALID {
            return StateReturn::Failure;
        }
        // "just stay where we are."
        self.jet_set_locomotor_goal_none(id);
        if let Some(o) = self.objects.get_mut(&id) {
            o.jet.check_airfield = HOW_OFTEN_TO_CHECK;
        }
        StateReturn::Continue
    }

    fn jet_circling_update(&mut self, id: ObjectId) -> StateReturn {
        self.jet_set_locomotor_goal_none(id);
        let (rate, max_health) = {
            let Some(o) = self.objects.get(&id) else { return StateReturn::Failure };
            (o.template.jet.as_ref().map_or(0.0, |j| j.out_of_ammo_damage_per_second), o.body.as_ref().map_or(0.0, |b| b.max_health()))
        };
        if rate > 0.0 {
            // "convert to damage/sec to damage/frame ... since it's a percentage, multiply times the max health"
            let amount = rate * (1.0 / LOGICFRAMES_PER_SECOND as f32) * max_health;
            self.damage_object_with_death(
                id,
                crate::body::DamageInfo { source: ObjectId::INVALID, amount, kind: crate::damage::DamageType::Unresistable },
                crate::damage::DeathType::Normal,
            );
        }
        let check = {
            let Some(o) = self.objects.get_mut(&id) else { return StateReturn::Failure };
            o.jet.check_airfield -= 1;
            o.jet.check_airfield <= 0
        };
        if check {
            if let Some(o) = self.objects.get_mut(&id) {
                o.jet.check_airfield = HOW_OFTEN_TO_CHECK;
            }
            if let Some(af) = self.jet_find_suitable_airfield(id) {
                self.set_producer(id, af);
                return StateReturn::Success;
            }
        }
        StateReturn::Continue
    }

}

/// `TaxiType` (`JetAIUpdate.cpp:54`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Taxi {
    FromParking,
    ToParking,
    FromHangar,
}

/// `intersectInfiniteLine2D` (`JetAIUpdate.cpp:444`): where the infinite line through `(ax, ay)` at
/// heading `ao` meets the one through `(cx, cy)` at `co`; `None` when they are parallel.
fn intersect_infinite_line_2d(ax: f32, ay: f32, ao: f32, cx: f32, cy: f32, co: f32) -> Option<(f32, f32)> {
    let bx = ax + cos(ao);
    let by = ay + sin(ao);
    let dx = cx + cos(co);
    let dy = cy + sin(co);
    let denom = (bx - ax) * (dy - cy) - (by - ay) * (dx - cx);
    if denom == 0.0 {
        return None;
    }
    let r = ((ay - cy) * (dx - cx) - (ax - cx) * (dy - cy)) / denom;
    Some((ax + r * (bx - ax), ay + r * (by - ay)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::ObjectTemplate;
    use crate::terrain::TerrainLogic;
    use std::sync::Arc;

    /// `JetOrHeliTaxiState::onExit` (`JetAIUpdate.cpp:686-694`) calls `AIMoveOutOfTheWayState::onExit`
    /// (`AIStates.cpp:2185-2194`), whose tail is `destroyPath(); setCanPathThroughUnits(false);
    /// clearMoveOutOfWay();`. The port's taxi exit dropped the path and the flag but kept both
    /// `m_moveOutOfWay` ids, so a parked jet kept avoiding a stale object.
    /// Revert check: without the `move_out_of_way` reset the ids stay set.
    #[test]
    fn the_taxi_exit_clears_the_move_out_of_way_ids() {
        let mut g = GameLogic::new(TerrainLogic::flat(100, 100, 0), 1);
        let id = g.create_object(&Arc::new(ObjectTemplate::new("Jet")), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let ahead = Coord3D::new(60.0, 60.0, 0.0);
        if let Some(o) = g.objects.get_mut(&id) {
            o.ai.can_path_through_units = true;
            o.ai.path = vec![ahead];
            o.ai.raw_path = Some(crate::pathfind::Path {
                nodes: vec![
                    crate::pathfind::PathNode { pos: Coord3D::new(50.0, 50.0, 0.0), can_optimize: true, next_optimized: Some(1) },
                    crate::pathfind::PathNode { pos: ahead, can_optimize: true, next_optimized: None },
                ],
                blocked_by_ally: false,
            });
            o.ai.move_out_of_way = [ObjectId(7), ObjectId(8)];
        }
        g.jet_taxi_exit(id);
        let o = g.find_object(id).unwrap();
        assert_eq!(o.ai.move_out_of_way, [ObjectId::INVALID, ObjectId::INVALID], "clearMoveOutOfWay");
        assert!(!o.ai.can_path_through_units, "setCanPathThroughUnits(false)");
        assert!(!o.ai.has_path(), "destroyPath");
    }
}
