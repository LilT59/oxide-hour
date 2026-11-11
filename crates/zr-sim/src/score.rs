//! `ScoreKeeper` (`GameEngine/Source/Common/RTS/ScoreKeeper.cpp`, GeneralsMD): one per player
//! (`Player::getScoreKeeper`, `Player.h:628`), the counters behind the skirmish score screen.
//! `ScoreScreen.cpp` sorts the rows by `calculateScore` (`grabMultiPlayerInfo`) and fills the stat
//! columns from the getters (`populatePlayerInfo`); see [`GameLogic::score_keeper`].
//!
//! Observational only: nothing in the simulation reads these counters, so they cannot change a
//! game. The C++ `ScoreKeeper::crc` is empty, so the scores are not in the CRC (`crc.rs`); its
//! `xfer` writes them into save games, which the port does not have yet.
//!
//! `TheGameLogic->isScoringEnabled()` is TRUE for the whole game: `GameLogic.cpp:466` sets it and
//! nothing in GeneralsMD calls `enableScoring`, so the hooks do not check it.
//!
//! Counts keyed by template use the template name (C++ keys by `ThingTemplate *`). The C++
//! `getTotalObjectsBuilt` counts `isEquivalentTo` matches; the port uses an exact name match.
//! Money is `u32` here (the port's `Money`), so the counters are `u32` with saturating adds.

use std::collections::BTreeMap;

use crate::damage::Relationship;
use crate::logic::GameLogic;
use crate::object::{ObjectId, PlayerIndex};
use crate::template::{collide_kind, kind_of, ObjectTemplate};

/// `ScoreKeeper`: the per-player statistics for one player.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScoreKeeper {
    /// `m_myPlayerIdx`: `calculateScore` does not count kills of this player's own units.
    my_player: PlayerIndex,
    total_money_earned: u32,
    total_money_spent: u32,
    /// `m_totalUnitsDestroyed[playerIdx]`: keyed by the victim's controlling player.
    units_destroyed: BTreeMap<PlayerIndex, u32>,
    units_built: u32,
    units_lost: u32,
    /// `m_totalBuildingsDestroyed[playerIdx]`: keyed by the victim's controlling player.
    buildings_destroyed: BTreeMap<PlayerIndex, u32>,
    buildings_built: u32,
    buildings_lost: u32,
    tech_buildings_captured: u32,
    faction_buildings_captured: u32,
    /// `m_objectsBuilt` (template name -> count).
    objects_built: BTreeMap<String, u32>,
    /// `m_objectsDestroyed[playerIdx]`.
    objects_destroyed: BTreeMap<PlayerIndex, BTreeMap<String, u32>>,
    objects_lost: BTreeMap<String, u32>,
    objects_captured: BTreeMap<String, u32>,
}

/// `ScoreKeeper::reset(playerIdx)`: a fresh keeper for `my_player` (`Player::init`, Player.cpp:477).
impl ScoreKeeper {
    pub fn new(my_player: PlayerIndex) -> Self {
        ScoreKeeper { my_player, ..Default::default() }
    }

    /// `getTotalMoneyEarned`.
    pub fn total_money_earned(&self) -> u32 {
        self.total_money_earned
    }

    /// `getTotalMoneySpent`.
    pub fn total_money_spent(&self) -> u32 {
        self.total_money_spent
    }

    /// `getTotalUnitsDestroyed`: over every player, the own player included (the C++ design change).
    pub fn total_units_destroyed(&self) -> u32 {
        self.units_destroyed.values().fold(0, |a, b| a.saturating_add(*b))
    }

    /// `getTotalUnitsBuilt`.
    pub fn total_units_built(&self) -> u32 {
        self.units_built
    }

    /// `getTotalUnitsLost`.
    pub fn total_units_lost(&self) -> u32 {
        self.units_lost
    }

    /// `getTotalBuildingsDestroyed`: over every player, the own player included.
    pub fn total_buildings_destroyed(&self) -> u32 {
        self.buildings_destroyed.values().fold(0, |a, b| a.saturating_add(*b))
    }

    /// `getTotalBuildingsBuilt`.
    pub fn total_buildings_built(&self) -> u32 {
        self.buildings_built
    }

    /// `getTotalBuildingsLost`.
    pub fn total_buildings_lost(&self) -> u32 {
        self.buildings_lost
    }

