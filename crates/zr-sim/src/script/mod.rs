//! Map script engine, phase 1.
//!
//! Ported from `GameLogic/ScriptEngine/ScriptEngine.cpp` (`update`, `executeScript`,
//! `executeScripts`, `evaluateConditions`, `executeActions`, counters/flags/timers,
//! `enableScript`/`disableScript`/`callSubroutine`), `ScriptConditions.cpp` and
//! `ScriptActions.cpp`. The script tree itself is decoded by `zr-map` (`PlayerScriptsList` /
//! `.scb`); this module runs it against [`GameLogic`].
//!
//! Semantics reproduced:
//! - per-side script lists, run in side order with that side's player as the implicit
//!   "this player" (`m_currentPlayer`);
//! - free scripts then active, non-subroutine groups (`executeScripts`), in list order;
//! - `active`, `one-shot`, `easy`/`normal`/`hard` flags and `EvaluationInterval` (delay seconds);
//! - conditions as OR-of-ANDs with short-circuit; actions run on true, false-actions on false;
//! - counters (also countdown timers, decremented once per frame), flags, named timers;
//! - `ENABLE_SCRIPT` / `DISABLE_SCRIPT` (by script or group name) and `CALL_SUBROUTINE`.
//!
//! Not ported (no-op with a counted warning): most conditions/actions. See
//! [`ScriptEngine::warnings`]; the count per name is reported by the demos.

mod actions;
mod command_button;
mod conditions;
mod reinforce;
mod sequential;
mod team;
mod unit_actions;

pub use sequential::SequentialScript;

pub use team::{try_to_recruit, TeamInstance, TeamPrototype, TeamState, TeamUnitInfo};

use std::collections::BTreeMap;

use zr_map::scripts::{Parameter, Script, ScriptGroup, ScriptList};

use crate::crc::Crc;
use crate::ai_states::CommandSource;
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::{ObjectId, PlayerIndex};

/// `"<This Player>"`: the script engine's token for `m_currentPlayer`.
pub const THIS_PLAYER: &str = "<This Player>";
/// `"None"` used for empty team/script references.
pub const NONE_STRING: &str = "None";

/// Comparison operators (`Parameter::LESS_THAN` .. `NOT_EQUAL`), stored in an int parameter.
pub mod comparison {
    pub const LESS_THAN: i32 = 0;
    pub const LESS_EQUAL: i32 = 1;
    pub const EQUAL: i32 = 2;
    pub const GREATER_EQUAL: i32 = 3;
    pub const GREATER: i32 = 4;
    pub const NOT_EQUAL: i32 = 5;

    /// `a <op> b` for the stored operator.
    pub fn apply(op: i32, a: i32, b: i32) -> bool {
        match op {
            LESS_THAN => a < b,
            LESS_EQUAL => a <= b,
            EQUAL => a == b,
            GREATER_EQUAL => a >= b,
            GREATER => a > b,
            NOT_EQUAL => a != b,
            _ => false,
        }
    }

    /// `a <op> b` for reals (`PLAYER_POWER_COMPARE_PERCENT`).
    pub fn apply_f(op: i32, a: f32, b: f32) -> bool {
        match op {
            LESS_THAN => a < b,
            LESS_EQUAL => a <= b,
            EQUAL => a == b,
            GREATER_EQUAL => a >= b,
            GREATER => a > b,
            NOT_EQUAL => a != b,
            _ => false,
        }
    }
}

/// `GameDifficulty` (`getGlobalDifficulty` / `Player::getPlayerDifficulty`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Difficulty {
    Easy,
    #[default]
    Normal,
    Hard,
}

impl Difficulty {
    /// Parses `"easy"` / `"normal"` / `"hard"` (the skirmish difficulty setting).
    pub fn from_token(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "easy" => Difficulty::Easy,
            "hard" => Difficulty::Hard,
            _ => Difficulty::Normal,
        }
    }
}

/// A counter or countdown timer (`m_counters` entry).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counter {
    pub value: i32,
    /// `isCountdownTimer`: decremented once per frame while `>= 0`.
    pub countdown: bool,
}

/// A script's runtime state plus its parsed definition.
#[derive(Clone, Debug)]
pub struct ScriptState {
    pub def: Script,
    /// `Script::isActive`, mutable via `ENABLE_SCRIPT` / `DISABLE_SCRIPT` / one-shot.
    pub active: bool,
    /// `Script::m_frameToEvaluate`: `EvaluationInterval` gate.
    pub frame_to_evaluate: u32,
}

impl ScriptState {
    fn new(def: Script) -> Self {
        let active = def.active;
        ScriptState { def, active, frame_to_evaluate: 0 }
    }
}

/// A script group's runtime state.
#[derive(Clone, Debug)]
pub struct GroupState {
    pub def: ScriptGroup,
    pub active: bool,
    pub scripts: Vec<ScriptState>,
}

/// One side's list of scripts (`ScriptList`), bound to a player.
#[derive(Clone, Debug)]
pub struct ScriptListState {
    pub player: PlayerIndex,
    pub scripts: Vec<ScriptState>,
    pub groups: Vec<GroupState>,
}

impl ScriptListState {
    fn from_list(player: PlayerIndex, list: &ScriptList) -> Self {
        ScriptListState {
            player,
            scripts: list.scripts.iter().cloned().map(ScriptState::new).collect(),
            groups: list
                .groups
                .iter()
                .cloned()
                .map(|g| GroupState { active: g.active, scripts: g.scripts.iter().cloned().map(ScriptState::new).collect(), def: g })
                .collect(),
        }
    }
}

/// `AttackPriorityInfo` (`ScriptEngine.cpp:141`): a named priority set. A template without an entry
/// has the set's default priority (initially `ATTACK_PRIORITY_DEFAULT` = 1); priority 0 means
/// "never attack".
#[derive(Clone, Debug, PartialEq)]
pub struct AttackPriorityInfo {
    pub default: i32,
    /// Lower-case template name -> priority.
    pub map: BTreeMap<String, i32>,
}

impl Default for AttackPriorityInfo {
    fn default() -> Self {
        AttackPriorityInfo { default: 1, map: BTreeMap::new() }
    }
}

impl AttackPriorityInfo {
    /// `AttackPriorityInfo::getPriority`.
    pub fn priority(&self, template: &str) -> i32 {
        self.map.get(&template.to_ascii_lowercase()).copied().unwrap_or(self.default)
    }
}

/// A `Waypoint` with its path labels and position (`TerrainLogic::Waypoint`), for the
/// `SKIRMISH_*_APPROACH_PATH` actions (`getClosestWaypointOnPath`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaypointInfo {
    pub name: String,
    pub pos: Coord3D,
    /// `m_pathLabel1..3`: the approach paths this waypoint belongs to.
    pub labels: Vec<String>,
}

/// What `ScriptActions::doTeamHuntWithCommandButton` (`ScriptActions.cpp:2073`) needs from a
/// `CommandButton`: the command type, and for `SPECIAL_POWER` the power and whether the button
/// takes an object target. Built by the game loader from the INI (the sim does not parse buttons).
#[derive(Clone, Debug, PartialEq)]
pub enum CommandButtonHunt {
    /// `GUI_COMMAND_FIRE_WEAPON` / `GUI_COMMAND_SWITCH_WEAPON` (`CommandButtonHuntUpdate::huntWeapon`).
    Weapon,
    /// `GUI_COMMAND_SPECIAL_POWER` with `COMMAND_OPTION_NEED_OBJECT_TARGET`.
    SpecialPower { power: String },
    /// `GUICOMMANDMODE_HIJACK_VEHICLE` / `CONVERT_TO_CARBOMB` / `SABOTAGE_BUILDING`.
    Enter(crate::enter_actions::EnterKind),
    /// Any other command type, or a special power without an object target: refused
    /// (`ScriptActions.cpp:2090`, `:2104`).
    Refused,
}

/// The `CommandButton` fields the team command-button actions need (`ControlBar.h`): the
/// `SPECIAL_POWER` it fires and its target options. Built by the game loader for every special-power
/// button. A button without a special power has no entry.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandButtonPower {
    /// `getSpecialPowerTemplate()->getName()`.
    pub power: String,
    /// `COMMAND_OPTION_NEED_OBJECT_TARGET`: any of `NEED_TARGET_{ENEMY,NEUTRAL,ALLY}_OBJECT`.
    pub need_object_target: bool,
    /// `NEED_TARGET_POS`.
    pub need_target_pos: bool,
}

/// A `PLAYER_UPGRADE` / `OBJECT_UPGRADE` `CommandButton`: the upgrade it queues (`getUpgradeTemplate`) and whether
/// that upgrade is an object upgrade (`UPGRADE_TYPE_OBJECT`, given to the producing object) rather than a player one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandButtonUpgrade {
    pub upgrade: String,
    pub object: bool,
}

/// The target options of a `CommandButton` (`ControlBar.h` `CommandOption`): which object relationships a
/// `FIRE_WEAPON` button may target, whether it takes a position, and whether it attacks the object's position
/// instead (`ATTACK_OBJECTS_POSITION`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FireTargets {
    /// `NEED_TARGET_ENEMY_OBJECT`.
    pub enemy: bool,
    /// `NEED_TARGET_NEUTRAL_OBJECT`.
    pub neutral: bool,
    /// `NEED_TARGET_ALLY_OBJECT`.
    pub ally: bool,
    /// `NEED_TARGET_POS`.
    pub position: bool,
    /// `ATTACK_OBJECTS_POSITION`.
    pub attack_objects_position: bool,
}

impl FireTargets {
    /// `BitIsSet(options, COMMAND_OPTION_NEED_OBJECT_TARGET)`: any of the three relationship bits.
    pub fn needs_object_target(&self) -> bool {
        self.enemy || self.neutral || self.ally
    }
}

/// The `CommandButton` command types that `Object::doCommandButton` (Object.cpp:5504-5630) runs for each team member,
/// with the fields each branch reads. The game loader builds the table for every such button; a button with no entry
/// (and no power or upgrade) does nothing, as the engine's default branch does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandButtonAction {
    /// `GUI_COMMAND_STOP`: `aiIdle(cmdSource)` (Object.cpp:5522-5525), for an object with an AI.
    Stop,
    /// `GUI_COMMAND_SWITCH_WEAPON`: `setWeaponLock(slot, LOCKED_PERMANENTLY)` (Object.cpp:5530-5534). `slot` is the
    /// `WeaponSlotType` index (PRIMARY 0, SECONDARY 1, TERTIARY 2).
    SwitchWeapon { slot: usize },
    /// `GUI_COMMAND_FIRE_WEAPON` (Object.cpp:5538-5548): with an AI and no object or position target, lock `slot`
    /// temporarily and `aiAttackPosition(own position, maxShots)`. The target options (`targets`) pick the
    /// object- and position-target branches (`doCommandButtonAtObject` / `AtPosition`).
    FireWeapon { slot: usize, max_shots: i32, targets: FireTargets },
    /// `GUI_COMMAND_UNIT_BUILD` (Object.cpp:5577-5583): `queueCreateUnit(thing)` for an object with a production interface.
    UnitBuild { thing: String },
    /// `GUI_COMMAND_DOZER_CONSTRUCT`: the same `queueCreateUnit` as `UnitBuild` on a member (Object.cpp:5577-5583), but at
    /// a position it is `TheBuildAssistant->buildObjectNow` (Object.cpp:5794-5795), which the sim does not port.
    DozerConstruct { thing: String },
    /// `GUI_COMMAND_HACK_INTERNET` (Object.cpp:5587-5593): `aiHackInternet(cmdSource)` for an object with an AI.
    HackInternet,
    /// `GUI_COMMAND_SELL` (Object.cpp:5596-5598): `BuildAssistant::sellObject(this)`.
    Sell,
}

