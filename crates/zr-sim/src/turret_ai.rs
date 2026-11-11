//! The `TurretAI` state machine (`GameLogic/AI/TurretAI.cpp`, `GameLogic/TurretAI.h`): the
//! `TurretStateMachine` (`IDLE -> IDLESCAN -> HOLD -> RECENTER -> IDLE`, plus `AIM`/`FIRE`) that
//! owns a turret's yaw and pitch and, for a weapon on the turret, its firing.
//!
//! In the engine the machine is a `StateMachine` inside the `TurretAI` object the `AIUpdateInterface`
//! constructor creates per `Turret` block (`AIUpdate.cpp:288-294`). `AIUpdateInterface::update`
//! runs the object's AI state machine and then, for every turret, `TurretAI::updateTurretAI`
//! (`AIUpdate.cpp:1090-1098`), which drives the machine and returns the turret's sleep. The
//! `AIAttack*` states do not turn a turret that can turn: `AIAttackAimAtTargetState::update` orders
//! it (`setTurretTargetObject`/`setTurretTargetPosition`) and returns `STATE_CONTINUE`
//! (`AIStates.cpp:5042-5065`), and the turret's own `AIM`/`FIRE` states aim and fire.
//!
//! **The port's world-space angle.** `TurretAI::m_angle` is *relative to the body*
//! (`friend_turnTowardsAngle` is fed `PartitionManager::getRelativeAngle2D`), and the model draw
//! applies it as a rotation on top of the body's yaw. The sim stores the turret angle in world
//! space (`ObjectCombat::turret_angle`) for the launch transform and the viewer, so every update
//! carries the angle by the body's yaw delta first (`last_angle`), which is what the relative
//! representation does implicitly.
//!
//! **Documented reductions.** One turret only: `AltTurret` (4 battleship templates) is parsed but
//! not simulated. `m_victimInitialTeam` is the victim's player (the port has no `Object::getTeam`).
//! `addTargeter`/`isTemporarilyPreventingAimSuccess` and `notifyNewVictimChosen`'s
//! `ContinueAttackRange` retarget are not modelled. The rotation/pitch sound bookkeeping is kept
//! for its state (`m_continuousFireExpirationFrame` gates only the sound, `m_didFire` arms the
//! sweep) but no audio is played.

use crate::combat::{AttackTarget, AIM_REL_THRESH};
use crate::logic::GameLogic;
use crate::math::{acos, angle_diff, asin, cos, normalize_angle, sin, sqrt, Coord3D};
use crate::object::{ObjectId, PlayerIndex};
use crate::template::ObjectTemplate;
use crate::weapon::CombatTemplate;

/// `UPDATE_SLEEP_FOREVER` (`UpdateModule.h:72`).
pub(crate) const TURRET_SLEEP_FOREVER: u32 = 0x3fffffff;
/// `UPDATE_SLEEP_NONE` (`UpdateModule.h:70`).
const TURRET_SLEEP_NONE: u32 = 1;

/// `TurretStateType` (`TurretAI.h:42`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurretStateType {
    Idle,
    IdleScan,
    Aim,
    Fire,
    Recenter,
    Hold,
}

/// `TurretTargetType` (`TurretAI.h:248`) plus the goal itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TurretTarget {
    None,
    Object(ObjectId),
    Position(Coord3D),
}

/// The state of one `TurretAI` (`TurretAI.h:339-371`) plus its state machine's sleep
/// (`StateMachine::m_sleepTill`).
#[derive(Clone, Debug, PartialEq)]
pub struct TurretMachine {
    pub state: TurretStateType,
    /// `TurretAI::m_enabled` (`!InitiallyDisabled`).
    pub enabled: bool,
    /// `TurretAI::m_target` + the machine's goal object/position.
    pub target: TurretTarget,
    /// `TurretAI::m_isForceAttacking`.
    pub force_attacking: bool,
    /// `TurretAI::m_targetWasSetByIdleMood`.
    pub target_was_set_by_idle_mood: bool,
    /// `TurretAI::m_victimInitialTeam`, as the victim's player (the port has no `Object::getTeam`).
    pub victim_initial_player: Option<PlayerIndex>,
    /// `TurretAI::m_sleepUntil` (the module's own sleep, `TurretAI.cpp:750`).
    pub sleep_until: u32,
    /// `StateMachine::m_sleepTill`.
    pub state_sleep_until: u32,
    /// `TurretAIIdleState::m_nextIdleScan`.
    pub next_idle_scan: u32,
    /// `TurretAIIdleScanState::m_desiredAngle` (relative to the body).
    pub desired_angle: f32,
    /// `TurretAIHoldTurretState::m_timestamp`.
    pub hold_timestamp: u32,
    /// `TurretAI::m_didFire`.
    pub did_fire: bool,
    /// `TurretAI::m_continuousFireExpirationFrame`.
    pub continuous_fire_expiration: u32,
    /// `TurretAI::m_enableSweepUntil`.
    pub enable_sweep_until: u32,
    /// `TurretAI::m_positiveSweep`.
    pub positive_sweep: bool,
    /// `TurretAI::m_firesWhileTurning`.
    pub fires_while_turning: bool,
    /// The body angle `turret_angle` was last carried with (world-space representation).
    pub last_angle: f32,
    /// `MODELCONDITION_TURRET_ROTATE`: set while `friend_turnTowardsAngle` takes a step.
    pub rotating: bool,
}

impl TurretMachine {
    /// `TurretAI::TurretAI` (`TurretAI.cpp:280-324`) and its `initDefaultState` (`IDLE::onEnter`).
    /// `next_idle_scan` is the creation-time `resetIdleScan` draw (`TurretAI.cpp:1286-1291`), which
    /// happens during module construction, before the AI state machines enter `AI_IDLE`.
    pub fn new(c: &CombatTemplate, angle: f32, next_idle_scan: u32) -> Self {
        Self {
            state: TurretStateType::Idle,
            enabled: !c.turret_initially_disabled,
            target: TurretTarget::None,
            force_attacking: false,
            target_was_set_by_idle_mood: false,
            victim_initial_player: None,
            sleep_until: 0,
            state_sleep_until: 0,
            next_idle_scan,
            desired_angle: 0.0,
            hold_timestamp: 0,
            did_fire: false,
            continuous_fire_expiration: u32::MAX,
            enable_sweep_until: 0,
            positive_sweep: true,
            fires_while_turning: c.turret_fires_while_turning,
            last_angle: angle,
            rotating: false,
        }
    }

    /// `isWeaponSlotOnTurret` (`TurretAI.cpp:517`): an empty mask (a synthetic template; the engine
    /// throws) stands for every slot.
    pub fn slot_on_turret(&self, c: &CombatTemplate, slot: usize) -> bool {
        c.turret_controlled_slots == 0 || c.turret_controlled_slots & (1u8 << slot) != 0
    }

    /// `friend_isSweepEnabled` (`TurretAI.cpp:862`).
    fn sweep_enabled(&self, now: u32) -> bool {
        self.enable_sweep_until != 0 && self.enable_sweep_until > now
    }
}

/// The `StateReturnType` the turret states return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StateRet {
    Continue,
    Success,
    Failure,
    Sleep(u32),
}

/// `frameToSleepTime` (`TurretAI.cpp:53-73`): sleep until the earliest frame that is still in the
/// future, else continue.
fn frame_to_sleep(now: u32, f1: u32, f2: u32) -> StateRet {
    let f = f1.min(f2);
    if f > now {
        StateRet::Sleep(f - now)
    } else {
        StateRet::Continue
    }
}

/// `PartitionManager::getRelativeAngle2D` (`PartitionManager.cpp:3634-3680`): the signed angle from
/// the object's heading vector to the point, via `ACos` of the dot with the sign from the cross.
pub(crate) fn relative_angle_2d(from: Coord3D, from_angle: f32, to: Coord3D) -> f32 {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let dist = sqrt(dx * dx + dy * dy);
    if dist == 0.0 {
        return 0.0;
    }
    let (ux, uy) = (dx / dist, dy / dist);
    let (dirx, diry) = (cos(from_angle), sin(from_angle));
    let c = (dirx * ux + diry * uy).clamp(-1.0, 1.0);
    let mut value = acos(c);
    if dirx * uy - diry * ux < 0.0 {
        value = -value;
    }
    value
}

