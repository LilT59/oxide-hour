//! Script teams: `TeamPrototype` (`Common/RTS/Team.cpp` `TeamTemplateInfo`) and `Team` instances
//! (`Team.cpp`), plus AI recruitment (`AIPlayer::recruitSpecificAITeam`,
//! `AISkirmishPlayer::selectTeamToBuild`).

use zr_map::Dict;

use super::ScriptEngine;
use crate::logic::GameLogic;
use crate::object::{ObjectId, PlayerIndex};

/// `PartitionSolver::solve` with `PREFER_FAST_SOLUTION` (`Common/PartitionSolver.cpp`): the entries
/// (`(unit, slots)`) and spaces (`(transport, capacity)`) are both sorted largest first, then each
/// entry takes the first space that still holds it (`size <= space`) and the space is reduced by it.
/// Entries that fit nowhere are skipped. Returns the `(unit, transport)` pairs in placement order.
/// `std::sort` is not stable in general, but the engine's insertion-sort path (up to 16 elements,
/// which covers any team) keeps equal sizes in their original order, so a stable sort matches it.
pub fn partition_fast(entries: &[(ObjectId, u32)], spaces: &[(ObjectId, u32)]) -> Vec<(ObjectId, ObjectId)> {
    let mut data = entries.to_vec();
    data.sort_by_key(|a| std::cmp::Reverse(a.1));
    let mut spaces = spaces.to_vec();
    spaces.sort_by_key(|a| std::cmp::Reverse(a.1));
    let mut solution = Vec::new();
    for (unit, size) in data {
        if let Some(space) = spaces.iter_mut().find(|s| size <= s.1) {
            space.1 -= size;
            solution.push((unit, space.0));
        }
    }
    solution
}

#[cfg(test)]
mod partition_tests {
    use super::partition_fast;
    use crate::object::ObjectId;

    #[test]
    fn largest_entries_take_the_first_fitting_largest_space() {
        // Entries sort 2 (3 slots), 1 (2), 3 (1); spaces sort 10 (3), 11 (3) (stable on ties).
        let units = [(ObjectId(1), 2), (ObjectId(2), 3), (ObjectId(3), 1)];
        let spaces = [(ObjectId(10), 3), (ObjectId(11), 3)];
        assert_eq!(
            partition_fast(&units, &spaces),
            vec![(ObjectId(2), ObjectId(10)), (ObjectId(1), ObjectId(11)), (ObjectId(3), ObjectId(11))]
        );
    }

    #[test]
    fn an_entry_that_fits_nowhere_is_skipped() {
        let units = [(ObjectId(1), 4), (ObjectId(2), 1)];
        let spaces = [(ObjectId(10), 2)];
        assert_eq!(partition_fast(&units, &spaces), vec![(ObjectId(2), ObjectId(10))]);
    }
}

/// `TCreateUnitsInfo`: one `teamUnitTypeN` / `teamUnitMinCountN` / `teamUnitMaxCountN` slot.
#[derive(Clone, Debug, PartialEq)]
pub struct TeamUnitInfo {
    pub template: String,
    pub min: i32,
    pub max: i32,
}