/// The script engine singleton (`TheScriptEngine`).
#[derive(Clone, Debug, Default)]
pub struct ScriptEngine {
    /// Per-side script lists, in side order.
    pub lists: Vec<ScriptListState>,
    pub flags: BTreeMap<String, bool>,
    pub counters: BTreeMap<String, Counter>,
    pub difficulty: Difficulty,
    /// Per-player difficulty (`Player::getPlayerDifficulty`); falls back to `difficulty`.
    pub player_difficulty: BTreeMap<PlayerIndex, Difficulty>,
    /// True once real scripts (map or `.scb`) are loaded: the skirmish AI then only builds what
    /// scripts ask for, instead of the phase-1 whole-list fallback.
    pub script_driven: bool,
    /// Unimplemented condition/action name -> invocation count.
    pub warnings: BTreeMap<String, u64>,
    /// Diagnostics (not hashed): `SKIRMISH_FIRE_SPECIAL_POWER_AT_MOST_COST` power-name -> count.
    pub special_power_action_counts: BTreeMap<String, u64>,
    pub conditions_evaluated: u64,
    /// `SKIRMISH_TECH_BUILDING_WITHIN_DISTANCE` results, kept for good by key (side, group, script, OR term,
    /// AND term): C++ stores the first result on the condition and returns it from then on
    /// (`ScriptConditions.cpp:2259`, `setCustomData`, no frame check).
    pub sticky_conditions: BTreeMap<(usize, Option<usize>, usize, usize, usize), bool>,
    pub actions_executed: u64,
    /// `getUnitNamed`: script-named objects (map-placed or created by scripts).
    pub named_objects: BTreeMap<String, ObjectId>,
    /// `OBJECTLIST_ADDOBJECTTYPE`: named object-type lists.
    pub object_lists: BTreeMap<String, Vec<String>>,
    /// `ScriptTeams` definitions, by lower-case name.
    pub team_protos: BTreeMap<String, TeamPrototype>,
    /// The prototypes' lower-case names in load order: `Player::m_playerTeamPrototypes` is filled
    /// in `TeamPrototype` construction order (`Player::addTeamToList`, `Team.cpp:823`), and the AI
    /// walks that list (`AIPlayer::selectTeamToBuild`, `AIPlayer.cpp:1680`).
    pub team_order: Vec<String>,
    /// `CommandButton` definitions the script actions need (`TEAM_HUNT_WITH_COMMAND_BUTTON`),
    /// by lower-case name. Installed by the game loader from the INI.
    pub command_buttons: BTreeMap<String, CommandButtonHunt>,
    /// The special-power command buttons (`CommandButtonPower`), by lower-case name, for the team
    /// command-button actions (`command_button.rs`).
    pub command_button_powers: BTreeMap<String, CommandButtonPower>,
    /// The `PLAYER_UPGRADE` / `OBJECT_UPGRADE` command buttons, by lower-case button name. Team command-button
    /// actions reach these through `Object::doCommandButton` (`command_button.rs`).
    pub command_button_upgrades: BTreeMap<String, CommandButtonUpgrade>,
    /// The other team-usable command buttons (`CommandButtonAction`), by lower-case name.
    pub command_button_actions: BTreeMap<String, CommandButtonAction>,
    /// Live teams, by lower-case name.
    pub teams: BTreeMap<String, TeamInstance>,
    /// `PolygonTrigger`s by name, for the trigger-area conditions/actions.
    pub trigger_areas: BTreeMap<String, zr_map::PolygonTrigger>,
    /// `SET_ATTACK_PRIORITY_*`: priority set name -> object type -> priority.
    pub attack_priorities: BTreeMap<String, AttackPriorityInfo>,
    /// Map waypoints by name, for `TEAM_*`/`NAMED_*` waypoint orders.
    pub waypoints: BTreeMap<String, Coord3D>,
    /// Map waypoints by id with their path labels (`getClosestWaypointOnPath`).
    pub waypoint_by_id: BTreeMap<i32, WaypointInfo>,
    /// Waypoint links (id -> linked ids), from the map's `WaypointsList`.
    pub waypoint_links: BTreeMap<i32, Vec<i32>>,
    /// `SET_ATTACK_PRIORITY_KIND_OF`: `KindOf` index -> template names, set by the adapter.
    pub kind_of_templates: BTreeMap<i32, Vec<String>>,
    /// Player side names (`Player::getSide`), set by the adapter.
    pub player_sides: BTreeMap<PlayerIndex, String>,
    /// Player start positions (`Player::getMpStartIndex`, 0-based), set by the adapter.
    pub player_start: BTreeMap<PlayerIndex, i32>,
    /// Player enemies (`Player::getCurrentEnemy`), for `<This Player's Enemy>`.
    pub player_enemy: BTreeMap<PlayerIndex, PlayerIndex>,
    /// Player names (`SkirmishChina`, `player0`, ...) -> index, for `teamOwner` resolution.
    pub player_names: BTreeMap<String, PlayerIndex>,
    /// `Player::m_defaultTeam`: each player's default team key (`Player::setDefaultTeam`).
    pub player_default_team: BTreeMap<PlayerIndex, String>,
    /// `m_triggeredSpecialPowers`: `(power, source)` events for `PLAYER_TRIGGERED_SPECIAL_POWER`.
    pub triggered_special_powers: BTreeMap<PlayerIndex, Vec<(String, ObjectId)>>,
    /// `m_finishedSpecialPowers`: `(power, source)` events for `PLAYER_COMPLETED_SPECIAL_POWER`.
    pub completed_special_powers: BTreeMap<PlayerIndex, Vec<(String, ObjectId)>>,
    /// `m_completedUpgrades`: `(upgrade, producing object)` events for `PLAYER_BUILT_UPGRADE` and
    /// `PLAYER_BUILT_UPGRADE_FROM_NAMED` (`ScriptEngine::notifyOfCompletedUpgrade`, ScriptEngine.cpp:7244).
    pub completed_upgrades: BTreeMap<PlayerIndex, Vec<(String, ObjectId)>>,
    current_player: Option<PlayerIndex>,
    /// `m_callingTeam`: the team whose script is running, for `<This Team>` (a unique instance key).
    current_team: Option<String>,
    /// `TeamFactory::m_uniqueTeamID`: the next `Team` instance id.
    next_team_id: u32,
    /// `m_sequentialScripts`: one queue (head + `m_nextScriptInSequence` chain) per team/unit.
    sequential: Vec<std::collections::VecDeque<SequentialScript>>,
    next_sequential_id: u32,
}

impl ScriptEngine {
    /// `newMap` + `ScriptEngine::update`'s `m_firstUpdate`: install the parsed script lists,
    /// one entry per player, in side order. Empty input leaves `script_driven` false.
    pub fn load(&mut self, entries: Vec<(PlayerIndex, ScriptList)>) {
        self.lists = entries.into_iter().map(|(p, l)| ScriptListState::from_list(p, &l)).collect();
        self.script_driven = self.lists.iter().any(|l| !l.scripts.is_empty() || !l.groups.is_empty());
    }

    /// Installs `ScriptTeams` / map-team definitions.
    pub fn load_teams(&mut self, teams: &[zr_map::Dict]) {
        for d in teams {
            if let Some(p) = TeamPrototype::from_dict(d) {
                let key = p.name.to_ascii_lowercase();
                if !self.team_protos.contains_key(&key) {
                    self.team_order.push(key.clone());
                }
                self.team_protos.insert(key, p);
            }
        }
    }

    /// The default team prototypes the sides list always has before the players' own teams:
    /// `SidesList::validateSides` adds `"team" + player name` for every side without one
    /// (SidesList.cpp:750-784; a missing neutral side is added by `addPlayerByTemplate`,
    /// SidesList.cpp:707-725) and `GameLogic::startNewGame` adds `teamReplayObserver`
    /// (GameLogic.cpp:1543-1548). They are registered before the qualified skirmish teams so
    /// `TeamFactory::initFromSides` creates their instances first (Team.cpp:217-241).
    pub fn load_default_team_protos(&mut self, teams: &[(&str, &str)]) {
        for (name, owner) in teams {
            let d = zr_map::Dict(vec![
                ("teamName".to_owned(), zr_map::DictValue::Ascii((*name).to_owned())),
                ("teamOwner".to_owned(), zr_map::DictValue::Ascii((*owner).to_owned())),
                ("teamIsSingleton".to_owned(), zr_map::DictValue::Bool(true)),
            ]);
            let Some(p) = TeamPrototype::from_dict(&d) else { continue };
            let key = p.name.to_ascii_lowercase();
            // `validateSides` keeps a team that is already there (and only checks its owner and
            // singleton flag, SidesList.cpp:752-772); the shell map's own team dicts must keep their
            // scripts.
            if self.team_protos.contains_key(&key) {
                continue;
            }
            self.team_order.push(key.clone());
            self.team_protos.insert(key, p);
        }
    }

    /// `PlayerList::newGame` -> `TeamFactory::initFromSides` (Team.cpp:217-241): create one inactive
    /// instance of every singleton prototype in prototype order, so instance ids follow
    /// `m_uniqueTeamID` (`++m_uniqueTeamID`, Team.cpp:353). Then `Player::setDefaultTeam`
    /// (Player.cpp:760-770) activates each player's default team (`Team::setActive`, Team.h:309).
    /// `defaults` is `(team name, player index)` in player order.
    pub fn init_from_sides(&mut self, defaults: &[(&str, PlayerIndex)]) {
        let singletons: Vec<String> =
            self.team_order.iter().filter(|k| self.team_protos.get(*k).is_some_and(|p| p.is_singleton)).cloned().collect();
        for key in singletons {
            if self.teams.contains_key(&key) {
                continue;
            }
            self.next_team_id += 1;
            let mut t = TeamInstance::new(&key);
            t.seq = self.next_team_id;
            // `TeamPrototype::TeamPrototype`'s owning player (Team.cpp:800-812).
            t.owner = self.team_owner(&key);
            self.teams.insert(key, t);
        }
        for (name, player) in defaults {
            let key = name.to_ascii_lowercase();
            if let Some(t) = self.teams.get_mut(&key) {
                t.created = true;
                t.owner = Some(*player);
                self.player_default_team.insert(*player, key);
            }
        }
    }

    /// The player's default team (`Player::m_defaultTeam`, set by `Player::setDefaultTeam`).
    pub fn player_default_team(&self, player: PlayerIndex) -> Option<&str> {
        self.player_default_team.get(&player).map(String::as_str)
    }

    // ---- object ./ team membership ---------------------------------------------------------

    /// `Team::getControllingPlayer()` (`Team.cpp:1386`: the prototype's owning player). Read from
    /// the instance, so it is fixed at creation and does not depend on the running script.
    pub(crate) fn team_player(&self, key: &str) -> Option<PlayerIndex> {
        self.teams.get(key).and_then(|t| t.owner).or_else(|| self.team_owner(key))
    }

    /// `TeamFactory::findTeam` (`Team.cpp:393-406`): the prototype's first instance (newest first,
    /// `Team.cpp:1328`), or a new inactive instance for a non-singleton prototype. A singleton with
    /// no instance has no team (`findTeam` returns null).
    pub fn find_team(&mut self, name: &str) -> Option<String> {
        let proto = name.to_ascii_lowercase();
        let is_singleton = self.team_protos.get(&proto)?.is_singleton;
        if let Some(k) = self.first_instance(&proto) {
            return Some(k);
        }
        if !is_singleton {
            return self.create_team(&proto);
        }
        None
    }

    /// `PlayerList::validateTeam` (`PlayerList.cpp:294-308`): a team by name, else the neutral
    /// player's default team (the engine also DEBUG_CRASHes). Used for a map object's
    /// `originalOwner` (`GameLogic.cpp:1923-1924`).
    pub fn validate_team(&mut self, owner: &str, neutral_default: Option<&str>) -> Option<String> {
        if let Some(k) = self.find_team(owner) {
            return Some(k);
        }
        neutral_default.map(str::to_owned).or_else(|| {
            // No default-team table (tests): look for the prototype of `team`.
            self.find_team("team")
        })
    }

    /// `Object::setTeam(Team*)` (`Object.cpp:851-860`) for the team instance `key`, including the
    /// tenant switch of `setOrRestoreTeam` (`Object.cpp:872-948`):
    ///
    /// - leave the old team's member list (`removeFrom_TeamMemberList`, `:885`),
    /// - the object's controlling player is the new team's (`Object::getControllingPlayer`,
    ///   `Object.cpp:1620-1627`),
    /// - *prepend* to the new team's list (`:898`), so DLINK iteration runs newest first,
    /// - take the team prototype's initial attitude and attack-priority set (`:902-918`),
    /// - `onCapture(oldTeam->getControllingPlayer(), newTeam->getControllingPlayer())` (`:928`).
    ///
    /// An inactive (dead) player's team is replaced by the neutral player's default team (`:855`).
    pub(crate) fn set_object_team(&mut self, logic: &mut GameLogic, id: ObjectId, key: &str) {
        // In order to prevent spawning useful units for a player after he dies, we just assign
        // objects to the neutral player if we try to misbehave (`Object.cpp:853-856`).
        let mut key = key.to_ascii_lowercase();
        if self.team_player(&key).is_some_and(|p| !logic.is_player_active(p)) {
            if let Some(neutral) = logic.neutral_player().and_then(|n| self.player_default_team(n).map(str::to_owned)) {
                key = neutral;
            }
        }
        let old = logic.find_object(id).and_then(|o| o.team.clone());
        if old.as_deref() == Some(key.as_str()) {
            return; // don't do anything if the team hasn't changed
        }
        let old_player = old.as_deref().and_then(|k| self.team_player(k));
        if let Some(old) = &old {
            if let Some(t) = self.teams.get_mut(old) {
                t.members.retain(|m| *m != id);
            }
        }
        let new_player = self.team_player(&key);
        if let Some(o) = logic.objects.get_mut(&id) {
            o.team = Some(key.clone());
        }
        self.register_object_in_team(logic, id, &key);
        // `onCapture` runs after the switch whenever there was an old team (`Object.cpp:928-929`).
        if let (Some(a), Some(b)) = (old_player, new_player) {
            if a != b {
                logic.on_object_captured(id, a, b);
            }
        }
    }

