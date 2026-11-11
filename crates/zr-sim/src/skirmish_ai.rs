//! Skirmish AI player (phase 1). Ported from `GameLogic/AI/AISkirmishPlayer.cpp`
//! (`processBaseBuilding`, `doTeamBuilding`, `adjustBuildList`) and `GameLogic/AI/AIPlayer.cpp`
//! (`update` schedule), reduced to the map/AIData build list plus simple gatherer and attack logic.
//!
//! The AI runs deterministically inside [`GameLogic::update`] on the logic frame, in player-index
//! order. It:
//! 1. processes its build list in order, dozer-constructing each missing structure (or reusing the
//!    already-placed command center), rebuilding per `Rebuilds` after `RebuildDelayTimeSeconds`,
//!    one structure per `StructureSeconds` (scaled by `Poor`/`Wealthy`), and prioritising power when
//!    underpowered;
//! 2. keeps `ResourceGatherersNormal` supply trucks in the field, queued from its supply center;
//! 3. every `TeamSeconds`, queues a small attack group from its war factory and sends it to the
//!    nearest enemy structure (a simplification of `AIPlayer::selectTeamToBuild`; the group's unit
//!    list comes from the map's skirmish teams, or a per-faction default).
//!
//! Phase-1 deviations from the original, all because the map/script layer isn't ported yet:
//! - The shipped `SkirmishBuildList`s mark most structures `AutomaticallyBuild = No` (retail map
//!   scripts enable them via `buildSpecificAIBuilding`), so the AI would otherwise only build the
//!   command center. Phase 1 builds the whole list in order.
//! - The shipped lists omit the supply center (scripts add it); the adapter inserts it after the
//!   command center so the war factory's `AnyOfObjects([<side>SupplyCenter])` prerequisite is met.
//!
//! Not ported: team prototypes/priorities, reinforcements, guard/retaliation, superweapons,
//! skill sets, base-defense placement, priority builds, and the `AIGroup` formation logic.

use std::collections::BTreeMap;

use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::{ObjectId, PlayerIndex};
use crate::template::{kind_of, ObjectTemplate};

/// One `BuildListInfo` entry, already adjusted to the player's base (`adjustBuildList`).
#[derive(Clone, Debug, PartialEq)]
pub struct AiBuildOrder {
    pub template: String,
    pub location: (f32, f32),
    pub angle: f32,
    pub initially_built: bool,
    pub rebuilds: i32,
    pub automatically_build: bool,
}

/// The AI player's configuration (the adapter builds it from `AIData` + the map's teams).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkirmishAiConfig {
    pub build_list: Vec<AiBuildOrder>,
    /// `ResourceGatherersNormal`.
    pub resource_gatherers: i32,
    /// `Player::setAIDifficulty` (`SLOT_EASY_AI` / `SLOT_MED_AI` / `SLOT_BRUTAL_AI`).
    pub difficulty: crate::script::Difficulty,
    /// The side's `BaseDefenseStructure1` (`SideInfo`), for `SKIRMISH_BUILD_BASE_DEFENSE_*`.
    pub base_defense: Option<String>,
    pub gatherer_template: String,
    pub gatherer_factory: String,
    /// Combat unit templates for one attack group.
    pub attack_units: Vec<String>,
    pub attack_factory: String,
    /// `StructureSeconds` / `TeamSeconds` / `RebuildDelayTimeSeconds`, in frames.
    pub structure_seconds: u32,
    pub team_seconds: u32,
    pub rebuild_delay_seconds: u32,
    pub resources_wealthy: i32,
    pub resources_poor: i32,
    pub structures_wealthy_mod: f32,
    pub structures_poor_mod: f32,
    /// `AIData` `TeamsWealthyRate` / `TeamsPoorRate` (`selectTeamToBuild`'s team-timer scaling).
    pub team_wealthy_mod: f32,
    pub team_poor_mod: f32,
    /// `AIData` `TeamResourcesToStart`: the multiplier on a team's cost estimate
    /// (`AIPlayer::isPossibleToBuildTeam`).
    pub team_resources_to_build: f32,
    /// `AIData` `SupplyCenterSafeRadius` (`AIPlayer::isLocationSafe`), default 250.
    pub supply_center_safe_radius: f32,
    /// `AIData` `MaxRecruitRadius` (`TAiData::m_maxRecruitDistance`, default 0): the distance
    /// `AIPlayer::queueUnits` passes to `Team::tryToRecruit`.
    pub max_recruit_distance: f32,
    /// `SideInfo`'s `SkillSet1..5` (`AI::parseSideInfo`): the sciences the AI buys on promotion
    /// (`AIPlayer::doUpgradesAndSkills`).
    pub skill_sets: Vec<Vec<String>>,
    /// `AIData` `SkirmishBaseDefenseExtraDistance` (`AI.h:199`): base defenses are pushed this far
    /// past the build-list bounding circle.
    pub base_defense_extra_distance: f32,
}

/// `TeamInQueue` (`AIPlayer.h`): a team being built / waiting to activate, or a reinforcement of an
/// existing team.
#[derive(Clone, Debug, PartialEq)]
pub struct TeamInQueue {
    /// Lower-case prototype name.
    pub key: String,
    /// `m_frameStarted`.
    pub frame_started: u32,
    /// `m_priorityBuild`.
    pub priority_build: bool,
    /// `m_reinforcement`: this entry tops up an existing active team instead of creating one.
    pub reinforcement: bool,
    /// `m_reinforcementID`: the unit recruited to reinforce with, if any.
    pub reinforcement_id: Option<ObjectId>,
    /// `m_sentToStartLocation`.
    pub sent_to_start_location: bool,
    /// `m_workOrders`.
    pub work_orders: Vec<WorkOrder>,
}

/// `WorkOrder` (`AIPlayer.h`): one unit type a queued team still needs.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkOrder {
    /// `m_thing` (template name).
    pub thing: String,
    /// `m_factoryID`: the factory currently building one, or `None`.
    pub factory: Option<ObjectId>,
    /// `m_numCompleted` / `m_numRequired`.
    pub completed: i32,
    pub required_count: i32,
    /// `m_required`: part of the team's minimum.
    pub required: bool,
    /// `m_isResourceGatherer` (`AIPlayer::queueSupplyTruck`, AIPlayer.cpp:410-416): the order is a
    /// supply truck (a GLA worker is both a dozer and a gatherer, so `includesADozer` skips these).
    pub resource_gatherer: bool,
}

impl WorkOrder {
    /// `WorkOrder::isWaitingToBuild`.
    pub fn is_waiting_to_build(&self) -> bool {
        self.factory.is_none() && self.completed < self.required_count
    }
}

impl TeamInQueue {
    /// The work orders `AIPlayer::buildSpecificAITeam` (`AIPlayer.cpp:2490-2527`) creates: optional
    /// (`max - min`) orders first, then the required (`min`) ones, each *prepended* to the list, so
    /// the list reads required slots (last to first) then optional slots (last to first).
    pub fn orders_for(proto: &crate::script::TeamPrototype) -> Vec<WorkOrder> {
        let mut orders: Vec<WorkOrder> = Vec::new();
        for u in &proto.units {
            let count = u.max - u.min;
            if count > 0 {
                orders.insert(0, WorkOrder { thing: u.template.clone(), factory: None, completed: 0, required_count: count, required: false, resource_gatherer: false });
            }
        }
        for u in &proto.units {
            orders.insert(0, WorkOrder { thing: u.template.clone(), factory: None, completed: 0, required_count: u.min, required: true, resource_gatherer: false });
        }
        orders
    }

    fn crc(&self, c: &mut crate::crc::Crc) {
        for w in &self.work_orders {
            c.add_str(&w.thing);
            c.add_u32(w.factory.map_or(u32::MAX, |id| id.0));
            c.add_u32(w.completed as u32);
            c.add_u32(w.required_count as u32);
            c.add_bool(w.required);
            c.add_bool(w.resource_gatherer);
        }
        c.add_str(&self.key);
        c.add_u32(self.frame_started);
        c.add_bool(self.priority_build);
        c.add_bool(self.reinforcement);
        c.add_u32(self.reinforcement_id.map_or(u32::MAX, |id| id.0));
        c.add_bool(self.sent_to_start_location);
    }
}

/// The supply fields of `BuildListInfo` (`SidesList.h`): `isSupplyBuilding`,
/// `getDesiredGatherers` / `getCurrentGatherers`. `object` is the building these were computed for
/// (`checkForSupplyCenter` re-runs for a rebuilt, new object).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SupplyInfo {
    pub is_supply: bool,
    pub desired: i32,
    pub current: i32,
    pub object: Option<ObjectId>,
}

/// One AI player's runtime state.
#[derive(Clone, Debug)]
pub struct SkirmishAi {
    pub config: SkirmishAiConfig,
    /// The object built for each build order (`None` = not built).
    pub built: Vec<Option<ObjectId>>,
    /// Frame a destroyed building was noticed, per order (for the rebuild delay).
    pub destroyed_at: Vec<u32>,
    pub structure_timer: u32,
    /// `AISkirmishPlayer::m_readyToBuildStructure` / `m_buildDelay`: the `doBaseBuilding` gate that
    /// only attempts a structure once the timer expires, and then only every 2 seconds.
    pub ready_to_build_structure: bool,
    pub build_delay: u32,
    pub team_timer: u32,
    /// `m_teamTimer`: frames until the AI may start another team (`TeamSeconds`).
    pub script_team_timer: u32,
    /// `m_readyToBuildTeam`: the timer expired, so a team may be selected now.
    pub ready_to_build_team: bool,
    /// `m_teamDelay`: the 2-second tick that runs `queueUnits`/`processTeamBuilding`.
    pub team_delay: u32,
    /// `TeamBuildQueue`: teams being built (inactive) or reinforced (active).
    pub team_build_queue: Vec<TeamInQueue>,
    /// `TeamReadyQueue`: fully-built teams waiting to activate once idle.
    pub team_ready_queue: Vec<TeamInQueue>,
    /// Units queued for the current attack group.
    pub attack_group: Vec<ObjectId>,
    /// Build orders a script marked with `buildSpecificAIBuilding` (`BuildListInfo::isPriorityBuild`).
    pub priority: std::collections::BTreeSet<usize>,
    /// Diagnostics (not hashed).
    pub structures_built: u32,
    pub attacks_launched: u32,
    pub teams_built: u32,
    /// Diagnostics: candidates seen / production-condition failures / affordability failures.
    pub team_candidates: u32,
    pub team_pc_fail: u32,
    pub team_aff_fail: u32,
    /// `AIPlayer::m_frameToCheckEnemy`: the next frame `getAiEnemy` recomputes the closest enemy.
    pub frame_to_check_enemy: u32,
    /// `AISkirmishPlayer::m_currentEnemy`: this AI's current enemy player.
    pub current_enemy: Option<PlayerIndex>,
    /// `BuildListInfo` supply fields (`isSupplyBuilding`, desired/current gatherers), per build order.
    pub supply: Vec<SupplyInfo>,
    /// `AIPlayer::m_curWarehouseID`: the warehouse `buildBySupplies` last built next to.
    pub cur_warehouse: Option<ObjectId>,
    /// `AIPlayer::m_baseCenter` / `m_baseRadius` (`computeCenterAndRadiusOfBase`), computed once at
    /// map start from the (adjusted) build list.
    pub base_center: (f32, f32),
    /// `AIPlayer::m_baseCenterSet` (`numBldg > 0`): `getBaseCenter` returns it (AIPlayer.h:189).
    pub base_center_set: bool,
    pub base_radius: f32,
    /// `AISkirmishPlayer::m_curFrontBaseDefense` / `m_curFlankBaseDefense` and the four pairs of
    /// running placement angles (`buildAIBaseDefenseStructure`).
    pub cur_front_base_defense: i32,
    pub cur_flank_base_defense: i32,
    pub cur_front_left_defense_angle: f32,
    pub cur_front_right_defense_angle: f32,
    pub cur_left_flank_left_defense_angle: f32,
    pub cur_left_flank_right_defense_angle: f32,
    pub cur_right_flank_left_defense_angle: f32,
    pub cur_right_flank_right_defense_angle: f32,
    /// `AIPlayer::m_skillsetSelector` (`INVALID_SKILLSET_SELECTION` = `None`): the random
    /// `SkillSet1..5` chosen when the first purchase point arrives.
    pub skillset_selector: Option<i32>,
    /// Diagnostics (not hashed): base defenses placed, special powers fired, sciences bought.
    pub base_defenses_built: u32,
    pub powers_fired: u32,
    pub sciences_bought: u32,
}

impl SkirmishAi {
    fn new(config: SkirmishAiConfig) -> Self {
        let n = config.build_list.len();
        SkirmishAi {
            config,
            built: vec![None; n],
            destroyed_at: vec![0; n],
            structure_timer: 2, // `AIPlayer::m_structureTimer(2)` (AIPlayer.cpp:77): not before frame 1.
            ready_to_build_structure: false,
            build_delay: 0,
            team_timer: 0,
            script_team_timer: 2, // `AIPlayer::m_teamTimer(2)`: don't start until frame 1.
            ready_to_build_team: false,
            team_delay: 0,
            team_build_queue: Vec::new(),
            team_ready_queue: Vec::new(),
            attack_group: Vec::new(),
            priority: std::collections::BTreeSet::new(),
            structures_built: 0,
            attacks_launched: 0,
            teams_built: 0,
            team_candidates: 0,
            team_pc_fail: 0,
            team_aff_fail: 0,
            frame_to_check_enemy: 0,
            current_enemy: None,
            supply: vec![SupplyInfo::default(); n],
            cur_warehouse: None,
            base_center: (0.0, 0.0),
            base_center_set: false,
            base_radius: 0.0,
            cur_front_base_defense: 0,
            cur_flank_base_defense: 0,
            cur_front_left_defense_angle: 0.0,
            cur_front_right_defense_angle: 0.0,
            cur_left_flank_left_defense_angle: 0.0,
            cur_left_flank_right_defense_angle: 0.0,
            cur_right_flank_left_defense_angle: 0.0,
            cur_right_flank_right_defense_angle: 0.0,
            skillset_selector: None,
            base_defenses_built: 0,
            powers_fired: 0,
            sciences_bought: 0,
        }
    }

    pub(crate) fn crc(&self, c: &mut crate::crc::Crc) {
        c.add_u32(self.structure_timer);
        c.add_bool(self.ready_to_build_structure);
        c.add_u32(self.build_delay);
        c.add_u32(self.team_timer);
        c.add_u32(self.script_team_timer);
        c.add_bool(self.ready_to_build_team);
        c.add_u32(self.team_delay);
        c.add_u32(self.structures_built);
        c.add_u32(self.attacks_launched);
        c.add_u32(self.team_build_queue.len() as u32);
        for q in &self.team_build_queue {
            q.crc(c);
        }
        c.add_u32(self.team_ready_queue.len() as u32);
        for q in &self.team_ready_queue {
            q.crc(c);
        }
        c.add_u32(self.priority.len() as u32);
        for i in &self.priority {
            c.add_u32(*i as u32);
        }
        for (i, b) in self.built.iter().enumerate() {
            c.add_u32(b.map_or(0xffff_ffff, |x| x.0));
            c.add_u32(self.destroyed_at[i]);
        }
        c.add_u32(self.attack_group.len() as u32);
        for id in &self.attack_group {
            c.add_u32(id.0);
        }
        c.add_u32(self.frame_to_check_enemy);
        c.add_u32(self.current_enemy.map_or(u32::MAX, u32::from));
        for s in &self.supply {
            c.add_bool(s.is_supply);
            c.add_u32(s.desired as u32);
            c.add_u32(s.current as u32);
            c.add_u32(s.object.map_or(u32::MAX, |x| x.0));
        }
        c.add_u32(self.cur_warehouse.map_or(u32::MAX, |x| x.0));
        c.add_f32(self.base_center.0);
        c.add_f32(self.base_center.1);
        c.add_f32(self.base_radius);
        c.add_u32(self.cur_front_base_defense as u32);
        c.add_u32(self.cur_flank_base_defense as u32);
        c.add_f32(self.cur_front_left_defense_angle);
        c.add_f32(self.cur_front_right_defense_angle);
        c.add_f32(self.cur_left_flank_left_defense_angle);
        c.add_f32(self.cur_left_flank_right_defense_angle);
        c.add_f32(self.cur_right_flank_left_defense_angle);
        c.add_f32(self.cur_right_flank_right_defense_angle);
        c.add_u32(self.skillset_selector.map_or(u32::MAX, |x| x as u32));
    }
}

impl GameLogic {
    /// Register `index` as an AI player with `config` (`AIPlayer` construction at skirmish start).
    pub fn add_ai_player(&mut self, index: PlayerIndex, config: SkirmishAiConfig) {
        self.ai_players.insert(index, SkirmishAi::new(config));
        self.ai_new_map(index);
    }

    /// `AISkirmishPlayer::newMap` (`AISkirmishPlayer.cpp:1093-1134`): the placed starting command center
    /// is destroyed (`adjustBuildList`, `:989-1090`) and every `InitiallyBuilt` build-list entry is
    /// built fresh with `AIPlayer::buildStructureNow` (`AIPlayer.cpp:452-...`), so the AI knows its
    /// factories before the first script runs (`findFactory` iterates the build list).
    fn ai_new_map(&mut self, player: PlayerIndex) {
        let Some(mut state) = self.ai_players.get(&player).cloned() else { return };
        // `AISkirmishPlayer::adjustBuildList` (`AISkirmishPlayer.cpp:996-1011`): take the player's
        // command center (the starting building), `onStructureUndone`, remove it from the pathfind
        // map and destroy it. Its id is spent; the build list's command center entry is rebuilt as a
        // new object, so the placed command center is not reused (it used to be here, which made every
        // later object id one lower per AI player than the original's). The loop walks
        // `TheGameLogic->getFirstObject()` and breaks at the first command center, and
        // `GameLogic::registerObject` *prepends* to `m_objList` (`GameLogic.cpp:4137`): the newest
        // command center of the player is the one destroyed, not the lowest id.
        let cc = self
            .objects
            .values()
            .rev()
            .find(|o| !o.destroyed && o.player == player && o.is_kind_of(kind_of::COMMANDCENTER))
            .map(|o| o.id);
        if let Some(cc) = cc {
            self.score_structure_undone(cc);
            self.destroy_object(cc);
        }
        let n = state.config.build_list.len();
        state.built.resize(n, None);
        state.destroyed_at.resize(n, 0);
        for i in 0..n {
            let order = state.config.build_list[i].clone();
            if !order.initially_built {
                continue;
            }
            let Some(t) = self.find_template(&order.template) else { continue };
            let pos = Coord3D::new(order.location.0, order.location.1, self.terrain.ground_height(order.location.0, order.location.1));
            // `BuildAssistant::buildObjectNow` (`BuildAssistant.cpp:321-430`) with no constructor
            // object: clear the removable objects under the footprint, move units out of it (the AI
            // gets to proceed even when they cannot all move, `:348-355`), create the object complete,
            // then `Player::onStructureConstructionComplete` + `AIPlayer::onStructureProduced`.
            self.clear_removable_for_construction(&t, &pos, order.angle);
            let _ = self.move_objects_for_construction(&t, &pos, order.angle, player);
            let id = self.create_object(&t, pos, order.angle, player);
            state.built[i] = Some(id);
            state.structures_built += 1;
            self.score_structure_completed(id);
            if let Some(ai) = self.ai_players.get_mut(&player) {
                *ai = state.clone();
            }
            self.ai_on_structure_produced(id);
        }
        // `AISkirmishPlayer::newMap`: "the initial build in the normal build list consumes a
        // rebuild, so add one" -- `incrementNumRebuilds` for every non-placed entry (unlimited is
        // left alone). Without this, a shipped `Rebuilds = 0` entry is never `isBuildable`.
        for order in state.config.build_list.iter_mut() {
            if !order.initially_built && order.rebuilds >= 0 {
                order.rebuilds += 1;
            }
        }
        // `AIPlayer::computeCenterAndRadiusOfBase` (AIPlayer.cpp:3111): the mean of the (adjusted)
        // build-list locations, and the largest `(|dx| + r*0.4, |dy| + r*0.4)` distance from it.
        let mut total = (0.0f32, 0.0f32);
        let mut count = 0;
        for order in &state.config.build_list {
            if self.find_template(&order.template).is_none() {
                continue;
            }
            total.0 += order.location.0;
            total.1 += order.location.1;
            count += 1;
        }
        if count > 0 {
            total.0 /= count as f32;
            total.1 /= count as f32;
        }
        state.base_center = total;
        // `m_baseCenterSet = numBldg > 0` (AIPlayer.cpp:3137).
        state.base_center_set = count > 0;
        let mut max_rad_sqr = 0.0f32;
        for order in &state.config.build_list {
            let Some(t) = self.find_template(&order.template) else { continue };
            let r = t.geometry.bounding_circle_radius() * 0.4;
            let dx = (order.location.0 - total.0).abs() + r;
            let dy = (order.location.1 - total.1).abs() + r;
            let rad_sqr = dx * dx + dy * dy;
            if rad_sqr > max_rad_sqr {
                max_rad_sqr = rad_sqr;
            }
        }
        state.base_radius = max_rad_sqr.sqrt();
        if let Some(ai) = self.ai_players.get_mut(&player) {
            *ai = state;
        }
    }

    pub fn is_ai_player(&self, index: PlayerIndex) -> bool {
        self.ai_players.contains_key(&index)
    }

    pub fn ai_player(&self, index: PlayerIndex) -> Option<&SkirmishAi> {
        self.ai_players.get(&index)
    }

    /// Diagnostics: `buildSpecificAIBuilding` template names and how often each was requested.
    pub fn ai_build_specific_counts(&self) -> &std::collections::BTreeMap<String, u64> {
        &self.ai_build_specific_counts
    }

    /// Diagnostics: chosen build orders whose construction could not start, by template.
    pub fn ai_structure_fail_counts(&self) -> &std::collections::BTreeMap<String, u64> {
        &self.ai_structure_fail_counts
    }

    /// `AIPlayer::update` for every AI player, in index order.
    pub(crate) fn update_ai_players(&mut self) {
        let players: Vec<PlayerIndex> = self.ai_players.keys().copied().collect();
        for p in players {
            self.update_ai_player(p);
        }
    }

