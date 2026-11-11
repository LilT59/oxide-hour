//! `Game`: skirmish start-up (a port of `GameLogic::startNewGame`) plus the client-facing API.
//!
//! # `startNewGame` (GeneralsMD/Code/GameEngine/Source/GameLogic/System/GameLogic.cpp, `tryStartNewGame`)
//!
//! | Original step | Here |
//! |---|---|
//! | `m_frame = 0`, `m_nextObjID = 1` | fresh [`GameLogic`] |
//! | `loadMapINI`, `TheTerrainLogic->loadMap` | map parsed with `zr-map`; terrain from its height map and water polygons; map.ini / solo.ini overlaid by `GameData::for_map` (templates rebuilt), `map.str` into the text table |
//! | per-slot `Dict` (`player<i>`, faction, start index, skirmish flags) + `TheSidesList->addSide/addTeam` | [`PlayerInfo`] list |
//! | `SidesList::prepareForMP_or_Skirmish` (drop every map side but `FactionCivilian`, drop map teams) | only neutral + the map's civilian sides survive; `team<player>` default teams are the only team names that resolve |
//! | extra `ReplayObserver` side | present, last player index, owns nothing |
//! | `PlayerList::newGame` | index 0 neutral, then civilian side(s), then `player0..`, then the observer |
//! | `Player::init`: `m_money = PlayerTemplate StartMoney`, deposit skirmish starting cash if 0 | same (`StartMoney`, else the `Default = Yes` `MultiplayerStartingMoneyChoice`) |
//! | `TerrainLogic::newMap` (waypoint z snapped to ground) | waypoints recorded with ground z |
//! | bridge-like map objects first (`isBridgeLike`), neutral team | ported: created before everything else |
//! | main map-object loop: skip road/bridge-flag points, no template, bridge-like (done), `OPTIMIZED_TREE`, `PROP`, `CLEARED_BY_BUILD` fluff when `forceFluffToProp`; owner via `originalOwner` team; `pos.z += groundHeight`; `normalizeAngle` | ported (skirmish/replay: fluff is a client prop; shell: kept, static LOD High) |
//! | `updateObjValuesFromMapProperties`, `onBuildComplete` creates, pathfind map | stub |
//! | `placeNetworkBuildingsForPlayer` / `placeObjectAtPosition` | ported: `Player_<n+1>_Start`, building at ground z facing `PlacementViewAngle`, units from `Player_<n+1>_Rally` or the start point shifted by `-boundingSphereRadius/2` in y, `findPositionAround` ring search (min `0.7R`, max `1.3R`, random start angle from the logic RNG) |
//! | `PartitionManager::update`, shroud, radar, scripts, victory conditions, camera, loadscreen | not ported |
//! | `ThePlayerList->newMap` | not ported |

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use zr_assets::{normalize_path, Vfs};
use zr_map::{Dict, MapFile, MapObject};
use zr_sim::crc::Crc;
use zr_sim::damage::Relationship;
use zr_sim::math::{cos, normalize_angle, real_to_int_ceil, sin, PI, TWO_PI};
use zr_sim::{
    AiBuildOrder, Command, Coord3D, GameLogic, ObjectId, ObjectTemplate, PlayerIndex, SkirmishAiConfig, TemplateSource,
    TerrainLogic,
};
use zr_templates::{BuildListEntry, KindOf, NameEnum};

use crate::data::GameData;

/// `FLAG_ROAD_FLAGS` / `FLAG_BRIDGE_FLAGS` (Core/GameEngine/Include/Common/MapObject.h).
const FLAG_ROAD_FLAGS: i32 = 0x2 | 0x4;
const FLAG_BRIDGE_FLAGS: i32 = 0x10 | 0x20;
/// `PartitionManager::findPositionAround` ring spacing.
const RING_SPACING: f32 = 5.0;

/// `ThingFactory::findTemplate` over a shared [`GameData`], handed to [`GameLogic`] so production
/// can resolve the template names carried by `Command::QueueUnit`.
struct StoreSource(Arc<GameData>);

impl TemplateSource for StoreSource {
    fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
        self.0.store.find_template(name)
    }
    fn harvester_templates(&self) -> Vec<String> {
        self.0.store.harvester_templates()
    }
    fn dozer_templates(&self) -> Vec<String> {
        self.0.store.dozer_templates()
    }
    fn can_make_unit(&self, builder: &str, unit: &str) -> Option<bool> {
        self.0.store.can_make_unit(builder, unit)
    }
    fn can_build_upgrade(&self, builder: &str, upgrade: &str) -> Option<bool> {
        self.0.store.can_build_upgrade(builder, upgrade)
    }
}

/// A [`TemplateSource`] over a shared [`GameData`], for callers that build a [`GameLogic`] by hand
/// (demos / tests) and need production / OCL to resolve template names.
pub fn store_template_source(data: Arc<GameData>) -> Arc<dyn TemplateSource> {
    Arc::new(StoreSource(data))
}

/// Skirmish AI difficulty (`SLOT_EASY_AI` / `SLOT_MED_AI` / `SLOT_BRUTAL_AI`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AiDifficulty {
    Easy,
    #[default]
    Normal,
    Hard,
}

/// One occupied skirmish slot.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerSetup {
    /// `PlayerTemplate` name, e.g. `FactionAmerica` (or a general's template).
    pub faction: String,
    /// Zero-based start position: waypoint `Player_<start_position + 1>_Start`.
    pub start_position: u8,
    /// Run the skirmish AI for this slot (`AIPlayer`).
    pub is_ai: bool,
    /// Skirmish AI difficulty (ignored for human slots).
    pub difficulty: AiDifficulty,
    /// `GameInfo` team number: `0` = no team (free-for-all), `1..=4` group allies.
    pub team: u8,
    /// `MultiplayerColor` index, or `-1` for the faction's `PreferredColor`.
    pub color: i32,
    /// `GameInfo` per-slot starting cash override (`None` uses the game default).
    pub starting_cash: Option<u32>,
}

impl PlayerSetup {
    /// A human slot.
    pub fn new(faction: &str, start_position: u8) -> Self {
        Self { faction: faction.to_owned(), start_position, is_ai: false, difficulty: AiDifficulty::Normal, team: 0, color: -1, starting_cash: None }
    }
    /// A computer slot (`AISkirmishPlayer`).
    pub fn ai(faction: &str, start_position: u8) -> Self {
        Self { faction: faction.to_owned(), start_position, is_ai: true, difficulty: AiDifficulty::Normal, team: 0, color: -1, starting_cash: None }
    }
    pub fn with_difficulty(mut self, difficulty: AiDifficulty) -> Self {
        self.difficulty = difficulty;
        self
    }
    pub fn with_team(mut self, team: u8) -> Self {
        self.team = team;
        self
    }
    pub fn with_color(mut self, color: i32) -> Self {
        self.color = color;
        self
    }
    pub fn with_cash(mut self, starting_cash: u32) -> Self {
        self.starting_cash = Some(starting_cash);
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerKind {
    /// Index 0; owns scenery and tech buildings by default.
    Neutral,
    /// A `FactionCivilian` side from the map.
    Civilian,
    /// A skirmish slot (`player<i>`).
    Skirmish,
    /// The permanent `ReplayObserver`.
    Observer,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerInfo {
    pub index: PlayerIndex,
    /// `playerName` (`""` for neutral, `player0`, ...).
    pub name: String,
    pub kind: PlayerKind,
    pub faction: String,
    pub side: String,
    /// Current money (`Money::countMoney`); not yet changed by the sim.
    pub money: u32,
    pub start_position: Option<u8>,
    /// This slot runs the skirmish AI (`AISkirmishPlayer`).
    pub is_ai: bool,
    /// `GameInfo` team number (`0` = no team).
    pub team: u8,
    /// `MultiplayerColor` index, or `-1` for the faction's `PreferredColor`.
    pub color: i32,
}

/// A named waypoint (`TerrainLogic::addWaypoint`), z already snapped to the ground.
#[derive(Clone, Debug, PartialEq)]
pub struct Waypoint {
    pub name: String,
    pub id: i32,
    pub pos: Coord3D,
    /// `waypointPathLabel1..3`: the approach paths this waypoint belongs to.
    pub labels: Vec<String>,
}

/// What the start-up created for a player's starting set (for diagnostics and tests).
#[derive(Clone, Debug, PartialEq)]
pub struct Spawned {
    pub player: PlayerIndex,
    pub id: ObjectId,
    pub template: String,
    pub pos: Coord3D,
    /// `"StartingBuilding"` or `"StartingUnit<i>"`.
    pub role: String,
}

/// Counters from the map-object pass.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MapLoadStats {
    pub map_objects: usize,
    pub created: usize,
    pub waypoints: usize,
    pub road_or_bridge_points: usize,
    pub no_template: usize,
    pub props_or_trees_skipped: usize,
}

/// Read-only view of one live object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ObjectView<'a> {
    pub id: ObjectId,
    pub template: &'a str,
    pub owner: PlayerIndex,
    pub position: Coord3D,
    pub angle: f32,
    pub health: f32,
    /// `ModelConditionFlags` for this frame (`crate::GameLogic::model_conditions`).
    pub model_conditions: zr_sim::ModelConditionFlags,
    /// `Drawable::isDrawableHidden` (`zr_sim::Object::is_drawable_hidden`).
    pub drawable_hidden: bool,
}

impl ObjectView<'_> {
    /// The client does not draw this object (e.g. a railroad car waiting in the wings).
    pub fn is_drawable_hidden(&self) -> bool {
        self.drawable_hidden
    }
}

fn view(o: &zr_sim::Object) -> ObjectView<'_> {
    ObjectView {
        id: o.id,
        template: &o.template.name,
        owner: o.player,
        position: o.position,
        angle: o.angle,
        health: o.health(),
        model_conditions: o.model_conditions,
        drawable_hidden: o.is_drawable_hidden(),
    }
}

pub struct Game {
    data: Arc<GameData>,
    logic: GameLogic,
    map_name: String,
    players: Vec<PlayerInfo>,
    waypoints: Vec<Waypoint>,
    spawned: Vec<Spawned>,
    stats: MapLoadStats,
    /// Active replay recorder (`Recorder.cpp`); every [`submit`](Self::submit) is appended.
    recorder: Option<crate::replay::Replay>,
}

fn dict_str<'a>(d: &'a Dict, k: &str) -> &'a str {
    d.str(k).unwrap_or("")
}

impl Game {
    /// Load the data from `vfs` and start a skirmish (convenience; loading the INI takes a few
    /// hundred ms, so prefer [`new_skirmish_with`](Self::new_skirmish_with) when starting several games).
    pub fn new_skirmish(vfs: &Vfs, map_name: &str, players: &[PlayerSetup], seed: u32) -> Result<Game> {
        Self::new_skirmish_with(Arc::new(GameData::load(vfs)), vfs, map_name, players, seed)
    }

    /// `startNewGame` for a skirmish. `map_name` is `Alpine Assault` or a full `maps\..\x.map` path.
    pub fn new_skirmish_with(data: Arc<GameData>, vfs: &Vfs, map_name: &str, setups: &[PlayerSetup], seed: u32) -> Result<Game> {
        let path = map_path(map_name);
        let raw = vfs.read(&path).ok_or_else(|| anyhow!("map `{path}` not found"))?;
        let bytes = zr_assets::compression::decompress_if_needed(&raw).context("decompressing map")?;
        let map = MapFile::parse(&bytes).map_err(|e| anyhow!("parsing map `{path}`: {e:?}"))?;
        // The client builds bridge geometry from the map + INI (`W3DBridgeBuffer::loadBridges`) and
        // hands the rectangles to `TerrainLogic`; do the same before `GameLogic::new`.
        let point_bridges = match (map.height_map.as_ref(), data.ini_db()) {
            (Some(hm), Some(ini)) => {
                let t = TerrainLogic::from_height_map(hm);
                crate::bridges::point_bridges(&map, vfs, &ini, &|x, y| t.ground_height(x, y))
            }
            _ => Vec::new(),
        };
        // GameLogic::loadMapINI: the map's own INI overrides apply before any template is used.
        let data = data.for_map(vfs, &path);
        // The scripts load inside `from_map_inner`, before the map objects (`ScriptEngine::newMap`,
        // GameLogic.cpp:1554, runs before the object loop at :1900 and the starting units at :2042).
        let mut game = Self::from_map_inner(data, &map, map_name, setups, seed, point_bridges, false, Some(vfs))?;
        // Scripts load before the AI configs so the phase-1 fallbacks (whole-list build, attack
        // group, inserted supply center) are only used when no scripts drive the AI.
        game.setup_ai_players(&map, setups);
        Ok(game)
    }