    /// `getTotalTechBuildingsCaptured`.
    pub fn total_tech_buildings_captured(&self) -> u32 {
        self.tech_buildings_captured
    }

    /// `getTotalFactionBuildingsCaptured`.
    pub fn total_faction_buildings_captured(&self) -> u32 {
        self.faction_buildings_captured
    }

    /// `getTotalObjectsBuilt(pTemplate)`: how many objects of this template were built.
    pub fn total_objects_built(&self, template: &str) -> u32 {
        self.objects_built.get(template).copied().unwrap_or(0)
    }

    /// `calculateScore`: 100 per unit built, 100 per building built, the money earned, and 100 per
    /// enemy unit or building destroyed (kills of the own player do not count).
    pub fn calculate_score(&self) -> u64 {
        let mut score = self.units_built as u64 * 100;
        score += self.total_money_earned as u64;
        score += self.buildings_built as u64 * 100;
        for (p, n) in &self.units_destroyed {
            if *p != self.my_player {
                score += *n as u64 * 100;
            }
        }
        for (p, n) in &self.buildings_destroyed {
            if *p != self.my_player {
                score += *n as u64 * 100;
            }
        }
        score
    }

    /// `addMoneySpent`.
    pub(crate) fn add_money_spent(&mut self, money: u32) {
        self.total_money_spent = self.total_money_spent.saturating_add(money);
    }

    /// `addMoneyEarned`.
    pub(crate) fn add_money_earned(&mut self, money: u32) {
        self.total_money_earned = self.total_money_earned.saturating_add(money);
    }

    /// `addObjectBuilt`: a scoring building (`STRUCTURE` with `SCORE` or `SCORE_CREATE`), or a scoring
    /// infantry or vehicle (`INFANTRY`/`VEHICLE` with `SCORE` or `SCORE_CREATE`).
    pub(crate) fn add_object_built(&mut self, t: &ObjectTemplate) {
        let counted = if is_scoring_building(t) || is_scoring_building_create(t) {
            self.buildings_built = self.buildings_built.saturating_add(1);
            true
        } else if is_unit(t) && (t.is_kind_of(kind_of::SCORE) || t.is_collide_kind_of(collide_kind::SCORE_CREATE)) {
            self.units_built = self.units_built.saturating_add(1);
            true
        } else {
            false
        };
        if counted {
            bump(&mut self.objects_built, &t.name, 1);
        }
    }

    /// `removeObjectBuilt` (`ScoreKeeper.cpp:181-215`), called by `Player::onStructureUndone`
    /// (Player.cpp:1719). The C++ also leaves the per-template count at -1 when it was not counted;
    /// its `Int` map allows that, the port's `u32` saturates at 0.
    pub(crate) fn remove_object_built(&mut self, t: &ObjectTemplate) {
        let counted = if is_scoring_building(t) || is_scoring_building_create(t) {
            self.buildings_built = self.buildings_built.saturating_sub(1);
            true
        } else if is_unit(t) && (t.is_kind_of(kind_of::SCORE) || t.is_collide_kind_of(collide_kind::SCORE_CREATE)) {
            self.units_built = self.units_built.saturating_sub(1);
            true
        } else {
            false
        };
        if counted {
            let e = self.objects_built.entry(t.name.clone()).or_default();
            *e = e.saturating_sub(1);
        }
    }

    /// `addObjectDestroyed`, called on the killer's keeper. `victim_owner` is the victim's controlling
    /// player, which the C++ indexes the counts by. Buildings and units still under construction do
    /// not count.
    pub(crate) fn add_object_destroyed(&mut self, victim_owner: PlayerIndex, t: &ObjectTemplate, under_construction: bool) {
        let counted = if is_scoring_building(t) || is_scoring_building_destroy(t) {
            if !under_construction {
                bump_player(&mut self.buildings_destroyed, victim_owner);
                true
            } else {
                false
            }
        } else if is_unit(t) && (t.is_kind_of(kind_of::SCORE) || t.is_collide_kind_of(collide_kind::SCORE_DESTROY)) {
            if !under_construction {
                bump_player(&mut self.units_destroyed, victim_owner);
                true
            } else {
                false
            }
        } else {
            false
        };
        if counted {
            bump(self.objects_destroyed.entry(victim_owner).or_default(), &t.name, 1);
        }
    }