    /// `Object::setOrRestoreTeam`'s "after switch" half (`Object.cpp:891-923`) for an object whose
    /// `team` already holds `key`: prepend it to the team's member list (`:898`), make it take the
    /// team's controlling player (`Object::getControllingPlayer`, `:1620`) and its prototype's
    /// initial attitude and attack-priority set (`:902-918`).
    pub(crate) fn register_object_in_team(&mut self, logic: &mut GameLogic, id: ObjectId, key: &str) {
        let new_player = self.team_player(key);
        if let Some(t) = self.teams.get_mut(key) {
            if !t.members.contains(&id) {
                t.members.insert(0, id);
            }
        }
        if let Some(p) = new_player {
            if let Some(o) = logic.objects.get_mut(&id) {
                o.player = p;
            }
        }
        let (attitude, attack) = self.team_proto(key).map_or((0, None), |p| (p.initial_attitude, self.team_attack_info(p)));
        logic.set_unit_attitude(id, attitude);
        logic.set_unit_attack_info(id, attack);
    }

    /// `Object::setTemporaryTeam` (`Object.cpp:864`): the same switch without the
    /// `m_originalTeamName` bookkeeping (which the sim does not keep).
    pub fn set_object_temporary_team(&mut self, logic: &mut GameLogic, id: ObjectId, key: &str) {
        self.set_object_team(logic, id, key);
    }

    /// `~Object` -> `setTeam(nullptr)` (`Object.cpp:623`): the object leaves its team's member list.
    pub(crate) fn remove_object_from_team(&mut self, key: &str, id: ObjectId) {
        if let Some(t) = self.teams.get_mut(key) {
            t.members.retain(|m| *m != id);
        }
    }

    /// `Team::setActive` (`Team.h:309`) only: `m_created`/`m_active` are set, the create script runs
    /// later in `ScriptEngine::update`'s team pass (`Team::updateState`, `Team.cpp:1812-1819`). This
    /// is the path of `AIPlayer::checkReadyTeams` (`AIPlayer.cpp:2851`), which runs after the frame's
    /// `ScriptEngine::UPDATE`.
    pub(crate) fn mark_team_active(&mut self, key: &str) {
        if let Some(t) = self.teams.get_mut(key) {
            if !t.created {
                t.created = true;
                t.created_pending = true;
            }
        }
    }

    /// Installs the `CommandButton` hunt table (lower-cased on insert).
    pub fn load_command_buttons(&mut self, buttons: impl IntoIterator<Item = (String, CommandButtonHunt)>) {
        self.command_buttons = buttons.into_iter().map(|(n, b)| (n.to_ascii_lowercase(), b)).collect();
    }

    /// Installs the special-power command-button table (lower-cased on insert).
    pub fn load_command_button_powers(&mut self, buttons: impl IntoIterator<Item = (String, CommandButtonPower)>) {
        self.command_button_powers = buttons.into_iter().map(|(n, b)| (n.to_ascii_lowercase(), b)).collect();
    }

    /// Installs the upgrade command-button table (lower-cased on insert).
    pub fn load_command_button_upgrades(&mut self, buttons: impl IntoIterator<Item = (String, CommandButtonUpgrade)>) {
        self.command_button_upgrades = buttons.into_iter().map(|(n, b)| (n.to_ascii_lowercase(), b)).collect();
    }

    /// Installs the other team-usable command-button table (lower-cased on insert).
    pub fn load_command_button_actions(&mut self, buttons: impl IntoIterator<Item = (String, CommandButtonAction)>) {
        self.command_button_actions = buttons.into_iter().map(|(n, b)| (n.to_ascii_lowercase(), b)).collect();
    }

    /// Installs the map's polygon triggers (for the trigger-area conditions/actions).
    pub fn set_triggers(&mut self, triggers: &zr_map::PolygonTriggers) {
        for t in &triggers.triggers {
            if t.is_valid() {
                self.trigger_areas.insert(t.name.clone(), t.clone());
            }
        }
    }

    pub fn set_difficulty(&mut self, d: Difficulty) {
        self.difficulty = d;
    }

    /// `Player::setAIDifficulty` (skirmish slot difficulty), used by the Easy/Normal/Hard script
    /// and team production-condition flags.
    pub fn set_player_difficulty(&mut self, player: PlayerIndex, d: Difficulty) {
        self.player_difficulty.insert(player, d);
    }

    /// Registers a player's side name and 0-based start position (for the skirmish conditions).
    pub fn set_player_meta(&mut self, player: PlayerIndex, side: &str, start: i32) {
        self.player_sides.insert(player, side.to_owned());
        self.player_start.insert(player, start);
    }

    /// Resolves a script player parameter. `<This Player's Enemy>` -> the current player's enemy;
    /// anything else (`<This Player>`, empty, names) -> the current player.
    pub(crate) fn player_from_param(&self, name: &str) -> Option<PlayerIndex> {
        if name == "<This Player's Enemy>" {
            return self.current_player.and_then(|p| self.player_enemy.get(&p).copied());
        }
        self.current_player
    }

    /// Resolves a team parameter: `<This Team>` -> the calling team, else the first live instance of
    /// the named prototype (`ScriptEngine::getTeamNamed`). A bare prototype name with no live
    /// instance resolves to itself (so `create_team` can still be reached).
    pub(crate) fn resolve_team(&self, name: &str) -> String {
        if name == "<This Team>" {
            return self.current_team.clone().unwrap_or_default();
        }
        let key = name.to_ascii_lowercase();
        if self.teams.contains_key(&key) {
            return key;
        }
        self.first_instance(&key).unwrap_or(key)
    }

    /// The prototype name behind a team key (unique `proto#id` keys and bare proto keys both work).
    pub(crate) fn proto_of(&self, key: &str) -> String {
        self.teams.get(key).map(|t| t.proto.clone()).unwrap_or_else(|| key.to_ascii_lowercase())
    }

    /// `Team::getPrototype()->getName()` (`Team.h:246`) for a team key: the prototype name behind a
    /// `proto#id` instance key (a bare prototype name is its own prototype). The snapshot's `T key`
    /// prints this — `TeamInQueue::m_team->getPrototype()->getName()` — not the instance key, and a
    /// supply-truck entry's `m_team` is the player's default team (`AIPlayer.cpp:409`).
    pub fn team_proto_name(&self, key: &str) -> String {
        self.proto_of(key)
    }

    /// Live instance keys of a prototype, newest-first (`TeamFactory` prepends a new `Team` to the
    /// prototype's instance list, so `iterate_TeamInstanceList`/`getFirstItemIn_TeamInstanceList`
    /// see the most recently created instance first).
    pub fn instances_of(&self, proto: &str) -> Vec<String> {
        let p = proto.to_ascii_lowercase();
        let mut v: Vec<(u32, String)> = self
            .teams
            .iter()
            .filter(|(_, t)| t.proto == p)
            .map(|(k, t)| (t.seq, k.clone()))
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        v.into_iter().map(|(_, k)| k).collect()
    }

    /// `TeamPrototype::countTeamInstances`.
    pub fn count_team_instances(&self, proto: &str) -> usize {
        self.instances_of(proto).len()
    }

    /// The first live instance key of a prototype (`TeamFactory::findTeam` / `getTeamNamed`).
    pub(crate) fn first_instance(&self, proto: &str) -> Option<String> {
        self.instances_of(proto).into_iter().next()
    }

    /// Registers a player's current enemy (`Player::getCurrentEnemy`).
    pub fn set_player_enemy(&mut self, player: PlayerIndex, enemy: PlayerIndex) {
        self.player_enemy.insert(player, enemy);
    }

    /// Registers a player name (`SkirmishChina`, `player0`, ...) for `teamOwner` resolution.
    pub fn set_player_name(&mut self, name: &str, player: PlayerIndex) {
        if !name.is_empty() {
            self.player_names.insert(name.to_ascii_lowercase(), player);
        }
    }

    /// The player whose list is currently being evaluated (`getCurrentPlayer`).
    pub fn current_player(&self) -> Option<PlayerIndex> {
        self.current_player
    }

    /// `TheScriptEngine->UPDATE()`: counters then every side's scripts, in side order.
    pub fn update(&mut self, logic: &mut GameLogic) {
        for c in self.counters.values_mut() {
            if c.countdown && c.value >= 0 {
                c.value -= 1;
            }
        }
        for side in 0..self.lists.len() {
            let player = self.lists[side].player;
            if logic.player(player).is_none() {
                continue;
            }
            self.current_player = Some(player);
            let count = self.lists[side].scripts.len();
            for i in 0..count {
                if self.lists[side].scripts[i].def.subroutine {
                    continue; // subroutine scripts only run when called.
                }
                self.execute_script(logic, side, None, i, false);
            }
            let groups = self.lists[side].groups.len();
            for gi in 0..groups {
                if !self.lists[side].groups[gi].active {
                    continue;
                }
                if self.lists[side].groups[gi].def.subroutine {
                    continue; // subroutine groups only run when called.
                }
                let count = self.lists[side].groups[gi].scripts.len();
                for si in 0..count {
                    self.execute_script(logic, side, Some(gi), si, false);
                }
            }
            self.current_player = None;
        }
        // `Team::update`: every live team runs its generic scripts with `<This Team>` bound.
        let team_names: Vec<String> = self.teams.iter().filter(|(_, t)| t.created).map(|(n, _)| n.clone()).collect();
        for name in team_names {
            let proto_name = self.proto_of(&name);
            let Some(proto) = self.team_protos.get(&proto_name).cloned() else { continue };
            self.current_player = self.team_owner(&name);
            self.current_team = Some(name.clone());
            // `Team::updateState` (`Team.cpp:1812-1819`): the create script runs once, on the first
            // update after `Team::setActive`. For a team activated by a script action this is the same
            // `ScriptEngine::UPDATE` (the side scripts above ran before this pass), for one activated
            // by `AIPlayer::checkReadyTeams` it is the next frame's.
            if self.teams.get(&name).is_some_and(|t| t.created_pending) {
                if let Some(t) = self.teams.get_mut(&name) {
                    t.created_pending = false;
                }
                if !proto.on_create.is_empty() {
                    let s = proto.on_create.clone();
                    self.run_named_script(logic, &s);
                }
                if !self.teams.contains_key(&name) {
                    // The create script merged/disbanded the team.
                    self.current_team = None;
                    self.current_player = None;
                    continue;
                }
            }
            // Prune the dead; gather whether the team sees an enemy and whether it is idle.
            let (members, sees, idle) = {
                let Some(t) = self.teams.get_mut(&name) else { continue };
                // C++ keeps a dead member in the list until the object is deleted (`~Object` ->
                // `setTeam(nullptr)`, Object.cpp:623), so there is nothing to prune here.
                let members = t.members.clone();
                // `m_checkEnemySighted` is only set when the prototype has a sighted/all-clear script.
                let sees = if proto.on_enemy_sighted.is_empty() && proto.on_all_clear.is_empty() {
                    (false, false)
                } else {
                    logic.team_sees_enemy(&members)
                };
                // `Team::updateState` (Team.cpp:1895): needs one live AI member, all of them idle
                // (`AIUpdateInterface::isIdle`: `AI_IDLE`; guard is not idle).
                let mut any_alive = false;
                let mut all_idle = true;
                for id in &members {
                    let Some(o) = logic.find_object(*id) else { continue };
                    // `Team::updateState` (Team.cpp:1895-1905) counts a member as live unless
                    // `isEffectivelyDead()` (the port's `is_dying()`).
                    if o.is_dying() || o.ai_handle.is_none() {
                        continue;
                    }
                    any_alive = true;
                    if !logic.ai_is_idle(o) {
                        all_idle = false;
                    }
                }
                (members, sees, any_alive && all_idle)
            };
            // OnDestroyed (once, when the last member dies).
            if members.is_empty() {
                if self.teams.get(&name).is_some_and(|t| t.created && !t.destroyed_run) {
                    if let Some(t) = self.teams.get_mut(&name) {
                        t.destroyed_run = true;
                    }
                    if !proto.on_destroyed.is_empty() {
                        let s = proto.on_destroyed.clone();
                        self.run_named_script(logic, &s);
                    }
                }
                // `TeamPrototype::updateState` (Team.cpp:1054-1088): remove empty *active* teams so
                // `countTeamInstances()` drops and the AI can rebuild a wiped team. Singleton teams
                // are never removed (the player's default team is not modelled: unassigned units do
                // not live in a team here).
                if !proto.is_singleton {
                    self.teams.remove(&name);
                    self.notify_of_team_destruction(&name); // `Team::~Team`
                }
                self.current_team = None;
                self.current_player = None;
                continue;
            }
            // OnEnemySighted / OnAllClear (`m_prevSeeEnemy` / `m_seeEnemy`): the latch follows the
            // scan every frame, the scripts fire on a change while someone in the team is alive.
            if !proto.on_enemy_sighted.is_empty() || !proto.on_all_clear.is_empty() {
                let (any_alive, sees_enemy) = sees;
                let prev = self.teams.get(&name).is_some_and(|t| t.enemy_sighted);
                if let Some(t) = self.teams.get_mut(&name) {
                    t.enemy_sighted = sees_enemy;
                }
                if any_alive && prev != sees_enemy {
                    let s = if sees_enemy { proto.on_enemy_sighted.clone() } else { proto.on_all_clear.clone() };
                    if !s.is_empty() {
                        self.run_named_script(logic, &s);
                    }
                }
            }
            // OnIdle: every frame the team is idle for the second frame running (`m_wasIdle`);
            // `teamInitialIdleFrames` is not used here (only `TeamInQueue::isStale`). `m_wasIdle` is
            // only tracked while the team has an OnIdle script.
            if !proto.on_idle.is_empty() {
                let was_idle = self.teams.get(&name).is_some_and(|t| t.was_idle);
                if idle && was_idle {
                    let s = proto.on_idle.clone();
                    self.run_named_script(logic, &s);
                }
                if let Some(t) = self.teams.get_mut(&name) {
                    t.was_idle = idle;
                }
            }
            self.current_team = None;
            self.current_player = None;
        }
        // `ScriptEngine::update` tail: progress every sequential script.
        self.evaluate_and_progress_all_sequential_scripts(logic);
        // `Player::update` -> `Team::updateGenericScripts` for every instance (active or still being
        // built) of the player's team prototypes.
        let all_teams: Vec<String> = self.teams.keys().cloned().collect();
        for name in all_teams {
            self.update_generic_scripts(logic, &name);
        }
    }