    fn update_ai_player(&mut self, player: PlayerIndex) {
        let Some(ai) = self.ai_players.get(&player) else { return };
        let mut state = ai.clone();
        let config = state.config.clone();
        // 1. Structures (`AISkirmishPlayer::doBaseBuilding`).
        self.ai_process_structures(player, &config, &mut state);

        // 2. Gatherers: `AIPlayer::queueSupplyTruck` runs from `queueUnits` (the 2-second team tick,
        //    `ai_queue_units`), which also works the supply truck's team-queue entry; the non-script
        //    fallback path has no such tick, so use one here.
        if !self.scripts.script_driven && self.frame % (2 * crate::math::LOGICFRAMES_PER_SECOND) == 0 {
            self.ai_queue_units(player, &mut state);
        }

        // `AISkirmishPlayer::getAiEnemy` recomputes the closest enemy every 5 seconds; scripts read
        // it through `<This Player's Enemy>`.
        self.ai_acquire_enemy(player, &mut state);

        // 3. Attack groups: script teams when scripts drive the AI, else the phase-1 fallback. The
        //    fallbacks (whole-list build, attack group) are *only* for maps/tests without scripts.
        if self.scripts.script_driven {
            self.ai_update_team_building(player, &config, &mut state);
        } else {
            self.ai_process_teams(player, &config, &mut state);
        }

        // 4. `AIPlayer::doUpgradesAndSkills` (last in `AIPlayer::update`).
        self.ai_do_upgrades_and_skills(player, &config, &mut state);

        if let Some(ai) = self.ai_players.get_mut(&player) {
            *ai = state;
        }
    }

    /// `AIPlayer::doUpgradesAndSkills` (`AIPlayer.cpp:2968`): while the player has science purchase
    /// points, pick a `SkillSet1..5` once (random among the defined sets, `GameLogicRandomValue(0,
    /// limit)`), then buy every science in it that `isCapableOfPurchasingScience` accepts.
    fn ai_do_upgrades_and_skills(&mut self, player: PlayerIndex, config: &SkirmishAiConfig, state: &mut SkirmishAi) {
        if self.frame < 2 {
            return; // can't do updates on the first few frames
        }
        if self.player_science_purchase_points(player) == 0 {
            return;
        }
        if config.skill_sets.is_empty() {
            return;
        }
        if state.skillset_selector.is_none() {
            let mut limit = 0;
            // Designers sometimes only define skillset 1 & 2, or some such.
            if config.skill_sets.get(1).is_some_and(|s| !s.is_empty()) {
                limit = 1;
                if config.skill_sets.get(2).is_some_and(|s| !s.is_empty()) {
                    limit = 2;
                    if config.skill_sets.get(3).is_some_and(|s| !s.is_empty()) {
                        limit = 3;
                        if config.skill_sets.get(4).is_some_and(|s| !s.is_empty()) {
                            limit = 4;
                        }
                    }
                }
            }
            // `AISkirmishPlayer::isSkirmishAI` is always true for a sim AI player.
            state.skillset_selector = Some(self.rng_mut().value(0, limit));
        }
        let selector = state.skillset_selector.unwrap_or(0).max(0) as usize;
        let skills = config.skill_sets.get(selector).cloned().unwrap_or_default();
        for science in skills {
            if self.is_capable_of_purchasing_science(player, &science) && self.command_purchase_science(player, &science) {
                state.sciences_bought += 1;
            }
        }
    }