impl GameLogic {
    /// The creation-time `TurretAIIdleState::onEnter` `resetIdleScan` draw (`TurretAI.cpp:1286-1291`):
    /// `GameLogicRandomValue(MinIdleScanInterval, MaxIdleScanInterval)`. It runs in the `TurretAI`
    /// constructor (`TurretAI.cpp:321`, before `chooseLocomotorSet` at `AIUpdate.cpp:296`), so the
    /// caller draws it before the AI creation draws. Returns the turret's `m_nextIdleScan`.
    pub(crate) fn turret_creation_idle_scan(&mut self, c: &CombatTemplate) -> u32 {
        let delay = self.rng.value(c.turret_min_idle_scan_interval as i32, c.turret_max_idle_scan_interval as i32);
        self.frame.wrapping_add(delay as u32)
    }

    /// The turret's state machine, driven from [`GameLogic::update_turrets`].
    fn turret_machine(&self, id: ObjectId) -> Option<&TurretMachine> {
        self.objects.get(&id).and_then(|o| o.combat.turret.as_ref())
    }

    /// `TurretAI::updateTurretAI` (`TurretAI.cpp:689-759`): the module sleep, the machine, the
    /// `m_didFire` sweep arming.
    pub(crate) fn update_turrets(&mut self, id: ObjectId) {
        let now = self.frame;
        let (has_turret, suspended, body_angle) = {
            let Some(o) = self.objects.get(&id) else { return };
            (o.combat.turret.is_some(), o.ai.suspended, o.angle)
        };
        if !has_turret {
            return;
        }
        // Possession drives the turret directly (it sets `turret_angle`); just keep the carry
        // reference in sync so a release does not rotate the turret by the possession's own turns.
        if suspended {
            if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                m.last_angle = body_angle;
            }
            return;
        }
        // The world-space angle follows the body (`TurretAI::m_angle` is relative).
        let carried = self.objects.get(&id).map_or(0.0, |o| o.combat.turret_angle);
        let delta = {
            let m = match self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                Some(m) => m,
                None => return,
            };
            let delta = angle_diff(body_angle, m.last_angle);
            m.last_angle = body_angle;
            delta
        };
        if delta != 0.0 {
            if let Some(o) = self.objects.get_mut(&id) {
                o.combat.turret_angle = normalize_angle(carried + delta);
            }
        }
        // `AIUpdate.cpp:1082-1099`: the turrets do not run for a dead or disabled object.
        if !self.turret_runnable(id) {
            return;
        }
        let (module_sleep, enabled_or_recenter) = {
            let m = self.turret_machine(id).expect("checked");
            if m.sleep_until != 0 && now < m.sleep_until {
                return;
            }
            (m.sleep_until, m.enabled || m.state == TurretStateType::Recenter)
        };
        let _ = module_sleep;
        let sub_sleep = if enabled_or_recenter {
            if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                m.did_fire = false;
            }
            let st = self.turret_update_machine(id);
            let frames = match st {
                StateRet::Sleep(n) => n,
                _ => TURRET_SLEEP_NONE,
            };
            if self.turret_machine(id).is_some_and(|m| m.did_fire) {
                if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                    m.enable_sweep_until = now + 3;
                    m.continuous_fire_expiration = now + 3;
                }
            }
            frames
        } else {
            TURRET_SLEEP_FOREVER
        };
        if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
            m.sleep_until = now.wrapping_add(sub_sleep);
        }
    }

    /// The `AIUpdate.cpp:1082-1087` gate: not effectively dead and not disabled by a type that
    /// `AIUpdate.cpp:1085-1090` (identical in the fork and `src-patched`): `updateTurretAI` runs only
    /// when the object is not `isEffectivelyDead()` and is not disabled by `PARALYZED`, `UNMANNED`,
    /// `EMP`, `SUBDUED` or `HACKED`. `isEffectivelyDead` is the port's [`Object::is_dying`]: the
    /// `effectively_dead` bit (`InactiveBody`, the die pipeline) or a body at 0 health
    /// (`ActiveBody::internalChangeHealth` reflags it, `ActiveBody.cpp:1255`). `isDestroyed()` is a
    /// different bit the engine does not test here.
    pub(crate) fn turret_runnable(&self, id: ObjectId) -> bool {
        use crate::disabled::DisabledType;
        let Some(o) = self.objects.get(&id) else { return false };
        if o.is_dying() {
            return false;
        }
        [
            DisabledType::Paralyzed,
            DisabledType::Unmanned,
            DisabledType::Emp,
            DisabledType::Subdued,
            DisabledType::Hacked,
        ]
        .iter()
        .all(|t| !o.is_disabled_by_type(*t))
    }

    /// `StateMachine::updateStateMachine` (`StateMachine.cpp:412-474`) reduced to the turret's
    /// states: the machine's own sleep, one state update, then the success/failure/condition
    /// transitions.
    fn turret_update_machine(&mut self, id: ObjectId) -> StateRet {
        let now = self.frame;
        let cur = match self.turret_machine(id) {
            Some(m) => m.state,
            None => return StateRet::Sleep(TURRET_SLEEP_FOREVER),
        };
        let sleeping = self.turret_machine(id).is_some_and(|m| m.state_sleep_until != 0 && now < m.state_sleep_until);
        if sleeping {
            let until = self.turret_machine(id).expect("checked").state_sleep_until;
            return StateRet::Sleep(until - now);
        }
        if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
            m.state_sleep_until = 0;
        }
        let ret = self.turret_state_update(id, cur);
        // "if the state changed, ignore any sleep result and pretend CONTINUE" (`StateMachine.cpp:452`).
        if self.turret_machine(id).is_some_and(|m| m.state != cur) {
            return StateRet::Continue;
        }
        match ret {
            StateRet::Continue => self.turret_check_conditions(id, cur),
            StateRet::Success | StateRet::Failure => self.turret_transition(id, cur, ret),
            StateRet::Sleep(n) => {
                // `StateMachine::updateStateMachine` (`StateMachine.cpp:457-462`): the update's sleep
                // becomes the machine's sleep.
                if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                    m.state_sleep_until = now.wrapping_add(n);
                }
                StateRet::Sleep(n)
            }
        }
    }

    /// `State::friend_checkForTransitions` (`StateMachine.cpp:110-176`) for the turret states:
    /// `IDLE` success->`IDLE` failure->`IDLESCAN`; `IDLESCAN` both->`HOLD`; `AIM` success->`FIRE`
    /// failure->`HOLD`; `FIRE` both->`AIM`; `RECENTER` both->`IDLE`; `HOLD` both->`RECENTER`.
    fn turret_transition(&mut self, id: ObjectId, cur: TurretStateType, status: StateRet) -> StateRet {
        let next = match (cur, status) {
            (TurretStateType::Idle, StateRet::Success) => TurretStateType::Idle,
            (TurretStateType::Idle, StateRet::Failure) => TurretStateType::IdleScan,
            (TurretStateType::IdleScan, _) => TurretStateType::Hold,
            (TurretStateType::Aim, StateRet::Success) => TurretStateType::Fire,
            (TurretStateType::Aim, StateRet::Failure) => TurretStateType::Hold,
            (TurretStateType::Fire, _) => TurretStateType::Aim,
            (TurretStateType::Recenter, _) => TurretStateType::Idle,
            (TurretStateType::Hold, _) => TurretStateType::Recenter,
            _ => return StateRet::Continue,
        };
        self.set_turret_state(id, next)
    }

    /// `State::friend_checkForTransitions`'s condition list (`StateMachine.cpp:158-176`): only
    /// `FIRE` has one, `outOfWeaponRangeObject -> AIM` (`TurretAI.cpp:88`).
    fn turret_check_conditions(&mut self, id: ObjectId, cur: TurretStateType) -> StateRet {
        if cur == TurretStateType::Fire && self.turret_out_of_weapon_range(id) {
            return self.set_turret_state(id, TurretStateType::Aim);
        }
        StateRet::Continue
    }

    /// `StateMachine::internalSetState` (`StateMachine.cpp:559-651`): clear the sleep, run
    /// `onEnter`, then follow its result (a sleep, or its own transitions).
    fn set_turret_state(&mut self, id: ObjectId, new: TurretStateType) -> StateRet {
        let now = self.frame;
        // `TurretStateMachine::setState` -> `friend_notifyStateMachineChanged` sets
        // `m_sleepUntil = now` when the state changes (`TurretAI.cpp:129-139`, `:677-680`).
        if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
            m.state = new;
            m.state_sleep_until = 0;
            m.sleep_until = now;
        }
        let ret = self.turret_on_enter(id, new);
        // "onEnter could conceivably change m_currentState" (`StateMachine.cpp:644-650`).
        if self.turret_machine(id).is_some_and(|m| m.state != new) {
            return StateRet::Continue;
        }
        match ret {
            StateRet::Sleep(n) => {
                if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                    m.state_sleep_until = now.wrapping_add(n);
                }
                StateRet::Sleep(n)
            }
            StateRet::Continue => self.turret_check_conditions(id, new),
            StateRet::Success | StateRet::Failure => self.turret_transition(id, new, ret),
        }
    }

    /// The states' `onEnter`.
    fn turret_on_enter(&mut self, id: ObjectId, state: TurretStateType) -> StateRet {
        match state {
            // `TurretAIIdleState::onEnter` (`TurretAI.cpp:1294-1308`).
            TurretStateType::Idle => {
                // `ai->resetNextMoodCheckTime()` (`TurretAI.cpp:1299`).
                self.reset_next_mood_check_time(id);
                // `resetIdleScan()` (`TurretAI.cpp:1286-1291`).
                let (lo, hi) = self
                    .objects
                    .get(&id)
                    .map_or((9_999_999, 9_999_999), |o| {
                        (o.template.combat.turret_min_idle_scan_interval, o.template.combat.turret_max_idle_scan_interval)
                    });
                let delay = self.rng.value(lo as i32, hi as i32) as u32;
                if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                    m.next_idle_scan = self.frame.wrapping_add(delay);
                }
                self.turret_idle_sleep(id)
            }
            // `TurretAIIdleScanState::onEnter` (`TurretAI.cpp:1363-1375`).
            TurretStateType::IdleScan => {
                let (min_a, max_a) = self.objects.get(&id).map_or((0.0, 0.0), |o| {
                    (o.template.combat.turret_min_idle_scan_angle, o.template.combat.turret_max_idle_scan_angle)
                });
                if min_a == 0.0 && max_a == 0.0 {
                    return StateRet::Success;
                }
                let mut desired = min_a + self.rng.value_real(0.0, max_a - min_a);
                if self.rng.value(0, 1) == 0 {
                    desired = -desired;
                }
                if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                    m.desired_angle = desired;
                }
                StateRet::Continue
            }
            // `TurretAIHoldTurretState::onEnter` (`TurretAI.cpp:1438-1444`).
            TurretStateType::Hold => {
                let recenter = self.objects.get(&id).map_or(0, |o| o.template.combat.turret_recenter_time);
                if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                    m.hold_timestamp = self.frame.wrapping_add(recenter);
                }
                self.turret_hold_sleep(id)
            }
            // `TurretAIFireState` is `AIAttackFireWeaponState` (TurretAI.cpp:96): its `onEnter` runs
            // `preFireCurrentWeapon(getMachineGoalObject())` (AIStates.cpp:5181-5211) on the turret's target. A position
            // target has no goal object, so it has no victim.
            TurretStateType::Fire => {
                let victim = match self.turret_machine(id).map(|m| m.target) {
                    Some(TurretTarget::Object(v)) => v,
                    _ => ObjectId::INVALID,
                };
                if let Some(slot) = self.turret_current_slot(id) {
                    self.pre_fire_current_weapon(id, slot, victim);
                }
                StateRet::Continue
            }
            // `TurretAIAimTurretState::onEnter` / `Recenter`: `STATE_CONTINUE`.
            TurretStateType::Aim | TurretStateType::Recenter => StateRet::Continue,
        }
    }

    /// `TurretAIIdleState::onEnter`'s `frameToSleepTime(nextMoodCheck, nextIdleScan)`
    /// (`TurretAI.cpp:1307`).
    fn turret_idle_sleep(&mut self, id: ObjectId) -> StateRet {
        let now = self.frame;
        let mood = self.next_mood_check_time(id);
        let scan = self.turret_machine(id).map_or(0, |m| m.next_idle_scan);
        frame_to_sleep(now, mood, scan)
    }

    /// `TurretAIHoldTurretState::onEnter`'s `frameToSleepTime(nextMoodCheck, m_timestamp)`
    /// (`TurretAI.cpp:1443`).
    fn turret_hold_sleep(&mut self, id: ObjectId) -> StateRet {
        let now = self.frame;
        let mood = self.next_mood_check_time(id);
        let ts = self.turret_machine(id).map_or(0, |m| m.hold_timestamp);
        frame_to_sleep(now, mood, ts)
    }

    /// One `state->update()` for the current state.
    fn turret_state_update(&mut self, id: ObjectId, state: TurretStateType) -> StateRet {
        let now = self.frame;
        match state {
            // `TurretAIIdleState::update` (`TurretAI.cpp:1311-1329`).
            TurretStateType::Idle => {
                if now >= self.turret_machine(id).map_or(0, |m| m.next_idle_scan) {
                    return StateRet::Failure;
                }
                self.turret_check_for_idle_mood_target(id);
                self.turret_idle_sleep(id)
            }
            // `TurretAIIdleScanState::update` (`TurretAI.cpp:1382-1396`).
            TurretStateType::IdleScan => {
                if self.objects.get(&id).is_some_and(|o| o.under_construction) {
                    return StateRet::Continue;
                }
                let (natural, desired) = self.objects.get(&id).map_or((0.0, 0.0), |o| {
                    (o.template.combat.turret_natural_angle, o.combat.turret.as_ref().map_or(0.0, |m| m.desired_angle))
                });
                let angle_aligned = self.turret_turn_towards_angle(id, natural + desired, 0.5, 0.0);
                let natural_pitch = self.objects.get(&id).map_or(0.0, |o| o.template.combat.turret_natural_pitch);
                let pitch_aligned = self.turret_turn_towards_pitch(id, natural_pitch, 0.5);
                if angle_aligned && pitch_aligned {
                    StateRet::Success
                } else {
                    StateRet::Continue
                }
            }
            // `TurretAIRecenterTurretState::update` (`TurretAI.cpp:1227-1244`).
            TurretStateType::Recenter => {
                if self.objects.get(&id).is_some_and(|o| o.under_construction) {
                    return StateRet::Continue;
                }
                let (natural, natural_pitch) = self.objects.get(&id).map_or((0.0, 0.0), |o| {
                    (o.template.combat.turret_natural_angle, o.template.combat.turret_natural_pitch)
                });
                let angle_aligned = self.turret_turn_towards_angle(id, natural, 0.5, 0.0);
                let pitch_aligned = self.turret_turn_towards_pitch(id, natural_pitch, 0.5);
                if angle_aligned && pitch_aligned {
                    StateRet::Success
                } else {
                    StateRet::Continue
                }
            }
            // `TurretAIHoldTurretState::update` (`TurretAI.cpp:1456-1466`).
            TurretStateType::Hold => {
                if now >= self.turret_machine(id).map_or(0, |m| m.hold_timestamp) {
                    return StateRet::Success;
                }
                self.turret_check_for_idle_mood_target(id);
                self.turret_hold_sleep(id)
            }
            TurretStateType::Aim => self.turret_aim_update(id),
            TurretStateType::Fire => self.turret_fire_update(id),
        }
    }

    /// `TurretAI::friend_turnTowardsAngle` (`TurretAI.cpp:393-430`). `desired` is relative to the
    /// body; the port's world angle is written back. Returns whether the turret is within
    /// `rel_thresh` of the desired angle.
    fn turret_turn_towards_angle(&mut self, id: ObjectId, desired: f32, rate_modifier: f32, rel_thresh: f32) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        let turn_rate = o.template.combat.turret_turn_rate.unwrap_or(0.0) * rate_modifier;
        let body = o.angle;
        let desired = normalize_angle(desired);
        let orig = normalize_angle(o.combat.turret_angle - body);
        let mut actual = orig;
        let mut rotating = false;
        let diff = normalize_angle(desired - actual);
        if diff.abs() < turn_rate {
            actual = desired;
        } else {
            actual += if diff > 0.0 { turn_rate } else { -turn_rate };
            rotating = true;
        }
        actual = normalize_angle(actual);
        let aligned = (actual - desired).abs() <= rel_thresh;
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.turret_angle = normalize_angle(body + actual);
        }
        if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
            m.rotating = rotating;
        }
        aligned
    }

    /// `TurretAI::friend_turnTowardsPitch` (`TurretAI.cpp:433-462`).
    fn turret_turn_towards_pitch(&mut self, id: ObjectId, desired: f32, rate_modifier: f32) -> bool {
        let Some(o) = self.objects.get(&id) else { return true };
        if !o.template.combat.turret_allows_pitch {
            return true;
        }
        let pitch_rate = o.template.combat.turret_pitch_rate.unwrap_or(0.0) * rate_modifier;
        let desired = normalize_angle(desired);
        let mut actual = o.combat.turret_pitch;
        let diff = normalize_angle(desired - actual);
        if diff.abs() < pitch_rate {
            actual = desired;
        } else {
            actual += if diff > 0.0 { pitch_rate } else { -pitch_rate };
        }
        actual = normalize_angle(actual);
        let aligned = actual == desired;
        if let Some(o) = self.objects.get_mut(&id) {
            o.combat.turret_pitch = actual;
        }
        aligned
    }

    /// `TurretAI::friend_checkForIdleMoodTarget` (`TurretAI.cpp:880-901`).
    fn turret_check_for_idle_mood_target(&mut self, id: ObjectId) {
        // `getMoodMatrixActionAdjustment(MM_Action_Idle)`'s `MAA_Affect_Range_IgnoreAll` (a
        // sleeping attitude); angry-mob infantry are exempt (`AIUpdate.cpp:4360-4375`).
        let (attitude, exempt) = match self.objects.get(&id) {
            Some(o) => (o.ai.attitude, o.template.ignored_in_gui && o.is_kind_of(crate::template::kind_of::INFANTRY)),
            None => return,
        };
        if attitude <= -2 && !exempt {
            return;
        }
        // `ai->getNextMoodTarget(true, true)`.
        let Some(enemy) = self.ai_idle_scan(id, false, None, crate::orders::GuardMode::Normal, false) else { return };
        // `obj->chooseBestWeaponForTarget(enemy, PREFER_MOST_DAMAGE, CMD_FROM_AI)`.
        let now = self.frame;
        let slot = self
            .target_info(&AttackTarget::Object(enemy), self.objects.get(&id).map_or(0, |o| o.player), false)
            .and_then(|t| {
                let o = self.objects.get(&id)?;
                Self::choose_weapon(o, Some(&t), now, None)
            });
        self.set_turret_target_object(id, Some(enemy), false);
        if let Some(slot) = slot {
            if let Some(o) = self.objects.get_mut(&id) {
                o.combat.current_weapon_slot = slot;
            }
        }
        if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
            // `m_targetWasSetByIdleMood = true` (after `setTurretTargetObject` clears it).
            m.target_was_set_by_idle_mood = true;
        }
    }

    /// `TurretAI::setTurretTargetObject` (`TurretAI.cpp:572-611`).
    pub(crate) fn set_turret_target_object(&mut self, id: ObjectId, victim: Option<ObjectId>, force_attacking: bool) {
        let Some(o) = self.objects.get(&id) else { return };
        if o.combat.turret.is_none() {
            return;
        }
        let linked = o.template.combat.turrets_linked;
        let on_turret = {
            let slot = self.turret_current_slot(id);
            let c = &o.template.combat;
            slot.is_some_and(|s| c.turret_controlled_slots == 0 || c.turret_controlled_slots & (1u8 << s) != 0)
        };
        let victim = match victim {
            Some(v) if on_turret || linked => Some(v),
            _ => None,
        };
        let (sid, victim_player) = {
            let o = self.objects.get(&id).expect("checked");
            let sid = o.combat.turret.as_ref().expect("checked").state;
            let victim_player = victim.and_then(|v| self.objects.get(&v)).map(|v| v.player);
            (sid, victim_player)
        };
        let now = self.frame;
        if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
            m.target = victim.map_or(TurretTarget::None, TurretTarget::Object);
            m.target_was_set_by_idle_mood = false;
            m.force_attacking = force_attacking;
        }
        if victim.is_some() {
            if sid != TurretStateType::Aim && sid != TurretStateType::Fire {
                self.set_turret_state(id, TurretStateType::Aim);
            }
            if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                m.victim_initial_player = victim_player;
            }
        } else {
            if sid == TurretStateType::Aim || sid == TurretStateType::Fire {
                self.set_turret_state(id, TurretStateType::Hold);
            }
            if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                m.victim_initial_player = None;
            }
        }
        let _ = now;
    }

    /// `TurretAI::setTurretTargetPosition` (`TurretAI.cpp:614-651`).
    pub(crate) fn set_turret_target_position(&mut self, id: ObjectId, pos: Option<Coord3D>) {
        let Some(o) = self.objects.get(&id) else { return };
        if o.combat.turret.is_none() {
            return;
        }
        let linked = o.template.combat.turrets_linked;
        let on_turret = {
            let slot = self.turret_current_slot(id);
            let c = &o.template.combat;
            slot.is_some_and(|s| c.turret_controlled_slots == 0 || c.turret_controlled_slots & (1u8 << s) != 0)
        };
        let pos = match pos {
            Some(p) if on_turret || linked => Some(p),
            _ => None,
        };
        let sid = self.turret_machine(id).map_or(TurretStateType::Idle, |m| m.state);
        if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
            m.target = pos.map_or(TurretTarget::None, TurretTarget::Position);
            m.target_was_set_by_idle_mood = false;
            m.victim_initial_player = None;
        }
        if pos.is_some() {
            if sid != TurretStateType::Aim && sid != TurretStateType::Fire {
                self.set_turret_state(id, TurretStateType::Aim);
            }
        } else if sid == TurretStateType::Aim || sid == TurretStateType::Fire {
            self.set_turret_state(id, TurretStateType::Hold);
        }
    }

    /// `AIAttackState::onExit`'s `setTurretTargetObject(i, nullptr, FALSE)` (`AIStates.cpp:5740-5741`).
    pub(crate) fn clear_turret_target(&mut self, id: ObjectId) {
        if self.objects.get(&id).is_some_and(|o| o.combat.turret.is_some()) {
            self.set_turret_target_object(id, None, false);
        }
    }

    /// The weapon slot the turret's current weapon is in (`WeaponSet::m_curWeapon`): the attack
    /// order's slot while an attack drives the turret, else the object's current weapon slot (0 at
    /// construction, updated by the turret's own idle-mood choice).
    fn turret_current_slot(&self, id: ObjectId) -> Option<usize> {
        let o = self.objects.get(&id)?;
        if let Some(a) = &o.combat.attack {
            return Some(a.slot);
        }
        Some(o.combat.current_weapon_slot)
    }

    /// `TurretAI::friend_isAnyWeaponInRangeOf` (`TurretAI.cpp:837-859`): any weapon on the turret
    /// whose attack range reaches the victim.
    fn turret_any_weapon_in_range(&self, id: ObjectId, victim: ObjectId) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        let Some(v) = self.objects.get(&victim) else { return false };
        for slot in 0..crate::weapon::WEAPON_SLOT_COUNT {
            let Some(w) = o.combat.weapons.weapon(slot) else { continue };
            if !o.combat.turret.as_ref().is_some_and(|m| m.slot_on_turret(&o.template.combat, slot)) {
                continue;
            }
            let range = w.template.attack_range() * self.combined_weapon_bonus(id, &w.template).get(crate::bonus::WeaponBonusField::Range);
            let min = w.template.min_attack_range();
            let d2 = crate::combat::boundary_dist_sqr(&o.position, o.template.geometry.bounding_circle_radius(), &v.position, v.template.geometry.bounding_circle_radius());
            if d2 <= range * range && d2 >= min * min {
                return true;
            }
        }
        false
    }

    /// `outOfWeaponRangeObject` (`AIStates.cpp:1102-1157`) for the turret's `FIRE` state: only an
    /// object target can be out of range (a position target has no victim).
    fn turret_out_of_weapon_range(&self, id: ObjectId) -> bool {
        let Some(victim) = self.turret_machine(id).and_then(|m| match m.target {
            TurretTarget::Object(v) => Some(v),
            _ => None,
        }) else {
            return false;
        };
        !self.turret_any_weapon_in_range(id, victim)
    }

    /// `Weapon::isWithinAttackRange` for the turret's `AIM` success test (2D bounding-sphere
    /// distance, attack range with the object's bonus, minimum range).
    fn turret_weapon_in_range(&self, id: ObjectId, slot: usize, target: &TurretTarget) -> bool {
        let Some(o) = self.objects.get(&id) else { return false };
        let Some(w) = o.combat.weapons.weapon(slot) else { return false };
        let src_r = o.template.geometry.bounding_circle_radius();
        let range = w.template.attack_range() * self.combined_weapon_bonus(id, &w.template).get(crate::bonus::WeaponBonusField::Range);
        let min = w.template.min_attack_range();
        let d2 = match target {
            TurretTarget::Object(v) => {
                let Some(v) = self.objects.get(v) else { return false };
                crate::combat::boundary_dist_sqr(&o.position, src_r, &v.position, v.template.geometry.bounding_circle_radius())
            }
            TurretTarget::Position(p) => crate::combat::boundary_dist_sqr(&o.position, src_r, p, 0.0),
            TurretTarget::None => return false,
        };
        d2 <= range * range && d2 >= min * min
    }

    /// `TurretAIAimTurretState::update` (`TurretAI.cpp:969-1205`).
    fn turret_aim_update(&mut self, id: ObjectId) -> StateRet {
        let now = self.frame;
        let Some((pos, angle, player)) = self.objects.get(&id).map(|o| (o.position, o.angle, o.player)) else {
            return StateRet::Failure;
        };
        let target = self.turret_machine(id).map_or(TurretTarget::None, |m| m.target);
        let force = self.turret_machine(id).is_some_and(|m| m.force_attacking);
        // `friend_getTurretTarget` with `clearDeadTargets`: a gone/effectively-dead object target is
        // cleared, and the state fails.
        let enemy_pos = match target {
            TurretTarget::None => return StateRet::Failure,
            TurretTarget::Position(p) => p,
            TurretTarget::Object(v) => {
                let alive = self.objects.get(&v).is_some_and(|o| !o.is_dying());
                if !alive {
                    if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                        m.target = TurretTarget::None;
                    }
                    return StateRet::Failure;
                }
                self.objects.get(&v).map_or(Coord3D::ZERO, |o| o.position)
            }
        };
        let Some(slot) = self.turret_current_slot(id) else { return StateRet::Failure };
        let Some(wt) = self.objects.get(&id).and_then(|o| o.combat.weapons.weapon(slot)).map(|w| w.template.clone()) else {
            return StateRet::Failure;
        };

        // `TARGET_OBJECT`'s continued-attack validity (`TurretAI.cpp:996-1024`).
        let mut nothing_in_range = false;
        if let TurretTarget::Object(v) = target {
            let is_primary = self
                .objects
                .get(&id)
                .and_then(|o| o.combat.attack.as_ref())
                .is_some_and(|a| a.target == AttackTarget::Object(v));
            let able = self.is_able_to_attack(id)
                && self.target_info(&AttackTarget::Object(v), player, force).is_some();
            nothing_in_range = !self.turret_any_weapon_in_range(id, v);
            let team_changed = self
                .objects
                .get(&v)
                .is_some_and(|vo| Some(vo.player) != self.turret_machine(id).and_then(|m| m.victim_initial_player));
            if !able || (!is_primary && nothing_in_range) || team_changed {
                if self.turret_machine(id).is_some_and(|m| m.target_was_set_by_idle_mood) {
                    self.set_turret_target_object(id, None, false);
                }
                return StateRet::Failure;
            }
        }

        let rel_angle = relative_angle_2d(pos, angle, enemy_pos);
        let mut aim_angle = rel_angle;
        let mut turn_modifier = 1.0f32;
        let sweep = self.objects.get(&id).map_or(0.0, |o| o.template.combat.turret_fire_angle_sweep[slot]);
        if sweep > 0.0 && self.turret_machine(id).is_some_and(|m| m.sweep_enabled(now)) {
            let positive = self.turret_machine(id).is_some_and(|m| m.positive_sweep);
            aim_angle += if positive { sweep } else { -sweep };
            turn_modifier = self.objects.get(&id).map_or(1.0, |o| o.template.combat.turret_sweep_speed_modifier[slot]);
        }
        let mut turn_aligned = self.turret_turn_towards_angle(id, aim_angle, turn_modifier, AIM_REL_THRESH);
        if sweep > 0.0 {
            if turn_aligned {
                if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                    m.positive_sweep = !m.positive_sweep;
                }
            }
            let cur_rel = self.objects.get(&id).map_or(0.0, |o| normalize_angle(o.combat.turret_angle - o.angle));
            let diff = normalize_angle(rel_angle - cur_rel);
            turn_aligned = diff.abs() < sweep;
        }

        let mut pitch_aligned = true;
        let (allows_pitch, fire_pitch, min_pitch, ground_pitch, max_height) = self.objects.get(&id).map_or((false, 0.0, 0.0, 0.0, 0.0), |o| {
            (
                o.template.combat.turret_allows_pitch,
                o.template.combat.turret_fire_pitch,
                o.template.combat.turret_min_pitch,
                o.template.combat.turret_ground_unit_pitch,
                o.template.combat.turret_max_height,
            )
        });
        if allows_pitch {
            let desired = if fire_pitch > 0.0 {
                fire_pitch
            } else {
                let mut v = Coord3D::new(enemy_pos.x - pos.x, enemy_pos.y - pos.y, enemy_pos.z - pos.z);
                v.z -= max_height / 2.0;
                let len = sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
                let actual = if len > 0.0 { asin((v.z / len).clamp(-1.0, 1.0)) } else { 0.0 };
                let mut d = actual;
                if d < min_pitch {
                    d = min_pitch;
                }
                if ground_pitch > 0.0 {
                    // `if (!enemy) adjust = true` — `enemy` was nulled before the pitch block, so a
                    // ground adjustment always applies for an object target (`TurretAI.cpp:1142-1163`).
                    let range = (wt.attack_range() * self.combined_weapon_bonus(id, &wt).get(crate::bonus::WeaponBonusField::Range)).max(1.0);
                    let dist = len;
                    let ground = ground_pitch * (dist / range);
                    d = actual + ground;
                    if d < min_pitch {
                        d = min_pitch;
                    }
                }
                d
            };
            pitch_aligned = self.turret_turn_towards_pitch(id, desired, 1.0);
        }

        let in_range = self.turret_weapon_in_range(id, slot, &target);
        if turn_aligned && pitch_aligned && in_range && !nothing_in_range {
            StateRet::Success
        } else {
            StateRet::Continue
        }
    }

    /// `AIAttackFireWeaponState::update` driven by the turret (`TurretAI.cpp:96`, `AIStates.cpp:5213-5337`).
    fn turret_fire_update(&mut self, id: ObjectId) -> StateRet {
        let now = self.frame;
        let target = self.turret_machine(id).map_or(TurretTarget::None, |m| m.target);
        if let TurretTarget::Object(v) = target {
            if self.objects.get(&v).is_none_or(|o| o.is_dying()) {
                return StateRet::Failure;
            }
        }
        let Some(slot) = self.turret_current_slot(id) else { return StateRet::Failure };
        let Some(o) = self.objects.get(&id) else { return StateRet::Failure };
        if !o.combat.turret.as_ref().is_some_and(|m| m.slot_on_turret(&o.template.combat, slot)) {
            return StateRet::Failure;
        }
        let Some(status) = o.combat.weapons.weapon(slot).map(|w| w.status(now)) else {
            return StateRet::Failure;
        };
        match status {
            crate::weapon::WeaponStatus::PreAttack => StateRet::Continue,
            crate::weapon::WeaponStatus::ReadyToFire => {
                let target = match target {
                    TurretTarget::Object(v) => AttackTarget::Object(v),
                    TurretTarget::Position(p) => AttackTarget::Ground(p),
                    TurretTarget::None => return StateRet::Failure,
                };
                // `AIAttackFireWeaponState::update` -> `fireCurrentWeapon` (`AIStates.cpp:5265`): a reloaded shot
                // releases the temporary lock (`Object.cpp:1510`).
                if self.fire_weapon(id, slot, target) {
                    self.release_weapon_lock(id, crate::weapon::WeaponLockType::Temporarily);
                }
                if let Some(m) = self.objects.get_mut(&id).and_then(|o| o.combat.turret.as_mut()) {
                    m.did_fire = true;
                }
                // `AIAttackState`'s shot bookkeeping (`consume_shot`/`m_consecutiveShotsFired`).
                if let Some(o) = self.objects.get_mut(&id) {
                    if let Some(a) = o.combat.attack.as_mut() {
                        a.consecutive_shots += 1;
                    }
                }
                StateRet::Success
            }
            _ => StateRet::Failure,
        }
    }
}

