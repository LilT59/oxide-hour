//! `GameLogic` (GameLogic/System/GameLogic.cpp): the frame loop, object registry and command intake.
//!
//! # `GameLogic::update()` ordering
//!
//! | Original step | Here |
//! |---|---|
//! | `TheScriptEngine->UPDATE()` | stub ([`GameLogic::script_engine_update`], no-op) |
//! | frozen-time check | not ported (no frame pacer) |
//! | `TheTerrainLogic->UPDATE()` | stub (bridges/water not ported) |
//! | CRC generation for replays/MP | not automatic; call [`GameLogic::crc`] when needed |
//! | `TheRecorder->UPDATE()`, stats | not ported |
//! | `processCommandList(TheCommandList)` | ported: [`GameLogic::process_command_list`] |
//! | `m_normalUpdates` (`ALLOW_NONSLEEPY_UPDATES`) | not ported (the original compiles it out too) |
//! | sleepy update loop over `m_sleepyUpdates` | ported: due modules run in heap order |
//! | `TheAI->UPDATE()` (pathfinder queue + AIPlayers) | ported: [`GameLogic::process_pathfind_queue`] then AI players |
//! | `TheBuildAssistant->UPDATE()` | stub |
//! | `ThePartitionManager->UPDATE()` | ported: re-registers moved objects |
//! | `processDestroyList()` | ported |
//! | `TheCommandList->reset()` | ported (the queue is drained when processed) |
//! | weapon/locomotor store, victory conditions | stubs |
//! | `checkDisabledStatus` | ported ([`GameLogic::check_disabled_status`](crate::disabled)) |
//! | `m_frame++` | ported |
//!
//! Object ids are allocated from a monotonically increasing counter starting at 1
//! (`allocateObjectID`) and never reused. Objects live in a `BTreeMap<ObjectId, Object>` so all
//! iteration is in id order.

use crate::ai::AiUpdate;
use crate::combat::{AttackTarget, CombatState, ObjectCombat};
use crate::body::{ActiveBody, DamageInfo};
use crate::command::{Command, GameMessage};
use crate::crc::Crc;
use crate::damage::{DeathType, Relationship};
use crate::die::{FxEvent, SlowDeathMap};
use crate::energy::GameRules;
use crate::math::Coord3D;
use crate::module::{Request, UpdateContext, UpdateModule, UPDATE_SLEEP_FOREVER, UPDATE_SLEEP_NONE};
use crate::object::{Object, ObjectId, PlayerIndex};
use crate::partition::{PartitionManager, DEFAULT_CELL_SIZE};
use crate::pathfind::{PathObject, Pathfinder};
use crate::player::Player;
use crate::skirmish_ai::AiPlayerMap;
use crate::production::ProductionState;
use crate::object_helpers::{object_helper_wakes, ObjectHelper};
use crate::random::GameRandom;
use crate::schedule::UpdateSchedule;
use crate::shroud::{shroud_clearing_range, ObjectShroudStatus, Shroud, Sighting, PLAYERMASK_ALL};
use crate::template::{kind_of, ObjectTemplate, TemplateSource};
use crate::terrain::TerrainLogic;
use crate::ai::AiState;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Counters for diagnostics; never hashed.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub module_updates: u64,
    /// Path requests serviced by the async pathfind queue (`processPathfindQueue`).
    pub pathfind_requests: u64,
    /// Ground-unit collision-scan candidates considered (`processCollision`).
    pub collision_checks: u64,
}

/// `VictoryConditions` outcome: the winner (once all but one player are out) and the defeated.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameEnd {
    pub winner: Option<PlayerIndex>,
    pub defeated: Vec<PlayerIndex>,
}

/// A behaviour module of a new object that is registered in the sleepy heap, before its position in the template's
/// INI order is known (`create_object_full` orders them by [`ObjectTemplate::update_module_order`]).
#[derive(Clone, Copy)]
enum PendingModule {
    Ai,
    Family(crate::sleepy_families::FamilyKind, u32, usize),
    Railroad,
}

/// A behaviour module whose C++ constructor draws from the logic RNG. `Object::Object` constructs the modules in
/// the template's INI order (`Object.cpp:300-410`), so each constructor's draw happens in that order; only
/// `onObjectCreated` work (the AI state machine's idle entries) comes after every constructor (`Object.cpp:474-477`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CtorDraw {
    /// `LifetimeUpdate::calcSleepDelay` (`LifetimeUpdate.cpp:77`).
    Lifetime,
    /// `DeletionUpdate::calcSleepDelay` (`DeletionUpdate.cpp:74`).
    Deletion,
    /// `AIUpdateInterface`'s constructor: the `TurretAI`s' idle scans (`TurretAI.cpp:321`, `:1286-1291`) and
    /// `chooseLocomotorSet`'s `Locomotor` wander draws (`AIUpdate.cpp:290-296`, `Locomotor.cpp:673-675`).
    Ai,
    /// `StealthDetectorUpdate`'s random phase (`StealthDetectorUpdate.cpp:88-90`).
    StealthDetector,
    /// `AutoHealBehavior`'s starts-active phase (`AutoHealBehavior.cpp:98-105`).
    AutoHeal,
    /// `HordeUpdate`'s first wake, `GameLogicRandomValue(1, UpdateRate)` (`HordeUpdate.cpp:161-169`).
    Horde,
}

/// The RNG-drawing module constructors of `template`, in `Object::Object`'s construction order: the template's INI
/// module order first, then every module the order does not name, in the port's legacy fixed order. `want_ai` is
/// `create_object_full`'s AI-module predicate (a template can carry a locomotor without an `AIUpdate` block).
fn ctor_draws(template: &ObjectTemplate, want_ai: bool) -> Vec<CtorDraw> {
    use crate::template::SleepyModuleKind as K;
    let present = |d: CtorDraw| match d {
        CtorDraw::Lifetime => template.lifetime.is_some(),
        CtorDraw::Deletion => template.deletion.is_some(),
        CtorDraw::Ai => want_ai || crate::turret_ai::has_turret(template),
        CtorDraw::StealthDetector => template.stealth_detector.is_some(),
        CtorDraw::AutoHeal => !template.auto_heal.is_empty(),
        CtorDraw::Horde => template.horde.is_some(),
    };
    // An AutoHeal draw is one per AutoHeal module (not deduplicated: an object may carry several, lane 178).
    let mut out: Vec<CtorDraw> = Vec::new();
    for kind in &template.update_module_order {
        let draw = match kind {
            K::Lifetime => CtorDraw::Lifetime,
            K::Deletion => CtorDraw::Deletion,
            K::Ai => CtorDraw::Ai,
            K::StealthDetector => CtorDraw::StealthDetector,
            K::AutoHeal => CtorDraw::AutoHeal,
            K::Horde => CtorDraw::Horde,
            _ => continue,
        };
        if present(draw) && (draw == CtorDraw::AutoHeal || !out.contains(&draw)) {
            out.push(draw);
        }
    }
    // The AutoHeal modules the order does not name (a synthetic template) follow the listed ones, as before.
    let named = out.iter().filter(|d| **d == CtorDraw::AutoHeal).count();
    let mut legacy = vec![CtorDraw::Lifetime, CtorDraw::Deletion];
    if template.ai_before_stealth {
        legacy.push(CtorDraw::Ai);
        legacy.push(CtorDraw::StealthDetector);
    } else {
        legacy.push(CtorDraw::StealthDetector);
        legacy.push(CtorDraw::Ai);
    }
    legacy.extend(std::iter::repeat_n(CtorDraw::AutoHeal, template.auto_heal.len().saturating_sub(named)));
    legacy.push(CtorDraw::Horde);
    for draw in legacy {
        if present(draw) && (draw == CtorDraw::AutoHeal || !out.contains(&draw)) {
            out.push(draw);
        }
    }
    out
}

pub struct GameLogic {
    pub(crate) frame: u32,
    /// True while [`update`](Self::update) runs. An object created in a frame is in no partition cell until
    /// `update_partition` (see `create_object_with`); setup code outside a frame sees its objects at once.
    in_update: bool,
    next_obj_id: u32,
    pub(crate) objects: BTreeMap<ObjectId, Object>,
    pub(crate) schedule: UpdateSchedule,
    /// Test-only record of the AI modules' run order (`ai_states::ai_update_object`).
    #[cfg(test)]
    pub(crate) ai_pass_log: Vec<ObjectId>,
    /// Test-only record of one `ai_update_one`'s sub-updates, in order: the `ChinookAIUpdate` half,
    /// the `SupplyTruckAIUpdate` half and `AIUpdateInterface::update` (lane 137, R11 F9).
    #[cfg(test)]
    pub(crate) ai_step_log: Vec<(&'static str, ObjectId)>,
    pub(crate) partition: PartitionManager,
    /// Per-player shroud / fog-of-war (`ThePartitionManager`'s shroud levels).
    pub(crate) shroud: Shroud,
    /// `PartitionCell::m_cashValue`: the build cost each player's objects add to the cells their vision
    /// circle covers (`Object::addValue`, `PartitionManager.cpp:4271-4334`). Read by
    /// `getNearestGroupWithValue` (`:4880`).
    pub(crate) value_map: crate::partition_value::ValueMap,
    /// `Object::m_partitionLastValue` per object: the affect currently added to the value map.
    pub(crate) value_affects: BTreeMap<ObjectId, crate::partition_value::ValueAffect>,
    pub(crate) terrain: TerrainLogic,
    pub(crate) pathfinder: Pathfinder,
    /// `Pathfinder::m_moveAlliesDepth` (AIPathfind.cpp:10205): nesting of `moveAllies` through
    /// `privateMoveAwayFromUnit`, capped at 3 levels.
    pub(crate) move_allies_depth: u32,
    pub(crate) rng: GameRandom,
    pub(crate) combat: CombatState,
    #[cfg(feature = "possession")]
    pub(crate) possessed: BTreeMap<PlayerIndex, crate::possession::PossessState>,
    command_list: Vec<GameMessage>,
    objects_to_destroy: Vec<ObjectId>,
    /// The map's polygon triggers and `m_frameObjectsChangedTriggerAreas` (lane 184, `guard_area.rs`).
    pub(crate) trigger_areas: crate::guard_area::TriggerAreas,
    /// Objects in a `SlowDeathBehavior` (`m_slowDeaths`), processed once per frame in id order.
    pub(crate) slow_deaths: SlowDeathMap,
    /// FX events recorded by die modules for the client (`FXListDie`, death FX).
    pub(crate) fx_events: Vec<FxEvent>,
    /// Player money, by index (`Money`). The rest of the `Player` subsystem is not ported.
    pub(crate) players: BTreeMap<PlayerIndex, Player>,
    /// `ThingFactory::findTemplate`, used to resolve production/OCL template names. Set by the
    /// adapter (`Game::from_map`); `None` in bare sim tests.
    pub(crate) template_source: Option<Arc<dyn TemplateSource>>,
    /// Global economy/build rules from the `GameData` block (set by the adapter).
    pub(crate) rules: GameRules,
    /// Skirmish AI players, by index (`AIPlayer`).
    pub(crate) ai_players: AiPlayerMap,
    /// `Player::m_upgrades`: upgrade names researched per player.
    pub(crate) player_upgrades: BTreeMap<PlayerIndex, std::collections::BTreeSet<String>>,
    /// Global `GameData` `WeaponBonus` set (applied before each weapon's own entries).
    pub(crate) global_weapon_bonus: crate::bonus::WeaponBonusSet,
    /// `GameData` `HealthBonus_<Level>` (`GlobalData::m_healthBonus`).
    pub(crate) health_bonus: [f32; 4],
    /// Upgrade research cost/time by name (`UpgradeTemplate::calcCostToBuild` / `calcTimeToBuild`).
    pub(crate) upgrade_info: BTreeMap<String, crate::upgrade::UpgradeInfo>,
    /// Special-power store by name (`SpecialPowerStore`), set by the adapter.
    pub(crate) special_powers: BTreeMap<String, Arc<crate::special_power::SimSpecialPowerTemplate>>,
    /// Shared special-power cooldowns (`SharedSyncedTimer`), by `(player, power)`.
    pub(crate) shared_power_ready: BTreeMap<(PlayerIndex, String), u32>,
    /// Player sciences / skill points / rank (`Player`).
    pub(crate) sciences: BTreeMap<PlayerIndex, crate::science::ScienceState>,
    /// `Science` templates by name.
    pub(crate) science_templates: BTreeMap<String, crate::science::ScienceInfo>,
    /// `Rank` entries (`RankInfoStore`).
    pub(crate) rank_info: Vec<crate::science::RankInfo>,
    /// `GameLogic::m_rankLevelLimit` (reset to 1000 each game; `PLAYER_SET_RANKLEVELLIMIT`).
    pub(crate) rank_level_limit: i32,
    /// `GameLogic::m_isScoringEnabled` (`ENABLE_SCORING` / `DISABLE_SCORING`, `score.rs`).
    pub(crate) scoring_enabled: bool,
    /// In-flight `DeliverPayload` transports (`DeliverPayloadAIUpdate`).
    pub(crate) deliveries: Vec<crate::ocl::DeliveryState>,
    /// The map script engine (`TheScriptEngine`).
    pub(crate) scripts: crate::script::ScriptEngine,
    /// `TheAI->getAiData()` tunables used by target selection.
    pub(crate) ai_tunables: crate::ai::AiTunables,
    /// The players that can win/lose a skirmish (`VictoryConditions`), set by the adapter.
    pub(crate) victory_players: std::collections::BTreeSet<PlayerIndex>,
    /// `ScriptActions::doVictory`: a script declared this player the winner (`startEndGameTimer`).
    pub(crate) scripted_winner: Option<PlayerIndex>,
    /// `ScriptActions::doDefeat` / `doLocalDefeat`: scripted losers (`SetVictorious(FALSE)`).
    pub(crate) scripted_defeated: std::collections::BTreeSet<PlayerIndex>,
    /// `VictoryConditions::m_isDefeated`: players already marked defeated and `killPlayer`ed.
    pub(crate) victory_defeated: std::collections::BTreeSet<PlayerIndex>,
    /// `Player::m_isPlayerDead`: set by [`GameLogic::kill_player`] (`Player::killPlayer`). Read
    /// through [`GameLogic::is_player_active`] (`Player::isPlayerActive`).
    pub(crate) player_dead: std::collections::BTreeSet<PlayerIndex>,
    /// Per-player tunnel networks (`Player::m_tunnelSystem`); the shared passenger list of every
    /// `TunnelContain` a player owns.
    pub(crate) tunnel_trackers: BTreeMap<PlayerIndex, crate::tunnel::TunnelTracker>,
    /// Per-structure `GarrisonContain` fire-point assignments (`m_garrisonPointData`), keyed by
    /// the garrison object. Present only for enclosing garrisons with an occupant.
    pub(crate) garrison_points: BTreeMap<ObjectId, crate::garrison::GarrisonPointState>,
    /// `ChinookAIUpdate` combat-drop states, keyed by the Chinook object.
    pub(crate) chinooks: crate::chinook::ChinookMap,
    /// Descending rappellers (`AIRappelState`), keyed by the passenger.
    pub(crate) rappels: crate::chinook::RappelMap,
    /// Per-object `RailroadBehavior` state (the locomotive and every carriage), keyed by object id.
    pub(crate) railroad_states: BTreeMap<ObjectId, crate::railroad::RailroadState>,
    /// `Player::getEnergy()->setPowerSabotagedTillFrame` (power-plant sabotage): the player is in
    /// brownout until this frame.
    pub(crate) power_sabotaged_till: BTreeMap<PlayerIndex, u32>,
    pub(crate) stats: Stats,
    /// Diagnostics (not hashed): special powers actually fired, by power name.
    pub(crate) power_fire_counts: BTreeMap<String, u64>,
    /// Diagnostics (not hashed): `buildSpecificAIBuilding` template names, by invocation count.
    pub(crate) ai_build_specific_counts: BTreeMap<String, u64>,
    /// Diagnostics (not hashed): a chosen build order whose construction could not start, by template.
    pub(crate) ai_structure_fail_counts: BTreeMap<String, u64>,
    /// Objects an object-creating *script action* made while the script engine was taken out of
    /// this struct (`std::mem::take`): their team's member list is filled in right after
    /// `ScriptEngine::update` returns. `Object::Object` sets the team in the constructor, so the
    /// object already carries the key; only the list insert waits for the engine.
    pending_team_joins: Vec<(ObjectId, String)>,
}

impl GameLogic {
    /// `GameLogic::init` + `startNewGame`: an empty world over `terrain`, RNG seeded with `seed`.
    pub fn new(terrain: TerrainLogic, seed: u32) -> Self {
        Self::with_cell_size(terrain, seed, DEFAULT_CELL_SIZE)
    }

