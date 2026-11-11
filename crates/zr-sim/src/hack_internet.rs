//! `HackInternetAIUpdate`: the hacker's `HACK_INTERNET` button (`MSG_INTERNET_HACK`). The unit kneels
//! down (unpacks), hacks for cash every `CashUpdateDelay` frames, and packs up before it can take
//! another order.
//!
//! Ported from `HackInternetAIUpdate.cpp` (the state machine and `aiDoCommand`),
//! `AIUpdate.cpp:4175` (`privateHackInternet`), `GameLogicDispatch.cpp:1559` (`onInternetHack`) and
//! `AIGroup.cpp:2622` (`groupHackInternet`).
//!
//! The retail hack state machine is the unit's AI machine, so while the unit unpacks, hacks or
//! packs, its other AI does not run ([`is_busy`], used by `update_ai_state_machines`). A player order
//! that reaches a hacking or packing unit is held (`m_pendingCommand`) until it has packed, and an
//! order that reaches an unpacking unit cancels the unpack (its move / attack state replaces the
//! hack state, as `AIStateMachine::clear` does).
//!
//! Not ported: the floating cash text, the sounds and the model conditions (client only),
//! the `InternetHackContain` passenger hook that starts hacking for riders. (The payout is scored:
//! `ScoreKeeper::addMoneyEarned`, see `score.rs`.)

use crate::command::{Command, GameMessage};
use crate::crc::Crc;
use crate::disabled::DisabledType;
use crate::experience::VeterancyLevel;
use crate::logic::GameLogic;
use crate::object::{Object, ObjectId, PlayerIndex};

/// `HackInternetAIUpdateModuleData` (`HackInternetAIUpdate.h`), with the durations in frames
/// (`INI::parseDurationUnsignedInt`: ms rounded up to frames).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HackInternetTemplate {
    /// `UnpackTime`.
    pub unpack_frames: u32,
    /// `PackTime`.
    pub pack_frames: u32,
    /// `CashUpdateDelay`: frames between cash payouts.
    pub cash_update_delay: u32,
    /// `CashUpdateDelayFast`: the delay while the hacker is inside a building (`getCashUpdateDelay`).
    pub cash_update_delay_fast: u32,
    /// `RegularCashAmount`, `VeteranCashAmount`, `EliteCashAmount`, `HeroicCashAmount`.
    pub regular_cash: u32,
    pub veteran_cash: u32,
    pub elite_cash: u32,
    pub heroic_cash: u32,
    /// `XpPerCashUpdate`.
    pub xp_per_cash_update: u32,
    /// `PackUnpackVariationFactor` (`AIUpdateModuleData`): the pack and unpack length is scaled by
    /// a random factor in `1 +- v`.
    pub pack_unpack_variation: f32,
}

/// The hack state machine's states (`UNPACKING`, `HACK_INTERNET`, `PACKING`), plus `Idle` for the
/// unit after it has packed (`AI_IDLE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HackPhase {
    Idle,
    Unpacking,
    Hacking,
    Packing,
}

/// A unit's hack state: the phase, its `m_framesRemaining`, and the order held while it packs.
#[derive(Clone, Debug, PartialEq)]
pub struct HackState {
    pub phase: HackPhase,
    pub frames_remaining: u32,
    /// `HackInternetAIUpdate::m_pendingCommand`: an order that runs once the unit is idle.
    pub pending: Option<GameMessage>,
}

impl HackState {
    pub(crate) fn crc(&self, c: &mut Crc) {
        c.add_u32(self.phase as u32);
        c.add_u32(self.frames_remaining);
        match &self.pending {
            Some(m) => {
                c.add_bool(true);
                c.add_u32(u32::from(m.player));
                c.add_str(&format!("{:?}", m.command));
            }
            None => c.add_bool(false),
        }
    }
}

/// Whether the unit is running its hack state machine (its other AI is paused).
pub fn is_busy(o: &Object) -> bool {
    o.hack_internet.as_ref().is_some_and(|h| h.phase != HackPhase::Idle)
}

/// `HackInternetAIUpdate::getHeroic/Elite/...CashAmount` in `HackInternetState::update`
/// (`HackInternetAIUpdate.cpp`): the amount for the unit's veterancy, falling through to the next
/// lower level when that amount is zero, and to 1 when every amount is zero.
pub fn cash_amount(level: VeterancyLevel, t: &HackInternetTemplate) -> u32 {
    let by_rank = [t.regular_cash, t.veteran_cash, t.elite_cash, t.heroic_cash];
    let rank = level as usize;
    (0..=rank).rev().map(|i| by_rank[i]).find(|&a| a != 0).unwrap_or(1)
}