    /// `Team::updateGenericScripts` (Team.cpp:2530): each hook script whose conditions hold runs its
    /// *actions* with `<This Team>` bound. It ignores the script's active/easy/normal/hard/delay
    /// settings (`evaluateConditions(script, team)` + `friend_executeAction`), and a one-shot hook is
    /// retired for this team instance only (`m_shouldAttemptGenericScript`).
    fn update_generic_scripts(&mut self, logic: &mut GameLogic, key: &str) {
        let proto_name = self.proto_of(key);
        let hooks = match self.team_protos.get(&proto_name) {
            Some(p) if !p.generic_scripts.is_empty() => p.generic_scripts.clone(),
            _ => return,
        };
        let owner = self.team_owner(key);
        let saved_player = self.current_player;
        let saved_team = self.current_team.take();
        for (i, hook) in hooks.iter().enumerate().take(32) {
            let Some(t) = self.teams.get(key) else { break };
            if t.generic_done & (1 << i) != 0 {
                continue;
            }
            let Some((side, group, index)) = self.find_script_by_name(hook) else {
                if let Some(t) = self.teams.get_mut(key) {
                    t.generic_done |= 1 << i;
                }
                continue;
            };
            self.current_player = owner;
            self.current_team = Some(key.to_owned());
            if !self.evaluate_conditions(logic, side, group, index) {
                continue;
            }
            let (one_shot, actions) = {
                let s = self.script_state(side, group, index);
                (s.def.one_shot, s.def.actions.clone())
            };
            if one_shot {
                if let Some(t) = self.teams.get_mut(key) {
                    t.generic_done |= 1 << i;
                }
            }
            for a in &actions {
                self.execute_action(logic, a);
            }
        }
        self.current_team = saved_team;
        self.current_player = saved_player;
    }

    /// `ScriptEngine::findScript` (ScriptEngine.cpp:6306): the first free script, then group script,
    /// named `name`, searching every side in order.
    fn find_script_by_name(&self, name: &str) -> Option<(usize, Option<usize>, usize)> {
        for side in 0..self.lists.len() {
            if let Some(i) = self.lists[side].scripts.iter().position(|s| s.def.name == name) {
                return Some((side, None, i));
            }
            for (gi, g) in self.lists[side].groups.iter().enumerate() {
                if let Some(i) = g.scripts.iter().position(|s| s.def.name == name) {
                    return Some((side, Some(gi), i));
                }
            }
        }
        None
    }

    /// `ScriptEngine::checkConditionsForTeamNames` (`ScriptEngine.cpp:6885`), run for every script in
    /// side order by `ScriptEngine::newMap` (`ScriptEngine.cpp:5465-5478`: a side's own scripts, then
    /// its groups' scripts). A script with a delay starts at a random offset,
    /// `GameLogicRandomValue(0, 2*LOGICFRAMES_PER_SECOND)` (`ScriptEngine.cpp:6892`); the others at 0.
    pub(crate) fn draw_first_evaluation_frames(&mut self, rng: &mut crate::random::GameRandom) {
        for list in &mut self.lists {
            let scripts = list.scripts.iter_mut().chain(list.groups.iter_mut().flat_map(|g| g.scripts.iter_mut()));
            for s in scripts {
                s.frame_to_evaluate = if s.def.delay_seconds > 0 {
                    rng.value(0, (2 * crate::math::LOGICFRAMES_PER_SECOND) as i32) as u32
                } else {
                    0
                };
            }
        }
    }

    /// `ScriptEngine::executeScript`. `group` selects a group member, `None` a free script.
    /// `called_directly` is true for `CALL_SUBROUTINE`/`runScript`, which run subroutines.
    fn execute_script(&mut self, logic: &mut GameLogic, side: usize, group: Option<usize>, index: usize, called_directly: bool) {
        let (active, subroutine, one_shot, delay, frame_to_evaluate, easy, normal, hard) = {
            let s = self.script_state(side, group, index);
            (
                s.active,
                s.def.subroutine,
                s.def.one_shot,
                s.def.delay_seconds,
                s.frame_to_evaluate,
                s.def.easy,
                s.def.normal,
                s.def.hard,
            )
        };
        if !active || (subroutine && !called_directly) {
            return;
        }
        let player = self.lists[side].player;
        if !self.difficulty_ok(player, easy, normal, hard) {
            return;
        }
        if logic.frame() < frame_to_evaluate {
            return;
        }
        if delay > 0 {
            let next = logic.frame() + (delay.max(0) as u32) * crate::math::LOGICFRAMES_PER_SECOND;
            self.script_state_mut(side, group, index).frame_to_evaluate = next;
        }

        // Condition team (named team whose instances are tested) is not modelled; conditions that
        // reference a team test the team as a whole.
        let test = self.evaluate_conditions(logic, side, group, index);
        let (acts, false_acts) = {
            let s = self.script_state(side, group, index);
            (s.def.actions.clone(), s.def.actions_false.clone())
        };
        let run = if test { acts } else { false_acts };
        if test || !run.is_empty() {
            for a in &run {
                self.execute_action(logic, a);
            }
            if one_shot {
                self.script_state_mut(side, group, index).active = false;
            }
        }
    }

    fn difficulty_ok(&self, player: PlayerIndex, easy: bool, normal: bool, hard: bool) -> bool {
        match self.player_difficulty.get(&player).copied().unwrap_or(self.difficulty) {
            Difficulty::Easy => easy,
            Difficulty::Normal => normal,
            Difficulty::Hard => hard,
        }
    }

    fn script_state(&self, side: usize, group: Option<usize>, index: usize) -> &ScriptState {
        match group {
            Some(g) => &self.lists[side].groups[g].scripts[index],
            None => &self.lists[side].scripts[index],
        }
    }

    fn script_state_mut(&mut self, side: usize, group: Option<usize>, index: usize) -> &mut ScriptState {
        match group {
            Some(g) => &mut self.lists[side].groups[g].scripts[index],
            None => &mut self.lists[side].scripts[index],
        }
    }

    // ---- flags / counters / timers ---------------------------------------------------------

    pub fn get_flag(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }

    pub fn set_flag(&mut self, name: &str, value: bool) {
        self.flags.insert(name.to_owned(), value);
    }

    /// `ScriptEngine::clearTeamFlags` (`ScriptEngine.cpp:5781`): the skirmish "<side> Team is Building"
    /// flags, cleared when a queued team is disbanded.
    pub(crate) fn clear_team_flags(&mut self) {
        for f in [
            "USA Team is Building",
            "USA Air Team Is Building",
            "USA Inf Team Is Building",
            "China Team is Building",
            "China Air Team Is Building",
            "China Inf Team Is Building",
            "GLA Team is Building",
            "GLA Inf Team is Building",
        ] {
            self.set_flag(f, false);
        }
    }

    /// `TeamInQueue::disband` (`AIPlayer.cpp:3615-3628`): the team's members are transferred to the
    /// controlling player's default team (`Team::transferUnitsTo` -> `Object::setTeam`) and a
    /// non-singleton instance is deleted.
    pub(crate) fn disband_team(&mut self, logic: &mut GameLogic, key: &str) {
        let Some(t) = self.teams.get(key) else { return };
        let proto = t.proto.clone();
        let player = t.owner.or_else(|| self.team_owner(key));
        let members = t.members.clone();
        if let Some(dst) = player.and_then(|p| self.player_default_team(p).map(str::to_owned)) {
            if dst != key {
                for id in &members {
                    self.set_object_team(logic, *id, &dst);
                }
            }
        }
        if !self.team_protos.get(&proto).is_some_and(|p| p.is_singleton) {
            self.teams.remove(key);
            self.notify_of_team_destruction(key); // `Team::~Team`
        }
    }

    pub fn counter(&self, name: &str) -> i32 {
        self.counters.get(name).map_or(0, |c| c.value)
    }

    pub fn set_counter(&mut self, name: &str, value: i32) {
        self.counters.entry(name.to_owned()).or_default().value = value;
    }

    pub fn add_counter(&mut self, name: &str, value: i32) {
        let c = self.counters.entry(name.to_owned()).or_default();
        c.value += value;
    }

    /// `setTimer`: `seconds` is `SET_TIMER`'s int (frames) or `SET_MILLISECOND_TIMER`'s
    /// `seconds * 30` frames.
    pub fn set_timer_frames(&mut self, name: &str, frames: i32) {
        let c = self.counters.entry(name.to_owned()).or_default();
        c.value = frames;
        c.countdown = true;
    }

    pub fn stop_timer(&mut self, name: &str) {
        self.counters.entry(name.to_owned()).or_default().countdown = false;
    }

    pub fn restart_timer(&mut self, name: &str) {
        let c = self.counters.entry(name.to_owned()).or_default();
        if c.value > 0 {
            c.countdown = true;
        }
    }

    pub fn timer_expired(&self, name: &str) -> bool {
        self.counters.get(name).is_some_and(|c| c.countdown && c.value < 1)
    }

    // ---- enable / disable / subroutine -----------------------------------------------------

    /// `ENABLE_SCRIPT`: sets the script or group active, searching every side.
    pub fn enable_script(&mut self, name: &str) {
        self.set_script_active(name, true);
    }

    pub fn disable_script(&mut self, name: &str) {
        self.set_script_active(name, false);
    }

    fn set_script_active(&mut self, name: &str, active: bool) {
        for list in &mut self.lists {
            for s in &mut list.scripts {
                if s.def.name == name {
                    s.active = active;
                }
            }
            for g in &mut list.groups {
                if g.def.name == name {
                    g.active = active;
                }
                for s in &mut g.scripts {
                    if s.def.name == name {
                        s.active = active;
                    }
                }
            }
        }
    }

    /// `CALL_SUBROUTINE`: runs a subroutine script/group by name (in every side, first match).
    pub fn call_subroutine(&mut self, logic: &mut GameLogic, name: &str) {
        for side in 0..self.lists.len() {
            let groups = self.lists[side].groups.len();
            for gi in 0..groups {
                if self.lists[side].groups[gi].def.name == name && self.lists[side].groups[gi].def.subroutine {
                    if !self.lists[side].groups[gi].active {
                        return;
                    }
                    let count = self.lists[side].groups[gi].scripts.len();
                    let saved = self.current_player;
                    self.current_player = Some(self.lists[side].player);
                    for si in 0..count {
                        self.execute_script(logic, side, Some(gi), si, true);
                    }
                    self.current_player = saved;
                    return;
                }
            }
            let count = self.lists[side].scripts.len();
            for si in 0..count {
                if self.lists[side].scripts[si].def.name == name && self.lists[side].scripts[si].def.subroutine {
                    let saved = self.current_player;
                    self.current_player = Some(self.lists[side].player);
                    self.execute_script(logic, side, None, si, true);
                    self.current_player = saved;
                    return;
                }
            }
            // `ScriptEngine::findScript` (ScriptEngine.cpp:6306) also searches every group's scripts.
            for gi in 0..groups {
                let count = self.lists[side].groups[gi].scripts.len();
                for si in 0..count {
                    if self.lists[side].groups[gi].scripts[si].def.name == name
                        && self.lists[side].groups[gi].scripts[si].def.subroutine
                    {
                        let saved = self.current_player;
                        self.current_player = Some(self.lists[side].player);
                        self.execute_script(logic, side, Some(gi), si, true);
                        self.current_player = saved;
                        return;
                    }
                }
            }
        }
    }

    // ---- evaluation ------------------------------------------------------------------------

