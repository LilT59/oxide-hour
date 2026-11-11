//! Player money (`Common/RTS/Money.cpp`, `Common/RTS/Player.cpp`).
//!
//! The engine's `Money` is a simple non-negative counter: `withdraw` clamps to the balance and
//! returns the amount actually taken; `deposit` adds. The income-bucket / cash-per-minute tracking
//! is not ported.

use crate::object::PlayerIndex;
use crate::score::ScoreKeeper;

/// One player's cash (`Money`) and score keeper (`Player::m_scoreKeeper`). The rest of the `Player`
/// subsystem (teams, upgrades, sciences, energy) is not ported yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    pub index: PlayerIndex,
    pub money: u32,
    /// `Player::m_defaultTeam` (`Player.cpp:760-770`): the name of the player's default team, whose
    /// team instance every object created for the player joins (`Object::Object`, `Object.cpp:312`).
    pub default_team: Option<String>,
    /// `Player::m_cashBountyPercent`: fraction of a killed enemy's build cost paid to this player
    /// (`CashBountyPower`).
    pub cash_bounty: f32,
    /// `Player::m_logicalRetaliationModeEnabled` (`Player.cpp:498`): always `false` when a game starts;
    /// only the logic message `MSG_ENABLE_RETALIATION_MODE` sets it (`GameLogicDispatch.cpp:1082`). Read by
    /// `ActiveBody::attemptDamage`'s retaliation block (`ActiveBody.cpp:694`).
    pub logical_retaliation_mode_enabled: bool,
    /// `Player::m_scoreKeeper`: the score screen's counters (`score.rs`).
    pub score: ScoreKeeper,
    /// `Player::m_radarCount` (`Player.cpp:314`): radar facilities this player owns.
    pub radar_count: i32,
    /// `Player::m_disableProofRadarCount` (`Player.cpp:315`): the subset of `radar_count` whose
    /// `RadarUpgrade` has `DisableProof = Yes` (the GLA radar van).
    pub disable_proof_radar_count: i32,
    /// `Player::m_radarDisabled` (`Player.cpp:316`): the power grid cannot support radar
    /// (`Player::onPowerBrownOutChange` -> `disableRadar` / `enableRadar`).
    pub radar_disabled: bool,
    /// `Player::m_kindOfPercentProductionChangeList` (`Player.cpp:3862-3938`): the
    /// `CostModifierUpgrade` entries, `(EffectKindOf mask, Percentage, refcount)` in insertion
    /// order (`getProductionCostChangeBasedOnKindOf` multiplies them in that order).
    pub production_cost_changes: Vec<(u64, f32, i32)>,
}

impl Player {
    pub fn new(index: PlayerIndex, money: u32) -> Self {
        Player {
            index,
            money,
            default_team: None,
            cash_bounty: 0.0,
            logical_retaliation_mode_enabled: false,
            score: ScoreKeeper::new(index),
            radar_count: 0,
            disable_proof_radar_count: 0,
            radar_disabled: false,
            production_cost_changes: Vec::new(),
        }
    }

    /// `Money::deposit`.
    pub fn deposit(&mut self, amount: u32) {
        self.money = self.money.saturating_add(amount);
    }

    /// `Money::withdraw`: clamps to the current balance and returns what was taken.
    pub fn withdraw(&mut self, amount: u32) -> u32 {
        let taken = amount.min(self.money);
        self.money -= taken;
        taken
    }

    /// `Money::countMoney`.
    pub fn count_money(&self) -> u32 {
        self.money
    }

    /// `Player::addRadar` (`Player.cpp:3195-3214`): bump the refcount; a `DisableProof` facility
    /// also bumps the disable-proof count (it is included in the normal count).
    pub fn add_radar(&mut self, disable_proof: bool) {
        self.radar_count += 1;
        if disable_proof {
            self.disable_proof_radar_count += 1;
        }
    }

    /// `Player::removeRadar` (`Player.cpp:3217-3236`).
    pub fn remove_radar(&mut self, disable_proof: bool) {
        self.radar_count -= 1;
        if disable_proof {
            self.disable_proof_radar_count -= 1;
        }
    }

    /// `Player::disableRadar` (`Player.cpp:3238-3252`).
    pub fn disable_radar(&mut self) {
        self.radar_disabled = true;
    }

    /// `Player::enableRadar` (`Player.cpp:3254-3268`): removes the restriction only.
    pub fn enable_radar(&mut self) {
        self.radar_disabled = false;
    }

    /// `Player::hasRadar` (`Player.cpp:3270-3278`): a brownout hides every radar unless a
    /// `DisableProof` facility is online.
    pub fn has_radar(&self) -> bool {
        if self.radar_disabled && self.disable_proof_radar_count == 0 {
            return false;
        }
        self.radar_count > 0
    }

    /// `Player::addKindOfProductionCostChange` (`Player.cpp:3862-3886`): an identical
    /// `(kindOf, percent)` entry bumps its reference count; otherwise a new entry is appended.
    pub fn add_production_cost_change(&mut self, kind_of: u64, percentage: f32) {
        if let Some(e) = self.production_cost_changes.iter_mut().find(|e| e.1 == percentage && e.0 == kind_of) {
            e.2 += 1;
            return;
        }
        self.production_cost_changes.push((kind_of, percentage, 1));
    }

    /// `Player::removeKindOfProductionCostChange` (`Player.cpp:3888-3911`): drop one reference,
    /// removing the entry at zero.
    pub fn remove_production_cost_change(&mut self, kind_of: u64, percentage: f32) {
        let Some(i) = self.production_cost_changes.iter().position(|e| e.1 == percentage && e.0 == kind_of) else { return };
        if self.production_cost_changes[i].2 <= 1 {
            self.production_cost_changes.remove(i);
        } else {
            self.production_cost_changes[i].2 -= 1;
        }
    }

    /// `Player::getProductionCostChangeBasedOnKindOf` (`Player.cpp:3913-3932`): the product of
    /// `1 + Percentage` over every entry whose kind-of mask the object's mask contains
    /// (`TEST_KINDOFMASK_MULTI`).
    pub fn production_cost_change_based_on_kind_of(&self, kind_of: u64) -> f32 {
        let mut start = 1.0f32;
        for e in &self.production_cost_changes {
            if e.0 & !kind_of == 0 {
                start *= 1.0 + e.1;
            }
        }
        start
    }
}

impl crate::logic::GameLogic {
    /// `GameLogic::onEnableRetaliationMode` (`GameLogicDispatch.cpp:1082-1108`): the message's own player sets
    /// its flag. The maintained engine's `#else` branch (no `RETAIL_COMPATIBLE_CRC`) reads the boolean from
    /// argument 0 and writes `msgPlayer`, so a client can only change its own mode.
    pub(crate) fn command_enable_retaliation_mode(&mut self, player: PlayerIndex, enabled: bool) {
        if let Some(p) = self.players.get_mut(&player) {
            p.logical_retaliation_mode_enabled = enabled;
        }
    }
}
