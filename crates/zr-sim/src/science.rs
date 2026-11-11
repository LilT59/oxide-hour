//! Sciences and general's promotions (`Common/RTS/Science.cpp`, `Player::addSciencePurchasePoints`
//! / `Player::promote`, `RankInfo`).
//!
//! A player earns skill points from kills (`ExperienceValue`), which promote them through the
//! `Rank` table (`SkillPointsNeeded`), granting sciences (`SciencesGranted`) and science purchase
//! points (`SciencePurchasePointsGranted`). Sciences gate special powers and upgrades; they can
//! also be bought with purchase points (`Command::PurchaseScience`).

use std::collections::BTreeSet;

use crate::logic::GameLogic;
use crate::object::PlayerIndex;

/// A `Science` template (sim copy).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ScienceInfo {
    pub science_purchase_point_cost: i32,
    pub prerequisite_sciences: Vec<String>,
    /// `ScienceTemplate::m_isGrantable` (`IsGrantable`): only a grantable science can be granted by
    /// `Player::grantScience` (`Player.cpp:2655-2664`), which `GrantScienceUpgrade` uses.
    pub grantable: bool,
}

/// A `Rank` entry (sim copy).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RankInfo {
    pub skill_points_needed: i32,
    pub sciences_granted: Vec<String>,
    pub science_purchase_points_granted: u32,
}

/// A player's science state (`Player::m_sciences`, `m_sciencePurchasePoints`, `m_skillPoints`,
/// `m_rankLevel`).
#[derive(Clone, Debug)]
pub struct ScienceState {
    pub sciences: BTreeSet<String>,
    pub science_purchase_points: u32,
    pub skill_points: u32,
    pub rank_level: usize,
    /// `Player::m_skillPointsModifier` (1.0 by default; `PLAYER_AFFECT_RECEIVING_EXPERIENCE` sets it). Not in the
    /// CRC: the C++ `Player::crc` does not hash it either.
    pub skill_points_modifier: f32,
}

impl Default for ScienceState {
    fn default() -> Self {
        ScienceState { sciences: BTreeSet::new(), science_purchase_points: 0, skill_points: 0, rank_level: 0, skill_points_modifier: 1.0 }
    }
}

impl ScienceState {
    pub(crate) fn crc(&self, c: &mut crate::crc::Crc) {
        c.add_u32(self.sciences.len() as u32);
        for s in &self.sciences {
            c.add_str(s);
        }
        c.add_u32(self.science_purchase_points);
        c.add_u32(self.skill_points);
        c.add_u32(self.rank_level as u32);
    }
}

impl GameLogic {
    fn science_state(&self, player: PlayerIndex) -> Option<&ScienceState> {
        self.sciences.get(&player)
    }
    fn science_state_mut(&mut self, player: PlayerIndex) -> &mut ScienceState {
        self.sciences.entry(player).or_default()
    }

    /// `Player::hasScience`.
    pub fn player_has_science(&self, player: PlayerIndex, name: &str) -> bool {
        self.science_state(player).is_some_and(|s| s.sciences.contains(name))
    }

    /// A player's science/rank state (`Player::m_sciences`, purchase/skill points, rank level).
    pub fn player_science_state(&self, player: PlayerIndex) -> Option<&ScienceState> {
        self.science_state(player)
    }

    /// `Player::getSciences`.
    pub fn player_sciences(&self, player: PlayerIndex) -> Option<&BTreeSet<String>> {
        self.science_state(player).map(|s| &s.sciences)
    }

    /// `Player::getSciencePurchasePoints`.
    pub fn player_science_purchase_points(&self, player: PlayerIndex) -> u32 {
        self.science_state(player).map_or(0, |s| s.science_purchase_points)
    }

    /// `Player::getSkillPoints`.
    pub fn player_skill_points(&self, player: PlayerIndex) -> u32 {
        self.science_state(player).map_or(0, |s| s.skill_points)
    }