/// `TeamTemplateInfo` parsed from a `ScriptTeams` dict (and map team dicts).
#[derive(Clone, Debug, PartialEq)]
pub struct TeamPrototype {
    pub name: String,
    /// `teamOwner`.
    pub owner: String,
    pub units: Vec<TeamUnitInfo>,
    /// `teamHome` waypoint name.
    pub home: String,
    pub on_create: String,
    pub on_idle: String,
    pub initial_idle_frames: i32,
    pub on_enemy_sighted: String,
    pub on_all_clear: String,
    pub on_destroyed: String,
    pub on_unit_destroyed: String,
    pub production_priority: i32,
    pub production_priority_success: i32,
    pub production_priority_failure: i32,
    pub is_ai_recruitable: bool,
    pub auto_reinforce: bool,
    pub is_base_defense: bool,
    pub is_perimeter_defense: bool,
    pub execute_actions: bool,
    pub max_instances: i32,
    pub transport: String,
    pub generic_scripts: Vec<String>,
    pub production_condition: String,
    /// `teamIsSingleton`: a singleton team's instance is never removed, even when empty
    /// (`TeamPrototype::updateState`).
    pub is_singleton: bool,
    /// `teamAttackCommonTarget` (`TeamTemplateInfo::m_attackCommonTarget`, Team.cpp:723).
    pub attack_common_target: bool,
    /// `teamAggressiveness` (`TeamTemplateInfo::m_initialTeamAttitude`, Team.cpp:714): the
    /// `AttitudeType` a unit takes on when it joins the team (`Object::setTeam`); default normal (0).
    pub initial_attitude: i32,
    /// `TeamPrototype::m_attackPriorityName` (`TEAM_APPLY_ATTACK_PRIORITY_SET`, `Team::setAttackPriorityName`
    /// writes the prototype): members joining later take this set (`Object::setTeam`).
    pub attack_priority_name: String,
    /// `teamReinforcementOrigin` (`TeamTemplateInfo::m_startReinforceWaypoint`): where a reinforcement
    /// team's units and transport first appear (`doCreateReinforcements`).
    pub start_reinforce_waypoint: String,
    /// `teamStartsFull` (`m_teamStartsFull`): the reinforcement's units start loaded into its transports.
    pub starts_full: bool,
    /// `teamTransportsExit` (`m_transportsExit`): the transport leaves the map after unloading.
    pub transports_exit: bool,
}

impl TeamPrototype {
    /// Parses a team dict (`TeamTemplateInfo::TeamTemplateInfo`).
    pub fn from_dict(d: &Dict) -> Option<Self> {
        let name = d.str("teamName")?.to_owned();
        let mut units = Vec::new();
        for i in 1..=7 {
            let Some(template) = d.str(&format!("teamUnitType{i}")) else { continue };
            if template.is_empty() {
                continue;
            }
            let max = d.int(&format!("teamUnitMaxCount{i}")).unwrap_or(0);
            if max <= 0 {
                continue;
            }
            let min = d.int(&format!("teamUnitMinCount{i}")).unwrap_or(0);
            units.push(TeamUnitInfo { template: template.to_owned(), min, max });
        }
        // `teamGenericScriptHook0..MAX_GENERIC_SCRIPTS` (the first hook is index 0).
        let generic_scripts = (0..16)
            .map(|i| d.str(&format!("teamGenericScriptHook{i}")).unwrap_or("").to_owned())
            .filter(|s| !s.is_empty())
            .collect();
        Some(TeamPrototype {
            name,
            owner: d.str("teamOwner").unwrap_or("").to_owned(),
            units,
            home: d.str("teamHome").unwrap_or("").to_owned(),
            on_create: d.str("teamOnCreateScript").unwrap_or("").to_owned(),
            on_idle: d.str("teamOnIdleScript").unwrap_or("").to_owned(),
            initial_idle_frames: d.int("teamInitialIdleFrames").unwrap_or(0),
            on_enemy_sighted: d.str("teamEnemySightedScript").unwrap_or("").to_owned(),
            on_all_clear: d.str("teamAllClearScript").unwrap_or("").to_owned(),
            on_destroyed: d.str("teamOnDestroyedScript").unwrap_or("").to_owned(),
            on_unit_destroyed: d.str("teamOnUnitDestroyedScript").unwrap_or("").to_owned(),
            production_priority: d.int("teamProductionPriority").unwrap_or(0),
            production_priority_success: d.int("teamProductionPrioritySuccessIncrease").unwrap_or(0),
            production_priority_failure: d.int("teamProductionPriorityFailureDecrease").unwrap_or(0),
            is_ai_recruitable: d.get("teamIsAIRecruitable").and_then(bool_of).unwrap_or(false),
            auto_reinforce: d.get("teamAutoReinforce").and_then(bool_of).unwrap_or(false),
            is_base_defense: d.get("teamIsBaseDefense").and_then(bool_of).unwrap_or(false),
            is_perimeter_defense: d.get("teamIsPerimeterDefense").and_then(bool_of).unwrap_or(false),
            execute_actions: d.get("teamExecutesActionsOnCreate").and_then(bool_of).unwrap_or(false),
            max_instances: d.int("teamMaxInstances").unwrap_or(0),
            transport: d.str("teamTransport").unwrap_or("").to_owned(),
            generic_scripts,
            production_condition: d.str("teamProductionCondition").unwrap_or("").to_owned(),
            is_singleton: d.get("teamIsSingleton").and_then(bool_of).unwrap_or(false),
            attack_common_target: d.get("teamAttackCommonTarget").and_then(bool_of).unwrap_or(false),
            initial_attitude: d.int("teamAggressiveness").unwrap_or(0),
            attack_priority_name: String::new(),
            start_reinforce_waypoint: d.str("teamReinforcementOrigin").unwrap_or("").to_owned(),
            starts_full: d.get("teamStartsFull").and_then(bool_of).unwrap_or(false),
            transports_exit: d.get("teamTransportsExit").and_then(bool_of).unwrap_or(false),
        })
    }

