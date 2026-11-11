//! `AIGuardRetaliateMachine` (`AIGuardRetaliate.cpp` / `AIGuardRetaliate.h`) and its top-level
//! `AI_GUARD_RETALIATE` state (`AIGuardRetaliateState`, `AIStates.cpp:6798-6936`).
//!
//! The engine's state owns a sub-machine whose states are `AI_GUARD_RETALIATE_ATTACK_AGGRESSOR`
//! (the default), `_RETURN`, `_IDLE`, `_INNER`, `_OUTER` and `_GET_CRATE`
//! (`AIGuardRetaliate.h:49-58`). The port keeps the main `AiStateType::GuardRetaliate` state for the
//! whole order — C++ `getCurrentStateID()` stays `AI_GUARD_RETALIATE` while its sub-machine attacks —
//! and runs the sub-machine from here, the way `crate::ai_states` runs the `AIGuardMachine` from
//! `AI_GUARD`. That means the snapshot's `ai=` prints `GuardRetaliate` for the whole order, as the
//! C++ writer prints `AI_GUARD_RETALIATE` (docs/HARNESS.md, "AI state names").
//!
//! The sub-machine's attacks are `AIAttackState`s with `GuardRetaliateExitConditions`
//! (`AIGuardRetaliate.cpp:113-166`): the goal object's distance from the guard position and the
//! owner's own distance from it (never beyond the standard guard radius), the chase give-up frame
//! (`AIData GuardChaseUnitsDuration`) and "no goal object" combine per state. The port's
//! `AttackExitConditions` (`combat.rs`) covers the first three; the owner-radius test is added here
//! ([`GameLogic::retaliate_exit_should_exit`]).
//!
//! Not ported:
//! - the enter-guard / hijack-guard branches (`isEnterGuard` / `isHijackGuard`,
//!   `AIGuardRetaliate.cpp:256-270`, `:379-395`): `ObjectTemplate` does not carry the flags yet (only
//!   the four shipped GLA Hijacker templates set them, `EnterGuard = Yes` + `HijackGuard = Yes`), so a
//!   hijacker takes the attack branch here where the engine would enter vehicles;
//! - `AIPickUpCrateState` (`AI_PICK_UP_CRATE` is not ported): retail's `checkForCrateToPickup` clears
//!   the crate id before looking it up and always returns null (`AIUpdate.cpp:893`), so the state
//!   succeeds straight into `AI_GUARD_RETALIATE_RETURN`;
//! - `AIGuardRetaliateMachine::isIdle` compares the state id with `AI_IDLE` and so is always false
//!   (`AIGuardRetaliate.cpp:205-212`); nothing in the sim asks it.

use crate::ai_states::{AiStateType, CommandSource, GuardTargetType, StateReturn};
use crate::combat::{AttackExitConditions, AttackTarget};
use crate::damage::Relationship;
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::ObjectId;
use crate::partition::DistanceMode;
use crate::pathfind::PathObject;
use crate::template::kind_of;

/// `NO_MAX_SHOTS_LIMIT` (`Weapon.h`): `ActiveBody`'s retaliation order (`ActiveBody.cpp:722`).
pub(crate) const NO_MAX_SHOTS_LIMIT: i32 = -1;

/// The `AIGuardRetaliateMachine` sub-state ids (`AIGuardRetaliate.h:49-58`). The crate state keeps
/// its own variant even though its enter always succeeds into `Return` (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetaliateSub {
    /// `AI_GUARD_RETALIATE_INNER` (5000): attack anything inside the inner circle.
    Inner,
    /// `AI_GUARD_RETALIATE_IDLE` (5001): scan for a target, then end the order or go to `Inner`.
    Idle,
    /// `AI_GUARD_RETALIATE_OUTER` (5002): chase the target until the give-up frame.
    Outer,
    /// `AI_GUARD_RETALIATE_RETURN` (5003): move back to the guarded position, scanning on the way.
    Return,
    /// `AI_GUARD_RETALIATE_GET_CRATE` (5004): pick up a crate (`AIPickUpCrateState`, not ported).
    GetCrate,
    /// `AI_GUARD_RETALIATE_ATTACK_AGGRESSOR` (5005): attack the object that attacked us.
    AttackAggressor,
}

impl RetaliateSub {
    /// The engine's `StateID` (`AIGuardRetaliate.h:52-57`).
    pub fn id(self) -> u32 {
        match self {
            RetaliateSub::Inner => 5000,
            RetaliateSub::Idle => 5001,
            RetaliateSub::Outer => 5002,
            RetaliateSub::Return => 5003,
            RetaliateSub::GetCrate => 5004,
            RetaliateSub::AttackAggressor => 5005,
        }
    }
}