    pub fn with_cell_size(terrain: TerrainLogic, seed: u32, cell_size: f32) -> Self {
        crate::id_dump::enable_from_env();
        let (w, h) = terrain.world_size();
        let partition = PartitionManager::new(w, h, cell_size);
        let (cx, cy) = partition.cell_counts();
        Self {
            frame: 0,
            in_update: false,
            next_obj_id: 1,
            objects: BTreeMap::new(),
            schedule: UpdateSchedule::default(),
            #[cfg(test)]
            ai_pass_log: Vec::new(),
            #[cfg(test)]
            ai_step_log: Vec::new(),
            shroud: Shroud::new(cx, cy, partition.cell_size()),
            value_map: crate::partition_value::ValueMap::new(cx, cy, partition.cell_size()),
            value_affects: BTreeMap::new(),
            partition,
            pathfinder: Pathfinder::new(&terrain),
            move_allies_depth: 0,
            terrain,
            rng: GameRandom::new(seed).with_env_trace(),
            combat: CombatState::default(),
            #[cfg(feature = "possession")]
            possessed: BTreeMap::new(),
            command_list: Vec::new(),
            objects_to_destroy: Vec::new(),
            trigger_areas: Default::default(),
            slow_deaths: SlowDeathMap::new(),
            fx_events: Vec::new(),
            players: BTreeMap::new(),
            template_source: None,
            rules: GameRules::default(),
            ai_players: AiPlayerMap::new(),
            player_upgrades: BTreeMap::new(),
            global_weapon_bonus: crate::bonus::WeaponBonusSet::default(),
            health_bonus: [1.0; 4],
            upgrade_info: BTreeMap::new(),
            special_powers: BTreeMap::new(),
            shared_power_ready: BTreeMap::new(),
            sciences: BTreeMap::new(),
            science_templates: BTreeMap::new(),
            rank_info: Vec::new(),
            rank_level_limit: 1000,
            scoring_enabled: true,
            deliveries: Vec::new(),
            scripts: crate::script::ScriptEngine::default(),
            ai_tunables: crate::ai::AiTunables::default(),
            victory_players: std::collections::BTreeSet::new(),
            scripted_winner: None,
            scripted_defeated: std::collections::BTreeSet::new(),
            victory_defeated: std::collections::BTreeSet::new(),
            player_dead: std::collections::BTreeSet::new(),
            tunnel_trackers: BTreeMap::new(),
            garrison_points: BTreeMap::new(),
            chinooks: BTreeMap::new(),
            rappels: BTreeMap::new(),
            railroad_states: BTreeMap::new(),
            power_sabotaged_till: BTreeMap::new(),
            stats: Stats::default(),
            power_fire_counts: BTreeMap::new(),
            ai_build_specific_counts: BTreeMap::new(),
            ai_structure_fail_counts: BTreeMap::new(),
            pending_team_joins: Vec::new(),
        }
    }

    // ---- accessors -------------------------------------------------------------------------

    /// `getFrame`.
    pub fn frame(&self) -> u32 {
        self.frame
    }
    pub fn terrain(&self) -> &TerrainLogic {
        &self.terrain
    }
    pub fn pathfinder(&self) -> &Pathfinder {
        &self.pathfinder
    }
    /// The `AIData` tunables read from the INI, set once at game start.
    pub fn set_ai_tunables(&mut self, t: crate::ai::AiTunables) {
        self.ai_tunables = t;
    }

    /// `Pathfinder::m_wallHeight` (`AIData WallHeight`), set once at game start.
    pub fn set_wall_height(&mut self, h: f32) {
        self.pathfinder.set_wall_height(h);
    }

    /// `Pathfinder::newMap`'s wall-layer build: after every map wall piece has been registered
    /// (`addWallPiece`), classify the wall layer once.
    pub fn rebuild_wall_layer(&mut self) {
        self.pathfinder.ensure_wall_layer();
    }

    /// Build the pathfinder's ally table from `GameLogic`'s relationship matrix. Called once per
    /// frame (before pathfinding) so [`crate::pathfind::Pathfinder::check_for_movement_full`]'s
    /// `ALLIES` tests match `Player::getRelationship`.
    fn sync_pathfinder_allies(&mut self) {
        let mut allies = [0u32; 32];
        for a in 0..32u32 {
            let pa = a as PlayerIndex;
            for b in 0..32u32 {
                if self.relationship(pa, b as PlayerIndex) == crate::damage::Relationship::Allies {
                    allies[a as usize] |= 1u32 << b;
                }
            }
        }
        self.pathfinder.set_allies(allies);
    }
    pub fn partition(&self) -> &PartitionManager {
        &self.partition
    }
    /// The per-player shroud grids.
    pub fn shroud(&self) -> &Shroud {
        &self.shroud
    }

    /// The shroud status of the cell containing `pos` for `player` (`getShroudStatusForPlayer`).
    pub fn cell_shroud_status(&self, player: PlayerIndex, pos: &Coord3D) -> crate::shroud::CellShroudStatus {
        self.shroud.cell_status_at(player, pos)
    }

    /// `Object::getShroudedStatus`: how `player` sees the object `id` (destroyed objects are
    /// treated as absent, matching `findObjectByID`).
    pub fn object_shroud_status(&self, player: PlayerIndex, id: ObjectId) -> ObjectShroudStatus {
        let Some(obj) = self.find_object(id) else { return ObjectShroudStatus::Shrouded };
        let neutral = self.relationship(player, obj.player) == Relationship::Neutral;
        self.shroud.object_status(
            player,
            &obj.position,
            obj.template.geometry.bounding_circle_radius(),
            obj.is_kind_of(kind_of::IMMOBILE),
            neutral,
            obj.is_kind_of(kind_of::ALWAYS_VISIBLE),
        )
    }
    pub fn rng_mut(&mut self) -> &mut GameRandom {
        &mut self.rng
    }
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Register a player's starting money (`Player::init`).
    pub fn add_player(&mut self, index: PlayerIndex, money: u32) {
        self.players.insert(index, Player::new(index, money));
        // `Player::init` -> `resetRank`: rank 1's purchase points and granted sciences.
        self.reset_rank(index);
    }

    /// `CashBountyPower::onObjectCreated` / `onSpecialPowerCreation`: raise the player's cash-bounty
    /// fraction to `bounty` if it is higher than the current one.
    pub fn set_player_cash_bounty(&mut self, player: PlayerIndex, bounty: f32) {
        if let Some(p) = self.players.get_mut(&player) {
            if bounty > p.cash_bounty {
                p.cash_bounty = bounty;
            }
        }
    }

    /// The player's current cash (`Money::countMoney`).
    pub fn player_money(&self, player: PlayerIndex) -> u32 {
        self.players.get(&player).map_or(0, |p| p.money)
    }

    /// `Object::setProducer`: record the object that produced/spawned `id`.
    pub fn set_producer(&mut self, id: ObjectId, producer: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.producer_id = producer;
        }
    }