    /// `evaluateConditions`: OR of ANDs, short-circuiting.
    fn evaluate_conditions(&mut self, logic: &GameLogic, side: usize, group: Option<usize>, index: usize) -> bool {
        let ors = self.script_state(side, group, index).def.or_conditions.clone();
        for (oi, oc) in ors.iter().enumerate() {
            if oc.conditions.is_empty() {
                continue;
            }
            let mut and_term = true;
            for (ci, c) in oc.conditions.iter().enumerate() {
                self.conditions_evaluated += 1;
                let sticky = c.name() == Some("SKIRMISH_TECH_BUILDING_WITHIN_DISTANCE");
                let key = (side, group, index, oi, ci);
                let ok = match self.sticky_conditions.get(&key) {
                    Some(v) if sticky => *v,
                    _ => {
                        let v = self.evaluate_condition(logic, c);
                        if sticky {
                            self.sticky_conditions.insert(key, v);
                        }
                        v
                    }
                };
                if !ok {
                    and_term = false;
                    break;
                }
            }
            if and_term {
                return true;
            }
        }
        false
    }

    // ---- named objects / object lists ------------------------------------------------------

    pub fn named_object(&self, name: &str) -> Option<ObjectId> {
        self.named_objects.get(name).copied().filter(|id| id.0 != 0)
    }

    pub fn add_named_object(&mut self, name: &str, id: ObjectId) {
        if !name.is_empty() {
            self.named_objects.insert(name.to_owned(), id);
        }
    }

    pub fn add_object_list_type(&mut self, list: &str, type_name: &str) {
        let e = self.object_lists.entry(list.to_owned()).or_default();
        if !e.iter().any(|t| t == type_name) {
            e.push(type_name.to_owned());
        }
    }

    // ---- teams -----------------------------------------------------------------------------

    /// `TeamFactory::createInactiveTeam`: a new instance of `name`'s prototype, keyed uniquely
    /// (`proto#id`). A singleton prototype reuses its existing instance. Returns the instance key.
    pub fn create_team(&mut self, name: &str) -> Option<String> {
        let proto = name.to_ascii_lowercase();
        let is_singleton = self.team_protos.get(&proto)?.is_singleton;
        if is_singleton {
            if let Some(k) = self.first_instance(&proto) {
                return Some(k);
            }
        }
        self.next_team_id += 1;
        let key = format!("{proto}#{}", self.next_team_id);
        let mut instance = TeamInstance::new(&proto);
        instance.seq = self.next_team_id;
        // `TeamPrototype::getControllingPlayer` (Team.cpp:863): the prototype's owner.
        instance.owner = self.team_owner(&proto);
        self.teams.insert(key.clone(), instance);
        Some(key)
    }

    /// The prototype of a live team (accepts a unique instance key or a bare prototype name).
    pub fn team_proto(&self, name: &str) -> Option<&TeamPrototype> {
        let proto = self.proto_of(&name.to_ascii_lowercase());
        self.team_protos.get(&proto)
    }

    /// `Team::setActive` (`Team.h:309`): mark `name` active. The prototype's `teamOnCreateScript`
    /// runs in [`Self::update`](Self::update)'s team pass (`Team::updateState`, `Team.cpp:1812-1819`),
    /// which is at the end of `ScriptEngine::UPDATE` (`PlayerList::updateTeamStates`,
    /// ScriptEngine.cpp:5593) — the same frame for a team a script action just created, the next one
    /// for a team the AI activates later in the frame.
    pub fn on_team_created(&mut self, _logic: &mut GameLogic, name: &str) {
        let key = name.to_ascii_lowercase();
        let proto = self.proto_of(&key);
        if !self.team_protos.contains_key(&proto) {
            return;
        }
        if !self.teams.contains_key(&key) {
            self.next_team_id += 1;
            let mut instance = TeamInstance::new(&proto);
            instance.seq = self.next_team_id;
            instance.owner = self.team_owner(&proto);
            self.teams.insert(key.clone(), instance);
        }
        self.mark_team_active(&key);
    }

    /// `TeamPrototype::evaluateProductionCondition`: finds the script named by the team's
    /// `teamProductionCondition`, checks its easy/normal/hard flag against the player's difficulty,
    /// and evaluates its conditions with that player.
    pub fn evaluate_production_condition(&mut self, logic: &GameLogic, player: PlayerIndex, name: &str) -> bool {
        if name.is_empty() {
            return false;
        }
        for side in 0..self.lists.len() {
            let free = self.lists[side].scripts.len();
            for i in 0..free {
                if self.lists[side].scripts[i].def.name == name {
                    return self.production_condition_ok(logic, player, side, None, i);
                }
            }
            let groups = self.lists[side].groups.len();
            for gi in 0..groups {
                let count = self.lists[side].groups[gi].scripts.len();
                for si in 0..count {
                    if self.lists[side].groups[gi].scripts[si].def.name == name {
                        return self.production_condition_ok(logic, player, side, Some(gi), si);
                    }
                }
            }
        }
        false
    }

    fn production_condition_ok(&mut self, logic: &GameLogic, player: PlayerIndex, side: usize, group: Option<usize>, index: usize) -> bool {
        let (easy, normal, hard) = {
            let s = self.script_state(side, group, index);
            (s.def.easy, s.def.normal, s.def.hard)
        };
        if !self.difficulty_ok(player, easy, normal, hard) {
            return false;
        }
        let saved = self.current_player;
        self.current_player = Some(player);
        let r = self.evaluate_conditions(logic, side, group, index);
        self.current_player = saved;
        r
    }