    /// `addObjectLost`, called on the victim's controlling player's keeper.
    pub(crate) fn add_object_lost(&mut self, t: &ObjectTemplate, under_construction: bool) {
        let counted = if is_scoring_building(t) || is_scoring_building_destroy(t) {
            if !under_construction {
                self.buildings_lost = self.buildings_lost.saturating_add(1);
                true
            } else {
                false
            }
        } else if is_unit(t) && (t.is_kind_of(kind_of::SCORE) || t.is_collide_kind_of(collide_kind::SCORE_DESTROY)) {
            if !under_construction {
                self.units_lost = self.units_lost.saturating_add(1);
                true
            } else {
                false
            }
        } else {
            false
        };
        if counted {
            bump(&mut self.objects_lost, &t.name, 1);
        }
    }

    /// `addObjectCaptured`: any `STRUCTURE`, as faction (`SCORE`) or tech (otherwise) building.
    pub(crate) fn add_object_captured(&mut self, t: &ObjectTemplate) {
        if !t.is_kind_of(kind_of::STRUCTURE) {
            return;
        }
        if t.is_kind_of(kind_of::SCORE) {
            self.faction_buildings_captured = self.faction_buildings_captured.saturating_add(1);
        } else {
            self.tech_buildings_captured = self.tech_buildings_captured.saturating_add(1);
        }
        bump(&mut self.objects_captured, &t.name, 1);
    }
}

/// `scoringBuildingMask` (`STRUCTURE` and `SCORE`, all set, nothing cleared).
fn is_scoring_building(t: &ObjectTemplate) -> bool {
    t.is_kind_of(kind_of::STRUCTURE | kind_of::SCORE)
}

/// `scoringBuildingCreateMask` (`STRUCTURE` and `SCORE_CREATE`).
fn is_scoring_building_create(t: &ObjectTemplate) -> bool {
    t.is_kind_of(kind_of::STRUCTURE) && t.is_collide_kind_of(collide_kind::SCORE_CREATE)
}

/// `scoringBuildingDestroyMask` (`STRUCTURE` and `SCORE_DESTROY`).
fn is_scoring_building_destroy(t: &ObjectTemplate) -> bool {
    t.is_kind_of(kind_of::STRUCTURE) && t.is_collide_kind_of(collide_kind::SCORE_DESTROY)
}

/// `KINDOF_INFANTRY || KINDOF_VEHICLE`.
fn is_unit(t: &ObjectTemplate) -> bool {
    t.is_kind_of(kind_of::INFANTRY) || t.is_kind_of(kind_of::VEHICLE)
}

fn bump(map: &mut BTreeMap<String, u32>, key: &str, by: u32) {
    let e = map.entry(key.to_string()).or_insert(0);
    *e = e.saturating_add(by);
}

fn bump_player(map: &mut BTreeMap<PlayerIndex, u32>, player: PlayerIndex) {
    let e = map.entry(player).or_insert(0);
    *e = e.saturating_add(1);
}

/// The `Player::onUnitCreated` / `onStructureConstructionComplete` / `Object::scoreTheKill` /
/// `Object::onCapture` / `Player::addMoneyEarned` call sites, as `GameLogic` methods. Each takes
/// the object (or player) the C++ call has in hand and looks up the keeper of the player C++ calls.
impl GameLogic {
    /// `Player::getScoreKeeper()` for `player` (`ScoreScreen.cpp` reads the keepers at the end of a game).
    pub fn score_keeper(&self, player: PlayerIndex) -> Option<&ScoreKeeper> {
        self.players.get(&player).map(|p| &p.score)
    }

    /// `GameLogic::isScoringEnabled`.
    pub fn is_scoring_enabled(&self) -> bool {
        self.scoring_enabled
    }

    /// `GameLogic::enableScoring` (`ENABLE_SCORING` / `DISABLE_SCORING`, ScriptActions.cpp:2890). While disabled the
    /// keepers do not count built, lost, destroyed or captured objects; money and the kill rewards still apply.
    pub fn set_scoring_enabled(&mut self, enabled: bool) {
        self.scoring_enabled = enabled;
    }

    fn score_mut(&mut self, player: PlayerIndex) -> Option<&mut ScoreKeeper> {
        self.players.get_mut(&player).map(|p| &mut p.score)
    }