    /// `SpecialPowerCompletionDie::setCreator`: set the object whose special power `id` is the
    /// payload of.
    pub fn set_sp_creator(&mut self, id: ObjectId, creator: ObjectId) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.sp_creator = creator;
        }
    }

    /// Teleport an object (tests / scripts). Marks the partition dirty.
    pub fn set_object_position(&mut self, id: ObjectId, pos: Coord3D) {
        if let Some(o) = self.objects.get_mut(&id) {
            o.set_position(pos);
        }
    }

    /// A player's cash (`Money`).
    pub fn player(&self, index: PlayerIndex) -> Option<&Player> {
        self.players.get(&index)
    }

    /// Players in index order.
    pub fn players(&self) -> impl Iterator<Item = &Player> {
        self.players.values()
    }

    /// `ThingFactory::findTemplate` source, set by the adapter so production can resolve names.
    pub fn set_template_source(&mut self, source: Arc<dyn TemplateSource>) {
        self.template_source = Some(source);
    }

    /// `ThingFactory::findTemplate` through the adapter source, when one is set.
    pub fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.template_source.as_ref().and_then(|s| s.find_template(name))
    }

    /// Set the global economy/build rules (from the `GameData` block).
    pub fn set_rules(&mut self, rules: GameRules) {
        self.rules = rules;
    }

    /// The global economy/build rules.
    pub fn rules(&self) -> GameRules {
        self.rules
    }
    /// `findObjectByID`: destroyed-but-not-yet-deleted objects are hidden, like the original.
    pub fn find_object(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(&id).filter(|o| !o.destroyed)
    }
    /// Live objects in id order.
    pub fn objects(&self) -> impl Iterator<Item = &Object> {
        self.objects.values().filter(|o| !o.destroyed)
    }
    /// The whole object list in id order, including objects destroyed earlier in the frame that are still
    /// waiting for the frame's `processDestroyList` (the engine's `m_objList`, `GameLogic.cpp:4795-4804`).
    /// Only the snapshot writer and [`object_count`](Self::object_count) use it: the C++ frame-0 block is
    /// printed before the first update, when `startNewGame`'s two replaced command centres are still listed.
    pub fn object_list(&self) -> impl Iterator<Item = &Object> {
        self.objects.values()
    }
    pub fn object_count(&self) -> usize {
        // `GameLogic::getObjectCount` (`GameLogic.cpp:4795-4804`) walks the object list and counts every
        // entry: `destroyObject` only marks `OBJECT_STATUS_DESTROYED` and queues the object
        // (`GameLogic.cpp:4232-4247`); it leaves the list at the frame's `processDestroyList`. So an object
        // destroyed earlier in the frame still counts, which is why the C++ frame-0 snapshot has the two
        // command centres `startNewGame` replaced.
        self.objects.len()
    }
    /// An object's current `ModelConditionFlags` (`None` for an unknown object).
    pub fn model_conditions(&self, id: ObjectId) -> Option<crate::condition::ModelConditionFlags> {
        self.objects.get(&id).map(|o| o.model_conditions)
    }

    /// The number of sleepy-heap entries an object owns. `registerObject` pushes one per `UpdateModule`
    /// of the object's template (`GameLogic.cpp:4165-4200`), and a template may name the same module class
    /// more than once (the China Gattling Cannon's two `ProductionUpdate`s). Read-only.
    pub fn update_module_count(&self, id: ObjectId) -> usize {
        self.objects.get(&id).map_or(0, |o| o.update_handles.len())
    }
    /// Read-only snapshots of the live projectiles, for client rendering.
    pub fn projectiles(&self) -> Vec<crate::combat::ProjectileSnapshot> {
        self.combat.projectile_snapshots()
    }
    /// Number of registered sleepy update modules.
    pub fn update_count(&self) -> usize {
        self.schedule.len()
    }

    /// The frame a registered module wakes on (`UpdateModule::friend_getNextCallFrame`), for tests and
    /// diagnostics. `u32::MAX` (never) for a module that sleeps forever.
    pub fn update_wake_frame(&self, h: crate::schedule::UpdateHandle) -> u32 {
        self.schedule.next_call_frame(h)
    }

    /// The object's `AIUpdateInterface` heap handle (`Object::getAIUpdateInterface`), for tests and diagnostics.
    pub fn ai_update_handle(&self, id: ObjectId) -> Option<crate::schedule::UpdateHandle> {
        self.objects.get(&id).and_then(|o| o.ai_handle)
    }

    // ---- object lifetime -------------------------------------------------------------------

    /// `allocateObjectID`. Projectiles, laser beams and projectile streams take their ids from here too
    /// (`Object::Object` / `Thing::newObject` at Weapon.cpp:1105, 2515 and 2543), so the counter matches
    /// the original's. `template` and `loc` only feed the optional `ZR_ID_DUMP` diagnostic (the caller's
    /// location labels the creation path, as the C++ `ZRL64Path` does).
    pub(crate) fn allocate_object_id(&mut self, template: &str, loc: &std::panic::Location<'_>) -> ObjectId {
        let id = ObjectId(self.next_obj_id);
        self.next_obj_id += 1;
        crate::id_dump::record(id.0, self.frame, template, loc);
        id
    }

    /// `newObject` + `registerObject`. Ground units snap to the terrain height.
    /// Standard modules are derived from the template: a body when `max_health > 0`,
    /// an [`AiUpdate`] when it has a locomotor.
    #[track_caller]
    pub fn create_object(&mut self, template: &Arc<ObjectTemplate>, pos: Coord3D, angle: f32, player: PlayerIndex) -> ObjectId {
        self.create_object_with(template, pos, angle, player, Vec::new())
    }

    /// `ThePartitionManager->UPDATE()` (`PartitionManager.cpp:2796-2840`, run at `GameLogic.cpp:3897` and
    /// before each starting unit's `findPositionAround`, `GameLogic.cpp:588`, and in `tryStartNewGame`,
    /// `GameLogic.cpp:2135`): every dirty object places its cells (`updateCellsTouched`), the dirty
    /// modules gather their contacts, then the contact list is processed (`processContactList`). The
    /// dirty flags are cleared by the contact gathering (`process_object_collisions`).
    pub fn update_partition(&mut self) {
        let dirty: Vec<(ObjectId, Coord3D)> = self
            .objects
            .values()
            .filter(|o| o.partition_dirty && !o.destroyed)
            .map(|o| (o.id, o.position))
            .collect();
        for (id, pos) in dirty {
            self.partition.update(id, pos);
        }
        self.process_object_collisions();
    }

    /// The mobile tail of `GameLogic::placeObjectAtPosition` (`GameLogic.cpp:520-527`): an object that has an
    /// AI and is not immobile asks `Pathfinder::adjustDestination` for a valid, unoccupied cell near its spot
    /// and moves there. Retail (`RETAIL_COMPATIBLE_PATHFINDING`, GameDefines.h:116) takes the first valid
    /// cell even when the spot was already valid (`checkForAdjust` sets `*dest`, AIPathfind.cpp:5355). The
    /// centre of a centred footprint, else `cell + 0.05` (`adjustCoordToCell`, AIPathfind.cpp:9011-9020), is
    /// the new x/y, and the z is that cell's ground height. `setPosition` then moves the pathfinder's unit
    /// footprint (`Pathfinder::updatePos`, Object.cpp:2650-2657). The starting units of
    /// `GameLogic::setupPlayers` (`GameLogic.cpp:593`) take this step.
    pub fn adjust_placed_unit(&mut self, id: ObjectId) {
        let Some(o) = self.objects.get(&id) else { return };
        // The AI predicate of `create_object_with`: a locomotor or a locomotor set.
        if o.is_kind_of(crate::template::kind_of::IMMOBILE) {
            return;
        }
        let Some(loco) = o.template.locomotor_for(crate::template::LocomotorSet::Normal).or_else(|| o.template.locomotor_sets.values().next().copied()) else {
            return;
        };
        let Some(po) = crate::pathfind::PathObject::for_locomotor(&o.template, &loco, id, o.player) else { return };
        let pos = o.position;
        let Some(adj) = self.pathfinder.adjust_destination(&self.terrain, &po, &pos, None) else { return };
        // `updateGoal(obj, &pos, LAYER_GROUND)` before `setPosition` (GameLogic.cpp:523). `updateGoal`
        // dispatches on `isDoingGroundMovement` (`AIPathfind.cpp:9816-9827`): an air-only placed unit
        // goes through `updateAircraftGoal` (which removes the old goal and, for a hover/wing
        // locomotor, marks aircraft goals), never the ground goal marks.
        let doing_ground_movement = self.is_doing_ground_movement(id);
        self.pathfinder.update_goal_dispatch(&po, &adj, doing_ground_movement);
        if let Some(o) = self.objects.get_mut(&id) {
            o.set_position(adj);
            o.mark_partition_dirty();
        }
        self.pathfinder.update_unit_pos(&po, &adj, false);
    }

    /// As [`create_object`](Self::create_object) plus extra behaviour modules from the adapter.
    #[track_caller]
    pub fn create_object_with(
        &mut self,
        template: &Arc<ObjectTemplate>,
        pos: Coord3D,
        angle: f32,
        player: PlayerIndex,
        extra_updates: Vec<Box<dyn UpdateModule>>,
    ) -> ObjectId {
        self.create_object_full(template, pos, angle, player, extra_updates, false, None, None)
    }

    /// `TheThingFactory->newObject(template, team)` for an explicit team: the object's controlling
    /// player is that team's (`Object::getControllingPlayer`, `Object.cpp:1620`). C++ passes a team
    /// for map objects (`GameLogic.cpp:1927`), script-created units (`ScriptActions.cpp:1004`,
    /// `:1195`), a spawner's own team (`SpawnBehavior.cpp:629`) and a factory's default team
    /// (`ProductionUpdate.cpp:773`), among others.
    ///
    /// The team table lives in the script engine, so this takes the engine out for the lookups; a
    /// script action that creates a unit must instead call
    /// [`create_object_in_team_with`](Self::create_object_in_team_with) with the engine it runs on
    /// (`script_engine_update` has already taken `self.scripts` there, so this entry point would see
    /// an empty team table).
    #[track_caller]
    pub fn create_object_in_team(&mut self, template: &Arc<ObjectTemplate>, pos: Coord3D, angle: f32, team: &str) -> ObjectId {
        let mut engine = std::mem::take(&mut self.scripts);
        let id = self.create_object_in_team_with(template, pos, angle, team, &mut engine);
        self.scripts = engine;
        id
    }

    /// [`create_object_in_team`](Self::create_object_in_team) on an explicit script engine
    /// (`ScriptActions.cpp:1195`'s `newObject(thingTemplate, theTeam)` while the engine is out of
    /// `GameLogic`). The owner and the team membership come from that engine, and `Object::Object`'s
    /// `setTeam` (`Object.cpp:312`) runs inside creation -- before the modules' first update -- so the
    /// object is in its team's member list for everything the same script action does afterwards
    /// (`ScriptActions.cpp:607`, `:662`, `:730` iterate the team in the same call).
    #[track_caller]
    pub(crate) fn create_object_in_team_with(
        &mut self,
        template: &Arc<ObjectTemplate>,
        pos: Coord3D,
        angle: f32,
        team: &str,
        scripts: &mut crate::script::ScriptEngine,
    ) -> ObjectId {
        let key = team.to_ascii_lowercase();
        let player = scripts.team_player(&key).or_else(|| self.neutral_player()).unwrap_or(0);
        self.create_object_full(template, pos, angle, player, Vec::new(), false, Some(&key), Some(scripts))
    }

    /// A structure created under construction (`DozerAIUpdate::construct`, `BuildAssistant`): its
    /// `OBJECT_STATUS_UNDER_CONSTRUCTION` is set before its modules run, so no special-power recharge
    /// or upgrade update happens until construction completes (SpecialPowerModule.cpp:114,
    /// Object.cpp:2481).
    #[track_caller]
    pub(crate) fn create_object_under_construction(&mut self, template: &Arc<ObjectTemplate>, pos: Coord3D, angle: f32, player: PlayerIndex) -> ObjectId {
        self.create_object_full(template, pos, angle, player, Vec::new(), true, None, None)
    }

    #[track_caller]
    #[allow(clippy::too_many_arguments)] // the creation path's C++ inputs (template, transform, owner, modules, status, team, engine)
    fn create_object_full(
        &mut self,
        template: &Arc<ObjectTemplate>,
        mut pos: Coord3D,
        angle: f32,
        player: PlayerIndex,
        extra_updates: Vec<Box<dyn UpdateModule>>,
        under_construction: bool,
        initial_team: Option<&str>,
        scripts: Option<&mut crate::script::ScriptEngine>,
    ) -> ObjectId {
        let id = self.allocate_object_id(&template.name, std::panic::Location::caller());
        if template.locomotor.is_some_and(|l| l.stick_to_ground) {
            pos.z = self.terrain.surface_height(pos.x, pos.y, pos.z);
        }
        // The modules' constructors draw in the template's INI module order (`Object::Object` builds
        // `m_behaviors` with `mi.getNthName(modIdx)`, Object.cpp:300-410): `LifetimeUpdate`'s lifetime
        // (LifetimeUpdate.cpp:77), `DeletionUpdate`'s lifetime (DeletionUpdate.cpp:74), the `AIUpdateInterface`
        // constructor's turret idle scans and locomotor wander draws (AIUpdate.cpp:288-296), the
        // `StealthDetectorUpdate` phasing (StealthDetectorUpdate.cpp:88-90) and `AutoHealBehavior`'s
        // starts-active phase (AutoHealBehavior.cpp:98-105). Only `onObjectCreated` work comes after all of
        // them (Object.cpp:474-477), which is the AI state machine's idle entries.
        let want_ai = template.locomotor.is_some() || !template.locomotor_sets.is_empty() || template.ai_update.is_some();
        let mut lifetime_die = 0;
        let mut deletion_die = 0;
        let mut turret_scan = None;
        let mut detect_next = None;
        // One wake per AutoHeal module, in the draw order (= the template's module order).
        let mut auto_heal_wakes: Vec<u32> = Vec::new();
        let mut horde_wake = 0;
        for draw in ctor_draws(template, want_ai) {
            match draw {
                CtorDraw::Lifetime => {
                    // `LifetimeUpdate::calcSleepDelay` (`LifetimeUpdate.cpp:77`) draws
                    // `GameLogicRandomValue(min, max)` unconditionally, so `min == max` still advances the generator.
                    lifetime_die = template
                        .lifetime
                        .map(|(lo, hi)| {
                            let delay = (self.rng.value(lo as i32, hi as i32) as u32).max(1);
                            self.frame.wrapping_add(delay)
                        })
                        .unwrap_or(0);
                }
                CtorDraw::Deletion => {
                    deletion_die = template
                        .deletion
                        .map(|(lo, hi)| {
                            let delay = (self.rng.value(lo as i32, hi as i32) as u32).max(1);
                            self.frame.wrapping_add(delay)
                        })
                        .unwrap_or(0);
                }
                CtorDraw::Ai => {
                    // The `TurretAI` constructor runs inside the `AIUpdate` constructor, before
                    // `chooseLocomotorSet` (`AIUpdate.cpp:288-296`), and its `initDefaultState` enters `IDLE`,
                    // whose `resetIdleScan` draws (`TurretAI.cpp:321`, `:1286-1291`).
                    turret_scan = crate::turret_ai::has_turret(template).then(|| self.turret_creation_idle_scan(&template.combat));
                    if let Some(n) = template.ai_update {
                        self.ai_creation_locomotor_draws(n);
                    }
                }
                CtorDraw::StealthDetector => {
                    if let Some(d) = template.stealth_detector.as_ref().filter(|d| !d.initially_disabled) {
                        let phase = self.rng.value(1, d.detection_rate as i32) as u32;
                        detect_next = Some(self.frame.saturating_add(phase));
                    }
                }
                CtorDraw::AutoHeal => {
                    let module = template.auto_heal.get(auto_heal_wakes.len());
                    let starts_active = module.is_some_and(|t| t.starts_active);
                    let wake = if starts_active {
                        // `GameLogicRandomValue(1, delay)` (AutoHealBehavior.cpp:98-109). A zero delay has `hi - lo + 1 == 0`,
                        // which returns `hi` without drawing under RETAIL_COMPATIBLE_CRC (RandomValue.cpp:267-272), so the
                        // module wakes on the constructor's frame (lane 185, R22-6).
                        let delay = module.map_or(1, |t| t.healing_delay);
                        if delay == 0 {
                            self.frame
                        } else {
                            self.frame.saturating_add(self.rng.value(1, delay.min(i32::MAX as u32) as i32) as u32)
                        }
                    } else {
                        UPDATE_SLEEP_FOREVER
                    };
                    auto_heal_wakes.push(wake);
                }
                CtorDraw::Horde => {
                    // `HordeUpdate::HordeUpdate` (HordeUpdate.cpp:161-169): the first wake is a random phase in
                    // `1..=UpdateRate`, drawn here in the module order.
                    if let Some(t) = template.horde.as_ref() {
                        horde_wake = self.horde_constructor_wake(t.update_rate, self.frame);
                    }
                }
            }
        }
        // `AIUpdateInterface::onObjectCreated` (Object.cpp:474-477 -> AIUpdate.cpp:621-631): the state machine is
        // built and enters `AI_IDLE`, which draws the sleep offsets. This runs after every constructor.
        let creation_idle = template.ai_update.map(|_| self.ai_creation_idle_draws());
        // `PhysicsBehavior::PhysicsBehavior` ends with `setWakeFrame(getObject(), UPDATE_SLEEP_NONE)`
        // (`PhysicsUpdate.cpp:231-233`): `UpdateModule::setWakeFrame` schedules `now + 1`
        // (UpdateModule.cpp:80-84), and `registerObject` keeps a non-zero frame (GameLogic.cpp:4157-4175),
        // so the module's first update is one frame after creation.
        // The body keeps the `GameData` damage thresholds it reads (`calcDamageState`, `ActiveBody.cpp:100`).
        let (damaged, really) = (self.rules.unit_damaged_threshold, self.rules.unit_really_damaged_threshold);
        let mut obj = Object {
            id,
            template: template.clone(),
            player,
            // `Object::Object`: `setTeam(team ? team : neutral player's default team)` (`Object.cpp:309-312`);
            // `assign_initial_team` below runs it once the object is in the list.
            team: None,
            position: pos,
            // The first trigger check runs on the first pass (`Object::setTriggerAreaFlagsForChangeInPosition`).
            trigger: crate::guard_area::TriggerMembership { inside: Vec::new(), pending: true },
            angle,
            // `Thing::Thing` starts from `Make_Identity` (`Thing.cpp:74`); `set_position` and `set_orientation` after the
            // literal place the object as the creation does.
            transform: crate::transform::Matrix3D::identity(),
            // `Object::m_shroudClearingRange = tt->friend_calcShroudClearingRange()`; `-1` means `VisionRange`
            // (`Object.cpp:286-289`). `DynamicShroudClearingRangeUpdate` changes it through
            // `setShroudClearingRange` (`DynamicShroudClearingRangeUpdate.cpp:283`).
            shroud_clearing_range: {
                let r = template.shroud_clearing_range;
                if r < 0.0 { template.vision_range } else { r }
            },
            body: (template.max_health > 0.0).then(|| {
                Box::new(ActiveBody::new(template.max_health, template.initial_health, damaged, really)
                        .with_armor(template.combat.armor.clone())
                        .with_subdual(template.subdual_damage_cap, template.subdual_heal_rate, template.subdual_heal_amount))
                    as Box<dyn crate::body::BodyModule>
            }),
            // A template without an `ActiveBody` gets the default `Body = InactiveBody` (`data/ini/default/object.ini:89`),
            // whose constructor sets `EFFECTIVELY_DEAD` (`InactiveBody.cpp:47-50`: `getObject()->setEffectivelyDead(true)`).
            // Every `isEffectivelyDead()` reader therefore sees a bodyless object (a map's ambient sound markers,
            // crates, ...) as dead: it is not selectable, not a target, and `BuildAssistant::isRemovableForConstruction`
            // clears it under a new building's footprint (`BuildAssistant.cpp:1381-1382`).
            effectively_dead: template.max_health <= 0.0,
            ai: AiState::default(),
            locomotor_set: crate::template::LocomotorSet::Normal,
            preferred_height_override: None,
            loco_upgraded: false,
            loco_rt: Default::default(),
            jet: Default::default(),
            parking: template.parking_place.is_some().then(|| crate::jet_ai::ParkingRt::new(template)),
            physics: template.physics.is_some().then(|| {
                let mut p = crate::physics::Physics::new();
                // `Locomotor::Locomotor`'s ctor (`Locomotor.cpp:658`, `:676`): `m_brakingFactor = 1.0f`
                // (the `Physics` default) and `m_donutTimer = getFrame() + DONUT_TIME_DELAY_SECONDS *
                // LOGICFRAMES_PER_SECOND`. The AIUpdate creates its Locomotors during module
                // construction, i.e. at the creation frame.
                p.donut_timer = self.frame + crate::locomotor::DONUT_TIME_DELAY_FRAMES;
                p
            }),
            combat: ObjectCombat::new(template, angle, self.frame, turret_scan),
            behavior: {
                let mut b = crate::behaviors::BehaviorState { lifetime_die, deletion_die, ..Default::default() };
                // `HordeUpdate`'s constructor: `m_lastHordeRefreshFrame = getFrame()` (HordeUpdate.cpp:165) and the
                // wake drawn above (`horde_wake`, read by `family_registrations`).
                b.horde_last_refresh = self.frame;
                b.horde_wake = horde_wake;
                // `StealthUpdate`'s constructor: `m_stealthAllowedFrame = getFrame() + StealthDelay` (StealthUpdate.cpp:129).
                if let Some(st) = &template.stealth {
                    b.stealth_allowed = self.frame.saturating_add(st.stealth_delay);
                }
                // `CountermeasuresBehavior`'s constructor seeds the available volleys; the demo
                // trap's constructor starts in proximity or manual mode.
                if let Some(cm) = &template.countermeasures {
                    b.cm_available = cm.number_of_volleys * cm.volley_size;
                }
                if let Some(dt) = &template.demo_trap {
                    b.trap_proximity = dt.default_proximity_mode;
                }
                // `DynamicShroudClearingRangeUpdate`'s constructor (DynamicShroudClearingRangeUpdate.cpp:84-128):
                // the countdown and the three deadlines derived from the module data, the frame it must be done by,
                // and `m_nativeClearingRange = obj->getShroudClearingRange()`. `m_stateCountDown` is an `Int` and the
                // deadlines `UnsignedInt`; the shipped values are small, so wrapping subtraction mirrors the engine.
                if let Some(ds) = &template.dynamic_shroud {
                    let total = ds.shrink_delay.wrapping_add(ds.shrink_time);
                    b.ds_state = crate::sleepy_families::DS_STATE_NOT_STARTED_YET;
                    b.ds_state_countdown = total as i32;
                    b.ds_total_frames = total.max(1) as i32;
                    b.ds_shrink_start_deadline = total.wrapping_sub(ds.shrink_delay);
                    b.ds_grow_start_deadline = total.wrapping_sub(ds.grow_delay);
                    b.ds_sustain_deadline = b.ds_grow_start_deadline.wrapping_sub(ds.grow_time);
                    b.ds_done_forever_frame = self.frame.wrapping_add(total);
                    b.ds_change_interval_countdown = 0;
                    b.ds_native_clearing_range = if under_construction {
                        template.geometry.bounding_circle_radius()
                    } else {
                        let r = template.shroud_clearing_range;
                        if r < 0.0 { template.vision_range } else { r }
                    };
                    b.ds_current_clearing_range = 0.0;
                }
                b
            },
            production: template.production.as_ref().map(ProductionState::new),
            superweapon: Default::default(),
            contained_by: None,
            contained: Vec::new(),
            contained_mass: 0.0,
            contain_passengers_allowed_to_fire: false,
            enter_target: None,
            exit_at: 0,
            contained_frame: 0,
            under_construction,
            construction_percent: 0.0,
            construction_arrived: false,
            build_target: None,
            builder: None,
            bridge: None,
            master: None,
            spawn: Default::default(),
            payload_created: false,
            repair_target: None,
            // `DozerPrimaryIdleState::onEnter`: the idle-too-long timestamp starts at the current frame.
            dozer_idle_frame: self.frame,
            boxes: template.dock.as_ref().map_or(0, |d| d.starting_boxes),
            supply_target: None,
            supply_timer: 0,
            dock_state: Default::default(),
            supply_ai: Default::default(),
            upgrades: std::collections::BTreeSet::new(),
            applied_upgrades: std::collections::BTreeSet::new(),
            armor_set_flags: 0,
            weapon_set_flags: 0,
            weapon_bonus_player_upgrade: false,
            experience: crate::experience::ExperienceTracker::new(template.is_trainable),
            special_power_ready: std::collections::BTreeMap::new(),
            special_power_pause: std::collections::BTreeMap::new(),
            ability: None,
            hack_internet: None,
            producer_id: ObjectId::INVALID,
            undetected_defector: false,
            spawned: Vec::new(),
            sp_creator: ObjectId::INVALID,
            disabled_till_frame: [crate::disabled::NEVER; crate::disabled::DISABLED_COUNT],
            disabled_mask: 0,
            update_handles: Vec::new(),
            ai_handle: None,
            destroyed: false,
            status_bits: if template.stealth.as_ref().is_some_and(|s| s.innate_stealth) {
                // `StealthUpdate`'s constructor (`StealthUpdate.cpp:147-151`): an innate stealth unit gets
                // `OBJECT_STATUS_CAN_STEALTH` so other code can test the bit (the group move/stop stealth rule).
                crate::object::status::CAN_STEALTH
            } else {
                0
            },
            model_conditions: crate::condition::ModelConditionFlags::default(),
            upgrade_conditions: 0,
            rider: Default::default(),
            last_damage_info: crate::body::DamageInfo { source: ObjectId::INVALID, amount: 0.0, kind: crate::damage::DamageType::Unresistable },
            last_damage_cleared: false,
            last_damage_timestamp: 0,
            // Newly registered partition data is dirty in the engine (`registerObject` ->
            // `makeDirty(true)`), so a freshly spawned unit checks collisions on its first frame.
            partition_dirty: true,
            collision_dirty: false,
            dirty_stamp: crate::object::next_dirty_stamp(),
            railroad_hitched: false,
            drawable_hidden: false,
        };
        // `Thing::setPosition(pos)` then `setOrientation(angle)` at creation (the matrix rows come from the angle,
        // the cached heading is normalised).
        obj.set_position(pos);
        obj.set_orientation(angle);
        // The main machine entered `AI_IDLE` at construction: its first update does not enter again.
        if let Some(off) = creation_idle {
            obj.ai.machine.idle_inited = true;
            obj.ai.machine.idle_sleep_offset = off;
            // `AIIdleState::onEnter` at construction also runs `ai->resetNextMoodCheckTime()` (`AIStates.cpp:1321` ->
            // `AIUpdate.cpp:4498-4502`): the first mood check is `ForceIdleMSEC` frames after the creation frame, and
            // that check carries the one random offset. The later `onEnter`s of the sub-machines reset it to the same
            // value, so the main machine's entry is the one that stays.
            obj.combat.next_mood_check = self.frame.saturating_add(self.ai_tunables.force_idle_frames);
            obj.combat.randomly_offset_mood_check = true;
        }
        // The `AIUpdateInterface` module's first update is one frame after creation
        // (`setWakeFrame(getObject(), UPDATE_SLEEP_NONE)` in its ctor, `AIUpdate.cpp:299-301`, and
        // `UpdateModule::setWakeFrame` schedules `now + 1`, `UpdateModule.cpp:80-84`). Every part of that
        // module in the port is gated on this frame: the state machine, and the dozer/worker/supply/hack/
        // chinook halves that the engine keeps in `DozerAIUpdate::update` and friends.
        if template.ai_update.is_some() {
            obj.ai.wake_frame = self.frame.saturating_add(1);
        }
        if let Some(next) = detect_next {
            obj.behavior.detect_next = next;
        }
        // registerObject: modules get a wake frame of max(now, 1).
        let now = self.frame.max(1);
        // The `Object` constructor's helper modules come first in the behaviour array (Object.cpp:320-395), so
        // they are pushed before the AI and the behaviour modules (`object_helpers.rs`).
        for (kind, wake) in object_helper_wakes(template, self.frame) {
            let h = self.schedule.register(id, Box::new(ObjectHelper { kind, wake }), now);
            if kind == crate::object_helpers::HelperKind::SubdualDamage {
                self.schedule.slot_mut(h.0).subdual_heal = true;
            }
            self.schedule.slot_mut(h.0).types_to_process = crate::sleepy_families::helper_types_to_process(kind);
            obj.update_handles.push(h);
        }
        // The behaviour modules (`registerObject`, GameLogic.cpp:4165-4200): AI, the families and the railroad, in the
        // template's INI module order (`update_module_order`, lane 110). A module the list does not name (a synthetic
        // template, or a kind the list has no variant for) follows the listed ones, in the legacy order below.
        let regs = self.family_registrations(template, self.frame, &mut obj.behavior, &auto_heal_wakes);
        let want_ai = template.locomotor.is_some() || !template.locomotor_sets.is_empty() || template.ai_update.is_some();
        let mut pending: Vec<PendingModule> = Vec::new();
        if want_ai || template.update_module_order.contains(&crate::template::SleepyModuleKind::Ai) {
            pending.push(PendingModule::Ai);
        }
        for (kind, wake, die_index) in regs {
            pending.push(PendingModule::Family(kind, wake, die_index));
        }
        if template.railroad.is_some() {
            pending.push(PendingModule::Railroad);
        }
        let order = &template.update_module_order;
        // `registerObject` walks the object's `m_behaviors` array, which `Object::Object` built in the template's INI
        // module order (`Object.cpp:300-410`; `mi.getNthName` skips cleared entries), and `update_module_order` holds
        // that order (lane 110). A kind can appear several times: the US ranger carries two `CommandButtonHuntUpdate`
        // modules (`AmericaInfantry.ini` ModuleTag_02 and ModuleTag_09) with the AI, physics and production modules
        // between them, so the walk consumes one pending module per order entry, in registration order. Modules the
        // order does not name (a synthetic template, or a kind the list has no variant for) follow the listed ones in
        // the legacy order.
        let kind_of = |p: &PendingModule| match p {
            PendingModule::Ai => crate::template::SleepyModuleKind::Ai,
            PendingModule::Family(k, _, die) => k.sleepy_kind(*die),
            PendingModule::Railroad => crate::template::SleepyModuleKind::Railroad,
        };
        let mut ordered: Vec<PendingModule> = Vec::with_capacity(pending.len());
        let mut used = vec![false; pending.len()];
        for entry in order {
            for (i, p) in pending.iter().enumerate() {
                if !used[i] && kind_of(p) == *entry {
                    ordered.push(*p);
                    used[i] = true;
                    break;
                }
            }
        }
        for (i, p) in pending.iter().enumerate() {
            if !used[i] {
                ordered.push(*p);
            }
        }
        for p in ordered {
            match p {
                PendingModule::Ai => {
                    // `AIUpdateInterface`'s ctor calls `setWakeFrame(getObject(), UPDATE_SLEEP_NONE)` (AIUpdate.cpp:299-301),
                    // which is creation frame + 1 whatever the frame: the first AI update is the next frame.
                    let h = self.schedule.register_ai(id, Box::new(AiUpdate), self.frame.saturating_add(1));
                    obj.update_handles.push(h);
                    obj.ai_handle = Some(h);
                }
                PendingModule::Family(kind, wake, die_index) => {
                    let h = self.schedule.register_family(id, kind, wake, now);
                    obj.update_handles.push(h);
                    obj.behavior.set_family_handle(kind, die_index, h);
                    if kind == crate::sleepy_families::FamilyKind::Production
                        || kind == crate::sleepy_families::FamilyKind::ProductionIdle
                    {
                        // `ProductionUpdate`'s `DisabledTypesToProcess` (production.rs, default HELD): the gate of the sleepy loop.
                        self.schedule.slot_mut(h.0).types_to_process =
                            template.production.as_ref().map_or(0, |p| p.disabled_types_to_process);
                    }
                }
                PendingModule::Railroad => {
                    // `RailroadBehavior` (the locomotive and every carriage): the module's first update is one
                    // frame after creation (`PhysicsBehavior::PhysicsBehavior` ends with
                    // `setWakeFrame(getObject(), UPDATE_SLEEP_NONE)`, PhysicsUpdate.cpp:231-233; the railroad
                    // module derives from `PhysicsBehavior`), so a carriage created during a frame's update
                    // first runs on the next frame. The state is per object, like the C++ module.
                    let is_locomotive = template.railroad.as_ref().is_some_and(|r| r.is_locomotive);
                    let h = self.schedule.register(id, Box::new(crate::railroad::RailroadUpdate), self.frame.saturating_add(1));
                    // `RailroadBehavior` derives from `PhysicsBehavior`, whose `getDisabledTypesToProcess` is ALL (PhysicsUpdate.h:220).
                    self.schedule.slot_mut(h.0).types_to_process = u16::MAX;
                    obj.update_handles.push(h);
                    self.railroad_states.insert(id, crate::railroad::RailroadState::new(is_locomotive));
                }
            }
        }
        for m in extra_updates {
            let h = self.schedule.register(id, m, now);
            obj.update_handles.push(h);
        }
        // Inside a frame the cells are placed by the end-of-frame partition pass (`partition_dirty` is set
        // above); setup code outside a frame is followed by `ThePartitionManager->update()` in C++ (GameLogic.cpp:2135).
        if self.in_update {
            self.partition.register_deferred(id, pos, template.geometry.bounding_circle_radius());
        } else {
            self.partition.register(id, pos, template.geometry.bounding_circle_radius());
        }
        // addObjectToPathfindMap: structures become obstacles.
        self.pathfinder.classify_object_footprint(&self.terrain, id, template, &pos, angle, true);
        // `GameLogic::createObject` -> `addWallPiece` for a `KINDOF_WALK_ON_TOP_OF_WALL` object.
        if template.is_kind_of(crate::template::kind_of::WALK_ON_TOP_OF_WALL) {
            self.pathfinder.add_wall_piece(id, &template.geometry, &pos, angle);
        }
        self.objects.insert(id, obj);
        // `Object::Object` (Object.cpp:581): a non-projectile, non-inert object stamps the trigger-area clock.
        if !template.is_kind_of(crate::template::kind_of::PROJECTILE) && !template.is_kind_of(crate::template::kind_of::INERT) {
            self.note_trigger_area_change();
        }
        // `Object::Object`'s `setTeam` (`Object.cpp:312`): before the modules get their first update.
        self.assign_initial_team(id, initial_team, scripts);
        // `Locomotor::setPhysicsOptions`: the locomotor's ground-friction flags reach the physics.
        self.apply_locomotor_physics_options(id);
        // `JetAIUpdate::onObjectCreated`: parked until proven otherwise (taxi locomotor).
        self.jet_on_created(id);
        // `TunnelContain::onObjectCreated` / `CreateModule`: register with the player's network.
        self.on_tunnel_created(id);
        // `Object::initObject` runs every module's `onObjectCreated` before `registerObject`
        // (Object.cpp:473-484): `OverlordContain` / `HelixContain` / `MobNexusContain` /
        // `GarrisonContain` create their initial payload here, so its id follows the container's.
        self.contain_on_object_created(id);
        // `CashBountyPower::onObjectCreated` in the same loop (CashBountyPower.cpp:134; science-gated, cash_bounty_l193.rs).
        self.cash_bounty_on_object_created(id);
        // addUnitToPathfindMap (`Pathfinder::updatePos`): mobile units occupy cells. `updatePos` reads
        // the live object's current locomotor set (`AIPathfind.cpp:10420-10433`, `getRadiusAndCenter`,
        // `isDoingGroundMovement`), and the module `onObjectCreated` calls above (the jet's taxi
        // locomotor) have already run.
        if template.locomotor.is_some() && template.kind_of & crate::template::kind_of::IMMOBILE == 0 {
            if let Some(po) = self.objects.get(&id).and_then(PathObject::for_object) {
                self.pathfinder.update_unit_pos(&po, &pos, false);
            }
        }
        // `VeterancyGainCreate` (starting level). `SpawnBehavior` makes its replacement list on its first update
        // (`SpawnBehavior::update`, :203-225), not here: its constructor only sets the defaults.
        // The `SpecialPowerModule` constructors (recharge, `StartsPaused`), then the create modules'
        // `onCreate` (`ThingFactory::newObject` -> `create->onCreate()`, ThingFactory.cpp:333-341),
        // then the upgrade modules (`Object::initObject` -> `updateUpgradeModules`, Object.cpp:528;
        // not under construction).
        self.init_special_powers(id);
        self.grant_upgrade_create(id, false);
        if !under_construction {
            self.update_upgrade_modules(id);
        }
        self.apply_veterancy_gain(id);
        id
    }

    /// `destroyObject`: marks the object and queues it; deletion happens at the end of the frame.
    pub fn destroy_object(&mut self, id: ObjectId) {
        let (has_contained, contained_by) = match self.objects.get(&id) {
            Some(o) if !o.destroyed => (!o.contained.is_empty() || self.is_tunnel_container(id), o.contained_by),
            _ => return,
        };
        // Containment (`OpenContain::onDie` / `removeFromContain`): a dying container damages and
        // ejects its passengers; a dying passenger leaves its container's list. A dying tunnel
        // (`TunnelContain::onDie`) instead tells the network, which caves in on the last one.
        if has_contained {
            self.on_container_destroyed(id);
        }
        if let Some(container) = contained_by {
            if self.is_tunnel_container(container) {
                self.tunnel_remove_from_contain(id);
            } else {
                // `Object::onDestroy` -> `removeFromContain`: a rider leaving a combat bike reverts the
                // rider table, and scuttles the bike (`RiderChangeContain::onRemoving`).
                if self.is_rider_change(container) {
                    self.rider_on_removing(container, id, false);
                }
                if let Some(c) = self.objects.get_mut(&container) {
                    c.contained.retain(|&x| x != id);
                }
                self.refresh_contained_mass(container);
            }
        }
        // `JetAIUpdate::onDelete`: give the parking space back.
        self.jet_on_delete(id);
        // `Object::onDestroy`'s module deletes (`Object.cpp:777-783`): a `RadarUpgrade` gives the
        // player's radar back and a `CostModifierUpgrade` removes its cost change.
        self.upgrade_module_on_delete(id);
        let Some(obj) = self.objects.get_mut(&id) else { return };
        if obj.destroyed {
            return;
        }
        obj.destroyed = true;
        // removeObjectFromPathfindMap: a destroyed structure stops blocking.
        self.pathfinder.classify_object_footprint(&self.terrain, id, &obj.template, &obj.position, obj.angle, false);
        // `GameLogic::destroyObject` -> `removeWallPiece` for a wall piece.
        if obj.template.is_kind_of(crate::template::kind_of::WALK_ON_TOP_OF_WALL) {
            self.pathfinder.remove_wall_piece(id);
        }
        self.pathfinder.clear_unit(id);
        self.partition.unregister(id);
        self.objects_to_destroy.push(id);
    }

    /// `processDestroyList`: erase each object's sleepy updates, then delete the object.
    fn process_destroy_list(&mut self) {
        // Destruction may enqueue more entries (sub-objects); the original iterates the live list.
        let mut i = 0;
        while i < self.objects_to_destroy.len() {
            let id = self.objects_to_destroy[i];
            i += 1;
            // `AIUpdateInterface::~AIUpdateInterface` (`AIUpdate.cpp:639`) halts the state machine,
            // and `AIDockMachine::halt` (`AIDock.cpp:70`) cancels the docker's dock reservation, so
            // a dead truck does not hold an approach slot or the active-docker role forever.
            if self.objects.get(&id).is_some_and(|o| o.ai.machine.current == crate::ai_states::AiStateType::Dock) {
                self.ai_dock_exit(id);
            }
            // `~Object` (`Object.cpp:645-646`) deletes `m_partitionLastValue` without removing it from the
            // cells, so an object destroyed while it still passes `addValue`'s test (a sold building: not
            // dead) leaves its value behind. A dead object's value was already removed by the
            // `handlePartitionCellMaintenance` in `onDestroy` (`Object.cpp:786`).
            if let Some(a) = self.value_affects.remove(&id) {
                let keep = self.objects.get(&id).is_some_and(|o| {
                    !o.under_construction && !o.is_dying() && crate::shroud::shroud_clearing_range(o) > 0.0
                });
                if !keep {
                    self.value_map.apply(&a, false);
                }
            }
            if let Some(obj) = self.objects.remove(&id) {
                // `~Object` (Object.cpp:606): a non-projectile, non-inert object stamps the trigger-area clock.
                if !obj.is_kind_of(crate::template::kind_of::PROJECTILE) && !obj.is_kind_of(crate::template::kind_of::INERT) {
                    self.note_trigger_area_change();
                }
                // `GameLogic::destroyObject`'s erase (GameLogic.cpp:2578-2609), in the engine's order.
                self.schedule.erase_object(id);
                // `~Object` -> `setTeam(nullptr)` (`Object.cpp:623`): leave the team's member list.
                if let Some(team) = &obj.team {
                    self.scripts.remove_object_from_team(team, id);
                }
            }
            // The railroad state lives beside the object (the C++ module is deleted with it).
            self.railroad_states.remove(&id);
        }
        self.objects_to_destroy.clear();
    }

    /// `Object::attemptDamage` followed by the die-module pipeline, with a default `NORMAL` death
    /// type (weapon damage calls [`damage_object_with_death`](Self::damage_object_with_death)).
    pub fn damage_object(&mut self, id: ObjectId, info: DamageInfo) -> f32 {
        self.damage_object_with_death(id, info, DeathType::Normal)
    }

    /// `Object::attemptDamage` + `Object::onDie` (`ActiveBody::attemptDamage` calls `onDie` when
    /// health crosses zero). The die modules decide whether and when the object is destroyed.
    pub fn damage_object_with_death(&mut self, id: ObjectId, info: DamageInfo, death: DeathType) -> f32 {
        // `ActiveBody::attemptDamage` returns at its top for an effectively dead object (`ActiveBody.cpp:357`),
        // before its retaliation block; the hit that kills the object still retaliates ("even if I'm dead").
        let already_dead = self.objects.get(&id).is_none_or(|o| o.destroyed || o.effectively_dead);
        // `DAMAGE_KILL_PILOT` (`ActiveBody`): the driver is killed; a vehicle becomes unmanned and
        // neutral instead of taking health damage (Jarmen Kell's snipe).
        if info.kind == crate::damage::DamageType::KillPilot {
            let neutral = self.neutral_player();
            let is_vehicle = self
                .objects
                .get(&id)
                .is_some_and(|o| !o.destroyed && o.template.kind_of & crate::template::kind_of::VEHICLE != 0);
            if is_vehicle {
                // A carbomb's dead-man's trigger detonates when its driver is sniped; the sniper
                // scores the kill (`Object::setDisabledUntil`, Object.cpp:2214-2227).
                if self.objects.get(&id).is_some_and(|o| o.has_carbomb_weapon_set()) {
                    self.damage_object_with_death(
                        id,
                        DamageInfo { source: info.source, amount: crate::damage::HUGE_DAMAGE_AMOUNT, kind: crate::damage::DamageType::Unresistable },
                        DeathType::Normal,
                    );
                    return 0.0;
                }
                // A combat bike with a rider: the rider leaves (scuttling the bike) or the moving bike
                // blows up (`ActiveBody.cpp:394-421`). A bike with no rider is unmanned as usual.
                if self.is_rider_change(id) && !self.contained_units(id).is_empty() {
                    self.rider_kill_pilot(id, info.source);
                    if !already_dead {
                        self.retaliate_after_damage(id, &info);
                    }
                    return 0.0;
                }
                // `ActiveBody` (`DAMAGE_KILL_PILOT`): `setDisabled(DISABLED_UNMANNED)`, `aiIdle` and
                // the neutral team (`ActiveBody.cpp:426`).
                self.set_disabled(id, crate::disabled::DisabledType::Unmanned);
                let Some(o) = self.objects.get_mut(&id) else { return 0.0 };
                o.ai.stop();
                o.combat.attack = None;
                // `ActiveBody`: the vehicle goes to the neutral player (`setTeam(neutral default team)`).
                if let Some(n) = neutral {
                    self.set_object_team(id, n);
                }
                // The kill-pilot path falls through to `attemptDamage`'s retaliation block in the C++
                // (`ActiveBody.cpp:688-725`), even though it dealt no health damage.
                if !already_dead {
                    self.retaliate_after_damage(id, &info);
                }
                return 0.0;
            }
        }
        // `DAMAGE_KILL_GARRISONED` (`ActiveBody::attemptDamage`, ActiveBody.cpp:445-478): the building's
        // garrison loses `REAL_TO_INT_FLOOR(m_amount)` occupants and the building takes no health damage
        // (`alreadyHandled`). The amount is the hit's raw `m_amount`, not the armour-adjusted value.
        if info.kind == crate::damage::DamageType::KillGarrisoned {
            let kills = info.amount.floor() as i32;
            self.kill_garrisoned(id, kills, info.source, 0, 0);
            if !already_dead {
                self.retaliate_after_damage(id, &info);
            }
            return 0.0;
        }
        let (dealt, died, old_state, new_state, was_subdued) = {
            let Some(obj) = self.objects.get_mut(&id) else { return 0.0 };
            if obj.destroyed {
                return 0.0;
            }
            let Some(body) = obj.body.as_mut() else { return 0.0 };
            // `ActiveBody::attemptDamage` (`ActiveBody.cpp:529`): `oldState` is the stored `m_curDamageState`.
            let was_alive = !body.is_dead();
            let was_subdued = body.is_subdued();
            let old = body.damage_state();
            let dealt = body.attempt_damage(&info);
            let new = body.damage_state();
            (dealt, was_alive && body.is_dead(), old as u8, new as u8, was_subdued)
        };
        // The subdual branch of `ActiveBody::attemptDamage` (`ActiveBody.cpp:495-513`): wake the
        // `SubdualDamageHelper` and run `onSubdualChange` (`object_helpers.rs`). The armour-adjusted
        // subdual amount is the hit's `amount` for the last-damage record below.
        let subdual_hit = if info.kind.is_subdual() {
            let amount = self.objects.get(&id).and_then(|o| o.body.as_ref()).map_or(0.0, |b| b.last_subdual_hit());
            self.after_subdual_hit(id, was_subdued);
            amount
        } else {
            0.0
        };
        // `ActiveBody::attemptDamage` records the last attacker for the guard aggressor (`:527-600`). A
        // subdual hit falls through to the same block with `alreadyHandled` set: it records, but
        // `internalChangeHealth` (and so the health-decrease damage-module callbacks) is skipped. The
        // block's gate is `amount > 0`, so a subdual heal (negative) records nothing. Status damage is
        // applied in `combat.rs` before it reaches the body. Healing never records.
        let handled_amount = if info.kind.is_subdual() { subdual_hit } else { dealt };
        if info.source != ObjectId::INVALID
            && info.kind != crate::damage::DamageType::Healing
            && handled_amount > 0.0
        {
            self.record_last_damage(id, &info);
        }
        // `TransitionDamageFX` (client FX hook) and `ProneUpdate`.
        self.on_transition_damage(id, old_state, new_state);
        if dealt > 0.0 {
            self.on_prone_damage(id, dealt);
            // `TunnelContain::update`: remember the enemy that hit a tunnel for the guard AI.
            if info.source != ObjectId::INVALID && self.is_tunnel_container(id) {
                self.update_tunnel_nemesis(id, info.source);
            }
        }
        // `BridgeBehavior` / `BridgeTowerBehavior`: propagate the hit and sync the terrain `Bridge`
        // (`Bridge::updateDamageState`).
        if self.objects.get(&id).is_some_and(|o| o.template.is_bridge || o.bridge.is_some()) {
            self.on_bridge_damage(id, dealt, info.source);
            if self.objects.get(&id).is_some_and(|o| o.template.is_bridge) {
                self.update_bridge_state(id);
            }
        }
        // `AutoHealBehavior::onDamage`, `BaseRegenerateUpdate::onDamage`, `FlammableUpdate::onDamage` and
        // `PoisonedBehavior::onDamage` (`ActiveBody::attemptDamage`, ActiveBody.cpp:620).
        if info.kind == crate::damage::DamageType::Healing {
            if dealt > 0.0 {
                self.on_poison_healing(id);
            }
        } else if dealt > 0.0 {
            self.auto_heal_on_damage(id);
            self.base_regen_on_damage(id, &info);
            // `SpawnBehavior::onDamage` (a damage interface on the update module, SpawnBehavior.cpp:797-821).
            self.spawn_behavior_on_damage(id, dealt);
            match info.kind {
                crate::damage::DamageType::Flame | crate::damage::DamageType::ParticleBeam => self.on_flame_damage(id, dealt),
                crate::damage::DamageType::Poison => self.on_poison_damage(id, dealt, death),
                _ => {}
            }
        }
        if died {
            if let Some(o) = self.objects.get_mut(&id) {
                if o.combat.death.is_none() {
                    o.combat.death = Some(death);
                }
            }
            let overkill = (info.amount - dealt).max(0.0);
            // `ActiveBody::attemptDamage` (ActiveBody.cpp:672): `damager->scoreTheKill(obj)` before `onDie`.
            self.score_the_kill(id, info.source);
            self.grant_kill_experience(id, Some(info.source)); // ExperienceTracker kill points
            // `TechBuildingBehavior::onDie` (a die interface on the update module, TechBuildingBehavior.cpp:122-132).
            self.tech_building_on_die(id);
            // `SpawnBehavior::onDie` (a die interface on the update module, SpawnBehavior.cpp:134-189).
            self.spawn_behavior_on_die(id, death);
            self.on_die(id, death, overkill, info.source);
            self.refund_all_production(id); // ProductionUpdate::onDie
            // `ParkingPlaceBehavior::onDie`: `killAllParkedUnits`.
            if self.objects.get(&id).is_some_and(|o| o.parking.is_some()) {
                self.pp_kill_all_parked_units(id);
            }
        }
        // The `ActiveBody::attemptDamage` tail: `doDamageFX`, the repulsor block and the retaliation assist
        // (`ActiveBody.cpp:681-725`). The client FX and the repulsor status are not simulated.
        if !already_dead {
            self.retaliate_after_damage(id, &info);
        }
        dealt
    }

    /// `ActiveBody::attemptDamage`'s last-damage bookkeeping (`ActiveBody.cpp:573-598`): record the
    /// attacker and clear the cleared flag. Multiple hits in one or two frames prefer a source that
    /// is a vehicle, infantry or faction structure; the timestamp guards against overwriting a hit
    /// from this or the previous frame with a less useful one.
    pub(crate) fn record_last_damage(&mut self, id: ObjectId, info: &DamageInfo) {
        let frame = self.frame;
        let (old_ts, old_src) = match self.objects.get(&id) {
            Some(o) => (o.last_damage_timestamp, o.last_damage_info.source),
            None => return,
        };
        let overwrite = if old_ts != frame && old_ts != frame.wrapping_sub(1) {
            true
        } else {
            // Prefer the new source when it is a "real" combatant, or when the old one is gone.
            let new_exists = self.objects.contains_key(&info.source);
            let new_is_combatant = self.objects.get(&info.source).is_some_and(|o| {
                o.template.is_kind_of(crate::template::kind_of::VEHICLE)
                    || o.template.is_kind_of(crate::template::kind_of::INFANTRY)
                    || o.template.kind_of & crate::template::kind_of::STRUCTURE != 0
            });
            let old_exists = old_src != ObjectId::INVALID && self.objects.contains_key(&old_src);
            new_exists && (!old_exists || new_is_combatant)
        };
        if overwrite {
            if let Some(o) = self.objects.get_mut(&id) {
                o.last_damage_info = *info;
                o.last_damage_cleared = false;
                o.last_damage_timestamp = frame;
            }
        }
    }

    // ---- scheduler helpers -----------------------------------------------------------------

    /// `AIUpdateInterface::wakeUpNow` (AIUpdate.cpp:973-988): a module asleep past the next frame is set to wake
    /// on the next frame (`setWakeFrame(getObject(), UPDATE_SLEEP_NONE)`, `getWakeFrame() > UPDATE_SLEEP_NONE`). A
    /// module due this frame or the next keeps its slot: the pull is only forward, never a delay. A module that
    /// changes its own state during its update does nothing (`m_isInUpdate`), which the scheduler's running slot
    /// covers (`awaken` returns for the running module). The machine's own sleep (`m_sleepTill`) is not touched:
    /// only a state change clears it (`StateMachine::internalSetState`, StateMachine.cpp:567).
    pub(crate) fn wake_up_now(&mut self, id: ObjectId) {
        let now = self.frame;
        let Some(h) = self.objects.get(&id).and_then(|o| o.ai_handle) else { return };
        // `getWakeFrame()` is `UPDATE_SLEEP(nextCallFrame - now)` when the frame is still ahead, else NONE, so it is
        // `> UPDATE_SLEEP_NONE` exactly when the next call is past `now + UPDATE_SLEEP_NONE`.
        let next_call = self.schedule.slot(h.0).next_call_and_phase >> 2;
        if next_call > now.saturating_add(crate::module::UPDATE_SLEEP_NONE) {
            self.schedule.awaken(h, now, now.saturating_add(crate::module::UPDATE_SLEEP_NONE));
        }
    }

    /// `AIUpdateInterface::setQueueForPathTime(frames)` (AIUpdate.cpp:955-968): a path queued `frames` ahead wakes a
    /// module that sleeps past that, to `now + frames`; a module already due sooner is left alone, and nothing happens
    /// in the module's own update (`m_isInUpdate`). `frames == 0` (cleared queue) wakes nothing (`frames >= NONE`).
    pub(crate) fn wake_up_for_queue_time(&mut self, id: ObjectId, frames: u32) {
        let now = self.frame;
        if frames < crate::module::UPDATE_SLEEP_NONE {
            return;
        }
        let Some(h) = self.objects.get(&id).and_then(|o| o.ai_handle) else { return };
        let next_call = self.schedule.slot(h.0).next_call_and_phase >> 2;
        if next_call > now.saturating_add(frames) {
            self.schedule.awaken(h, now, now.saturating_add(frames));
        }
    }

    // ---- commands --------------------------------------------------------------------------

    /// Append a message to `TheCommandList`; applied at the start of the next `update()`.
    pub fn submit(&mut self, player: PlayerIndex, command: Command) {
        self.submit_from(player, crate::ai_states::CommandSource::Player, command);
    }

    /// [`submit`](Self::submit) with the engine command source. Script actions pass
    /// `CommandSource::Script` (the engine's `CMD_FROM_SCRIPT`).
    pub(crate) fn submit_from(&mut self, player: PlayerIndex, source: crate::ai_states::CommandSource, command: Command) {
        self.command_list.push(GameMessage { player, command, source });
    }

    /// Apply the queued messages now. The sequential-script evaluator calls this after each action,
    /// because retail orders take effect immediately and the script then tests the group's idle state.
    pub(crate) fn flush_command_list(&mut self) {
        if !self.command_list.is_empty() {
            self.process_command_list();
        }
    }

    /// Apply one message now, outside the frame's list (an order held by a hacker, which runs in its
    /// `update`). Messages already queued keep their place after it.
    pub(crate) fn apply_message_now(&mut self, msg: GameMessage) {
        let saved = std::mem::take(&mut self.command_list);
        self.command_list.push(msg);
        self.process_command_list();
        let added = std::mem::replace(&mut self.command_list, saved);
        self.command_list.extend(added);
    }

    /// `processCommandList`: apply messages in submission order.
    pub(crate) fn process_command_list(&mut self) {
        let list = std::mem::take(&mut self.command_list);
        for msg in list {
            // `HackInternetAIUpdate::aiDoCommand`: a hacking or packing unit holds its AI orders.
            let Some(msg) = self.hold_hacker_orders(msg) else { continue };
            // `AssaultTransportAIUpdate::aiDoCommand`'s default branch (AssaultTransportAIUpdate.cpp:109-115): a transport that
            // receives an AI command other than attack, attack-move or idle resets (its own branches run their own resets). The
            // reset is skipped for CMD_FROM_AI (:86). The commands here are the player's AI orders on the units they name.
            if msg.source != crate::ai_states::CommandSource::Ai {
                let units: Vec<ObjectId> = match &msg.command {
                    Command::MoveTo { ids, .. } | Command::ForceMove { ids, .. } | Command::Enter { ids, .. } | Command::Exit { ids, .. } => ids.clone(),
                    Command::Dock { ids, .. } | Command::Guard { ids, .. } => ids.clone(),
                    Command::Evacuate { container } => vec![*container],
                    _ => Vec::new(),
                };
                for id in units {
                    self.assault_command(id, crate::assault_transport::AssaultCommand::Other, msg.source);
                }
            }
            match msg.command {
                // `MSG_DO_MOVETO` and `MSG_DO_FORCEMOVETO` ({@link crate::player_orders}) share this
                // body, as `onDoMoveto` and `onDoForcemoveto` both call `groupMoveToPosition`.
                Command::MoveTo { ids, pos } | Command::ForceMove { ids, pos } => {
                    self.release_group_temporary_locks(msg.player, msg.source, &ids);
                    self.command_move_to_from(msg.player, &ids, pos, msg.source, true);
                }
                Command::AddWaypoint { ids, pos } => {
                    self.release_group_temporary_locks(msg.player, msg.source, &ids);
                    self.command_add_waypoint(msg.player, &ids, pos)
                }
                Command::Scatter { ids } => self.command_scatter(msg.player, &ids),
                Command::ToggleOvercharge { ids } => self.command_toggle_overcharge(msg.player, &ids),
                Command::SwitchWeapons { ids, slot } => self.command_switch_weapons(msg.player, &ids, slot),
                Command::CancelUpgrade { building, upgrade } => self.command_cancel_upgrade(msg.player, building, &upgrade),
                Command::CombatDrop { ids, target } => self.command_combat_drop(msg.player, &ids, target),
                // `onDoAttackObject` / `onDoForceAttackObject` release only when the enemy is still there
                // (`GameLogicDispatch.cpp:1703-1709,1720-1726`).
                Command::Attack { ids, target } => {
                    if self.objects.get(&target).is_some_and(|o| !o.destroyed) {
                        self.release_group_temporary_locks(msg.player, msg.source, &ids);
                    }
                    self.command_attack_from(msg.player, &ids, AttackTarget::Object(target), msg.source)
                }
                Command::AttackGround { ids, pos } => self.command_force_attack_ground(msg.player, msg.source, &ids, pos),
                Command::ForceAttack { ids, target } => {
                    if self.objects.get(&target).is_some_and(|o| !o.destroyed) {
                        self.release_group_temporary_locks(msg.player, msg.source, &ids);
                    }
                    self.command_force_attack_from(msg.player, &ids, AttackTarget::Object(target), msg.source)
                }
                Command::AttackMove { ids, pos } => {
                    self.release_group_temporary_locks(msg.player, msg.source, &ids);
                    self.command_attack_move(msg.player, &ids, pos, msg.source)
                }
                Command::Guard { ids, target, mode } => self.command_guard(msg.player, &ids, target, mode),
                Command::Sell { building } => self.command_sell(msg.player, building),
                Command::Stop { ids } => {
                    for id in ids {
                        let Some(obj) = self.objects.get(&id) else { continue };
                        if obj.destroyed || obj.player != msg.player || obj.ai_handle.is_none() || obj.ai.suspended {
                            continue;
                        }
                        // `AIGroup::groupIdle(CMD_FROM_PLAYER)` (`AIGroup.cpp:2093-2127`): `aiIdle` first, then the
                        // stealth mood-check delay (the idle entry has already reset the timer).
                        self.ai_idle(id, crate::ai_states::CommandSource::Player);
                        self.group_stealth_mood_delay(id);
                    }
                }
                // `onEnter` returns before the release when the target is gone (`GameLogicDispatch.cpp:1475-1481`).
                Command::Enter { ids, container } => {
                    if self.objects.get(&container).is_some_and(|o| !o.destroyed) {
                        self.release_group_temporary_locks(msg.player, msg.source, &ids);
                    }
                    self.command_enter(msg.player, &ids, container)
                }
                Command::Exit { container, ids } => self.command_exit(msg.player, container, &ids),
                // `onEvacuate` releases the selected group, which for the port's container order is the container
                // itself (`GameLogicDispatch.cpp:1535`).
                Command::Evacuate { container } => {
                    self.release_group_temporary_locks(msg.player, msg.source, &[container]);
                    self.command_evacuate(msg.player, container, msg.source)
                }
                Command::QueueUnit { factory, template } => {
                    self.queue_unit(msg.player, factory, &template);
                }
                Command::QueueUpgrade { building, upgrade } => {
                    self.command_queue_upgrade(msg.player, building, &upgrade);
                }
                Command::CancelUnit { factory, index } => {
                    self.cancel_unit(msg.player, factory, index);
                }
                Command::SetRallyPoint { factory, pos } => {
                    self.set_rally_point(msg.player, factory, pos);
                }
                Command::Construct { dozer, template, pos, angle } => {
                    // Only the dozer's owner may order it.
                    if self.objects.get(&dozer).is_some_and(|o| o.player == msg.player) {
                        self.construct(dozer, &template, pos, angle);
                    }
                }
                Command::CancelConstruct { dozer } => {
                    if self.objects.get(&dozer).is_some_and(|o| o.player == msg.player) {
                        self.cancel_construct(dozer);
                    }
                }
                Command::Repair { dozer, target } => {
                    // `ActionManager::canRepairObject` gates the repair command (retail rejects
                    // bridges and non-structures); the low-level `command_repair` still carries the
                    // bridge mechanics for scripts/tests.
                    if self.objects.get(&dozer).is_some_and(|o| o.player == msg.player) && self.can_repair_object(dozer, target) {
                        self.command_repair(msg.player, dozer, target);
                    }
                }
                Command::ResumeConstruction { dozer, target } => {
                    self.command_resume_construction(msg.player, dozer, target);
                }
                Command::Dock { ids, target } => self.command_dock(msg.player, &ids, target),
                Command::GetRepaired { ids, target } => self.command_get_repaired(msg.player, &ids, target),
                Command::GetHealed { ids, target } => self.command_get_healed(msg.player, &ids, target),
                // Possession protocol (docs/POSSESSION.md, crate::possession).
                #[cfg(feature = "possession")]
                Command::Possess { id } => self.command_possess(msg.player, id),
                #[cfg(feature = "possession")]
                Command::PossessInput(input) => self.command_possess_input(msg.player, input),
                #[cfg(feature = "possession")]
                Command::Release => self.command_release(msg.player),
                Command::DoSpecialPower { source, power, target } => {
                    self.command_do_special_power(msg.player, source, &power, target);
                }
                Command::PurchaseScience { science } => {
                    self.command_purchase_science(msg.player, &science);
                }
                Command::DoWeapon { ids, slot, max_shots } => self.command_do_weapon(msg.player, &ids, slot, max_shots),
                Command::DoWeaponAtObject { ids, slot, target, max_shots } => {
                    self.command_do_weapon_at_object(msg.player, &ids, slot, target, max_shots)
                }
                Command::DoWeaponAtLocation { ids, slot, pos, max_shots } => {
                    self.command_do_weapon_at_location(msg.player, &ids, slot, pos, max_shots)
                }
                Command::HackInternet { ids } => {
                    self.release_group_temporary_locks(msg.player, msg.source, &ids);
                    self.command_hack_internet(msg.player, &ids)
                }
                Command::SetMineClearingDetail { ids } => self.command_set_mine_clearing_detail(msg.player, &ids),
                // `MSG_ENABLE_RETALIATION_MODE` (`GameLogicDispatch.cpp:1082`).
                Command::EnableRetaliationMode { enabled } => self.command_enable_retaliation_mode(msg.player, enabled),
                Command::DoSpecialPowerAtLocation { source, power, pos, angle } => {
                    self.command_do_special_power_at_location(msg.player, source, &power, pos, angle);
                }
            }
        }
    }

    // ---- the frame -------------------------------------------------------------------------

    /// `TheScriptEngine->UPDATE()`: counters then each side's scripts, in side order. The engine
    /// is taken out of `self` so its `update` can borrow `GameLogic` mutably (the original engine
    /// is a separate singleton).
    ///
    /// **Rule for every path reachable from a script action or condition:** the engine is a
    /// *singleton* in the original (`TheScriptEngine`), so code that an action reaches must use the
    /// `&ScriptEngine` handed down to it (lane 144's `ai_build_base_structure`/`_defense`, lane 151's
    /// `ai_compute_superweapon_target`, `script_do_special_power*` and `create_object_in_team_with`),
    /// never `self.scripts`: while `engine.update` runs, `self.scripts` is an empty engine, so a read
    /// sees no teams/waypoints/special-power events and a write is discarded when the field is
    /// restored below. Grep for `self.scripts.` / `logic.scripts.` before adding a call from an
    /// action, condition, or anything one of those calls.
    fn script_engine_update(&mut self) {
        if self.scripts.lists.is_empty() {
            return;
        }
        let mut engine = std::mem::take(&mut self.scripts);
        engine.update(self);
        self.scripts = engine;
        // Objects the scripts created joined a team while the engine was out (`Object::Object`'s
        // `setTeam`); their member-list insert happens now, still before the rest of the frame.
        self.apply_pending_team_joins();
    }

    /// The script engine (`TheScriptEngine`), read-only.
    pub fn scripts(&self) -> &crate::script::ScriptEngine {
        &self.scripts
    }

    /// The script engine, mutable (for setup by the adapter).
    pub fn scripts_mut(&mut self) -> &mut crate::script::ScriptEngine {
        &mut self.scripts
    }

    /// The first-evaluation draws `ScriptEngine::newMap` makes once the script lists are loaded
    /// (`checkConditionsForTeamNames`). Call after [`ScriptEngine::load`](crate::script::ScriptEngine::load).
    pub fn draw_script_first_evaluation(&mut self) {
        self.scripts.draw_first_evaluation_frames(&mut self.rng);
    }

    /// Registers the players that can win/lose (`VictoryConditions`).
    pub fn set_victory_players(&mut self, players: impl IntoIterator<Item = PlayerIndex>) {
        self.victory_players = players.into_iter().collect();
    }

    /// `Player::hasAnyBuildings(KINDOF_MP_COUNT_FOR_VICTORY)`: a player owns a live structure whose
    /// template also has `KINDOF_MP_COUNT_FOR_VICTORY` (the mask `VictoryConditions.cpp:330`
    /// builds). Effectively dead / destroyed objects are skipped (`Team::hasAnyBuildings`).
    fn has_any_victory_building(&self, player: PlayerIndex) -> bool {
        const VICTORY_MASK: u64 =
            crate::template::kind_of::STRUCTURE | crate::template::kind_of::MP_COUNT_FOR_VICTORY;
        self.objects()
            .any(|o| o.player == player && !o.is_destroyed() && !o.is_dying() && o.is_kind_of(VICTORY_MASK))
    }

    /// `VictoryConditions::hasSinglePlayerBeenDefeated` for the skirmish `VICTORY_NOBUILDINGS` rule
    /// (`GameLogic.cpp:1702` sets `VICTORY_NOBUILDINGS`): a player is out once they have no live
    /// `MP_COUNT_FOR_VICTORY` buildings, or once a script declared them defeated (`DEFEAT`).
    pub fn player_alive(&self, player: PlayerIndex) -> bool {
        !self.scripted_defeated.contains(&player) && self.has_any_victory_building(player)
    }

    /// `Player::isPlayerActive` (`Player.cpp:3994`): `!m_observer && !m_isPlayerDead`. The sim does
    /// not model observers, so this is just the dead flag set by [`GameLogic::kill_player`]. The
    /// neutral player is *not* dead (the engine's `Player::init` leaves `m_isPlayerDead` false for
    /// it), so callers that exclude neutrals do so separately (`RebuildHoleExposeDie.cpp:127`).
    pub fn is_player_active(&self, player: PlayerIndex) -> bool {
        !self.player_dead.contains(&player)
    }

    /// `Object::setTeam` (`Object.cpp:851`): "In order to prevent spawning useful units for a
    /// player after he dies, we just assign objects to the neutral player if we try to misbehave."
    /// The object joins the target player's default team (`setOrRestoreTeam`, `Object.cpp:864-948`);
    /// `Player::getControllingPlayer` is the team's (`Object.cpp:1620`).
    pub fn set_object_team(&mut self, id: ObjectId, player: PlayerIndex) {
        let target = if self.is_player_active(player) { player } else { self.neutral_player().unwrap_or(player) };
        // Keep the player even when no default team exists (synthetic games without a sides list).
        if let Some(o) = self.objects.get_mut(&id) {
            o.player = target;
        }
        if let Some(key) = self.player_default_team(target).map(str::to_owned) {
            self.set_object_team_key(id, &key);
        }
        // `Player::becomingTeamMember` (`Player.cpp:1036`) re-arms a joined object's deposit timer
        // (`AutoDepositUpdate::awardInitialCaptureBonus`, AutoDepositUpdate.cpp:117-124).
        self.auto_deposit_on_team_member(id);
    }

    /// `Object::setTemporaryTeam` (`Object.cpp:864`).
    pub fn set_object_temporary_team(&mut self, id: ObjectId, player: PlayerIndex) {
        self.set_object_team(id, player);
    }

    /// `Object::setTeam` for an explicit team instance key (`setOrRestoreTeam`, `Object.cpp:872`):
    /// membership lists, controlling player, team attitude and `onCapture`. The script engine owns
    /// the teams, so it is taken out while the switch runs (script paths call
    /// [`ScriptEngine::set_object_team`](crate::script::ScriptEngine) with the already-taken engine).
    pub fn set_object_team_key(&mut self, id: ObjectId, key: &str) {
        let mut engine = std::mem::take(&mut self.scripts);
        engine.set_object_team(self, id, key);
        self.scripts = engine;
    }

    /// `PlayerList::newGame` -> `TeamFactory::initFromSides` + `Player::setDefaultTeam`: create the
    /// singleton team instances, activate each player's default team and record it on the player
    /// (`Player::m_defaultTeam`). `defaults` is `(team name, player index)` in player order.
    pub fn init_from_sides(&mut self, defaults: &[(&str, PlayerIndex)]) {
        let mut engine = std::mem::take(&mut self.scripts);
        engine.init_from_sides(defaults);
        for (name, player) in defaults {
            if let Some(p) = self.players.get_mut(player) {
                p.default_team = Some(name.to_ascii_lowercase());
            }
        }
        self.scripts = engine;
    }

    /// The player's default team key (`Player::getDefaultTeam` -> `Team::getName`).
    pub fn player_default_team(&self, player: PlayerIndex) -> Option<&str> {
        self.players.get(&player).and_then(|p| p.default_team.as_deref())
    }

    /// `Team::setActive` (`Team.h:309`) for the team instance `key`: marks it created and runs its
    /// `teamOnCreateScript` (`Team::updateState`, `Team.cpp:1812-1819`). `GameLogic.cpp:1957`
    /// activates the team of every map object it creates.
    pub fn activate_team(&mut self, key: &str) {
        let key = key.to_ascii_lowercase();
        let mut engine = std::mem::take(&mut self.scripts);
        engine.mark_team_active(&key);
        self.scripts = engine;
    }

    /// `Object::Object`'s initial `setTeam` (`Object.cpp:312`): the explicit team, else the owner's
    /// default team, else the neutral player's default team. A script action creates objects while
    /// the script engine is taken out of this struct; the engine the action runs on is passed in as
    /// `scripts` and the object joins its team at once, exactly as `Object::Object` -> `setTeam` ->
    /// `prependTo_TeamMemberList` (`Object.cpp:898`) does, so the rest of the same action sees it
    /// (`ScriptActions.cpp:607`, `:662`, `:730`). Only when no engine is handed in (a creation from
    /// some other taken-engine window, e.g. `ai_recruit_into`) does the member-list insert wait for
    /// [`Self::apply_pending_team_joins`].
    fn assign_initial_team(&mut self, id: ObjectId, team: Option<&str>, scripts: Option<&mut crate::script::ScriptEngine>) {
        let Some(player) = self.objects.get(&id).map(|o| o.player) else { return };
        let key = match team {
            Some(k) => Some(k.to_ascii_lowercase()),
            None => self.player_default_team(player).map(str::to_owned).or_else(|| {
                self.neutral_player().and_then(|n| self.player_default_team(n).map(str::to_owned))
            }),
        };
        let Some(key) = key else { return };
        if let Some(engine) = scripts {
            // The script action's engine: `Object::setTeam` runs now, before the modules' first update.
            if engine.teams.contains_key(&key) {
                engine.set_object_team(self, id, &key);
                self.auto_deposit_on_team_member(id);
            } else {
                if let Some(o) = self.objects.get_mut(&id) {
                    o.team = Some(key.clone());
                }
                self.pending_team_joins.push((id, key));
            }
        } else if self.scripts.teams.contains_key(&key) {
            // The engine is present (an ordinary creation): join now.
            self.set_object_team_key(id, &key);
        } else {
            // The script engine is taken out of this struct while its actions run (`std::mem::take`):
            // the object carries the team from its constructor, and the member-list insert happens in
            // [`Self::apply_pending_team_joins`] right after the engine returns.
            if let Some(o) = self.objects.get_mut(&id) {
                o.team = Some(key.clone());
            }
            self.pending_team_joins.push((id, key));
        }
    }

    /// Fills in the member lists of objects made while the script engine was taken out (see
    /// [`Self::assign_initial_team`]). Runs right after `ScriptEngine::update`, so nothing observes a
    /// half-created team list.
    fn apply_pending_team_joins(&mut self) {
        if self.pending_team_joins.is_empty() {
            return;
        }
        let joins = std::mem::take(&mut self.pending_team_joins);
        for (id, key) in joins {
            if self.objects.get(&id).is_some_and(|o| o.team.as_deref() == Some(key.as_str())) {
                let mut engine = std::mem::take(&mut self.scripts);
                engine.register_object_in_team(self, id, &key);
                self.scripts = engine;
            }
        }
    }

    /// `VictoryConditions::update`'s elimination loop (the multiplayer branch): latch each newly
    /// defeated player and `killPlayer` their remaining objects. The winner/defeat query itself is
    /// [`check_for_game_end`](Self::check_for_game_end).
    fn update_victory_conditions(&mut self) {
        if self.victory_players.is_empty() {
            return;
        }
        let newly: Vec<PlayerIndex> = self
            .victory_players
            .iter()
            .copied()
            .filter(|p| !self.victory_defeated.contains(p) && !self.player_alive(*p))
            .collect();
        for p in newly {
            self.victory_defeated.insert(p);
            // `VictoryConditions.cpp:205-208`: after frame 1, reveal the whole map for the defeated
            // player permanently (observer mode) before killing their units.
            if self.frame > 1 {
                self.shroud.reveal_map_permanently(p);
            }
            self.kill_player(p);
        }
    }

    /// `Player::killPlayer` -> `Team::evacuateTeam` / `Team::killTeam`: dump every container's
    /// passengers, then kill every remaining object (`Object::kill()` = max-health unresistable
    /// damage, `Object.cpp:2010`). Used when a player is eliminated.
    fn kill_player(&mut self, player: PlayerIndex) {
        // `Player::killPlayer` sets `m_isPlayerDead = TRUE` before killing the teams, so OCLs never
        // again spawn useful units for the player (`Player.cpp:2032`).
        self.player_dead.insert(player);
        let ids: Vec<ObjectId> = self
            .objects()
            .filter(|o| o.player == player && !o.is_destroyed() && !o.is_dying())
            .map(|o| o.id)
            .collect();
        // `Team::evacuateTeam`: containers dump their contents first.
        for &id in &ids {
            if self.objects.get(&id).is_some_and(|o| !o.contained.is_empty() || self.is_tunnel_container(id)) {
                self.on_container_destroyed(id);
            }
        }
        // `Team::killTeam`: kill everything that is left (`Object::kill()`).
        for id in ids {
            self.damage_object_with_death(
                id,
                crate::body::DamageInfo {
                    source: ObjectId::INVALID,
                    amount: crate::damage::HUGE_DAMAGE_AMOUNT,
                    kind: crate::damage::DamageType::Unresistable,
                },
                crate::damage::DeathType::Normal,
            );
        }
        // `Player::killPlayer` (`Player.cpp:2089`): force $$$ to 0 on death.
        if let Some(p) = self.players.get_mut(&player) {
            p.money = 0;
        }
    }

    /// `VictoryConditions::checkForGameEnd`: the last standing victory player wins once every
    /// other victory player has lost all buildings (`VICTORY_NOBUILDINGS`). A script's `VICTORY`
    /// action declares the winner directly (`ScriptActions::doVictory`).
    pub fn check_for_game_end(&self) -> GameEnd {
        let alive: Vec<PlayerIndex> = self.victory_players.iter().copied().filter(|p| self.player_alive(*p)).collect();
        let defeated: Vec<PlayerIndex> = self.victory_players.iter().copied().filter(|p| !self.player_alive(*p)).collect();
        let winner = self
            .scripted_winner
            .or_else(|| if alive.len() == 1 && self.victory_players.len() > 1 { Some(alive[0]) } else { None });
        GameEnd { winner, defeated }
    }

    /// `ScriptActions::doVictory`: declare `player` the winner.
    pub(crate) fn script_victory(&mut self, player: PlayerIndex) {
        self.scripted_winner = Some(player);
    }

    /// `ScriptActions::doDefeat` / `doLocalDefeat`: mark `player` a scripted loser.
    pub(crate) fn script_defeat(&mut self, player: PlayerIndex) {
        self.scripted_defeated.insert(player);
    }

    /// Advance exactly one logic frame (`GameLogic::update`).
    pub fn update(&mut self) {
        let now = self.frame;
        self.pathfinder.set_frame(now); // the zone schedule of `processPathfindQueue` (AIPathfind.cpp:6010)
        self.in_update = true;

        // Client FX events are per-frame: the viewer drains them after each step.
        self.fx_events.clear();

        // `Pathfinder::setIgnoreObstacleID`-era state: the pathfinder's ally table mirrors the
        // relationship matrix so `checkForMovement`'s ally/enemy tests see the same relationships.
        self.sync_pathfinder_allies();

        self.script_engine_update();
        // TheTerrainLogic->UPDATE(): stub.

        self.process_command_list();
        // `Player::onPowerBrownOutChange` (Player.cpp:3281): `KINDOF_POWERED` objects get
        // `DISABLED_UNDERPOWERED` while their player is in brownout.
        self.update_brownout_disabled();
        #[cfg(feature = "possession")]
        self.update_possession();
        // The AI modules (`PHASE_INITIAL`) run in sleepy-heap order; the other objects that carry a
        // state machine without an AI module keep the id walk after them. A supply truck's machine is
        // part of its `SupplyTruckAIUpdate`/`ChinookAIUpdate` module, so it runs in its AI slot
        // (`ai_update_one`), as `SupplyTruckAIUpdate::update` does (`SupplyTruckAIUpdate.cpp:97-107`).
        // `HackInternetAIUpdate::update`: the hack state machine (a hacking unit's AI is paused below).
        self.update_hack_internet();
        self.dump_sleepy_heap(now);
        self.run_ai_heap_pass(now);
        // `PhysicsBehavior::update` is `PHASE_PHYSICS` (PhysicsUpdate.h:220): after the phase-0 AI and the
        // physics-phase heap entries of this frame (trains), before the normal-phase modules below.
        self.update_ai_state_machines();
        self.update_behaviors(); // AutoHeal / Flammable / Poisoned / Lifetime / Stealth
        self.update_modules2(); // Crush / income / fire / horde / topple / slaves / crates
        self.update_modules3(); // Aura heal / overcharge / prone / mines
        // `ParkingPlaceBehavior::update` runs in its own sleepy-heap slot (`sleepy_families.rs`).
        self.update_modules4(); // Rebuild holes / countermeasures / traps / cleanup / minefields
        self.update_rappels(); // AIRappelState (the Chinook's own state machine runs in its AI slot)
        // Refresh the pathfinder's occupancy before servicing the queue, so a unit that just got a
        // move order is already "moving" and `moveAllies` does not push it aside.
        self.sync_pathfinder_units();
        // `AI::update` (`AI.cpp:356`): service the pathfind queue, then the AIPlayers.
        self.process_pathfind_queue();
        self.update_ai_players(); // TheAI->UPDATE() (AIPlayer::update)
        self.sync_pathfinder_units();
        // ProjectileStreamUpdate::update runs before the projectiles detonate: a projectile that detonates
        // this frame is still found by findObjectByID (it is only queued), and is culled next frame.
        self.update_projectile_streams();
        self.update_projectiles();
        self.update_lasers(); // LifetimeUpdate / DeletionUpdate of the laser objects
        self.update_deliveries(); // DeliverPayloadAIUpdate

        // TheAI->UPDATE(), TheBuildAssistant->UPDATE(): stubs.

        self.update_partition();

        self.process_destroy_list();
        self.update_delayed_damage(); // TheWeaponStore->UPDATE()
        self.command_list.clear(); // TheCommandList->reset()
        // `TheVictoryConditions->UPDATE()` (GameLogic.cpp:3909): eliminate and `killPlayer` losers.
        self.update_victory_conditions();
        // End-of-frame clean-up: expire disabled types (`Object::checkDisabledStatus`,
        // GameLogic.cpp:3917).
        self.check_disabled_status();

        // ThePartitionManager->update: reconcile every object's shroud look and its value cells.
        self.update_shroud();
        self.update_value_map();

        // `ModelConditionFlags` for the client: derive once per frame from the settled state.
        self.update_model_conditions();
        // Objects created while a script engine was taken out (a script action fired during the AI's
        // team-building phase) join their team's member list before the frame ends.
        self.apply_pending_team_joins();
        // Motive forces from the locomotor kernels wake their physics for the next frame (see `flush_physics_wake_requests`).
        self.flush_physics_wake_requests();
        // The end-of-frame trigger-area pass: a move that no area scan has read yet is stamped this frame.
        self.trigger_membership_pass();
        self.in_update = false;
        self.frame += 1;
        // RNG trace frame (no-op unless ZR_RNG_TRACE is set): the frame counter the next draws see.
        self.rng.set_trace_frame(self.frame);
    }

    /// `ThePartitionManager->update` shroud half: for every object that can look, compute the
    /// players it reveals to and the clearing range, then reconcile the grids.
    fn update_shroud(&mut self) {
        let now = self.frame;
        let players: Vec<PlayerIndex> = self.players.keys().copied().collect();
        // `SpyVisionUpdate`: players whose active spy vision makes them look through the eyes of
        // this player's matching enemy units (`Object::m_visionSpiedMask`).
        let spies = self.spy_vision_sources();
        let mut desired: BTreeMap<ObjectId, Vec<Sighting>> = BTreeMap::new();
        for obj in self.objects.values() {
            if obj.is_destroyed() || obj.is_dying() || obj.contained_by.is_some() {
                continue;
            }
            let mut looks: Vec<Sighting> = Vec::new();
            let range = shroud_clearing_range(obj);
            if range > 0.0 {
                let mut mask = if obj.is_kind_of(kind_of::REVEAL_TO_ALL) {
                    PLAYERMASK_ALL
                } else {
                    Shroud::ally_mask(obj.player, &players, |p, owner| self.relationship(p, owner) == Relationship::Allies)
                };
                // `Object::look`'s `lookingMask |= m_visionSpiedMask`: a spied unit reveals its
                // vision to the spying (enemy) player too (`isAnyKindOf`).
                for (owner, kind) in &spies {
                    if *kind != 0
                        && obj.template.kind_of & kind != 0
                        && u32::from(*owner) < 32
                        && self.relationship(obj.player, *owner) == Relationship::Enemies
                    {
                        mask |= 1u32 << *owner;
                    }
                }
                if mask != 0 {
                    looks.push(Sighting { pos: obj.position, range, mask });
                }
            }
            // `ThingTemplate::getShroudRevealToAllRange`: a second, usually smaller, circle revealed
            // to enemies and neutrals (but not while under construction).
            let reveal_all = obj.template.shroud_reveal_to_all_range;
            if reveal_all > 0.0 && !obj.under_construction {
                let mut mask = 0u32;
                for &p in &players {
                    if p != obj.player && u32::from(p) < 32 && self.relationship(p, obj.player) != Relationship::Allies {
                        mask |= 1u32 << p;
                    }
                }
                if mask != 0 {
                    looks.push(Sighting { pos: obj.position, range: reveal_all, mask });
                }
            }
            if !looks.is_empty() {
                desired.insert(obj.id, looks);
            }
        }
        self.shroud.update(now, &desired);
    }

    /// `Object::handlePartitionCellMaintenance`'s value half (`Object.cpp:4902-4906`, which is
    /// `removeValue(); addValue();`) for every object, once per frame at the end of the frame — the point
    /// where the engine's `ThePartitionManager->UPDATE()` (`GameLogic.cpp:3894`) runs the cell-change
    /// callbacks. The next frame's scripts therefore read the map as it stood at the end of this frame,
    /// which is what the engine's `ScriptEngine::UPDATE()` at the top of the frame sees.
    ///
    /// The engine only re-runs the pair at a fixed set of state changes; `addValue` depends on nothing but
    /// the object's cell, vision range, build cost, owner and the under-construction / effectively-dead /
    /// shroud-range tests, so re-deriving every frame produces the same cells. See
    /// [`crate::partition_value`].
    fn update_value_map(&mut self) {
        // Objects that left the world: `~Object` drops `m_partitionLastValue` (`Object.cpp:645-646`)
        // without touching the cells, so a destroyed-but-not-dead object's value stays (the engine leaks
        // it). `process_destroy_list` already applied that rule; anything left here is stale.
        let gone: Vec<ObjectId> = self.value_affects.keys().copied().filter(|id| !self.objects.contains_key(id)).collect();
        for id in gone {
            if let Some(a) = self.value_affects.remove(&id) {
                self.value_map.apply(&a, false);
            }
        }
        let desired: Vec<(ObjectId, Option<crate::partition_value::ValueAffect>)> =
            self.objects.values().map(|o| (o.id, crate::partition_value::value_affect_for(o))).collect();
        for (id, want) in desired {
            let have = self.value_affects.get(&id).copied();
            // `Object::onPartitionCellChange` (`Object.cpp:4876`) only re-runs the pair when the object's
            // touched cells change; `same_affect` compares the cell, not the sub-cell position.
            let unchanged = match (have, want) {
                (None, None) => true,
                (Some(a), Some(b)) => self.value_map.same_affect(&a, &b),
                _ => false,
            };
            if unchanged {
                continue;
            }
            if let Some(a) = have {
                self.value_map.apply(&a, false);
            }
            match want {
                Some(a) => {
                    self.value_map.apply(&a, true);
                    self.value_affects.insert(id, a);
                }
                None => {
                    self.value_affects.remove(&id);
                }
            }
        }
    }

    /// `PlayerList::getPlayersWithRelationship` (`PlayerList.cpp:394`) with `ALLOW_ENEMIES`
    /// (`Common/GameCommon.h`): every registered player other than `player` whose relationship to it is
    /// `ENEMIES`, as a bit mask of `1 << playerIndex` (`Player::getPlayerMask`, `Player.h:260`).
    pub fn enemy_player_mask(&self, player: PlayerIndex) -> u32 {
        let mut mask = 0u32;
        for &p in self.players.keys() {
            if p == player || u32::from(p) >= 32 {
                continue;
            }
            if self.relationship(player, p) == Relationship::Enemies {
                mask |= 1u32 << u32::from(p);
            }
        }
        mask
    }

    /// `PartitionManager::getNearestGroupWithValue(playerIndex, ALLOW_ENEMIES, VOT_CashValue, source,
    /// valueRequired, greaterThan, out)` (`PartitionManager.cpp:4880`): the world position of the nearest
    /// partition cell whose summed enemy build cost passes `value_required`, searched breadth-first from
    /// `source`. `None` when no cell passes (the engine leaves its out parameter untouched then).
    pub fn nearest_group_with_value(&self, player: PlayerIndex, value_required: i32, source: &Coord3D) -> Option<Coord3D> {
        let mask = self.enemy_player_mask(player);
        self.value_map.nearest_group_with_value(mask, value_required, source)
    }

    /// The value cells (read-only), for the client and tests.
    pub fn value_map(&self) -> &crate::partition_value::ValueMap {
        &self.value_map
    }

    /// Recompute every live object's [`ModelConditionFlags`](crate::condition::ModelConditionFlags).
    fn update_model_conditions(&mut self) {
        let now = self.frame;
        for obj in self.objects.values_mut() {
            obj.model_conditions = crate::condition::derive(obj, now);
            // Upgrade conditions, the rider table's conditions and a tech building's `CAPTURED` are sticky
            // (`setModelConditionState`).
            let sticky = obj.upgrade_conditions | obj.rider.conditions;
            let mut flags = crate::condition::ModelConditionFlags::from_bits(obj.model_conditions.bits() | sticky);
            if obj.behavior.tech_captured {
                // `TechBuildingBehavior::update`'s `setModelConditionState(MODELCONDITION_CAPTURED)`.
                if let Some(bit) = crate::condition::bit_for_name("CAPTURED") {
                    flags.set(bit);
                }
            }
            obj.model_conditions = flags;
        }
    }

    /// `ZR_L99_HEAP=<frames>` (diagnostic, read-only): the sleepy heap in array order, one stderr line per    /// entry, in the format of the patched C++ `ZR_L99_HEAP` print (`GameLogic::update`, before the sleepy
    /// loop). `mod` is the C++ class name: the AI modules are `AIUpdateInterface`, `DozerAIUpdate` or
    /// `WorkerAIUpdate` by template.
    fn dump_sleepy_heap(&self, now: u32) {
        static FRAMES: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
        let frames = FRAMES.get_or_init(|| {
            std::env::var("ZR_L99_HEAP")
                .ok()
                .map(|v| v.split(',').filter_map(|f| f.trim().parse().ok()).collect())
                .unwrap_or_default()
        });
        if !frames.contains(&now) {
            return;
        }
        for (i, &h) in self.schedule.heap_order().iter().enumerate() {
            let s = self.schedule.slot(h);
            let (module, tmpl) = match self.objects.get(&s.object) {
                Some(o) => (self.slot_class_name(h), o.template.name.as_str()),
                None => ("?", "?"),
            };
            eprintln!(
                "ZR_L99_HEAP f={now} i={i} id={} key={} mod={module} tmpl={tmpl}",
                s.object.0,
                s.next_call_and_phase
            );
        }
    }

    /// The C++ `UpdateModule` class name of a heap slot's owner module (`ZR_L99_HEAP`'s `mod` field): the AI modules
    /// by template, the families by class, the helpers by name.
    fn slot_class_name(&self, h: u32) -> &'static str {
        let s = self.schedule.slot(h);
        if let Some(kind) = s.family {
            return kind.class_name();
        }
        if s.ai {
            if self.is_worker(s.object) {
                return "WorkerAIUpdate";
            }
            return match self.objects.get(&s.object) {
                Some(o) if o.template.dozer.is_some() => "DozerAIUpdate",
                _ => "AIUpdateInterface",
            };
        }
        s.module.as_ref().map_or("?", |m| m.name())
    }

    /// The `while (!m_sleepyUpdates.empty())` loop of `GameLogic::update` (GameLogic.cpp:3851-3905): every due
    /// sleepy module in heap order, in one loop. An AI module (`ai`, `PHASE_INITIAL`) runs its object's AI update
    /// through `ai_call` (the AI pass, `ai_states.rs`); every other module runs its `UpdateModule::update` here.
    /// The module that runs is the heap's top, so a wake it gives itself is ignored (`GameLogic.cpp:3094`).
    pub(crate) fn run_sleepy_updates_with(&mut self, now: u32, mut ai_call: impl FnMut(&mut Self, ObjectId) -> u32) {
        let mut requests: Vec<Request> = Vec::new();
        while let Some(h) = self.schedule.peek() {
            if self.schedule.slot(h).next_call_and_phase >> 2 > now {
                break; // everyone else is sleeping
            }
            self.schedule.set_running(Some(h));
            if self.schedule.slot(h).ai {
                let id = self.schedule.slot(h).object;
                // `GameLogic::update`'s disabled-types gate (GameLogic.cpp:3849-3885): `AIUpdateInterface`'s
                // `getDisabledTypesToProcess` is `MAKE_DISABLED_MASK(DISABLED_HELD)` (AIUpdate.h:305), so a unit
                // disabled by anything else (EMP, PARALYZED, SUBDUED, HACKED, UNMANNED, UNDERPOWERED, ...) has its
                // whole AI module deferred to `now + 1` — it neither runs its machine nor `doLocomotor`. Only a
                // contained (HELD) unit keeps running its AI.
                let deferred = self.disabled_gate_defers(h);
                let sleep = if deferred {
                    // "else defer it till next frame and re-push it" (`sleepLen` stays `UPDATE_SLEEP_NONE`).
                    UPDATE_SLEEP_NONE
                } else {
                    // `ZR_L99_POP=<frames>` (diagnostic): the AI pops of those frames, in pop order. The C++ print
                    // sits inside the gate, so a deferred module does not appear.
                    static POP_FRAMES: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
                    let pop_frames = POP_FRAMES.get_or_init(|| {
                        std::env::var("ZR_L99_POP")
                            .ok()
                            .map(|v| v.split(',').filter_map(|f| f.trim().parse().ok()).collect())
                            .unwrap_or_default()
                    });
                    if pop_frames.contains(&now) {
                        eprintln!("ZR_L99_POP f={now} id={} key={}", id.0, self.schedule.slot(h).next_call_and_phase);
                    }
                    let sleep = ai_call(self, id).max(1);
                    // The locomotor's motive forces from this AI update wake its physics now (`applyForce`, PhysicsUpdate.cpp:401-402),
                    // inside the update, before the AI module's own re-key (`GameLogic.cpp:3875-3876`).
                    self.wake_flagged_physics(id);
                    sleep
                };
                self.schedule.set_running(None);
                if self.schedule.is_live_at_top(h) {
                    self.schedule.set_next_call_frame(h, now.saturating_add(sleep));
                    self.schedule.rebalance(0);
                }
                continue;
            }
            // A family slot (`sleepy_families.rs`): its update needs the whole `GameLogic`, so it runs here.
            if let Some(kind) = self.schedule.slot(h).family {
                let id = self.schedule.slot(h).object;
                // `GameLogic::update`'s disabled-types gate (GameLogic.cpp:3851-3885): a gated update is deferred.
                let sleep = if self.disabled_gate_defers(h) { UPDATE_SLEEP_NONE } else { self.run_family(kind, id, now).max(1) };
                self.schedule.set_running(None);
                self.schedule.set_next_call_frame(h, now.saturating_add(sleep));
                self.schedule.rebalance(0);
                continue;
            }
            let obj_id = self.schedule.slot(h).object;
            let mut sleep = UPDATE_SLEEP_NONE; // default if the owner is gone/destroyed
            // The SubdualDamageHelper's heal runs through the body: `onSubdualChange` (ActiveBody.cpp:1285) reads
            // the subdued state before and after the update.
            let was_subdued = if self.schedule.slot(h).subdual_heal {
                self.objects.get(&obj_id).and_then(|o| o.body.as_ref()).map(|b| b.is_subdued())
            } else {
                None
            };
            let deferred = self.disabled_gate_defers(h);
            let taken = if deferred { None } else { self.schedule.slot_mut(h).module.take() };
            if let Some(mut module) = taken {
                if let Some(obj) = self.objects.get_mut(&obj_id).filter(|o| !o.destroyed && !o.is_dying() && o.contained_by.is_none()) {
                    let mut ctx = UpdateContext {
                        frame: now,
                        object: obj,
                        terrain: &self.terrain,
                        rng: &mut self.rng,
                        requests: &mut requests,
                    };
                    sleep = module.update(&mut ctx);
                    self.stats.module_updates += 1;
                    if sleep < 1 {
                        sleep = UPDATE_SLEEP_NONE;
                    }
                } else {
                    sleep = UPDATE_SLEEP_FOREVER;
                }
                self.schedule.slot_mut(h).module = Some(module);
            }
            if let Some(was) = was_subdued {
                self.on_subdual_change(obj_id, was);
            }
            self.schedule.set_running(None);
            self.schedule.set_next_call_frame(h, now.saturating_add(sleep));
            self.schedule.rebalance(0);

            for r in std::mem::take(&mut requests) {
                match r {
                    Request::Destroy(id) => self.destroy_object(id),
                    Request::Spawn { template, pos, angle, player } => {
                        self.create_object(&template, pos, angle, player);
                    }
                    Request::Wake { handle, frame } => self.schedule.awaken(handle, now, frame),
                    Request::QueuePath(id) => {
                        self.pathfinder.queue_for_path(id);
                    }
                    Request::RailroadUpdate(id) => self.railroad_update(id),
                }
            }
        }
    }

    /// Run `n` frames.
    pub fn run_frames(&mut self, n: u32) {
        for _ in 0..n {
            self.update();
        }
    }

    /// `Pathfinder::moveAllies` leaves the idle allies' move-away paths in the pathfinder
    /// (`AIPathfind.cpp:10190`); assign them so those units step aside next frame. `moveAllies` picks
    /// only allies that are not moving (`!otherObj->getAI() || otherObj->getAI()->isMoving()` -> skip,
    /// `AIPathfind.cpp:10260-10266`); the port keeps that test as a guard because the assignment is a
    /// frame later. The AI's `m_goalPosition` is *not* part of the engine's test: a move that
    /// finished keeps its goal (`AIStates.cpp:1929-1939`), so a retained `move_goal` must not veto
    /// the step-aside.
    /// `Pathfinder::moveAllies` (AIPathfind.cpp:10190-10320), applied at its call site: every ally standing on the
    /// mover's path cells gets `aiMoveAwayFromUnit(mover)` at the moment of its visit, in the engine's cell order.
    /// The scan is the pathfinder's ([`Pathfinder::ally_scan`]); the per-visit tests read the live objects, as the
    /// engine's do (`isMoving` at AIPathfind.cpp:10262, `isAttacking`/`isBusy`/ability at :10267-10275).
    /// `KINDOF_NO_COLLIDE` is tested by the callers (AIUpdate.cpp:1746, :1995, :3266).
    pub(crate) fn move_allies(&mut self, mover: ObjectId, path: &crate::pathfind::Path) {
        let Some(mover_po) = self.objects.get(&mover).and_then(|o| {
            let mut po = crate::pathfind::PathObject::for_object(o)?;
            po.ignore_obstacle = o.ai.ignore_obstacle_id;
            Some(po)
        }) else {
            return;
        };
        let visits = self.pathfinder.ally_scan(&mover_po, path);
        if visits.is_empty() {
            return;
        }
        // `LatchRestore recursiveDepth(m_moveAlliesDepth, m_moveAlliesDepth+1); if (m_moveAlliesDepth > 2) return false;`
        // (AIPathfind.cpp:10205-10208): the nesting counts this call, and a call past the third level does nothing.
        self.move_allies_depth += 1;
        if self.move_allies_depth <= 2 {
            for other in visits {
                if self.ally_visit_pushable(mover, &mover_po, path.blocked_by_ally, other) {
                    // `otherObj->getAI()->aiMoveAwayFromUnit(obj, CMD_FROM_AI)` (AIPathfind.cpp:10279).
                    self.ai_move_away_from_unit(other, mover);
                }
            }
        }
        self.move_allies_depth -= 1;
    }

    /// The per-visit tests of `Pathfinder::moveAllies` (AIPathfind.cpp:10217-10266, :10267-10275), read live: the
    /// unit is an ally, not an infantry being walked through, not a vehicle an infantry would push (unless the path is
    /// blocked by an ally), has an AI that is not moving, and is neither attacking, busy nor using an ability.
    fn ally_visit_pushable(&self, mover: ObjectId, mover_po: &crate::pathfind::PathObject, blocked_by_ally: bool, other: ObjectId) -> bool {
        if other == mover {
            return false;
        }
        let Some(o) = self.objects.get(&other) else { return false };
        let Some(other_po) = crate::pathfind::PathObject::for_object(o) else { return false };
        if !self.pathfinder.are_allies(mover_po.player, other_po.player) {
            return false; // "Only move allies."
        }
        if mover_po.infantry && other_po.infantry {
            return false; // infantry can walk through other infantry
        }
        if mover_po.infantry && !other_po.infantry && !blocked_by_ally {
            return false; // a general clear operation does not let infantry push vehicles
        }
        if o.ai_handle.is_none() || o.is_ai_moving() {
            return false; // `!otherObj->getAI() || otherObj->getAI()->isMoving()`
        }
        if self.is_ai_attacking(other) {
            return false; // "Don't move units that are attacking."
        }
        let using_ability = o.status_mask(self.frame) & crate::object::status::USING_ABILITY != 0;
        if using_ability || o.ai.machine.current == crate::ai_states::AiStateType::Busy {
            return false; // "Black Lotus exploit fix -- moving while hacking."
        }
        true
    }

    /// `TheAI->pathfinder()->updatePos` per frame: refresh every live mobile unit's occupancy
    /// cells (so a moving unit stops blocking, and an idle one does).
    fn sync_pathfinder_units(&mut self) {
        let frame = self.frame;
        let mut updates: Vec<(PathObject, Coord3D, bool)> = Vec::new();
        // Lane 188: the vehicle kind (`findAttackPath`'s strip-back) and `isIdle` (its `unitIdle` exemption) of each unit.
        let mut vehicles: Vec<u32> = Vec::new();
        let mut idle: Vec<u32> = Vec::new();
        // Lane 191: each unit's crush levels and `DISABLED_UNMANNED` (`checkDestination`'s `canCrushOrSquish`).
        let mut crush: Vec<(u32, crate::modules2::CrushInfo, bool)> = Vec::new();
        for o in self.objects.values() {
            if o.destroyed || o.is_dying() || o.contained_by.is_some() || o.template.locomotor.is_none() || o.template.kind_of & crate::template::kind_of::IMMOBILE != 0 {
                continue;
            }
            if let Some(mut po) = PathObject::for_object(o) {
                // The three predicates `moveAllies`' ZH vetoes read off the live object
                // (`AIUpdateInterface::isAttacking`/`isBusy`, `OBJECT_STATUS_IS_USING_ABILITY`,
                // `AIPathfind.cpp:10267-10275`), refreshed with the unit's occupancy.
                po.attacking = self.is_ai_attacking(o.id);
                po.busy = o.ai.machine.current == crate::ai_states::AiStateType::Busy;
                po.using_ability = o.status_mask(frame) & crate::object::status::USING_ABILITY != 0;
                if o.template.kind_of & crate::template::kind_of::VEHICLE != 0 {
                    vehicles.push(o.id.0);
                }
                if self.is_ai_idle(o.id) {
                    idle.push(o.id.0);
                }
                crush.push((o.id.0, o.template.crush, o.unmanned()));
                updates.push((po, o.position, o.is_ai_moving()));
            }
        }
        for (po, pos, moving) in updates {
            self.pathfinder.update_unit_pos(&po, &pos, moving);
        }
        self.pathfinder.set_unit_kinds(vehicles, idle);
        self.pathfinder.set_unit_crush(crush);
    }

    // ---- hashing ---------------------------------------------------------------------------

    /// `GameLogic::getCRC`: hash of all simulation state (frame, id counter, RNG, every object in
    /// id order, and the scheduler heap). Terrain is static and hashed only by [`crc_with_terrain`](Self::crc_with_terrain).
    pub fn crc(&self) -> u32 {
        let mut c = Crc::new();
        c.add_u32(self.frame);
        c.add_u32(self.next_obj_id);
        self.rng.crc(&mut c);
        c.add_u32(self.objects.len() as u32);
        for obj in self.objects.values() {
            obj.crc(&mut c);
        }
        c.add_u32(self.schedule.len() as u32);
        for (id, pri) in self.schedule.heap_iter() {
            c.add_u32(id.0);
            c.add_u32(pri);
        }
        self.combat.crc(&mut c);
        self.shroud.crc(&mut c);
        self.slow_deaths_crc(&mut c);
        // Only hashed when a tunnel network exists, so other games keep their CRC.
        if !self.tunnel_trackers.is_empty() {
            c.add_u32(self.tunnel_trackers.len() as u32);
            for (p, tr) in &self.tunnel_trackers {
                c.add_u32(u32::from(*p));
                tr.crc(&mut c);
            }
        }
        // Only hashed while a garrison has occupants, so games without garrisons keep their CRC.
        if !self.garrison_points.is_empty() {
            c.add_u32(self.garrison_points.len() as u32);
            for (id, st) in &self.garrison_points {
                c.add_u32(id.0);
                st.crc(&mut c);
            }
        }
        // Combat drops / rappels, only hashed while one is active.
        crate::chinook::crc_chinooks(&self.chinooks, &self.rappels, &mut c);
        c.add_u32(self.ai_players.len() as u32);
        for (p, ai) in &self.ai_players {
            c.add_u32(u32::from(*p));
            ai.crc(&mut c);
        }
        c.add_u32(self.players.len() as u32);
        for p in self.players.values() {
            c.add_u32(u32::from(p.index));
            c.add_u32(p.money);
        }
        // Sciences / skill points / ranks and the shared special-power cooldowns.
        c.add_u32(self.sciences.len() as u32);
        for (p, st) in &self.sciences {
            c.add_u32(u32::from(*p));
            st.crc(&mut c);
        }
        c.add_u32(self.shared_power_ready.len() as u32);
        for ((p, name), ready) in &self.shared_power_ready {
            c.add_u32(u32::from(*p));
            c.add_str(name);
            c.add_u32(*ready);
        }
        // Only hashed while someone possesses, so games without possession keep their CRC.
        #[cfg(feature = "possession")]
        if !self.possessed.is_empty() {
            c.add_u32(self.possessed.len() as u32);
            for (p, st) in &self.possessed {
                c.add_u32(u32::from(*p));
                st.crc(&mut c);
            }
        }
        self.scripts.crc(&mut c);
        // Scripted game-end override (`VICTORY` / `DEFEAT`), only when a script set one so games
        // that never take the branch keep their CRC.
        c.add_u32(self.scripted_winner.map_or(u32::MAX, u32::from));
        c.add_u32(self.scripted_defeated.len() as u32);
        for p in &self.scripted_defeated {
            c.add_u32(u32::from(*p));
        }
        c.add_u32(self.victory_defeated.len() as u32);
        for p in &self.victory_defeated {
            c.add_u32(u32::from(*p));
        }
        // `Player::m_isPlayerDead` (hashed so a replay sees the same active players).
        c.add_u32(self.player_dead.len() as u32);
        for p in &self.player_dead {
            c.add_u32(u32::from(*p));
        }
        c.add_u32(self.command_list.len() as u32);
        c.get()
    }

    /// [`crc`](Self::crc) plus the terrain samples (slower; for load-time checks).
    pub fn crc_with_terrain(&self) -> u32 {
        let mut c = Crc::new();
        c.add_u32(self.crc());
        self.terrain.crc(&mut c);
        c.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Player::isPlayerActive`: a player is active until `killPlayer` marks them dead
    /// (`Player.cpp:3994`). The neutral player is *not* dead.
    #[test]
    fn kill_player_marks_the_player_inactive() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        g.set_neutral_player(Some(9));
        assert!(g.is_player_active(0), "a fresh player is active");
        assert!(g.is_player_active(9), "the neutral player is not dead");
        // The victory path `killPlayer`s a victory player with no `MP_COUNT_FOR_VICTORY` building.
        g.victory_players.insert(0);
        g.update_victory_conditions();
        assert!(!g.is_player_active(0), "a killed player is inactive");
        assert!(g.victory_defeated.contains(&0));
    }

    /// `GameLogic::createObject` inside a frame: the object is in no partition cell until
    /// `ThePartitionManager->UPDATE()` (`PartitionManager.cpp:2913` attaches a dirty module; `:2796`
    /// places the cells; `GameLogic.cpp:3897` runs it at the end of the frame). A dozer's start-point search
    /// in the frame it is ordered (`DozerAIUpdate.cpp:1925`) does not see the building. Outside a frame
    /// (setup, before the first update) the object is visible at once.
    #[test]
    fn an_object_created_in_a_frame_is_in_no_cell_until_the_partition_update() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let t = std::sync::Arc::new(ObjectTemplate::new("Barracks"));
        let at = Coord3D::new(200.0, 100.0, 0.0);
        let found = |g: &GameLogic| g.partition().objects_in_range(&at, 5.0, crate::partition::DistanceMode::FromCenter2D);
        g.in_update = true;
        let inside = g.create_object(&t, at, 0.0, 0);
        g.in_update = false;
        assert!(found(&g).is_empty(), "created in a frame: no cell yet");
        g.update_partition();
        assert_eq!(found(&g), vec![inside], "the partition update places it");
        let outside = g.create_object(&t, Coord3D::new(300.0, 100.0, 0.0), 0.0, 0);
        assert!(g.partition().objects_in_range(&Coord3D::new(300.0, 100.0, 0.0), 5.0, crate::partition::DistanceMode::FromCenter2D).contains(&outside), "setup creation is visible at once");
    }

    /// Two overlapping objects created in one frame collide that frame: the end-of-frame partition pass places
    /// their cells and then processes the contacts (`PartitionManager::update`, PartitionManager.cpp:2810-2840,
    /// `processContactList` after the loop). The contact list finds the pair and leaves both dirty for the
    /// next frame (`makeDirty(false)` after a contact, `collision_dirty`).
    #[test]
    fn two_overlapping_objects_created_in_a_frame_collide_that_frame() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        let mut t = ObjectTemplate::new("Pebble");
        t.geometry.major_radius = 5.0;
        t.geometry.minor_radius = 5.0;
        let t = std::sync::Arc::new(t);
        g.in_update = true;
        let a = g.create_object(&t, Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let b = g.create_object(&t, Coord3D::new(102.0, 100.0, 0.0), 0.0, 0);
        g.in_update = false;
        assert!(!g.objects[&a].collision_dirty && !g.objects[&b].collision_dirty, "no contact has been processed yet");
        g.update_partition();
        assert!(g.objects[&a].collision_dirty && g.objects[&b].collision_dirty, "the pair collided in the pass");
    }

    /// `Object::setTeam` (`Object.cpp:851`): assigning an object to an inactive player's team
    /// neutralises it instead ("we just assign objects to the neutral player if we try to
    /// misbehave").
    #[test]
    fn set_object_team_neutralises_an_inactive_player() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        g.set_neutral_player(Some(9));
        let t = std::sync::Arc::new(ObjectTemplate::new("Thing"));
        let id = g.create_object(&t, Coord3D::new(10.0, 10.0, 0.0), 0.0, 0);
        // An active player's team is honoured.
        g.set_object_team(id, 1);
        assert_eq!(g.find_object(id).unwrap().player, 1);
        // A dead player's team is replaced by the neutral player.
        g.player_dead.insert(2);
        g.set_object_temporary_team(id, 2);
        assert_eq!(g.find_object(id).unwrap().player, 9, "neutralised");
    }

    /// `Player::killPlayer` (`Player.cpp:2089`): the dead player's money is forced to 0.
    #[test]
    fn kill_player_zeroes_the_money() {
        let mut g = GameLogic::new(TerrainLogic::flat(50, 50, 0), 1);
        g.add_player(0, 1234);
        g.player_dead.insert(0);
        g.kill_player(0);
        assert_eq!(g.player(0).unwrap().money, 0);
    }

}