/// `TurretAI::isWeaponSlotOnTurret`-driven weapon choice: `WeaponSet::chooseBestWeaponForTarget`
/// treats a weapon whose turret is already aiming at the victim as not ready
/// (`WeaponSet.cpp:891-894`), so another weapon can fire while it turns.
pub(crate) fn turret_aiming_at(attacker: &crate::object::Object, slot: usize, victim: ObjectId) -> bool {
    let Some(t) = attacker.combat.turret.as_ref() else { return false };
    t.slot_on_turret(&attacker.template.combat, slot)
        && matches!(t.state, TurretStateType::Aim | TurretStateType::Fire)
        && t.target == TurretTarget::Object(victim)
}

/// The creation-time draw for an object's `Turret` block, if it has one: the caller runs it at the
/// `TurretAI` constructor's position in the module-construction order.
pub(crate) fn has_turret(t: &ObjectTemplate) -> bool {
    t.combat.turret_turn_rate.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::ObjectId;
    use crate::template::{kind_of, LocomotorKind, LocomotorParams};
    use crate::weapon::{CombatTemplate, WeaponSetTemplate, WeaponStatus, WeaponTemplate};
    use crate::{Command, GameLogic, TerrainLogic};
    use crate::disabled::DisabledType;
    use std::sync::Arc;

    fn turreted(name: &str, weapon: Option<WeaponTemplate>) -> Arc<ObjectTemplate> {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE | kind_of::CAN_ATTACK;
        t.geometry.major_radius = 8.0;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.vision_range = 200.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
        t.combat = CombatTemplate {
            weapon_set: weapon.map(|w| {
                let mut s = WeaponSetTemplate::default();
                s.weapons[0] = Some(Arc::new(w));
                s
            }),
            turret_turn_rate: Some(0.1),
            turret_pitch_rate: Some(0.05),
            ..CombatTemplate::default()
        };
        Arc::new(t)
    }

    fn instant_weapon() -> WeaponTemplate {
        let mut w = WeaponTemplate::new("Gun");
        w.primary_damage = 20.0;
        w.attack_range = 150.0;
        w.min_delay_between_shots = 30;
        w.max_delay_between_shots = 30;
        w
    }

    fn flat() -> TerrainLogic {
        TerrainLogic::flat(100, 100, 0)
    }

    /// The machine's fields, cloned for assertions (the object borrow ends).
    fn machine(g: &GameLogic, id: ObjectId) -> TurretMachine {
        g.find_object(id).unwrap().combat.turret.clone().expect("has a turret")
    }

    /// `AIAttackFireWeaponState::update` -> `fireCurrentWeapon` (`AIStates.cpp:5265`, `Object.cpp:1510`): the
    /// turret's reloaded shot releases the temporary lock on its slot.
    #[test]
    fn a_turret_reloaded_shot_releases_the_temporary_lock() {
        let mut w = instant_weapon();
        w.clip_size = 1;
        w.auto_reloads_clip = true;
        w.clip_reload_time = 1;
        let mut g = GameLogic::new(flat(), 7);
        let t = turreted("T", Some(w));
        let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let enemy = g.create_object(&turreted("E", None), Coord3D::new(120.0, 100.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        assert!(g.set_weapon_lock(id, 0, crate::weapon::WeaponLockType::Temporarily));
        g.objects.get_mut(&id).unwrap().combat.turret.as_mut().unwrap().target = TurretTarget::Object(enemy);
        g.frame = 10;
        assert!(matches!(g.turret_fire_update(id), StateRet::Success), "the turret fires");
        assert_eq!(g.find_object(id).unwrap().combat.weapons.temporary_lock, None, "the reloaded shot releases the lock");
    }

    /// `AIAttackFireWeaponState::onEnter` -> `preFireCurrentWeapon(victim)` (AIStates.cpp:5204, Object.cpp:1549-1561):
    /// entering the turret's FIRE state starts the 10-frame pre-attack on the target, so the weapon reads PreAttack
    /// until frame 20 and is ready from then. Without the Fire-entry call the weapon is ready at once.
    #[test]
    fn entering_fire_starts_the_turret_weapons_pre_attack() {
        let mut w = instant_weapon();
        w.pre_attack_delay = 10;
        let mut g = GameLogic::new(flat(), 7);
        let id = g.create_object(&turreted("T", Some(w)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let enemy = g.create_object(&turreted("E", None), Coord3D::new(120.0, 100.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        g.objects.get_mut(&id).unwrap().combat.turret.as_mut().unwrap().target = TurretTarget::Object(enemy);
        g.frame = 10;
        let _ = g.turret_on_enter(id, TurretStateType::Fire);
        let weapon = |g: &GameLogic| g.find_object(id).unwrap().combat.weapons.weapon(0).unwrap().status(g.frame);
        assert_eq!(weapon(&g), WeaponStatus::PreAttack, "the pre-attack starts on entering fire");
        g.frame = 20;
        assert_eq!(weapon(&g), WeaponStatus::ReadyToFire, "the pre-attack lasts its 10 frames");
    }

    /// `Object::preFireCurrentWeapon` starts a pre-attack only when the weapon can fire next frame
    /// (`frame + 1 >= getPossibleNextShotFrame()`, Object.cpp:1557). A weapon still 30 frames from its next shot does not
    /// pre-fire on the Fire entry; its status stays on the reload.
    #[test]
    fn a_weapon_that_cannot_fire_next_frame_does_not_pre_fire() {
        let mut w = instant_weapon();
        w.pre_attack_delay = 10;
        let mut g = GameLogic::new(flat(), 7);
        let id = g.create_object(&turreted("T", Some(w)), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let enemy = g.create_object(&turreted("E", None), Coord3D::new(120.0, 100.0, 0.0), 0.0, 1);
        g.set_enemies(0, 1);
        g.frame = 10;
        // A shot at frame 10 sets the next shot 30 frames on (`min_delay_between_shots`).
        let _ = g.fire_weapon(id, 0, AttackTarget::Object(enemy));
        g.objects.get_mut(&id).unwrap().combat.turret.as_mut().unwrap().target = TurretTarget::Object(enemy);
        g.frame = 11;
        let _ = g.turret_on_enter(id, TurretStateType::Fire);
        let status = g.find_object(id).unwrap().combat.weapons.weapon(0).unwrap().status(g.frame);
        assert_ne!(status, WeaponStatus::PreAttack, "no pre-attack 19 frames before the next shot");
    }

    #[test]
    fn creation_draws_the_idle_scan_interval() {
        // `resetIdleScan` at creation consumes one integer draw and sets `m_nextIdleScan`.
        let mut g = GameLogic::new(flat(), 7);
        let t = turreted("T", None);
        let before = g.rng.clone();
        let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        assert_ne!(g.rng, before, "the turret constructor draws resetIdleScan");
        let m = machine(&g, id);
        assert!(m.next_idle_scan > 0, "m_nextIdleScan = now + the draw");
        assert_eq!(m.state, TurretStateType::Idle);
    }

    #[test]
    fn idle_entry_resets_the_mood_timer_and_the_scan() {
        // `TurretAIIdleState::onEnter` (`TurretAI.cpp:1294-1308`): `resetNextMoodCheckTime` and
        // `resetIdleScan`, then sleep until the earlier of the two.
        let mut g = GameLogic::new(flat(), 7);
        let t = turreted("T", None);
        let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        // Force the machine into a state whose success/failure re-enters IDLE.
        g.objects.get_mut(&id).unwrap().combat.turret.as_mut().unwrap().state = TurretStateType::Recenter;
        // Snap the recenter immediately (natural angle/pitch, no turn needed).
        g.frame = 100;
        let before = g.rng.clone();
        g.update_turrets(id);
        assert_ne!(g.rng, before, "the IDLE entry draws resetIdleScan");
        let m = machine(&g, id);
        assert_eq!(m.state, TurretStateType::Idle);
        assert_eq!(g.next_mood_check_time(id), 100 + g.ai_tunables.force_idle_frames);
        assert!(m.next_idle_scan >= 100);
    }

    #[test]
    fn idle_scan_draws_a_real_angle_then_a_sign() {
        // `TurretAIIdleScanState::onEnter` (`TurretAI.cpp:1363-1375`): a real draw over the angle
        // range, then an integer draw for the sign.
        let mut g = GameLogic::new(flat(), 7);
        let mut t = (*turreted("T", None)).clone();
        t.combat.turret_min_idle_scan_angle = 0.2;
        t.combat.turret_max_idle_scan_angle = 1.0;
        let t = Arc::new(t);
        let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        // The IDLE scan is due now.
        {
            let m = g.objects.get_mut(&id).unwrap().combat.turret.as_mut().unwrap();
            m.next_idle_scan = 0;
            m.state = TurretStateType::Idle;
            m.state_sleep_until = 0;
            m.sleep_until = 0;
        }
        let before = g.rng.clone();
        g.frame = 50;
        g.update_turrets(id); // IDLE update -> failure -> IDLESCAN onEnter draws
        let mut expected = before.clone();
        let real = 0.2 + expected.value_real(0.0, 0.8);
        let sign = expected.value(0, 1);
        assert_eq!(g.rng, expected, "the scan draws a real angle then the sign");
        let m = machine(&g, id);
        assert_eq!(m.state, TurretStateType::IdleScan);
        assert_eq!(m.desired_angle, if sign == 0 { -real } else { real });
    }

    #[test]
    fn idle_scan_turns_then_holds_then_recenters() {
        // The `IDLE -> IDLESCAN -> HOLD -> RECENTER -> IDLE` cycle with `RecenterTime`.
        let mut g = GameLogic::new(flat(), 7);
        let mut t = (*turreted("T", None)).clone();
        t.combat.turret_min_idle_scan_angle = 0.5;
        t.combat.turret_max_idle_scan_angle = 0.5;
        t.combat.turret_turn_rate = Some(1.0); // snaps in one step
        t.combat.turret_recenter_time = 10;
        let t = Arc::new(t);
        let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        {
            let m = g.objects.get_mut(&id).unwrap().combat.turret.as_mut().unwrap();
            m.next_idle_scan = 0;
            m.state_sleep_until = 0;
            m.sleep_until = 0;
        }
        g.frame = 50;
        g.update_turrets(id); // IDLE -> IDLESCAN (draws)
        assert_eq!(machine(&g, id).state, TurretStateType::IdleScan);
        g.frame = 51;
        g.update_turrets(id); // IDLESCAN snaps to natural +/- 0.5 -> HOLD
        assert_eq!(machine(&g, id).state, TurretStateType::Hold);
        assert_eq!(machine(&g, id).hold_timestamp, 51 + 10);
        g.frame = 60;
        g.update_turrets(id); // not yet at the timestamp
        assert_eq!(machine(&g, id).state, TurretStateType::Hold);
        g.frame = 61;
        g.update_turrets(id); // HOLD -> RECENTER
        assert_eq!(machine(&g, id).state, TurretStateType::Recenter);
        g.frame = 62;
        g.update_turrets(id); // RECENTER snaps to natural -> IDLE (draws resetIdleScan)
        assert_eq!(machine(&g, id).state, TurretStateType::Idle);
    }

    #[test]
    fn aim_turns_at_the_turn_rate_and_fires() {
        let mut g = GameLogic::new(flat(), 7);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.set_enemies(0, 1);
        let a = g.create_object(&turreted("A", Some(instant_weapon())), Coord3D::new(100.0, 100.0, 0.0), std::f32::consts::FRAC_PI_2, 0);
        let b = g.create_object(&turreted("B", None), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
        // Aim at a 90-degree relative angle: 0.1 rad/frame needs ~16 frames.
        g.submit(0, Command::Attack { ids: vec![a], target: b });
        g.run_frames(2);
        assert_eq!(machine(&g, a).state, TurretStateType::Aim, "the turret aims for the AI");
        assert_eq!(g.find_object(b).unwrap().health(), 100.0, "not aligned yet");
        g.run_frames(30);
        assert!(g.find_object(b).unwrap().health() < 100.0, "the turret fires once aligned");
    }

    #[test]
    fn turret_aiming_weapon_is_deprioritised_for_the_other_slot() {
        // `WeaponSet.cpp:891-894`: a weapon whose turret is aiming at the victim is not "ready".
        let mut g = GameLogic::new(flat(), 7);
        let mut t = (*turreted("T", None)).clone();
        let mut s = WeaponSetTemplate::default();
        // The hull weapon is weak, the turret weapon strong: without the "aiming" rule the strong
        // turret weapon would be picked; with it the hull weapon fires while the turret turns.
        let mut gun = instant_weapon();
        gun.primary_damage = 10.0;
        let mut mg = instant_weapon();
        mg.primary_damage = 40.0;
        s.weapons[0] = Some(Arc::new(gun));
        s.weapons[1] = Some(Arc::new(mg));
        t.combat.weapon_set = Some(s);
        // Slot 1 is on the turret, slot 0 is not.
        t.combat.turret_controlled_slots = 1 << 1;
        let t = Arc::new(t);
        let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        // Pretend the turret is aiming slot 1 at the victim.
        let victim = g.create_object(&turreted("V", None), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
        {
            let m = g.objects.get_mut(&id).unwrap().combat.turret.as_mut().unwrap();
            m.state = TurretStateType::Aim;
            m.target = TurretTarget::Object(victim);
        }
        let o = g.find_object(id).unwrap();
        let ti = g.target_info(&AttackTarget::Object(victim), 0, false).unwrap();
        let slot = GameLogic::choose_weapon(o, Some(&ti), g.frame, None).unwrap();
        assert_eq!(slot, 0, "the aiming turret weapon steps aside for the hull weapon");
    }

    #[test]
    fn out_of_range_idle_mood_target_is_cleared() {
        // `TurretAIAimTurretState::update` clears a target it set itself when out of range
        // (`TurretAI.cpp:1019-1023`).
        let mut g = GameLogic::new(flat(), 7);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.set_enemies(0, 1);
        let a = g.create_object(&turreted("A", Some(instant_weapon())), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let b = g.create_object(&turreted("B", None), Coord3D::new(600.0, 100.0, 0.0), 0.0, 1);
        {
            let m = g.objects.get_mut(&a).unwrap().combat.turret.as_mut().unwrap();
            m.state = TurretStateType::Aim;
            m.target = TurretTarget::Object(b);
            m.target_was_set_by_idle_mood = true;
        }
        g.update_turrets(a);
        let m = machine(&g, a);
        assert_eq!(m.target, TurretTarget::None, "the idle-mood target is dropped");
        assert_eq!(m.state, TurretStateType::Hold, "and the turret holds");
    }

    #[test]
    fn an_idle_turret_acquires_an_idle_mood_target() {
        // `TurretAI::friend_checkForIdleMoodTarget` (`TurretAI.cpp:880-901`): the idle update scans
        // (`getNextMoodTarget(true, true)`) and sets the turret target itself.
        let mut g = GameLogic::new(flat(), 7);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
        g.set_enemies(0, 1);
        let mut t = (*turreted("A", Some(instant_weapon()))).clone();
        t.combat.auto_acquire_enemies = true;
        let t = Arc::new(t);
        let a = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let b = g.create_object(&turreted("B", None), Coord3D::new(180.0, 100.0, 0.0), 0.0, 1);
        {
            let m = g.objects.get_mut(&a).unwrap().combat.turret.as_mut().unwrap();
            m.next_idle_scan = 999_999;
            m.sleep_until = 0;
            m.state_sleep_until = 0;
        }
        if let Some(o) = g.objects.get_mut(&a) {
            o.combat.next_mood_check = 0;
            o.combat.randomly_offset_mood_check = false;
        }
        g.update_partition();
        g.update_turrets(a);
        let m = machine(&g, a);
        assert_eq!(m.target, TurretTarget::Object(b), "the idle scan set the target");
        assert!(m.target_was_set_by_idle_mood);
        assert_eq!(m.state, TurretStateType::Aim);
    }

    #[test]
    fn ending_the_attack_clears_the_turret_target() {
        // `AIAttackState::onExit` clears every turret's target (`AIStates.cpp:5740-5741`), which
        // sends an aiming turret to HOLD.
        let mut g = GameLogic::new(flat(), 7);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.set_enemies(0, 1);
        let a = g.create_object(&turreted("A", Some(instant_weapon())), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let b = g.create_object(&turreted("B", None), Coord3D::new(150.0, 100.0, 0.0), 0.0, 1);
        g.set_turret_target_object(a, Some(b), false);
        assert_eq!(machine(&g, a).state, TurretStateType::Aim);
        g.end_attack(a);
        let m = machine(&g, a);
        assert_eq!(m.target, TurretTarget::None);
        assert_eq!(m.state, TurretStateType::Hold);
    }

    #[test]
    fn the_turret_angle_follows_the_body() {
        // `TurretAI::m_angle` is relative to the body, so a body turn carries the turret; the port's
        // world-space angle is carried by the body delta.
        let mut g = GameLogic::new(flat(), 7);
        let t = turreted("T", None);
        let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        {
            let m = g.objects.get_mut(&id).unwrap().combat.turret.as_mut().unwrap();
            m.state = TurretStateType::Hold;
            m.sleep_until = 0;
            m.state_sleep_until = 0;
        }
        g.objects.get_mut(&id).unwrap().combat.turret_angle = 0.5;
        let rel_before = g.find_object(id).unwrap().combat.turret_angle - g.find_object(id).unwrap().angle;
        // Turn the body by 0.3 rad; the turret's relative angle must not change.
        if let Some(o) = g.objects.get_mut(&id) {
            o.angle = 0.3;
        }
        g.update_turrets(id);
        let o = g.find_object(id).unwrap();
        let rel_after = o.combat.turret_angle - o.angle;
        assert!((rel_after - rel_before).abs() < 1e-5, "relative angle kept ({rel_before} -> {rel_after})");
        assert!((o.combat.turret_angle - 0.8).abs() < 1e-5, "world angle carried ({})", o.combat.turret_angle);
    }

    #[test]
    fn disabled_turret_sleeps_until_enabled() {
        // `updateTurretAI` runs the machine only when enabled or recentering
        // (`TurretAI.cpp:715`), and `setTurretEnabled(true)` wakes it (`:762-770`).
        let mut g = GameLogic::new(flat(), 7);
        let mut t = (*turreted("T", None)).clone();
        t.combat.turret_initially_disabled = true;
        let t = Arc::new(t);
        let id = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        g.frame = 10;
        g.update_turrets(id);
        let m = machine(&g, id);
        assert!(!m.enabled);
        assert_eq!(m.sleep_until, 10 + TURRET_SLEEP_FOREVER);
        // Enabling wakes it to the current frame.
        if let Some(m) = g.objects.get_mut(&id).unwrap().combat.turret.as_mut() {
            m.enabled = true;
            m.sleep_until = g.frame;
        }
        g.frame = 11;
        g.update_turrets(id);
        assert_ne!(machine(&g, id).sleep_until, 11 + TURRET_SLEEP_FOREVER);
    }

    #[test]
    fn a_dead_or_disabled_turret_does_not_move() {
        // `AIUpdate.cpp:1085-1090`: `updateTurretAI` runs only when the object is not
        // `isEffectivelyDead()` and not disabled by PARALYZED/UNMANNED/EMP/SUBDUED/HACKED.
        let mut g = GameLogic::new(flat(), 7);
        g.add_player(0, 0);
        g.add_player(1, 0);
        g.set_enemies(0, 1);
        // Body faces north, the target is to the east: an aiming turret would turn -pi/2.
        let a = g.create_object(&turreted("A", Some(instant_weapon())), Coord3D::new(100.0, 100.0, 0.0), std::f32::consts::FRAC_PI_2, 0);
        let b = g.create_object(&turreted("B", None), Coord3D::new(200.0, 100.0, 0.0), 0.0, 1);
        let aim = |g: &mut GameLogic| {
            let m = g.objects.get_mut(&a).unwrap().combat.turret.as_mut().unwrap();
            m.state = TurretStateType::Aim;
            m.target = TurretTarget::Object(b);
            m.victim_initial_player = Some(1);
            m.sleep_until = 0;
            m.state_sleep_until = 0;
            g.objects.get_mut(&a).unwrap().combat.turret_angle = std::f32::consts::FRAC_PI_2;
        };
        // A paralyzed turret does not run the machine.
        aim(&mut g);
        g.set_disabled(a, DisabledType::Paralyzed);
        g.update_turrets(a);
        let o = g.find_object(a).unwrap();
        assert!((o.combat.turret_angle - std::f32::consts::FRAC_PI_2).abs() < 1e-6, "a paralyzed turret does not turn ({})", o.combat.turret_angle);
        assert_eq!(machine(&g, a).state, TurretStateType::Aim, "and its state does not change");
        // Cleared, it turns again (0.1 rad/frame).
        g.clear_disabled(a, DisabledType::Paralyzed);
        g.update_turrets(a);
        assert!(g.find_object(a).unwrap().combat.turret_angle < std::f32::consts::FRAC_PI_2 - 0.09, "it turns once cleared");
        // An effectively dead turret does not run either.
        aim(&mut g);
        g.objects.get_mut(&a).unwrap().effectively_dead = true;
        g.update_turrets(a);
        assert!((g.find_object(a).unwrap().combat.turret_angle - std::f32::consts::FRAC_PI_2).abs() < 1e-6, "a dead turret does not turn");
    }
}