    /// `Player::onUnitCreated` -> `m_scoreKeeper.addObjectBuilt(unit)` (Player.cpp:1557). Called for
    /// every unit the factories and spawn modules finish.
    pub fn score_unit_created(&mut self, unit: ObjectId) {
        // `ScoreKeeper::addObjectBuilt` returns early when scoring is disabled (ScoreKeeper.cpp:122).
        if !self.scoring_enabled {
            return;
        }
        let Some((owner, template)) = self.objects.get(&unit).map(|o| (o.player, o.template.clone())) else { return };
        if let Some(s) = self.score_mut(owner) {
            s.add_object_built(&template);
        }
    }

    /// `Player::onStructureUndone(structure)` (Player.cpp:1719): `m_scoreKeeper.removeObjectBuilt`.
    /// `AISkirmishPlayer::adjustBuildList` calls it before destroying the placed command center.
    pub fn score_structure_undone(&mut self, structure: ObjectId) {
        // `removeObjectBuilt` returns early when scoring is disabled (ScoreKeeper.cpp:183).
        if !self.scoring_enabled {
            return;
        }
        let Some((owner, template)) = self.objects.get(&structure).map(|o| (o.player, o.template.clone())) else { return };
        if let Some(s) = self.score_mut(owner) {
            s.remove_object_built(&template);
        }
    }

    /// `Player::onStructureConstructionComplete(builder, structure, isRebuild = FALSE)`: `addObjectBuilt`
    /// and `addMoneySpent(calcCostToBuild)` (Player.cpp:1644-1646). The port has no rebuild flag yet
    /// (`DozerAIUpdate::getIsRebuild`), so every completion counts.
    pub fn score_structure_completed(&mut self, structure: ObjectId) {
        let Some((owner, template)) = self.objects.get(&structure).map(|o| (o.player, o.template.clone())) else { return };
        // `addMoneySpent` has no scoring-enabled check (inline in ScoreKeeper.h), only `addObjectBuilt` does.
        let enabled = self.scoring_enabled;
        // `Player::onStructureConstructionComplete` (`Player.cpp:1646`) scores `calcCostToBuild`.
        let cost = self.calc_cost_to_build(owner, &template);
        if let Some(s) = self.score_mut(owner) {
            if enabled {
                s.add_object_built(&template);
            }
            s.add_money_spent(cost);
        }
    }

    /// `Object::scoreTheKill(victim)` (Object.cpp:2959), called by the killer: the victim's controller
    /// is `addObjectLost` first, then a killer's kill of an enemy (not its own) is `addObjectDestroyed`.
    /// Like the C++, this only runs when there is a killer, so kills without one are not counted.
    pub(crate) fn score_the_kill(&mut self, victim: ObjectId, killer: ObjectId) {
        let (Some(v), Some(k)) = (self.objects.get(&victim), self.objects.get(&killer)) else { return };
        let (victim_owner, template, under_construction) = (v.player, v.template.clone(), v.under_construction);
        let killer_owner = k.player;
        // `victimController->isPlayableSide()`: the neutral (civilian) side is not scored, and neither is
        // `KINDOF_IGNORED_IN_GUI`.
        if Some(victim_owner) == self.neutral_player() || template.ignored_in_gui {
            return;
        }
        // `addObjectLost` / `addObjectDestroyed` return early when scoring is disabled (ScoreKeeper.cpp:296, :252).
        let enabled = self.scoring_enabled;
        if enabled {
            if let Some(s) = self.score_mut(victim_owner) {
                s.add_object_lost(&template, under_construction);
            }
        }
        if self.relationship(killer_owner, victim_owner) != Relationship::Enemies || killer_owner == victim_owner {
            return;
        }
        if enabled {
            if let Some(s) = self.score_mut(killer_owner) {
                s.add_object_destroyed(victim_owner, &template, under_construction);
            }
        }
    }

    /// `Object::scoreTheKill` in full (Object.cpp:2959): the score keeper's lost and destroyed counts, then the
    /// killer's owner's skill points and cash bounty and the killer's experience. C++ kill sites that do
    /// `killer->scoreTheKill(victim); victim->kill()` (AIStates.cpp:452, Chinook `killEnemiesInContainer`) call this.
    pub(crate) fn object_score_the_kill(&mut self, victim: ObjectId, killer: ObjectId) {
        self.score_the_kill(victim, killer);
        self.grant_kill_experience(victim, Some(killer));
    }