    /// `TCreateUnitsInfo` totals (min/max units in the team).
    pub fn max_units(&self) -> i32 {
        self.units.iter().map(|u| u.max).sum()
    }
}

fn bool_of(v: &zr_map::DictValue) -> Option<bool> {
    match v {
        zr_map::DictValue::Bool(b) => Some(*b),
        zr_map::DictValue::Int(i) => Some(*i != 0),
        _ => None,
    }
}

/// `Team::TeamState` (the team orders a `TEAM_*` action can set).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TeamState {
    #[default]
    Idle,
    Guard,
    Hunt,
    Wander,
    /// Guard a trigger area / waypoint.
    GuardArea,
}

/// A live `Team` instance.
#[derive(Clone, Debug)]
pub struct TeamInstance {
    /// Lower-case prototype name.
    pub proto: String,
    /// `TeamFactory::m_uniqueTeamID` at creation. Retail prepends a new `Team` to the prototype's
    /// instance list (`Team.cpp:1328`), so iteration (and `getFirstItemIn_TeamInstanceList`) is
    /// newest-first; `seq` restores that order over the name-keyed `BTreeMap`.
    pub seq: u32,
    /// `Team::getControllingPlayer()` (`Team.cpp:1386`: the prototype's owning player). Fixed when
    /// the instance is created from the prototype's `teamOwner`, so moving an object between teams
    /// does not depend on which script is running.
    pub owner: Option<PlayerIndex>,
    /// `Team::m_TeamMemberList`, in the engine's order: `Object::setOrRestoreTeam` *prepends*
    /// (`Object.cpp:898`, `prependTo_TeamMemberList`), so the newest member is first and DLINK
    /// iteration (scripts, `getFirstItemIn_TeamMemberList`, `tryToRecruit` bookkeeping) runs newest
    /// to oldest.
    pub members: Vec<ObjectId>,
    pub created: bool,
    /// `Team::m_created` (`Team.cpp:1319`, `:310`): activated but its `teamOnCreateScript` has not
    /// run yet. `Team::updateState` clears it and runs the script (`Team.cpp:1812-1819`), which is at
    /// the end of `ScriptEngine::UPDATE` (`PlayerList::updateTeamStates`, ScriptEngine.cpp:5593),
    /// i.e. the frame *after* a team activated during the AI's update.
    pub created_pending: bool,
    pub state: TeamState,
    /// `OnEnemySighted` / `OnAllClear` latch.
    pub enemy_sighted: bool,
    /// `Team::m_wasIdle`: the team was idle last frame (`OnIdle` fires on the second idle frame).
    pub was_idle: bool,
    /// `OnDestroyed` fired once when the last member dies.
    pub destroyed_run: bool,
    /// `Team::m_isRecruitablitySet` + `m_isRecruitable` (`TEAM_AVAILABLE_FOR_RECRUITMENT`): `None`
    /// uses the prototype's `IsAIRecruitable`.
    pub recruitable: Option<bool>,
    /// `Team::m_commonAttackTarget` (Team.cpp:1320): the target the team's AI members share
    /// (`setTeamTargetObject` / `getTeamTargetObject`).
    pub common_attack_target: ObjectId,
    /// `!Team::m_shouldAttemptGenericScript[i]`: bit `i` set once hook `i` was a one-shot that fired
    /// (or its script is missing). One-shot is per team instance, not per script (Team.cpp:2541).
    pub generic_done: u32,
}