/// The persistent `AIGuardRetaliateMachine` payload (`AIGuardRetaliate.h:89-121`) plus its states'
/// fields (`AIGuardRetaliate.h:141-217`). One instance lives on [`crate::ai_states::AiMachine`] while
/// the main state is `GuardRetaliate`; the C++ state creates and destroys the machine in its
/// `onEnter` / `onExit`.
#[derive(Clone, Debug, PartialEq)]
pub struct RetaliateMachine {
    /// The machine's current sub-state (`StateMachine::m_currentState`'s id).
    pub sub: RetaliateSub,
    /// `m_positionToGuard`.
    pub position_to_guard: Coord3D,
    /// `m_nemesisToAttack`.
    pub nemesis: ObjectId,
    /// The presence of `m_attackState` in `Inner` / `Outer` / `AttackAggressor` (the enter state is
    /// not modelled; the enter-guard branch is unported).
    pub attack_active: bool,
    /// `GuardRetaliateExitConditions::m_center` of the current attack: the guard position.
    pub attack_center: Coord3D,
    /// `GuardRetaliateExitConditions::m_radiusSqr`.
    pub attack_radius_sqr: f32,
    /// `GuardRetaliateExitConditions::m_attackGiveUpFrame`.
    pub attack_give_up_frame: u32,
    /// `AIGuardRetaliateIdleState::m_nextEnemyScanTime`.
    pub next_enemy_scan_time: u32,
    /// `AIGuardRetaliateReturnState::m_nextReturnScanTime`.
    pub next_return_scan_time: u32,
}

impl Default for RetaliateMachine {
    fn default() -> Self {
        Self {
            // "order matters: first state is the default state" (`AIGuardRetaliate.cpp:183-186`).
            sub: RetaliateSub::AttackAggressor,
            position_to_guard: Coord3D::ZERO,
            nemesis: ObjectId::INVALID,
            attack_active: false,
            attack_center: Coord3D::ZERO,
            attack_radius_sqr: 0.0,
            attack_give_up_frame: 0,
            next_enemy_scan_time: 0,
            next_return_scan_time: 0,
        }
    }
}

impl RetaliateMachine {
    pub(crate) fn crc(&self, c: &mut crate::crc::Crc) {
        c.add_u32(self.sub.id());
        c.add_coord(&self.position_to_guard);
        c.add_u32(self.nemesis.0);
        c.add_bool(self.attack_active);
        c.add_coord(&self.attack_center);
        c.add_f32(self.attack_radius_sqr);
        c.add_u32(self.attack_give_up_frame);
        c.add_u32(self.next_enemy_scan_time);
        c.add_u32(self.next_return_scan_time);
    }
}

impl GameLogic {
    /// `AIUpdateInterface::aiGuardRetaliate` (`AI.h:592-599`) -> `aiDoCommand`'s
    /// `AICMD_GUARD_RETALIATE` (`AIUpdate.cpp:2708-2710`) -> `privateGuardRetaliate`
    /// (`AIUpdate.cpp:3470-3486`): clear the machine, make `victim` the goal object, guard `pos`,
    /// record the command source and enter `AI_GUARD_RETALIATE`.
    ///
    /// `max_shots` is the engine's `m_intValue` (`NO_MAX_SHOTS_LIMIT` from `ActiveBody.cpp:722`); a
    /// positive limit would be stored on the current weapon (`Weapon::setMaxShotCount`), which the
    /// port does not model. `pos` is *not* clipped for `CMD_FROM_AI` (`setGoalPositionClipped`,
    /// `AIUpdate.cpp:351-389`): only `CMD_FROM_PLAYER` clamps to the map edge.
    pub(crate) fn ai_guard_retaliate_order(&mut self, id: ObjectId, victim: ObjectId, pos: Coord3D, max_shots: i32, source: CommandSource) {
        // `isAllowedToRespondToAiCommands` (`AIUpdate.cpp:2592-2598`): "the dead don't listen very
        // well". The mood-sleep and `ForbidPlayerCommands` vetoes are not modelled by the port's
        // other command entry points either.
        if self.objects.get(&id).is_none_or(|o| o.effectively_dead || o.is_destroyed()) {
            return;
        }
        // `privateGuardRetaliate`: `if (!victim) return;`.
        if !self.objects.contains_key(&victim) {
            return;
        }
        let _ = max_shots; // see the doc comment
        self.ai_clear(id);
        if let Some(o) = self.objects.get_mut(&id) {
            // `setGoalObject(victim)` also copies the victim's position into the goal
            // (`StateMachine::internalSetGoalObject`, StateMachine.cpp:766-775); the following
            // `setGoalPositionClipped(pos)` overwrites it with the guarded position.
            o.ai.machine.goal_object = victim;
            o.ai.machine.goal_position = pos;
            o.ai.machine.last_command_source = source;
        }
        // `getStateMachine()->setState(AI_GUARD_RETALIATE)` (`AIUpdate.cpp:3480`).
        self.ai_command_set_state(id, AiStateType::GuardRetaliate);
    }

