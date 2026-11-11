//! The AI mood-check timer: `AIUpdateInterface::m_nextMoodCheckTime` and `m_randomlyOffsetMoodCheck`
//! (`GameLogic/Object/Update/AIUpdate.cpp`). `ai_scan` (`combat.rs`) reads and advances the timer in
//! `getNextMoodTarget` (AIUpdate.cpp:4603-4614); these are the setters the rest of the sim calls.

use crate::ai_states::AiStateType;
use crate::logic::GameLogic;
use crate::object::{status, ObjectId};

impl AiStateType {
    /// `State::isIdle` (`StateMachine.h:150`), overridden to true only by `AIIdleState`
    /// (`AIStateMachine.h:228`). `AI_IDLE` is that class's only state id, so only the `Idle` variant is an idle
    /// state: the guard machine's `AI_GUARD_IDLE` and the attack-move / attack-follow sub-machines' `AI_IDLE`
    /// are different states of *other* machines and do not make the object idle (see [`GameLogic::is_ai_idle`]).
    fn is_idle_state(self) -> bool {
        self == AiStateType::Idle
    }
}

impl GameLogic {
    /// `AIUpdateInterface::getNextMoodCheckTime`: the frame of the next mood check (the idle state sleeps until it,
    /// AIStates.cpp:1457).
    pub(crate) fn next_mood_check_time(&self, id: ObjectId) -> u32 {
        self.objects.get(&id).map_or(0, |o| o.combat.next_mood_check)
    }