    /// `Player::getRankLevel` (1-based, like the engine).
    pub fn player_rank_level(&self, player: PlayerIndex) -> i32 {
        self.science_state(player).map_or(1, |s| s.rank_level as i32 + 1)
    }

    /// `Player::getSkillPointsLevelDown/Up` (`m_levelDown`/`m_levelUp`, set in `setRankLevel`):
    /// the skill-point band of the current rank, for the experience bar.
    pub fn player_rank_band(&self, player: PlayerIndex) -> (u32, u32) {
        let level = self.science_state(player).map_or(0, |s| s.rank_level);
        let down = self.rank_info.get(level).map_or(0, |r| r.skill_points_needed.max(0) as u32);
        let up = self.rank_info.get(level + 1).map_or(u32::MAX, |r| r.skill_points_needed.max(0) as u32);
        (down, up)
    }

    /// The `Science` template (`TheScienceStore::findScience`), for cost / prerequisites.
    pub fn science_info(&self, name: &str) -> Option<&ScienceInfo> {
        self.science_templates.get(name)
    }

    /// The `Rank` table length (`TheRankInfoStore::getRankLevelCount`).
    pub fn rank_count(&self) -> usize {
        self.rank_info.len()
    }

    /// One `Rank` entry (`TheRankInfoStore::getRankInfo`), for diagnostics.
    pub fn rank_info_at(&self, i: usize) -> Option<&RankInfo> {
        self.rank_info.get(i)
    }

    /// `Player::addScience` (Player.cpp:2565): a science the player already has changes nothing;
    /// a new one wakes the special powers that require it.
    pub fn add_science(&mut self, player: PlayerIndex, name: &str) {
        if !self.science_state_mut(player).sciences.insert(name.to_string()) {
            return;
        }
        self.wake_special_powers_for_science(player, name);
    }

    /// `Player::resetRank` (`Player.cpp:2700`): rank level 1 (the sim stores `rank_level` 0-based),
    /// zero skill points, the rank-1 `SciencePurchasePointsGranted`, and `resetSciences`' rank-1
    /// `SciencesGranted` (`SCIENCE_Rank1`). The `PlayerTemplate`'s `IntrinsicSciences` are granted
    /// separately by the adapter, as in `resetSciences`.
    pub fn reset_rank(&mut self, player: PlayerIndex) {
        let cur = self.rank_info.first().cloned().unwrap_or_default();
        // `resetRank` leaves `m_skillPointsModifier` alone (it is set at `Player::init`, not per rank).
        let modifier = self.skill_points_modifier(player);
        let mut st = ScienceState { science_purchase_points: cur.science_purchase_points_granted, skill_points_modifier: modifier, ..Default::default() };
        for s in &cur.sciences_granted {
            st.sciences.insert(s.clone());
        }
        self.sciences.insert(player, st);
    }

    /// `Player::isCapableOfPurchasingScience` (`Player.cpp:2667`): the science exists, is not owned,
    /// its prerequisite sciences are all owned, and it is purchasable (`SciencePurchasePointCost`
    /// of 0 means "not purchasable") for at most the player's purchase points. `isScienceDisabled` /
    /// `isScienceHidden` are not modelled (no data marks any shipped science so).
    pub fn is_capable_of_purchasing_science(&self, player: PlayerIndex, name: &str) -> bool {
        let Some(info) = self.science_templates.get(name) else { return false };
        if self.player_has_science(player, name) {
            return false;
        }
        for pre in &info.prerequisite_sciences {
            if !self.player_has_science(player, pre) {
                return false;
            }
        }
        let cost = info.science_purchase_point_cost;
        if cost == 0 || cost > self.player_science_purchase_points(player) as i32 {
            return false;
        }
        true
    }