/// The AI orders that `HackInternetAIUpdate::aiDoCommand` intercepts (every `ai*` call that goes
/// through `aiDoCommand`): the move, attack, guard, stop, enter and weapon-lock families. Other
/// commands are not intercepted (see the module doc).
fn ai_order_ids(cmd: &mut Command) -> Option<&mut Vec<ObjectId>> {
    match cmd {
        Command::MoveTo { ids, .. }
        | Command::ForceMove { ids, .. }
        | Command::AttackMove { ids, .. }
        | Command::Attack { ids, .. }
        | Command::ForceAttack { ids, .. }
        | Command::AttackGround { ids, .. }
        | Command::Stop { ids }
        | Command::Guard { ids, .. }
        | Command::Scatter { ids }
        | Command::Enter { ids, .. }
        | Command::DoWeapon { ids, .. }
        | Command::DoWeaponAtObject { ids, .. }
        | Command::DoWeaponAtLocation { ids, .. } => Some(ids),
        _ => None,
    }
}

impl GameLogic {
    /// `MSG_INTERNET_HACK` -> `onInternetHack` (`GameLogicDispatch.cpp:1559`): `AIGroup::groupHackInternet` for each
    /// owned unit. The temporary-lock release (`releaseWeaponLockForGroup(LOCKED_TEMPORARILY)`, `:1564`) runs in the
    /// dispatch before this call, on the owner-filtered selection (`release_group_temporary_locks`), so it is not
    /// repeated here: a unit the player does not own keeps its lock.
    pub(crate) fn command_hack_internet(&mut self, player: PlayerIndex, ids: &[ObjectId]) {
        for &id in ids {
            let Some(o) = self.objects.get_mut(&id) else { continue };
            if o.destroyed || o.player != player || o.ai.suspended {
                continue;
            }
            self.hack_order(player, id);
        }
    }

    /// `Object::doCommandButton`'s `GUI_COMMAND_HACK_INTERNET` branch (Object.cpp:5587-5593): `ai->aiHackInternet(cmdSource)`
    /// for one member. Unlike `MSG_INTERNET_HACK` (`onInternetHack`, GameLogicDispatch.cpp:1559), the script button
    /// does not release the unit's temporary weapon locks, so this is the hack order alone. The caller checks the AI.
    pub(crate) fn ai_hack_internet(&mut self, id: ObjectId) {
        let Some(o) = self.objects.get(&id) else { return };
        if o.destroyed {
            return;
        }
        let player = o.player;
        self.hack_order(player, id);
    }

    /// `AIUpdateInterface::aiHackInternet` -> `HackInternetAIUpdate::aiDoCommand`
    /// (`HackInternetAIUpdate.cpp:124`) -> `privateHackInternet` (`AIUpdate.cpp:4175`).
    fn hack_order(&mut self, player: PlayerIndex, id: ObjectId) {
        let Some(o) = self.objects.get(&id) else { return };
        if o.template.hack_internet.is_none() {
            // `privateHackInternet` clears the machine before it looks for the module, then
            // `DEBUG_CRASH`s: a unit without `HackInternetAIUpdate` just stops what it was doing.
            if o.ai_handle.is_some() {
                self.end_attack(id);
                self.ai_clear(id);
            }
            return;
        }
        let phase = o.hack_internet.as_ref().map_or(HackPhase::Idle, |h| h.phase);
        match phase {
            // A hack order while already hacking or unpacking is ignored (the
            // `!RETAIL_COMPATIBLE_CRC` bugfix at `HackInternetAIUpdate.cpp:133`).
            HackPhase::Hacking | HackPhase::Unpacking => {}
            // While packing the order is held and the unit packs on (`aiDoCommand`'s pending path).
            HackPhase::Packing => {
                self.hold_order(player, id, Command::HackInternet { ids: vec![id] }, crate::ai_states::CommandSource::Ai);
            }
            HackPhase::Idle => self.hack_begin_unpack(id),
        }
    }

    /// `privateHackInternet` -> `HackInternetAIUpdate::hackInternet` -> `setState(UNPACKING)`
    /// (`UnpackingState::onEnter`): the machine is cleared (the AI order and any attack go), then the
    /// unit unpacks for `UnpackTime` scaled by the random variation.
    fn hack_begin_unpack(&mut self, id: ObjectId) {
        let Some(t) = self.objects.get(&id).and_then(|o| o.template.hack_internet) else { return };
        self.end_attack(id);
        self.ai_clear(id);
        let frames = self.variation_frames(t.unpack_frames, t.pack_unpack_variation);
        if let Some(o) = self.objects.get_mut(&id) {
            o.hack_internet = Some(HackState { phase: HackPhase::Unpacking, frames_remaining: frames, pending: None });
        }
    }