    /// As [`new_skirmish_with`](Self::new_skirmish_with) over an already parsed map (no point-pair
    /// bridges; `IsBridge` objects are still added).
    pub fn from_map(data: Arc<GameData>, map: &MapFile, map_name: &str, setups: &[PlayerSetup], seed: u32) -> Result<Game> {
        let mut game = Self::from_map_with_bridges(data, map, map_name, setups, seed, Vec::new())?;
        // No VFS, so no scripts: use the phase-1 AI fallbacks.
        game.setup_ai_players(map, setups);
        Ok(game)
    }

    /// [`from_map`](Self::from_map) plus bridge rectangles the caller built from `FLAG_BRIDGE_POINT`
    /// pairs (`W3DBridgeBuffer`); their layers are reassigned after the object bridges.
    pub fn from_map_with_bridges(
        data: Arc<GameData>,
        map: &MapFile,
        map_name: &str,
        setups: &[PlayerSetup],
        seed: u32,
        extra_bridges: Vec<zr_sim::bridge::Bridge>,
    ) -> Result<Game> {
        Self::from_map_inner(data, map, map_name, setups, seed, extra_bridges, false, None)
    }

    /// The skirmish and shell starts share the world setup; `shell` selects `GAME_SHELL`, where every
    /// named side is a player (see [`Self::new_shell`]). `scripts` is the VFS for the skirmish script
    /// lists, loaded before the map objects (`None` leaves the caller to install scripts).
    #[allow(clippy::too_many_arguments)]
    fn from_map_inner(
        data: Arc<GameData>,
        map: &MapFile,
        map_name: &str,
        setups: &[PlayerSetup],
        seed: u32,
        extra_bridges: Vec<zr_sim::bridge::Bridge>,
        shell: bool,
        scripts: Option<&Vfs>,
    ) -> Result<Game> {
        let hm = map.height_map.as_ref().ok_or_else(|| anyhow!("map has no height map"))?;
        let mut terrain = TerrainLogic::from_height_map(hm);
        // Water areas from the map's polygon triggers (TerrainLogic::getWaterHandle / isUnderwater).
        if let Some(tr) = &map.triggers {
            terrain.set_water_from_triggers(tr);
        }
        // The map's own cliff bits (BlendTileData v7+), used by the pathfinder; older maps use the slope rule.
        if let Some(bt) = &map.blend_tiles {
            terrain.set_cliff_state(bt.cell_cliff_state.clone());
        }
        // Bridges: `IsBridge` objects (the object-transform formula) then the caller's point pairs.
        let mut bridges = {
            let ground = |x: f32, y: f32| terrain.ground_height(x, y);
            crate::bridges::object_bridges(&data, map, &ground)
        };
        for mut b in extra_bridges {
            b.layer = crate::bridges::FIRST_BRIDGE_LAYER + bridges.len() as u32;
            bridges.push(b);
        }
        terrain.set_bridges(bridges);
        let mut logic = GameLogic::with_cell_size(terrain, seed, data.partition_cell_size);
        // The map's `Lighting` / `WorldInfo` chunks set the game-wide `TheGlobalData->m_timeOfDay` / `m_weather`
        // (`WorldHeightMap::ParseLightingDataChunk`, WorldHeightMap.cpp:742-744; `ParseWorldDictDataChunk`, :723-733),
        // and `Object::friend_bindToDrawable` (Object.cpp:3038-3048) folds them into every drawable's model conditions
        // as `NIGHT` / `SNOW`. The engine defaults are afternoon / normal (GlobalData.cpp:725-730), so a map without
        // those chunks leaves the sim's default. `TimeOfDay` is 1..4 = morning/afternoon/evening/night and `Weather`
        // 0/1 = normal/snowy (Core/GameEngine/Include/Common/GameType.h:64-85).
        let night = map.lighting.as_ref().is_some_and(|l| l.time_of_day == 4);
        let snow = map.world_info.as_ref().and_then(|d| d.int("weather")) == Some(1);
        logic.set_launch_environment(night, snow);
        // `populateRandomSideAndColor` runs first in `startNewGame` (GameLogic.cpp:1309), before any map or
        // script draw, so its draws start the logic RNG stream.
        let num_colors = data.ini_db().map_or(0, |ini| ini.of_kind("MultiplayerColor").count() as i32);
        draw_random_colors(logic.rng_mut(), setups, num_colors);
        // `Pathfinder::m_wallHeight` (`AIData WallHeight`): lets a unit on a wall be detected by z.
        if let Some(h) = data.templates().ai_data.as_ref().and_then(|a| a.extra.get("WallHeight")).and_then(|s| s.trim().parse::<f32>().ok()) {
            logic.set_wall_height(h);
        }
        // `AIData` target-selection tunables (`AI.cpp:159-169`); unset keys keep the retail defaults.
        if let Some(a) = data.templates().ai_data.as_ref() {
            let mut t = zr_sim::ai::AiTunables::default();
            let real = |k: &str, d: f32| a.extra.get(k).and_then(|s| s.split(';').next()).and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(d);
            t.alert_range_modifier = real("AlertRangeModifier", t.alert_range_modifier);
            t.aggressive_range_modifier = real("AggressiveRangeModifier", t.aggressive_range_modifier);
            t.guard_outer_modifier_ai = real("GuardOuterModifierAI", t.guard_outer_modifier_ai);
            t.guard_outer_modifier_human = real("GuardOuterModifierHuman", t.guard_outer_modifier_human);
            t.guard_inner_modifier_ai = real("GuardInnerModifierAI", t.guard_inner_modifier_ai);
            t.guard_inner_modifier_human = real("GuardInnerModifierHuman", t.guard_inner_modifier_human);
            t.attack_priority_distance_modifier = real("AttackPriorityDistanceModifier", t.attack_priority_distance_modifier);
            t.ai_dozer_bored_radius_modifier = real("AIDozerBoredRadiusModifier", t.ai_dozer_bored_radius_modifier);
            // `GuardChaseUnitsDuration` (`INI::parseDurationUnsignedInt`: ms in, frames rounded up out).
            if let Some(ms) = a.extra.get("GuardChaseUnitsDuration").and_then(|s| s.split(';').next()).and_then(|s| s.trim().parse::<u32>().ok()) {
                t.guard_chase_unit_frames = zr_templates::value::duration_ms_to_frames_ceil(ms);
            }
            // `ForceIdleMSEC` (`INI::parseDurationUnsignedInt`); an absent key keeps the engine default of 1 frame.
            if a.force_idle_frames != 0 {
                t.force_idle_frames = a.force_idle_frames;
            }
            // `AttackUsesLineOfSight` (`INI::parseBool`, `AI.cpp:165`): an absent key keeps the engine
            // default of true (`AI.cpp:941`).
            if let Some(v) = a.extra.get("AttackUsesLineOfSight").and_then(|s| s.split(';').next()).map(|s| s.trim().to_ascii_lowercase()) {
                t.attack_uses_line_of_sight = v == "yes" || v == "true" || v == "1";
            }
            logic.set_ai_tunables(t);
        }
        logic.set_neutral_player(Some(0)); // PlayerList index 0 is the neutral player
        // Production resolves `Command::QueueUnit` template names through the typed store.
        logic.set_template_source(Arc::new(StoreSource(data.clone())));
        // Global economy/build rules from the `GameData` block (`GlobalData.cpp`).
        {
            let t = data.templates();
            logic.set_rules(zr_sim::GameRules {
                value_per_supply_box: t.value_per_supply_box,
                allowed_height_variation_for_building: t.allowed_height_variation_for_building,
                low_energy_penalty_modifier: t.low_energy_penalty_modifier,
                min_low_energy_production_speed: t.min_low_energy_production_speed,
                max_low_energy_production_speed: t.max_low_energy_production_speed,
                gravity: t.gravity,
                ground_stiffness: t.ground_stiffness,
                structure_stiffness: t.structure_stiffness,
                unit_damaged_threshold: data.unit_damaged_threshold,
                unit_really_damaged_threshold: data.unit_really_damaged_threshold,
                movement_penalty_damage_state: data.movement_penalty_damage_state,
                base_regen_health_percent_per_second: data.base_regen_health_percent_per_second,
                base_regen_delay: data.base_regen_delay,
                max_tunnel_capacity: data.max_tunnel_capacity,
            });
            // Global `WeaponBonus` + `HealthBonus_<Level>` and the upgrade research table.
            logic.set_global_bonuses(crate::adapter::global_weapon_bonus(t), t.health_bonus);
            logic.set_upgrade_info(crate::adapter::upgrade_info(t));
            logic.set_special_powers(crate::adapter::special_power_store(t));
            let (sciences, ranks) = crate::adapter::science_tables(t);
            logic.set_science_tables(sciences, ranks);
        }

        // ---- SidesList::prepareForMP_or_Skirmish + PlayerList::newGame -----------------------
        let mut players = vec![PlayerInfo {
            index: 0,
            name: String::new(),
            kind: PlayerKind::Neutral,
            faction: String::new(),
            side: String::new(),
            money: 0,
            start_position: None,
            is_ai: false,
            team: 0,
            color: -1,
        }];
        // Team names that resolve (`TeamFactory::findTeam`), mapped to their owner.
        let mut teams: BTreeMap<String, PlayerIndex> = BTreeMap::new();
        teams.insert("team".to_owned(), 0); // neutral default team ("team" + "")
        // `TheGameInfo->getStartingCash()` (else `TheGlobalData->m_defaultStartingCash`):
        // `Player::init` (Player.cpp:427-443) gives every player whose PlayerTemplate `Money` is 0
        // this amount, civilian and replay-observer players included.
        let game_starting_cash = setups.first().and_then(|s| s.starting_cash).unwrap_or(data.default_starting_cash);
        if let Some(sides) = &map.sides {
            for s in &sides.sides {
                let civilian = dict_str(&s.dict, "playerFaction") == "FactionCivilian";
                // Shell maps: `PlayerList::newGame` makes a player for every side with a name.
                if civilian || (shell && !dict_str(&s.dict, "playerName").is_empty()) {
                    let idx = next_index(&players)?;
                    let name = dict_str(&s.dict, "playerName").to_owned();
                    teams.insert(format!("team{name}").to_ascii_lowercase(), idx);
                    let faction = dict_str(&s.dict, "playerFaction");
                    let faction = if civilian || faction.is_empty() { "FactionCivilian" } else { faction };
                    let templ = data.player_template(faction);
                    // `Player::init`: the template's `Money`, else the starting cash.
                    let start_money = templ.map_or(0, |p| p.start_money);
                    players.push(PlayerInfo {
                        index: idx,
                        name,
                        kind: if civilian { PlayerKind::Civilian } else { PlayerKind::Skirmish },
                        faction: faction.to_owned(),
                        side: templ.map(|p| p.side.clone()).unwrap_or_default(),
                        money: if start_money != 0 { start_money } else { game_starting_cash },
                        start_position: None,
                        is_ai: false,
                        team: 0,
                        color: -1,
                    });
                }
            }
        }
        for (slot, s) in setups.iter().enumerate() {
            let pt = data.player_template(&s.faction).ok_or_else(|| anyhow!("unknown player template `{}`", s.faction))?;
            let idx = next_index(&players)?;
            let name = format!("player{slot}");
            teams.insert(format!("team{name}").to_ascii_lowercase(), idx);
            // Player::init: `StartMoney` wins, else the skirmish starting-cash choice.
            let money = if pt.start_money != 0 { pt.start_money } else { s.starting_cash.unwrap_or(data.default_starting_cash) };
            players.push(PlayerInfo {
                index: idx,
                name,
                kind: if pt.is_observer { PlayerKind::Observer } else { PlayerKind::Skirmish },
                faction: pt.name.clone(),
                side: pt.side.clone(),
                money,
                start_position: Some(s.start_position),
                is_ai: s.is_ai && !pt.is_observer,
                team: s.team,
                color: s.color,
            });
        }
        let idx = next_index(&players)?;
        teams.insert("teamreplayobserver".to_owned(), idx);
        // `Player::init` (Player.cpp:427-443): the FactionObserver template's `Money`, else the
        // starting cash — the observer is a normal player to `Player::init` (the snapshot's
        // civilian/observer money row).
        let observer_money = data.player_template("FactionObserver").map_or(0, |p| p.start_money);
        players.push(PlayerInfo {
            index: idx,
            name: "ReplayObserver".to_owned(),
            kind: PlayerKind::Observer,
            faction: "FactionObserver".to_owned(),
            side: "Observer".to_owned(),
            money: if observer_money != 0 { observer_money } else { game_starting_cash },
            start_position: Some(0),
            is_ai: false,
            team: 0,
            color: -1,
        });

        // `Player::init` starting money lives in the sim so production can withdraw/refund it.
        for p in &players {
            logic.add_player(p.index, p.money);
        }
        // `GameLogic::startNewGame` (GameLogic.cpp:1803-1822): observers see the whole map for good,
        // and with `UseShroud` off (retail `Multiplayer.ini`) every occupied skirmish slot gets
        // `revealMapForPlayer`, which clears the passive shroud into fog (never-seen cells are fogged,
        // not black). Shell sides are left alone.
        if !shell {
            for p in &players {
                match p.kind {
                    PlayerKind::Observer => logic.reveal_map_permanently_for_player(p.index),
                    PlayerKind::Skirmish if !data.use_shroud_in_multiplayer => logic.reveal_map_for_player(p.index),
                    _ => {}
                }
            }
        }
        // `Player::resetSciences`: grant the `PlayerTemplate`'s `IntrinsicSciences` (the faction
        // science plus `SCIENCE_Rank1`, which the skill-set sciences list as a prerequisite).
        for p in &players {
            let Some(pt) = data.player_templates.iter().find(|pt| pt.name == p.faction) else { continue };
            for sci in &pt.intrinsic_sciences {
                logic.add_science(p.index, sci);
            }
        }

        // `PlayerList::newGame` (Common/RTS/PlayerList.cpp:186) and `GameLogic::startNewGame`
        // (GameLogic/System/GameLogic.cpp:2166): skirmish slots are mutual enemies, except slots on
        // the same non-zero `GameInfo` team, which are allies; civilians and the neutral player keep
        // the default `NEUTRAL` relationship.
        // Shell sides take their relations from `playerEnemies` / `playerAllies` (`new_shell`).
        let skirmish: Vec<(PlayerIndex, u8)> =
            players.iter().filter(|p| !shell && p.kind == PlayerKind::Skirmish).map(|p| (p.index, p.team)).collect();
        for (i, &(a, ta)) in skirmish.iter().enumerate() {
            for &(b, tb) in &skirmish[i + 1..] {
                let rel = if ta > 0 && ta == tb { Relationship::Allies } else { Relationship::Enemies };
                logic.set_relationship_symmetric(a, b, rel);
            }
        }

        let mut game = Game {
            data,
            logic,
            map_name: map_name.to_owned(),
            players,
            waypoints: Vec::new(),
            spawned: Vec::new(),
            stats: MapLoadStats::default(),
            recorder: None,
        };
        if let Some(vfs) = scripts {
            game.load_scripts(vfs, map);
        }
        // `forceFluffToProp` (`GameLogic.cpp:1833-1843`): true in a multiplayer game (skirmish or
        // replay). The shell (`GAME_SHELL`, `isMultiplayer` false) keeps the static-LOD-High default:
        // fluff stays a logic object.
        game.load_map_objects(map, &teams, !shell);
        if scripts.is_some() {
            game.install_script_world(map, false);
        }
        // `Pathfinder::newMap`: build the wall layer once, after every `addWallPiece`.
        game.logic.rebuild_wall_layer();
        game.place_starting_sets(setups)?;
        game.set_player_difficulties(setups);
        Ok(game)
    }