impl TeamInstance {
    /// An empty instance of a prototype (`TeamFactory::createTeam`). `owner` is the prototype's
    /// controlling player, filled in by the caller that knows it.
    pub fn new(name: &str) -> Self {
        TeamInstance {
            proto: name.to_ascii_lowercase(),
            seq: 0,
            owner: None,
            members: Vec::new(),
            created: false,
            created_pending: false,
            state: TeamState::Idle,
            enemy_sighted: false,
            was_idle: false,
            destroyed_run: false,
            recruitable: None,
            common_attack_target: ObjectId::INVALID,
            generic_done: 0,
        }
    }

    /// `Team::hasAnyObjects` reduced by the callers that need it: any live member at all.
    pub fn has_any_objects(&self, logic: &GameLogic) -> bool {
        self.members.iter().any(|id| logic.find_object(*id).is_some_and(|o| !o.is_destroyed()))
    }

    /// `Team::hasAnyUnits` (`Team.cpp:1727`): a live member that is not a structure, projectile or
    /// mine, and is not effectively dead (`isEffectivelyDead`, the port's `is_dying()`).
    pub fn has_any_units(&self, logic: &GameLogic) -> bool {
        use crate::template::kind_of;
        self.members.iter().any(|id| {
            logic.find_object(*id).is_some_and(|o| {
                if o.is_dying() || o.is_destroyed() {
                    return false;
                }
                if o.is_kind_of(kind_of::STRUCTURE) || o.is_kind_of(kind_of::PROJECTILE) || o.is_kind_of(kind_of::MINE) {
                    return false;
                }
                true
            })
        })
    }

    /// Live members of `template`.
    pub fn count_template(&self, logic: &GameLogic, template: &str) -> i32 {
        self.members
            .iter()
            .filter(|id| logic.find_object(**id).is_some_and(|o| o.template.name == template))
            .count() as i32
    }
}