    /// `AISkirmishPlayer::acquireEnemy` (called from `getAiEnemy` every 5 s): pick the closest
    /// enemy player by structure-bounds centre, with the anti-ganging distance penalties. The
    /// result feeds `<This Player's Enemy>` / `getSkirmishEnemyPlayer`.
    fn ai_acquire_enemy(&mut self, player: PlayerIndex, state: &mut SkirmishAi) {
        let now = self.frame;
        if now < state.frame_to_check_enemy {
            return;
        }
        state.frame_to_check_enemy = now + 5 * crate::math::LOGICFRAMES_PER_SECOND;
        // Keep the current enemy while it still has units and a build facility.
        if let Some(e) = state.current_enemy {
            if self.player_has_units(e) && self.player_has_build_facility(e) {
                return;
            }
        }
        // `m_baseCenter` (AISkirmishPlayer.cpp:500-505): the distance is measured from the AI's base centre, the mean of its
        // build list, not from its command center.
        let base = state.base_center;
        let enemies: Vec<PlayerIndex> = self
            .players
            .keys()
            .copied()
            .filter(|q| {
                *q != player
                    && self.relationship(player, *q) == crate::damage::Relationship::Enemies
                    && self.player_has_objects(*q)
            })
            .collect();
        let mut best: Option<(f32, PlayerIndex)> = None;
        for q in &enemies {
            let center = self
                .player_structure_bounds_center(*q)
                .or_else(|| self.player_first_object_pos(*q))
                .unwrap_or(Coord3D::ZERO);
            let (dx, dy) = (center.x - base.0, center.y - base.1);
            let mut d = dx * dx + dy * dy;
            if !self.player_has_units(*q) || !self.player_has_build_facility(*q) {
                d = 1.0e12 * 0.5; // crippled enemies are low priority
            }
            for r in self.ai_players.keys().copied().filter(|r| r != q) {
                match self.ai_players.get(&r).and_then(|a| a.current_enemy) {
                    Some(e) if e == *q => d += 500.0 * 500.0,
                    Some(e) if e == player => d = (d - 25.0 * 25.0).max(0.0),
                    _ => {}
                }
            }
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, *q));
            }
        }
        if let Some((_, e)) = best {
            state.current_enemy = Some(e);
            self.scripts.set_player_enemy(player, e);
        }
    }

    /// `Player::hasAnyObjects`.
    fn player_has_objects(&self, p: PlayerIndex) -> bool {
        self.objects().any(|o| o.player == p && !o.destroyed)
    }

    /// `Player::hasAnyUnits` (reduced: a live mobile object).
    fn player_has_units(&self, p: PlayerIndex) -> bool {
        self.objects().any(|o| o.player == p && !o.destroyed && !o.is_kind_of(kind_of::STRUCTURE) && o.template.locomotor.is_some())
    }

    /// `Player::hasAnyBuildFacility` (reduced: a factory or dozer).
    fn player_has_build_facility(&self, p: PlayerIndex) -> bool {
        self.objects().any(|o| {
            o.player == p && !o.destroyed && (o.template.production.is_some() || o.is_kind_of(kind_of::DOZER))
        })
    }

    /// The centre of `p`'s structure bounding box (`getPlayerStructureBounds`).
    fn player_structure_bounds_center(&self, p: PlayerIndex) -> Option<Coord3D> {
        let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
        let mut any = false;
        for o in self.objects().filter(|o| o.player == p && !o.destroyed && o.is_kind_of(kind_of::STRUCTURE)) {
            lo = (lo.0.min(o.position.x), lo.1.min(o.position.y));
            hi = (hi.0.max(o.position.x), hi.1.max(o.position.y));
            any = true;
        }
        any.then(|| Coord3D::new((lo.0 + hi.0) * 0.5, (lo.1 + hi.1) * 0.5, 0.0))
    }

    fn player_first_object_pos(&self, p: PlayerIndex) -> Option<Coord3D> {
        self.objects().find(|o| o.player == p && !o.destroyed).map(|o| o.position)
    }

    /// `AIPlayer::doTeamBuilding` + `checkReadyTeams` + `checkQueuedTeams` for the script-driven
    /// path. Retail keeps every team it selected in a *build queue* and keeps producing/recruiting
    /// for it (`queueUnits`) until it is complete or expires; the team is only activated (and only
    /// then runs its generic scripts) once built. The old code activated a team the moment it was
    /// picked, so on Hard — where the wave production conditions are all true early — every team was
    /// created empty, `maxInstances` blocked it, and production stopped.
    fn ai_update_team_building(&mut self, player: PlayerIndex, config: &SkirmishAiConfig, state: &mut SkirmishAi) {
        // `AIPlayer::update` order: checkReadyTeams, checkQueuedTeams, doTeamBuilding.
        self.ai_check_ready_teams(state);
        self.ai_check_queued_teams(state);

        // `AISkirmishPlayer::doTeamBuilding`: team timer, then a 2-second tick.
        if !state.ready_to_build_team {
            if state.script_team_timer > 0 {
                state.script_team_timer -= 1;
            }
            if state.script_team_timer == 0 {
                state.ready_to_build_team = true;
                state.team_delay = 0;
            }
            if state.script_team_timer > 3 * crate::math::LOGICFRAMES_PER_SECOND {
                state.script_team_timer = 3 * crate::math::LOGICFRAMES_PER_SECOND;
            }
        }
        if state.team_delay > 0 {
            state.team_delay -= 1;
        }
        if state.team_delay == 0 {
            self.ai_queue_units(player, state); // `queueUnits`: update the queues.
            if state.ready_to_build_team && self.ai_select_team_to_build(player, config, state) {
                self.ai_queue_units(player, state);
            }
            state.team_delay = 2 * crate::math::LOGICFRAMES_PER_SECOND;
        }
    }

    /// `Team::tryToRecruit` for every unfilled slot of team `key` (`AIPlayer::queueUnits`,
    /// AIPlayer.cpp:2705-2730): the closest eligible unit within `MaxRecruitRadius` of the team's
    /// home (else the base centre) joins the team and is sent home. `Object::setTeam` moves it (and
    /// gives it the team's attitude); `aiMoveToPosition(&home, CMD_FROM_AI)` runs right here, as in
    /// the engine's `queueUnits` loop.
    fn ai_recruit_into(&mut self, player: PlayerIndex, state: &mut SkirmishAi, qi: usize) {
        // `getBaseCenter(&home)` (AIPlayer.cpp:2706): a team without a home location uses the AI's base centre, which
        // is the mean of the build list (`computeCenterAndRadiusOfBase`, AIPlayer.cpp:3088, :3111), not the command
        // centre's location.
        // `hasHome` (AIPlayer.cpp:2701-2707): a team with a home location uses it; otherwise `getBaseCenter(&home)`
        // gives `m_baseCenter` (zero when the AI has no buildings) and `hasHome` is `m_baseCenterSet`.
        let key = state.team_build_queue[qi].key.clone();
        let team_home = self.scripts.team_proto(&key).and_then(|p| self.scripts.waypoints.get(&p.home).copied());
        let has_home = team_home.is_some() || state.base_center_set;
        let home = team_home.unwrap_or(Coord3D::new(state.base_center.0, state.base_center.1, 0.0));
        let max_dist = state.config.max_recruit_distance;
        for oi in 0..state.team_build_queue[qi].work_orders.len() {
            while state.team_build_queue[qi].work_orders[oi].is_waiting_to_build() {
                let thing = state.team_build_queue[qi].work_orders[oi].thing.clone();
                let mut engine = std::mem::take(&mut self.scripts);
                let unit = engine.recruit_one(self, player, &key, &thing, home, max_dist);
                self.scripts = engine;
                let Some(unit) = unit else { break };
                state.team_build_queue[qi].work_orders[oi].completed += 1;
                if state.team_build_queue[qi].reinforcement {
                    state.team_build_queue[qi].reinforcement_id = Some(unit);
                }
                // `if (hasHome) ai->aiMoveToPosition(&home, CMD_FROM_AI); else ai->aiIdle(CMD_FROM_AI);`
                // (AIPlayer.cpp:2726-2731).
                if has_home {
                    self.ai_move_to_position_cmd(unit, home, crate::ai_states::CommandSource::Ai);
                } else {
                    self.ai_idle(unit, crate::ai_states::CommandSource::Ai);
                }
            }
        }
    }

    /// `AIPlayer::queueUnits` (`AIPlayer.cpp:2688-2740`): for every team in the build queue and each
    /// of its work orders, recruit an existing unit (`tryToRecruit`) while the order is waiting to
    /// build, then `startTraining` one unit for what is still missing (an idle factory only,
    /// unless the team is a priority build), else validate the order's factory.
    fn ai_queue_units(&mut self, player: PlayerIndex, state: &mut SkirmishAi) {
        // `AIPlayer::queueUnits` starts with `queueSupplyTruck` (`AIPlayer.cpp:2689`).
        self.ai_queue_supply_truck(player, state);
        for qi in 0..state.team_build_queue.len() {
            self.ai_recruit_into(player, state, qi);
            let busy_ok = state.team_build_queue[qi].priority_build;
            for oi in 0..state.team_build_queue[qi].work_orders.len() {
                let (waiting, thing) = {
                    let w = &state.team_build_queue[qi].work_orders[oi];
                    (w.is_waiting_to_build(), w.thing.clone())
                };
                if waiting {
                    // `startTraining`: `findFactory(thing, busyOK)` then `queueCreateUnit`.
                    if let Some(f) = self.ai_find_build_list_factory(state, &thing, busy_ok) {
                        if self.queue_unit(player, f, &thing) {
                            state.team_build_queue[qi].work_orders[oi].factory = Some(f);
                        }
                    }
                } else if let Some(f) = state.team_build_queue[qi].work_orders[oi].factory {
                    // `WorkOrder::validateFactory`.
                    if self.objects.get(&f).is_none_or(|o| o.player != player) {
                        state.team_build_queue[qi].work_orders[oi].factory = None;
                    }
                }
            }
        }
    }

    /// `AIPlayer::selectTeamToBuild`: the highest-priority `isAGoodIdeaToBuildTeam` prototype; a
    /// random pick among ties. Reinforcement of an existing `AutoReinforce` team is attempted first
    /// (`selectTeamToReinforce(hiPri)`). On success a `TeamInQueue` is put in the build queue.
    fn ai_select_team_to_build(&mut self, player: PlayerIndex, config: &SkirmishAiConfig, state: &mut SkirmishAi) -> bool {
        let mut hi_pri = -99999i32;
        let mut candidates: Vec<(i32, String)> = Vec::new();
        // Collect first: `evaluate_production_condition` needs `&mut self.scripts`.
        // `m_player->getPlayerTeams()` order: the prototypes in load (creation) order.
        let protos: Vec<(String, i32, i32, String)> = self
            .scripts
            .team_order
            .iter()
            .filter_map(|k| self.scripts.team_protos.get(k))
            .filter(|p| !p.units.is_empty() && self.scripts.team_owner(&p.name) == Some(player))
            .map(|p| (p.name.clone(), p.production_priority, p.max_instances, p.production_condition.clone()))
            .collect();
        for (name, pri, max_inst, cond) in protos {
            // `isAGoodIdeaToBuildTeam` (`AIPlayer.cpp:1521-1557`), in order:
            // 1. `TeamPrototype::evaluateProductionCondition` (`Team.cpp`): a prototype with no
            //    production-condition script is `m_productionConditionAlwaysFalse` ("No script
            //    defined"), so it is never a good idea to build by itself.
            let ok = if cond.is_empty() {
                false
            } else {
                let mut engine = std::mem::take(&mut self.scripts);
                let ok = engine.evaluate_production_condition(self, player, &cond);
                self.scripts = engine;
                ok
            };
            if !ok {
                state.team_pc_fail += 1;
                continue;
            }
            // 2. `countTeamInstances() >= maxInstances` -> max already built. Instances of one
            //    prototype coexist: each has its own `proto#id` key and `seq`, and scripts resolve a
            //    bare prototype name to the newest instance (`getTeamNamed`).
            if self.scripts.count_team_instances(&name) as i32 >= max_inst {
                continue;
            }
            // 3. A team of this prototype already in the build queue -> currently building one.
            let proto_key = name.to_ascii_lowercase();
            if state.team_build_queue.iter().any(|q| self.scripts.proto_of(&q.key) == proto_key) {
                continue;
            }
            state.team_candidates += 1;
            // 4. `isPossibleToBuildTeam` (money + factory/tech).
            if !self.ai_team_affordable(player, state, &name) {
                state.team_aff_fail += 1;
                continue;
            }
            if pri > hi_pri {
                hi_pri = pri;
            }
            candidates.push((pri, name));
        }
        // `selectTeamToReinforce(hiPri)` runs before a new team is chosen, even when no new team is
        // buildable.
        if self.ai_select_team_to_reinforce(player, state, hi_pri) {
            return true;
        }
        if hi_pri == -99999 {
            return false;
        }
        // `selectTeamToBuild`: pick a random team from the highest-priority set.
        let hi: Vec<&(i32, String)> = candidates.iter().filter(|(p, _)| *p == hi_pri).collect();
        // `GameLogicRandomValue(0, count-1)` draws even for one candidate (RETAIL_COMPATIBLE_CRC).
        let which = self.rng_mut().value(0, hi.len() as i32 - 1) as usize;
        let name = hi[which].1.clone();
        // `buildSpecificAITeam`: create the inactive team instance and put it in the build queue.
        let Some(key) = self.scripts.create_team(&name) else { return false };
        let work_orders = self.scripts.team_proto(&name).map(TeamInQueue::orders_for).unwrap_or_default();
        state.team_build_queue.push(TeamInQueue {
            key,
            frame_started: self.frame,
            priority_build: false,
            reinforcement: false,
            reinforcement_id: None,
            sent_to_start_location: false,
            work_orders,
        });
        state.ready_to_build_team = false;
        state.script_team_timer = self.ai_team_timer(config, player);
        true
    }

    /// `AIPlayer::selectTeamToReinforce`: find an `AutoReinforce` team of this player with a slot
    /// below its maximum and an available factory, and queue a one-unit reinforcement of it. The
    /// highest production priority above `min_priority` wins (C++ keeps raising `curPriority`).
    fn ai_select_team_to_reinforce(&mut self, player: PlayerIndex, state: &mut SkirmishAi, min_priority: i32) -> bool {
        let mut cur_priority = min_priority;
        let mut chosen: Option<(String, String)> = None; // (team key, unit template)
        let protos: Vec<(String, i32, bool)> = self
            .scripts
            .team_order
            .iter()
            .filter_map(|k| self.scripts.team_protos.get(k))
            .filter(|p| self.scripts.team_owner(&p.name) == Some(player))
            .map(|p| (p.name.clone(), p.production_priority, p.auto_reinforce))
            .collect();
        for (name, pri, auto) in protos {
            if !auto || pri <= cur_priority {
                continue;
            }
            let proto_key = name.to_ascii_lowercase();
            if state
                .team_build_queue
                .iter()
                .any(|q| self.scripts.proto_of(&q.key) == proto_key)
            {
                continue; // busy building/reinforcing one of these.
            }
            let Some(proto) = self.scripts.team_protos.get(&proto_key).cloned() else { continue };
            // `TeamPrototype::iterate_TeamInstanceList`: the first instance with units wins
            // (`Team::hasAnyUnits`, Team.cpp:1727).
            let instances = self.scripts.instances_of(&proto_key);
            for key in instances {
                let Some(t) = self.scripts.teams.get(&key) else { continue };
                if !t.has_any_units(self) {
                    continue;
                }
                for slot in &proto.units {
                    if slot.max < 1 {
                        continue;
                    }
                    if t.count_template(self, &slot.template) < slot.max
                        && self.ai_find_factory(player, "", &slot.template).is_some()
                    {
                        cur_priority = pri;
                        chosen = Some((key.clone(), slot.template.clone()));
                    }
                }
            }
        }
        let Some((key, thing)) = chosen else { return false };
        // The entry goes to the front of the queue with one required order for `thing`
        // (`AIPlayer.cpp:1620-1636`).
        let mut order = WorkOrder { thing: thing.clone(), factory: None, completed: 0, required_count: 1, required: true, resource_gatherer: false };
        let mut reinforcement_id = None;
        // `origin` = the first member's position (else the prototype's home); recruit an idle unit
        // now, else `startTraining(order, priorityBuild = false)`.
        let origin = self
            .scripts
            .teams
            .get(&key)
            .and_then(|t| t.members.first().copied())
            .and_then(|id| self.objects.get(&id).map(|o| o.position))
            .or_else(|| self.scripts.team_proto(&key).and_then(|p| self.scripts.waypoints.get(&p.home).copied()))
            .unwrap_or(Coord3D::ZERO);
        let mut engine = std::mem::take(&mut self.scripts);
        let unit = engine.recruit_one(self, player, &key, &thing, origin, state.config.max_recruit_distance);
        self.scripts = engine;
        if let Some(unit) = unit {
            order.completed = 1;
            reinforcement_id = Some(unit);
            // `AIPlayer.cpp:1700`: `ai->aiIdle(CMD_FROM_AI)` on the recruited unit (a re-idle that
            // enters `AI_IDLE` again even when it was already idle).
            self.ai_idle(unit, crate::ai_states::CommandSource::Ai);
        } else if let Some(f) = self.ai_find_build_list_factory(state, &thing, false) {
            if self.queue_unit(player, f, &thing) {
                order.factory = Some(f);
            }
        }
        state.team_build_queue.insert(
            0,
            TeamInQueue {
                key,
                frame_started: self.frame,
                priority_build: false,
                reinforcement: true,
                reinforcement_id,
                sent_to_start_location: false,
                work_orders: vec![order],
            },
        );
        state.team_delay = 0;
        true
    }

    /// `selectTeamToBuild`'s `m_teamTimer`: `TeamSeconds`, scaled by `TeamsPoorRate` /
    /// `TeamsWealthyRate` against the player's money.
    fn ai_team_timer(&self, config: &SkirmishAiConfig, player: PlayerIndex) -> u32 {
        let mut timer = config.team_seconds;
        let money = self.player(player).map_or(0, |p| p.money) as i32;
        if money < config.resources_poor && config.team_poor_mod > 0.0 {
            timer = (timer as f32 / config.team_poor_mod) as u32;
        } else if money > config.resources_wealthy && config.team_wealthy_mod > 0.0 {
            timer = (timer as f32 / config.team_wealthy_mod) as u32;
        }
        timer
    }

    /// `AIPlayer::checkQueuedTeams`: first expire teams that ran out of build time, then move
    /// fully-built teams to the ready queue and run the `m_executeActions` production-condition
    /// action for a queued team with an idle member.
    fn ai_check_queued_teams(&mut self, state: &mut SkirmishAi) {
        // Loop 1: expired teams (`TeamInQueue::isBuildTimeExpired`; 0 means unlimited).
        let mut i = 0;
        while i < state.team_build_queue.len() {
            let q = state.team_build_queue[i].clone();
            let idle_frames = self.scripts.team_proto(&q.key).map_or(0, |p| p.initial_idle_frames);
            if idle_frames >= 1 && self.frame > q.frame_started + idle_frames as u32 {
                if Self::ai_team_minimum_built(&q) {
                    // `areBuildsComplete`: a unit still in production keeps the entry queued.
                    if q.work_orders.iter().any(|w| w.factory.is_some()) {
                        i += 1;
                        continue;
                    }
                    state.team_build_queue.remove(i);
                    // `prependTo_TeamReadyQueue`.
                    state.team_ready_queue.insert(0, q);
                } else {
                    // `TeamInQueue::disband` (AIPlayer.cpp:3615): the members go to the default team and
                    // a non-singleton instance is deleted, then `clearTeamFlags` (skirmish). Retail does
                    // this for a reinforcement entry too: its `m_team` is the existing active team, which
                    // is disbanded with it (an expired, unfilled reinforcement dissolves the team).
                    state.team_build_queue.remove(i);
                    let mut engine = std::mem::take(&mut self.scripts);
                    engine.disband_team(self, &q.key);
                    self.scripts = engine;
                    self.scripts.clear_team_flags();
                }
                continue;
            }
            i += 1;
        }
        // Loop 2: all-built -> ready; else run the production-condition action for an idle member.
        let mut i = 0;
        while i < state.team_build_queue.len() {
            let q = state.team_build_queue[i].clone();
            if Self::ai_team_all_built(&q) {
                state.team_build_queue.remove(i);
                state.team_ready_queue.insert(0, q);
                continue;
            }
            // `AIPlayer::checkQueuedTeams` (AIPlayer.cpp:2921-2928): `m_executeActions` re-runs the
            // production-condition script's actions once a member is idle, giving the team its
            // `TEAM_GUARD_AREA` order as it is built. Only for teams under construction; a
            // reinforcement entry references an already-active team and must not re-guard it.
            if !q.reinforcement
                && self.scripts.team_proto(&q.key).is_some_and(|p| p.execute_actions)
                && self.ai_team_any_idle(&q.key)
            {
                let mut engine = std::mem::take(&mut self.scripts);
                engine.execute_team_production_action(self, &q.key);
                self.scripts = engine;
            }
            i += 1;
        }
    }

    /// `AIPlayer::checkReadyTeams`: activate a ready team once its members are idle (or after 60 s).
    /// A reinforcement entry needs no activation: its units already joined the existing active team.
    fn ai_check_ready_teams(&mut self, state: &mut SkirmishAi) {
        let mut i = 0;
        while i < state.team_ready_queue.len() {
            let q = state.team_ready_queue[i].clone();
            let time_expired = self.frame > q.frame_started + 60 * crate::math::LOGICFRAMES_PER_SECOND;
            // `AIPlayer::checkReadyTeams`: with `m_executeActions`, an `anyIdle` member is enough to
            // start the team (its production-condition action handles the rest, e.g. a guard order).
            let execute_actions = self.scripts.team_proto(&q.key).is_some_and(|p| p.execute_actions);
            let (mut all_idle, any_idle) = if q.reinforcement {
                // A reinforcement entry watches only the unit it adds (`m_reinforcementID`).
                match q.reinforcement_id.and_then(|id| self.find_object(id)) {
                    Some(o) => {
                        let idle = o.ai_handle.is_none() || self.ai_is_idle(o);
                        (idle, idle)
                    }
                    None => (true, false),
                }
            } else {
                (self.ai_team_all_idle(&q.key), self.ai_team_any_idle(&q.key))
            };
            if execute_actions && any_idle {
                all_idle = true;
            }
            let ready = all_idle;
            if ready || time_expired {
                state.team_ready_queue.remove(i);
                if !q.reinforcement {
                    let key = q.key.clone();
                    let mut engine = std::mem::take(&mut self.scripts);
                    // `AIPlayer::checkReadyTeams`: `team->setActive()`; the create script runs in the
                    // next frame's team pass (`Team::updateState`, `Team.cpp:1812-1819`).
                    engine.mark_team_active(&key);
                    self.scripts = engine;
                    // `if (isSkirmishAI()) TheScriptEngine->clearTeamFlags();` (AIPlayer.cpp:2853). A
                    // supply truck's entry is on the default team, which is already active, so it runs
                    // here too (AIPlayer.cpp:409).
                    self.scripts.clear_team_flags();
                    state.teams_built += 1;
                } else if let Some(id) = q.reinforcement_id {
                    // `AIPlayer.cpp:2900-2904`: `obj->getAIUpdateInterface()->joinTeam()`.
                    self.ai_join_team(id);
                }
                continue;
            }
            i += 1;
        }
    }

    /// `checkQueuedTeams` / `checkReadyTeams` `anyIdle` loop (AIPlayer.cpp:2910, :2807): any member
    /// with an AI whose `AIUpdateInterface::isIdle` holds (dead members are not skipped there).
    fn ai_team_any_idle(&self, key: &str) -> bool {
        let Some(t) = self.scripts.teams.get(key) else { return false };
        t.members
            .iter()
            .any(|id| self.find_object(*id).is_some_and(|o| o.ai_handle.is_some() && self.ai_is_idle(o)))
    }

    /// `TeamInQueue::isAllBuilt`: every work order has `numCompleted >= numRequired`.
    fn ai_team_all_built(q: &TeamInQueue) -> bool {
        q.work_orders.iter().all(|w| w.required_count <= w.completed)
    }

    /// `TeamInQueue::isMinimumBuilt`: every required order is complete, counting a unit that is in
    /// production (`m_factoryID != INVALID_ID`) as one more.
    fn ai_team_minimum_built(q: &TeamInQueue) -> bool {
        q.work_orders.iter().all(|w| {
            let count = w.completed + i32::from(w.factory.is_some());
            !(w.required_count > count && w.required)
        })
    }

    /// `Team::isIdle` (Team.cpp:1752) for a ready team: shared with the `OnIdle` / sequential-script
    /// code (`ScriptEngine::team_group_idle`): AI-less and effectively dead members are skipped.
    fn ai_team_all_idle(&self, key: &str) -> bool {
        self.scripts.team_group_idle(self, key)
    }

    /// `AIPlayer::isPossibleToBuildTeam` (money + factory/tech).
    fn ai_team_affordable(&self, player: PlayerIndex, state: &SkirmishAi, name: &str) -> bool {
        let Some(proto) = self.scripts.team_proto(name) else { return false };
        self.ai_possible_to_build_team(player, state, proto, true).0
    }

    /// `AIPlayer::isPossibleToBuildTeam` (`AIPlayer.cpp:1477`): returns `(possible, notEnoughMoney)`.
    /// `sum(cost * (maxUnits + minUnits)/2) * TeamResourcesToStart` is compared against the player's
    /// money; every unit needs its tech and a factory (`findFactory(thing, true)`), and with
    /// `require_idle_factory` an idle one (`findFactory(thing, false)`) too.
    pub(crate) fn ai_possible_to_build_team(
        &self,
        player: PlayerIndex,
        state: &SkirmishAi,
        proto: &crate::script::TeamPrototype,
        require_idle_factory: bool,
    ) -> (bool, bool) {
        // `Int cost`: each `cost += thing * ((max+min)/2.0f)` and the `*= TeamResourcesToStart` truncate
        // to an integer (`AIPlayer.cpp:1477-1500`), as the float-to-int conversions do.
        let mut cost = 0i32;
        let mut any_idle = false;
        for u in &proto.units {
            // `if (thing)`: a missing template contributes nothing (AIPlayer.cpp:1487).
            let Some(t) = self.find_template(&u.template) else { continue };
            if !self.prerequisites_satisfied(player, &t) {
                return (false, false);
            }
            if self.ai_find_build_list_factory(state, &u.template, true).is_none() {
                return (false, false);
            }
            if self.ai_find_build_list_factory(state, &u.template, false).is_some() {
                any_idle = true;
            }
            cost = (cost as f32 + self.calc_cost_to_build(player, &t) as f32 * ((u.max + u.min) as f32 * 0.5)) as i32;
        }
        cost = (cost as f32 * state.config.team_resources_to_build) as i32;
        if (self.player(player).map_or(0, |p| p.money) as i64) < i64::from(cost) {
            return (false, true);
        }
        (any_idle || !require_idle_factory, false)
    }

    /// `AISkirmishPlayer::doBaseBuilding` (`AISkirmishPlayer.cpp:869`): once the `StructureSeconds`
    /// timer expires set `m_readyToBuildStructure`, and then run `processBaseBuilding` every 2
    /// seconds while ready. A successful build clears the flag and resets the timer.
    fn ai_process_structures(&mut self, player: PlayerIndex, config: &SkirmishAiConfig, state: &mut SkirmishAi) {
        if !state.ready_to_build_structure {
            if state.structure_timer > 0 {
                state.structure_timer -= 1;
            }
            if state.structure_timer == 0 {
                state.ready_to_build_structure = true;
                state.build_delay = 0;
            }
            if state.structure_timer > 3 * crate::math::LOGICFRAMES_PER_SECOND {
                state.structure_timer = 3 * crate::math::LOGICFRAMES_PER_SECOND;
            }
        }
        if state.build_delay > 0 {
            state.build_delay -= 1;
        }
        if state.build_delay != 0 {
            return;
        }
        state.build_delay = 2 * crate::math::LOGICFRAMES_PER_SECOND;
        if state.ready_to_build_structure {
            self.ai_process_base_building(player, config, state);
        }
    }

    /// `AISkirmishPlayer::processBaseBuilding` (`AISkirmishPlayer.cpp:100`): scan the build list in
    /// order and pick, in this priority, the first script-marked priority build, the first
    /// automatic / location-safe / prerequisite-met / `isBuildable` build, and finally force an
    /// underpowered power plant. One structure is dozer-built per attempt.
    fn ai_process_base_building(&mut self, player: PlayerIndex, config: &SkirmishAiConfig, state: &mut SkirmishAi) {
        if state.built.len() < config.build_list.len() {
            state.built.resize(config.build_list.len(), None);
            state.destroyed_at.resize(config.build_list.len(), 0);
            state.supply.resize(config.build_list.len(), SupplyInfo::default());
        }
        let underpowered = !self.has_sufficient_power(player);
        let mut bldg_plan: Option<usize> = None;
        let mut is_priority = false;
        let mut power_plan: Option<usize> = None;
        let mut power_under_construction = false;
        for i in 0..config.build_list.len() {
            let order = config.build_list[i].clone();
            if order.template.is_empty() {
                continue;
            }
            let Some(t) = self.find_template(&order.template) else { continue };
            // Destroyed / captured / under-construction bookkeeping.
            if let Some(id) = state.built[i] {
                match self.find_object(id) {
                    None => {
                        // The tracked building died. A GLA `RebuildHoleExposeDie` may already have
                        // left a rebuild hole reconstructing it; re-point the entry at that hole so
                        // the AI does not send a second dozer (`AISkirmishPlayer::processBaseBuilding`,
                        // AISkirmishPlayer.cpp:130-142).
                        let prior = id;
                        let hole = self
                            .objects
                            .values()
                            .find(|o| {
                                !o.destroyed
                                    && o.is_kind_of(kind_of::REBUILD_HOLE)
                                    && o.behavior.hole_spawner == Some(prior)
                            })
                            .map(|o| o.id);
                        state.built[i] = hole;
                        if hole.is_none() && state.destroyed_at[i] == 0 {
                            state.destroyed_at[i] = self.frame + 1;
                        }
                    }
                    Some(o) => {
                        if o.player != player {
                            // got captured.
                            state.built[i] = None;
                            state.destroyed_at[i] = self.frame + 1;
                        } else if o.under_construction
                            && t.is_kind_of(kind_of::FS_POWER)
                            && !t.is_kind_of(kind_of::CASH_GENERATOR)
                        {
                            power_under_construction = true;
                        }
                    }
                }
            }
            if state.built[i].is_none() && state.destroyed_at[i] > 0 {
                // Wait a few seconds before initiating a rebuild.
                if self.frame < state.destroyed_at[i] + config.rebuild_delay_seconds {
                    continue;
                }
                state.destroyed_at[i] = 0;
            }
            if state.built[i].is_some_and(|id| self.find_object(id).is_some()) {
                continue; // already built.
            }
            // Make sure it is safe to build here (`isLocationSafe`).
            let loc = Coord3D::new(order.location.0, order.location.1, 0.0);
            if !self.is_location_safe(player, &loc, &t, None) {
                continue;
            }
            // Always take the first priority build.
            if state.priority.contains(&i) && !is_priority {
                bldg_plan = Some(i);
                is_priority = true;
            }
            if t.is_kind_of(kind_of::FS_POWER)
                && !t.is_kind_of(kind_of::CASH_GENERATOR)
                && power_plan.is_none()
                && (underpowered || order.automatically_build)
            {
                power_plan = Some(i);
            }
            if !order.automatically_build {
                continue; // marked to not build automatically.
            }
            let Some(_dozer) = self.ai_find_dozer(player, &loc, state) else {
                if underpowered {
                    self.ai_queue_dozer(player, state);
                }
                continue;
            };
            // `TheBuildAssistant->canMakeUnit(dozer, bldgPlan)` (AISkirmishPlayer.cpp:226) tests the
            // plan chosen so far, not this entry. With no plan chosen it is `CANMAKE_NO_PREREQ`
            // (`BuildAssistant::canMakeUnit`, BuildAssistant.cpp:1316), so the entry is skipped. Only
            // a priority (script) entry or a forced power plant can therefore be the plan, and the
            // `isBuildable()` (`Rebuilds`) selection that followed in the C++ is unreachable: retail
            // does not rebuild automatic list entries on its own.
            if bldg_plan.is_none() {
                continue;
            }
        }
        // Force a power plant when underpowered (unless one is already under construction).
        if let Some(pi) = power_plan {
            let equivalent = bldg_plan.is_some_and(|bi| config.build_list[bi].template == config.build_list[pi].template);
            if !equivalent && !power_under_construction {
                bldg_plan = Some(pi);
            }
        }
        let Some(i) = bldg_plan else { return };
        let order = config.build_list[i].clone();
        let pos = Coord3D::new(order.location.0, order.location.1, self.terrain.ground_height(order.location.0, order.location.1));
        if l133_build_diag(&order.template) {
            eprintln!(
                "ZR_L133_PLAN f={} player={} idx={} tmpl={} loc=({:.4},{:.4}) angle={:.4} priority={}",
                self.frame, player, i, order.template, order.location.0, order.location.1, order.angle, state.priority.contains(&i)
            );
        }
        let Some(dozer) = self.ai_find_dozer(player, &pos, state) else { return };
        if let Some(id) = self.ai_construct_near(player, dozer, &order.template, pos, order.angle) {
            state.built[i] = Some(id);
            state.destroyed_at[i] = 0;
            // `BuildListInfo::decrementNumRebuilds`: finite rebuilds count down; unlimited (-1) and
            // 0 are left alone.
            if order.rebuilds > 0 {
                state.config.build_list[i].rebuilds -= 1;
            }
            state.structures_built += 1;
            state.ready_to_build_structure = false;
            let mut timer = config.structure_seconds;
            let money = self.player(player).map_or(0, |p| p.money) as i32;
            if money < config.resources_poor && config.structures_poor_mod > 0.0 {
                timer = (timer as f32 / config.structures_poor_mod) as u32;
            } else if money > config.resources_wealthy && config.structures_wealthy_mod > 0.0 {
                timer = (timer as f32 / config.structures_wealthy_mod) as u32;
            }
            state.structure_timer = timer;
        } else {
            let cost = self.find_template(&order.template).map_or(0, |t| self.calc_cost_to_build(player, &t));
            let money = self.player(player).map_or(0, |p| p.money);
            // The gate `buildStructureWithDozer` runs first (AIPlayer.cpp:536).
            let legal = self.find_template(&order.template).is_some_and(|t| self.ai_dozer_enemy_only_site(dozer, &t, &pos, order.angle));
            let reason = if money < cost { "money" } else if !legal { "location" } else { "other" };
            *self.ai_structure_fail_counts.entry(format!("{}:{reason}", order.template)).or_default() += 1;
        }
    }

    /// Construct `template` near `pos`, trying a few deterministic offsets when the exact spot is
    /// blocked (a rebuilt structure whose original footprint is now covered by another building).
    fn ai_construct_near(&mut self, player: PlayerIndex, dozer: ObjectId, template: &str, pos: Coord3D, angle: f32) -> Option<ObjectId> {
        let t = self.find_template(template)?;
        let diag = l133_build_diag(template);
        if diag {
            eprintln!("ZR_L133_SITE f={} dozer={} tmpl={} req=({:.4},{:.4}) angle={:.4}", self.frame, dozer.0, template, pos.x, pos.y, angle);
        }
        // `AIPlayer::buildStructureWithDozer` (`AIPlayer.cpp:523-525`): "Check available funds" runs
        // right after the dozer is found and **before** both legality tests. An unaffordable plan
        // therefore never runs the site search, and never reaches `buildObjectNow`'s
        // `moveObjectsForConstruction` (`BuildAssistant.cpp:1421-1494`), which moves allied units out
        // of the footprint and draws two logic RNG values per unit. AH7 s1 f2764: the GLA ArmsDealer
        // plan had 1775 of 2500 and the port still moved worker 209 out of the way.
        let cost = self.calc_cost_to_build(player, &t);
        if self.player(player).map_or(0, |p| p.money) < cost {
            return None;
        }
        // `buildStructureWithDozer` (AIPlayer.cpp:536): the enemy-only test on the requested spot. A failure
        // returns null before any search, so the structure is not started.
        if !self.ai_dozer_enemy_only_site(dozer, &t, &pos, angle) {
            if diag {
                eprintln!("ZR_L133_ENEMY f={} tmpl={} rejected", self.frame, template);
            }
            return None;
        }
        // `AIPlayer::buildStructureWithDozer` (AIPlayer.cpp:537-611): the requested spot when it is legal
        // (`CLEAR_PATH | TERRAIN_RESTRICTIONS | NO_OBJECT_OVERLAP`); otherwise the first legal spot of an
        // expanding spiral around it. The skirmish variant uses a 120-cell limit and, because the body adds
        // another `2*CELL` on top of the `for` increment, tests the rings at ``posOffset`` 20, 60, 100, ...
        // (offsets 10, 30, 50, ...). The loop structure, including which `valid` value survives the end of a
        // row, follows C++. If no spot is found, the requested spot is used. C++'s last fallback (:610)
        // repeats the enemy-only test on that same spot, which already passed above, so it cannot fail here.
        // `construct` then starts the build (`DozerAIUpdate.cpp:1671-1697`).
        const CELL: f32 = 10.0; // PATHFIND_CELL_SIZE_F
        let limit = 120.0 * CELL;
        let mut target = pos;
        if !self.ai_dozer_legal_site(dozer, &t, &pos, angle) {
            let mut valid = false;
            let mut new_pos = pos;
            let mut pos_offset = 0.0f32;
            'spiral: while pos_offset < limit {
                pos_offset += 2.0 * CELL; // skirmish: `posOffset += 2*CELL` in the loop body
                let offset = pos_offset / 2.0;
                // Top row (y = pos.y - offset) and bottom row (y = pos.y - offset + posOffset).
                let top = pos.y - offset;
                let mut x = pos.x - offset;
                while x <= pos.x + offset {
                    x += CELL; // skirmish: `xPos += CELL` in the body
                    new_pos = Coord3D::new(x, top, pos.z);
                    valid = self.ai_dozer_legal_site(dozer, &t, &new_pos, angle);
                    if valid {
                        break;
                    }
                    new_pos.y = top + pos_offset;
                    valid = self.ai_dozer_legal_site(dozer, &t, &new_pos, angle);
                    x += CELL; // the `for` increment
                }
                if valid {
                    break 'spiral;
                }
                // Left column (x = pos.x - offset) and right column (x = pos.x - offset + posOffset).
                let left = pos.x - offset;
                let mut y = pos.y - offset;
                while y <= pos.y + offset {
                    y += CELL; // skirmish: `yPos += CELL` in the body
                    new_pos = Coord3D::new(left, y, pos.z);
                    valid = self.ai_dozer_legal_site(dozer, &t, &new_pos, angle);
                    if valid {
                        break;
                    }
                    new_pos.x = left + pos_offset;
                    valid = self.ai_dozer_legal_site(dozer, &t, &new_pos, angle);
                    y += CELL; // the `for` increment
                }
                if valid {
                    break 'spiral;
                }
                // The `for` loop's increment (`posOffset += 2*PATHFIND_CELL_SIZE_F`); the skirmish body
                // increment above sits on top of it, so the next ring is four cells wider.
                pos_offset += 2.0 * CELL;
            }
            if valid {
                target = new_pos;
            }
        }
        let p = Coord3D::new(target.x, target.y, self.terrain.ground_height(target.x, target.y));
        if diag {
            eprintln!("ZR_L133_CHOSEN f={} dozer={} tmpl={} pos=({:.4},{:.4})", self.frame, dozer.0, template, p.x, p.y);
        }
        self.construct(dozer, template, p, angle)
    }

    /// `isLocationLegalToBuild(pos, NO_ENEMY_OBJECT_OVERLAP, dozer, m_player)` (`AIPlayer.cpp:536`, the gate
    /// `buildStructureWithDozer` runs first, and again as its last fallback at :610): only the enemy and immobile
    /// object test, with the dozer as the builder. No terrain or clear-path test.
    fn ai_dozer_enemy_only_site(&self, dozer: ObjectId, t: &ObjectTemplate, pos: &Coord3D, angle: f32) -> bool {
        let builder = self.objects.get(&dozer).map(|o| o.player);
        let options = crate::build::LegalBuildOptions { terrain: false, objects: crate::build::ObjectOverlap::EnemiesOnly };
        self.is_location_legal_with(t, pos, angle, options, builder, builder)
    }

    /// `isLocationLegalToBuild(pos, CLEAR_PATH | TERRAIN_RESTRICTIONS | NO_OBJECT_OVERLAP, dozer)`, the
    /// test `AIPlayer::buildStructureWithDozer` runs on the requested spot and on every spiral
    /// candidate (`AIPlayer.cpp:553-575`, `:578-585`). For a mobile builder, `CLEAR_PATH` is
    /// `ai->isQuickPathAvailable(pos)` (`BuildAssistant.cpp:1003-1018`), i.e. the dozer's own
    /// `clientSafeQuickDoesPathExist` (`AIUpdate.cpp:2096`) from its position to the site.
    fn ai_dozer_legal_site(&mut self, dozer: ObjectId, t: &ObjectTemplate, pos: &Coord3D, angle: f32) -> bool {
        let builder = self.objects.get(&dozer).map(|o| o.player);
        let diag = l133_build_diag(&t.name);
        let terrain = self.is_location_legal_to_build_by(t, pos, angle, builder);
        if diag {
            eprintln!(
                "ZR_L133_CAND f={} dozer={} tmpl={} pos=({:.4},{:.4}) terrain_objects={}",
                self.frame, dozer.0, t.name, pos.x, pos.y, terrain
            );
        }
        if !terrain {
            return false;
        }
        let Some((from, po)) = self.objects.get(&dozer).and_then(|o| {
            crate::pathfind::PathObject::for_object(o).map(|p| (o.position, p))
        }) else {
            return false;
        };
        let quick = self.pathfinder.quick_does_path_exist(&po, &from, pos);
        if diag {
            eprintln!("ZR_L133_PATH f={} dozer={} tmpl={} pos=({:.4},{:.4}) quick={}", self.frame, dozer.0, t.name, pos.x, pos.y, quick);
        }
        quick
    }

    // ---- script-driven build hooks (AIPlayer::buildSpecificAIBuilding and friends) -------------

    /// `AISkirmishPlayer::buildSpecificAIBuilding`: mark the build order for `template` priority.
    /// If it isn't in the build list (base defenses), add a synthetic order near the base.
    pub fn ai_build_specific(&mut self, player: PlayerIndex, template: &str) {
        if template.is_empty() {
            return;
        }
        *self.ai_build_specific_counts.entry(template.to_string()).or_default() += 1;
        self.ai_build_at(player, template);
    }

    /// `AIPlayer::buildBySupplies` (`AIPlayer.cpp:1875`): find the best supply warehouse worth at least
    /// `minimum_cash` (`findSupplyCenter`; a non-cash-generator template, i.e. a defense, uses the
    /// current warehouse instead) and add a one-shot priority build-list entry next to it
    /// (`Player::addToPriorityBuildList`: `Rebuilds = 1`, priority flag). A cash generator goes 3 cells
    /// toward the base from the warehouse; anything else goes one warehouse radius toward the enemy.
    /// `minimum_cash` is the *warehouse's* supply value, not the player's money. The entry is
    /// prepended and an illegal spot is wiggled like the engine; legality is the sim's
    /// `is_location_legal_to_build` (terrain + overlap, no `CLEAR_PATH`/bib/shroud flags).
    pub fn ai_build_by_supplies(&mut self, player: PlayerIndex, minimum_cash: i32, template: &str) {
        let Some(t) = self.find_template(template) else { return };
        let cash_generator = t.is_kind_of(kind_of::CASH_GENERATOR);
        let mut best = self.ai_find_supply_warehouse(player, minimum_cash);
        if !cash_generator {
            // Build by the current warehouse.
            if let Some(cur) = self.ai_players.get(&player).and_then(|a| a.cur_warehouse).filter(|c| self.find_object(*c).is_some()) {
                best = Some(cur);
            }
        }
        let Some(warehouse) = best else { return };
        let Some(wh) = self.find_object(warehouse) else { return };
        let (wh_pos, wh_radius) = (wh.position, wh.template.geometry.bounding_circle_radius());
        // `AIPlayer::m_baseCenter` (`computeCenterAndRadiusOfBase`, AIPlayer.cpp:3160): the average of
        // the build-list locations, not the command center.
        let base = self.ai_players.get(&player).map_or(wh_pos, |ai| Coord3D::new(ai.base_center.0, ai.base_center.1, 0.0));
        let (mut dx, mut dy) = (wh_pos.x - base.x, wh_pos.y - base.y);
        let mut radius = 3.0 * crate::pathfind::PATHFIND_CELL_SIZE_F;
        if !cash_generator {
            // It's probably a defensive structure: build towards the enemy.
            let enemy = self.ai_players.get(&player).and_then(|a| a.current_enemy);
            let e = enemy.and_then(|e| self.player_structure_bounds_center(e)).unwrap_or(Coord3D::ZERO);
            dx = wh_pos.x - e.x;
            dy = wh_pos.y - e.y;
            radius = wh_radius;
        }
        let len = (dx * dx + dy * dy).sqrt();
        let (nx, ny) = if len > 0.0 { (dx / len, dy / len) } else { (0.0, 0.0) };
        let mut loc = (wh_pos.x - nx * radius, wh_pos.y - ny * radius);
        let angle = t.placement_view_angle;
        // "Validate that the position to build at is valid": when the spot is not legal, wiggle it
        // over growing squares (`AIPlayer.cpp:1925-1985`); if nothing is legal the original stays.
        // `buildBySupplies` (AIPlayer.cpp:1921): the first check is `NO_OBJECT_OVERLAP` alone (no terrain test);
        // the wiggle (:1945-1977) adds `TERRAIN_RESTRICTIONS`. Both take a null builder and `m_player`.
        let legal = |logic: &GameLogic, x: f32, y: f32, terrain: bool| {
            let p = Coord3D::new(x, y, logic.terrain.ground_height(x, y));
            let options = crate::build::LegalBuildOptions { terrain, objects: crate::build::ObjectOverlap::All };
            logic.is_location_legal_with(&t, &p, angle, options, None, Some(player))
        };
        if !legal(self, loc.0, loc.1, false) {
            let cell = crate::pathfind::PATHFIND_CELL_SIZE_F;
            let close = 20.0 * cell; // SUPPLY_CENTER_CLOSE_DIST
            let mut found: Option<(f32, f32)> = None;
            let mut pos_offset = 0.0f32;
            'wiggle: while pos_offset < 2.0 * close {
                let offset = pos_offset / 2.0;
                let y_pos = loc.1 - offset;
                let mut x_pos = loc.0 - offset;
                while x_pos <= loc.0 + offset {
                    for (x, y) in [(x_pos, y_pos), (x_pos, y_pos + pos_offset)] {
                        if legal(self, x, y, true) {
                            found = Some((x, y));
                            break 'wiggle;
                        }
                    }
                    x_pos += cell;
                }
                let x_pos = loc.0 - offset;
                let mut y_pos = loc.1 - offset;
                while y_pos <= loc.1 + offset {
                    for (x, y) in [(x_pos, y_pos), (x_pos + pos_offset, y_pos)] {
                        if legal(self, x, y, true) {
                            found = Some((x, y));
                            break 'wiggle;
                        }
                    }
                    y_pos += cell;
                }
                pos_offset += 2.0 * cell;
            }
            if let Some(f) = found {
                loc = f;
            }
        }
        let Some(ai) = self.ai_players.get_mut(&player) else { return };
        // `Player::addToPriorityBuildList` (`Player.cpp:656`) *prepends* (`setNextBuildList(m_pBuildList)`),
        // so every per-entry index shifts by one.
        let n = ai.config.build_list.len();
        ai.built.resize(n, None);
        ai.destroyed_at.resize(n, 0);
        ai.supply.resize(n, SupplyInfo::default());
        ai.config.build_list.insert(
            0,
            AiBuildOrder { template: template.to_owned(), location: loc, angle, initially_built: false, rebuilds: 1, automatically_build: true },
        );
        ai.built.insert(0, None);
        ai.destroyed_at.insert(0, 0);
        ai.supply.insert(0, SupplyInfo::default());
        ai.priority = ai.priority.iter().map(|i| i + 1).collect();
        ai.priority.insert(0);
        ai.cur_warehouse = Some(warehouse);
    }


    /// `AIPlayer::findSupplyCenter` (`AIPlayer.cpp:2212`): the neutral/allied supply warehouse worth
    /// at least `minimum_cash` (boxes * `ValuePerSupplyBox`, halved down to 100 until one is found)
    /// that has none of this player's `CASH_GENERATOR`s within `SUPPLY_CENTER_CLOSE_DIST` + its
    /// radius, is not closer to the enemy than 60/40, and is closest to the base center.
    fn ai_find_supply_warehouse(&self, player: PlayerIndex, minimum_cash: i32) -> Option<ObjectId> {
        let ai = self.ai_players.get(&player);
        // `AIPlayer::m_baseCenter` (see `ai_build_by_supplies`).
        let base = ai.map_or(Coord3D::ZERO, |ai| Coord3D::new(ai.base_center.0, ai.base_center.1, 0.0));
        let enemy_center = ai
            .and_then(|a| a.current_enemy)
            .map(|e| self.player_structure_bounds_center(e).unwrap_or(Coord3D::ZERO));
        let close = 20.0 * crate::pathfind::PATHFIND_CELL_SIZE_F;
        let mut minimum_cash = minimum_cash;
        loop {
            let mut best: Option<(f32, ObjectId)> = None;
            for o in self.objects() {
                if !o.is_kind_of(kind_of::STRUCTURE) || !o.is_kind_of(kind_of::SUPPLY_SOURCE) {
                    continue;
                }
                if !o.template.dock.as_ref().is_some_and(|d| d.kind == crate::supply::DockKind::Warehouse) {
                    continue;
                }
                let available = i64::from(o.boxes) * i64::from(self.rules.value_per_supply_box);
                if available < i64::from(minimum_cash) {
                    continue;
                }
                if self.relationship(player, o.player) == crate::damage::Relationship::Enemies {
                    continue;
                }
                // Make sure we don't have a supply center near it.
                let radius = close + o.template.geometry.bounding_circle_radius();
                let have_center = self.objects().any(|c| {
                    !c.destroyed
                        && c.player == player
                        && c.is_kind_of(kind_of::CASH_GENERATOR)
                        && (c.position.dist_2d(&o.position) - c.template.geometry.bounding_circle_radius()).max(0.0) <= radius
                });
                if have_center {
                    continue;
                }
                let dist_sqr = {
                    let (dx, dy) = (o.position.x - base.x, o.position.y - base.y);
                    dx * dx + dy * dy
                };
                if let Some(e) = enemy_center {
                    // Closer than 60/40 to the enemy than to us: not a good expansion candidate.
                    let (dx, dy) = (o.position.x - e.x, o.position.y - e.y);
                    if dist_sqr * 0.4 > (dx * dx + dy * dy) * 0.6 {
                        continue;
                    }
                }
                if best.is_none_or(|(bd, _)| dist_sqr < bd) {
                    best = Some((dist_sqr, o.id));
                }
            }
            if let Some((_, id)) = best {
                return Some(id);
            }
            minimum_cash /= 2;
            if minimum_cash <= 100 {
                return None;
            }
        }
    }

    /// `AIPlayer::isSupplySourceSafe` -> `AIPlayer::isSupplySourceSafe`: true when the AI has no
    /// qualifying supply source (nothing to lose) or the nearest one is a safe build location.
    pub fn supply_source_safe(&self, player: PlayerIndex, min_supplies: i32) -> bool {
        let Some(warehouse) = self.ai_find_supply_warehouse(player, min_supplies) else { return true };
        let Some(wh) = self.find_object(warehouse) else { return true };
        self.is_location_safe(player, &wh.position, &wh.template, Some(warehouse))
    }

    /// `AIPlayer::isLocationSafe` (`AIPlayer.cpp:1004`): true when no significant, non-stealthed
    /// enemy (harvesters and dozers excluded) is within `SupplyCenterSafeRadius + the template's
    /// bounding radius`. `ignore` skips the object being tested (the supply source itself). The
    /// filters are exactly the original's (`:1015-1031`): affiliation, alive, stealth,
    /// `PartitionFilterInsignificantBuildings`, harvesters and dozers. A projectile is not excluded
    /// (the original has no such filter) — it blocks a site like any other enemy non-building.
    ///
    /// The candidate set is the partition manager's, not every live object:
    /// `getClosestObject` (`PartitionManager.cpp:3285`) walks the cells, and a newly created object
    /// is in none until `ThePartitionManager->update()` places them at the end of the frame
    /// (`ThingFactory::newObject` only calls `registerObject`, ThingFactory.cpp:346, and
    /// `PartitionData::attachToObject` only marks the module dirty, PartitionManager.cpp:2311-2345).
    /// So a structure created earlier in the same frame (e.g. the USA spy-satellite ping the frame's
    /// scripts fire at the enemy base) does not make a base location unsafe yet. The port's
    /// `register_deferred` models that (`partition.rs`); scanning `self.objects` here made AN1 seed 1
    /// skip the China 1st Power Plant at frame 1 (`O id=214 SpySatellitePing` as the blocker).
    pub fn is_location_safe(&self, player: PlayerIndex, pos: &Coord3D, t: &ObjectTemplate, ignore: Option<ObjectId>) -> bool {
        let radius = self.ai_players.get(&player).map_or(250.0, |ai| ai.config.supply_center_safe_radius)
            + t.geometry.bounding_circle_radius();
        // Whole-grid query: `objects_in_range` clamps its cell span to the partition, so this is
        // "every object whose cells are placed", the superset of the cells C++ searches from `pos`.
        // The distance test below uses the object's current position, as `distCalcProc_BoundaryAndBoundary_2D`
        // does (`PartitionManager.cpp:806-858`); objects move at most a few units per frame, far below
        // one cell, so the wider candidate span cannot add an object the C++ would skip.
        let (cx, cy) = self.partition.cell_counts();
        let span = self.partition.cell_size() * (cx.max(cy) as f32) * 2.0;
        for id in self.partition.objects_in_range(pos, span, crate::partition::DistanceMode::FromBoundingSphere2D) {
            let Some(o) = self.find_object(id) else { continue };
            if Some(id) == ignore
                || o.destroyed
                || o.is_dying()
                || o.contained_by.is_some() // contained objects are out of the partition
                || self.relationship(player, o.player) != crate::damage::Relationship::Enemies
                || self.is_stealthed(id)
                || o.is_kind_of(kind_of::HARVESTER)
                || o.is_kind_of(kind_of::DOZER)
                || !self.is_significant_building(o)
            {
                continue;
            }
            // Centre distance minus the object's bounding radius, clamped at 0, strictly < radius.
            let shrunk = (o.position.dist_2d(pos) - o.template.geometry.bounding_circle_radius()).max(0.0);
            if shrunk * shrunk < radius * radius {
                return false;
            }
        }
        true
    }

    /// `PartitionFilterInsignificantBuildings(allowNonBuildings = true, allowInsignificant = false)`
    /// (`PartitionManager.cpp:5074`): a structure outside `KINDOFMASK_FS` (faction structures) that
    /// has a contain module only counts when it is garrisonable (`GarrisonContain`) and occupied.
    fn is_significant_building(&self, o: &crate::object::Object) -> bool {
        use crate::template::collide_kind as ck;
        if !o.is_kind_of(kind_of::STRUCTURE) {
            return true;
        }
        let t = &o.template;
        let faction = t.kind_of
            & (kind_of::FS_FACTORY
                | kind_of::FS_BASE_DEFENSE
                | kind_of::FS_TECHNOLOGY
                | kind_of::FS_SUPERWEAPON
                | kind_of::FS_STRATEGY_CENTER
                | kind_of::FS_INTERNET_CENTER)
            != 0
            || t.collide_kinds
                & (ck::FS_FAKE | ck::FS_BARRACKS | ck::FS_WARFACTORY | ck::FS_AIRFIELD | ck::FS_SUPPLY_CENTER | ck::FS_SUPPLY_DROPZONE)
                != 0;
        if faction {
            return true;
        }
        match t.contain.as_ref() {
            Some(c) => c.is_garrison() && !self.contained_units(o.id).is_empty(),
            None => true,
        }
    }

    /// `AISkirmishPlayer::buildSpecificAIBuilding`: mark the first instance of `template` that has no
    /// live object and is not already queued. When the template is absent from the build list
    /// entirely, a synthetic order is added (a sim deviation for structures the shipped list omits).
    fn ai_build_at(&mut self, player: PlayerIndex, template: &str) {
        if template.is_empty() {
            return;
        }
        let mut chosen: Option<usize> = None;
        if let Some(ai) = self.ai_players.get(&player) {
            for i in 0..ai.config.build_list.len() {
                if ai.config.build_list[i].template != template {
                    continue;
                }
                let alive = ai.built.get(i).copied().flatten().is_some_and(|id| self.find_object(id).is_some());
                if alive || ai.priority.contains(&i) {
                    continue;
                }
                chosen = Some(i);
                break;
            }
        }
        // `AISkirmishPlayer::buildSpecificAIBuilding` (AISkirmishPlayer.cpp:410-460): the first unbuilt
        // entry is marked priority and `m_buildDelay = 0`, so the next structure check runs at once. An
        // entry that is built or already queued, or a template that is not in the list at all, is only
        // reported (the engine's `AppendDebugMessage`); no entry is added.
        if let Some(i) = chosen {
            if let Some(ai) = self.ai_players.get_mut(&player) {
                ai.priority.insert(i);
                ai.build_delay = 0;
            }
        }
    }

    /// `AIPlayer::buildUpgrade` (`AIPlayer.cpp:1794-1860`): one shot at a player upgrade, queued at the
    /// first factory of the AI's build list that takes it, and dropped otherwise. The request is refused
    /// when the upgrade does not exist (`findUpgrade`), is already in progress for the player
    /// (`hasUpgradeInProduction`, here the player's factory queues) or completed (`hasUpgradeComplete`),
    /// or the player cannot afford it (`canAffordUpgrade`). A factory is skipped while it is under
    /// construction, has no production interface, or its command set has no button for the upgrade
    /// (`canUpgradeHere`, via `TemplateSource::can_build_upgrade`; `None` counts as no button, as a missing
    /// command set does in C++). C++ has no fallback to another factory. Object upgrades are skipped, as
    /// `AIPlayer::buildUpgrade` skips them (AIPlayer.cpp:1786; see `ai_build_upgrade_typed`). Not modelled: the
    /// sold status (the port has none) and `CommandSetUpgrade` (the template's base command set is used).
    ///
    /// The type (`UpgradeTemplate::getUpgradeType`) is read from the command-button table. A caller that runs
    /// from a script action must use [`Self::ai_build_upgrade_typed`] instead: `script_engine_update` takes the
    /// engine out of `GameLogic`, so `self.scripts` is an empty engine there and the lookup would find nothing.
    pub fn ai_build_upgrade(&mut self, player: PlayerIndex, upgrade: &str) {
        let object = self.scripts.command_button_upgrades.values().any(|b| b.object && b.upgrade.eq_ignore_ascii_case(upgrade));
        self.ai_build_upgrade_typed(player, upgrade, object);
    }

    /// [`Self::ai_build_upgrade`] with the upgrade's type passed in (`UPGRADE_TYPE_OBJECT`, AIPlayer.cpp:1786).
    pub fn ai_build_upgrade_typed(&mut self, player: PlayerIndex, upgrade: &str, object_upgrade: bool) {
        if upgrade.is_empty() {
            return;
        }
        let Some(info) = self.upgrade_info.get(upgrade).copied() else { return };
        // `AIPlayer::buildUpgrade` (AIPlayer.cpp:1786): an object upgrade is not a player upgrade and is ignored.
        if object_upgrade {
            return;
        }
        if self.player_upgrades(player).any(|u| u == upgrade) || self.upgrade_in_production(player, upgrade) {
            return;
        }
        if self.player(player).is_none_or(|p| p.money < info.cost) {
            return;
        }
        let source = self.template_source.clone();
        // `m_player->getBuildList()` order: the AI's own factory slots, earliest entry first.
        let factories: Vec<ObjectId> = self.ai_players.get(&player).map_or_else(Vec::new, |ai| ai.built.iter().flatten().copied().collect());
        for id in factories {
            let Some(o) = self.objects.get(&id) else { continue };
            if o.destroyed || o.player != player || o.under_construction || o.production.is_none() {
                continue;
            }
            // `canUpgradeHere` (AIPlayer.cpp:1822): the factory's command set must hold the button.
            if source.as_ref().and_then(|s| s.can_build_upgrade(&o.template.name, upgrade)) != Some(true) {
                continue;
            }
            // `queueUpgrade` with the type known: a player upgrade (`object_upgrade == false`). Passing it avoids
            // re-reading the command-button table, which is empty while a script action is running.
            if self.queue_upgrade_typed(player, id, upgrade, false) {
                return;
            }
        }
    }

    /// `Player::hasUpgradeInProduction` (`Player.cpp`): an upgrade is in progress for the player when any
    /// of its objects has it queued (the port's equivalent of the player's in-progress upgrade set).
    pub(crate) fn upgrade_in_production(&self, player: PlayerIndex, upgrade: &str) -> bool {
        self.objects.values().any(|o| {
            !o.destroyed
                && o.player == player
                && o.production.as_ref().is_some_and(|p| {
                    p.queue.iter().any(|e| matches!(&e.kind, crate::production::ProductionKind::Upgrade(n) if n == upgrade))
                })
        })
    }

    /// `AISkirmishPlayer::buildAIBaseDefenseStructure` (`AISkirmishPlayer.cpp:599`): place a
    /// structure on the base circle in the direction of the player's `Center`/`Flank`/`Backdoor`
    /// approach path. Successive defenses fan out from the path by one `angleOffset` each; front
    /// defenses alternate left/right of the path, flank defenses alternate between the `Flank` and
    /// `Backdoor` paths. `addToPriorityBuildList` prepends a one-rebuild priority entry. The loop
    /// retries while the spot is illegal and stops once the fan angle passes `PI/3` (or, on a flank
    /// with no path, immediately). An unknown template is a silent return (the engine
    /// `DEBUG_CRASH`es).
    ///
    /// `paths` is the script engine, for `m_player->getMpStartIndex()` (`player_start`, `+1` for the
    /// `Center<n>` label) and `TheTerrainLogic->getClosestWaypointOnPath`. It must be passed in: every caller
    /// is a script action, and `script_engine_update` takes the engine out of `GameLogic`, so `self.scripts`
    /// is an empty engine there (all `Center<n>` lookups would fail and every defense would land off-path).
    pub fn ai_build_base_structure(
        &mut self,
        player: PlayerIndex,
        template: &str,
        flank: bool,
        paths: &crate::script::ScriptEngine,
    ) {
        let Some(t) = self.find_template(template) else { return };
        let Some(mut state) = self.ai_players.get(&player).cloned() else { return };
        let center = state.base_center;
        let extra = state.config.base_defense_extra_distance;
        let mp_ndx = paths.player_start.get(&player).copied().unwrap_or(0) + 1;
        loop {
            let path_label = if flank {
                if state.cur_flank_base_defense & 1 != 0 {
                    format!("Flank{mp_ndx}")
                } else {
                    format!("Backdoor{mp_ndx}")
                }
            } else {
                format!("Center{mp_ndx}")
            };
            let mut goal_pos = Coord3D::new(center.0, center.1, 0.0);
            if let Some(p) = paths.closest_waypoint_on_path_pos(&goal_pos, &path_label) {
                goal_pos = p;
            } else if flank {
                return;
            } else {
                // `getPlayerStructureBounds(getMyEnemyPlayerIndex())`: fall back to the enemy base
                // centre when the map has no `Center<n>` approach path.
                let (lo, hi) = state
                    .current_enemy
                    .and_then(|e| self.ai_player_structure_bounds(e))
                    .unwrap_or(((0.0, 0.0), (0.0, 0.0)));
                goal_pos.x = lo.0 + (hi.0 - lo.0) * 0.5;
                goal_pos.y = lo.1 + (hi.1 - lo.1) * 0.5;
            }
            let (mut ox, mut oy) = (goal_pos.x - center.0, goal_pos.y - center.1);
            let len = (ox * ox + oy * oy).sqrt();
            if len > 0.0 {
                ox /= len;
                oy /= len;
            } else {
                ox = 0.0;
                oy = 0.0;
            }
            let defense_distance = state.base_radius + extra;
            ox *= defense_distance;
            oy *= defense_distance;
            let structure_radius = t.geometry.bounding_circle_radius();
            let base_circumference = 2.0 * std::f32::consts::PI * defense_distance;
            let angle_offset = if base_circumference > 0.0 {
                2.0 * std::f32::consts::PI * (structure_radius * 4.0 / base_circumference)
            } else {
                0.0
            };
            let angle;
            if flank {
                let selector = state.cur_flank_base_defense >> 1;
                if state.cur_flank_base_defense & 1 != 0 {
                    if selector & 1 != 0 {
                        state.cur_left_flank_right_defense_angle -= angle_offset;
                        angle = state.cur_left_flank_right_defense_angle;
                    } else {
                        angle = state.cur_left_flank_left_defense_angle;
                        state.cur_left_flank_left_defense_angle += angle_offset;
                    }
                } else if selector & 1 != 0 {
                    state.cur_right_flank_right_defense_angle -= angle_offset;
                    angle = state.cur_right_flank_right_defense_angle;
                } else {
                    angle = state.cur_right_flank_left_defense_angle;
                    state.cur_right_flank_left_defense_angle += angle_offset;
                }
            } else {
                let selector = state.cur_front_base_defense;
                if selector & 1 != 0 {
                    state.cur_front_right_defense_angle -= angle_offset;
                    angle = state.cur_front_right_defense_angle;
                } else {
                    angle = state.cur_front_left_defense_angle;
                    state.cur_front_left_defense_angle += angle_offset;
                }
            }
            if angle > std::f32::consts::PI / 3.0 {
                break;
            }
            let s = crate::math::sin(angle);
            let c = crate::math::cos(angle);
            let (bx, by) = (center.0 + ox * c - oy * s, center.1 + oy * c + ox * s);
            let place_angle = t.placement_view_angle;
            // `buildPos = m_baseCenter` (AISkirmishPlayer.cpp:688), so z is the base centre's z, which is 0
            // (`AIPlayer::m_baseCenter.zero()`, AIPlayer.cpp:97; `computeCenterAndRadiusOfBase` sets only x/y).
            // The ground height is added later, in `buildStructureWithDozer` (AIPlayer.cpp:530).
            let build_pos = Coord3D::new(bx, by, 0.0);
            // `AISkirmishPlayer::buildAIBaseDefenseStructure` (AISkirmishPlayer.cpp:695): `nullptr` builder, `m_player`.
            let can_build = self.is_location_legal_to_build_for(&t, &build_pos, place_angle, None, Some(player));
            if flank {
                state.cur_flank_base_defense += 1;
            } else {
                state.cur_front_base_defense += 1;
            }
            if can_build {
                self.ai_prepend_priority_build(player, template, (bx, by), place_angle);
                state.base_defenses_built += 1;
                break;
            }
        }
        if let Some(ai) = self.ai_players.get_mut(&player) {
            ai.cur_front_base_defense = state.cur_front_base_defense;
            ai.cur_flank_base_defense = state.cur_flank_base_defense;
            ai.cur_front_left_defense_angle = state.cur_front_left_defense_angle;
            ai.cur_front_right_defense_angle = state.cur_front_right_defense_angle;
            ai.cur_left_flank_left_defense_angle = state.cur_left_flank_left_defense_angle;
            ai.cur_left_flank_right_defense_angle = state.cur_left_flank_right_defense_angle;
            ai.cur_right_flank_left_defense_angle = state.cur_right_flank_left_defense_angle;
            ai.cur_right_flank_right_defense_angle = state.cur_right_flank_right_defense_angle;
            ai.base_defenses_built = state.base_defenses_built;
        }
    }

    /// `AIPlayer::buildSpecificBuildingNearestTeam` (`AIPlayer.cpp:2109-2205`): place a one-shot
    /// priority build order at the team's estimated position (`Team::getEstimateTeamPosition`,
    /// Team.cpp:2228: the position of the **first** member in the team's member list, which prepends
    /// on every join, `Object.cpp:896-898`), wiggled over growing squares when that spot is blocked.
    ///
    /// Two retail quirks are kept. The first probe is `NO_OBJECT_OVERLAP` only (no terrain test);
    /// the wiggle adds `TERRAIN_RESTRICTIONS`. `CLEAR_PATH` is passed to the wiggle but is a no-op
    /// with the null builder this function gives (`BuildAssistant.cpp:1080-1085`). And `valid` is
    /// only set inside the illegal branch, so a team standing on a legal spot gets **no** build
    /// order at all (AIPlayer.cpp:2204-2283). The `offset` vector at :2195-2199 is computed and
    /// never read; it is not modelled.
    pub fn ai_build_specific_nearest_team(&mut self, player: PlayerIndex, template: &str, team_pos: Coord3D) {
        let Some(t) = self.find_template(template) else { return };
        let diag = l174_nearest_diag();
        if diag {
            eprintln!(
                "ZR_L174_NEAREST f={} p={} tmpl={} pos=({:.4},{:.4}) geom={:?} major={} minor={}",
                self.frame, player, template, team_pos.x, team_pos.y, t.geometry.shape,
                t.geometry.major_radius, t.geometry.minor_radius
            );
        }
        let angle = t.placement_view_angle;
        let mut valid = false;
        let mut new_pos = team_pos;
        // Every probe keeps the team's z: `newPos` is copied from `*location` and only x and y change
        // (AIPlayer.cpp:2119-2202). The z feeds the bridge-layer test in TERRAIN_RESTRICTIONS
        // (`getLayerForDestination(worldPos)`, BuildAssistant.cpp:1036), so a ground z would be wrong there.
        let legal = |logic: &GameLogic, x: f32, y: f32, terrain: bool| {
            let p = Coord3D::new(x, y, team_pos.z);
            let options = crate::build::LegalBuildOptions { terrain, objects: crate::build::ObjectOverlap::All };
            logic.is_location_legal_with(&t, &p, angle, options, None, Some(player))
        };
        // `isLocationLegalToBuild(location, tTemplate, angle, NO_OBJECT_OVERLAP, nullptr, m_player)`
        // (AIPlayer.cpp:2206). A legal team spot leaves `valid` false: the retail code only assigns
        // the adjusted position inside this branch.
        if !legal(self, team_pos.x, team_pos.y, false) {
            let cell = crate::pathfind::PATHFIND_CELL_SIZE_F;
            let close = 20.0 * cell; // SUPPLY_CENTER_CLOSE_DIST (AIPlayer.cpp:66)
            let mut pos_offset = 0.0f32;
            'wiggle: while pos_offset < 2.0 * close {
                let offset = pos_offset / 2.0;
                let y_pos = team_pos.y - offset;
                let mut x_pos = team_pos.x - offset;
                while x_pos <= team_pos.x + offset {
                    // Top row, first cell: a hit breaks the row (`if (valid) break;`, AIPlayer.cpp:2230).
                    if legal(self, x_pos, y_pos, true) {
                        new_pos = Coord3D::new(x_pos, y_pos, 0.0);
                        valid = true;
                        break 'wiggle;
                    }
                    // The second cell of the row has **no** break in the C++ (`:2232-2237`): the row
                    // keeps scanning, so the last test's result wins and `newPos` ends at its x.
                    valid = legal(self, x_pos, y_pos + pos_offset, true);
                    if valid {
                        new_pos = Coord3D::new(x_pos, y_pos + pos_offset, 0.0);
                    }
                    x_pos += cell;
                }
                if valid {
                    break 'wiggle;
                }
                let x_pos = team_pos.x - offset;
                let mut y_pos = team_pos.y - offset;
                while y_pos <= team_pos.y + offset {
                    if legal(self, x_pos, y_pos, true) {
                        new_pos = Coord3D::new(x_pos, y_pos, 0.0);
                        valid = true;
                        break 'wiggle;
                    }
                    // Same missing break on the column's second cell (`:2256-2261`).
                    valid = legal(self, x_pos + pos_offset, y_pos, true);
                    if valid {
                        new_pos = Coord3D::new(x_pos + pos_offset, y_pos, 0.0);
                    }
                    y_pos += cell;
                }
                if valid {
                    break 'wiggle;
                }
                // The `for` increment: the next ring is four cells wider.
                pos_offset += 2.0 * cell;
            }
        }
        if valid {
            // `Player::addToPriorityBuildList(thingName, &newPos, angle)` (`Player.cpp:656`): a
            // prepended one-rebuild priority entry, `automaticallyBuild` true (`BuildListInfo` ctor).
            if diag {
                eprintln!("ZR_L174_NEAREST f={} chosen=({:.4},{:.4})", self.frame, new_pos.x, new_pos.y);
            }
            // revert-check: self.ai_prepend_priority_build(player, template, (new_pos.x, new_pos.y), angle);
            self.ai_prepend_priority_build(player, template, (new_pos.x, new_pos.y), angle);
        } else if diag {
            eprintln!("ZR_L174_NEAREST f={} no-legal-spot", self.frame);
        }
    }

    /// `AISkirmishPlayer::buildAIBaseDefense`: build the side's `BaseDefenseStructure1`. `paths` as
    /// [`Self::ai_build_base_structure`].
    pub fn ai_build_base_defense(&mut self, player: PlayerIndex, flank: bool, paths: &crate::script::ScriptEngine) {
        let template = self.ai_base_defense_template(player).unwrap_or_default();
        if !template.is_empty() {
            self.ai_build_base_structure(player, &template, flank, paths);
        }
    }

    fn ai_base_defense_template(&self, player: PlayerIndex) -> Option<String> {
        // The side's `BaseDefenseStructure1` (`SideInfo`, AIData.ini), copied onto the config by the
        // adapter. Empty when the side has none.
        self.ai_players
            .get(&player)
            .and_then(|ai| ai.config.base_defense.clone())
            .filter(|s| !s.is_empty())
    }

    /// `Player::addToPriorityBuildList` (`Player.cpp:656`): *prepend* a one-rebuild, priority build
    /// order (so every per-entry index shifts by one). Shared by `buildBySupplies` and
    /// `buildAIBaseDefenseStructure`.
    fn ai_prepend_priority_build(&mut self, player: PlayerIndex, template: &str, loc: (f32, f32), angle: f32) {
        let Some(ai) = self.ai_players.get_mut(&player) else { return };
        let n = ai.config.build_list.len();
        ai.built.resize(n, None);
        ai.destroyed_at.resize(n, 0);
        ai.supply.resize(n, SupplyInfo::default());
        ai.config.build_list.insert(
            0,
            AiBuildOrder { template: template.to_owned(), location: loc, angle, initially_built: false, rebuilds: 1, automatically_build: true },
        );
        ai.built.insert(0, None);
        ai.destroyed_at.insert(0, 0);
        ai.supply.insert(0, SupplyInfo::default());
        ai.priority = ai.priority.iter().map(|i| i + 1).collect();
        ai.priority.insert(0);
    }

    /// `AIPlayer::getPlayerStructureBounds` (`AIPlayer.cpp:3822`): the bounding box of `player`'s
    /// structures as `((lo.x, lo.y), (hi.x, hi.y))`, or `None` when it has none (the caller then
    /// falls back to the map extent). `conservative` is always false here (superweapon targeting).
    /// `thePlayer->isSkirmishAIPlayer()` (`BuildAssistant::isLocationClearOfObjects`): a player with an
    /// AI state in this sim.
    pub(crate) fn is_skirmish_ai_player(&self, player: PlayerIndex) -> bool {
        self.ai_players.contains_key(&player)
    }

    pub(crate) fn ai_player_structure_bounds(&self, player: PlayerIndex) -> Option<((f32, f32), (f32, f32))> {
        let mut lo = (f32::MAX, f32::MAX);
        let mut hi = (f32::MIN, f32::MIN);
        let mut any = false;
        for o in self.objects().filter(|o| o.player == player && !o.destroyed && o.is_kind_of(kind_of::STRUCTURE)) {
            lo.0 = lo.0.min(o.position.x);
            lo.1 = lo.1.min(o.position.y);
            hi.0 = hi.0.max(o.position.x);
            hi.1 = hi.1.max(o.position.y);
            any = true;
        }
        any.then_some((lo, hi))
    }

    /// `AIPlayer::getPlayerSuperweaponValue` (`AIPlayer.cpp:1319`): the weighted build-cost value of
    /// `player`'s objects within `radius` of `center`, falling off from the middle. With
    /// `include_military_units` false (a sneak attack) base defenses and combat units count
    /// *negatively* and command centers / superweapons count 5x; with it true, flying aircraft are
    /// skipped and command centers / superweapons count 1/10 (they cannot be destroyed).
    fn ai_player_superweapon_value(&self, center: &Coord3D, player: PlayerIndex, radius: f32, include_military_units: bool) -> i32 {
        let radius = radius.max(4.0 * crate::pathfind::PATHFIND_CELL_SIZE_F);
        let rad_sqr = radius * radius;
        let mut cash = 0.0f32;
        for o in self.objects().filter(|o| o.player == player && !o.destroyed) {
            let mut apply_neg = false;
            if !include_military_units {
                let tech_defense = o.template.collide_kinds & crate::template::collide_kind::TECH_BASE_DEFENSE != 0;
                let is_defense = o.is_kind_of(kind_of::FS_BASE_DEFENSE) || tech_defense;
                let is_combat_unit = (o.is_kind_of(kind_of::VEHICLE) || o.is_kind_of(kind_of::INFANTRY))
                    && !o.is_kind_of(kind_of::DOZER)
                    && !o.is_kind_of(kind_of::HARVESTER);
                if is_defense || is_combat_unit {
                    apply_neg = true;
                }
            } else if o.is_kind_of(kind_of::AIRCRAFT) {
                // `Object::isSignificantlyAboveTerrain`: don't target flying aircraft (airstrip ok).
                let g = self.terrain.surface_height(o.position.x, o.position.y, o.position.z);
                if o.position.z - g > 9.0 * -self.rules.gravity {
                    continue;
                }
            }
            let (dx, dy) = (center.x - o.position.x, center.y - o.position.y);
            if dx * dx + dy * dy < rad_sqr {
                let dist = (dx * dx + dy * dy).sqrt();
                let factor = 1.0 - dist / (2.0 * radius);
                let mut value = self.calc_cost_to_build(player, &o.template) as f32;
                if o.is_kind_of(kind_of::COMMANDCENTER) {
                    value = if include_military_units { value / 10.0 } else { value * 5.0 };
                }
                if o.is_kind_of(kind_of::FS_SUPERWEAPON) {
                    value = if include_military_units { value / 10.0 } else { value * 5.0 };
                }
                if apply_neg {
                    cash -= factor * value * 5.0;
                } else {
                    cash += factor * value;
                }
            }
        }
        cash as i32
    }

    /// `AIPlayer::computeSuperweaponTarget` (`AIPlayer.cpp:1166`) with the `AISkirmishPlayer`
    /// override for cluster mines (`AISkirmishPlayer.cpp:1134`): a coarse grid over the enemy
    /// structure bounds scored by [`Self::ai_player_superweapon_value`], then an 11x11 fine pass.
    /// Returns `None` when the power template is unknown or no location scores.
    ///
    /// `paths` is the script engine, for `m_player->getMpStartIndex()` (`player_start`, `+1` for the
    /// `Center<n>` label) and `TheTerrainLogic->getClosestWaypointOnPath`. It must be passed in: the
    /// only caller is a script action, and `script_engine_update` takes the engine out of
    /// `GameLogic`, so `self.scripts` is an empty engine there (the mines would always be aimed at
    /// the enemy base centre instead of the player's own approach path).
    pub(crate) fn ai_compute_superweapon_target(
        &mut self,
        power: &str,
        player: PlayerIndex,
        enemy: PlayerIndex,
        weapon_radius: f32,
        paths: &crate::script::ScriptEngine,
    ) -> Option<Coord3D> {
        let kind = self.special_power_kind(power).to_string();
        if kind == "SPECIAL_CLUSTER_MINES" || kind == "NUKE_SPECIAL_CLUSTER_MINES" {
            // Hackus brutus: mine the entrances to our own base.
            let mode = self.rng_mut().value(0, 2);
            let label = match mode {
                1 => "Flank",
                2 => "Backdoor",
                _ => "Center",
            };
            let mp_ndx = paths.player_start.get(&player).copied().unwrap_or(0) + 1;
            let path_label = format!("{label}{mp_ndx}");
            let center = self.ai_players.get(&player).map_or((0.0, 0.0), |a| a.base_center);
            let mut goal = Coord3D::new(center.0, center.1, 0.0);
            if let Some(p) = paths.closest_waypoint_on_path_pos(&goal, &path_label) {
                goal = p;
            } else {
                let (lo, hi) = self.ai_player_structure_bounds(enemy).unwrap_or(((0.0, 0.0), (0.0, 0.0)));
                goal.x = lo.0 + (hi.0 - lo.0) * 0.5;
                goal.y = lo.1 + (hi.1 - lo.1) * 0.5;
            }
            let (mut ox, mut oy) = (goal.x - center.0, goal.y - center.1);
            let len = (ox * ox + oy * oy).sqrt();
            if len > 0.0 {
                ox /= len;
                oy /= len;
            } else {
                ox = 0.0;
                oy = 0.0;
            }
            let r = self.ai_players.get(&player).map_or(0.0, |a| a.base_radius);
            let (x, y) = (center.0 + ox * r, center.1 + oy * r);
            return Some(Coord3D::new(x, y, self.terrain.ground_height(x, y)));
        }

        let (mut lo, mut hi) = self.ai_player_structure_bounds(enemy).unwrap_or(((0.0, 0.0), (0.0, 0.0)));
        if lo.0 == 0.0 && hi.0 == 0.0 && lo.1 == 0.0 && hi.1 == 0.0 {
            // Degenerate bounds because he has no buildings: scan the whole map for leftovers.
            let (w, h) = self.terrain.world_size();
            lo = (0.0, 0.0);
            hi = (w, h);
        }
        let weapon_radius = weapon_radius.max(1.0);
        // The engine shrinks only x by the weapon radius (y is left alone).
        lo.0 += weapon_radius;
        hi.0 -= weapon_radius;
        if hi.0 < lo.0 {
            let m = (hi.0 + lo.0) * 0.5;
            hi.0 = m;
            lo.0 = m;
        }
        if hi.1 < lo.1 {
            let m = (hi.1 + lo.1) * 0.5;
            hi.1 = m;
            lo.1 = m;
        }
        let width = hi.0 - lo.0;
        let height = hi.1 - lo.1;
        let x_count = ((width / weapon_radius).ceil() as i32 + 1).min(10);
        let y_count = ((height / weapon_radius).ceil() as i32 + 1).min(10);
        let target_military_units = kind != "SPECIAL_SNEAK_ATTACK";
        // Randomize which way we iterate the grid.
        let (x_delta, y_delta, x_start, y_start) = match self.rng_mut().value(1, 4) {
            1 => (1, 1, 0, 0),
            2 => (-1, 1, x_count, 0),
            3 => (1, -1, 0, y_count),
            _ => (-1, -1, x_count, y_count),
        };
        let mut cash = -1;
        let mut best_pos = Coord3D::ZERO;
        let mut success = false;
        let mut x_index = x_start;
        for _ in 0..x_count {
            let mut y_index = y_start;
            for _ in 0..y_count {
                let pos = Coord3D::new(
                    lo.0 + width * x_index as f32 / x_count as f32,
                    lo.1 + height * y_index as f32 / y_count as f32,
                    0.0,
                );
                let cur = self.ai_player_superweapon_value(&pos, enemy, 2.0 * weapon_radius, target_military_units);
                if cur > cash {
                    cash = cur;
                    best_pos = pos;
                    success = true;
                }
                y_index += y_delta;
            }
            x_index += x_delta;
        }
        if !success {
            return None;
        }
        // Fine-tune around `best_pos` (the engine uses `x` for both axes here).
        let mut very_best = Coord3D::ZERO;
        cash = -1;
        let mut count = 0;
        for x in 0..11 {
            for _ in 0..11 {
                let pos = Coord3D::new(
                    best_pos.x + (x as f32 - 5.0) * (weapon_radius / 10.0),
                    best_pos.y + (x as f32 - 5.0) * (weapon_radius / 10.0),
                    0.0,
                );
                let cur = self.ai_player_superweapon_value(&pos, enemy, weapon_radius, target_military_units);
                if cur > cash {
                    cash = cur;
                    very_best = pos;
                    count = 1;
                } else if cur == cash {
                    very_best.x += pos.x;
                    very_best.y += pos.y;
                    count += 1;
                }
            }
        }
        if count > 1 {
            very_best.x /= count as f32;
            very_best.y /= count as f32;
        }
        very_best.z = self.terrain.ground_height(very_best.x, very_best.y);
        (cash > -1).then_some(very_best)
    }

    /// `ScriptActions::doSkirmishFireSpecialPowerAtMostCost` (`ScriptActions.cpp:4232`): find the
    /// player's first ready `power` module, compute the most costly target location for `enemy` via
    /// [`Self::ai_compute_superweapon_target`], and fire at it. `enemy` is the current script
    /// player's `getSkirmishEnemyPlayer` (the engine returns early when it is null).
    ///
    /// `paths` is the script engine the action runs on (see
    /// [`Self::ai_compute_superweapon_target`]); the fire also records its
    /// `PLAYER_TRIGGERED/COMPLETED_SPECIAL_POWER` events on it, because `GameLogic::scripts` is
    /// empty while a script action runs.
    pub fn ai_fire_special_power_at_most_cost(
        &mut self,
        player: PlayerIndex,
        enemy: Option<PlayerIndex>,
        power: &str,
        paths: &mut crate::script::ScriptEngine,
    ) {
        let Some(enemy) = enemy else { return };
        if !self.special_powers.contains_key(power) {
            return; // `TheSpecialPowerStore->findSpecialPowerTemplate` returned null.
        }
        let radius = 50.0f32.max(self.special_power_radius_cursor_radius(power));
        let sources: Vec<ObjectId> = self
            .objects()
            .filter(|o| o.player == player && !o.destroyed && o.template.special_powers.iter().any(|sp| sp.power == power))
            .map(|o| o.id)
            .collect();
        for source in sources {
            // `if (!mod->isReady()) continue;` (ScriptActions.cpp:4271). `isReady` also starts the player's
            // shared timer when there is none (`getOrStartSpecialPowerReadyFrame`), which the fire below resets.
            self.special_power_start_shared_timer(source, power);
            if !self.special_power_is_ready(source, power) {
                continue;
            }
            if let Some(loc) = self.ai_compute_superweapon_target(power, player, enemy, radius, paths) {
                let non_zero = loc.x * loc.x + loc.y * loc.y + loc.z * loc.z > 0.0;
                // `mod->doSpecialPowerAtLocation(&location, INVALID_ANGLE, COMMAND_FIRED_BY_SCRIPT)`
                // (ScriptActions.cpp:4295): a forced script fire, after the `isReady` test above.
                if non_zero && self.script_do_special_power_at_location(player, source, power, loc, paths) {
                    if let Some(ai) = self.ai_players.get_mut(&player) {
                        ai.powers_fired += 1;
                    }
                }
            }
            break;
        }
    }

    /// `AIPlayer::doTeamBuilding` (reduced): every `TeamSeconds`, queue an attack group from the
    /// war factory; when it is complete, send it at the nearest enemy structure.
    fn ai_process_teams(&mut self, player: PlayerIndex, config: &SkirmishAiConfig, state: &mut SkirmishAi) {
        // Drop dead group members.
        state.attack_group.retain(|id| self.find_object(*id).is_some());
        // Pick up any idle combat units of the group's templates (newly produced).
        let idle: Vec<ObjectId> = self
            .objects
            .values()
            .filter(|o| {
                !o.destroyed
                    && o.player == player
                    && config.attack_units.contains(&o.template.name)
                    && o.combat.attack.is_none()
                    && !state.attack_group.contains(&o.id)
            })
            .map(|o| o.id)
            .collect();
        state.attack_group.extend(idle);

        if state.attack_group.len() >= config.attack_units.len().max(1) {
            self.ai_launch_attack(player, state);
            return;
        }
        if state.team_timer > 0 {
            state.team_timer -= 1;
            return;
        }
        if config.attack_units.is_empty() {
            return;
        }
        let want = &config.attack_units[state.attack_group.len().min(config.attack_units.len() - 1)];
        if self.ai_queue_from(player, &config.attack_factory, want, false) {
            state.team_timer = config.team_seconds;
        } else {
            state.team_timer = config.team_seconds / 2;
        }
    }

    /// Send the current attack group at the nearest enemy structure (`AIGroup` attack move).
    fn ai_launch_attack(&mut self, player: PlayerIndex, state: &mut SkirmishAi) {
        let Some(target) = self.ai_nearest_enemy_structure(player) else { return };
        let ids: Vec<ObjectId> = std::mem::take(&mut state.attack_group);
        for id in ids {
            if let Some(o) = self.objects.get_mut(&id) {
                o.combat.attack = Some(crate::combat::AttackOrder::new(crate::combat::AttackTarget::Object(target)));
                o.ai.stop();
            }
            self.wake_up_now(id);
        }
        state.attacks_launched += 1;
    }

    // ---- helpers ---------------------------------------------------------------------------

    /// `AIPlayer::checkForSupplyCenter` (`AIPlayer.cpp:188`): a completed supply center becomes a
    /// supply building wanting `ResourceGatherers<difficulty> + 1` gatherers ("a freebie with the
    /// supply depots"), none counted yet (`current = -1`).
    fn ai_check_for_supply_center(&self, state: &mut SkirmishAi, i: usize, bldg: ObjectId) {
        let Some(o) = self.objects.get(&bldg) else { return };
        if !o.template.dock.as_ref().is_some_and(|d| d.kind == crate::supply::DockKind::Center) {
            return;
        }
        if state.supply.len() <= i {
            state.supply.resize(i + 1, SupplyInfo::default());
        }
        state.supply[i] = SupplyInfo { is_supply: true, desired: state.config.resource_gatherers + 1, current: -1, object: Some(bldg) };
    }

    /// `AIPlayer::onStructureProduced` (`AIPlayer.cpp:113-121`), reduced to what the economy needs:
    /// the build-list entry that made `bldg` runs `checkForSupplyCenter`, and both 2-second ticks are
    /// forced (`m_teamDelay = 0; m_buildDelay = 0`) so `queueSupplyTruck` sees the new center before
    /// its free truck spawns and base building runs its next attempt immediately.
    pub(crate) fn ai_on_structure_produced(&mut self, bldg: ObjectId) {
        let Some(player) = self.objects.get(&bldg).map(|o| o.player) else { return };
        let Some(mut state) = self.ai_players.get(&player).cloned() else { return };
        for i in 0..state.built.len() {
            if state.built[i] == Some(bldg) {
                self.ai_check_for_supply_center(&mut state, i, bldg);
                continue;
            }
            // `AIPlayer::onStructureProduced` (AIPlayer.cpp:150-178): the entry may still point at the
            // GLA rebuild hole whose reconstruction just finished; adopt the new building.
            let Some(entry) = state.built[i] else { continue };
            if self.objects.get(&entry).is_some_and(|o| {
                o.is_kind_of(kind_of::REBUILD_HOLE) && o.behavior.hole_reconstructing == Some(bldg)
            }) {
                state.built[i] = Some(bldg);
                self.ai_check_for_supply_center(&mut state, i, bldg);
            }
        }
        // `m_teamDelay = 0; // Cause the update queues & selection to happen immediately.` and
        // `m_buildDelay = 0;` (`AIPlayer.cpp:116-117`).
        state.team_delay = 0;
        state.build_delay = 0;
        self.ai_players.insert(player, state);
    }

    /// The gatherer part of `AIPlayer::onUnitProduced` (`AIPlayer.cpp:1054`): a unit that comes out
    /// of `factory` completes the matching gatherer work order, is forced into the Wanting state and
    /// is sent to *every* supply building that still wants gatherers with `aiDock(center,
    /// CMD_FROM_PLAYER)` (the last one wins, as in the loop at :1122, which has no `break`).
    /// Other production leaves the supply truck AI alone (`setForceWantingState(false)`).
    pub(crate) fn ai_on_unit_produced(&mut self, factory: ObjectId, unit: ObjectId) {
        let Some(player) = self.objects.get(&factory).map(|o| o.player) else { return };
        let Some(mut state) = self.ai_players.get(&player).cloned() else { return };
        let Some(uname) = self.objects.get(&unit).map(|o| o.template.name.clone()) else { return };
        // `AIPlayer::onUnitProduced` (`AIPlayer.cpp:1144-1153`): a produced dozer that is not a
        // resource gatherer (`supplyTruck` false) restarts structure building at once
        // (`m_buildDelay = 0; m_structureTimer = 1`). (`m_dozerQueuedForRepair` / `m_repairDozer`
        // is the repair-dozer feature, not ported.) A gatherer-order truck skips it (`supplyTruck`).
        let is_dozer = self.objects.get(&unit).is_some_and(|o| o.is_kind_of(kind_of::DOZER));
        // `AIPlayer::onUnitProduced` (`AIPlayer.cpp:1080-1108`): the first queued team with a work
        // order on this factory for this unit type takes it: `numCompleted++`, `setTeam`, and with a
        // home location the unit walks there; the order releases the factory.
        for qi in 0..state.team_build_queue.len() {
            let Some(oi) = state.team_build_queue[qi]
                .work_orders
                .iter()
                .position(|w| w.factory == Some(factory) && w.completed < w.required_count && w.thing == uname)
            else {
                continue;
            };
            let key = state.team_build_queue[qi].key.clone();
            // `order->m_isResourceGatherer` (AIPlayer.cpp:1080-1090): read before the order is released.
            let gatherer = state.team_build_queue[qi].work_orders[oi].resource_gatherer;
            state.team_build_queue[qi].work_orders[oi].completed += 1;
            state.team_build_queue[qi].work_orders[oi].factory = None;
            if state.team_build_queue[qi].reinforcement {
                state.team_build_queue[qi].reinforcement_id = Some(unit);
            }
            let mut engine = std::mem::take(&mut self.scripts);
            engine.set_object_team(self, unit, &key);
            self.scripts = engine;
            let home =self.scripts.team_proto(&key).and_then(|p| self.scripts.waypoints.get(&p.home).copied());
            if let Some(pos) = home {
                // `ai->aiMoveToPosition(&home, CMD_FROM_AI)` (`AIPlayer.cpp:1093`).
                self.ai_move_to_position_cmd(unit, pos, crate::ai_states::CommandSource::Ai);
            }
            // `AIPlayer::onUnitProduced` (`AIPlayer.cpp:1116-1141`): `supplyTruck` is the order's
            // `m_isResourceGatherer` for a supply truck and false otherwise. A gatherer is forced into the
            // Wanting state and docked at the supply buildings that still want gatherers; any other truck
            // gets `setForceWantingState(false)`. A dozer with `supplyTruck` false restarts the structure
            // timer at once (`AIPlayer.cpp:1159`).
            let is_truck = self.objects.get(&unit).is_some_and(|o| o.template.supply_truck.is_some());
            let supply_truck = is_truck && gatherer;
            if is_truck {
                self.set_supply_force_wanting(unit, supply_truck);
                if supply_truck {
                    self.ai_dock_new_gatherer(unit, &mut state);
                }
            }
            if !supply_truck && is_dozer {
                state.build_delay = 0;
                state.structure_timer = 1;
            }
            state.team_delay = 0;
            self.ai_players.insert(player, state);
            return;
        }
        if is_dozer {
            state.build_delay = 0;
            state.structure_timer = 1;
        }
        state.team_delay = 0; // `m_teamDelay = 0` on every produced unit.
        self.ai_players.insert(player, state);
    }

    /// The docking loop of `AIPlayer::onUnitProduced` (`AIPlayer.cpp:1122-1141`): a new gatherer is sent to
    /// every supply building that still wants gatherers (`desired > current`) with `aiDock(center,
    /// CMD_FROM_PLAYER)`, and that building's `currentGatherers` is counted up. The last one wins (the loop
    /// has no `break`).
    fn ai_dock_new_gatherer(&mut self, unit: ObjectId, state: &mut SkirmishAi) {
        for i in 0..state.supply.len() {
            if state.supply[i].is_supply && state.supply[i].desired > 0 && state.supply[i].desired > state.supply[i].current {
                if let Some(center) = state.built.get(i).copied().flatten().filter(|c| self.objects.contains_key(c)) {
                    state.supply[i].current += 1;
                    self.ai_dock_command(unit, center, crate::ai_states::CommandSource::Player);
                }
            }
        }
    }

    /// `AIPlayer::findFactory` (`AIPlayer.cpp:1437`): a build-list building (not under construction,
    /// not being sold) that can make `unit`; an idle one is preferred, else with `busy_ok` the last
    /// busy one. The sim has no command sets, so "can make" is the unit's prerequisite objects (or the
    /// configured `gatherer_factory`).
    fn ai_find_build_list_factory(&self, state: &SkirmishAi, unit: &str, busy_ok: bool) -> Option<ObjectId> {
        let prereqs: Vec<String> = self
            .find_template(unit)
            .map(|t| {
                t.prerequisites
                    .iter()
                    .flat_map(|p| match p {
                        crate::production::Prerequisite::AnyOfObjects(names) => names.clone(),
                        crate::production::Prerequisite::Science(_) => Vec::new(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let named = &state.config.gatherer_factory;
        let mut busy: Option<ObjectId> = None;
        for b in state.built.iter().flatten() {
            let Some(o) = self.objects.get(b) else { continue };
            if o.destroyed || o.under_construction || o.production.is_none() {
                continue;
            }
            // `BuildAssistant::isPossibleToMakeUnit`: the factory's command set builds `unit`. Without
            // command-set data (hand-built test logic) fall back to the prerequisite objects.
            let can_make = match self.template_source.as_ref().and_then(|s| s.can_make_unit(&o.template.name, unit)) {
                Some(b) => b,
                None if !named.is_empty() => o.template.name == *named,
                None => prereqs.iter().any(|n| *n == o.template.name),
            };
            if !can_make {
                continue;
            }
            if o.production.as_ref().is_some_and(|p| p.queue.is_empty()) {
                return Some(o.id);
            }
            busy = Some(o.id);
        }
        if busy_ok {
            busy
        } else {
            None
        }
    }

    /// `AIPlayer::queueSupplyTruck` (`AIPlayer.cpp:223`). Its order is a `TeamInQueue` on the default team in the
    /// team build queue (`queueUnits` then recruits or trains it, `ai_queue_units`). Ported per supply building: a building
    /// with enough gatherers re-counts them (re-docking idle ones); one that is short re-attaches a
    /// loose gatherer or queues one more truck (the first is free: `current == -1` makes the order
    /// wait for the center's own spawned truck, `m_factoryID` = the factory).
    pub(crate) fn ai_queue_supply_truck(&mut self, player: PlayerIndex, state: &mut SkirmishAi) {
        let n = state.config.build_list.len();
        if state.supply.len() < n {
            state.supply.resize(n, SupplyInfo::default());
        }
        // A center that finished without `onStructureProduced` (placed complete) is picked up here.
        for i in 0..n {
            if let Some(b) = state.built.get(i).copied().flatten() {
                if state.supply[i].object != Some(b) && self.objects.get(&b).is_some_and(|o| !o.under_construction && !o.destroyed) {
                    self.ai_check_for_supply_center(state, i, b);
                }
            }
        }
        // `AIPlayer::queueSupplyTruck` (AIPlayer.cpp:226-233): a supply truck's work order (`m_isResourceGatherer`)
        // anywhere in the team build queue means one is already being built. The order itself is worked by
        // `queueUnits` (recruit, then `startTraining`), which runs right after this in `ai_queue_units`.
        if state.team_build_queue.iter().any(|q| q.work_orders.iter().any(|w| w.resource_gatherer)) {
            return;
        }
        let gatherer_ids = |logic: &GameLogic| -> Vec<ObjectId> {
            logic
                .objects
                .values()
                .filter(|o| !o.destroyed && o.player == player && o.is_kind_of(kind_of::HARVESTER) && o.template.supply_truck.is_some())
                .map(|o| o.id)
                .collect()
        };
        let total_harvesters = gatherer_ids(self).len() as i32;
        let close_dist = 20.0 * crate::pathfind::PATHFIND_CELL_SIZE_F;
        for i in 0..n {
            if !state.supply[i].is_supply {
                continue;
            }
            let (desired, cur) = (state.supply[i].desired, state.supply[i].current);
            let center = state.built.get(i).copied().flatten().filter(|c| self.objects.contains_key(c));
            if cur >= desired {
                // Check & see if any have died.
                let Some(center) = center else { continue };
                let Some(c) = self.objects.get(&center) else { continue };
                if c.is_kind_of(kind_of::REBUILD_HOLE) {
                    continue; // don't consider rebuild holes.
                }
                // Make sure there are supplies near the center: the closest non-own supply source
                // within `SUPPLY_CENTER_CLOSE_DIST` + the center's radius.
                let radius = close_dist + c.template.geometry.bounding_circle_radius();
                let cpos = c.position;
                let mut src: Option<(f32, ObjectId)> = None;
                for o in self.objects.values() {
                    if o.destroyed || o.player == player || !o.is_kind_of(kind_of::SUPPLY_SOURCE) {
                        continue;
                    }
                    let d = (cpos.dist_2d(&o.position) - o.template.geometry.bounding_circle_radius()).max(0.0);
                    if d <= radius && src.is_none_or(|(bd, _)| d < bd) {
                        src = Some((d, o.id));
                    }
                }
                let Some((_, src)) = src else { continue }; // No supplies.
                if let Some(s) = self.objects.get(&src) {
                    if s.template.dock.as_ref().is_some_and(|d| d.kind == crate::supply::DockKind::Warehouse) {
                        if i64::from(s.boxes) * i64::from(self.rules.value_per_supply_box) <= 0 {
                            continue;
                        }
                        if self.relationship(player, s.player) == crate::damage::Relationship::Enemies {
                            continue;
                        }
                    }
                }
                // Ok, it has supplies available near it: `checkForSupplyCenter` (resets), then count the
                // harvesters servicing this center and re-dock the idle ones (`CMD_FROM_PLAYER`).
                self.ai_check_for_supply_center(state, i, center);
                let mut count = 0;
                for id in gatherer_ids(self) {
                    let Some(o) = self.objects.get(&id) else { continue };
                    if o.supply_ai.preferred_dock == center {
                        count += 1;
                        let ferrying = matches!(o.supply_ai.state, crate::supply::SupplyState::Wanting | crate::supply::SupplyState::Docking);
                        if !ferrying {
                            self.ai_dock_command(id, center, crate::ai_states::CommandSource::Player);
                        }
                    }
                }
                state.supply[i].current = count;
            } else {
                // "Loose" harvesters (the supply center got nuked): ferrying or forced wanting, with no
                // living preferred dock, are re-attached to this building, one per call.
                for id in gatherer_ids(self) {
                    let Some(o) = self.objects.get(&id) else { continue };
                    if self.objects.contains_key(&o.supply_ai.preferred_dock) {
                        continue;
                    }
                    let ferrying = matches!(o.supply_ai.state, crate::supply::SupplyState::Wanting | crate::supply::SupplyState::Docking);
                    if ferrying || o.supply_ai.force_wanting {
                        if let Some(center) = center {
                            state.supply[i].current += 1;
                            self.ai_dock_command(id, center, crate::ai_states::CommandSource::Player);
                            return;
                        }
                    }
                }
                if total_harvesters >= desired * 3 {
                    continue; // we got lotsa gatherers.
                }
                // Scan every `KINDOF_HARVESTER` template (`firstTemplate` order) for the first one an
                // idle build-list factory can make.
                let mut harvesters = self.template_source.as_ref().map_or_else(Vec::new, |s| s.harvester_templates());
                if harvesters.is_empty() {
                    harvesters.push(state.config.gatherer_template.clone());
                }
                for thing in harvesters {
                    let Some(f) = self.ai_find_build_list_factory(state, &thing, false) else { continue };
                    // `AIPlayer::queueSupplyTruck` (AIPlayer.cpp:385-410): a `TeamInQueue` with one required
                    // `WorkOrder` (`m_isResourceGatherer`), `m_priorityBuild`, `m_frameStarted` = now, on the
                    // player's default team (`m_team = getDefaultTeam()`), prepended to the team build queue.
                    let Some(team) = self.scripts.player_default_team(player).map(str::to_owned) else { break };
                    let mut order = WorkOrder { thing: thing.clone(), factory: None, completed: 0, required_count: 1, required: true, resource_gatherer: true };
                    if state.supply[i].current == -1 {
                        // First one is automatic (AIPlayer.cpp:402-405): the center's own spawn is this truck.
                        order.factory = Some(f);
                        state.supply[i].current = 0;
                    } else if let Some(f) = self.ai_find_build_list_factory(state, &thing, true) {
                        // `startTraining(order, m_priorityBuild = true, ...)` (AIPlayer.cpp:407): a busy factory is fine.
                        if self.queue_unit(player, f, &thing) {
                            order.factory = Some(f);
                        }
                    }
                    state.team_build_queue.insert(
                        0,
                        TeamInQueue {
                            key: team,
                            frame_started: self.frame,
                            priority_build: true,
                            reinforcement: false,
                            reinforcement_id: None,
                            sent_to_start_location: false,
                            work_orders: vec![order],
                        },
                    );
                    state.team_delay = 0;
                    break;
                }
            }
        }
    }


    /// Queue `unit` from the first owned factory matching `factory_name` (or any factory). With
    /// `unique`, skip if the factory is already producing that unit (`AIPlayer::queueSupplyTruck`).
    /// `AIPlayer::findFactory` (reduced): an owned, finished production building that can make
    /// `unit`. The engine consults the factory's command set; the sim has no command-set mapping,
    /// so use the unit's `Prerequisites` object names (the barracks/war factory/airfield it is
    /// built from), else any production building. `factory_name` forces a template.
    fn ai_find_factory(&self, player: PlayerIndex, factory_name: &str, unit: &str) -> Option<ObjectId> {
        let is_factory = |o: &&crate::object::Object| {
            !o.destroyed && !o.under_construction && o.player == player && o.production.is_some()
        };
        if !factory_name.is_empty() {
            return self.objects.values().find(|o| is_factory(o) && o.template.name == factory_name).map(|o| o.id);
        }
        let prereqs: Vec<String> = self
            .find_template(unit)
            .map(|t| {
                t.prerequisites
                    .iter()
                    .flat_map(|p| match p {
                        crate::production::Prerequisite::AnyOfObjects(names) => names.clone(),
                        crate::production::Prerequisite::Science(_) => Vec::new(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(f) = self.objects.values().find(|o| is_factory(o) && prereqs.iter().any(|n| n == &o.template.name)) {
            return Some(f.id);
        }
        self.objects.values().find(|o| is_factory(o)).map(|o| o.id)
    }

    fn ai_queue_from(&mut self, player: PlayerIndex, factory_name: &str, unit: &str, unique: bool) -> bool {
        let Some(factory) = self.ai_find_factory(player, factory_name, unit) else { return false };
        if unique {
            if let Some(o) = self.objects.get(&factory) {
                if o.production.as_ref().is_some_and(|p| {
                    p.queue.iter().any(|e| matches!(&e.kind, crate::production::ProductionKind::Unit(u) if u.name == unit))
                }) {
                    return false;
                }
            }
        }
        self.queue_unit(player, factory, unit)
    }

    /// `AIPlayer::findDozer` (`AIPlayer.cpp:3248`): the closest idle dozer. A worker gathering
    /// supplies is not stolen (`SupplyTruckAIUpdate::isCurrentlyFerryingSupplies` =
    /// `ST_WANTING`/`ST_DOCKING`, or `isForcedIntoWantingState`), and a dozer already building is
    /// skipped. Falls back to any non-building dozer when none is idle; the caller queues one when
    /// this returns `None` and the base is underpowered.
    fn ai_find_dozer(&mut self, player: PlayerIndex, pos: &Coord3D, state: &mut SkirmishAi) -> Option<ObjectId> {
        let mut need_dozer = true;
        let mut idle: Option<(f32, ObjectId)> = None;
        let mut fallback: Option<ObjectId> = None;
        for o in self.objects.values() {
            if o.destroyed || o.is_dying() || o.player != player || !o.is_kind_of(kind_of::DOZER) {
                continue;
            }
            let any_task = o.build_target.is_some() || o.repair_target.is_some();
            // The ferry / forced-wanting skip (`!isAnyTaskPending() && supplyTruckAI`) comes before
            // `needDozer = false`, so a worker that is only gathering does not count as a dozer.
            if o.template.supply_truck.is_some() && !any_task {
                let ferrying = matches!(
                    o.supply_ai.state,
                    crate::supply::SupplyState::Wanting | crate::supply::SupplyState::Docking
                );
                if ferrying || o.supply_ai.force_wanting {
                    continue; // don't steal a gathering worker.
                }
            }
            need_dozer = false; // dozer exists, may be busy.
            if o.build_target.is_some() {
                continue; // already building.
            }
            if !any_task {
                let d = o.position.dist_2d(pos);
                if idle.is_none_or(|(bd, _)| d < bd) {
                    idle = Some((d, o.id));
                }
            }
            fallback.get_or_insert(o.id);
        }
        // `if (needDozer) queueDozer();` (`AIPlayer.cpp:3340`): every dozer is a gathering worker.
        if need_dozer {
            self.ai_queue_dozer(player, state);
        }
        idle.map(|(_, id)| id).or(fallback)
    }

    /// `AIPlayer::dozerInQueue` (AIPlayer.cpp:3169): a team in the build queue includes a dozer
    /// (`TeamInQueue::includesADozer`, AIPlayer.cpp:3584: a KINDOF_DOZER order that is not a resource
    /// gatherer, since a GLA worker is both).
    fn ai_dozer_in_queue(&self, state: &SkirmishAi) -> bool {
        state.team_build_queue.iter().any(|q| {
            q.work_orders
                .iter()
                .any(|w| !w.resource_gatherer && self.find_template(&w.thing).is_some_and(|t| t.is_kind_of(kind_of::DOZER)))
        })
    }

    /// `AIPlayer::queueDozer` (AIPlayer.cpp:3189-3236): orders one dozer as a priority `TeamInQueue` on the
    /// player's default team (`m_team = getDefaultTeam()`, `m_frameStarted` = now), prepended to the team build
    /// queue, with one required work order that is not a resource gatherer. The order is started at a
    /// build-list factory with `startTraining(order, busyOK = true)`; a busy factory is fine. Nothing is queued
    /// while a dozer order is already in the build queue. Not ported: C++ takes the first `KINDOF_DOZER` template
    /// in `firstTemplate` order that a factory can make, but `TemplateSource` has no dozer list, so the candidates
    /// here are the player's own dozer templates in object-id order (the faction's dozer while the AI has one).
    /// `m_player->setCanBuildUnits(true)` (AIPlayer.cpp:3204) is not modelled: the port has no such flag.
    fn ai_queue_dozer(&mut self, player: PlayerIndex, state: &mut SkirmishAi) -> bool {
        if self.ai_dozer_in_queue(state) {
            return false;
        }
        // `TheThingFactory->firstTemplate()` walk (AIPlayer.cpp:3198-3200): every KINDOF_DOZER template, newest first.
        let candidates: Vec<String> = self.template_source.as_ref().map_or_else(Vec::new, |s| s.dozer_templates());
        let Some(team) = self.scripts.player_default_team(player).map(str::to_owned) else { return false };
        for thing in candidates {
            // `findFactory(tTemplate, true)` (AIPlayer.cpp:3214): the first template a build-list factory can make.
            if self.ai_find_build_list_factory(state, &thing, true).is_none() {
                continue;
            }
            // `prependTo_TeamBuildQueue` (AIPlayer.cpp:3218-3225), then `m_teamDelay = 0` (:3230).
            state.team_build_queue.insert(
                0,
                TeamInQueue {
                    key: team,
                    frame_started: self.frame,
                    priority_build: true,
                    reinforcement: false,
                    reinforcement_id: None,
                    sent_to_start_location: false,
                    work_orders: vec![WorkOrder { thing: thing.clone(), factory: None, completed: 0, required_count: 1, required: true, resource_gatherer: false }],
                },
            );
            state.team_delay = 0;
            // `startTraining(order, m_priorityBuild, ...)` (AIPlayer.cpp:3233): the factory is searched again.
            if let Some(f) = self.ai_find_build_list_factory(state, &thing, true) {
                if self.queue_unit(player, f, &thing) {
                    state.team_build_queue[0].work_orders[0].factory = Some(f);
                }
            }
            return true;
        }
        false
    }

    fn ai_nearest_enemy_structure(&self, player: PlayerIndex) -> Option<ObjectId> {
        let me = self.objects.values().find(|o| !o.destroyed && o.player == player)?;
        let from = me.position;
        let mut best: Option<(f32, ObjectId)> = None;
        for o in self.objects.values() {
            if o.destroyed || o.player == player || o.template.kind_of & kind_of::STRUCTURE == 0 {
                continue;
            }
            if self.relationship(player, o.player) != crate::damage::Relationship::Enemies {
                continue;
            }
            let d = from.dist_2d(&o.position);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, o.id));
            }
        }
        best.map(|(_, id)| id)
    }
}

/// The AI player states live in `GameLogic`; this alias keeps the field type readable.
pub(crate) type AiPlayerMap = BTreeMap<PlayerIndex, SkirmishAi>;

/// Lane 133, read-only: `ZR_L133_BUILD=<template>` traces the skirmish AI's build-site choice for
/// that structure (`ai_process_base_building` plan, `buildStructureWithDozer`'s spiral candidates
/// and their legality tests). Diagnostics only; no simulation state is read or written beyond the
/// values the caller already computed.
pub(crate) fn l133_build_diag(template: &str) -> bool {
    static NAME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    NAME.get_or_init(|| std::env::var("ZR_L133_BUILD").ok())
        .as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case(template))
}

/// `ZR_L174_NEAREST`: read-only diagnostic for `AI_PLAYER_BUILD_TYPE_NEAREST_TEAM`
/// (`AIPlayer::buildSpecificBuildingNearestTeam`, AIPlayer.cpp:2109-2205).
pub(crate) fn l174_nearest_diag() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("ZR_L174_NEAREST").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::{TeamPrototype, TeamUnitInfo};

    fn proto() -> TeamPrototype {
        let d = zr_map::Dict(vec![("teamName".into(), zr_map::DictValue::Ascii("t".into()))]);
        let mut p = TeamPrototype::from_dict(&d).unwrap();
        p.units = vec![
            TeamUnitInfo { template: "A".into(), min: 1, max: 3 },
            TeamUnitInfo { template: "B".into(), min: 2, max: 2 },
        ];
        p
    }

    /// `buildSpecificAITeam` work orders: required (min) orders first, last slot first, then the
    /// optional (max - min) ones; `isMinimumBuilt` counts a unit in production, `isAllBuilt` doesn't.
    #[test]
    fn work_orders_follow_build_specific_ai_team() {
        let orders = TeamInQueue::orders_for(&proto());
        let shape: Vec<(&str, i32, bool)> = orders.iter().map(|w| (w.thing.as_str(), w.required_count, w.required)).collect();
        assert_eq!(shape, vec![("B", 2, true), ("A", 1, true), ("A", 2, false)]);
        let mut q = TeamInQueue {
            key: "t#1".into(),
            frame_started: 0,
            priority_build: false,
            reinforcement: false,
            reinforcement_id: None,
            sent_to_start_location: false,
            work_orders: orders,
        };
        assert!(!GameLogic::ai_team_minimum_built(&q));
        q.work_orders[0].completed = 1;
        q.work_orders[0].factory = Some(ObjectId(7)); // one more in production counts.
        q.work_orders[1].completed = 1;
        assert!(GameLogic::ai_team_minimum_built(&q));
        assert!(!GameLogic::ai_team_all_built(&q));
        q.work_orders[0].completed = 2;
        q.work_orders[2].completed = 2;
        assert!(GameLogic::ai_team_all_built(&q));
    }

    /// `AIPlayer::onStructureProduced` (`AIPlayer.cpp:113-121`) forces both 2-second ticks:
    /// `m_teamDelay = 0` and `m_buildDelay = 0`. The port only did the team one, so a finished
    /// structure could leave base building asleep for up to two seconds.
    #[test]
    fn on_structure_produced_clears_the_build_delay_as_well() {
        let mut g = GameLogic::new(crate::TerrainLogic::flat(100, 100, 0), 1);
        g.add_player(0, 1000);
        g.add_ai_player(0, SkirmishAiConfig::default());
        // The base-building tick sets the delay after its first attempt (`m_structureTimer` is 2).
        g.run_frames(4);
        let before = g.ai_player(0).unwrap().build_delay;
        assert!(before > 0, "the AI is waiting for its next base-building attempt: {before}");
        let t = std::sync::Arc::new(ObjectTemplate::new("TestStructure"));
        let bldg = g.create_object(&t, Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        g.ai_on_structure_produced(bldg);
        assert_eq!(g.ai_player(0).unwrap().team_delay, 0, "m_teamDelay = 0");
        assert_eq!(g.ai_player(0).unwrap().build_delay, 0, "m_buildDelay = 0");
    }

    /// `AIPlayer::buildUpgrade` (AIPlayer.cpp:1786): an object upgrade is not a player upgrade and is ignored, while
    /// a player upgrade from the same factory's command set is queued.
    #[test]
    fn ai_build_upgrade_ignores_object_upgrades() {
        struct CommandSets;
        impl crate::TemplateSource for CommandSets {
            fn find_template(&self, _: &str) -> Option<std::sync::Arc<crate::ObjectTemplate>> {
                None
            }
            fn can_build_upgrade(&self, builder: &str, _: &str) -> Option<bool> {
                Some(builder == "Plant")
            }
        }
        let mut g = GameLogic::new(crate::TerrainLogic::flat(100, 100, 0), 1);
        g.add_player(0, 1000);
        g.set_upgrade_info(
            [
                ("Upgrade_X".to_string(), crate::upgrade::UpgradeInfo { cost: 100, build_time_frames: 10 }),
                ("Upgrade_Y".to_string(), crate::upgrade::UpgradeInfo { cost: 100, build_time_frames: 10 }),
            ]
            .into_iter()
            .collect(),
        );
        g.set_template_source(std::sync::Arc::new(CommandSets));
        g.scripts_mut().load_command_button_upgrades([
            ("Cmd_X".to_owned(), crate::script::CommandButtonUpgrade { upgrade: "Upgrade_X".into(), object: false }),
            ("Cmd_Y".to_owned(), crate::script::CommandButtonUpgrade { upgrade: "Upgrade_Y".into(), object: true }),
        ]);
        let mut t = crate::ObjectTemplate::new("Plant");
        t.production = Some(crate::production::ProductionTemplate {
            max_queue_entries: 5,
            quantity_modifiers: vec![],
            exit: crate::production::ProductionExit::default(),
            ..Default::default()
        });
        let f = g.create_object(&std::sync::Arc::new(t), crate::math::Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
        g.add_ai_player(0, SkirmishAiConfig::default());
        g.ai_players.get_mut(&0).unwrap().built = vec![Some(f)];
        // The script action path (`AI_PLAYER_BUILD_UPGRADE`) passes the type in (`ai_build_upgrade_typed`), because
        // the command-button table is not reachable from `GameLogic` while the script engine runs.
        g.ai_build_upgrade_typed(0, "Upgrade_Y", true);
        assert!(g.find_object(f).unwrap().production.as_ref().unwrap().queue.is_empty(), "an object upgrade is ignored");
        assert_eq!(g.player(0).unwrap().money, 1000);
        g.ai_build_upgrade(0, "Upgrade_Y");
        assert!(g.find_object(f).unwrap().production.as_ref().unwrap().queue.is_empty(), "an object upgrade is ignored");
        assert_eq!(g.player(0).unwrap().money, 1000);
        g.ai_build_upgrade_typed(0, "Upgrade_X", false);
        assert_eq!(g.find_object(f).unwrap().production.as_ref().unwrap().queue.len(), 1, "a player upgrade is queued");
        assert_eq!(g.player(0).unwrap().money, 900);
    }

    /// `SpawnBehavior::createSpawn` (`SpawnBehavior.cpp:635`, then `:728`): `Player::onUnitCreated` runs
    /// before the spawn exits through its supply center. `AIPlayer::onUnitProduced` gives the gatherer
    /// its dock (`aiDock`, `AIPlayer.cpp:1116-1122`), and the exit's `aiFollowExitProductionPath` then
    /// replaces it, so the new worker walks out on its exit path (`FollowExitProductionPath`, GLA worker
    /// 238 of AN3 at frame 1127 in the C++ build). Exiting first left it docked.
    #[test]
    fn a_spawned_gatherer_walks_its_exit_path_after_its_dock_order() {
        use crate::production::{ProductionExit, ProductionTemplate};
        use crate::supply::{DockKind, DockTemplate, SupplyTruckTemplate};
        let mut center = ObjectTemplate::new("SupplyCenter");
        center.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        center.max_health = 500.0;
        center.initial_health = 500.0;
        center.dock = Some(DockTemplate { kind: DockKind::Center, starting_boxes: 0, delete_when_empty: false, number_approach_positions: 5, allows_passthrough: true });
        center.production = Some(ProductionTemplate {
            exit: ProductionExit::SupplyCenter { unit_create_point: (0.0, 0.0, 0.0), natural_rally_point: (10.0, 0.0, 0.0), grant_temporary_stealth: 0 },
            ..Default::default()
        });
        let mut truck = ObjectTemplate::new("SupplyTruck");
        truck.max_health = 100.0;
        truck.initial_health = 100.0;
        truck.geometry.major_radius = 5.0;
        truck.locomotor = Some(crate::template::LocomotorParams::from_ini(crate::template::LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        truck.supply_truck = Some(SupplyTruckTemplate { max_boxes: 1, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 100.0 });
        let center = std::sync::Arc::new(center);
        let truck = std::sync::Arc::new(truck);
        struct Store(Vec<std::sync::Arc<crate::ObjectTemplate>>);
        impl crate::TemplateSource for Store {
            fn find_template(&self, name: &str) -> Option<std::sync::Arc<crate::ObjectTemplate>> {
                self.0.iter().find(|t| t.name == name).cloned()
            }
        }
        let mut g = GameLogic::new(crate::TerrainLogic::flat(200, 200, 0), 1);
        g.add_player(0, 1000);
        g.set_template_source(std::sync::Arc::new(Store(vec![center.clone(), truck.clone()])));
        // The player's default team (`Player::setDefaultTeam`): the supply truck's entry is on it.
        g.scripts_mut().set_player_name("p0", 0);
        g.scripts_mut().load_default_team_protos(&[("teamP0", "p0")]);
        g.init_from_sides(&[("teamP0", 0)]);
        let c = g.create_object(&center, crate::math::Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        g.add_ai_player(0, SkirmishAiConfig::default());
        {
            let ai = g.ai_players.get_mut(&0).unwrap();
            ai.built = vec![Some(c)];
            ai.supply = vec![SupplyInfo { is_supply: true, desired: 2, current: -1, object: Some(c) }];
            ai.team_build_queue = vec![TeamInQueue {
                key: "teamp0".into(),
                frame_started: 0,
                priority_build: true,
                reinforcement: false,
                reinforcement_id: None,
                sent_to_start_location: false,
                work_orders: vec![WorkOrder { thing: "SupplyTruck".into(), factory: Some(c), completed: 0, required_count: 1, required: true, resource_gatherer: true }],
            }];
        }
        let spawned = g.spawn_produced_unit(c, &truck);
        assert_ne!(spawned, ObjectId::INVALID);
        assert_eq!(g.ai_player(0).unwrap().supply[0].current, 0, "the gatherer order docks the truck at the center");
        assert_eq!(
            g.find_object(spawned).unwrap().ai.machine.current,
            crate::ai_states::AiStateType::FollowExitProductionPath,
            "the exit path comes after the dock order"
        );
    }

    /// Player 0's default team (`Player::setDefaultTeam`, Player.cpp:760-770): key `teamp0`.
    fn with_default_team(g: &mut GameLogic) {
        g.scripts_mut().set_player_name("p0", 0);
        g.scripts_mut().load_default_team_protos(&[("teamP0", "p0")]);
        g.init_from_sides(&[("teamP0", 0)]);
    }

    /// A supply center (the spawner and the gatherer factory) for player 0, one build-list entry, and a
    /// `SupplyTruck` (a dozer when `dozer`, as the GLA worker is). Returns the world, the center and the truck.
    fn spawn_world(dozer: bool) -> (GameLogic, ObjectId, std::sync::Arc<ObjectTemplate>) {
        use crate::production::{ProductionExit, ProductionTemplate};
        use crate::supply::{DockKind, DockTemplate, SupplyTruckTemplate};
        let mut center = ObjectTemplate::new("SupplyCenter");
        center.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        center.max_health = 500.0;
        center.initial_health = 500.0;
        center.dock = Some(DockTemplate { kind: DockKind::Center, starting_boxes: 0, delete_when_empty: false, number_approach_positions: 5, allows_passthrough: true });
        center.production = Some(ProductionTemplate {
            exit: ProductionExit::SupplyCenter { unit_create_point: (0.0, 0.0, 0.0), natural_rally_point: (10.0, 0.0, 0.0), grant_temporary_stealth: 0 },
            ..Default::default()
        });
        let mut truck = ObjectTemplate::new("SupplyTruck");
        truck.max_health = 100.0;
        truck.initial_health = 100.0;
        truck.build_cost = 100;
        truck.geometry.major_radius = 5.0;
        truck.locomotor = Some(crate::template::LocomotorParams::from_ini(crate::template::LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        truck.supply_truck = Some(SupplyTruckTemplate { max_boxes: 1, center_action_delay: 1, warehouse_action_delay: 1, scan_distance: 100.0 });
        if dozer {
            truck.kind_of = kind_of::DOZER;
        }
        let center = std::sync::Arc::new(center);
        let truck = std::sync::Arc::new(truck);
        struct Store(Vec<std::sync::Arc<crate::ObjectTemplate>>);
        impl crate::TemplateSource for Store {
            fn find_template(&self, name: &str) -> Option<std::sync::Arc<crate::ObjectTemplate>> {
                self.0.iter().find(|t| t.name == name).cloned()
            }
        }
        let mut g = GameLogic::new(crate::TerrainLogic::flat(200, 200, 0), 1);
        g.add_player(0, 1000);
        with_default_team(&mut g);
        g.set_template_source(std::sync::Arc::new(Store(vec![center.clone(), truck.clone()])));
        let c = g.create_object(&center, crate::math::Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let cfg = SkirmishAiConfig {
            build_list: vec![AiBuildOrder { template: "SupplyCenter".into(), location: (50.0, 50.0), angle: 0.0, initially_built: true, rebuilds: 1, automatically_build: true }],
            resource_gatherers: 1,
            gatherer_template: "SupplyTruck".into(),
            gatherer_factory: "SupplyCenter".into(),
            ..Default::default()
        };
        g.add_ai_player(0, cfg);
        let ai = g.ai_players.get_mut(&0).unwrap();
        ai.built = vec![Some(c)];
        ai.supply = vec![SupplyInfo { is_supply: true, desired: 2, current: -1, object: Some(c) }];
        (g, c, truck)
    }

    /// A team-queue entry with one work order for `SupplyTruck` (a gatherer) or any other team order.
    fn truck_entry(key: &str, factory: ObjectId, gatherer: bool) -> TeamInQueue {
        TeamInQueue {
            key: key.into(),
            frame_started: 0,
            priority_build: true,
            reinforcement: false,
            reinforcement_id: None,
            sent_to_start_location: false,
            work_orders: vec![WorkOrder { thing: "SupplyTruck".into(), factory: Some(factory), completed: 0, required_count: 1, required: true, resource_gatherer: gatherer }],
        }
    }

    /// `AIPlayer::queueUnits` (AIPlayer.cpp:2705-2728): a recruit into a team with no home location goes to
    /// `getBaseCenter` (`m_baseCenter`, the build list's mean, `computeCenterAndRadiusOfBase` AIPlayer.cpp:3111-3139)
    /// and is sent there with `aiMoveToPosition` (AIPlayer.cpp:2706, :2727-2728). The port sent it to the first command
    /// center's location instead. Revert check: take the fallback home from the first build-list entry (the command
    /// center at (50, 50)) instead of `state.base_center` in `ai_recruit_into` and the goal is (50, 50), so the assert fails.
    #[test]
    fn a_recruit_without_a_team_home_moves_to_the_base_centre_mean() {
        let (mut g, _c, _truck_template) = spawn_world(false);
        // A lower-priority, AI-recruitable team holds the truck (`Team::tryToRecruit`, Team.cpp:2355-2378).
        let low = zr_map::Dict(vec![
            ("teamName".into(), zr_map::DictValue::Ascii("teamLow".into())),
            ("teamOwner".into(), zr_map::DictValue::Ascii("p0".into())),
            ("teamIsSingleton".into(), zr_map::DictValue::Bool(false)),
            ("teamIsAIRecruitable".into(), zr_map::DictValue::Bool(true)),
            ("teamProductionPriority".into(), zr_map::DictValue::Int(-1)),
        ]);
        g.scripts_mut().load_teams(&[low]);
        let low_team = g.scripts_mut().create_team("teamLow").unwrap();
        g.scripts_mut().teams.get_mut(&low_team).unwrap().created = true;
        let recruit = g.create_object_in_team(&g.find_template("SupplyTruck").unwrap(), crate::math::Coord3D::new(60.0, 60.0, 0.0), 0.0, &low_team);
        let mut state = g.ai_players.get(&0).unwrap().clone();
        // The mean of the build list (the command center is at (50, 50), so the two differ).
        state.base_center = (200.0, 150.0);
        // `MaxRecruitDistance` (the AI data's), so the truck at (60, 60) is within reach of the home.
        state.config.max_recruit_distance = 500.0;
        // A waiting team order for a supply truck on the default team (`teamp0`, priority 0).
        state.team_build_queue.push(TeamInQueue {
            key: "teamp0".into(),
            frame_started: 0,
            priority_build: true,
            reinforcement: false,
            reinforcement_id: None,
            sent_to_start_location: false,
            work_orders: vec![WorkOrder { thing: "SupplyTruck".into(), factory: None, completed: 0, required_count: 1, required: true, resource_gatherer: false }],
        });
        let qi = state.team_build_queue.len() - 1;
        g.ai_recruit_into(0, &mut state, qi);
        assert_eq!(state.team_build_queue[qi].work_orders[0].completed, 1, "the truck is recruited");
        let goal = g.find_object(recruit).unwrap().ai.machine.goal_position;
        assert!(
            (goal.x - 200.0).abs() < 0.01 && (goal.y - 150.0).abs() < 0.01,
            "the recruit goes to the build list's mean (200, 150), not the command center (50, 50): {goal:?}"
        );
    }

    /// `AISkirmishPlayer::acquireEnemy` (AISkirmishPlayer.cpp:500-505): the enemy's distance is measured from
    /// `m_baseCenter`, the build list's mean, not from the command center. Enemy 1's structure is nearer the command
    /// center (50, 50), enemy 2's nearer the mean (300, 300). Revert check: measure from the command center in
    /// `ai_acquire_enemy` and enemy 1 is chosen, so the assert fails.
    #[test]
    fn the_enemy_is_the_one_nearest_the_base_centre_mean() {
        let (mut g, _c, _t) = spawn_world(false);
        g.add_player(1, 1000);
        g.add_player(2, 1000);
        g.set_enemies(0, 1);
        g.set_enemies(0, 2);
        let center_t = g.find_template("SupplyCenter").unwrap();
        let truck_t = g.find_template("SupplyTruck").unwrap();
        g.create_object(&center_t, crate::math::Coord3D::new(60.0, 60.0, 0.0), 0.0, 1);
        g.create_object(&truck_t, crate::math::Coord3D::new(70.0, 70.0, 0.0), 0.0, 1);
        g.create_object(&center_t, crate::math::Coord3D::new(300.0, 200.0, 0.0), 0.0, 2);
        g.create_object(&truck_t, crate::math::Coord3D::new(310.0, 210.0, 0.0), 0.0, 2);
        let mut state = g.ai_players.get(&0).unwrap().clone();
        state.base_center = (300.0, 300.0);
        g.ai_acquire_enemy(0, &mut state);
        assert_eq!(state.current_enemy, Some(2), "the enemy nearest the build list's mean is chosen");
    }

    /// `AIPlayer::queueUnits` (AIPlayer.cpp:2701-2731): a team with no home location and an AI with no base centre
    /// (`m_baseCenterSet` false) recruits the unit and does `aiIdle(CMD_FROM_AI)`, not a move. Revert check: always
    /// `ai_move_to_position_cmd` the recruit, and it is given a `MoveTo`, so the assert fails.
    #[test]
    fn a_recruit_with_no_home_and_no_base_centre_is_idled_not_moved() {
        let (mut g, _c, _t) = spawn_world(false);
        let low = zr_map::Dict(vec![
            ("teamName".into(), zr_map::DictValue::Ascii("teamLow".into())),
            ("teamOwner".into(), zr_map::DictValue::Ascii("p0".into())),
            ("teamIsSingleton".into(), zr_map::DictValue::Bool(false)),
            ("teamIsAIRecruitable".into(), zr_map::DictValue::Bool(true)),
            ("teamProductionPriority".into(), zr_map::DictValue::Int(-1)),
        ]);
        g.scripts_mut().load_teams(&[low]);
        let low_team = g.scripts_mut().create_team("teamLow").unwrap();
        g.scripts_mut().teams.get_mut(&low_team).unwrap().created = true;
        let truck_t = g.find_template("SupplyTruck").unwrap();
        let recruit = g.create_object_in_team(&truck_t, crate::math::Coord3D::new(60.0, 60.0, 0.0), 0.0, &low_team);
        let mut state = g.ai_players.get(&0).unwrap().clone();
        state.base_center_set = false;
        state.base_center = (0.0, 0.0);
        state.config.max_recruit_distance = 500.0;
        state.team_build_queue.push(TeamInQueue {
            key: "teamp0".into(),
            frame_started: 0,
            priority_build: true,
            reinforcement: false,
            reinforcement_id: None,
            sent_to_start_location: false,
            work_orders: vec![WorkOrder { thing: "SupplyTruck".into(), factory: None, completed: 0, required_count: 1, required: true, resource_gatherer: false }],
        });
        let qi = state.team_build_queue.len() - 1;
        g.ai_recruit_into(0, &mut state, qi);
        assert_eq!(state.team_build_queue[qi].work_orders[0].completed, 1, "the truck is recruited");
        let current = g.find_object(recruit).unwrap().ai.machine.current;
        assert_eq!(current, crate::ai_states::AiStateType::Idle, "no home and no base centre: the recruit is idled, not moved");
    }

    /// `AIPlayer::queueSupplyTruck` (AIPlayer.cpp:223-416): the truck order is a `TeamInQueue` on the default
    /// team (`m_team = getDefaultTeam()`) with `m_priorityBuild`, prepended to the team build queue, and the
    /// first truck is the center's own (`m_factoryID`). A second one waits while the first is queued
    /// (`truckInQueue`). Pushing the entry at the back, or dropping the `truckInQueue` check, fails this.
    #[test]
    fn a_supply_truck_order_is_a_priority_entry_at_the_front_of_the_team_queue() {
        let (mut g, c, _truck) = spawn_world(false);
        let mut state = g.ai_players.get(&0).unwrap().clone();
        // A queued team that is not a gatherer: the truck order goes in front of it.
        state.team_build_queue.push(TeamInQueue {
            key: "other#9".into(),
            frame_started: 0,
            priority_build: false,
            reinforcement: false,
            reinforcement_id: None,
            sent_to_start_location: false,
            work_orders: Vec::new(),
        });
        g.ai_queue_supply_truck(0, &mut state);
        assert_eq!(state.team_build_queue.len(), 2, "one truck order added");
        assert_eq!(state.team_build_queue[1].key, "other#9", "the truck order is prepended");
        let q = &state.team_build_queue[0];
        assert_eq!(q.key, "teamp0", "the entry is on the player's default team");
        assert!(q.priority_build, "m_priorityBuild");
        assert_eq!(q.frame_started, g.frame, "m_frameStarted is the frame it was queued");
        let w = &q.work_orders[0];
        assert_eq!(w.thing, "SupplyTruck");
        assert!(w.resource_gatherer && w.required && w.required_count == 1 && w.completed == 0);
        assert_eq!(w.factory, Some(c), "the first truck is the center's own (m_factoryID = the factory)");
        assert_eq!(state.supply[0].current, 0);
        g.ai_queue_supply_truck(0, &mut state);
        assert_eq!(state.team_build_queue.len(), 2, "a supply truck in the build queue blocks another (truckInQueue)");
    }

    /// `AIPlayer::onUnitProduced` (AIPlayer.cpp:1116-1159): `supplyTruck` is the order's `m_isResourceGatherer`.
    /// A dozer out of a gatherer order (a GLA worker is both) keeps the structure timer; a dozer out of a team
    /// order restarts it (`!supplyTruck && isKindOf(DOZER)`). Restarting for every dozer fails the first half.
    #[test]
    fn a_gatherer_dozer_does_not_restart_the_structure_timer() {
        let (mut g, c, truck) = spawn_world(true);
        g.ai_players.get_mut(&0).unwrap().team_build_queue = vec![truck_entry("teamp0", c, true)];
        g.spawn_produced_unit(c, &truck);
        assert_eq!(g.ai_player(0).unwrap().structure_timer, 2, "a gatherer dozer does not restart the timer");

        let (mut g, c, truck) = spawn_world(true);
        g.ai_players.get_mut(&0).unwrap().team_build_queue = vec![truck_entry("teamp0", c, false)];
        g.spawn_produced_unit(c, &truck);
        assert_eq!(g.ai_player(0).unwrap().structure_timer, 1, "a team dozer restarts it (m_structureTimer = 1)");
    }

    /// `AIPlayer::checkReadyTeams` (AIPlayer.cpp:2853): activating a ready team runs `clearTeamFlags` for a
    /// skirmish AI. The supply truck's entry (on the default team) goes through this too. Removing the call
    /// leaves the flag set and fails this test.
    #[test]
    fn activating_a_ready_team_clears_the_team_building_flags() {
        let mut g = GameLogic::new(crate::TerrainLogic::flat(200, 200, 0), 1);
        g.add_player(0, 1000);
        with_default_team(&mut g);
        g.scripts_mut().set_flag("GLA Team is Building", true);
        let mut state = SkirmishAi::new(SkirmishAiConfig::default());
        state.team_ready_queue.push(TeamInQueue {
            key: "teamp0".into(),
            frame_started: 0,
            priority_build: true,
            reinforcement: false,
            reinforcement_id: None,
            sent_to_start_location: false,
            work_orders: Vec::new(),
        });
        g.ai_check_ready_teams(&mut state);
        assert!(state.team_ready_queue.is_empty(), "the default team has no member to wait for");
        assert!(!g.scripts_mut().get_flag("GLA Team is Building"), "clearTeamFlags ran");
    }

    /// `AIPlayer::queueUnits` (AIPlayer.cpp:2686-2740) runs `queueSupplyTruck` and then works every team-queue
    /// order on the 2-second tick. Without scripts the fallback path runs the same tick, so a supply truck order
    /// that could not be trained for lack of money is trained once the money arrives. Calling only
    /// `queueSupplyTruck` there leaves it waiting forever (truckInQueue blocks a new one).
    #[test]
    fn a_waiting_supply_truck_order_is_trained_when_money_arrives_without_scripts() {
        let (mut g, c, _truck) = spawn_world(false);
        g.players.get_mut(&0).unwrap().money = 0;
        g.ai_players.get_mut(&0).unwrap().supply[0].current = 0; // not the free first truck
        g.run_frames(61); // the tick at frame 60: the order is queued, the factory refuses it (no money)
        let queue = &g.ai_player(0).unwrap().team_build_queue;
        assert_eq!(queue.len(), 1, "one truck order queued");
        assert_eq!(queue[0].work_orders[0].factory, None, "no money: the order waits");
        g.players.get_mut(&0).unwrap().deposit(1000);
        g.run_frames(60); // the tick at frame 120 starts the waiting order, and the truck is built
        let queue = &g.ai_player(0).unwrap().team_build_queue;
        assert_eq!(queue[0].work_orders[0].factory, Some(c), "the waiting order is trained at the factory");
    }

    /// Templates for the dozer tests: a dozer (`KINDOF_DOZER`, mobile) and a war factory.
    struct DozerStore(Vec<std::sync::Arc<ObjectTemplate>>);
    impl crate::TemplateSource for DozerStore {
        fn find_template(&self, name: &str) -> Option<std::sync::Arc<ObjectTemplate>> {
            self.0.iter().find(|t| t.name == name).cloned()
        }
        /// `ThingFactory::firstTemplate` order (newest first), KINDOF_DOZER only.
        fn dozer_templates(&self) -> Vec<String> {
            self.0.iter().rev().filter(|t| t.kind_of & kind_of::DOZER != 0).map(|t| t.name.clone()).collect()
        }
    }

    /// The dozer is queued from the template list, not from the objects the player owns (AIPlayer.cpp:3198-3200):
    /// with no dozer alive the order is still placed. Candidates taken from the owned objects fail this.
    #[test]
    fn a_dozer_is_queued_when_the_player_owns_none() {
        let (mut g, f, d) = dozer_world();
        g.destroy_object(d);
        g.update();
        let mut state = dozer_state(f);
        assert!(g.ai_queue_dozer(0, &mut state), "queueDozer walks the templates");
        assert_eq!(state.team_build_queue[0].work_orders[0].thing, "Dozer");
    }

    /// Player 0 with its default team, one owned dozer and a war factory, which is the build list's only entry.
    fn dozer_world() -> (GameLogic, ObjectId, ObjectId) {
        use crate::production::ProductionTemplate;
        let mut dozer = ObjectTemplate::new("Dozer");
        dozer.kind_of = kind_of::DOZER;
        dozer.max_health = 100.0;
        dozer.initial_health = 100.0;
        dozer.locomotor = Some(crate::template::LocomotorParams::from_ini(crate::template::LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        let mut factory = ObjectTemplate::new("WarFactory");
        factory.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        factory.max_health = 500.0;
        factory.initial_health = 500.0;
        factory.production = Some(ProductionTemplate::default());
        let dozer = std::sync::Arc::new(dozer);
        let factory = std::sync::Arc::new(factory);
        let mut g = GameLogic::new(crate::TerrainLogic::flat(200, 200, 0), 1);
        g.add_player(0, 1000);
        with_default_team(&mut g);
        g.set_template_source(std::sync::Arc::new(DozerStore(vec![dozer.clone(), factory.clone()])));
        let f = g.create_object(&factory, crate::math::Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        let d = g.create_object(&dozer, crate::math::Coord3D::new(60.0, 50.0, 0.0), 0.0, 0);
        (g, f, d)
    }

    /// An AI state whose build list holds the war factory `f` (`built`), and which makes dozers and gatherers there.
    fn dozer_state(f: ObjectId) -> SkirmishAi {
        let cfg = SkirmishAiConfig {
            build_list: vec![AiBuildOrder { template: "WarFactory".into(), location: (50.0, 50.0), angle: 0.0, initially_built: true, rebuilds: 1, automatically_build: true }],
            gatherer_factory: "WarFactory".into(),
            ..Default::default()
        };
        let mut state = SkirmishAi::new(cfg);
        state.built = vec![Some(f)];
        state
    }

    /// `AIPlayer::queueDozer` (AIPlayer.cpp:3189-3236): one dozer order becomes a `TeamInQueue` on the default team
    /// with `m_priorityBuild`, prepended to the team build queue, with one required work order that is not a gatherer,
    /// started at the build-list factory. A second call does nothing while the order is queued (`dozerInQueue`).
    /// Dropping the `dozerInQueue` check or appending instead of prepending fails this test.
    #[test]
    fn queue_dozer_prepends_a_priority_team_entry_once() {
        let (mut g, f, _d) = dozer_world();
        let mut state = dozer_state(f);
        state.team_build_queue.push(TeamInQueue {
            key: "other#9".into(),
            frame_started: 0,
            priority_build: false,
            reinforcement: false,
            reinforcement_id: None,
            sent_to_start_location: false,
            work_orders: Vec::new(),
        });
        assert!(g.ai_queue_dozer(0, &mut state), "a dozer is queued");
        assert_eq!(state.team_build_queue.len(), 2);
        assert_eq!(state.team_build_queue[1].key, "other#9", "the dozer entry is prepended");
        let q = &state.team_build_queue[0];
        assert_eq!(q.key, "teamp0", "m_team = the default team");
        assert!(q.priority_build, "m_priorityBuild");
        assert_eq!(q.frame_started, g.frame, "m_frameStarted is the frame it was queued");
        let w = &q.work_orders[0];
        assert_eq!(w.thing, "Dozer");
        assert!(w.required && w.required_count == 1 && w.completed == 0 && !w.resource_gatherer);
        assert_eq!(w.factory, Some(f), "startTraining put it in production at the factory");
        assert!(!g.ai_queue_dozer(0, &mut state), "a dozer order is already in the build queue (dozerInQueue)");
        assert_eq!(state.team_build_queue.len(), 2, "no second dozer");
    }

    /// `TeamInQueue::includesADozer` (AIPlayer.cpp:3584) skips a resource gatherer: a GLA worker is both a dozer and
    /// a gatherer, so a worker gathering supplies does not stop a dozer from being ordered. Counting it fails this.
    #[test]
    fn a_gatherer_dozer_order_does_not_block_a_dozer() {
        let (mut g, f, _d) = dozer_world();
        let mut state = dozer_state(f);
        state.team_build_queue.push(TeamInQueue {
            key: "teamp0".into(),
            frame_started: 0,
            priority_build: true,
            reinforcement: false,
            reinforcement_id: None,
            sent_to_start_location: false,
            work_orders: vec![WorkOrder { thing: "Dozer".into(), factory: None, completed: 0, required_count: 1, required: true, resource_gatherer: true }],
        });
        assert!(g.ai_queue_dozer(0, &mut state), "the gatherer order is not a dozer order");
        assert_eq!(state.team_build_queue.len(), 2);
    }

    /// `AIPlayer::isLocationSafe` (`AIPlayer.cpp:1004`) asks the partition manager
    /// (`getClosestObject`, `PartitionManager.cpp:3285`), which only knows objects whose cells are
    /// placed. `ThingFactory::newObject` (`ThingFactory.cpp:346`) and `PartitionData::attachToObject`
    /// (`PartitionManager.cpp:2311-2345`) only mark a new object dirty, and the cells are placed by
    /// `ThePartitionManager->UPDATE()` at the end of the frame (`GameLogic.cpp:3897`). So an enemy
    /// structure created earlier in the same frame — the USA spy-satellite ping that made AN1 seed 1
    /// skip its China 1st Power Plant at frame 1 — does not block a build site yet. A plain
    /// `self.objects()` scan (the pre-lane-143 code) fails the middle assertion.
    #[test]
    fn a_structure_created_in_the_frame_does_not_make_a_build_site_unsafe_yet() {
        use crate::damage::Relationship;
        let mut g = GameLogic::new(crate::TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        g.add_player(1, 1000);
        g.set_relationship_symmetric(0, 1, Relationship::Enemies);
        g.add_ai_player(0, SkirmishAiConfig::default());
        let mut plant = ObjectTemplate::new("ChinaPowerPlant");
        plant.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        plant.geometry.major_radius = 20.0;
        let mut ping = ObjectTemplate::new("SpySatellitePing");
        ping.geometry.major_radius = 1.0;
        let ping = std::sync::Arc::new(ping);
        let site = crate::math::Coord3D::new(200.0, 200.0, 0.0);
        let ping_pos = crate::math::Coord3D::new(210.0, 200.0, 0.0);
        // Created before the frame (setup): the cells are placed at once, so it blocks.
        let enemy = g.create_object(&ping, ping_pos, 0.0, 1);
        assert!(!g.is_location_safe(0, &site, &plant, None), "an enemy in a placed cell blocks the site");
        // What `GameLogic::create_object` does inside a frame (`register_deferred`): no cell yet.
        g.partition.register_deferred(enemy, ping_pos, ping.geometry.bounding_circle_radius());
        assert!(g.is_location_safe(0, &site, &plant, None), "created this frame: the cells are not placed until the partition update");
        // The end-of-frame partition pass places them (`PartitionData::friend_updateCellsTouched`).
        g.partition.update(enemy, ping_pos);
        assert!(!g.is_location_safe(0, &site, &plant, None), "after the partition update the ping blocks again");
    }

    /// `AIPlayer::isLocationSafe` (`AIPlayer.cpp:1004`) applies exactly six filters (`:1015-1031`):
    /// affiliation, alive, stealthed-and-undetected, `PartitionFilterInsignificantBuildings`,
    /// harvesters and dozers. There is no projectile filter, so an enemy projectile in range makes a
    /// site unsafe; the pre-lane-143 code excluded `KINDOF_PROJECTILE` and returns true here.
    #[test]
    fn an_enemy_projectile_blocks_a_build_site_like_any_enemy_non_building() {
        use crate::damage::Relationship;
        let mut g = GameLogic::new(crate::TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        g.add_player(1, 1000);
        g.set_relationship_symmetric(0, 1, Relationship::Enemies);
        g.add_ai_player(0, SkirmishAiConfig::default());
        let mut plant = ObjectTemplate::new("ChinaPowerPlant");
        plant.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE;
        plant.geometry.major_radius = 20.0;
        let mut missile = ObjectTemplate::new("TomahawkMissile");
        missile.kind_of = kind_of::PROJECTILE;
        missile.geometry.major_radius = 1.0;
        let missile = std::sync::Arc::new(missile);
        let site = crate::math::Coord3D::new(200.0, 200.0, 0.0);
        g.create_object(&missile, crate::math::Coord3D::new(210.0, 200.0, 0.0), 0.0, 1);
        assert!(!g.is_location_safe(0, &site, &plant, None), "an enemy projectile is a blocker, as in the original");
    }
}