    /// `Player::initFromDict` -> `m_ai->setAIDifficulty(difficulty)`: record each skirmish slot's
    /// difficulty so the Easy/Normal/Hard script and team production-condition flags use it.
    fn set_player_difficulties(&mut self, setups: &[PlayerSetup]) {
        for (slot, s) in setups.iter().enumerate() {
            let index = self.player_by_name(&format!("player{slot}")).map(|p| p.index);
            if let Some(index) = index {
                self.logic.scripts_mut().set_player_difficulty(index, to_script_difficulty(s.difficulty));
            }
        }
    }

    /// `SidesList::prepareForMP_or_Skirmish` + `Player::initFromDict`: install the map's scripts, or
    /// (the normal skirmish case) `SkirmishScripts.scb`, **duplicated and qualified per player**
    /// (`duplicateAndQualify`), plus `MultiplayerScripts.scb`'s victory/defeat scripts. Counters,
    /// flags, scripts, subroutines and teams get the player's `m_mpStartIndex` appended, so each
    /// player has its own script state instead of sharing globals.
    ///
    /// Who gets which list (`Player::initFromDict`, `Player.cpp:822-905`): a **computer** player gets
    /// its faction's `Skirmish<Faction>` list qualified by its `multiplayerStartIndex` (plus the
    /// skirmish teams); a **human** player gets the *Civilian* skirmish side's list (`PlyrCivilian`),
    /// qualified with `m_mpStartIndex` as it stands at that point, which is still unset (0) because
    /// it is only assigned from the dict after the human branch. The civilian side's list is
    /// consumed (`setScriptList(nullptr)`), so only the first human slot receives it.
    fn load_scripts(&mut self, vfs: &Vfs, map: &MapFile) {
        /// Per-skirmish-player qualification inputs.
        struct Info {
            index: PlayerIndex,
            name: String,
            start: i32,
            script_name: String,
            is_ai: bool,
        }
        let infos: Vec<Info> = self
            .players
            .iter()
            .filter(|p| p.kind == PlayerKind::Skirmish)
            .map(|p| Info {
                index: p.index,
                name: p.name.clone(),
                start: i32::from(p.start_position.unwrap_or(0)),
                script_name: skirmish_script_name(&p.faction),
                is_ai: p.is_ai,
            })
            .collect();
        let qualifier_of = |i: &Info| i.start.to_string();
        let template_of = |i: &Info| format!("{}{}", i.script_name, qualifier_of(i));

        // The first skirmish side whose template side is "Civilian" (`Player.cpp:835-843`): its
        // `playerName` names the list a human player receives. `m_mpStartIndex` is 0 there.
        let civ_side: Option<(usize, String)> = map.sides.as_ref().and_then(|s| {
            s.sides.iter().enumerate().find_map(|(i, side)| {
                let pt = self.data.player_template(dict_str(&side.dict, "playerFaction"))?;
                (pt.side == "Civilian").then(|| (i, dict_str(&side.dict, "playerName").to_owned()))
            })
        });
        // Only the first human takes the civilian list (the skirmish side's list is then nulled).
        let first_human = infos.iter().find(|i| !i.is_ai).map(|i| i.index);

        let mut entries: Vec<(PlayerIndex, zr_map::ScriptList)> = Vec::new();
        let mut teams: Vec<zr_map::Dict> = Vec::new();

        // The map's own scripts, if any (one `ScriptList` per side, in side order). The retail
        // `prepareForMP_or_Skirmish` (`SidesList.cpp:507`) falls back to the shared `.scb` only when
        // no non-civilian side has any script.
        let non_civilian_scripts = map.scripts().is_some_and(|ps| {
            let sides = map.sides.as_ref().map_or(&[][..], |s| &s.sides);
            ps.lists.iter().enumerate().any(|(i, l)| {
                let civilian = sides.get(i).is_some_and(|s| dict_str(&s.dict, "playerFaction") == "FactionCivilian");
                !civilian && (!l.scripts.is_empty() || !l.groups.is_empty())
            })
        });
        if non_civilian_scripts {
            if let Some(ps) = map.scripts() {
                let sides = map.sides.as_ref().map_or(&[][..], |s| &s.sides);
                for info in &infos {
                    if !info.is_ai {
                        if Some(info.index) == first_human {
                            if let Some((ci, cname)) = &civ_side {
                                if let Some(list) = ps.lists.get(*ci) {
                                    entries.push((info.index, list.duplicate_and_qualify("0", &format!("{cname}0"), &info.name)));
                                }
                            }
                        }
                        continue;
                    }
                    let faction = format!("Faction{}", &info.script_name["Skirmish".len()..]);
                    let Some(list) =
                        sides.iter().position(|s| dict_str(&s.dict, "playerFaction") == faction).and_then(|i| ps.lists.get(i))
                    else {
                        continue;
                    };
                    if list.scripts.is_empty() && list.groups.is_empty() {
                        continue;
                    }
                    entries.push((info.index, list.duplicate_and_qualify(&qualifier_of(info), &template_of(info), &info.name)));
                }
            }
        } else if let Some(bytes) = vfs.read("Data\\Scripts\\SkirmishScripts.scb") {
            if let Ok(f) = zr_map::parse_scripts_file(&bytes) {
                if let Some(ps) = &f.scripts {
                    for info in &infos {
                        if !info.is_ai {
                            // Human: the Civilian side's list, qualified with start index 0.
                            if Some(info.index) == first_human {
                                if let Some((_, cname)) = &civ_side {
                                    if let Some(list) = f.players.iter().position(|sp| &sp.name == cname).and_then(|i| ps.lists.get(i)) {
                                        entries.push((info.index, list.duplicate_and_qualify("0", &format!("{cname}0"), &info.name)));
                                    }
                                }
                            }
                            continue;
                        }
                        let Some(i) = f.players.iter().position(|sp| sp.name == info.script_name) else { continue };
                        let Some(list) = ps.lists.get(i) else { continue };
                        entries.push((info.index, list.duplicate_and_qualify(&qualifier_of(info), &template_of(info), &info.name)));
                    }
                }
                // Per-player qualified team copies (`Player::initFromDict`): computer players only.
                for info in infos.iter().filter(|i| i.is_ai) {
                    for t in &f.teams {
                        if t.str("teamOwner") == Some(info.script_name.as_str()) {
                            teams.push(zr_map::qualify_team_dict(t, &qualifier_of(info), &info.name));
                        }
                    }
                }
            }
        }
        // `MultiplayerScripts.scb` victory/defeat scripts (`GameLogic.cpp:1583`, only with 2+
        // teams). Retail appends them once, unqualified, to side 0 and evaluates them against the
        // *local* player; the sim has no local-player notion, so each player gets its own qualified
        // copy (documented deviation).
        if infos.len() > 1 {
            if let Some(bytes) = vfs.read("Data\\Scripts\\MultiplayerScripts.scb") {
                if let Ok(mp) = zr_map::parse_scripts_file(&bytes) {
                    if let Some(l0) = mp.scripts.as_ref().and_then(|ps| ps.lists.first()) {
                        for info in &infos {
                            let q = l0.duplicate_and_qualify(&qualifier_of(info), &template_of(info), &info.name);
                            match entries.iter_mut().find(|(idx, _)| *idx == info.index) {
                                Some((_, entry)) => entry.scripts.extend(q.scripts),
                                None => entries.push((info.index, zr_map::ScriptList { scripts: q.scripts, groups: Vec::new() })),
                            }
                        }
                    }
                }
            }
        }

        // `GameLogic::startNewGame` adds `teamReplayObserver` (GameLogic.cpp:1543-1548) and then
        // `SidesList::validateSides` fills in every side's missing default team (SidesList.cpp:750-784).
        // They come before every player's qualified skirmish teams, so `TeamFactory::initFromSides`
        // gives their instances the first ids.
        self.install_script_lists(entries, &teams, false);
    }