    /// `TeamFactory::createInactiveTeam` / `AIPlayer::checkQueuedTeams`'s `m_executeActions` path:
    /// run the team's production-condition script's *actions* unconditionally with `<This Team>`
    /// bound (`ScriptEngine::friend_executeAction`). This is what gives wave teams their
    /// `TEAM_GUARD_AREA(<This Team>, MyOuterPerimeter)` order as they are built, so the
    /// `_LAUNCH_ATTACK` + `TEAM_INSIDE_AREA_PARTIALLY` gate can fire.
    pub(crate) fn execute_team_production_action(&mut self, logic: &mut GameLogic, team: &str) {
        let key = team.to_ascii_lowercase();
        let proto_name = self.proto_of(&key);
        let Some(proto) = self.team_protos.get(&proto_name).cloned() else { return };
        if !proto.execute_actions || proto.production_condition.is_empty() {
            return;
        }
        let mut actions = None;
        'outer: for side in 0..self.lists.len() {
            for i in 0..self.lists[side].scripts.len() {
                if self.lists[side].scripts[i].def.name == proto.production_condition {
                    actions = Some(self.lists[side].scripts[i].def.actions.clone());
                    break 'outer;
                }
            }
            for gi in 0..self.lists[side].groups.len() {
                for si in 0..self.lists[side].groups[gi].scripts.len() {
                    if self.lists[side].groups[gi].scripts[si].def.name == proto.production_condition {
                        actions = Some(self.lists[side].groups[gi].scripts[si].def.actions.clone());
                        break 'outer;
                    }
                }
            }
        }
        let Some(actions) = actions else { return };
        let saved_player = self.current_player;
        let saved_team = self.current_team.take();
        self.current_player = self.team_owner(&key);
        self.current_team = Some(key);
        for a in &actions {
            self.execute_action(logic, a);
        }
        self.current_team = saved_team;
        self.current_player = saved_player;
    }

    /// Runs a script (or every script in a group) by name, searching every side.
    fn run_named_script(&mut self, logic: &mut GameLogic, name: &str) {
        if name.is_empty() || name == NONE_STRING {
            return;
        }
        for side in 0..self.lists.len() {
            let saved = self.current_player;
            self.current_player = Some(self.lists[side].player);
            let free = self.lists[side].scripts.len();
            for i in 0..free {
                if self.lists[side].scripts[i].def.name == name {
                    self.execute_script(logic, side, None, i, true);
                    self.current_player = saved;
                    return;
                }
            }
            let groups = self.lists[side].groups.len();
            for gi in 0..groups {
                if self.lists[side].groups[gi].def.name == name {
                    let count = self.lists[side].groups[gi].scripts.len();
                    for si in 0..count {
                        self.execute_script(logic, side, Some(gi), si, true);
                    }
                    self.current_player = saved;
                    return;
                }
                let count = self.lists[side].groups[gi].scripts.len();
                for si in 0..count {
                    if self.lists[side].groups[gi].scripts[si].def.name == name {
                        self.execute_script(logic, side, Some(gi), si, true);
                        self.current_player = saved;
                        return;
                    }
                }
            }
            self.current_player = saved;
        }
    }

    /// `AIPlayer::buildSpecificAITeam(proto, priorityBuild = true)` (`AIPlayer.cpp:2445`, via
    /// `Player::buildSpecificTeam`), the `BUILD_TEAM` script action: queue an inactive team at the
    /// front of the AI's team build queue (not when the singleton already has members or the
    /// factories/tech are missing; a money shortage still queues it).
    pub(crate) fn build_specific_ai_team(&mut self, logic: &mut GameLogic, player: PlayerIndex, name: &str) {
        let proto_name = self.proto_of(&name.to_ascii_lowercase());
        let Some(proto) = self.team_protos.get(&proto_name).cloned() else { return };
        // A priority build of a singleton that already has members is refused.
        if proto.is_singleton {
            if let Some(k) = self.first_instance(&proto_name) {
                if self.teams.get(&k).is_some_and(|t| !t.members.is_empty()) {
                    return;
                }
            }
        }
        // Only an AI player has an `AIPlayer` to queue on (`Player::buildSpecificTeam`: `m_ai`).
        let (possible, need_money) = {
            let Some(state) = logic.ai_players.get(&player) else { return };
            logic.ai_possible_to_build_team(player, state, &proto, false)
        };
        if !possible && !need_money {
            return; // "because required factories/tech don't exist."
        }
        if proto.units.is_empty() {
            return; // no work orders.
        }
        let Some(key) = self.create_team(&proto_name) else { return };
        if let Some(ai) = logic.ai_players.get_mut(&player) {
            // `prependTo_TeamBuildQueue`: the front of the queue.
            ai.team_build_queue.insert(
                0,
                crate::skirmish_ai::TeamInQueue {
                    key: key.clone(),
                    frame_started: logic.frame,
                    priority_build: true,
                    reinforcement: false,
                    reinforcement_id: None,
                    sent_to_start_location: false,
                    work_orders: crate::skirmish_ai::TeamInQueue::orders_for(&proto),
                },
            );
            ai.team_delay = 0;
        }
        if proto.execute_actions {
            self.execute_team_production_action(logic, &key);
        }
    }

    /// `AISkirmishPlayer::recruitSpecificAITeam` (`AISkirmishPlayer.cpp:750`), the `RECRUIT_TEAM`
    /// action: create an inactive team, recruit idle units into it and send them home. With units
    /// recruited the team goes to the *front of the TeamReadyQueue* (so `checkReadyTeams` activates
    /// it once idle); with none the team is deleted (unless singleton). Without the queue entry the
    /// recruits would sit locked in an inactive, never-activated team forever.
    pub(crate) fn recruit_specific_ai_team(&mut self, logic: &mut GameLogic, player: PlayerIndex, name: &str, radius: f32) {
        let proto_name = self.proto_of(&name.to_ascii_lowercase());
        let Some(proto) = self.team_protos.get(&proto_name).cloned() else { return };
        let radius = if radius < 1.0 { 99999.0 } else { radius };
        if proto.is_singleton {
            if let Some(k) = self.first_instance(&proto_name) {
                if self.teams.get(&k).is_some_and(|t| !t.members.is_empty()) {
                    return; // "Unable to recruit singleton team because team already exists."
                }
            }
        }
        let Some(key) = self.create_team(&proto_name) else { return };
        let ids = self.recruit_team(logic, player, &key, radius, Coord3D::ZERO);
        // `ai->aiMoveToPosition(&teamProto->getTemplateInfo()->m_homeLocation, CMD_FROM_AI)` for every recruit
        // (AIPlayer.cpp:2638). `m_homeLocation` is zero for a team without a home (the same zero the recruit search
        // uses, AIPlayer.cpp:2604), so the move happens regardless.
        let home = self.waypoints.get(&proto.home).copied().unwrap_or(Coord3D::ZERO);
        for id in &ids {
            logic.ai_move_to_position_cmd(*id, home, crate::ai_states::CommandSource::Ai);
        }
        if !ids.is_empty() {
            if let Some(ai) = logic.ai_players.get_mut(&player) {
                ai.team_ready_queue.insert(
                    0,
                    crate::skirmish_ai::TeamInQueue {
                        key,
                        frame_started: logic.frame,
                        priority_build: false,
                        reinforcement: false,
                        reinforcement_id: None,
                        sent_to_start_location: false,
                        work_orders: Vec::new(),
                    },
                );
            }
        } else if !proto.is_singleton {
            self.teams.remove(&key);
            self.notify_of_team_destruction(&key); // `Team::~Team`
        }
    }

    /// The recruiting loop of `AIPlayer::recruitSpecificAITeam` / `queueUnits`: for each slot,
    /// `Team::tryToRecruit` (the closest eligible unit to `home`, within `max_dist`) until the slot's
    /// maximum is filled or nothing is left. Returns the recruited objects.
    pub(crate) fn recruit_team(
        &mut self,
        logic: &mut GameLogic,
        player: PlayerIndex,
        name: &str,
        max_dist: f32,
        fallback_home: Coord3D,
    ) -> Vec<ObjectId> {
        // `name` is a unique instance key (the AI) or a bare prototype name (`RECRUIT_TEAM`), which
        // gets a new inactive instance (`AIPlayer::recruitSpecificAITeam`).
        let proto_name = self.proto_of(&name.to_ascii_lowercase());
        let key = if self.teams.contains_key(&name.to_ascii_lowercase()) {
            name.to_ascii_lowercase()
        } else {
            let Some(k) = self.create_team(&proto_name) else { return Vec::new() };
            k
        };
        let Some(proto) = self.team_protos.get(&proto_name).cloned() else { return Vec::new() };
        let home = self.waypoints.get(&proto.home).copied().unwrap_or(fallback_home);
        let mut add: Vec<ObjectId> = Vec::new();
        for slot in &proto.units {
            let have = self.teams.get(&key).map_or(0, |t| t.count_template(logic, &slot.template));
            for _ in have..slot.max {
                // `Team::tryToRecruit` reads the object's own team; `Object::setTeam` below moves it.
                let Some(id) = try_to_recruit(self, logic, player, &slot.template, (home.x, home.y), max_dist, proto.production_priority)
                else {
                    break;
                };
                add.push(id);
            }
        }
        // `Object::setTeam(theTeam)` (`AISkirmishPlayer.cpp:806`): leave the old team, join the new
        // one (newest first) and take its initial attitude/attack-priority set.
        for id in &add {
            self.set_object_team(logic, *id, &key);
        }
        add
    }

    /// One `Team::tryToRecruit(thing, home, max_dist)` + `Object::setTeam(team)` (the body of the
    /// `while (order->isWaitingToBuild())` loop in `AIPlayer::queueUnits`, `AIPlayer.cpp:2708`).
    /// `key` is the unique instance key of the recruiting (or reinforced) team.
    pub(crate) fn recruit_one(
        &mut self,
        logic: &mut GameLogic,
        player: PlayerIndex,
        key: &str,
        thing: &str,
        home: Coord3D,
        max_dist: f32,
    ) -> Option<ObjectId> {
        let proto_name = self.proto_of(key);
        let priority = self.team_protos.get(&proto_name)?.production_priority;
        let id = try_to_recruit(self, logic, player, thing, (home.x, home.y), max_dist, priority)?;
        self.set_object_team(logic, id, key);
        Some(id)
    }

    /// Resolves a trigger area by name, applying the `[Skirmish]` perimeter rewrite
    /// (`ScriptEngine::getQualifiedTriggerAreaByName`).
    pub(crate) fn trigger_area(&self, name: &str) -> Option<&zr_map::PolygonTrigger> {
        const MY_INNER: &str = "[Skirmish]MyInnerPerimeter";
        const MY_OUTER: &str = "[Skirmish]MyOuterPerimeter";
        const ENEMY_INNER: &str = "[Skirmish]EnemyInnerPerimeter";
        const ENEMY_OUTER: &str = "[Skirmish]EnemyOuterPerimeter";
        if name == MY_INNER || name == MY_OUTER {
            let n = self.current_player.and_then(|p| self.player_start.get(&p)).copied()? + 1;
            let key = format!("{}{n}", if name == MY_INNER { "InnerPerimeter" } else { "OuterPerimeter" });
            return self.trigger_areas.get(&key);
        }
        if name == ENEMY_INNER || name == ENEMY_OUTER {
            let enemy_start = self.current_enemy_start()?;
            let key = format!("{}{}", if name == ENEMY_INNER { "InnerPerimeter" } else { "OuterPerimeter" }, enemy_start + 1);
            return self.trigger_areas.get(&key);
        }
        self.trigger_areas.get(name)
    }

    /// The start index of the current player's current enemy (for `[Skirmish]Enemy*Perimeter`).
    fn current_enemy_start(&self) -> Option<i32> {
        let me = self.current_player?;
        // Skirmish enemies are the other skirmish players; pick the nearest start by index order.
        self.player_start
            .iter()
            .filter(|(p, _)| **p != me)
            .min_by_key(|(_, s)| **s)
            .map(|(_, s)| *s)
    }

    /// `Object::setTeam` (Object.cpp:912): the attack-priority set a unit joining a team of
    /// prototype `proto` takes (only when the prototype names a defined set).
    pub fn team_attack_info(&self, proto: &TeamPrototype) -> Option<String> {
        let n = &proto.attack_priority_name;
        (!n.is_empty() && self.attack_priorities.contains_key(n)).then(|| n.clone())
    }

    /// Records an attack-priority entry (`ScriptEngine::setPriorityThing`; the set is created on
    /// first use, `findAttackInfo(name, true)`).
    pub fn set_attack_priority(&mut self, set: &str, type_name: &str, priority: i32) {
        self.attack_priorities.entry(set.to_owned()).or_default().map.insert(type_name.to_ascii_lowercase(), priority);
    }

    /// `ScriptEngine::setPriorityDefault`.
    pub fn set_default_attack_priority(&mut self, set: &str, priority: i32) {
        self.attack_priorities.entry(set.to_owned()).or_default().default = priority;
    }

    /// The priority of `type_name` in `set` (`AttackPriorityInfo::getPriority`; a missing set is the
    /// default info, whose priority is 1 for everything).
    pub fn attack_priority(&self, set: &str, type_name: &str) -> i32 {
        self.attack_priorities.get(set).map_or(1, |i| i.priority(type_name))
    }

    /// Whether the map has any waypoint: `TerrainLogic::getFirstWaypoint` is non-null, so a train's
    /// track has an anchor (`RailroadBehavior::loadTrackData`).
    pub(crate) fn has_waypoints(&self) -> bool {
        !self.waypoints.is_empty() || !self.waypoint_by_id.is_empty()
    }

    /// Installs map waypoints (name -> position) for `TEAM_*`/`NAMED_*` waypoint orders.
    pub fn set_waypoints(&mut self, waypoints: impl IntoIterator<Item = (String, Coord3D)>) {
        for (n, p) in waypoints {
            self.waypoints.insert(n, p);
        }
    }

    /// Installs the waypoints with path labels and the map's waypoint links, for the
    /// `SKIRMISH_*_APPROACH_PATH` actions (`TerrainLogic::addWaypoint` + `addWaypointLink`).
    pub fn set_waypoint_paths(
        &mut self,
        waypoints: impl IntoIterator<Item = (i32, String, Coord3D, Vec<String>)>,
        links: impl IntoIterator<Item = (i32, i32)>,
    ) {
        for (id, name, pos, labels) in waypoints {
            self.waypoint_by_id.insert(id, WaypointInfo { name, pos, labels });
        }
        for (a, b) in links {
            self.waypoint_links.entry(a).or_default().push(b);
        }
    }

    /// Installs the `KindOf` index -> template-name table for `SET_ATTACK_PRIORITY_KIND_OF`
    /// (`ThingFactory` iteration in `ScriptEngine::setPriorityKind`).
    pub fn set_kind_of_templates(&mut self, map: BTreeMap<i32, Vec<String>>) {
        self.kind_of_templates = map;
    }

    /// `TerrainLogic::getClosestWaypointOnPath`: the closest waypoint whose path labels include
    /// `label` (case-insensitive).
    fn closest_waypoint_on_path(&self, pos: &Coord3D, label: &str) -> Option<i32> {
        let mut best: Option<(f32, i32)> = None;
        for (id, w) in &self.waypoint_by_id {
            if !w.labels.iter().any(|l| l.eq_ignore_ascii_case(label)) {
                continue;
            }
            let d = pos.dist_2d(&w.pos);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, *id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// `TerrainLogic::getClosestWaypointOnPath` reduced to the waypoint's position: the closest
    /// waypoint on `label`, for `AISkirmishPlayer::buildAIBaseDefenseStructure` and
    /// `computeSuperweaponTarget`.
    pub(crate) fn closest_waypoint_on_path_pos(&self, pos: &Coord3D, label: &str) -> Option<Coord3D> {
        self.closest_waypoint_on_path(pos, label).and_then(|id| self.waypoint_by_id.get(&id).map(|w| w.pos))
    }

    /// `ScriptActions::doTeamFollowSkirmishApproachPath` / `doTeamMoveToSkirmishApproachPath`:
    /// move the team to (or follow the linked path from) the closest waypoint on the *enemy's*
    /// `<label><enemyStartIndex+1>` approach path.
    pub(crate) fn team_follow_approach_path(&mut self, logic: &mut GameLogic, team: &str, label: &str, attack: bool) {
        let Some(player) = self.team_owner(team) else { return };
        // `ScriptEngine::getSkirmishEnemyPlayer`: the current player's current enemy.
        let enemy = self.player_enemy.get(&player).copied().or_else(|| self.player_start.keys().copied().find(|p| *p != player));
        let Some(enemy) = enemy else { return };
        let mp_ndx = self.player_start.get(&enemy).copied().unwrap_or(0) + 1;
        let path_label = format!("{label}{mp_ndx}");
        let ids = self.team_members(logic, team);
        if ids.is_empty() {
            return;
        }
        // The team's centre point.
        let (mut cx, mut cy, mut n) = (0.0f32, 0.0f32, 0.0f32);
        for id in &ids {
            if let Some(o) = logic.find_object(*id) {
                cx += o.position.x;
                cy += o.position.y;
                n += 1.0;
            }
        }
        if n == 0.0 {
            return;
        }
        let center = Coord3D::new(cx / n, cy / n, 0.0);
        let Some(wp) = self.closest_waypoint_on_path(&center, &path_label) else { return };
        if attack {
            // `groupFollowWaypointPathAsTeam` (`doTeamFollowSkirmishApproachPath` is always called with
            // `asTeam` by the skirmish scripts): every member runs `AI_FOLLOW_WAYPOINT_PATH_AS_TEAM`,
            // which the mood turns into the attack-follow (`AIStates.cpp:4135`). Team-lockstep
            // waypoints and group speed (`m_moveAsGroup`) are not modelled: members walk individually.
            let Some((at, at_pos)) = self.walk_start(wp) else { return };
            for id in ids {
                logic.ai_follow_waypoint(id, at, at_pos, crate::ai_states::CommandSource::Script);
            }
        } else {
            // `groupMoveToPosition(way->getLocation(), false, CMD_FROM_SCRIPT)`.
            let pos = self.waypoint_by_id[&wp].pos;
            logic.submit(player, crate::command::Command::MoveTo { ids, pos });
        }
        if let Some(t) = self.teams.get_mut(&team.to_ascii_lowercase()) {
            t.state = TeamState::Hunt;
        }
    }

    /// `ScriptActions::doTeamGarrisonNearestBuilding`: send the team's infantry into the nearest
    /// garrisonable building (an internet center when the leader is a money hacker), one unit per
    /// free slot. Ported from `ScriptActions.cpp` (the `PartitionFilterAcceptByKindOf
    /// KINDOF_FS_INTERNET_CENTER` / `PartitionFilterGarrisonableByPlayer` choice and the
    /// `!isKindOf(KINDOF_NO_GARRISON)` infantry gate).
    pub(crate) fn team_garrison_nearest_building(&mut self, logic: &mut GameLogic, team: &str) {
        use crate::template::kind_of;
        let Some(player) = self.team_owner(team) else { return };
        let ids = self.team_members(logic, team);
        let Some(&leader) = ids.first() else { return };
        let Some(leader_obj) = logic.find_object(leader) else { return };
        let leader_pos = leader_obj.position;
        let hacker = leader_obj.is_kind_of(kind_of::MONEY_HACKER);
        let mut buildings: Vec<(f32, ObjectId)> = logic
            .objects()
            .filter(|o| !o.destroyed && logic.relationship(player, o.player) != crate::damage::Relationship::Enemies)
            .filter(|o| {
                if hacker {
                    o.is_kind_of(kind_of::FS_INTERNET_CENTER)
                } else {
                    o.template.contain.as_ref().is_some_and(|c| c.capacity != 0 && c.is_garrison())
                }
            })
            .map(|o| (leader_pos.dist_2d(&o.position), o.id))
            .collect();
        buildings.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut infantry: Vec<ObjectId> = ids
            .iter()
            .copied()
            .filter(|id| {
                logic.find_object(*id).is_some_and(|o| {
                    o.is_kind_of(kind_of::INFANTRY) && !o.is_kind_of(kind_of::NO_GARRISON) && !logic.is_contained(*id)
                })
            })
            .collect();
        infantry.reverse(); // `DLINK_ITERATOR` consumes from the front.
        for (_, bid) in buildings {
            let free = logic.find_object(bid).and_then(|o| o.template.contain.as_ref()).map_or(0, |c| {
                let cap = if c.capacity < 0 { i32::MAX } else { c.capacity };
                cap - logic.contained_units(bid).len() as i32
            });
            let mut batch = Vec::new();
            for _ in 0..free.max(0) {
                let Some(u) = infantry.pop() else { break };
                batch.push(u);
            }
            if !batch.is_empty() {
                logic.submit(player, crate::command::Command::Enter { ids: batch, container: bid });
            }
            if infantry.is_empty() {
                break;
            }
        }
    }

    /// Resolves a team's owner player (`teamOwner`): `<This Player>`/empty -> the current player,
    /// else the registered player name (`SkirmishChina`, `player0`, ...).
    pub(crate) fn team_owner(&self, name: &str) -> Option<PlayerIndex> {
        let proto = self.team_proto(name)?;
        if proto.owner.is_empty() || proto.owner == THIS_PLAYER {
            return self.current_player;
        }
        self.player_names.get(&proto.owner.to_ascii_lowercase()).copied().or(self.current_player)
    }

    /// The team's member ids in the engine's order (newest first); dead members stay until the
    /// object is deleted (`~Object` -> `setTeam(nullptr)`).
    pub(crate) fn team_members(&mut self, _logic: &GameLogic, name: &str) -> Vec<ObjectId> {
        let key = name.to_ascii_lowercase();
        let Some(t) = self.teams.get(&key) else { return Vec::new() };
        t.members.clone()
    }

    /// The position a team order should start from (first live member, else the team home, else 0).
    fn team_anchor(&self, logic: &GameLogic, name: &str) -> Coord3D {
        if let Some(t) = self.teams.get(&name.to_ascii_lowercase()) {
            if let Some(p) = t.members.iter().find_map(|id| logic.find_object(*id).map(|o| o.position)) {
                return p;
            }
        }
        if let Some(proto) = self.team_proto(name) {
            if let Some(p) = self.waypoints.get(&proto.home) {
                return *p;
            }
        }
        Coord3D::ZERO
    }

    /// `TEAM_*` move/attack-move orders: issue an attack-move to `pos` for every member.
    pub(crate) fn team_move(&mut self, logic: &mut GameLogic, name: &str, pos: Coord3D, attack: bool) {
        let Some(player) = self.team_owner(name) else { return };
        let ids = self.team_members(logic, name);
        if ids.is_empty() {
            return;
        }
        if attack {
            // `AIGroup::groupAttackMoveToPosition(&destination, NO_MAX_SHOTS_LIMIT, CMD_FROM_SCRIPT)`.
            logic.command_attack_move(player, &ids, pos, CommandSource::Script);
        } else {
            // `ScriptActions` -> `AIGroup::groupMoveToPosition(&destination, false, CMD_FROM_SCRIPT)`
            // (`ScriptActions.cpp:427`): a script order, so `groupMoveToPosition`'s player-only stealth rule does
            // not run (`AIGroup.cpp:1759`). Kept on the command list (the sim's script engine defers orders).
            logic.submit_from(player, CommandSource::Script, crate::command::Command::MoveTo { ids, pos });
        }
    }

    /// `TEAM_GUARD_AREA` / `TEAM_GUARD_POSITION`: guard `pos` (`AIGuardState`).
    pub(crate) fn team_guard_point(&mut self, logic: &mut GameLogic, name: &str, pos: Coord3D) {
        let Some(player) = self.team_owner(name) else { return };
        let ids = self.team_members(logic, name);
        if ids.is_empty() {
            return;
        }
        logic.command_guard(player, &ids, crate::orders::GuardTarget::Point(pos), crate::orders::GuardMode::Normal);
        if let Some(t) = self.teams.get_mut(&name.to_ascii_lowercase()) {
            t.state = TeamState::Guard;
        }
    }

    /// `TEAM_HUNT` / `TEAM_ATTACK_AREA`: put the team into `AIHuntState` (no area) or
    /// `AIAttackAreaState` (with an area), which keep acquiring the closest enemy anywhere.
    pub(crate) fn team_hunt(&mut self, logic: &mut GameLogic, name: &str, area: Option<(Coord3D, f32)>) {
        let Some(player) = self.team_owner(name) else { return };
        let ids = self.team_members(logic, name);
        if ids.is_empty() {
            return;
        }

        match area {
            Some((a, r)) => logic.command_attack_area(player, &ids, a, r),
            None => logic.command_hunt(player, &ids),
        }
        if let Some(t) = self.teams.get_mut(&name.to_ascii_lowercase()) {
            t.state = TeamState::Hunt;
        }
    }

    /// `TEAM_ENTER_NAMED` / `TEAM_FOLLOW_WAYPOINTS`: move to a named object, waypoint or the home.
    pub(crate) fn team_move_named(&mut self, logic: &mut GameLogic, name: &str, target: &str, attack: bool) {
        let pos = self
            .named_object(target)
            .and_then(|id| logic.find_object(id).map(|o| o.position))
            .or_else(|| self.waypoints.get(target).copied())
            .or_else(|| {
                let home = self.team_proto(name)?.home.clone();
                self.waypoints.get(&home).copied()
            });
        if let Some(pos) = pos {
            self.team_move(logic, name, pos, attack);
        }
    }

    /// `TEAM_GUARD_AREA` (`ScriptActions::doTeamGuardArea`, ScriptActions.cpp:2010): every member guards the trigger
    /// area itself (`AIGroup::groupGuardArea` with `CMD_FROM_SCRIPT`), not only its centre (lane 184).
    pub(crate) fn team_guard_area(&mut self, logic: &mut GameLogic, name: &str, area: &str) {
        let Some(trig) = self.trigger_area(area).cloned() else { return };
        let Some(player) = self.team_owner(name) else { return };
        let ids = self.team_members(logic, name);
        if ids.is_empty() {
            return;
        }
        logic.command_guard_area(player, &ids, std::sync::Arc::new(trig), crate::orders::GuardMode::Normal);
        if let Some(t) = self.teams.get_mut(&name.to_ascii_lowercase()) {
            t.state = TeamState::Guard;
        }
    }

    /// `ScriptActions::doMoveTeamTowardsNearest` (`ScriptActions.cpp:5327`): every member gets
    /// `chooseLocomotorSet(LOCOMOTORSET_NORMAL)` and `aiMoveToObject(bestObj, CMD_FROM_SCRIPT)`, so each
    /// one moves to the nearest object inside `trigger` whose type is `type_name`, of any owner. A
    /// template name is searched directly (no match returns); otherwise `type_name` is an object list
    /// (`OBJECTLIST_ADDOBJECTTYPE`) and the nearest of its templates wins (first on ties). The list
    /// branch never checks for a found object, so with none the members get a null object and move to
    /// the zeroed goal position (`ScriptActions.cpp:5422-5423`, `StateMachine::internalClear`). Members
    /// move individually, with no group destination (`AIGroup` is not involved).
    pub(crate) fn team_move_towards_nearest(&mut self, logic: &mut GameLogic, name: &str, type_name: &str, trigger: &str) {
        let Some(trig) = self.trigger_area(trigger).cloned() else { return };
        let from = self.team_anchor(logic, name);
        let is_template = logic.find_template(type_name).is_some();
        let types: Vec<String> = if is_template {
            vec![type_name.to_owned()]
        } else {
            self.object_lists.get(type_name).cloned().unwrap_or_default()
        };
        let mut best: Option<(f32, crate::object::ObjectId)> = None;
        for t in &types {
            // `getClosestObject` per template; the list keeps the first of equal distances (`dist < closestDist`).
            let closest = logic
                .objects()
                .filter(|o| {
                    !o.destroyed
                        && !o.is_dying()
                        && o.template.name.eq_ignore_ascii_case(t)
                        && trig.contains(o.position.x as i32, o.position.y as i32)
                })
                .map(|o| (from.dist_2d(&o.position), o.id))
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((d, id)) = closest {
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, id));
                }
            }
        }
        // `if (!bestObj) return;` is only in the template branch (`ScriptActions.cpp:5375`).
        if is_template && best.is_none() {
            return;
        }
        let target = best.map_or(crate::object::ObjectId::INVALID, |(_, id)| id);        // The member loop returns at the first member without an object or AI (`ScriptActions.cpp:5415,5420`).
        for id in self.team_members(logic, name) {
            if logic.find_object(id).is_none_or(|o| o.ai_handle.is_none()) {
                break;
            }
            logic.ai_choose_locomotor_set(id, crate::template::LocomotorSet::Normal);
            logic.ai_move_to_object_cmd(id, target, crate::ai_states::CommandSource::Script);
        }
    }

    /// `ScriptActions::doSkirmishAttackNearestGroupWithValue` (`ScriptActions.cpp:5450-5479`): the team's
    /// `AIGroup` attack-moves to the nearest partition cell whose summed **enemy** build cost passes `value`
    /// (`PartitionManager::getNearestGroupWithValue`, `PartitionManager.cpp:4880`, with `VOT_CashValue` and
    /// `ALLOW_ENEMIES`), searched breadth-first from the group's centre (`AIGroup::getCenter`,
    /// `AIGroup.cpp:325`). `comparison` is `Parameter::LESS_THAN..NOT_EQUAL` (`Scripts.h:780-786`); only
    /// `GREATER_EQUAL` (3) and `GREATER` (4) run the search. For any other value, and when no cell passes,
    /// the engine's `Coord3D loc` is never written and it attack-moves to uninitialised memory
    /// (`:5467-5477`); the port drops the order instead.
    pub(crate) fn skirmish_attack_nearest_group_with_value(&mut self, logic: &mut GameLogic, name: &str, comparison: i32, value: i32) {
        let Some(player) = self.team_owner(name) else { return };
        let ids = self.team_members(logic, name);
        let Some(center) = crate::orders::ai_group_center(logic, &ids) else { return };
        const GREATER_EQUAL: i32 = 3;
        const GREATER: i32 = 4;
        let loc = if comparison == GREATER_EQUAL || comparison == GREATER {
            match logic.nearest_group_with_value(player, value, &center) {
                Some(loc) => loc,
                None => return,
            }
        } else {
            return;
        };
        logic.command_attack_move(player, &ids, loc, CommandSource::Script);
    }

    /// `ScriptActions::doTeamHuntWithCommandButton` (`ScriptActions.cpp:2073`): every member that has
    /// a `CommandButtonHuntUpdate` takes the button's hunt (`setCommandButton`). An unknown or refused
    /// button does nothing. Members are not checked against the button's command set (not modelled).
    pub(crate) fn team_hunt_with_command_button(&mut self, logic: &mut GameLogic, name: &str, ability: &str) {
        use crate::modules4::HuntCommand;
        let Some(button) = self.command_buttons.get(&ability.to_ascii_lowercase()).cloned() else { return };
        let command = match button {
            CommandButtonHunt::Weapon => HuntCommand::Weapon,
            CommandButtonHunt::SpecialPower { power } => HuntCommand::SpecialPower { power, object_type: String::new() },
            CommandButtonHunt::Enter(kind) => HuntCommand::Enter { kind, object_type: String::new() },
            CommandButtonHunt::Refused => return,
        };
        for id in self.team_members(logic, name) {
            logic.set_hunt_command(id, command.clone());
        }
    }

    /// `ScriptActions::doTeamCaptureNearestUnownedFactionUnit` (`ScriptActions.cpp:5988`): the group
    /// enters the nearest object of an enemy or neutral player that is `DISABLED_UNMANNED`
    /// (`PartitionFilterUnmannedObject`). Ordinary structures such as oil derricks are not candidates,
    /// so the action usually does nothing; the derricks are taken by the command-button hunt instead.
    pub(crate) fn team_capture_nearest_unowned(&mut self, logic: &mut GameLogic, name: &str) {
        let Some(player) = self.team_owner(name) else { return };
        let ids = self.team_members(logic, name);
        if ids.is_empty() {
            return;
        }
        let from = self.team_anchor(logic, name);
        let target = logic
            .objects()
            .filter(|o| {
                !o.destroyed
                    && o.is_disabled_by_type(crate::disabled::DisabledType::Unmanned)
                    && matches!(
                        logic.relationship(player, o.player),
                        crate::damage::Relationship::Enemies | crate::damage::Relationship::Neutral
                    )
            })
            .min_by(|a, b| from.dist_2d(&a.position).total_cmp(&from.dist_2d(&b.position)))
            .map(|o| o.id);
        if let Some(container) = target {
            logic.submit(player, crate::command::Command::Enter { ids, container });
        }
    }

    /// `TEAM_STOP` / `TEAM_STOP_AND_DISBAND` (`ScriptActions::doTeamStop`, ScriptActions.cpp:4642-4675):
    /// `AIGroup::groupIdle(CMD_FROM_SCRIPT)` (`AIGroup.cpp:2078-2085`) issues one `aiIdle` per member
    /// in the team member-list order (newest first); with `shouldDisband` every member becomes
    /// recruitable again and the team is merged into its controlling player's default team
    /// (`doMergeTeamIntoTeam`, `ScriptActions.cpp:3231-3260`).
    pub fn team_stop(&mut self, logic: &mut GameLogic, name: &str, disband: bool) {
        let ids = self.team_members(logic, name);
        for id in &ids {
            logic.ai_idle(*id, CommandSource::Script);
        }
        if disband {
            // `doTeamStop`: every member becomes recruitable again before the merge into the
            // default team (ScriptActions.cpp:4661-4669).
            for id in &ids {
                if let Some(o) = logic.objects.get_mut(id) {
                    o.ai.recruitable = true;
                }
            }
            let key = name.to_ascii_lowercase();
            let player = self.team_player(&key);
            if let Some(dst) = player.and_then(|p| self.player_default_team(p).map(str::to_owned)) {
                if dst != key {
                    self.team_merge(logic, &key, &dst);
                }
            }
        }
    }

    /// `TEAM_MERGE_INTO_TEAM` (`ScriptActions::doMergeTeamIntoTeam`, ScriptActions.cpp:3231-3260): every
    /// member of `name` joins `other` through `Object::setTeam` (leaving its old team's list, joining
    /// the new one's head and taking its attitude), then `teamSrc->deleteTeam()` and
    /// `teamDest->setActive()`. `deleteTeam` leaves the now-empty non-singleton source for
    /// `TeamPrototype::updateState` to remove (`Team.cpp:1058-1088`). Both names are resolved by the
    /// caller (`getTeamNamed`); the destination is created when no instance exists
    /// (`TeamFactory::findTeam`; none for a singleton). Returns the destination key.
    pub(crate) fn team_merge(&mut self, logic: &mut GameLogic, name: &str, other: &str) -> Option<String> {
        let key = name.to_ascii_lowercase();
        if !self.teams.contains_key(&key) {
            return None; // `!teamSrc`: nothing happens.
        }
        let dst = if self.teams.contains_key(&other.to_ascii_lowercase()) {
            other.to_ascii_lowercase()
        } else {
            let proto = self.proto_of(other);
            if self.team_protos.get(&proto)?.is_singleton && self.first_instance(&proto).is_none() {
                return None;
            }
            self.create_team(&proto)?
        };
        let members = self.teams.get(&key)?.members.clone();
        for id in &members {
            self.set_object_team(logic, *id, &dst);
            // `doMergeTeamIntoTeam`: `updateTeamAndPlayerStuff(obj)` after each `setTeam`
            // (`ScriptActions.cpp:3248`, :3253): re-run the upgrade modules.
            logic.update_upgrade_modules(*id);
        }
        // `teamSrc->deleteTeam()` (`ScriptActions.cpp:3256`): its (already moved) members are gone; the
        // empty instance is left for `TeamPrototype::updateState` (`Team.cpp:1058`).
        if let Some(t) = self.teams.get_mut(&key) {
            t.members.clear();
        }
        Some(dst)
    }

    /// `TEAM_INCREASE_PRIORITY` -> `TeamPrototype::increaseAIPriorityForSuccess`.
    pub(crate) fn team_increase_priority(&mut self, name: &str) {
        if let Some(p) = self.team_protos.get_mut(&name.to_ascii_lowercase()) {
            p.production_priority += p.production_priority_success;
        }
    }

    /// `TEAM_DECREASE_PRIORITY` -> `TeamPrototype::decreaseAIPriorityForFailure`.
    pub(crate) fn team_decrease_priority(&mut self, name: &str) {
        if let Some(p) = self.team_protos.get_mut(&name.to_ascii_lowercase()) {
            p.production_priority -= p.production_priority_failure;
        }
    }

    /// `ScriptActions::doTeamExitAll` -> `AIGroup::groupEvacuate` (`AIGroup.cpp:2475`), over the team's
    /// members (`getTeamAsAIGroup`). An airborne aircraft with an AI moves to the ground below itself
    /// and evacuates there (`aiMoveToAndEvacuate`, `AIGroup.cpp:2483-2488`). Every other member
    /// orders its own passengers out (`aiEvacuate` -> `orderAllPassengersToExit`, `AIUpdate.cpp:3922`),
    /// so a rider whose container is not a member stays where it is.
    pub(crate) fn team_exit_all(&mut self, logic: &mut GameLogic, name: &str) {
        use crate::disabled::DisabledType;
        use crate::template::kind_of;
        let ids = self.team_members(logic, name);
        for id in ids {
            let Some(o) = logic.find_object(id) else { continue };
            let contained = logic.contained_units(id).to_vec();
            if o.ai_handle.is_some() {
                // `AIGroup::groupEvacuate` (AIGroup.cpp:2483): `isKindOf(KINDOF_AIRCRAFT) && isAirborneTarget()`.
                if o.is_kind_of(kind_of::AIRCRAFT) && o.status_bits & crate::object::status::AIRBORNE_TARGET != 0 {
                    // `TheTerrainLogic->getHighestLayerForDestination` + `getLayerHeight` (`AIGroup.cpp:2486-2488`).
                    let mut pos = o.position;
                    let layer = logic.terrain.highest_layer_for_destination(&pos);
                    pos.z = logic.terrain.layer_height(pos.x, pos.y, layer);
                    logic.ai_move_and_evacuate(id, pos);
                } else if !o.is_disabled_by_type(DisabledType::Subdued) {
                    // `aiEvacuate(FALSE)` -> `privateEvacuate` (`AIUpdate.cpp:3922`): empty its own contain
                    // module, if any. A rider has none, so this is a no-op for it.
                    logic.exit_passengers(id, &contained);
                }
            } else if o.is_kind_of(kind_of::STRUCTURE) {
                // A building without an AI calls `orderAllPassengersToExit` directly (`AIGroup.cpp:2496-2505`).
                logic.exit_passengers(id, &contained);
            }
        }
    }

    /// `ScriptActions::updateTeamSetAttitude`: store each member's `AttitudeType`. A passive or
    /// sleeping unit drops its auto-acquire orders (`AIUpdateInterface::setAttitude`).
    pub(crate) fn team_set_attitude(&mut self, logic: &mut GameLogic, name: &str, attitude: i32) {
        let ids = self.team_members(logic, name);
        for id in ids {
            if let Some(o) = logic.objects.get_mut(&id) {
                o.ai.attitude = attitude as i8;
                if attitude <= -1 {
                    o.combat.attack = None;
                    o.combat.hunt = false;
                    o.combat.attack_move = false;
                }
            }
        }
    }

    /// `ScriptActions::doTeamGuardInTunnelNetwork`: the `AITNGuard` AI is not ported; the team
    /// moves into the nearest own tunnel network (`TunnelContain`), the containing half of that
    /// behaviour.
    pub(crate) fn team_guard_tunnel_network(&mut self, logic: &mut GameLogic, name: &str) {
        let Some(player) = self.team_owner(name) else { return };
        let ids = self.team_members(logic, name);
        if ids.is_empty() {
            return;
        }
        let anchor = self.team_anchor(logic, name);
        let tunnel = logic
            .objects()
            .filter(|o| {
                o.player == player
                    && !o.destroyed
                    && o.template.contain.as_ref().is_some_and(|c| c.kind == crate::contain::ContainKind::Tunnel)
            })
            .min_by(|a, b| anchor.dist_2d(&a.position).total_cmp(&anchor.dist_2d(&b.position)))
            .map(|o| o.id);
        if let Some(tunnel) = tunnel {
            logic.submit(player, crate::command::Command::Enter { ids, container: tunnel });
        }
    }

    // ---- warnings / diagnostics ------------------------------------------------------------

    fn warn(&mut self, name: &str) {
        *self.warnings.entry(name.to_owned()).or_default() += 1;
    }

    /// Hash the mutable script state (flags, counters, per-script active/interval).
    pub fn crc(&self, c: &mut Crc) {
        c.add_u32(self.lists.len() as u32);
        for l in &self.lists {
            c.add_u32(u32::from(l.player));
            c.add_u32(l.scripts.len() as u32);
            for s in &l.scripts {
                c.add_bool(s.active);
                c.add_u32(s.frame_to_evaluate);
            }
            c.add_u32(l.groups.len() as u32);
            for g in &l.groups {
                c.add_bool(g.active);
                c.add_u32(g.scripts.len() as u32);
                for s in &g.scripts {
                    c.add_bool(s.active);
                    c.add_u32(s.frame_to_evaluate);
                }
            }
        }
        c.add_u32(self.flags.len() as u32);
        for (k, v) in &self.flags {
            c.add_str(k);
            c.add_bool(*v);
        }
        c.add_u32(self.counters.len() as u32);
        for (k, v) in &self.counters {
            c.add_str(k);
            c.add_u32(v.value as u32);
            c.add_bool(v.countdown);
        }
        c.add_u32(self.next_team_id);
        c.add_u32(self.teams.len() as u32);
        for (k, t) in &self.teams {
            c.add_str(k);
            c.add_bool(t.created);
            c.add_bool(t.created_pending);
            c.add_u32(t.owner.map_or(0xffff_ffff, u32::from));
            c.add_u32(match t.recruitable {
                None => 2,
                Some(false) => 0,
                Some(true) => 1,
            });
            c.add_u32(t.state as u32);
            c.add_str(self.team_protos.get(&t.proto).map_or("", |p| p.attack_priority_name.as_str()));
            c.add_bool(t.enemy_sighted);
            c.add_bool(t.was_idle);
            c.add_bool(t.destroyed_run);
            c.add_u32(t.common_attack_target.0);
            c.add_u32(t.generic_done);
            c.add_u32(t.members.len() as u32);
            for m in &t.members {
                c.add_u32(m.0);
            }
        }
        self.crc_sequential(c);
        c.add_u32(self.attack_priorities.len() as u32);
        for (set, info) in &self.attack_priorities {
            c.add_str(set);
            c.add_u32(info.default as u32);
            c.add_u32(info.map.len() as u32);
            for (t, p) in &info.map {
                c.add_str(t);
                c.add_u32(*p as u32);
            }
        }
        for events in [&self.triggered_special_powers, &self.completed_special_powers, &self.completed_upgrades] {
            c.add_u32(events.len() as u32);
            for (player, list) in events {
                c.add_u32(u32::from(*player));
                c.add_u32(list.len() as u32);
                for (power, src) in list {
                    c.add_str(power);
                    c.add_u32(src.0);
                }
            }
        }
    }

    // ---- special-power events --------------------------------------------------------------

    /// `ScriptEngine::isSpecialPowerTriggered` / `isSpecialPowerComplete`: consumes (removes) the
    /// first matching event, as the engine conditions do (`removeFromList = TRUE`).
    pub(crate) fn consume_special_power_event(
        events: &mut BTreeMap<PlayerIndex, Vec<(String, ObjectId)>>,
        player: PlayerIndex,
        power: &str,
        source: Option<ObjectId>,
    ) -> bool {
        let Some(list) = events.get_mut(&player) else { return false };
        let found = list.iter().position(|(p, s)| p == power && source.is_none_or(|src| *s == src));
        match found {
            Some(i) => {
                list.remove(i);
                true
            }
            None => false,
        }
    }

}