    /// `AIUpdateInterface::resetNextMoodCheckTime` (AIUpdate.cpp:4502): the next check is `ForceIdleMSEC` frames
    /// away (`m_forceIdleFramesCount`, default 1), and that check is offset at random. Called by
    /// `AIIdleState::onEnter` (AIStates.cpp:1321).
    pub(crate) fn reset_next_mood_check_time(&mut self, id: ObjectId) {
        let now = self.frame;
        let force = self.ai_tunables.force_idle_frames;
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.next_mood_check = now + force;
            o.combat.randomly_offset_mood_check = true;
        }
    }

    /// `AIUpdateInterface::wakeUpAndAttemptToTarget` (AIUpdate.cpp:4483): only an idle unit checks for a target
    /// now (`if (!isIdle()) return`), with a random offset. Called by the transport exit (TransportContain.cpp:407)
    /// and by the stealth reveal (`StealthUpdate.cpp:854`, `crate::behaviors::GameLogic::mark_detected`).
    pub(crate) fn wake_up_and_attempt_to_target(&mut self, id: ObjectId) {
        if !self.is_ai_idle(id) {
            return;
        }
        let now = self.frame;
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.next_mood_check = now;
            o.combat.randomly_offset_mood_check = true;
        }
    }

    /// `AIUpdateInterface::setNextMoodCheckTime` (AIUpdate.cpp:4506): the next check is `frame`, and this check
    /// is not randomly offset. Used by the `AIGroup` stealth paths (`AIGroup.cpp:1777`, `:2123`).
    pub(crate) fn set_next_mood_check_time(&mut self, id: ObjectId, frame: u32) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.next_mood_check = frame;
            o.combat.randomly_offset_mood_check = false;
        }
    }

    /// `AIUpdateInterface::canAutoAcquire` (`AIUpdate.h:592`): any `AutoAcquireEnemiesWhenIdle` bit is set
    /// (`AAS_Idle`, `AAS_Idle_Attack_Buildings`, `AAS_Idle_Not_While_Attacking`, `AAS_Idle_Stealthed`). The engine's
    /// mask also has `AAS_Idle_No` (0x04), so a unit with only `NO` makes `canAutoAcquire()` true; the port does not
    /// carry that bit, which is unobservable here because such a unit never passes the idle caller's `AAS_Idle`
    /// gate, so the delayed timer it would set is never read.
    pub(crate) fn can_auto_acquire(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| {
            let c = &o.template.combat;
            c.auto_acquire_enemies || c.auto_acquire_buildings || c.auto_acquire_not_while_attacking || c.auto_acquire_while_stealthed
        })
    }

    /// The stealth-unit special case of `AIGroup::groupMoveToPosition` (`AIGroup.cpp:1759-1781`) and
    /// `AIGroup::groupIdle` (`AIGroup.cpp:2105-2127`), which runs only for a player order (`CMD_FROM_PLAYER`):
    /// a combat stealth unit that is not stealthed and not detected, and does not auto-acquire while stealthed,
    /// must not auto-acquire until it can stealth again. The mood check moves to `now + StealthDelay +
    /// GameLogicRandomValue(0, LOGICFRAMES_PER_SECOND)` and is not randomly offset. The engine runs this before
    /// `aiMoveToPosition` in a group move and after `aiIdle` in a group stop; both callers keep that order.
    pub(crate) fn group_stealth_mood_delay(&mut self, id: ObjectId) {
        let now = self.frame;
        let (bits, delay) = match self.objects.get(&id) {
            Some(o) => (o.status_mask(now), o.template.stealth.as_ref().map(|s| s.stealth_delay)),
            None => return,
        };
        if bits & status::CAN_STEALTH == 0 || !self.can_auto_acquire(id) {
            return;
        }
        if bits & (status::STEALTHED | status::DETECTED) != 0 {
            return;
        }
        if self.can_auto_acquire_while_stealthed(id) {
            return;
        }
        // `StealthUpdate *stealth = theUnit->getStealth(); if (stealth)`.
        let Some(delay) = delay else { return };
        // "Skew it a little due to having a large group selected" (`AIGroup.cpp:1776`).
        let random = self.rng.value(0, crate::math::LOGICFRAMES_PER_SECOND as i32) as u32;
        self.set_next_mood_check_time(id, now.wrapping_add(delay).wrapping_add(random));
    }

    /// The early returns of `getNextMoodTarget` that run before its team-victim check and its timer
    /// (AIUpdate.cpp:4531-4572): a dead unit, `IS_USING_ABILITY`, an attacking unit with `NOTWHILEATTACKING`
    /// (any caller), and for the idle caller (`calledDuringIdle`) the `AAS_Idle` flag and the stealthed rule.
    pub(crate) fn mood_scan_blocked_before_timer(&self, id: ObjectId, called_during_idle: bool) -> bool {
        let Some(o) = self.objects.get(&id) else { return true };
        // `obj->isEffectivelyDead()` (AIUpdate.cpp:4540).
        if o.effectively_dead || o.is_dead() {
            return true;
        }
        let bits = o.status_mask(self.frame);
        // `obj->testStatus(OBJECT_STATUS_IS_USING_ABILITY)` (AIUpdate.cpp:4544).
        if bits & crate::object::status::USING_ABILITY != 0 {
            return true;
        }
        // `isAttacking() && BitIsSet(m_autoAcquireEnemiesWhenIdle, AAS_Idle_Not_While_Attacking)` (AIUpdate.cpp:4555).
        if o.template.combat.auto_acquire_not_while_attacking && self.is_ai_attacking(id) {
            return true;
        }
        if !called_during_idle {
            return false;
        }
        // `calledDuringIdle` and `AAS_Idle` not set (AIUpdate.cpp:4551).
        if !o.template.combat.auto_acquire_enemies {
            return true;
        }
        // A stealthed unit (`OBJECT_STATUS_STEALTHED`) acquires only when `canAutoAcquireWhileStealthed`, or from a
        // container that lets its passengers fire (AIUpdate.cpp:4561-4572).
        if bits & crate::object::status::STEALTHED != 0 && !self.can_auto_acquire_while_stealthed(id) {
            let firing_container = o.contained_by.is_some_and(|c| self.is_passenger_allowed_to_fire(c));
            if !firing_container {
                return true;
            }
        }
        false
    }

    /// `AIUpdateInterface::canAutoAcquireWhileStealthed` (AIUpdate.cpp:4520): a special-power stealth, or the
    /// template's `AAS_Idle_Stealthed` (`STEALTHED` in `AutoAcquireEnemiesWhenIdle`).
    pub(crate) fn can_auto_acquire_while_stealthed(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| {
            o.template.stealth.as_ref().is_some_and(|s| s.granted_by_special_power) || o.template.combat.auto_acquire_while_stealthed
        })
    }

    /// `AIUpdateInterface::isAttacking` (AIUpdate.cpp:3131): `getStateMachine()->isInAttackState()`, i.e. the
    /// current main state's `State::isAttack` (`StateMachine.h:257`). `AIAttackState` is an attack state
    /// (`AIStateMachine.h:458,507`), and the attack-move, attack-follow, attack-area, hunt, guard, tunnel-guard
    /// and dock states delegate to their attack sub-machine (`:577,687,1030,1074,1285`; `AIStates.cpp:6713,6872,
    /// 7009,7135`). The sim runs an attack as its own main state whose `attack_return` is the enclosing state, so
    /// an enclosing state reports attacking while that attack runs. A guard's attack runs inside the guard state
    /// (`AIGuardInnerState`/`OuterState`/`AggressorState`'s `m_attackState`, `AIGuard.h:144,194,255`), tracked by
    /// `guard_attack_active`. When the sub-machine is idle none of them is attacking: the attack-follow path
    /// state's mood scan (`AIStates.cpp:4463-4500`) runs *because* `m_attackFollowMachine->isInIdleState()` is
    /// true, so an idle attack sub-machine must report false (`AIStateMachine.h:687`).
    ///
    /// `AI_GUARD_RETALIATE` is the exception: `AIGuardRetaliateState::isAttack` forwards to its machine
    /// (`AIStates.cpp:6867-6874`), but **none** of `AIGuardRetaliateInnerState`, `OuterState` or
    /// `AttackAggressorState` overrides the base `State::isAttack` (which is `false`, `StateMachine.h:151`;
    /// no `isAttack` in `AIGuardRetaliate.{h,cpp}`, and the machine overrides only `isIdle`,
    /// `AIGuardRetaliate.h:110`), unlike `AIGuard.h:144/194/255`. So a retaliating unit is never
    /// `isAttacking()`, whatever sub-state it is in; the engine's forwarding call is dead code and the port
    /// must not report the sub-state's attack (`AIPathfind.cpp:10267-10269` would keep the unit in place,
    /// and `AAS_Idle_Not_While_Attacking` would stop auto-acquire, where the engine does neither).
    pub fn is_ai_attacking(&self, id: ObjectId) -> bool {
        use crate::ai_states::AiStateType as S;
        self.objects.get(&id).is_some_and(|o| match o.ai.machine.current {
            // `AIAttackState::isAttack` returns true unconditionally (`AIStateMachine.h:986`).
            S::AttackObject | S::ForceAttackObject | S::AttackAndFollowObject | S::AttackPosition => true,
            // `AIAttackMoveToState::isAttack`, `AIAttackFollowWaypointPathState::isAttack`,
            // `AIAttackAreaState::isAttack` and `AIAttackSquadState::isAttack` all return
            // `m_...Machine ? m_...Machine->isInAttackState() : FALSE` (`AIStateMachine.h:577,687,1030,1285`):
            // the port's attack sub-machine state is the pending attack order.
            S::AttackMoveTo | S::AttackFollowWaypointPath | S::AttackArea | S::AttackSquad => o.combat.attack.is_some(),
            // `AIGuardState::isAttack` -> `m_guardMachine->isInAttackState()` -> the guard state's `m_attackState`.
            S::Guard => o.ai.machine.guard_attack_active,
            // `AIGuardRetaliateState::isAttack` forwards to `m_guardRetaliateMachine->isInAttackState()`
            // (`AIStates.cpp:6867-6874`), but no state of that machine overrides `State::isAttack`, so the
            // answer is always false (see the doc comment above). `retaliate.attack_active` is the
            // sub-machine's own attack latch, which the engine never consults through `isAttacking()`.
            S::GuardRetaliate => false,
            _ => false,
        })
    }

    /// `AIUpdateInterface::isIdle` (AIUpdate.cpp:3120-3128): `getCurrentStateID() == AI_IDLE` or
    /// `getStateMachine()->isInIdleState()`. The second test is `m_currentState->isIdle()` (`StateMachine.h:256`),
    /// which only `AIIdleState` overrides to true (`AIStateMachine.h:228`) — `AI_IDLE` is its only state id — so
    /// it is the same test as the first for the main machine. A sub-machine's idle state does not count:
    /// `isInIdleState` is asked of the main machine only (`getStateMachine()`), so a guarding unit in
    /// `AI_GUARD_IDLE`, or an attack-move whose attack sub-machine is idle, is not idle.
    /// The subclasses veto on top of the base answer: `JetAIUpdate::isIdle` (`JetAIUpdate.cpp:1898`) and
    /// `HackInternetAIUpdate::isIdle` (`HackInternetAIUpdate.cpp:68`) return false while a command is held.
    /// `ChinookAIUpdate::isIdle` (`ChinookAIUpdate.cpp:989`, also false while passengers want in or out) has no
    /// port path: the sim's Chinook has no pending-command flag.
    pub(crate) fn is_ai_idle(&self, id: ObjectId) -> bool {
        self.objects.get(&id).is_some_and(|o| o.ai.machine.current.is_idle_state() && !o.ai_idle_vetoed())
    }
}