    /// `GameLogicRandomValueReal(1 - v, 1 + v) * base`, truncated to frames. Drawn even for a zero
    /// base, as the engine does (`UnpackingState::onEnter` / `PackingState::onEnter`).
    fn variation_frames(&mut self, base: u32, variation: f32) -> u32 {
        let v = self.rng.value_real(1.0 - variation, 1.0 + variation);
        (base as f32 * v) as u32
    }

    /// The pack length: `getPackTime()` is zero when the unit is inside a building, as it does not
    /// need to pack when it leaves one (`HackInternetAIUpdate.cpp`).
    fn pack_frames_for(&mut self, id: ObjectId) -> u32 {
        let Some(o) = self.objects.get(&id) else { return 0 };
        let Some(t) = o.template.hack_internet else { return 0 };
        let base = if o.contained_by.is_some() { 0 } else { t.pack_frames };
        self.variation_frames(base, t.pack_unpack_variation)
    }

    /// The cash delay for the current containment (`getCashUpdateDelay`).
    fn cash_delay(&self, id: ObjectId) -> u32 {
        match self.objects.get(&id).and_then(|o| o.template.hack_internet.map(|t| (o.contained_by.is_some(), t))) {
            Some((true, t)) => t.cash_update_delay_fast,
            Some((false, t)) => t.cash_update_delay,
            None => 0,
        }
    }

    /// Hold a player order on a hacking unit (`aiDoCommand`: `m_pendingCommand.store`, then
    /// `PACKING` when it was hacking). Only the hacking and packing phases hold an order.
    fn hold_order(&mut self, player: PlayerIndex, id: ObjectId, command: Command, source: crate::ai_states::CommandSource) {
        let phase = match self.objects.get(&id).and_then(|o| o.hack_internet.as_ref()) {
            Some(h) => h.phase,
            None => return,
        };
        match phase {
            HackPhase::Hacking => {
                // `getStateMachine()->clear(); setState(PACKING)`: `PackingState::onEnter`.
                let frames = self.pack_frames_for(id);
                if let Some(h) = self.objects.get_mut(&id).and_then(|o| o.hack_internet.as_mut()) {
                    h.phase = HackPhase::Packing;
                    h.frames_remaining = frames;
                    h.pending = Some(GameMessage { player, command, source });
                }
            }
            HackPhase::Packing => {
                if let Some(h) = self.objects.get_mut(&id).and_then(|o| o.hack_internet.as_mut()) {
                    h.pending = Some(GameMessage { player, command, source });
                }
            }
            HackPhase::Unpacking | HackPhase::Idle => {}
        }
    }

    /// `HackInternetAIUpdate::aiDoCommand`'s interception of the player's AI orders, applied by
    /// `process_command_list` before dispatch. The orders of units that are hacking or packing are
    /// held (one message per unit, with that unit's id alone); an unpacking unit's order cancels the
    /// unpack and runs. Returns the message without the held units, or `None` if none are left.
    pub(crate) fn hold_hacker_orders(&mut self, mut msg: GameMessage) -> Option<GameMessage> {
        let player = msg.player;
        let source = msg.source;
        let held: Vec<ObjectId> = match ai_order_ids(&mut msg.command) {
            Some(ids) => ids
                .iter()
                .copied()
                .filter(|id| {
                    self.objects.get(id).is_some_and(|o| {
                        o.player == player && o.hack_internet.as_ref().is_some_and(|h| matches!(h.phase, HackPhase::Hacking | HackPhase::Packing))
                    })
                })
                .collect(),
            None => return Some(msg),
        };
        if held.is_empty() {
            self.cancel_unpacking(player, &msg.command);
            return Some(msg);
        }
        for &id in &held {
            let mut single = msg.command.clone();
            if let Some(v) = ai_order_ids(&mut single) {
                *v = vec![id];
            }
            self.hold_order(player, id, single, source);
        }
        self.cancel_unpacking(player, &msg.command);
        let ids = ai_order_ids(&mut msg.command)?;
        ids.retain(|id| !held.contains(id));
        if ids.is_empty() {
            None
        } else {
            Some(msg)
        }
    }