/// The bounding-box centre of a polygon trigger.
pub(crate) fn super_trigger_center(t: &zr_map::PolygonTrigger) -> (f32, f32) {
    let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for p in &t.points {
        lo = (lo.0.min(p[0] as f32), lo.1.min(p[1] as f32));
        hi = (hi.0.max(p[0] as f32), hi.1.max(p[1] as f32));
    }
    ((lo.0 + hi.0) * 0.5, (lo.1 + hi.1) * 0.5)
}

/// The bounding-box radius of a polygon trigger (half the diagonal).
pub(crate) fn super_trigger_radius(t: &zr_map::PolygonTrigger) -> f32 {
    let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for p in &t.points {
        lo = (lo.0.min(p[0] as f32), lo.1.min(p[1] as f32));
        hi = (hi.0.max(p[0] as f32), hi.1.max(p[1] as f32));
    }
    // Plain IEEE `* +` and `sqrt` (std's `hypot` is the platform libm, AGENTS.md "Numbers").
    let (hw, hh) = ((hi.0 - lo.0) * 0.5, (hi.1 - lo.1) * 0.5);
    crate::math::sqrt(hw * hw + hh * hh)
}

/// Reads a string parameter (`Parameter::getString`).
pub(crate) fn pstr(p: &[Parameter], i: usize) -> &str {
    p.get(i).and_then(Parameter::string).unwrap_or("")
}

/// Reads an int parameter (`Parameter::getInt`).
pub(crate) fn pint(p: &[Parameter], i: usize) -> i32 {
    p.get(i).and_then(Parameter::int).unwrap_or(0)
}

/// Reads a real parameter (`Parameter::getReal`).
pub(crate) fn preal(p: &[Parameter], i: usize) -> f32 {
    p.get(i).and_then(Parameter::real).unwrap_or(0.0)
}

/// Matches a command-button/ability name against a special power name. The engine resolves the
/// button through `TheControlBar->findCommandButton`; that table is not ported, so the common
/// `Command_`/`SpecialAbility` prefixes are stripped and the remainder compared.
fn command_power_matches(power: &str, ability: &str) -> bool {
    fn strip(s: &str) -> String {
        s.trim_start_matches("SpecialAbility")
            .trim_start_matches("Command_")
            .to_ascii_lowercase()
    }
    if power.eq_ignore_ascii_case(ability) {
        return true;
    }
    let (p, a) = (strip(power), strip(ability));
    !p.is_empty() && !a.is_empty() && (p == a || p.contains(&a) || a.contains(&p))
}