impl crate::object::Object {
    /// The subclass overrides of the virtual `AIUpdateInterface::isIdle` that return false while a command is held. The
    /// base test (the main machine in `AI_IDLE`) is applied by the caller. The port has these two:
    /// `JetAIUpdate::isIdle` (`JetAIUpdate.cpp:1898-1906`, `HAS_PENDING_COMMAND`) and `HackInternetAIUpdate::isIdle`
    /// (`HackInternetAIUpdate.cpp:68-76`, `m_hasPendingCommand`). `ChinookAIUpdate::isIdle` (`ChinookAIUpdate.cpp:989-1006`)
    /// has no port path: its `m_hasPendingCommand` and the `CHINOOK_LANDED` status with passengers wanting out are the
    /// takeoff/landing machinery, which the sim's Chinook does not model (chinook.rs covers only the combat drop).
    pub fn ai_idle_vetoed(&self) -> bool {
        // `getFlag(HAS_PENDING_COMMAND)` (`JetAIUpdate.cpp:1902`).
        if self.template.jet.is_some() && self.jet.flags & crate::jet_ai::jet_flag::HAS_PENDING_COMMAND != 0 {
            return true;
        }
        // `if (m_hasPendingCommand) return false;` (`HackInternetAIUpdate.cpp:71`).
        self.hack_internet.as_ref().is_some_and(|h| h.pending.is_some())
    }