    /// The default-team prototypes the sides list always has before the players' own teams:
    /// `GameLogic::startNewGame`'s slot loop adds `"team" + playerN` for every occupied slot
    /// (GameLogic.cpp:1494-1500), then the observer's `teamReplayObserver` (:1543-1548), then
    /// `SidesList::validateSides` adds `"team" + playerName` for every side that still has no default
    /// team (Core/SidesList.cpp:750-784). `Player::initFromDict` removes the slot team again for an AI
    /// skirmish slot (Player.cpp:917-926), which the qualified `.scb` teams replace; a human slot keeps
    /// it. The port's map sides are its map-side players: the neutral player, the map's civilian sides
    /// and, in the shell, every named side (`shell` is exactly the case where the map's named players
    /// are the skirmish-shaped ones and must be included).
    fn default_team_protos(&self, shell: bool) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if !shell {
            for p in self.players.iter().filter(|p| p.kind == PlayerKind::Skirmish && !p.is_ai) {
                out.push((format!("team{}", p.name), p.name.clone()));
            }
        }
        out.push(("teamReplayObserver".to_owned(), "ReplayObserver".to_owned()));
        for p in &self.players {
            if p.kind == PlayerKind::Observer || (p.kind == PlayerKind::Skirmish && !shell) {
                continue;
            }
            out.push((format!("team{}", p.name), p.name.clone()));
        }
        out
    }

    /// Installs the script lists, the teams and the map's script data. `shell` is `GAME_SHELL`:
    /// no victory players (`VictoryConditions` is not run), and the enemies come from the sides
    /// (set by [`Self::new_shell`]).
    fn install_scripts(&mut self, map: &MapFile, entries: Vec<(PlayerIndex, zr_map::ScriptList)>, teams: &[zr_map::Dict], shell: bool) {
        // GAME_SHELL goes through the same `startNewGame` block (GameLogic.cpp:1543-1557): the
        // observer's team and a default team for every side the shell map has.
        self.install_script_lists(entries, teams, shell);
        self.install_script_world(map, shell);
    }

    /// The script lists, teams, command buttons and the first-evaluation draws (`ScriptEngine::newMap`,
    /// `checkConditionsForTeamNames`). Runs before the map objects. `shell` is `GAME_SHELL`; it selects
    /// which players are sides list sides for the default teams (see [`Self::default_team_protos`]).
    fn install_script_lists(&mut self, entries: Vec<(PlayerIndex, zr_map::ScriptList)>, teams: &[zr_map::Dict], shell: bool) {
        let protos = self.default_team_protos(shell);
        let default_teams: Vec<(&str, &str)> = protos.iter().map(|(n, o)| (n.as_str(), o.as_str())).collect();
        for p in &self.players {
            if p.kind == PlayerKind::Skirmish {
                self.logic.scripts_mut().set_player_meta(p.index, &p.side, i32::from(p.start_position.unwrap_or(0)));
                self.logic.scripts_mut().set_player_name(&p.name, p.index);
                self.logic.scripts_mut().set_player_name(&skirmish_script_name(&p.faction), p.index);
            }
        }
        // A skirmish game's `prepareForMP_or_Skirmish` clears the map's teams (SidesList.cpp:496-500),
        // so its default teams come first and the qualified `.scb` teams after. GAME_SHELL keeps the
        // map's teams, and `startNewGame` appends the observer's and `validateSides`' missing default
        // teams after them (GameLogic.cpp:1543-1557, SidesList.cpp:750-784), so they go last there.
        if shell {
            self.logic.scripts_mut().load_teams(teams);
            self.logic.scripts_mut().load_default_team_protos(&default_teams);
        } else {
            self.logic.scripts_mut().load_default_team_protos(&default_teams);
            self.logic.scripts_mut().load_teams(teams);
        }
        self.logic.scripts_mut().load_command_buttons(self.data.command_buttons.clone());
        self.logic.scripts_mut().load_command_button_powers(self.data.command_button_powers.clone());
        self.logic.scripts_mut().load_command_button_upgrades(self.data.command_button_upgrades.clone());
        self.logic.scripts_mut().load_command_button_actions(self.data.command_button_actions.clone());
        self.logic.scripts_mut().load(entries);
        // `PlayerList::newGame` -> `TeamFactory::initFromSides` + `Player::setDefaultTeam`: the team
        // instances exist (and their ids are taken) before the map objects are created.
        let defaults = self.default_team_names();
        let refs: Vec<(&str, PlayerIndex)> = defaults.iter().map(|(n, p)| (n.as_str(), *p)).collect();
        self.logic.init_from_sides(&refs);
        self.logic.draw_script_first_evaluation();
    }

    /// Each player's default team name, `team` + the player's name (`Player::setDefaultTeam`,
    /// Player.cpp:760-770). The neutral player's name is empty; a skirmish **AI**'s is the qualified
    /// `Skirmish<Faction><start>` name (`Player::initFromDict` replaces `m_playerName`,
    /// Player.cpp:888-899), while a human slot keeps `playerN` (that branch never renames it, so its
    /// default team is the `teamplayerN` `startNewGame` added).
    fn default_team_names(&self) -> Vec<(String, PlayerIndex)> {
        self.players
            .iter()
            .map(|p| {
                let name = match p.kind {
                    PlayerKind::Neutral => String::new(),
                    PlayerKind::Skirmish => match p.start_position {
                        Some(s) if p.is_ai => format!("{}{s}", skirmish_script_name(&p.faction)),
                        _ => p.name.clone(),
                    },
                    _ => p.name.clone(),
                };
                (format!("team{name}"), p.index)
            })
            .collect()
    }

    /// The script world data that needs the map's waypoints (`load_map_objects`): triggers, waypoint paths,
    /// the attack-priority kinds, the skirmish enemies and the victory players.
    fn install_script_world(&mut self, map: &MapFile, shell: bool) {
        if let Some(tr) = &map.triggers {
            self.logic.scripts_mut().set_triggers(tr);
            // The polygons the AI's area guards and the trigger-area clock read (lane 184).
            self.logic.set_polygon_triggers(tr);
        }
        let wps: Vec<(String, zr_sim::Coord3D)> = self.waypoints.iter().map(|w| (w.name.clone(), w.pos)).collect();
        self.logic.scripts_mut().set_waypoints(wps);
        // `SKIRMISH_*_APPROACH_PATH`: waypoint path labels + the map's `WaypointsList` links.
        let paths: Vec<(i32, String, zr_sim::Coord3D, Vec<String>)> = self
            .waypoints
            .iter()
            .map(|w| (w.id, w.name.clone(), w.pos, w.labels.clone()))
            .collect();
        self.logic.scripts_mut().set_waypoint_paths(paths, directed_waypoint_links(map));
        // `SET_ATTACK_PRIORITY_KIND_OF`: the `KindOf` index -> template-name table.
        let mut kind_of_templates: BTreeMap<i32, Vec<String>> = BTreeMap::new();
        let templates = self.data.templates();
        for i in 0..zr_templates::KindOf::count() {
            let kind = zr_templates::KindOf::from_index(i);
            let names: Vec<String> = templates
                .things()
                .iter()
                .filter(|t| t.kind_of.contains(kind))
                .map(|t| t.name.clone())
                .collect();
            if !names.is_empty() {
                kind_of_templates.insert(i as i32, names);
            }
        }
        self.logic.scripts_mut().set_kind_of_templates(kind_of_templates);
        let skirmish: Vec<PlayerIndex> = self.players.iter().filter(|p| p.kind == PlayerKind::Skirmish).map(|p| p.index).collect();
        if !shell {
            for &p in &skirmish {
                if let Some(&e) = skirmish.iter().find(|&&q| q != p) {
                    self.logic.scripts_mut().set_player_enemy(p, e);
                }
            }
        }
        self.logic.set_victory_players(if shell { Vec::new() } else { skirmish });
    }

    /// The shell map as `GAME_SHELL` (`Shell::showShellMap` -> `GameLogic::startNewGame`): each named
    /// side is a player with its own script list, enemies and allies from its `playerEnemies` /
    /// `playerAllies` (`PlayerList::newGame`, `PlayerList.cpp`). There are no skirmish slots, no AI
    /// players and no victory check, so the map's own scripts drive everything.
    pub fn new_shell(data: Arc<GameData>, vfs: &Vfs, map_name: &str, seed: u32) -> Result<Game> {
        let path = map_path(map_name);
        let raw = vfs.read(&path).ok_or_else(|| anyhow!("map `{path}` not found"))?;
        let bytes = zr_assets::compression::decompress_if_needed(&raw).context("decompressing map")?;
        let map = MapFile::parse(&bytes).map_err(|e| anyhow!("parsing map `{path}`: {e:?}"))?;
        let point_bridges = match (map.height_map.as_ref(), data.ini_db()) {
            (Some(hm), Some(ini)) => {
                let t = TerrainLogic::from_height_map(hm);
                crate::bridges::point_bridges(&map, vfs, &ini, &|x, y| t.ground_height(x, y))
            }
            _ => Vec::new(),
        };
        let data = data.for_map(vfs, &path);
        let mut game = Self::from_map_inner(data, &map, map_name, &[], seed, point_bridges, true, None)?;
        game.load_shell_scripts(&map);
        Ok(game)
    }

    /// The shell's script set and relations: every side's list is installed for its own player
    /// (no qualification: the shell map is not a skirmish), its enemies / allies are the sides'.
    fn load_shell_scripts(&mut self, map: &MapFile) {
        let side_player: Vec<Option<PlayerIndex>> = map
            .sides
            .as_ref()
            .map_or_else(Vec::new, |s| {
                s.sides
                    .iter()
                    .map(|side| {
                        let name = dict_str(&side.dict, "playerName");
                        self.players.iter().find(|p| !name.is_empty() && p.name.eq_ignore_ascii_case(name)).map(|p| p.index)
                    })
                    .collect()
            });
        let mut entries: Vec<(PlayerIndex, zr_map::ScriptList)> = Vec::new();
        if let Some(ps) = map.scripts() {
            for (i, list) in ps.lists.iter().enumerate() {
                let Some(Some(index)) = side_player.get(i) else { continue };
                if !list.scripts.is_empty() || !list.groups.is_empty() {
                    entries.push((*index, list.clone()));
                }
            }
        }
        let teams: Vec<zr_map::Dict> = map.sides.as_ref().map_or_else(Vec::new, |s| s.teams.clone());
        self.install_scripts(map, entries, &teams, true);
        // `Player::setPlayerRelationship` from each side's `playerEnemies` / `playerAllies` names.
        if let Some(sides) = &map.sides {
            for side in &sides.sides {
                let Some(me) = self.players.iter().find(|p| p.name.eq_ignore_ascii_case(dict_str(&side.dict, "playerName"))).map(|p| p.index) else {
                    continue;
                };
                for (key, rel) in [("playerEnemies", Relationship::Enemies), ("playerAllies", Relationship::Allies)] {
                    for other in dict_str(&side.dict, key).split_whitespace() {
                        if let Some(o) = self.players.iter().find(|p| p.name.eq_ignore_ascii_case(other)).map(|p| p.index) {
                            if o != me {
                                self.logic.set_relationship_symmetric(me, o, rel);
                                if rel == Relationship::Enemies {
                                    self.logic.scripts_mut().set_player_enemy(me, o);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Waypoints (`TerrainLogic::addWaypoint`) and map objects (bridge-like first, then the rest).
    /// `fluff_to_prop` is the C++ `forceFluffToProp` (`GameLogic.cpp:1833-1843`): in a multiplayer game
    /// (skirmish or replay, `Recorder::isMultiplayer`) `CLEARED_BY_BUILD` objects without a fence width
    /// are created as client props (`TheTerrainVisual->addProp`, `:1909-1922`), not game objects.
    fn load_map_objects(&mut self, map: &MapFile, teams: &BTreeMap<String, PlayerIndex>, fluff_to_prop: bool) {
        self.stats.map_objects = map.objects.len();
        // `TerrainRoads` `TowerObjectName*` (`terrain.ini`), for the towers a landmark bridge creates
        // inside its own creation (`Bridge::Bridge(Object*)`).
        let tower_names = self.data.ini_db().as_deref().map(crate::bridges::tower_names).unwrap_or_default();
        let mut rest: Vec<(&MapObject, &str)> = Vec::new();
        let mut bridges: Vec<(&MapObject, &str)> = Vec::new();
        for o in &map.objects {
            if let Some(wname) = o.props.str("waypointName") {
                // MapObject::isWaypoint: recorded by TerrainLogic, not a game object.
                let p = Coord3D::new(o.pos[0], o.pos[1], 0.0);
                let z = self.logic.terrain().ground_height(p.x, p.y);
                // `TerrainLogic::addWaypoint`: `waypointPathLabel1..3` are the approach paths.
                let labels: Vec<String> = ["waypointPathLabel1", "waypointPathLabel2", "waypointPathLabel3"]
                    .iter()
                    .filter_map(|k| o.props.str(k))
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect();
                self.waypoints.push(Waypoint {
                    name: wname.to_owned(),
                    id: o.props.int("waypointID").unwrap_or(0),
                    pos: Coord3D::new(p.x, p.y, z),
                    labels,
                });
                self.stats.waypoints += 1;
                continue;
            }
            if o.flags & (FLAG_ROAD_FLAGS | FLAG_BRIDGE_FLAGS) != 0 {
                self.stats.road_or_bridge_points += 1;
                continue; // special-cased on the terrain side (zr-roads)
            }
            // handleNameChange
            let name = match o.template.as_str() {
                "AmericaTankLeopard" => "AmericaTankCrusader",
                "AmericaVehicleHumVee" => "AmericaVehicleHumvee",
                n => n,
            };
            let Some(thing) = self.data.templates().thing(name) else {
                self.stats.no_template += 1;
                continue;
            };
            let bridge_like = thing.is_bridge || thing.is_kind_of(KindOf::WalkOnTopOfWall);
            // `KINDOF_CLEARED_BY_BUILD` with `FenceWidth == 0` is "fluff" (`GameLogic.cpp:1909-1922`):
            // lamps, benches, crates and the like, drawn as client props in a multiplayer game. Fences
            // (`FenceWidth > 0`) stay game objects: they can be destroyed.
            let fluff = thing.is_kind_of(KindOf::ClearedByBuild) && thing.fence_width == 0.0;
            if bridge_like {
                bridges.push((o, name));
            } else if thing.is_kind_of(KindOf::OptimizedTree) || thing.is_kind_of(KindOf::Prop) || (fluff_to_prop && fluff) {
                self.stats.props_or_trees_skipped += 1; // client-only in the original
            } else {
                rest.push((o, name));
            }
        }
        for (o, name) in bridges {
            let neutral_team = self.logic.scripts().player_default_team(0).map(str::to_owned);
            self.create_map_object(o, name, 0, neutral_team.as_deref(), &tower_names);
        }
        for (o, name) in rest {
            // Get the team information
            // `PlayerList::validateTeam` (`PlayerList.cpp:294-308`): the map object's `originalOwner`
            // names a team; an unknown name falls back to the neutral player's default team. The
            // object then joins it (`TheThingFactory->newObject(thingTemplate, team)`,
            // `GameLogic.cpp:1923-1927`) and `team->setActive()` runs after it (`:1957`).
            let original_owner = o.props.str("originalOwner").unwrap_or("");
            let neutral_team = self.logic.scripts().player_default_team(0).map(str::to_owned);
            let team = self.logic.scripts_mut().validate_team(original_owner, neutral_team.as_deref());
            // The owner comes from the same team table the port used before (a map's `originalOwner`
            // is the default team name), so a team that cannot be resolved keeps its old owner.
            let owner = teams.get(&original_owner.to_ascii_lowercase()).copied().unwrap_or(0);
            self.create_map_object(o, name, owner, team.as_deref(), &tower_names);
        }
    }

    fn create_map_object(
        &mut self,
        o: &MapObject,
        name: &str,
        owner: PlayerIndex,
        team: Option<&str>,
        tower_names: &BTreeMap<String, [Option<String>; 4]>,
    ) {
        let Some(t) = self.data.store.find_template(name) else { return };
        let ground = self.logic.terrain().ground_height(o.pos[0], o.pos[1]);
        let pos = Coord3D::new(o.pos[0], o.pos[1], o.pos[2] + ground);
        let id = self.logic.create_object(&t, pos, normalize_angle(o.angle), owner);
        // `TheThingFactory->newObject(thingTemplate, team)`: the object is in the team from its
        // creation (the player above is the team's controlling player when that is known).
        if let Some(k) = team {
            self.logic.set_object_team_key(id, k);
        }
        // `if (thingTemplate->isBridge()) TheTerrainLogic->addLandmarkBridgeToLogic(obj);`
        // (`GameLogic.cpp:1772-1777`) runs for every landmark bridge object; `addLandmarkBridgeToLogic`
        // (`TerrainLogic.cpp:1516-1527`) appends a `Bridge` built from the object itself and its
        // constructor creates the towers (`Bridge::Bridge(Object*)`, :318-424, `Bridge::createTower`
        // :125). The port's terrain rectangles come from the map objects at load, so the link is only
        // `BridgeInfo::bridgeObjectID`; it can fail (a bridge object that is not a box has no
        // rectangle) and the towers are created either way.
        if t.is_bridge {
            self.logic.link_bridge_object(id, &pos);
            self.spawn_bridge_towers(id, tower_names);
        }
        // `obj->...; for (m = getBehaviorModules) create->onBuildComplete();` (GameLogic.cpp:1946-1953): every create
        // module of a map object is built at load, with no science test, before its team is activated.
        self.logic.create_modules_on_build_complete(id);
        // `GameLogic::startNewGame`: "Since the team now has members, activate it." (`:1957`).
        if let Some(k) = team {
            self.logic.activate_team(k);
        }
        self.stats.created += 1;
    }

    /// `Bridge::Bridge(Object*)` (`Core/GameEngine/Source/GameLogic/Map/TerrainLogic.cpp:318-424`): one
    /// tower object at each corner of the landmark bridge `bridge_id`, created right after the bridge
    /// itself (their ids follow it) and linked to it (`BridgeTowerBehavior::setBridge`). The corners come
    /// from the bridge object's own box and angle (`:329-339`), not from the terrain `Bridge`, and each
    /// tower is offset outward along the bridge by its own major radius (`:380-408`). Each corner is
    /// looked up and created separately (`:388-408`): a missing `TowerObjectName*` key or an unknown
    /// tower template creates no tower there. The order is FromLeft, FromRight, ToLeft, ToRight
    /// (`BRIDGE_MAX_TOWERS`).
    fn spawn_bridge_towers(&mut self, bridge_id: ObjectId, names: &BTreeMap<String, [Option<String>; 4]>) {
        let Some((pos, angle, player, template, half_x, half_y)) = self.logic.find_object(bridge_id).map(|o| {
            (o.position, o.angle, o.player, o.template.name.clone(), o.template.geometry.major_radius, o.template.geometry.minor_radius)
        }) else {
            return;
        };
        let Some(tower) = names.get(&template) else { return };
        // `Real c = Cos(angle); Real s = Sin(angle);` and the four corners (`:333-339`).
        let (c, s) = (cos(angle), sin(angle));
        let from_left = Coord3D::new(pos.x - half_x * c - half_y * s, pos.y + half_y * c - half_x * s, pos.z);
        let to_left = Coord3D::new(pos.x + half_x * c - half_y * s, pos.y + half_y * c + half_x * s, pos.z);
        let from_right = Coord3D::new(pos.x - half_x * c + half_y * s, pos.y - half_y * c - half_x * s, pos.z);
        let to_right = Coord3D::new(pos.x + half_x * c + half_y * s, pos.y - half_y * c + half_x * s, pos.z);
        // `v = toLeft - toRight`, normalised: the outward direction for the left towers (`:368-371`).
        let (mut vx, mut vy) = (to_left.x - to_right.x, to_left.y - to_right.y);
        let len = (vx * vx + vy * vy).sqrt();
        if len > 0.0 {
            vx /= len;
            vy /= len;
        }
        for (t, mut corner) in [from_left, from_right, to_left, to_right].into_iter().enumerate() {
            // `towerTemplate = TheThingFactory->findTemplate(bridgeTemplate->getTowerObjectName(type));`
            // (`:390`): an absent name or an unknown template leaves `createTower` with no template,
            // which creates nothing (`:126-131`).
            let Some(name) = tower[t].as_deref() else { continue };
            let Some(tt) = self.data.store.find_template(name) else { continue };
            // `if (towerTemplate) offset = towerTemplate->getTemplateGeometryInfo().getMajorRadius();`
            let offset = tt.geometry.major_radius;
            match t {
                // `BRIDGE_TOWER_FROM_LEFT` / `BRIDGE_TOWER_TO_LEFT`: `pos += v * offset`.
                0 | 2 => {
                    corner.x += vx * offset;
                    corner.y += vy * offset;
                }
                // The right towers: `pos -= v * offset`.
                _ => {
                    corner.x -= vx * offset;
                    corner.y -= vy * offset;
                }
            }
            // `case BRIDGE_TOWER_FROM_LEFT` / `FROM_RIGHT`: `angle = bridge->getOrientation() + PI`;
            // the two `TO_` towers take the bridge angle itself (`:143-160`). `Thing::setOrientation`
            // stores it normalised (`Common/Thing/Thing.cpp:255`).
            let tower_angle = if t < 2 { normalize_angle(angle + PI) } else { normalize_angle(angle) };
            let id = self.logic.create_object(&tt, corner, tower_angle, player);
            self.logic.link_bridge_tower(id, bridge_id);
            self.stats.created += 1;
        }
    }

    /// `placeNetworkBuildingsForPlayer` for every skirmish slot, in slot order.
    fn place_starting_sets(&mut self, setups: &[PlayerSetup]) -> Result<()> {
        for (slot, s) in setups.iter().enumerate() {
            let data = self.data.clone();
            let pt = data.player_template(&s.faction).ok_or_else(|| anyhow!("unknown player template `{}`", s.faction))?;
            if pt.is_observer {
                continue;
            }
            let player = self.player_by_name(&format!("player{slot}")).ok_or_else(|| anyhow!("player{slot} missing"))?.index;
            let start = self.waypoint(&format!("Player_{}_Start", u32::from(s.start_position) + 1)).cloned();
            let Some(start) = start else {
                bail!("map has no waypoint Player_{}_Start", u32::from(s.start_position) + 1);
            };
            let rally = self.waypoint(&format!("Player_{}_Rally", u32::from(s.start_position) + 1)).cloned();
            if pt.starting_building.is_empty() {
                continue;
            }
            let mut pos = Coord3D::new(start.pos.x, start.pos.y, self.logic.terrain().ground_height(start.pos.x, start.pos.y));
            let Some((bid, building_radius)) = self.place_object(player, &pt.starting_building, pos, "StartingBuilding") else {
                continue;
            };
            // `Player::onStructureConstructionComplete(nullptr, conYard, FALSE)` (GameLogic.cpp:564).
            self.logic.score_structure_completed(bid);
            pos.y -= building_radius / 2.0;
            pos.z = self.logic.terrain().ground_height(pos.x, pos.y);
            if let Some(r) = &rally {
                pos = Coord3D::new(r.pos.x, r.pos.y, self.logic.terrain().ground_height(r.pos.x, r.pos.y));
            }
            let _ = bid;
            for (i, unit) in pt.starting_units.iter().enumerate() {
                // `tryPosition` rejects water / cliff / obstacle cells for this locomotor's
                // surfaces (`Pathfinder::can_place`; `PartitionManager::tryPosition`, PartitionManager.cpp:3795).
                let surfaces = self
                    .data
                    .store
                    .find_template(unit)
                    .and_then(|t| t.locomotor)
                    .map_or(zr_sim::pathfind::surface::GROUND, |l| zr_sim::pathfind::PathObject::surfaces_for(&l));
                // `ThePartitionManager->update()` (GameLogic.cpp:588): the starting building must be in the cells.
                self.logic.update_partition();
                let Some(found) = self.find_position_around(&pos, building_radius * 0.7, building_radius * 1.3, surfaces) else {
                    continue; // "Could not find position"
                };
                if let Some((uid, _)) = self.place_object(player, unit, found, &format!("StartingUnit{i}")) {
                    // `placeObjectAtPosition`'s `adjustDestination` step (GameLogic.cpp:522-527) runs before
                    // `Player::onUnitCreated(nullptr, unit)` (GameLogic.cpp:592).
                    self.logic.adjust_placed_unit(uid);
                    self.logic.score_unit_created(uid);
                }
            }
        }
        // `ThePartitionManager->update()` after the starting sets (GameLogic.cpp:2132-2135, in `tryStartNewGame`):
        // the placed objects get their cells and their contacts are processed before the first frame.
        self.logic.update_partition();
        Ok(())
    }

    /// `AISkirmishPlayer::newMap` + `adjustBuildList`: build an AI config for each AI slot from the
    /// `AIData` `SkirmishBuildList` for its side (rotated to the player's start position) and the
    /// map's skirmish teams (or a per-side default attack group).
    fn setup_ai_players(&mut self, map: &MapFile, setups: &[PlayerSetup]) {
        let Some(ai_data) = self.data.templates().ai_data.clone() else { return };
        for (slot, s) in setups.iter().enumerate() {
            if !s.is_ai {
                continue;
            }
            let Some(info) = self.player_by_name(&format!("player{slot}")) else { continue };
            let (idx, side) = (info.index, info.side.clone());
            let Some(build) = ai_data.build_lists.iter().find(|b| b.faction == side) else { continue };
            let start_pos = self
                .spawned
                .iter()
                .find(|sp| sp.player == idx && sp.role == "StartingBuilding")
                .map_or(Coord3D::ZERO, |sp| sp.pos);
            let base = build
                .structures
                .iter()
                .find(|e| self.is_command_center(&e.template_name))
                .map(|e| e.location)
                .unwrap_or((start_pos.x, start_pos.y));
            let angle = self.skirmish_base_angle(&start_pos);
            let mut build_list: Vec<AiBuildOrder> =
                build.structures.iter().map(|e| self.adjust_build_order(e, base, &start_pos, angle)).collect();
            // The shipped `SkirmishBuildList`s omit the supply center: retail skirmish scripts add it
            // with `AI_PLAYER_BUILD_SUPPLY_CENTER` (`AISkirmishPlayer::buildBySupplies`). Only the
            // phase-1 fallback (no scripts) inserts it synthetically, after the command center.
            let supply_center = supply_center_template(&side);
            if !self.logic.scripts().script_driven && !build_list.iter().any(|o| self.is_supply_center(&o.template)) {
                let supply_order = AiBuildOrder {
                    template: supply_center.clone(),
                    location: rotate_about((base.0 + 220.0, base.1), base, &start_pos, angle),
                    angle: 0.0,
                    initially_built: false,
                    rebuilds: -1,
                    automatically_build: true,
                };
                build_list.insert(1.min(build_list.len()), supply_order);
            }
            let side_info = ai_data.side_infos.iter().find(|si| si.side == side);
            // `ResourceGatherersEasy/Normal/Hard` (`AI::parseSideInfo`), selected by the slot's
            // difficulty; fall back to Normal when the chosen field is unset.
            let resource_gatherers = side_info.map_or(2, |si| {
                let v = match s.difficulty {
                    AiDifficulty::Easy => si.resource_gatherers_easy,
                    AiDifficulty::Normal => si.resource_gatherers_normal,
                    AiDifficulty::Hard => si.resource_gatherers_hard,
                };
                if v > 0 { v } else { si.resource_gatherers_normal.max(1) }
            });
            let attack_units = self.ai_attack_units(map, &side);
            let config = SkirmishAiConfig {
                gatherer_template: gatherer_template(&side).to_owned(),
                base_defense: side_info.map(|si| si.base_defense_structure1.clone()).filter(|s| !s.is_empty()),
                gatherer_factory: supply_center,
                attack_factory: {
                    let f = factory_name(&build_list, "WarFactory");
                    if f.is_empty() {
                        factory_name(&build_list, "Barracks")
                    } else {
                        f
                    }
                },
                attack_units,
                resource_gatherers,
                build_list,
                structure_seconds: (ai_data.structure_seconds.max(0.0) * 30.0) as u32,
                team_seconds: (ai_data.team_seconds.max(0.0) * 30.0) as u32,
                rebuild_delay_seconds: ai_data.rebuild_delay_seconds.max(0) as u32 * 30,
                resources_wealthy: ai_data.resources_wealthy,
                resources_poor: ai_data.resources_poor,
                structures_wealthy_mod: ai_data.structures_wealthy_mod,
                structures_poor_mod: ai_data.structures_poor_mod,
                team_wealthy_mod: ai_data.team_wealthy_mod,
                team_poor_mod: ai_data.team_poor_mod,
                team_resources_to_build: ai_data.team_resources_to_build,
                supply_center_safe_radius: ai_data
                    .extra
                    .get("SupplyCenterSafeRadius")
                    .and_then(|s| s.trim().parse::<f32>().ok())
                    .unwrap_or(250.0),
                max_recruit_distance: ai_data
                    .extra
                    .get("MaxRecruitRadius")
                    .and_then(|s| s.trim().parse::<f32>().ok())
                    .unwrap_or(0.0),
                // `AIPlayer::doUpgradesAndSkills` buys from the side's `SkillSet1..5`.
                skill_sets: side_info.map(|si| si.skill_sets.clone()).unwrap_or_default(),
                base_defense_extra_distance: ai_data
                    .extra
                    .get("SkirmishBaseDefenseExtraDistance")
                    .and_then(|s| s.trim().parse::<f32>().ok())
                    .unwrap_or(0.0),
                difficulty: to_script_difficulty(s.difficulty),
            };
            self.logic.add_ai_player(idx, config);
        }
    }

    fn is_command_center(&self, name: &str) -> bool {
        self.data.templates().thing(name).is_some_and(|t| t.is_kind_of(KindOf::Commandcenter))
    }

    /// The base rotation angle from `adjustBuildList`'s 3x3 grid, plus its constant `3*PI/4`. The grid
    /// rotation applies only with `RotateSkirmishBases` (`AISkirmishPlayer.cpp:1000`, `if
    /// (TheAI->getAiData()->m_rotateSkirmishBases)`); otherwise the angle is the constant alone.
    fn skirmish_base_angle(&self, start_pos: &Coord3D) -> f32 {
        let rotate = self.data.templates().ai_data.as_ref().is_some_and(|a| a.rotate_skirmish_bases);
        if !rotate {
            return 3.0 * std::f32::consts::FRAC_PI_4;
        }
        let (w, h) = self.logic.terrain().world_size();
        let mut grid = 0;
        if start_pos.x > w / 3.0 {
            grid += 1;
        }
        if start_pos.x > 2.0 * w / 3.0 {
            grid += 1;
        }
        if start_pos.y > h / 3.0 {
            grid += 3;
        }
        if start_pos.y > 2.0 * h / 3.0 {
            grid += 3;
        }
        let grid_angle = match grid {
            1 => std::f32::consts::FRAC_PI_4,
            2 => std::f32::consts::FRAC_PI_2,
            3 => -std::f32::consts::FRAC_PI_4,
            5 => 3.0 * std::f32::consts::FRAC_PI_4,
            6 => -std::f32::consts::FRAC_PI_2,
            7 => -3.0 * std::f32::consts::FRAC_PI_4,
            8 => std::f32::consts::PI,
            _ => 0.0,
        };
        grid_angle + 3.0 * std::f32::consts::FRAC_PI_4
    }

    /// Rotate one build-list entry around the build-list base into the player's base; the command
    /// center becomes `InitiallyBuilt` (the skirmish start already placed it).
    fn adjust_build_order(&self, e: &BuildListEntry, base: (f32, f32), start: &Coord3D, angle: f32) -> AiBuildOrder {
        let is_cc = self.is_command_center(&e.template_name);
        AiBuildOrder {
            template: e.template_name.clone(),
            location: rotate_about(e.location, base, start, angle),
            angle: e.angle,
            initially_built: e.initially_built || is_cc,
            rebuilds: e.rebuilds,
            automatically_build: e.automatically_build,
        }
    }

    fn is_supply_center(&self, name: &str) -> bool {
        self.data
            .store
            .find_template(name)
            .is_some_and(|t| t.dock.as_ref().is_some_and(|d| d.kind == zr_sim::supply::DockKind::Center))
    }

    /// The skirmish teams owned by `side` (map `SidesList` `teamUnitTypeN` entries), else a default.
    fn ai_attack_units(&self, map: &MapFile, side: &str) -> Vec<String> {
        let mut units = Vec::new();
        if let Some(sides) = &map.sides {
            for team in &sides.teams {
                if dict_str(team, "teamOwner") != side {
                    continue;
                }
                for i in 1..=7 {
                    let t = dict_str(team, &format!("teamUnitType{i}"));
                    if !t.is_empty() {
                        units.push(t.to_owned());
                    }
                }
            }
        }
        if units.is_empty() {
            units = match side {
                "America" => vec!["AmericaTankCrusader".into(), "AmericaVehicleHumvee".into(), "AmericaInfantryRanger".into()],
                "China" => vec!["ChinaTankBattleMaster".into(), "ChinaInfantryRedguard".into(), "ChinaTankGattling".into()],
                _ => vec!["GLAVehicleScorpion".into(), "GLAInfantryRebel".into()],
            };
        }
        units.truncate(6);
        units
    }

    /// `placeObjectAtPosition`: returns the id and the template's bounding *sphere* radius.
    fn place_object(&mut self, player: PlayerIndex, template: &str, pos: Coord3D, role: &str) -> Option<(ObjectId, f32)> {
        let thing = self.data.templates().thing(template)?;
        let (angle, radius) = (thing.placement_view_angle, thing.geometry.bounding_sphere_radius);
        let t = self.data.store.find_template(template)?;
        let id = self.logic.create_object(&t, pos, angle, player);
        // `placeObjectAtPosition` (GameLogic.cpp:504-512): the starting buildings and units get `onBuildComplete` for
        // every create module, with no science test, before their team is activated.
        self.logic.create_modules_on_build_complete(id);
        let at = self.logic.find_object(id).map_or(pos, |o| o.position);
        self.spawned.push(Spawned { player, id, template: template.to_owned(), pos: at, role: role.to_owned() });
        Some((id, radius))
    }

    /// `PartitionManager::findPositionAround`: ring search around `center` for a spot that passes
    /// `tryPosition` (terrain/water/obstacle via the pathfinder, overlap via the partition).
    fn find_position_around(&mut self, center: &Coord3D, min_r: f32, max_r: f32, surfaces: u32) -> Option<Coord3D> {
        let (w, h) = self.logic.terrain().world_size();
        if center.x < 0.0 || center.y < 0.0 || center.x > w || center.y > h {
            return Some(*center); // off-map: scripted setup, use the centre
        }
        let start_angle = self.logic.rng_mut().value_real(0.0, TWO_PI); // RANDOM_START_ANGLE
        let mut dist = min_r;
        while dist <= max_r {
            let spacing = if dist == min_r { TWO_PI } else { (RING_SPACING / (dist + 1.0)) * (TWO_PI / 6.0) };
            let samples = real_to_int_ceil((TWO_PI / spacing) / 2.0);
            for i in 0..samples {
                let step = spacing * i as f32;
                if let Some(p) = self.try_position(center, dist, start_angle + step, surfaces) {
                    return Some(p);
                }
                if i != 0 {
                    if let Some(p) = self.try_position(center, dist, start_angle - step, surfaces) {
                        return Some(p);
                    }
                }
            }
            dist += RING_SPACING;
        }
        None
    }

    /// `PartitionManager::tryPosition`: ground z, then the pathfinder's cliff/water/obstacle test,
    /// then a radius-5 sphere against existing objects.
    fn try_position(&self, center: &Coord3D, dist: f32, angle: f32, surfaces: u32) -> Option<Coord3D> {
        let (x, y) = (dist * cos(angle) + center.x, dist * sin(angle) + center.y);
        let pos = Coord3D::new(x, y, self.logic.terrain().ground_height(x, y));
        if !self.logic.pathfinder().can_place(surfaces, &pos) {
            return None;
        }
        self.logic.small_sphere_clear_at(&pos, angle, &[]).then_some(pos)
    }

    // ---- client-facing API -----------------------------------------------------------------

    pub fn data(&self) -> &Arc<GameData> {
        &self.data
    }
    /// `TheGameText`: main strings plus this map's `map.str`.
    pub fn text(&self) -> &zr_text::GameText {
        &self.data.text
    }
    /// Localised display name of a template (`DisplayName` label -> string), e.g. `AmericaTankCrusader` -> `Crusader`.
    pub fn display_name(&self, template: &str) -> Option<&str> {
        self.data.display_name(template)
    }
    /// Read access to the simulation (terrain, partition, `find_object`).
    pub fn logic(&self) -> &GameLogic {
        &self.logic
    }
    /// Mutable access to the simulation (example/test helpers such as `start_combat_drop`).
    pub fn logic_mut(&mut self) -> &mut GameLogic {
        &mut self.logic
    }
    /// Spawn a live object from a template name (example/test helper; production uses `QueueUnit`).
    pub fn spawn_object(&mut self, template: &str, pos: Coord3D, angle: f32, player: PlayerIndex) -> Option<ObjectId> {
        let t = self.data.store.find_template(template)?;
        Some(self.logic.create_object(&t, pos, angle, player))
    }
    /// `Player::setPlayerRelationship` (symmetric): mark two players as enemies.
    pub fn set_enemies(&mut self, a: PlayerIndex, b: PlayerIndex) {
        self.logic.set_enemies(a, b);
    }
    /// Registers the skirmish slots as victory players (`VictoryConditions`). [`new_skirmish_with`]
    /// does this while loading the shared skirmish scripts; the viewer builds via [`from_map`] and
    /// calls this so `check_for_game_end` works.
    pub fn enable_skirmish_victory(&mut self) {
        let skirmish: Vec<PlayerIndex> =
            self.players.iter().filter(|p| p.kind == PlayerKind::Skirmish).map(|p| p.index).collect();
        self.logic.set_victory_players(skirmish);
    }
    pub fn map_name(&self) -> &str {
        &self.map_name
    }
    pub fn frame(&self) -> u32 {
        self.logic.frame()
    }
    pub fn players(&self) -> &[PlayerInfo] {
        &self.players
    }
    pub fn player(&self, index: PlayerIndex) -> Option<&PlayerInfo> {
        self.players.get(usize::from(index))
    }
    /// Current cash (`Money::countMoney`) from the simulation; `None` for an unknown player.
    pub fn player_money(&self, index: PlayerIndex) -> Option<u32> {
        self.logic.player(index).map(|p| p.money)
    }
    pub fn player_by_name(&self, name: &str) -> Option<&PlayerInfo> {
        self.players.iter().find(|p| p.name == name)
    }
    /// The player in skirmish slot `slot` (`player<slot>`).
    pub fn skirmish_player(&self, slot: usize) -> Option<&PlayerInfo> {
        self.player_by_name(&format!("player{slot}"))
    }
    pub fn waypoints(&self) -> &[Waypoint] {
        &self.waypoints
    }
    /// `TerrainLogic::getWaypointByName` (exact match).
    pub fn waypoint(&self, name: &str) -> Option<&Waypoint> {
        self.waypoints.iter().find(|w| w.name == name)
    }
    /// What the start-up placed for each player's starting building and units.
    pub fn spawned(&self) -> &[Spawned] {
        &self.spawned
    }
    pub fn map_load_stats(&self) -> &MapLoadStats {
        &self.stats
    }

    /// Live objects in id order.
    pub fn objects(&self) -> impl Iterator<Item = ObjectView<'_>> {
        self.logic.objects().map(view)
    }
    pub fn object(&self, id: ObjectId) -> Option<ObjectView<'_>> {
        self.logic.find_object(id).map(view)
    }
    /// Read-only projectile snapshots for client rendering.
    pub fn projectiles(&self) -> Vec<zr_sim::ProjectileSnapshot> {
        self.logic.projectiles()
    }
    /// Read-only projectile-stream snapshots (`W3DProjectileStreamDraw` input).
    pub fn projectile_streams(&self) -> Vec<zr_sim::ProjectileStreamSnapshot> {
        self.logic.projectile_streams()
    }
    /// Read-only laser beam records (`W3DLaserDraw` input; the client resolves the source bone).
    pub fn lasers(&self) -> Vec<zr_sim::LaserBeam> {
        self.logic.lasers()
    }
    /// Bounding-circle radius of a live object's geometry (for picking).
    pub fn object_radius(&self, id: ObjectId) -> Option<f32> {
        self.logic.find_object(id).map(|o| o.template.geometry.bounding_circle_radius())
    }

    /// Whether a map entry becomes a logic object at start-up (so a client should draw it from the
    /// simulation instead of statically). Mirrors the filters of the map-object pass.
    pub fn is_logic_map_object(&self, o: &MapObject) -> bool {
        if o.props.str("waypointName").is_some() || o.flags & (FLAG_ROAD_FLAGS | FLAG_BRIDGE_FLAGS) != 0 {
            return false;
        }
        let name = match o.template.as_str() {
            "AmericaTankLeopard" => "AmericaTankCrusader",
            "AmericaVehicleHumVee" => "AmericaVehicleHumvee",
            n => n,
        };
        let Some(thing) = self.data.templates().thing(name) else { return false };
        !(thing.is_kind_of(KindOf::OptimizedTree) || thing.is_kind_of(KindOf::Prop)) && self.data.store.find_template(name).is_some()
    }

    /// Live object count per owning player index.
    pub fn object_counts_by_owner(&self) -> BTreeMap<PlayerIndex, usize> {
        let mut m = BTreeMap::new();
        for o in self.logic.objects() {
            *m.entry(o.player).or_insert(0) += 1;
        }
        m
    }

    /// Queue a logic message (applied at the start of the next [`step`](Self::step)). When a replay
    /// is recording, the command is appended at the current frame (`Recorder::onGameMessage`).
    pub fn submit(&mut self, player: PlayerIndex, command: Command) {
        if let Some(rec) = &mut self.recorder {
            rec.record(self.logic.frame(), player, command.clone());
        }
        self.logic.submit(player, command);
    }

    /// Starts recording a replay (`Recorder::startRecording`). Every later [`submit`](Self::submit)
    /// is appended; call [`record_crc`](Self::record_crc) periodically.
    pub fn start_recording(&mut self, header: crate::replay::ReplayHeader, crc_interval: u32) {
        self.recorder = Some(crate::replay::Replay::new(header, crc_interval));
    }

    /// Records a state CRC at the current frame when recording (`Recorder`'s periodic checkpoint).
    pub fn record_crc(&mut self) {
        if self.recorder.is_none() {
            return;
        }
        let (frame, crc) = (self.logic.frame(), self.crc());
        if let Some(rec) = &mut self.recorder {
            rec.record_crc(frame, crc);
        }
    }

    /// Whether a replay is recording.
    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// The CRC interval of the active recording.
    pub fn recording_crc_interval(&self) -> u32 {
        self.recorder.as_ref().map_or(30, |r| r.crc_interval)
    }

    /// Ends recording and returns the replay.
    pub fn take_recording(&mut self) -> Option<crate::replay::Replay> {
        self.recorder.take()
    }
    /// Spawn `template` for `player` at `pos` facing `angle` (`ThingFactory::newObject`; used by
    /// examples and tests, and available to the client for scripted spawns).
    pub fn spawn_unit(&mut self, player: PlayerIndex, template: &str, pos: Coord3D, angle: f32) -> Option<ObjectId> {
        let t = self.data.store.find_template(template)?;
        Some(self.logic.create_object(&t, pos, angle, player))
    }
    /// Advance one 30 Hz logic frame.
    pub fn step(&mut self) {
        self.logic.update();
    }
    /// Apply explosive damage to `id` (examples/tests; drives `Bridge::updateDamageState`).
    pub fn damage_object(&mut self, id: ObjectId, amount: f32) -> f32 {
        self.logic.damage_object(
            id,
            zr_sim::body::DamageInfo { source: ObjectId::INVALID, amount, kind: zr_sim::body::DamageType::Explosion },
        )
    }
    /// The bridge object ids the terrain knows about (`BridgeInfo::bridgeObjectID`).
    pub fn bridge_object_ids(&self) -> Vec<ObjectId> {
        self.logic.terrain().bridges().iter().filter_map(|b| b.bridge_object).collect()
    }
    /// Whether terrain bridge `index` is destroyed (`Bridge::getDamageState() == BODY_RUBBLE`).
    pub fn bridge_destroyed(&self, index: usize) -> Option<bool> {
        self.logic.terrain().bridges().get(index).map(|b| b.destroyed)
    }
    pub fn run_frames(&mut self, n: u32) {
        self.logic.run_frames(n);
    }
    /// Hash of all simulation state plus player money (`GameLogic::getCRC`).
    pub fn crc(&self) -> u32 {
        let mut c = Crc::new();
        c.add_u32(self.logic.crc());
        c.add_u32(self.players.len() as u32);
        for p in &self.players {
            c.add_u32(u32::from(p.index));
            c.add_u32(p.money);
        }
        c.get()
    }
}

/// `populateRandomSideAndColor` (GameLogic.cpp:702-790), colour half: each slot without a usable colour
/// draws `GameLogicRandomValue(0, numColors-1)`, redrawn while another slot holds it. Only the draws are
/// kept: the port's team colour for such a slot stays the faction's preferred colour (display only).
fn draw_random_colors(rng: &mut zr_sim::random::GameRandom, setups: &[PlayerSetup], num_colors: i32) {
    if num_colors <= 0 {
        return;
    }
    let mut colors: Vec<i32> = setups.iter().map(|s| s.color).collect();
    for i in 0..colors.len() {
        if (0..num_colors).contains(&colors[i]) {
            continue;
        }
        loop {
            let c = rng.value(0, num_colors - 1);
            if colors.contains(&c) {
                continue;
            }
            colors[i] = c;
            break;
        }
    }
}

fn next_index(players: &[PlayerInfo]) -> Result<PlayerIndex> {
    // MAX_PLAYER_COUNT is 16 in the original.
    if players.len() >= 16 {
        bail!("too many players (max 16 including neutral, civilians and the observer)");
    }
    Ok(players.len() as PlayerIndex)
}

/// `Alpine Assault` -> `maps\alpine assault\alpine assault.map`; full paths pass through.
/// The supply gatherer a side uses (`AISkirmishPlayer::queueSupplyTruck`). GLA's worker is both its
/// dozer and its resource gatherer (there is no separate GLA supply truck).
/// `GameInfo` slot difficulty -> the script engine's `GameDifficulty`.
fn to_script_difficulty(d: AiDifficulty) -> zr_sim::script::Difficulty {
    match d {
        AiDifficulty::Easy => zr_sim::script::Difficulty::Easy,
        AiDifficulty::Normal => zr_sim::script::Difficulty::Normal,
        AiDifficulty::Hard => zr_sim::script::Difficulty::Hard,
    }
}

fn gatherer_template(side: &str) -> &'static str {
    match side {
        "America" => "AmericaVehicleChinook",
        "China" => "ChinaVehicleSupplyTruck",
        _ => "GLAInfantryWorker",
    }
}

/// The first build-list structure whose template name contains `needle` (its factory for a unit).
fn factory_name(build_list: &[AiBuildOrder], needle: &str) -> String {
    build_list
        .iter()
        .find(|o| o.template.contains(needle))
        .map_or_else(String::new, |o| o.template.clone())
}

/// The `SkirmishScripts.scb` `ScriptsPlayers` name for a faction (`FactionChina` -> `SkirmishChina`).
fn skirmish_script_name(faction: &str) -> String {
    format!("Skirmish{}", faction.strip_prefix("Faction").unwrap_or(faction))
}

/// The supply center a side needs to build a war factory and gather resources.
fn supply_center_template(side: &str) -> String {
    match side {
        "America" => "AmericaSupplyCenter",
        "China" => "ChinaSupplyCenter",
        _ => "GLASupplyStash",
    }
    .to_owned()
}

/// `adjustBuildList`'s rotation: move `world` (in the build list's frame) around `base` into the
/// player's base at `start`, by `angle`.
fn rotate_about(world: (f32, f32), base: (f32, f32), start: &Coord3D, angle: f32) -> (f32, f32) {
    let (s, c) = (sin(angle), cos(angle));
    let (dx, dy) = (world.0 - base.0, world.1 - base.1);
    (start.x + dx * c - dy * s, start.y + dy * c + dx * s)
}

/// The map's waypoint links as the engine stores them (`TerrainLogic::addWaypointLink`,
/// TerrainLogic.cpp:1334): each `WaypointsList` pair links `id1` to `id2`, and a bidirectional `id1`
/// (`waypointPathBiDirectional`) links back too. A link that already exists adds nothing.
fn directed_waypoint_links(map: &MapFile) -> Vec<(i32, i32)> {
    let bidirectional: std::collections::BTreeSet<i32> = map
        .objects
        .iter()
        .filter(|o| o.props.str("waypointName").is_some())
        .filter(|o| matches!(o.props.get("waypointPathBiDirectional"), Some(zr_map::DictValue::Bool(true))))
        .filter_map(|o| o.props.int("waypointID"))
        .collect();
    let mut out: Vec<(i32, i32)> = Vec::new();
    for &(a, b) in &map.waypoint_links {
        if a == b || out.contains(&(a, b)) {
            continue;
        }
        out.push((a, b));
        if bidirectional.contains(&a) && !out.contains(&(b, a)) {
            out.push((b, a));
        }
    }
    out
}

pub fn map_path(map_name: &str) -> String {
    let n = normalize_path(map_name);
    if n.ends_with(".map") {
        n
    } else {
        format!("maps\\{n}\\{n}.map")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zr_ini::IniDb;
    use zr_map::{Dict, DictValue, HeightMap, MapObject};

    /// Two tower templates and a landmark bridge with the default (non-box) geometry: the port's
    /// `object_bridges` builds no terrain rectangle for such an object, which is the case where the
    /// towers used to be skipped.
    const INI: &str = "
Object TestTowerA
  KindOf = IMMOBILE
  Geometry = BOX
  GeometryMajorRadius = 6
  GeometryMinorRadius = 2
  GeometryHeight = 4
End
Object TestTowerD
  KindOf = IMMOBILE
  Geometry = BOX
  GeometryMajorRadius = 6
  GeometryMinorRadius = 2
  GeometryHeight = 4
End
Object TestBridgeSphere
  KindOf = IMMOBILE
  IsBridge = Yes
  Geometry = SPHERE
  GeometryMajorRadius = 40
End
PlayerTemplate FactionTest
  Side = Test
  StartMoney = 0
End
";

    fn data() -> Arc<GameData> {
        let mut db = IniDb::new();
        db.extend(zr_ini::parse("t.ini", INI).expect("test INI parses"));
        Arc::new(GameData::from_ini(&db))
    }

    fn map() -> MapFile {
        let mut m = MapFile::default();
        let (w, h) = (60usize, 60usize);
        m.height_map = Some(HeightMap { version: 4, width: w as i32, height: h as i32, border_size: 0, boundaries: vec![], data: vec![16; w * h] });
        m.objects = vec![MapObject {
            version: 3,
            pos: [300.0, 300.0, 0.0],
            angle: 0.0,
            flags: 0,
            template: "*Waypoints/Waypoint".to_owned(),
            props: Dict(vec![
                ("waypointName".to_owned(), DictValue::Ascii("Player_1_Start".to_owned())),
                ("waypointID".to_owned(), DictValue::Int(1)),
            ]),
        }];
        m
    }

    /// `Bridge::Bridge(Object*)` (`TerrainLogic.cpp:388-408`) creates a landmark bridge's towers one
    /// corner at a time, from that corner's own `TowerObjectName*` lookup; a missing key or an
    /// unknown tower template creates nothing for that corner. The port used to require all four
    /// keys and a terrain `Bridge` rectangle containing the object. `Bridge::createTower` gives the
    /// two `FROM_` towers the bridge orientation + PI and the two `TO_` towers the bridge
    /// orientation (`:143-160`), stored normalised (`Thing::setOrientation`).
    #[test]
    fn bridge_towers_are_created_per_corner_without_a_terrain_rectangle() {
        let mut game = Game::from_map(data(), &map(), "synthetic", &[PlayerSetup::new("FactionTest", 0)], 7).unwrap();
        let mut names: BTreeMap<String, [Option<String>; 4]> = BTreeMap::new();
        names.insert(
            "TestBridgeSphere".to_owned(),
            [Some("TestTowerA".to_owned()), None, Some("NoSuchTower".to_owned()), Some("TestTowerD".to_owned())],
        );
        let o = MapObject {
            version: 3,
            pos: [300.0, 800.0, 0.0],
            angle: 0.5,
            flags: 0,
            template: "TestBridgeSphere".to_owned(),
            props: Dict(vec![("originalOwner".to_owned(), DictValue::Ascii("team".to_owned()))]),
        };
        game.create_map_object(&o, "TestBridgeSphere", 0, None, &names);
        let l = game.logic();
        let bridge = l.objects().find(|o| o.template.name == "TestBridgeSphere").expect("the bridge object is created");
        assert!(
            !l.terrain().bridges().iter().any(|b| b.bridge_object == Some(bridge.id)),
            "a non-box bridge object has no terrain rectangle in the port"
        );
        let got: Vec<_> =
            l.objects().filter(|o| o.bridge == Some(bridge.id)).map(|o| (o.id.0 - bridge.id.0, o.template.name.as_str(), o.angle)).collect();
        let flipped = normalize_angle(0.5 + PI);
        assert_eq!(
            got,
            vec![(1u32, "TestTowerA", flipped), (2u32, "TestTowerD", 0.5)],
            "FromLeft and ToRight, in BRIDGE_MAX_TOWERS order, each created right after the bridge"
        );
    }

    fn side(name: &str, faction: &str) -> zr_map::Side {
        zr_map::Side {
            dict: Dict(vec![
                ("playerName".to_owned(), DictValue::Ascii(name.to_owned())),
                ("playerFaction".to_owned(), DictValue::Ascii(faction.to_owned())),
            ]),
            build_list: Vec::new(),
        }
    }

    fn sides(list: Vec<zr_map::Side>) -> zr_map::Sides {
        zr_map::Sides { version: 3, sides: list, teams: Vec::new(), nested: Vec::new(), scripts: None, script_error: None }
    }

    /// `SidesList::validateSides` gives every side that exists its `"team" + playerName` default team
    /// (Core/SidesList.cpp:750-784) and `GameLogic::startNewGame` adds `teamReplayObserver`
    /// (GameLogic.cpp:1543-1548). A civilian side with another `playerName` gets its own team; a map
    /// without a civilian side gets no `teamPlyrCivilian`.
    #[test]
    fn default_teams_follow_the_sides_that_exist() {
        let mut m = map();
        m.sides = Some(sides(vec![side("", ""), side("PlyrTown", "FactionCivilian")]));
        let mut game = Game::from_map(data(), &m, "synthetic", &[PlayerSetup::new("FactionTest", 0)], 7).unwrap();
        game.install_script_lists(Vec::new(), &[], false);
        let s = game.logic().scripts();
        assert!(s.team_protos.contains_key("team"), "the neutral side's default team");
        assert!(s.team_protos.contains_key("teamplyrtown"), "the civilian side's own playerName");
        assert!(s.teams.get("teamplyrtown").is_some_and(|t| t.created), "and Player::setDefaultTeam activates it");
        assert!(s.team_protos.contains_key("teamreplayobserver"), "the replay observer's team is unconditional");
        assert!(!s.team_protos.contains_key("teamplyrcivilian"), "no team for a side the map does not have");
        assert!(!s.teams.contains_key("teamplyrcivilian"), "and no instance either");
        // A human slot is a side `startNewGame` added with playerName `playerN`; its `teamplayerN`
        // survives `Player::initFromDict` (only AI slots lose theirs, Player.cpp:917-926).
        assert!(s.team_protos.contains_key("teamplayer0"), "the human slot's default team");
        assert_eq!(s.player_default_team(2), Some("teamplayer0"), "and Player::setDefaultTeam finds it");

        // No civilian side at all: the neutral side and the observer keep their teams.
        let mut m = map();
        m.sides = Some(sides(vec![side("", "")]));
        let mut game = Game::from_map(data(), &m, "synthetic", &[PlayerSetup::new("FactionTest", 0)], 7).unwrap();
        game.install_script_lists(Vec::new(), &[], false);
        let s = game.logic().scripts();
        assert!(s.team_protos.contains_key("team"));
        assert!(s.team_protos.contains_key("teamreplayobserver"));
        assert!(!s.team_protos.contains_key("teamplyrcivilian"), "a map without a civilian side has no civilian default team");
        assert!(!s.teams.contains_key("teamplyrcivilian"));
    }

    /// GAME_SHELL runs the same `startNewGame` block (GameLogic.cpp:1543-1557), so every named shell
    /// side gets its default team as well, not only the skirmish game's map sides.
    #[test]
    fn shell_install_gives_every_named_side_its_default_team() {
        let mut m = map();
        m.sides = Some(sides(vec![side("", ""), side("PlyrTest", "FactionTest")]));
        let mut game =
            Game::from_map_inner(data(), &m, "synthetic", &[], 7, Vec::new(), true, None).unwrap();
        game.install_script_lists(Vec::new(), &[], true);
        let s = game.logic().scripts();
        assert!(s.team_protos.contains_key("teamplyrtest"), "the shell side's default team");
        assert!(s.teams.get("teamplyrtest").is_some_and(|t| t.created));
        assert!(s.teams.get("team").is_some_and(|t| t.created), "the neutral side's default team");
        assert!(s.teams.get("teamreplayobserver").is_some_and(|t| t.created));
    }
}