/// `Team::tryToRecruit` (`Team.cpp:2334`): the closest object of `template` owned by `player` that
/// may be taken from its current team. Every object is in exactly one team; the engine walks
/// `TheGameLogic->getFirstObject()` (`m_objList`, `Object::prependToList` at `GameLogic.cpp:4013`,
/// so newest first) and reads `obj->getTeam()`:
/// - the team must be active (`Team::isActive`, `Team.cpp:2355`),
/// - its prototype's `ProductionPriority` must be strictly below the recruiting team's
///   (`Team.cpp:2358`),
/// - the player's default team is always recruitable; otherwise `teamIsAIRecruitable`, overridden by
///   a per-team `SetTeamRecruitable` (`Team.cpp:2361-2371`),
/// - the unit's `AIUpdateInterface::isRecruitable` and `DISABLED_HELD` (`Team.cpp:2372-2378`),
/// - distances (`Team.cpp:2379-2392`): the first default-team match is taken regardless of distance
///   (it then sets the bound), later candidates must be within `max_dist`, ties go to the object
///   iterated last (the oldest).
pub fn try_to_recruit(
    scripts: &ScriptEngine,
    logic: &GameLogic,
    player: PlayerIndex,
    template: &str,
    home: (f32, f32),
    max_dist: f32,
    my_priority: i32,
) -> Option<ObjectId> {
    let default_team = scripts.player_default_team(player);
    let mut dist_sqr = max_dist * max_dist;
    let mut recruit: Option<ObjectId> = None;
    let mut objs: Vec<&crate::object::Object> = logic.objects().collect();
    objs.reverse();
    for o in objs {
        if o.template.name != template || o.player != player {
            continue;
        }
        // `team->isActive()` and the prototype lookup: an object without a team cannot happen in the
        // engine (`Object::Object` falls back to the neutral default team, `Object.cpp:312`).
        let Some(team) = o.team.as_deref().and_then(|key| scripts.teams.get(key)) else { continue };
        if !team.created {
            continue; // Team is building, so don't steal its members yet.
        }
        let Some(proto) = scripts.team_proto(&team.proto) else { continue };
        let is_default = default_team == o.team.as_deref();
        if proto.production_priority >= my_priority {
            continue;
        }
        let mut recruitable = is_default || proto.is_ai_recruitable;
        if let Some(r) = team.recruitable {
            recruitable = r;
        }
        if !recruitable {
            continue;
        }
        // `AIUpdateInterface::isRecruitable` (Team.cpp:2372): cleared by the object-panel flag actions.
        if !o.ai.recruitable {
            continue;
        }
        if o.is_disabled_by_type(crate::disabled::DisabledType::Held) {
            continue;
        }
        let (dx, dy) = (home.0 - o.position.x, home.1 - o.position.y);
        let d = dx * dx + dy * dy;
        if is_default && recruit.is_none() {
            recruit = Some(o.id);
            dist_sqr = d;
        }
        if d > dist_sqr {
            continue;
        }
        dist_sqr = d;
        recruit = Some(o.id);
    }
    recruit
}

impl GameLogic {
    /// The enemy-sighted scan of `Team::updateState` (Team.cpp:1835): returns `(any_alive, sees_enemy)`.
    /// For every member that is not effectively dead, `getClosestObject(member, visionRange,
    /// FROM_CENTER_2D, {PartitionFilterRelationship(ALLOW_ENEMIES), PartitionFilterAlive,
    /// PartitionFilterSameMapStatus})`; the first hit ends the scan. No shroud filter is involved
    /// (retail has none here). Off-map status is not modelled, so `PartitionFilterSameMapStatus`
    /// always passes.
    pub(crate) fn team_sees_enemy(&self, members: &[ObjectId]) -> (bool, bool) {
        let mut any_alive = false;
        for id in members {
            let Some(o) = self.find_object(*id) else { continue };
            if o.is_dying() {
                continue;
            }
            any_alive = true;
            let range = o.template.vision_range.max(0.0);
            let seen = self
                .partition
                .objects_in_range(&o.position, range, crate::partition::DistanceMode::FromCenter2D)
                .into_iter()
                .filter(|other| other != id)
                .filter_map(|other| self.find_object(other))
                .any(|v| !v.is_dying() && self.relationship(o.player, v.player) == crate::damage::Relationship::Enemies);
            if seen {
                return (any_alive, true);
            }
        }
        (any_alive, false)
    }

    /// `Team::setActive` on the team `id` belongs to (Team.h:309: `m_created` and `m_active` once, no
    /// effect when already active). The sim's [`ScriptEngine::on_team_created`] is the activation, so
    /// the engine is taken out while it runs the team's create script.
    pub(crate) fn activate_team_of(&mut self, id: ObjectId) {
        let Some(key) = self.find_object(id).and_then(|o| o.team.clone()) else { return };
        if self.scripts.teams.get(&key).is_some_and(|t| t.created) {
            return;
        }
        let mut engine = std::mem::take(&mut self.scripts);
        engine.on_team_created(self, &key);
        self.scripts = engine;
    }
}