    /// `AIUpdateInterface::isMoving` (AIUpdate.cpp:3194-3206) as the subclass `isIdle` overrides see it: `isIdle()` is
    /// virtual, so a jet or hacker holding a command reads as not idle and falls through to the goal and `m_isMoving`
    /// tests. Every `isMoving()` reader in the sim goes through this.
    pub fn is_ai_moving(&self) -> bool {
        self.ai.is_moving_with_idle_veto(self.ai_idle_vetoed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behaviors::StealthTemplate;
    use crate::combat::{AttackOrder, AttackTarget};
    use crate::command::Command;
    use crate::ai_states::CommandSource;
    use crate::orders::GuardMode;
    use crate::template::{kind_of, LocomotorKind, LocomotorParams, ObjectTemplate};
    use crate::terrain::TerrainLogic;
    use crate::weapon::{WeaponSetTemplate, WeaponTemplate};
    use crate::math::Coord3D;
    use std::sync::Arc;

    /// An armed unit that auto-acquires (its `AutoAcquireEnemiesWhenIdle`), with a 100-unit weapon.
    fn armed(name: &str) -> ObjectTemplate {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
        t.geometry.major_radius = 5.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.combat.auto_acquire_enemies = true;
        let mut w = WeaponTemplate::new("Hit");
        w.primary_damage = 1.0;
        w.attack_range = 100.0;
        let mut set = WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(w));
        t.combat.weapon_set = Some(set);
        t
    }

    /// A unit for player 0 at (100,100) and an enemy for player 1 at (150,100), in range. Both are `suspended` so the
    /// AI state machine (which runs for every armed object) does not scan on its own; the map is revealed to player 0
    /// (a non-AI unit only acquires what it sees, `object_shroud_status`).
    fn world() -> (GameLogic, ObjectId, ObjectId) {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let unit = g.create_object(&Arc::new(armed("Unit")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let enemy = g.create_object(&Arc::new(armed("Enemy")), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        g.reveal_map_permanently_for_player(0);
        for id in [unit, enemy] {
            g.objects.get_mut(&id).unwrap().ai.suspended = true;
        }
        (g, unit, enemy)
    }

    fn offset_pending(g: &GameLogic, id: ObjectId) -> bool {
        g.find_object(id).unwrap().combat.randomly_offset_mood_check
    }

    /// `resetNextMoodCheckTime` schedules the first check `ForceIdleMSEC` (1) frame on and marks it offset. Before
    /// it is due, the scan finds nothing and leaves the timer alone. When due, the scan finds the enemy, schedules the
    /// next check `MoodAttackCheckRate` (60) frames on, and applies the one random offset (AIUpdate.cpp:4603-4614).
    /// Revert check: drop the offset draw (or clear the flag without it) and the `next` assert fails.
    #[test]
    fn the_first_due_check_after_a_reset_is_offset_once() {
        let (mut g, unit, enemy) = world();
        g.reset_next_mood_check_time(unit);
        assert_eq!(g.next_mood_check_time(unit), 1);
        assert!(offset_pending(&g, unit));
        assert_eq!(g.ai_idle_scan(unit, false, None, GuardMode::Normal, false), None, "not due yet");
        assert_eq!(g.next_mood_check_time(unit), 1, "a scan before the check time does not move it");
        assert!(offset_pending(&g, unit), "and does not use the offset");
        g.run_frames(1);
        assert_eq!(g.ai_idle_scan(unit, false, None, GuardMode::Normal, false), Some(enemy));
        // The offset is the first draw of this world's RNG (seed 1): nothing else draws before the scan.
        let mut reference = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let offset = reference.rng_mut().value(-30, 30);
        assert_eq!(g.next_mood_check_time(unit) as i32, 61 + offset, "next check = now + 60 + GameLogicRandomValue(-30, 30)");
        assert!(!offset_pending(&g, unit), "the offset is applied once");
    }

    /// `getNextMoodTarget` is only reached for an idle unit that is not sleeping: `AIIdleState::update` checks the
    /// mood-matrix sleep first (AIStates.cpp:1440), so a sleeping unit never draws the offset.
    #[test]
    fn a_sleeping_unit_keeps_its_pending_offset() {
        let (mut g, unit, _) = world();
        g.reset_next_mood_check_time(unit);
        g.objects.get_mut(&unit).unwrap().ai.attitude = -2;
        g.run_frames(1);
        assert_eq!(g.ai_idle_scan(unit, false, None, GuardMode::Normal, false), None);
        assert!(offset_pending(&g, unit), "no timer work for a sleeping unit");
        assert_eq!(g.next_mood_check_time(unit), 1);
    }

    /// `wakeUpAndAttemptToTarget` checks at once, but only for an idle unit (AIUpdate.cpp:4493).
    #[test]
    fn wake_up_only_moves_an_idle_unit() {
        let (mut g, unit, _) = world();
        let other = g.create_object(&Arc::new(armed("Other")), Coord3D::new(100.0, 200.0, 0.0), 0.0, 0);
        for id in [unit, other] {
            g.objects.get_mut(&id).unwrap().ai.suspended = true;
        }
        g.objects.get_mut(&unit).unwrap().ai.machine.current = AiStateType::Idle;
        g.objects.get_mut(&other).unwrap().ai.machine.current = AiStateType::Exit;
        g.run_frames(5);
        g.wake_up_and_attempt_to_target(unit);
        g.wake_up_and_attempt_to_target(other);
        assert_eq!(g.next_mood_check_time(unit), g.frame(), "an idle unit is due now");
        assert!(offset_pending(&g, unit));
        assert_eq!(g.next_mood_check_time(other), 0, "a busy unit is untouched");
        assert!(!offset_pending(&g, other));
    }

    /// Hunt scans (`AI::findClosestEnemy`, AIStates.cpp:7230) do not use the mood timer: a hunt at frame 0 finds the
    /// enemy even though the idle check is not due.
    #[test]
    fn a_hunt_scan_ignores_the_mood_timer() {
        let (mut g, unit, enemy) = world();
        g.reset_next_mood_check_time(unit);
        assert_eq!(g.ai_idle_scan(unit, true, None, GuardMode::Normal, false), Some(enemy));
        assert!(offset_pending(&g, unit), "the hunt did not consume the idle offset");
    }

    /// `getNextMoodTarget`'s early returns come before its timer (AIUpdate.cpp:4531-4544): a dead unit and a unit in
    /// an ability (`IS_USING_ABILITY`) scan nothing and draw nothing. Revert check: move the early return below the
    /// timer and the pending offset is consumed.
    #[test]
    fn dead_or_busy_units_do_not_touch_the_timer() {
        let (mut g, unit, _) = world();
        g.reset_next_mood_check_time(unit);
        g.run_frames(1);
        g.objects.get_mut(&unit).unwrap().effectively_dead = true;
        assert_eq!(g.ai_idle_scan(unit, false, None, GuardMode::Normal, false), None);
        assert!(offset_pending(&g, unit), "a dead unit draws no offset");
        g.objects.get_mut(&unit).unwrap().effectively_dead = false;
        g.objects.get_mut(&unit).unwrap().status_bits |= crate::object::status::USING_ABILITY;
        assert_eq!(g.ai_idle_scan(unit, false, None, GuardMode::Normal, false), None);
        assert!(offset_pending(&g, unit), "a unit in an ability draws no offset");
    }

    /// A passenger whose container does not let it fire still runs the timer (AIUpdate.cpp:4604) and so draws its
    /// offset; the scan then finds nothing (`findClosestEnemy`'s `CAN_ATTACK` filter). Revert check: returning before
    /// the timer leaves the offset pending.
    #[test]
    fn a_passenger_that_cannot_fire_runs_the_timer_and_finds_nothing() {
        let (mut g, unit, _) = world();
        let container = g.create_object(&Arc::new(armed("Box")), Coord3D::new(100.0, 50.0, 0.0), 0.0, 0);
        g.objects.get_mut(&container).unwrap().ai.suspended = true;
        g.objects.get_mut(&unit).unwrap().contained_by = Some(container);
        g.reset_next_mood_check_time(unit);
        g.run_frames(1);
        assert_eq!(g.ai_idle_scan(unit, false, None, GuardMode::Normal, false), None);
        assert!(!offset_pending(&g, unit), "the timer ran and applied its offset");
    }

    /// `getMoodMatrixValue`'s passive AI unit answers its last attacker (AIUpdate.cpp:4626-4639), here a stealthed
    /// enemy that the normal scan skips. Revert check: remove the passive block and the scan finds nothing.
    #[test]
    fn a_passive_ai_unit_answers_its_last_attacker() {
        use crate::body::DamageInfo;
        use crate::damage::DamageType;
        let (mut g, unit, enemy) = world();
        g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
        g.objects.get_mut(&enemy).unwrap().behavior.stealthed = true;
        let o = g.objects.get_mut(&unit).unwrap();
        o.ai.attitude = -1;
        o.last_damage_info = DamageInfo { source: enemy, amount: 1.0, kind: DamageType::Explosion };
        g.reset_next_mood_check_time(unit);
        g.run_frames(1);
        assert_eq!(g.ai_idle_scan(unit, false, None, GuardMode::Normal, false), Some(enemy));
    }

    // ---- the AIGroup stealth rule (AIGroup.cpp:1759-1781, 2105-2127) -----------------------------------------

    /// A mobile, armed unit with `StealthDelay = delay` and innate stealth (so `CAN_STEALTH` is set at creation,
    /// StealthUpdate.cpp:147-151), which auto-acquires when idle.
    fn stealth_unit(name: &str, delay: u32, order_idles: bool) -> ObjectTemplate {
        let mut t = armed(name);
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
        t.stealth = Some(StealthTemplate {
            stealth_delay: delay,
            move_threshold_speed: 0.0,
            forbidden_conditions: 0,
            required_status: 0,
            forbidden_status: 0,
            innate_stealth: true,
            granted_by_special_power: false,
            disguises_as_team: false,
            reveal_distance_from_target: 0.0,
            order_idle_enemies_to_attack_on_reveal: order_idles,
            use_rider_stealth: false,
        });
        t
    }

    fn stealth_world(delay: u32, order_idles: bool) -> (GameLogic, ObjectId, Arc<ObjectTemplate>) {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let t = Arc::new(stealth_unit("Stealth", delay, order_idles));
        let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        (g, id, t)
    }

    /// The draw `command_move_to` would make, from a world built with the same creation draws.
    fn reference_stealth_world(t: &Arc<ObjectTemplate>) -> GameLogic {
        let mut r = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        r.create_object(t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        r
    }

    /// `AIGroup::groupMoveToPosition`'s stealth rule (`AIGroup.cpp:1759-1781`): a player move of a combat stealth
    /// unit that is not stealthed and not detected, and does not auto-acquire while stealthed, draws
    /// `GameLogicRandomValue(0, LOGICFRAMES_PER_SECOND)` and sets the next mood check to
    /// `now + StealthDelay + random` with no random offset. Revert check: drop the call (or its draw) and the
    /// `next` assert fails.
    #[test]
    fn a_player_move_of_a_stealth_unit_delays_its_mood_check() {
        let (mut g, id, t) = stealth_world(40, false);
        let mut reference = reference_stealth_world(&t);
        let random = reference.rng_mut().value(0, crate::math::LOGICFRAMES_PER_SECOND as i32) as u32;
        assert_ne!(g.find_object(id).unwrap().status_bits & status::CAN_STEALTH, 0, "innate stealth sets CAN_STEALTH");
        g.command_move_to_from(0, &[id], Coord3D::new(200.0, 100.0, 0.0), CommandSource::Player, true);
        assert_eq!(g.next_mood_check_time(id), 40 + random, "now (0) + StealthDelay + the draw");
        assert!(!offset_pending(&g, id), "setNextMoodCheckTime leaves the offset off");
    }

    /// The rule's guards: a stealthed unit, a detected one, one that may auto-acquire while stealthed, and a script
    /// order (`CMD_FROM_SCRIPT`) all leave the timer and the RNG alone (`AIGroup.cpp:1759-1780`).
    #[test]
    fn the_stealth_move_rule_skips_stealthed_detected_and_scripted_moves() {
        for case in 0..4 {
            let (mut g, id, t) = stealth_world(40, false);
            let mut reference = reference_stealth_world(&t);
            match case {
                0 => g.objects.get_mut(&id).unwrap().behavior.stealthed = true,
                1 => g.objects.get_mut(&id).unwrap().behavior.detected_until = 10,
                2 => g.objects.get_mut(&id).unwrap().template = {
                    let mut t2 = (*t).clone();
                    t2.combat.auto_acquire_while_stealthed = true;
                    Arc::new(t2)
                },
                _ => {}
            }
            let source = if case == 3 { CommandSource::Script } else { CommandSource::Player };
            g.command_move_to_from(0, &[id], Coord3D::new(200.0, 100.0, 0.0), source, true);
            assert_eq!(g.next_mood_check_time(id), 0, "case {case}: no delay");
            assert_eq!(g.rng_mut().value(0, 30), reference.rng_mut().value(0, 30), "case {case}: no draw");
        }
    }

    /// `AIGroup::groupIdle` (`AIGroup.cpp:2105-2127`): a player stop runs `aiIdle` first (its `AI_IDLE` sleep-offset
    /// draw) and then the same stealth rule. Revert check: drop the `group_stealth_mood_delay` call from the stop
    /// handler and the `next` assert fails.
    #[test]
    fn a_player_stop_of_a_stealth_unit_delays_its_mood_check_after_the_idle_draw() {
        let (mut g, id, t) = stealth_world(40, false);
        let mut reference = reference_stealth_world(&t);
        let idle_offset = reference.rng_mut().value(0, 60);
        let random = reference.rng_mut().value(0, crate::math::LOGICFRAMES_PER_SECOND as i32) as u32;
        g.submit(0, Command::Stop { ids: vec![id] });
        g.process_command_list();
        let o = g.find_object(id).unwrap();
        assert_eq!(o.ai.machine.idle_sleep_offset as i32, idle_offset, "AIIdleState::onEnter drew first");
        assert_eq!(g.next_mood_check_time(id), 40 + random);
        assert!(!offset_pending(&g, id));
    }

    /// `canAutoAcquire` is any `AutoAcquireEnemiesWhenIdle` bit (`AIUpdate.h:592`), not just `AAS_Idle`: a unit with
    /// only `AttackBuildings` still takes the delay, and a unit with no bit does not.
    #[test]
    fn can_auto_acquire_is_any_bit_and_without_a_stealth_module_there_is_no_delay() {
        let (mut g, id, t) = stealth_world(40, false);
        g.objects.get_mut(&id).unwrap().template = {
            let mut t2 = (*t).clone();
            t2.combat.auto_acquire_enemies = false;
            t2.combat.auto_acquire_buildings = true;
            Arc::new(t2)
        };
        let mut reference = reference_stealth_world(&t);
        let random = reference.rng_mut().value(0, crate::math::LOGICFRAMES_PER_SECOND as i32) as u32;
        g.command_move_to_from(0, &[id], Coord3D::new(200.0, 100.0, 0.0), CommandSource::Player, true);
        assert_eq!(g.next_mood_check_time(id), 40 + random);
        // No stealth module: `StealthUpdate *stealth = theUnit->getStealth(); if (stealth)` is false.
        let (mut g, id, t) = stealth_world(40, false);
        let mut t2 = (*t).clone();
        t2.stealth = None;
        let plain = Arc::new(t2);
        g.objects.get_mut(&id).unwrap().template = plain.clone();
        let mut reference = reference_stealth_world(&t);
        g.command_move_to_from(0, &[id], Coord3D::new(200.0, 100.0, 0.0), CommandSource::Player, true);
        assert_eq!(g.next_mood_check_time(id), 0);
        assert_eq!(g.rng_mut().value(0, 30), reference.rng_mut().value(0, 30), "no draw without a stealth module");
    }

    // ---- isAttacking / isIdle ---------------------------------------------------------------------------------

    /// `AIUpdateInterface::isAttacking` for a guard (`AIGuardState::isAttack` -> the guard machine's state's
    /// `m_attackState`, `AIGuard.h:144,194`): a guard with an active attack is attacking, so
    /// `AAS_Idle_Not_While_Attacking` blocks its mood scan (AIUpdate.cpp:4549). Revert check: drop the `Guard` arm
    /// and both asserts fail.
    #[test]
    fn a_guard_with_an_active_attack_is_attacking() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let mut t = armed("Guard");
        t.combat.auto_acquire_not_while_attacking = true;
        let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        g.objects.get_mut(&id).unwrap().ai.suspended = true;
        g.objects.get_mut(&id).unwrap().ai.machine.current = AiStateType::Guard;
        assert!(!g.is_ai_attacking(id), "the guard machine has no attack state yet");
        assert!(!g.mood_scan_blocked_before_timer(id, false));
        g.objects.get_mut(&id).unwrap().ai.machine.guard_attack_active = true;
        assert!(g.is_ai_attacking(id), "AIGuardInnerState/OuterState's m_attackState is an AIAttackState");
        assert!(g.mood_scan_blocked_before_timer(id, false), "NOTWHILEATTACKING then blocks the mood scan");
    }

    /// `AIAttackFollowWaypointPathState::isAttack` is `m_attackFollowMachine->isInAttackState()`
    /// (`AIStateMachine.h:687`), and the state's mood scan (`AIStates.cpp:4480-4495`) runs precisely when that
    /// sub-machine is idle, so the path state itself is not "attacking" while it scans. `AIAttackAreaState` and
    /// `AIAttackSquadState` have the same shape (`:1030,1285`), as does the attack-move state (`:577`). Revert
    /// check: restore the unconditional `true` arm for these four states and the first pair of asserts fails.
    #[test]
    fn an_attack_follow_area_or_squad_state_is_attacking_only_while_its_attack_runs() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let mut t = armed("Follower");
        t.combat.auto_acquire_not_while_attacking = true;
        let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        g.objects.get_mut(&id).unwrap().ai.suspended = true;
        for state in [
            AiStateType::AttackFollowWaypointPath,
            AiStateType::AttackArea,
            AiStateType::AttackSquad,
            AiStateType::AttackMoveTo,
        ] {
            let o = g.objects.get_mut(&id).unwrap();
            o.ai.machine.current = state;
            o.combat.attack = None;
            assert!(!g.is_ai_attacking(id), "{state:?}: the attack sub-machine is idle");
            assert!(!g.mood_scan_blocked_before_timer(id, false), "{state:?}: so the mood scan runs");
            g.objects.get_mut(&id).unwrap().combat.attack = Some(AttackOrder::new(AttackTarget::Ground(Coord3D::new(300.0, 100.0, 0.0))));
            assert!(g.is_ai_attacking(id), "{state:?}: the sub-machine is in an attack state");
            assert!(g.mood_scan_blocked_before_timer(id, false), "{state:?}: NOTWHILEATTACKING blocks the scan");
        }
    }

    /// `AIUpdateInterface::isIdle`'s subclass vetoes: `JetAIUpdate::isIdle` and `HackInternetAIUpdate::isIdle`
    /// return false while a command is held (`JetAIUpdate.cpp:1898-1906`, `HackInternetAIUpdate.cpp:68-75`), so
    /// `wakeUpAndAttemptToTarget` leaves the timer alone. Revert check: drop either veto and its assert fails.
    #[test]
    fn a_pending_command_keeps_a_jet_or_hacker_from_waking() {
        let (mut g, unit, _) = world();
        g.objects.get_mut(&unit).unwrap().ai.machine.current = AiStateType::Idle;
        assert!(g.is_ai_idle(unit), "an idle unit is idle");
        g.wake_up_and_attempt_to_target(unit);
        assert_eq!(g.next_mood_check_time(unit), 0);
        assert!(offset_pending(&g, unit));
        // Jet: `HAS_PENDING_COMMAND`.
        let mut jt = armed("Jet");
        jt.jet = Some(crate::modules2::JetAiTemplate {
            needs_runway: true,
            keeps_parking_space_when_airborne: false,
            takeoff_pause: 0,
            return_to_base_idle_time: 0,
            out_of_ammo_damage_per_second: 0.0,
            takeoff_dist_for_max_lift: 0.0,
            min_height: 0.0,
            parking_offset: 0.0,
            sneaky_offset_when_attacking: 0.0,
            attacking_loco: crate::template::LocomotorSet::Normal,
            returning_loco: crate::template::LocomotorSet::Normal,
            attack_loco_persist_time: 0,
            attackers_miss_persist_time: 0,
        });
        let jet = g.create_object(&Arc::new(jt), Coord3D::new(100.0, 200.0, 0.0), 0.0, 0);
        g.objects.get_mut(&jet).unwrap().ai.suspended = true;
        g.objects.get_mut(&jet).unwrap().ai.machine.current = AiStateType::Idle;
        g.objects.get_mut(&jet).unwrap().jet.flags |= crate::jet_ai::jet_flag::HAS_PENDING_COMMAND;
        assert!(!g.is_ai_idle(jet), "a jet with a pending command is not idle");
        g.wake_up_and_attempt_to_target(jet);
        assert_eq!(g.next_mood_check_time(jet), 0);
        assert!(!offset_pending(&g, jet), "the wake did not touch the jet");
        // Hacker: `m_pendingCommand`.
        let hack = g.create_object(&Arc::new(armed("Hacker")), Coord3D::new(100.0, 250.0, 0.0), 0.0, 0);
        g.objects.get_mut(&hack).unwrap().ai.suspended = true;
        g.objects.get_mut(&hack).unwrap().ai.machine.current = AiStateType::Idle;
        g.objects.get_mut(&hack).unwrap().hack_internet = Some(crate::hack_internet::HackState {
            phase: crate::hack_internet::HackPhase::Packing,
            frames_remaining: 10,
            pending: Some(crate::command::GameMessage {
                player: 0,
                command: Command::Stop { ids: Vec::new() },
                source: CommandSource::Player,
            }),
        });
        assert!(!g.is_ai_idle(hack), "a hacker holding an order is not idle");
        g.wake_up_and_attempt_to_target(hack);
        assert_eq!(g.next_mood_check_time(hack), 0);
        assert!(!offset_pending(&g, hack));
    }

    // ---- the stealth reveal wake (StealthUpdate.cpp:837-932) -------------------------------------------------

    /// `markAsDetected` -> `setWakeupIfInRange` (`StealthUpdate.cpp:912-931`): every enemy object whose own vision
    /// range reaches the revealed unit is woken; an idle one is due for a mood check now
    /// (`AIUpdate.cpp:4483-4492`). Revert check: remove the wake loop and the first assert fails.
    #[test]
    fn a_revealed_stealth_unit_wakes_idle_enemies_that_can_see_it() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let st = Arc::new(stealth_unit("Stinger", 20, true));
        let victim = g.create_object(&st, Coord3D::new(100.0, 100.0, 0.0), 0.0, 1);
        g.objects.get_mut(&victim).unwrap().ai.suspended = true;
        g.objects.get_mut(&victim).unwrap().behavior.stealthed = true;
        let mut ot = armed("Observer");
        ot.vision_range = 100.0;
        // `setWakeupIfInRange` needs `obj->getAI()` (`StealthUpdate.cpp:839-843`): the template's
        // `AIUpdateInterface` module, not just a weapon.
        ot.ai_update = Some(0);
        let ot = Arc::new(ot);
        let observer = g.create_object(&ot, Coord3D::new(150.0, 100.0, 0.0), 0.0, 0);
        let far = g.create_object(&ot, Coord3D::new(400.0, 100.0, 0.0), 0.0, 0);
        g.set_enemies(0, 1);
        for id in [observer, far] {
            let o = g.objects.get_mut(&id).unwrap();
            o.ai.suspended = true;
            o.ai.machine.current = AiStateType::Idle;
            o.combat.next_mood_check = 999;
            o.combat.randomly_offset_mood_check = false;
        }
        g.mark_detected(victim, 0);
        assert_eq!(g.find_object(victim).unwrap().behavior.detected_until, 20, "now + the owner's StealthDelay");
        assert_eq!(g.next_mood_check_time(observer), g.frame(), "an idle observer is due now");
        assert!(offset_pending(&g, observer));
        assert_eq!(g.next_mood_check_time(far), 999, "300 units away, out of its 100 vision");
        assert!(!offset_pending(&g, far));
    }

    /// The wake's `obj->getAI()` test (`StealthUpdate.cpp:839-843`): an enemy with a weapon but no
    /// `AIUpdateInterface` module is skipped — the engine asks for the AI module, not for a weapon.
    /// Revert check: the old `ai_handle || !weapons.is_empty()` test wakes the gun emplacement.
    #[test]
    fn a_weapon_without_an_ai_module_is_not_woken_by_a_reveal() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let st = Arc::new(stealth_unit("Stinger", 20, true));
        let victim = g.create_object(&st, Coord3D::new(100.0, 100.0, 0.0), 0.0, 1);
        g.objects.get_mut(&victim).unwrap().ai.suspended = true;
        let mut ot = armed("GunEmplacement");
        ot.vision_range = 100.0;
        ot.ai_update = None;
        let gun = g.create_object(&Arc::new(ot), Coord3D::new(150.0, 100.0, 0.0), 0.0, 0);
        g.set_enemies(0, 1);
        {
            let o = g.objects.get_mut(&gun).unwrap();
            o.ai.suspended = true;
            o.ai.machine.current = AiStateType::Idle;
            o.combat.next_mood_check = 999;
        }
        assert!(!g.find_object(gun).unwrap().combat.weapons.is_empty(), "it is armed");
        g.mark_detected(victim, 0);
        assert_eq!(g.next_mood_check_time(gun), 999, "no AI module, so no wake");
        assert!(!offset_pending(&g, gun));
    }

    /// The wake's guards: `OrderIdleEnemiesToAttackMeUponReveal` off, or an observer that is not idle, changes
    /// nothing. A positive `numFrames` keeps the later expiry (`StealthUpdate.cpp:901-910`).
    #[test]
    fn the_reveal_wake_respects_the_order_flag_and_the_observer_state() {
        // Order flag off.
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let st = Arc::new(stealth_unit("Stinger", 20, false));
        let victim = g.create_object(&st, Coord3D::new(100.0, 100.0, 0.0), 0.0, 1);
        g.objects.get_mut(&victim).unwrap().ai.suspended = true;
        let mut ot = armed("Observer");
        ot.vision_range = 100.0;
        // `setWakeupIfInRange` needs `obj->getAI()` (`StealthUpdate.cpp:839-843`): the template's
        // `AIUpdateInterface` module, not just a weapon.
        ot.ai_update = Some(0);
        let observer = g.create_object(&Arc::new(ot), Coord3D::new(150.0, 100.0, 0.0), 0.0, 0);
        g.set_enemies(0, 1);
        let o = g.objects.get_mut(&observer).unwrap();
        o.ai.suspended = true;
        o.ai.machine.current = AiStateType::Idle;
        o.combat.next_mood_check = 999;
        g.mark_detected(victim, 0);
        assert_eq!(g.next_mood_check_time(observer), 999, "no order to attack on reveal");
        // The offset is armed by the object's construction (`AIIdleState::onEnter` -> `resetNextMoodCheckTime`,
        // AIStates.cpp:1321, AIUpdate.cpp:4502), and the reveal wake leaves it alone.
        assert!(offset_pending(&g, observer), "the creation offset is untouched");

        // A busy observer is not woken.
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let st = Arc::new(stealth_unit("Stinger", 20, true));
        let victim = g.create_object(&st, Coord3D::new(100.0, 100.0, 0.0), 0.0, 1);
        g.objects.get_mut(&victim).unwrap().ai.suspended = true;
        let mut ot = armed("Observer");
        ot.vision_range = 100.0;
        // `setWakeupIfInRange` needs `obj->getAI()` (`StealthUpdate.cpp:839-843`): the template's
        // `AIUpdateInterface` module, not just a weapon.
        ot.ai_update = Some(0);
        let observer = g.create_object(&Arc::new(ot), Coord3D::new(150.0, 100.0, 0.0), 0.0, 0);
        g.set_enemies(0, 1);
        let o = g.objects.get_mut(&observer).unwrap();
        o.ai.suspended = true;
        o.ai.machine.current = AiStateType::Exit;
        o.combat.next_mood_check = 999;
        g.mark_detected(victim, 0);
        assert_eq!(g.next_mood_check_time(observer), 999, "not idle");
        assert!(offset_pending(&g, observer), "the creation offset is untouched");

        // A positive frame count keeps the longer detection.
        let o = g.objects.get_mut(&victim).unwrap();
        o.behavior.detected_until = 500;
        g.mark_detected(victim, 30);
        assert_eq!(g.find_object(victim).unwrap().behavior.detected_until, 500);
        g.mark_detected(victim, 900);
        assert_eq!(g.find_object(victim).unwrap().behavior.detected_until, 900);
    }
}