    /// `MSG_PURCHASE_SCIENCE`: spend purchase points on a science whose prerequisites are met.
    pub fn command_purchase_science(&mut self, player: PlayerIndex, name: &str) -> bool {
        let Some(info) = self.science_templates.get(name).cloned() else { return false };
        if self.player_has_science(player, name) {
            return false;
        }
        if info.science_purchase_point_cost < 0 {
            return false; // granted, not purchasable
        }
        for pre in &info.prerequisite_sciences {
            if !self.player_has_science(player, pre) {
                return false;
            }
        }
        let cost = info.science_purchase_point_cost as u32;
        let st = self.science_state_mut(player);
        if st.science_purchase_points < cost {
            return false;
        }
        st.science_purchase_points -= cost;
        st.sciences.insert(name.to_string());
        true
    }

    /// `Player::m_skillPointsModifier` (1.0 when the player has no state yet).
    pub fn skill_points_modifier(&self, player: PlayerIndex) -> f32 {
        self.science_state(player).map_or(1.0, |s| s.skill_points_modifier)
    }

    /// `Player::setSkillPointsModifier` (`PLAYER_AFFECT_RECEIVING_EXPERIENCE`, ScriptActions.cpp:6469).
    pub fn set_skill_points_modifier(&mut self, player: PlayerIndex, modifier: f32) {
        self.science_state_mut(player).skill_points_modifier = modifier;
    }

    /// `GameLogic::setRankLevelLimit` (`PLAYER_SET_RANKLEVELLIMIT`).
    pub fn set_rank_level_limit(&mut self, limit: i32) {
        self.rank_level_limit = limit;
    }

    /// `Player::addSkillPoints` (Player.cpp:2500): the points are scaled by the player's modifier and rounded up
    /// (`REAL_TO_INT_CEIL`), the total is capped at the points needed for the highest allowed rank
    /// (`min(GameLogic::getRankLevelLimit, getRankLevelCount)`, Player.cpp:2507-2509), and the player is promoted
    /// while the points reach the next rank, never past that highest rank (`setRankLevel` clamps it, :2721-2725).
    pub fn add_skill_points(&mut self, player: PlayerIndex, points: u32) {
        let delta = (self.skill_points_modifier(player) * points as f32).ceil() as i64;
        if delta <= 0 {
            return;
        }
        let ranks = self.rank_info.clone();
        // `RankInfo` is 1-based in the engine (`getRankInfo(level)`); the sim's `rank_info[0]` is rank 1.
        let level_cap = (i64::from(self.rank_level_limit).min(ranks.len() as i64)).max(0) as usize;
        let point_cap = level_cap.checked_sub(1).and_then(|i| ranks.get(i)).map_or(i64::MAX, |r| i64::from(r.skill_points_needed));
        let st = self.science_state_mut(player);
        st.skill_points = (i64::from(st.skill_points) + delta).min(point_cap).max(0) as u32;
        // `m_levelUp` is the next rank's points; `rank_level + 1 < level_cap` is "the next rank is allowed".
        while st.rank_level + 1 < level_cap && st.rank_level + 1 < ranks.len() {
            let next = &ranks[st.rank_level + 1];
            if next.skill_points_needed < 0 || (st.skill_points as i32) < next.skill_points_needed {
                break;
            }
            st.rank_level += 1;
            st.science_purchase_points += next.science_purchase_points_granted;
            for sci in &next.sciences_granted {
                st.sciences.insert(sci.clone());
            }
        }
    }

    /// Install the `Science` / `Rank` tables (set by the adapter).
    pub fn set_science_tables(
        &mut self,
        sciences: std::collections::BTreeMap<String, crate::science::ScienceInfo>,
        ranks: Vec<crate::science::RankInfo>,
    ) {
        self.science_templates = sciences;
        self.rank_info = ranks;
    }

    /// `Money::deposit`.
    pub(crate) fn deposit_cash(&mut self, player: PlayerIndex, amount: u32) {
        if let Some(p) = self.players.get_mut(&player) {
            p.deposit(amount);
        }
    }

    /// `Money::withdraw`, clamped to what the player has; returns the amount taken.
    pub(crate) fn withdraw_cash(&mut self, player: PlayerIndex, amount: u32) -> u32 {
        let Some(p) = self.players.get_mut(&player) else { return 0 };
        let taken = p.money.min(amount);
        p.withdraw(taken);
        taken
    }
}