    /// An order that reaches an unpacking unit replaces its hack state (`AIStateMachine::clear`).
    fn cancel_unpacking(&mut self, player: PlayerIndex, command: &Command) {
        let mut cmd = command.clone();
        let Some(ids) = ai_order_ids(&mut cmd) else { return };
        for &id in ids.iter() {
            if let Some(o) = self.objects.get_mut(&id) {
                if o.player != player {
                    continue;
                }
                if let Some(h) = o.hack_internet.as_mut() {
                    if h.phase == HackPhase::Unpacking {
                        h.phase = HackPhase::Idle;
                        h.frames_remaining = 0;
                        h.pending = None;
                    }
                }
            }
        }
    }

    /// Advance every hack state machine one frame (`HackInternetAIUpdate::update` and the state
    /// `update`s). Runs before the AI state machines, as the hack update is the unit's AI update.
    pub(crate) fn update_hack_internet(&mut self) {
        let now = self.frame;
        let ids: Vec<ObjectId> = self
            .objects
            .values()
            .filter(|o| o.hack_internet.is_some() && !o.destroyed && !o.is_dying() && o.ai.wake_frame <= now)
            .map(|o| o.id)
            .collect();
        for id in ids {
            // `HackInternetAIUpdate::update`: an order held while packing runs once the unit is idle.
            let pending = self.objects.get_mut(&id).and_then(|o| o.hack_internet.as_mut()).and_then(|h| {
                if h.phase == HackPhase::Idle {
                    h.pending.take()
                } else {
                    None
                }
            });
            if let Some(msg) = pending {
                self.apply_message_now(msg);
            }
            self.hack_tick(id);
        }
    }

    /// One frame of the unit's state: `UnpackingState::update`, `HackInternetState::update` and
    /// `PackingState::update`.
    fn hack_tick(&mut self, id: ObjectId) {
        let Some(o) = self.objects.get(&id) else { return };
        let Some(h) = o.hack_internet.as_ref() else { return };
        let (phase, frames) = (h.phase, h.frames_remaining);
        let hacked = o.is_disabled_by_type(DisabledType::Hacked);
        match phase {
            HackPhase::Idle => {}
            HackPhase::Unpacking => {
                if frames > 0 {
                    self.set_hack_frames(id, frames - 1);
                } else {
                    // `UnpackingState::update` returns SUCCESS: `HackInternetState::onEnter` waits a
                    // full cash delay before the first payout.
                    let delay = self.cash_delay(id);
                    self.set_hack_phase(id, HackPhase::Hacking, delay);
                }
            }
            HackPhase::Hacking => {
                // "Don't hack while hacked": the counter does not run either.
                if hacked {
                    return;
                }
                if frames > 0 {
                    self.set_hack_frames(id, frames - 1);
                } else {
                    self.hack_cash(id);
                    let delay = self.cash_delay(id);
                    self.set_hack_frames(id, delay);
                }
            }
            HackPhase::Packing => {
                if frames > 0 {
                    self.set_hack_frames(id, frames - 1);
                } else {
                    // `PackingState::update` returns SUCCESS: the machine goes to `AI_IDLE`. A held
                    // order runs on the next frame's `update`.
                    self.set_hack_phase(id, HackPhase::Idle, 0);
                }
            }
        }
    }

    /// `HackInternetState::update`'s payout (`HackInternetAIUpdate.cpp`): the veterancy amount is
    /// deposited, and the unit gains `XpPerCashUpdate`.
    fn hack_cash(&mut self, id: ObjectId) {
        let Some(o) = self.objects.get(&id) else { return };
        let Some(t) = o.template.hack_internet else { return };
        let (player, level) = (o.player, o.experience.level);
        let amount = cash_amount(level, &t);
        self.deposit_cash(player, amount);
        // `HackInternetAIUpdate` (HackInternetAIUpdate.cpp:541): `addMoneyEarned(amount)`.
        self.score_money_earned(player, amount);
        self.add_experience(id, t.xp_per_cash_update as i32);
    }

    fn set_hack_frames(&mut self, id: ObjectId, frames: u32) {
        if let Some(h) = self.objects.get_mut(&id).and_then(|o| o.hack_internet.as_mut()) {
            h.frames_remaining = frames;
        }
    }

    fn set_hack_phase(&mut self, id: ObjectId, phase: HackPhase, frames: u32) {
        if let Some(h) = self.objects.get_mut(&id).and_then(|o| o.hack_internet.as_mut()) {
            h.phase = phase;
            h.frames_remaining = frames;
        }
    }
}