    /// `AIGuardRetaliateState::onEnter` (`AIStates.cpp:6882-6903`): build the sub-machine with the
    /// machine's goal position as the place to guard and the goal object as the nemesis, then enter
    /// its default state. The sub-machine's default is `AI_GUARD_RETALIATE_ATTACK_AGGRESSOR`
    /// (`AIGuardRetaliate.cpp:183-186`).
    pub(crate) fn ai_guard_retaliate_enter(&mut self, id: ObjectId) -> StateReturn {
        let Some((pos, goal)) = self.objects.get(&id).map(|o| (o.ai.machine.goal_position, o.ai.machine.goal_object)) else {
            return StateReturn::Failure;
        };
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.retaliate = RetaliateMachine { position_to_guard: pos, nemesis: goal, ..RetaliateMachine::default() };
        }
        let status = self.retaliate_sub_enter(id, RetaliateSub::AttackAggressor);
        self.retaliate_follow(id, RetaliateSub::AttackAggressor, status, 0)
    }

    /// `AIGuardRetaliateState::update` (`AIStates.cpp:6916-6936`): the jet out-of-ammo check, then one
    /// step of the sub-machine.
    pub(crate) fn ai_guard_retaliate_update(&mut self, id: ObjectId) -> StateReturn {
        // "if all of our weapons are out of ammo, can't attack. (this can happen for units which
        // never auto-reload, like the Raptor)" (`AIStates.cpp:6925-6932`). The template's
        // `isEnterGuard` half of the condition is unported (see the module docs).
        if self.is_jet(id) && self.is_out_of_ammo(id) && self.objects.get(&id).is_some_and(|o| !o.is_kind_of(kind_of::PROJECTILE)) {
            return StateReturn::Failure;
        }
        let Some(sub) = self.objects.get(&id).map(|o| o.ai.machine.retaliate.sub) else {
            return StateReturn::Failure;
        };
        let status = match sub {
            RetaliateSub::AttackAggressor | RetaliateSub::Inner => self.retaliate_attack_update(id),
            RetaliateSub::Outer => {
                // `AIGuardRetaliateOuterState::update` (`AIGuardRetaliate.cpp:538-558`): reset the
                // give-up frame while the victim is still inside the standard guard radius, then run
                // the attack.
                self.retaliate_outer_refresh_give_up(id);
                self.retaliate_attack_update(id)
            }
            RetaliateSub::Return => self.retaliate_return_update(id),
            RetaliateSub::Idle => self.retaliate_idle_update(id),
            // `AIGuardRetaliatePickUpCrateState::update` -> `AIPickUpCrateState::update` (unported; no
            // crate can exist, so the enter already succeeded into `Return`).
            RetaliateSub::GetCrate => StateReturn::Success,
        };
        self.retaliate_follow(id, sub, status, 0)
    }

    /// `AIGuardRetaliateState::onExit` (`AIStates.cpp:6906-6913`): destroy the sub-machine (its
    /// destructor runs the current sub-state's `onExit(EXIT_RESET)`,
    /// `StateMachine.cpp:281-295`) and clear the guard target type.
    pub(crate) fn ai_guard_retaliate_exit(&mut self, id: ObjectId) {
        let sub = self.objects.get(&id).map_or(RetaliateSub::AttackAggressor, |o| o.ai.machine.retaliate.sub);
        self.retaliate_sub_exit(id, sub);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.retaliate = RetaliateMachine::default();
            // `obj->getAI()->clearGuardTargetType()` (`AIUpdate.h:362`).
            o.ai.machine.guard_target_type = GuardTargetType::None;
        }
    }

    // ---- the sub-machine -------------------------------------------------------------------

    /// `StateMachine::internalSetState` (`StateMachine.cpp:562-657`) for the sub-machine: run the old
    /// state's exit, the new state's enter and then its transitions.
    fn retaliate_set_sub(&mut self, id: ObjectId, new: RetaliateSub, depth: u32) -> StateReturn {
        let old = self.objects.get(&id).map_or(RetaliateSub::AttackAggressor, |o| o.ai.machine.retaliate.sub);
        self.retaliate_sub_exit(id, old);
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.retaliate.sub = new;
        }
        let status = self.retaliate_sub_enter(id, new);
        self.retaliate_follow(id, new, status, depth)
    }

    /// `State::friend_checkForTransitions` / `friend_checkForSleepTransitions`
    /// (`StateMachine.cpp:110-197`, `:203-...)`: follow the sub-state's `(success, failure)` targets,
    /// check the `attackAggressors` conditions attached to `Return` and `Idle` (retail,
    /// `AIGuardRetaliate.cpp:187-194`) and report `EXIT_MACHINE_WITH_SUCCESS` up to the main machine.
    fn retaliate_follow(&mut self, id: ObjectId, sub: RetaliateSub, status: StateReturn, depth: u32) -> StateReturn {
        // `friend_checkForTransitions`' recursion guard returns FAILURE past 20 transitions
        // (`StateMachine.cpp:115-119`); the machine's graph is acyclic apart from the crate loop, whose
        // enter never loops, so this is only a backstop.
        if depth > 20 {
            return StateReturn::Failure;
        }
        match status {
            StateReturn::Continue | StateReturn::Sleep(_) => {
                let has_aggressor_condition = matches!(sub, RetaliateSub::Return | RetaliateSub::Idle);
                if has_aggressor_condition && self.retaliate_check_aggressor_condition(id) {
                    self.retaliate_set_sub(id, RetaliateSub::AttackAggressor, depth + 1)
                } else {
                    status
                }
            }
            StateReturn::Success => match sub {
                // `AI_GUARD_RETALIATE_IDLE`'s success target is `AI_GUARD_RETALIATE_INNER`; its failure
                // target is `EXIT_MACHINE_WITH_SUCCESS` (`AIGuardRetaliate.cpp:194`), handled below.
                RetaliateSub::AttackAggressor => self.retaliate_set_sub(id, RetaliateSub::Return, depth + 1),
                RetaliateSub::Return => self.retaliate_set_sub(id, RetaliateSub::Idle, depth + 1),
                RetaliateSub::Idle => self.retaliate_set_sub(id, RetaliateSub::Inner, depth + 1),
                RetaliateSub::Inner => self.retaliate_set_sub(id, RetaliateSub::Outer, depth + 1),
                RetaliateSub::Outer => self.retaliate_set_sub(id, RetaliateSub::GetCrate, depth + 1),
                RetaliateSub::GetCrate => self.retaliate_set_sub(id, RetaliateSub::Return, depth + 1),
            },
            StateReturn::Failure => match sub {
                RetaliateSub::AttackAggressor => self.retaliate_set_sub(id, RetaliateSub::Return, depth + 1),
                RetaliateSub::Return => self.retaliate_set_sub(id, RetaliateSub::Inner, depth + 1),
                // `EXIT_MACHINE_WITH_SUCCESS` (`StateMachine.cpp:144-147`): the sub-machine exits with
                // success, so the enclosing `AI_GUARD_RETALIATE` state succeeds into `AI_IDLE`
                // (`AIStates.cpp:720`).
                RetaliateSub::Idle => StateReturn::Success,
                RetaliateSub::Inner => self.retaliate_set_sub(id, RetaliateSub::Outer, depth + 1),
                RetaliateSub::Outer => self.retaliate_set_sub(id, RetaliateSub::GetCrate, depth + 1),
                RetaliateSub::GetCrate => self.retaliate_set_sub(id, RetaliateSub::Return, depth + 1),
            },
        }
    }

    fn retaliate_sub_enter(&mut self, id: ObjectId, sub: RetaliateSub) -> StateReturn {
        match sub {
            RetaliateSub::AttackAggressor => self.retaliate_attack_aggressor_enter(id),
            RetaliateSub::Return => self.retaliate_return_enter(id),
            RetaliateSub::Idle => self.retaliate_idle_enter(id),
            RetaliateSub::Inner => self.retaliate_inner_enter(id),
            RetaliateSub::Outer => self.retaliate_outer_enter(id),
            RetaliateSub::GetCrate => self.retaliate_get_crate_enter(id),
        }
    }

    /// Each sub-state's `onExit` (`AIGuardRetaliate.cpp:441-461`, `:561-569`, `:636-639`, `:717-720`,
    /// `:754-757`, `:843-857`).
    fn retaliate_sub_exit(&mut self, id: ObjectId, sub: RetaliateSub) {
        match sub {
            // `AttackAggressor` and `Inner` exit their attack state (which ends the attack order in the
            // port) and clear the team's shared target.
            RetaliateSub::AttackAggressor | RetaliateSub::Inner => {
                self.retaliate_end_attack(id);
                self.set_team_target_object(id, None);
            }
            // `Outer` exits its attack state; it has no team-target clear.
            RetaliateSub::Outer => self.retaliate_end_attack(id),
            // `AIGuardRetaliateReturnState::onExit` calls `AIInternalMoveToState::onExit`
            // (`AIGuardRetaliate.cpp:636-639`): `friend_endingMove`.
            RetaliateSub::Return => self.ai_internal_move_exit(id),
            // `AIGuardRetaliateIdleState::onExit` and `...PickUpCrateState::onExit` are empty
            // (`:717-720`, `:754-757`) — note the crate state's override deliberately skips the base
            // move exit.
            RetaliateSub::Idle | RetaliateSub::GetCrate => {}
        }
    }

    // ---- attack states ---------------------------------------------------------------------

    /// `AIGuardRetaliateAttackAggressorState::onEnter` (`AIGuardRetaliate.cpp:785-832`).
    fn retaliate_attack_aggressor_enter(&mut self, id: ObjectId) -> StateReturn {
        let Some(o) = self.objects.get(&id) else { return StateReturn::Success };
        let mut nemesis = o.ai.machine.retaliate.nemesis;
        if !self.objects.contains_key(&nemesis) {
            // `if (!nemesis && body && body->getLastDamageInfo()->in.m_sourceID && ... != DAMAGE_HEALING)`
            // (`:794-803`). The port's `body` is the ActiveBody (an InactiveBody's
            // `getLastDamageInfo` is null, `BodyModule.h:230`).
            let info = o.last_damage_info;
            if o.body.is_some() && info.source != ObjectId::INVALID && info.kind != crate::damage::DamageType::Healing {
                if let Some(target) = self.objects.get(&info.source) {
                    // `nemesis = findObjectByID(nemID)` happens whether or not it is an enemy; the
                    // machine's nemesis is only updated for an enemy (`:799-802`).
                    if self.relationship(o.player, target.player) == Relationship::Enemies {
                        if let Some(o) = self.objects.get_mut(&id) {
                            o.ai.machine.retaliate.nemesis = info.source;
                        }
                    }
                    nemesis = info.source;
                }
            }
        }
        if !self.objects.contains_key(&nemesis) {
            return StateReturn::Success;
        }
        let Some(pos) = self.objects.get(&id).map(|o| o.ai.machine.retaliate.position_to_guard) else {
            return StateReturn::Success;
        };
        // `Real range = TheAI->getAdjustedVisionRangeForObject(obj, OWNERTYPE | MOOD)` (`:813`).
        let range = self.ai_guard_outer_range(id);
        let std = self.ai_guard_range(id);
        let exit = AttackExitConditions {
            exit_if_no_unit_found: true,
            exit_if_expired_duration: true,
            exit_if_outside_radius: true,
            center: pos,
            radius_sqr: (range + std) * (range + std),
            give_up_frame: self.frame + self.ai_tunables.guard_chase_unit_frames,
        };
        self.retaliate_begin_attack(id, nemesis, exit)
    }

    /// `AIGuardRetaliateInnerState::onEnter` (`AIGuardRetaliate.cpp:376-423`), attack branch. The
    /// enter-guard branch is not ported (see the module docs).
    fn retaliate_inner_enter(&mut self, id: ObjectId) -> StateReturn {
        let Some(m) = self.objects.get(&id).map(|o| o.ai.machine.retaliate.clone()) else {
            return StateReturn::Success;
        };
        if !self.objects.contains_key(&m.nemesis) {
            // "Unexpected null nemesis in AIGuardRetaliateInnerState." -> STATE_SUCCESS (`:401-405`).
            return StateReturn::Success;
        }
        // `m_exitConditions.m_radiusSqr = sqr(1.5f * getStdGuardRange(...))` and the two conditions
        // (`:406-409`); no duration.
        let std = self.ai_guard_range(id);
        let exit = AttackExitConditions {
            exit_if_no_unit_found: true,
            exit_if_expired_duration: false,
            exit_if_outside_radius: true,
            center: m.position_to_guard,
            radius_sqr: 1.5 * std * 1.5 * std,
            give_up_frame: 0,
        };
        self.retaliate_begin_attack(id, m.nemesis, exit)
    }

    /// `AIGuardRetaliateOuterState::onEnter` (`AIGuardRetaliate.cpp:498-535`).
    fn retaliate_outer_enter(&mut self, id: ObjectId) -> StateReturn {
        let Some(m) = self.objects.get(&id).map(|o| o.ai.machine.retaliate.clone()) else {
            return StateReturn::Success;
        };
        if !self.objects.contains_key(&m.nemesis) {
            return StateReturn::Success;
        }
        let range = self.ai_guard_outer_range(id);
        let std = self.ai_guard_range(id);
        let exit = AttackExitConditions {
            exit_if_no_unit_found: true,
            exit_if_expired_duration: true,
            exit_if_outside_radius: true,
            center: m.position_to_guard,
            radius_sqr: (0.67 * (range + std)) * (0.67 * (range + std)),
            give_up_frame: self.frame + self.ai_tunables.guard_chase_unit_frames,
        };
        self.retaliate_begin_attack(id, m.nemesis, exit)
    }

    /// `AIAttackState::onEnter`'s checks and the state's building for one of the retaliate attack
    /// states (`AIStates.cpp:5516-5592`; the calls are `AIGuardRetaliate.cpp:411`, `:525`, `:822`, all
    /// with `follow = FALSE`, `attackingObject = TRUE`, `forceAttacking = FALSE`).
    fn retaliate_begin_attack(&mut self, id: ObjectId, nemesis: ObjectId, exit: AttackExitConditions) -> StateReturn {
        // "if we've met the conditions specified by m_attackParameters, we consider ourselves
        // 'successful.'" (`:5526-5528`).
        if self.retaliate_exit_should_exit(id, &exit) {
            return StateReturn::Success;
        }
        // `AIAttackState::onEnter` (`:5530-5541`): an under-construction unit fails; so does an
        // out-of-ammo one. `isAbleToAttack` covers the former (and the disabled cases).
        if !self.is_able_to_attack(id) {
            return StateReturn::Failure;
        }
        if self.is_out_of_ammo(id) && self.objects.get(&id).is_some_and(|o| !o.is_kind_of(kind_of::PROJECTILE)) {
            return StateReturn::Failure;
        }
        // `chooseWeapon()` (`:5571-5573`): no usable weapon fails the attack.
        let Some(player) = self.objects.get(&id).map(|o| o.player) else { return StateReturn::Failure };
        let Some(info) = self.target_info(&AttackTarget::Object(nemesis), player, false) else {
            return StateReturn::Failure;
        };
        if self.objects.get(&id).is_none_or(|o| Self::choose_weapon(o, Some(&info), self.frame, None).is_none()) {
            return StateReturn::Failure;
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.retaliate.attack_active = true;
            o.ai.machine.retaliate.attack_center = exit.center;
            o.ai.machine.retaliate.attack_radius_sqr = exit.radius_sqr;
            o.ai.machine.retaliate.attack_give_up_frame = exit.give_up_frame;
        }
        // The attack order lives on `ObjectCombat` and is stepped by `ai_attack_update_object`, the same
        // seam the `AIGuardMachine` states use (`AIGuard.cpp:431/560/861`).
        self.ai_guard_begin_attack(id, nemesis, exit, false);
        if self.objects.get(&id).is_some_and(|o| o.combat.attack.is_none()) {
            // The attack's entry failed: the retaliate states' `onEnter` return SUCCESS (AIGuardRetaliate.cpp:411-418).
            self.retaliate_end_attack(id);
            return StateReturn::Success;
        }
        StateReturn::Continue
    }

    /// `AIAttackState::update`'s exit conditions (`AIStates.cpp:5600-5607`) for a live retaliate
    /// attack, including the owner-radius half of `GuardRetaliateExitConditions::shouldExit`
    /// (`AIGuardRetaliate.cpp:135-163`). Returns the sub-state's status.
    fn retaliate_attack_update(&mut self, id: ObjectId) -> StateReturn {
        let Some(active) = self.objects.get(&id).map(|o| o.ai.machine.retaliate.attack_active) else {
            return StateReturn::Success;
        };
        if !active {
            return StateReturn::Success;
        }
        let exit = self.retaliate_attack_exit(id);
        if self.retaliate_exit_should_exit(id, &exit) {
            self.retaliate_end_attack(id);
            return StateReturn::Success;
        }
        // "if all of our weapons are out of ammo, can't attack" (`AIStates.cpp:5611-5616`).
        if self.is_out_of_ammo(id) && self.objects.get(&id).is_some_and(|o| !o.is_kind_of(kind_of::PROJECTILE)) {
            self.retaliate_end_attack(id);
            return StateReturn::Failure;
        }
        self.ai_attack_update_object(id);
        if self.objects.get(&id).is_none_or(|o| o.combat.attack.is_none()) {
            // The order's own exit conditions fired inside the attack pass and ended it; the sub-state
            // transition reports success (`AIAttackState::update`).
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.retaliate.attack_active = false;
            }
            return StateReturn::Success;
        }
        StateReturn::Continue
    }

    /// `GuardRetaliateExitConditions::shouldExit` (`AIGuardRetaliate.cpp:113-166`) for the machine's
    /// stored attack conditions.
    fn retaliate_exit_should_exit(&self, id: ObjectId, exit: &AttackExitConditions) -> bool {
        let Some(o) = self.objects.get(&id) else { return true };
        let machine = &o.ai.machine.retaliate;
        // `if (!machine->getGoalObject())` (`:115-126`): the attack's goal object is the nemesis.
        let Some(goal) = self.objects.get(&machine.nemesis) else {
            return exit.exit_if_no_unit_found;
        };
        if exit.exit_if_expired_duration && self.frame >= exit.give_up_frame {
            return true;
        }
        if exit.exit_if_outside_radius {
            // The aggressor's 2D distance from the centre (`:137-157`; z is forced to 0 by the C++).
            let dx = goal.position.x - exit.center.x;
            let dy = goal.position.y - exit.center.y;
            if dx * dx + dy * dy > exit.radius_sqr {
                return true;
            }
            // "Never go beyond standard guard radius." (`:158-162`).
            let mx = o.position.x - exit.center.x;
            let my = o.position.y - exit.center.y;
            let std = self.ai_guard_range(id);
            if mx * mx + my * my > std * std {
                return true;
            }
        }
        false
    }

    /// The `m_exitConditions` of the machine's live attack, as the port's attack order copy
    /// (`AttackOrder::exit`).
    fn retaliate_attack_exit(&self, id: ObjectId) -> AttackExitConditions {
        let Some(o) = self.objects.get(&id) else { return AttackExitConditions::default() };
        let machine = &o.ai.machine.retaliate;
        // The state's condition flags: `Inner` has no duration; the other two have all three. The
        // flags are recoverable from the sub-state (the C++ keeps one `GuardRetaliateExitConditions`
        // per state and mutates it).
        let (no_unit, duration, outside) = match machine.sub {
            RetaliateSub::Inner => (true, false, true),
            RetaliateSub::Outer | RetaliateSub::AttackAggressor => (true, true, true),
            _ => (false, false, false),
        };
        AttackExitConditions {
            exit_if_no_unit_found: no_unit,
            exit_if_expired_duration: duration,
            exit_if_outside_radius: outside,
            center: machine.attack_center,
            radius_sqr: machine.attack_radius_sqr,
            give_up_frame: machine.attack_give_up_frame,
        }
    }

    /// End the attack order the sub-machine owns and clear the presence flag. The retaliate Inner, Outer and AttackAggressor
    /// exits call only `m_attackState->onExit` and delete it (`AIGuardRetaliate.cpp:441-448`, `:561-567`), an internal
    /// transition with no wake: the attack's own exit decides the move (`end_attack_with`, lane 191 R27 F2).
    fn retaliate_end_attack(&mut self, id: ObjectId) {
        if self.objects.get(&id).is_some_and(|o| o.combat.attack.is_some()) {
            self.end_attack_internal(id);
        }
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.retaliate.attack_active = false;
        }
    }

    /// `AIGuardRetaliateOuterState::update`'s give-up reset (`AIGuardRetaliate.cpp:538-558`).
    fn retaliate_outer_refresh_give_up(&mut self, id: ObjectId) {
        let Some(m) = self.objects.get(&id).map(|o| o.ai.machine.retaliate.clone()) else { return };
        // `AIGuardRetaliateOuterState::update`: `if (m_attackState==nullptr) return STATE_SUCCESS;`
        // (`AIGuardRetaliate.cpp:540`).
        if !m.attack_active {
            return;
        }
        let Some(goal) = self.objects.get(&m.nemesis) else { return };
        let dx = m.attack_center.x - goal.position.x;
        let dy = m.attack_center.y - goal.position.y;
        let dz = m.attack_center.z - goal.position.z;
        let std = self.ai_guard_range(id);
        if dx * dx + dy * dy + dz * dz <= std * std {
            let give_up = self.frame + self.ai_tunables.guard_chase_unit_frames;
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.retaliate.attack_give_up_frame = give_up;
                // The order holds a copy of the conditions (`AttackExitConditions`), so the running
                // attack's give-up frame must move with the state's.
                if let Some(a) = o.combat.attack.as_mut() {
                    a.exit.give_up_frame = give_up;
                }
            }
        }
    }

    // ---- return / idle / crate -------------------------------------------------------------

    /// `AIGuardRetaliateReturnState::onEnter` (`AIGuardRetaliate.cpp:600-618`).
    fn retaliate_return_enter(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        // `now + GameLogicRandomValue(0, m_guardEnemyReturnScanRate)` (`:603`).
        let off = self.rng.value(0, self.ai_tunables.guard_enemy_return_scan_rate as i32) as u32;
        let Some(pos) = self.objects.get(&id).map(|o| o.ai.machine.retaliate.position_to_guard) else {
            return StateReturn::Failure;
        };
        // `if (ai && ai->isDoingGroundMovement()) pathfinder()->adjustDestination(owner, locomotorSet,
        // &m_goalPosition)` (`:611-615`). This is the engine's *first* adjust: it ignores failure and
        // does not snap; `AIInternalMoveToState::onEnter` adjusts again with the snap fallback
        // (`AIStates.cpp:1661-1666`).
        let goal = if self.is_doing_ground_movement(id) { self.retaliate_adjust_destination(id, pos) } else { pos };
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.retaliate.next_return_scan_time = now + off;
            // `AIInternalMoveToState::m_goalPosition` is the move's goal in the port.
            o.ai.machine.goal_position = goal;
        }
        // `setAdjustsDestination(true); return AIInternalMoveToState::onEnter();` (`:616-617`).
        self.ai_internal_move_enter_pub(id, goal, true)
    }

    /// `AIGuardRetaliateReturnState::update` (`AIGuardRetaliate.cpp:621-633`): scan for an inner
    /// target at `m_guardEnemyReturnScanRate`; finding one fails the state into `Inner`, otherwise the
    /// move runs.
    fn retaliate_return_update(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        let next = self.objects.get(&id).map_or(0, |o| o.ai.machine.retaliate.next_return_scan_time);
        if now >= next {
            let rate = self.ai_tunables.guard_enemy_return_scan_rate;
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.retaliate.next_return_scan_time = now + rate;
            }
            if self.retaliate_look_for_inner_target(id) {
                return StateReturn::Failure;
            }
        }
        self.ai_internal_move_update_pub(id)
    }

    /// `AIGuardRetaliateIdleState::onEnter` (`AIGuardRetaliate.cpp:670-678`).
    fn retaliate_idle_enter(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        let off = self.rng.value(0, self.ai_tunables.guard_enemy_scan_rate as i32) as u32;
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.retaliate.next_enemy_scan_time = now + off;
        }
        StateReturn::Continue
    }

    /// `AIGuardRetaliateIdleState::update` (`AIGuardRetaliate.cpp:681-714`).
    fn retaliate_idle_update(&mut self, id: ObjectId) -> StateReturn {
        let now = self.frame;
        let next = self.objects.get(&id).map_or(0, |o| o.ai.machine.retaliate.next_enemy_scan_time);
        if now < next {
            return StateReturn::Sleep(next - now);
        }
        let rate = self.ai_tunables.guard_enemy_scan_rate;
        if let Some(o) = self.objects.get_mut(&id) {
            o.ai.machine.retaliate.next_enemy_scan_time = now + rate;
        }
        // "Check to see if we have created a crate we need to pick up" (`:696-701`): a pending crate id
        // goes to `AI_GUARD_RETALIATE_GET_CRATE` via `setState` inside the update; the state change
        // discards the sleep result (`StateMachine.cpp:452-455`). `checkForCrateToPickup` always
        // returns null, and the crate state's enter then succeeds into `Return`.
        if self.objects.get(&id).is_some_and(|o| o.ai.crate_created != ObjectId::INVALID) {
            let _ = self.retaliate_set_sub(id, RetaliateSub::GetCrate, 0);
            return StateReturn::Continue;
        }
        // `if (getGuardMachine()->lookForInnerTarget()) return STATE_SUCCESS; else return STATE_FAILURE;`
        // (`:703-711`): success goes to `Inner`, failure exits the whole machine with success.
        if self.retaliate_look_for_inner_target(id) {
            StateReturn::Success
        } else {
            StateReturn::Failure
        }
    }

    /// `AIGuardRetaliatePickUpCrateState::onEnter` (`AIGuardRetaliate.cpp:731-745`); the crate state
    /// always returns null in retail (`AIUpdate.cpp:893`), so this is `STATE_SUCCESS` into `Return`.
    fn retaliate_get_crate_enter(&mut self, id: ObjectId) -> StateReturn {
        let _ = self.check_for_crate_to_pickup(id);
        StateReturn::Success
    }

    // ---- targeting --------------------------------------------------------------------------

    /// `AIGuardRetaliateMachine::lookForInnerTarget` (`AIGuardRetaliate.cpp:224-305`): the closest
    /// attackable enemy within `getStdGuardRange` of the guarded position (2D, from the centre), or
    /// the team's shared target. Sets the machine's nemesis on success.
    fn retaliate_look_for_inner_target(&mut self, id: ObjectId) -> bool {
        if !self.is_able_to_attack(id) {
            return false;
        }
        // "Check if team auto targets same victim." (`:232-242`): no attackability test here.
        if self.team_attacks_common_target(id) {
            if let Some(v) = self.get_team_target_object(id) {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.ai.machine.retaliate.nemesis = v;
                }
                return true;
            }
        }
        let Some((center, player)) = self.objects.get(&id).map(|o| (o.ai.machine.retaliate.position_to_guard, o.player)) else {
            return false;
        };
        // `getStdGuardRange` (`:215-221`): `AI_VISIONFACTOR_OWNERTYPE | MOOD | GUARDINNER`.
        let range = self.ai_guard_range(id);
        // `PartitionFilterSameMapStatus` (`PartitionManager.cpp:5360-5364`) has no off-map objects to
        // test in the port.
        for cid in self.partition.objects_in_range(&center, range, DistanceMode::FromCenter2D) {
            if cid == id {
                continue;
            }
            let Some(v) = self.objects.get(&cid) else { continue };
            if v.destroyed || self.relationship(player, v.player) != Relationship::Enemies {
                continue;
            }
            // `PartitionFilterRejectBuildings` (`PartitionManager.cpp:5020-5063`, also applied by the
            // attack-guard branch at `AIGuardRetaliate.cpp:276`): a structure is rejected unless the
            // owner is a computer player, it is a fire base, or it is a garrisoned building that can
            // attack.
            if v.is_kind_of(kind_of::STRUCTURE) {
                let accepted = self.ai_players.contains_key(&player)
                    || v.is_kind_of(kind_of::FS_BASE_DEFENSE)
                    || (v.template.contain.is_some() && self.is_able_to_attack(cid));
                if !accepted {
                    continue;
                }
            }
            // `PartitionFilterPossibleToAttack(ATTACK_NEW_TARGET, owner, CMD_FROM_AI)`
            // (`PartitionManager.cpp:5452-5465`) -> `getAbleToAttackSpecificObject`: the stealth rule
            // (`WeaponSet.cpp:512-540`) and the weapon's targetability (`chooseWeapon`).
            if self.is_stealthed(cid) {
                continue;
            }
            let Some(info) = self.target_info(&AttackTarget::Object(cid), player, false) else { continue };
            if self.objects.get(&id).is_none_or(|o| Self::choose_weapon(o, Some(&info), self.frame, None).is_none()) {
                continue;
            }
            if let Some(o) = self.objects.get_mut(&id) {
                o.ai.machine.retaliate.nemesis = cid;
            }
            return true;
        }
        false
    }

    /// `hasAttackedMeAndICanReturnFire` (`AIGuardRetaliate.cpp:58-107`): the `attackAggressors`
    /// transition condition of `Return` and `Idle`. Clears the body's clearable last attacker as a
    /// side effect (even when the object it names is gone), so it fires once per hit.
    fn retaliate_check_aggressor_condition(&mut self, id: ObjectId) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        // `BodyModuleInterface *bmi = obj->getBodyModule(); if (!(obj && bmi)) return FALSE;`
        // (`:60-65`). An InactiveBody's `getClearableLastAttacker` is invalid; the port's `body` is the
        // ActiveBody.
        if o.body.is_none() {
            return false;
        }
        let attacker = o.clearable_last_attacker();
        if attacker == ObjectId::INVALID {
            return false;
        }
        // `Object *target = TheGameLogic->findObjectByID(...); bmi->clearLastAttacker();` (`:72-73`).
        if let Some(o) = self.objects.get_mut(&id) {
            o.clear_last_attacker();
        }
        let Some(target) = self.objects.get(&attacker) else { return false };
        let Some(player) = self.objects.get(&id).map(|o| o.player) else { return false };
        if self.relationship(player, target.player) != Relationship::Enemies {
            return false;
        }
        if target.effectively_dead {
            return false;
        }
        if !self.is_able_to_attack(id) {
            return false;
        }
        let Some(info) = self.target_info(&AttackTarget::Object(attacker), player, false) else { return false };
        self.objects.get(&id).is_some_and(|o| Self::choose_weapon(o, Some(&info), self.frame, None).is_some())
    }

    /// The first `adjustDestination` of `AIGuardRetaliateReturnState::onEnter` (`:611-615`): no snap
    /// fallback, and an air-only unit's destination is not adjusted (the port's path helper has no
    /// aircraft adjust; `ai_adjust_move_goal` makes the same exception).
    fn retaliate_adjust_destination(&mut self, id: ObjectId, goal: Coord3D) -> Coord3D {
        let po = {
            let Some(o) = self.objects.get(&id) else { return goal };
            let air_only = o.locomotor().is_some_and(|l| l.is_airborne()) && o.template.kind_of & kind_of::AIRCRAFT != 0;
            if air_only {
                return goal;
            }
            let Some(mut po) = PathObject::for_object(o) else { return goal };
            po.ignore_obstacle = o.ai.ignore_obstacle_id;
            po
        };
        self.pathfinder.adjust_destination(&self.terrain, &po, &goal, None).unwrap_or(goal)
    }
}