    /// `Object::onCapture` -> `newOwner->getScoreKeeper()->addObjectCaptured(this)` (Object.cpp:4593).
    pub(crate) fn score_object_captured(&mut self, object: ObjectId) {
        // `addObjectCaptured` returns early when scoring is disabled (ScoreKeeper.cpp:219).
        if !self.scoring_enabled {
            return;
        }
        let Some((owner, template)) = self.objects.get(&object).map(|o| (o.player, o.template.clone())) else { return };
        if let Some(s) = self.score_mut(owner) {
            s.add_object_captured(&template);
        }
    }

    /// `Player::addMoneyEarned` on `player`'s keeper (the call sites name the C++ caller).
    pub(crate) fn score_money_earned(&mut self, player: PlayerIndex, amount: u32) {
        if let Some(s) = self.score_mut(player) {
            s.add_money_earned(amount);
        }
    }

    /// `Player::getScoreKeeper()->addMoneySpent` on `player`'s keeper.
    pub(crate) fn score_money_spent(&mut self, player: PlayerIndex, amount: u32) {
        if let Some(s) = self.score_mut(player) {
            s.add_money_spent(amount);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::body::DamageInfo;
    use crate::damage::{DamageType, DeathType};
    use crate::math::Coord3D;
    use crate::production::ProductionTemplate;
    use crate::template::{LocomotorKind, LocomotorParams, SimpleTemplateStore};
    use crate::terrain::TerrainLogic;

    /// A synthetic template (no game data): `kind` and a build cost, 500 hp.
    fn unit(name: &str, kind: u64, cost: i32) -> ObjectTemplate {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind;
        t.build_cost = cost;
        t.max_health = 500.0;
        t.initial_health = 500.0;
        t
    }

    fn game() -> GameLogic {
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        g.add_player(0, 10_000);
        g.add_player(1, 10_000);
        g.set_enemies(0, 1);
        g
    }

    /// Building, killing and spending: a barracks the player's dozer builds, two enemy kills by the
    /// player's tank (a rifle and a barracks, each paying the 50% cash bounty), and a dozer kill that
    /// is not a scoring kind.
    #[test]
    fn building_killing_and_spending_update_the_scorekeeper() {
        let mut g = game();
        let mut store = SimpleTemplateStore::default();
        let mut dozer = unit("Dozer", kind_of::VEHICLE | kind_of::DOZER, 500);
        // `DozerAIUpdate::construct` needs a locomotor (`construct` refuses a builder without one).
        dozer.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        // The dozer machine is the `DozerAIUpdate` module: a template without its data has no dozer
        // behaviour in the engine (and no construction advance).
        dozer.dozer = Some(crate::dozer::DozerTemplate::default());
        let dozer_t = store.add(dozer);
        let barracks_t = store.add(unit("Barracks", kind_of::STRUCTURE | kind_of::SCORE, 500));
        let tank_t = store.add(unit("Tank", kind_of::VEHICLE | kind_of::SCORE, 800));
        let rifle_t = store.add(unit("Rifle", kind_of::INFANTRY | kind_of::SCORE, 100));
        g.set_template_source(Arc::new(store));

        let dozer = g.create_object(&dozer_t, Coord3D::new(20.0, 20.0, 0.0), 0.0, 0);
        let site = g.construct(dozer, "Barracks", Coord3D::new(60.0, 60.0, 0.0), 0.0).expect("a legal site");
        for _ in 0..3000 {
            if !g.find_object(site).is_some_and(|o| o.under_construction) {
                break;
            }
            g.run_frames(1);
        }
        assert!(!g.find_object(site).unwrap().under_construction, "the barracks finished");
        {
            // `Player::onStructureConstructionComplete`: the finished barracks counts, at its build cost.
            let k = g.score_keeper(0).unwrap();
            assert_eq!(k.total_buildings_built(), 1);
            assert_eq!(k.total_objects_built("Barracks"), 1);
            assert_eq!(k.total_money_spent(), 500);
        }

        // A 50% cash bounty pays the killer's owner half the victim's cost (`doBountyForKill`).
        g.set_player_cash_bounty(0, 0.5);
        let tank = g.create_object(&tank_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let rifle = g.create_object(&rifle_t, Coord3D::new(100.0, 110.0, 0.0), 0.0, 1);
        let enemy_barracks = g.create_object(&barracks_t, Coord3D::new(150.0, 150.0, 0.0), 0.0, 1);
        let dozer_enemy = g.create_object(&dozer_t, Coord3D::new(110.0, 100.0, 0.0), 0.0, 1);
        let hit = |g: &mut GameLogic, victim: ObjectId| {
            g.apply_damage(victim, DamageInfo { source: tank, amount: 5000.0, kind: DamageType::Unresistable }, DeathType::Normal);
        };
        hit(&mut g, rifle);
        hit(&mut g, enemy_barracks);
        hit(&mut g, dozer_enemy);

        let mine = g.score_keeper(0).unwrap();
        assert_eq!(mine.total_units_destroyed(), 1, "the rifle; the dozer is not a scoring kind");
        assert_eq!(mine.total_buildings_destroyed(), 1, "the barracks");
        // The bounty is paid for any enemy kind (`doBountyForKill`), the dozer's included.
        assert_eq!(mine.total_money_earned(), 50 + 250 + 250, "the three bounties");
        // 100 (barracks built) + 100 (rifle) + 100 (barracks) + money earned 550.
        assert_eq!(mine.calculate_score(), 850);
        let theirs = g.score_keeper(1).unwrap();
        assert_eq!(theirs.total_units_lost(), 1, "the rifle");
        assert_eq!(theirs.total_buildings_lost(), 1, "the barracks");
        assert_eq!(theirs.total_buildings_built(), 0);
        assert_eq!(theirs.total_money_earned(), 0);
    }

    /// `Player::onUnitCreated`: a factory's finished scoring unit is counted, and a unit's cost is
    /// not money spent (only buildings and upgrades are, `ProductionUpdate.cpp:855`).
    #[test]
    fn a_factory_unit_counts_as_built_without_spending() {
        let mut g = game();
        let mut store = SimpleTemplateStore::default();
        let mut factory = unit("Factory", kind_of::STRUCTURE | kind_of::SCORE, 1000);
        factory.production = Some(ProductionTemplate::default());
        let factory_t = store.add(factory);
        store.add(unit("Tank", kind_of::VEHICLE | kind_of::SCORE, 800));
        g.set_template_source(Arc::new(store));

        let f = g.create_object(&factory_t, Coord3D::new(20.0, 20.0, 0.0), 0.0, 0);
        assert!(g.queue_unit(0, f, "Tank"));
        for _ in 0..3000 {
            if g.score_keeper(0).unwrap().total_units_built() > 0 {
                break;
            }
            g.run_frames(1);
        }
        let k = g.score_keeper(0).unwrap();
        assert_eq!(k.total_units_built(), 1);
        assert_eq!(k.total_money_spent(), 0);
    }

    /// `Object::scoreTheKill` (Object.cpp:2959) and its helpers: the bounty, the skill points and the
    /// experience go only to a kill of an enemy on a playable side, not `IGNORED_IN_GUI`, and not of a
    /// victim under construction (Player.cpp:2466 and :2531, Object.cpp:3001).
    #[test]
    fn kill_rewards_need_an_enemy_playable_finished_victim() {
        let mut g = game();
        g.add_player(2, 0); // an ally of player 0
        g.add_player(9, 0); // the neutral side
        g.set_neutral_player(Some(9));
        g.set_relationship_symmetric(0, 2, Relationship::Allies);
        let mut tank = unit("Tank", kind_of::VEHICLE | kind_of::SCORE, 800);
        tank.is_trainable = true;
        let tank_t = Arc::new(tank);
        let mut rifle = unit("Rifle", kind_of::INFANTRY | kind_of::SCORE, 100);
        rifle.experience_value = [10, 20, 30, 40];
        rifle.skill_point_value = [3, 6, 9, 12];
        let rifle_t = Arc::new(rifle);
        g.set_player_cash_bounty(0, 0.5);
        let killer = g.create_object(&tank_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let kill = |g: &mut GameLogic, victim: ObjectId| {
            g.apply_damage(victim, DamageInfo { source: killer, amount: 5000.0, kind: DamageType::Unresistable }, DeathType::Normal);
        };
        let spawn = |g: &mut GameLogic, player: PlayerIndex| g.create_object(&rifle_t, Coord3D::new(100.0, 110.0, 0.0), 0.0, player);

        // An enemy's finished rifle pays the 50% bounty (50), 3 skill points (its SkillPointValue) and 10 experience.
        let money0 = g.player(0).unwrap().money;
        let skill0 = g.player_skill_points(0);
        let victim = spawn(&mut g, 1);
        kill(&mut g, victim);
        assert_eq!(g.player(0).unwrap().money, money0 + 50, "the bounty");
        assert_eq!(g.player_skill_points(0), skill0 + 3, "the skill points come from SkillPointValue, not ExperienceValue");
        assert_eq!(g.find_object(killer).unwrap().experience.current, 10, "the experience");

        // Under construction: no bounty, no skill points, no experience.
        let victim = spawn(&mut g, 1);
        g.objects.get_mut(&victim).unwrap().under_construction = true;
        kill(&mut g, victim);
        assert_eq!(g.player(0).unwrap().money, money0 + 50, "no bounty for a building site");
        assert_eq!(g.player_skill_points(0), skill0 + 3, "no skill points for a building site");
        assert_eq!(g.find_object(killer).unwrap().experience.current, 10, "no experience for a building site");

        // The neutral side is not a playable side: no reward (its relationship is always neutral).
        let victim = spawn(&mut g, 9);
        kill(&mut g, victim);
        assert_eq!(g.player(0).unwrap().money, money0 + 50, "no bounty for the neutral side");
        assert_eq!(g.find_object(killer).unwrap().experience.current, 10, "no experience for the neutral side");

        // An ally: no reward either.
        let victim = spawn(&mut g, 2);
        kill(&mut g, victim);
        assert_eq!(g.player(0).unwrap().money, money0 + 50, "no bounty for an ally");
        assert_eq!(g.find_object(killer).unwrap().experience.current, 10, "no experience for an ally");
    }

    /// The `SkillPointValue` fallback: `-999` (the default) is the experience value (ThingTemplate.cpp:1423).
    #[test]
    fn skill_point_value_falls_back_to_the_experience_value() {
        let mut t = ObjectTemplate::new("Rifle");
        t.experience_value = [10, 20, 30, 40];
        assert_eq!(t.skill_point_value_at(1), 20, "the default -999 uses ExperienceValue");
        t.skill_point_value = [3, 6, 9, 12];
        assert_eq!(t.skill_point_value_at(1), 6, "an explicit SkillPointValue is used as is");
    }

    fn rank_table() -> Vec<crate::science::RankInfo> {
        // Ranks 1, 2 and 3 (`RankInfoStore`; the engine's ranks are 1-based, the sim's `rank_info[0]` is rank 1).
        [0, 100, 300].iter().map(|&n| crate::science::RankInfo { skill_points_needed: n, ..Default::default() }).collect()
    }

    /// `Player::addSkillPoints` (Player.cpp:2500): the modifier scales the points rounded up, the total is capped at
    /// the points of the highest allowed rank, and promotion stops at that rank.
    #[test]
    fn skill_points_are_scaled_rounded_up_and_capped_by_the_rank_limit() {
        let mut g = game();
        g.rank_info = rank_table();
        g.set_skill_points_modifier(0, 0.5);
        g.add_skill_points(0, 3);
        assert_eq!(g.player_skill_points(0), 2, "ceil(0.5 * 3) = 2");
        g.set_skill_points_modifier(0, 1.0);
        g.add_skill_points(0, 10);
        assert_eq!(g.player_skill_points(0), 12);
        // Limit 2: the points stop at rank 2's 100, and rank 3 (300 points) is never reached.
        g.set_rank_level_limit(2);
        g.add_skill_points(0, 1000);
        assert_eq!(g.player_skill_points(0), 100, "capped at the points of the limit's rank");
        assert_eq!(g.sciences[&0].rank_level, 1, "rank 2 at most");
    }

    /// `GameLogic::enableScoring`: while scoring is off the keepers count no built, lost, destroyed or captured
    /// objects, but the money and the kill rewards still apply (`ScoreKeeper` gates only the `add*Object*` calls).
    #[test]
    fn disabled_scoring_counts_no_objects_but_keeps_the_rewards() {
        let mut g = game();
        let tank_t = Arc::new(unit("Tank", kind_of::VEHICLE | kind_of::SCORE, 800));
        let rifle_t = Arc::new(unit("Rifle", kind_of::INFANTRY | kind_of::SCORE, 100));
        g.set_player_cash_bounty(0, 0.5);
        let killer = g.create_object(&tank_t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let kill = |g: &mut GameLogic, victim: ObjectId| {
            g.apply_damage(victim, DamageInfo { source: killer, amount: 5000.0, kind: DamageType::Unresistable }, DeathType::Normal);
        };

        g.set_scoring_enabled(false);
        let built = g.create_object(&tank_t, Coord3D::new(20.0, 20.0, 0.0), 0.0, 0);
        g.score_unit_created(built);
        let victim = g.create_object(&rifle_t, Coord3D::new(100.0, 110.0, 0.0), 0.0, 1);
        kill(&mut g, victim);
        assert_eq!(g.score_keeper(0).unwrap().total_units_built(), 0, "no build count while disabled");
        assert_eq!(g.score_keeper(1).unwrap().total_units_lost(), 0, "no loss count while disabled");
        assert_eq!(g.score_keeper(0).unwrap().total_units_destroyed(), 0, "no kill count while disabled");
        assert_eq!(g.score_keeper(0).unwrap().total_money_earned(), 50, "the bounty is still paid and scored");

        g.set_scoring_enabled(true);
        let victim = g.create_object(&rifle_t, Coord3D::new(100.0, 110.0, 0.0), 0.0, 1);
        kill(&mut g, victim);
        assert_eq!(g.score_keeper(0).unwrap().total_units_destroyed(), 1, "counted again once enabled");
    }

    /// One-shot script with a single action, run for two frames (as in `command_button.rs`'s tests).
    fn run_action(g: &mut GameLogic, name: &str, params: Vec<zr_map::scripts::Parameter>) {
        use zr_map::scripts::{Action, Condition, OrCondition, Script, ScriptList};
        let script = Script {
            version: 2,
            name: "score".into(),
            comment: String::new(),
            condition_comment: String::new(),
            action_comment: String::new(),
            active: true,
            one_shot: true,
            easy: true,
            normal: true,
            hard: true,
            subroutine: false,
            delay_seconds: 0,
            or_conditions: vec![OrCondition {
                conditions: vec![Condition { version: 4, type_id: 0, internal_name: Some("CONDITION_TRUE".into()), params: Vec::new() }],
            }],
            actions: vec![Action { version: 2, type_id: 0, internal_name: Some(name.into()), params }],
            actions_false: Vec::new(),
        };
        g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![script], groups: Vec::new() })]);
        g.run_frames(2);
    }

    /// `ENABLE_SCORING` / `DISABLE_SCORING` (ScriptActions.cpp:6956), `PLAYER_SET_RANKLEVELLIMIT` (:7551) and
    /// `PLAYER_AFFECT_RECEIVING_EXPERIENCE` (:7667) reach the same state the C++ handlers set.
    #[test]
    fn script_actions_set_scoring_the_rank_limit_and_the_modifier() {
        use zr_map::scripts::{ParamValue, Parameter};
        let str_p = |s: &str| Parameter { kind: 0, value: ParamValue::Plain { int: 0, real: 0.0, string: s.to_owned() } };
        let int_p = |i: i32| Parameter { kind: 1, value: ParamValue::Plain { int: i, real: 0.0, string: String::new() } };
        let real_p = |r: f32| Parameter { kind: 2, value: ParamValue::Plain { int: 0, real: r, string: String::new() } };

        let mut g = game();
        g.scripts_mut().set_player_name("Alpha", 0);
        run_action(&mut g, "DISABLE_SCORING", Vec::new());
        assert!(!g.is_scoring_enabled(), "DISABLE_SCORING");
        run_action(&mut g, "ENABLE_SCORING", Vec::new());
        assert!(g.is_scoring_enabled(), "ENABLE_SCORING");
        run_action(&mut g, "PLAYER_SET_RANKLEVELLIMIT", vec![int_p(2)]);
        assert_eq!(g.rank_level_limit, 2, "PLAYER_SET_RANKLEVELLIMIT");
        run_action(&mut g, "PLAYER_AFFECT_RECEIVING_EXPERIENCE", vec![str_p("Alpha"), real_p(0.5)]);
        assert_eq!(g.skill_points_modifier(0), 0.5, "PLAYER_AFFECT_RECEIVING_EXPERIENCE");
    }
}
