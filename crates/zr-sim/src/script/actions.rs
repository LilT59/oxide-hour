//! Script action execution, phase 1 (`ScriptActions.cpp` + the control actions in
//! `ScriptEngine.cpp` `executeActions`).
//!
//! Implemented actions are listed in [`ScriptEngine::execute_action`]; every other type is a no-op
//! that increments [`ScriptEngine::warnings`].

use zr_map::scripts::Action;

use super::{pint, preal, pstr, ScriptEngine};
use crate::logic::GameLogic;

impl ScriptEngine {
    /// `ScriptEngine::executeActions` + `ScriptActions::executeAction`.
    pub(super) fn execute_action(&mut self, logic: &mut GameLogic, a: &Action) {
        self.actions_executed += 1;
        let name = a.name().unwrap_or("");
        let p = &a.params;
        match name {
            // ---- control ---------------------------------------------------------------
            "NO_OP" => {}
            "SET_FLAG" => self.set_flag(pstr(p, 0), pint(p, 1) != 0),
            "SET_COUNTER" => self.set_counter(pstr(p, 0), pint(p, 1)),
            "INCREMENT_COUNTER" => self.add_counter(pstr(p, 1), pint(p, 0)),
            "DECREMENT_COUNTER" => self.add_counter(pstr(p, 1), -pint(p, 0)),
            "SET_TIMER" => self.set_timer_frames(pstr(p, 0), pint(p, 1)),
            "SET_MILLISECOND_TIMER" => {
                let frames = (preal(p, 1) * 30.0).ceil() as i32;
                self.set_timer_frames(pstr(p, 0), frames);
            }
            "SET_RANDOM_TIMER" => {
                let v = logic.rng_mut().value(pint(p, 1), pint(p, 2));
                self.set_timer_frames(pstr(p, 0), v);
            }
            "SET_RANDOM_MSEC_TIMER" => {
                // `ScriptEngine::setTimer(pAction, true, true)` (`ScriptEngine.cpp:6742-6746`):
                // `GameLogicRandomValue(value, randomValue)` takes `int` arguments, so the seconds
                // truncate and the draw is an integer; the result converts to frames after.
                let v = logic.rng_mut().value(preal(p, 1) as i32, preal(p, 2) as i32) as f32;
                self.set_timer_frames(pstr(p, 0), (v * 30.0).ceil() as i32);
            }
            "ADD_TO_MSEC_TIMER" => self.add_counter(pstr(p, 1), (preal(p, 0) * 30.0).ceil() as i32),
            "SUB_FROM_MSEC_TIMER" => self.add_counter(pstr(p, 1), -((preal(p, 0) * 30.0).ceil() as i32)),
            "STOP_TIMER" => self.stop_timer(pstr(p, 0)),
            "RESTART_TIMER" => self.restart_timer(pstr(p, 0)),
            "ENABLE_SCRIPT" => self.enable_script(pstr(p, 0)),
            "DISABLE_SCRIPT" => self.disable_script(pstr(p, 0)),
            "CALL_SUBROUTINE" => self.call_subroutine(logic, pstr(p, 0)),

            // ---- game end (`ScriptActions::doVictory` / `doDefeat` / `doLocalDefeat`) ----------
            "VICTORY" => {
                if let Some(pl) = self.current() {
                    logic.script_victory(pl);
                }
            }
            "DEFEAT" | "LOCALDEFEAT" => {
                if let Some(pl) = self.current() {
                    logic.script_defeat(pl);
                }
            }

            // ---- audio (client-only; the sim has no music/speech subsystem) -------------------
            // `MUSIC_SET_TRACK` / `SPEECH_PLAY` drive the client's `AudioManager`; they have no
            // simulation effect, so they are deliberate no-ops here (not warnings).
            "MUSIC_SET_TRACK" | "SPEECH_PLAY" | "MUSIC_SET_VOLUME" => {}

            // ---- money -----------------------------------------------------------------
            // ---- scoring and rank (`ScriptActions.cpp`) ---------------------------------
            // `ScriptActions::enableScoring` -> `GameLogic::enableScoring` (ScriptActions.cpp:2890).
            "ENABLE_SCORING" => logic.set_scoring_enabled(true),
            "DISABLE_SCORING" => logic.set_scoring_enabled(false),
            // `doMapSetRankLevelLimit` -> `GameLogic::setRankLevelLimit` (ScriptActions.cpp:6058).
            "PLAYER_SET_RANKLEVELLIMIT" => logic.set_rank_level_limit(pint(p, 0)),
            // `doAffectSkillPointsModifier` -> `Player::setSkillPointsModifier` (ScriptActions.cpp:6469).
            "PLAYER_AFFECT_RECEIVING_EXPERIENCE" => {
                if let Some(pl) = self.player_from_param(pstr(p, 0)) {
                    logic.set_skill_points_modifier(pl, super::preal(p, 1));
                }
            }
            "PLAYER_GIVE_MONEY" => {
                if let Some(pl) = self.player_from_param(pstr(p, 0)) {
                    if let Some(player) = logic.players.get_mut(&pl) {
                        player.deposit(pint(p, 1).max(0) as u32);
                    }
                }
            }
            // `Player::sellEverythingUnderTheSun`: sell every structure (`sellBuildings`).
            "PLAYER_SELL_EVERYTHING" => {
                if let Some(pl) = self.player_from_param(pstr(p, 0)) {
                    let ids: Vec<crate::object::ObjectId> = logic
                        .objects()
                        .filter(|o| {
                            o.player == pl && !o.destroyed && o.is_kind_of(crate::template::kind_of::STRUCTURE)
                        })
                        .map(|o| o.id)
                        .collect();
                    for id in ids {
                        logic.submit(pl, crate::command::Command::Sell { building: id });
                    }
                }
            }

            // ---- building / AI ---------------------------------------------------------
            "SKIRMISH_BUILD_BUILDING" => self.build_specific(logic, pstr(p, 0)),
            "AI_PLAYER_BUILD_SUPPLY_CENTER" => {
                if let Some(pl) = self.player_from_param(pstr(p, 0)) {
                    logic.ai_build_by_supplies(pl, pint(p, 2), pstr(p, 1));
                }
            }
            "AI_PLAYER_BUILD_UPGRADE" => {
                // `doPlayerBuildUpgrade` -> `Player::buildUpgrade` -> `AIPlayer::buildUpgrade`
                // (ScriptActions.cpp:1131, AIPlayer.cpp:1778). That function refuses an *object* upgrade
                // (`curUpgrade->getUpgradeType() == UPGRADE_TYPE_OBJECT`, AIPlayer.cpp:1786-1794): only
                // `TEAM_USE_COMMANDBUTTON_ABILITY` can order one. The type comes from the command-button table
                // here because the engine is out of `GameLogic` while scripts run (`script_engine_update` takes
                // it), so `logic.scripts` cannot answer.
                if let Some(pl) = self.player_from_param(pstr(p, 0)) {
                    let upgrade = pstr(p, 1).to_owned();
                    let object = self
                        .command_button_upgrades
                        .values()
                        .any(|b| b.object && b.upgrade.eq_ignore_ascii_case(&upgrade));
                    logic.ai_build_upgrade_typed(pl, &upgrade, object);
                }
            }
            "AI_PLAYER_BUILD_TYPE_NEAREST_TEAM" => {
                // `doBuildObjectNearestTeam` (ScriptActions.cpp:1112-1122) -> `Player::
                // buildSpecificBuildingNearestTeam` (Player.cpp:2416) -> `AIPlayer::
                // buildSpecificBuildingNearestTeam` (AIPlayer.cpp:2109-2205). The team must exist on
                // both sides; its first member is `Team::getEstimateTeamPosition` (Team.cpp:2228).
                if crate::skirmish_ai::l174_nearest_diag() {
                    eprintln!(
                        "ZR_L174_ACT f={} p0={} tmpl={} team={}",
                        logic.frame, pstr(p, 0), pstr(p, 1), pstr(p, 2)
                    );
                }
                if let Some(pl) = self.player_from_param(pstr(p, 0)) {
                    let team = self.resolve_team(pstr(p, 2));
                    let loc = self
                        .team_members(logic, &team)
                        .first()
                        .copied()
                        .and_then(|id| logic.find_object(id))
                        .map(|o| o.position);
                    if let Some(loc) = loc {
                        logic.ai_build_specific_nearest_team(pl, pstr(p, 1), loc);
                    }
                }
            }
            "SKIRMISH_BUILD_STRUCTURE_FRONT" => self.build_base_structure(logic, pstr(p, 0), false),
            "SKIRMISH_BUILD_STRUCTURE_FLANK" => self.build_base_structure(logic, pstr(p, 0), true),
            "SKIRMISH_BUILD_BASE_DEFENSE_FRONT" => self.build_base_defense(logic, false),
            "SKIRMISH_BUILD_BASE_DEFENSE_FLANK" => self.build_base_defense(logic, true),
            // Construction is always enabled in the sim (phase-1 simplification).
            "PLAYER_ENABLE_BASE_CONSTRUCTION" | "PLAYER_ENABLE_UNIT_CONSTRUCTION" => {}

            // ---- object lists / attack priorities --------------------------------------
            "OBJECTLIST_ADDOBJECTTYPE" => self.add_object_list_type(pstr(p, 0), pstr(p, 1)),
            "OBJECTLIST_REMOVEOBJECTTYPE" => {
                if let Some(list) = self.object_lists.get_mut(pstr(p, 0)) {
                    list.retain(|t| t != pstr(p, 1));
                }
            }
            // `ScriptEngine::setPriorityThing`: the argument is an object-type list or one template.
            "SET_ATTACK_PRIORITY_THING" => {
                let set = pstr(p, 0).to_owned();
                let priority = pint(p, 2);
                match self.object_lists.get(pstr(p, 1)).cloned() {
                    Some(names) => {
                        for n in names {
                            self.set_attack_priority(&set, &n, priority);
                        }
                    }
                    None => self.set_attack_priority(&set, pstr(p, 1), priority),
                }
            }
            "SET_DEFAULT_ATTACK_PRIORITY" => self.set_default_attack_priority(pstr(p, 0), pint(p, 1)),
            // `ScriptEngine::setPriorityKind`: every template of the given `KindOf` index.
            "SET_ATTACK_PRIORITY_KIND_OF" => {
                let set = pstr(p, 0).to_owned();
                let priority = pint(p, 2);
                if let Some(names) = self.kind_of_templates.get(&pint(p, 1)).cloned() {
                    for n in names {
                        self.set_attack_priority(&set, &n, priority);
                    }
                }
            }

            // ---- teams -----------------------------------------------------------------
            "BUILD_TEAM" => {
                let name = pstr(p, 0).to_owned();
                // `doBuildTeam` -> `Player::buildSpecificTeam` -> `AIPlayer::buildSpecificAITeam`.
                if let Some(player) = self.team_owner(&name) {
                    self.build_specific_ai_team(logic, player, &name);
                }
            }
            "RECRUIT_TEAM" => {
                let team = pstr(p, 0).to_owned();
                if let Some(player) = self.team_owner(&team) {
                    self.recruit_specific_ai_team(logic, player, &team, preal(p, 1));
                }
            }
            // `doAffectObjectPanelFlagsUnit` / `...Team` -> `changeObjectPanelFlagForSingleObject`
            // (ScriptActions.cpp:273): only the "AI Recruitable" flag (`setIsRecruitable`) is ported;
            // the other panel flags (Enabled, Powered, Indestructible, ...) are not.
            "UNIT_AFFECT_OBJECT_PANEL_FLAGS" => {
                if pstr(p, 1) == "AI Recruitable" {
                    if let Some(id) = self.named_object(pstr(p, 0)) {
                        if let Some(o) = logic.objects.get_mut(&id) {
                            o.ai.recruitable = pint(p, 2) != 0;
                        }
                    }
                }
            }
            "TEAM_AFFECT_OBJECT_PANEL_FLAGS" => {
                if pstr(p, 1) == "AI Recruitable" {
                    let team = self.resolve_team(pstr(p, 0));
                    for id in self.team_members(logic, &team) {
                        if let Some(o) = logic.objects.get_mut(&id) {
                            o.ai.recruitable = pint(p, 2) != 0;
                        }
                    }
                }
            }
            "TEAM_AVAILABLE_FOR_RECRUITMENT" => {
                // `ScriptActions::doTeamAvailableForRecruitment`: `Team::setRecruitable` on the
                // named team instance (overrides the prototype's `IsAIRecruitable`).
                let key = self.resolve_team(pstr(p, 0));
                if let Some(t) = self.teams.get_mut(&key) {
                    t.recruitable = Some(pint(p, 1) != 0);
                }
            }
            "TEAM_HUNT" => {
                let team = self.resolve_team(pstr(p, 0));
                self.team_hunt(logic, &team, None);
            }
            "TEAM_HUNT_WITH_COMMAND_BUTTON" => {
                let (team, ability) = (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned());
                self.team_hunt_with_command_button(logic, &team, &ability);
            }
            "TEAM_ATTACK_AREA" => {
                let (team, area) = (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned());
                if let Some(t) = self.trigger_area(&area).cloned() {
                    let (cx, cy) = super::super_trigger_center(&t);
                    let r = super::super_trigger_radius(&t);
                    self.team_hunt(logic, &team, Some((crate::math::Coord3D::new(cx, cy, 0.0), r)));
                }
            }
            "TEAM_ATTACK_NAMED" => {
                let (team, target) = (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned());
                self.team_move_named(logic, &team, &target, true);
            }
            "TEAM_ENTER_NAMED" => {
                let (team, target) = (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned());
                self.team_move_named(logic, &team, &target, false);
            }
            "TEAM_MOVE_TOWARDS_NEAREST_OBJECT_TYPE" => {
                let (team, ty, area) = (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned(), pstr(p, 2).to_owned());
                self.team_move_towards_nearest(logic, &team, &ty, &area);
            }
            "TEAM_GUARD" => {
                let team = self.resolve_team(pstr(p, 0));
                let anchor = self.team_anchor(logic, &team);
                self.team_guard_point(logic, &team, anchor);
            }
            "TEAM_GUARD_POSITION" => {
                let (team, pos) = (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned());
                if let Some(p) = self.waypoints.get(&pos).copied() {
                    self.team_guard_point(logic, &team, p);
                }
            }
            "TEAM_GUARD_OBJECT" => {
                let (team, target) = (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned());
                if let Some(pos) = self.named_object(&target).and_then(|id| logic.find_object(id).map(|o| o.position)) {
                    self.team_guard_point(logic, &team, pos);
                }
            }
            "TEAM_GUARD_AREA" => {
                let (team, area) = (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned());
                self.team_guard_area(logic, &team, &area);
            }
            "TEAM_WANDER" => {
                let (team, area) = (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned());
                if let Some(t) = self.trigger_area(&area).cloned() {
                    let (cx, cy) = super::super_trigger_center(&t);
                    self.team_move(logic, &team, crate::math::Coord3D::new(cx, cy, 0.0), false);
                }
            }
            "TEAM_WANDER_IN_PLACE" => {
                let team = self.resolve_team(pstr(p, 0));
                let anchor = self.team_anchor(logic, &team);
                self.team_guard_point(logic, &team, anchor);
            }
            "TEAM_SET_STATE" => {
                // `doTeamSetState`: `getTeamNamed` -> the newest instance of the prototype.
                let key = self.resolve_team(pstr(p, 0));
                if let Some(t) = self.teams.get_mut(&key) {
                    t.state = match pint(p, 1) {
                        1 => crate::script::TeamState::Guard,
                        2 => crate::script::TeamState::Hunt,
                        3 => crate::script::TeamState::Wander,
                        _ => crate::script::TeamState::Idle,
                    };
                }
            }
            "TEAM_STOP" => {
                let team = self.resolve_team(pstr(p, 0));
                self.team_stop(logic, &team, false);
            }
            "TEAM_STOP_AND_DISBAND" => {
                let team = self.resolve_team(pstr(p, 0));
                self.team_stop(logic, &team, true);
            }
            // `doTeamExitAll`: every member evacuates its container (`groupEvacuate`).
            "TEAM_EXIT_ALL" => {
                let team = self.resolve_team(pstr(p, 0));
                self.team_exit_all(logic, &team);
            }
            // `updateTeamSetAttitude`: store the team's `AttitudeType`.
            "TEAM_SET_ATTITUDE" => {
                let team = self.resolve_team(pstr(p, 0));
                self.team_set_attitude(logic, &team, pint(p, 1));
            }
            // `doTeamGuardInTunnelNetwork`: the `AITNGuard` AI is not ported, so the team moves
            // into the nearest own tunnel network (the containing half of that behaviour).
            "TEAM_GUARD_IN_TUNNEL_NETWORK" => {
                let team = self.resolve_team(pstr(p, 0));
                self.team_guard_tunnel_network(logic, &team);
            }
            // `ScriptActions::updateNamedAttackPrioritySet` (ScriptActions.cpp:1222).
            "NAMED_APPLY_ATTACK_PRIORITY_SET" => {
                if let Some(id) = self.named_object(pstr(p, 0)) {
                    let set = pstr(p, 1);
                    let defined = self.attack_priorities.contains_key(set);
                    logic.set_unit_attack_info(id, Some(if defined { set.to_owned() } else { String::new() }));
                }
            }
            "TEAM_APPLY_ATTACK_PRIORITY_SET" => {
                // `ScriptActions::updateTeamAttackPrioritySet` (ScriptActions.cpp:1242): a defined set
                // is stored on the team's prototype; every member's AI takes the set (an unknown name
                // is the default info).
                let key = self.resolve_team(pstr(p, 0));
                if self.teams.contains_key(&key) {
                    let set = pstr(p, 1).to_owned();
                    let defined = self.attack_priorities.contains_key(&set);
                    if defined {
                        let proto = self.proto_of(&key);
                        if let Some(pr) = self.team_protos.get_mut(&proto) {
                            pr.attack_priority_name = set.clone();
                        }
                    }
                    for id in self.team_members(logic, &key) {
                        logic.set_unit_attack_info(id, Some(if defined { set.clone() } else { String::new() }));
                    }
                }
            }
            "TEAM_INCREASE_PRIORITY" => self.team_increase_priority(pstr(p, 0)),
            "TEAM_DECREASE_PRIORITY" => self.team_decrease_priority(pstr(p, 0)),
            "TEAM_MERGE_INTO_TEAM" => {
                // `doMergeTeamIntoTeam`: `getTeamNamed` on both names; every member joins the
                // destination through `Object::setTeam` (attitude included).
                let (src, dst) = (self.resolve_team(pstr(p, 0)), self.resolve_team(pstr(p, 1)));
                if let Some(dst) = self.team_merge(logic, &src, &dst) {
                    // `teamDest->setActive()`.
                    if self.teams.get(&dst).is_some_and(|t| !t.created) {
                        self.on_team_created(logic, &dst);
                    }
                }
            }
            "SKIRMISH_ATTACK_NEAREST_GROUP_WITH_VALUE" => {
                let team = self.resolve_team(pstr(p, 0));
                let (comparison, value) = (pint(p, 1), pint(p, 2));
                self.skirmish_attack_nearest_group_with_value(logic, &team, comparison, value);
            }
            // `doTeamMoveToSkirmishApproachPath`: move to the closest waypoint on the enemy's path.
            "SKIRMISH_MOVE_TO_APPROACH_PATH" => {
                let (team, label) = (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned());
                self.team_follow_approach_path(logic, &team, &label, false);
            }
            // `doTeamFollowSkirmishApproachPath`: follow the linked path from that waypoint.
            "SKIRMISH_FOLLOW_APPROACH_PATH" => {
                let (team, label) = (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned());
                self.team_follow_approach_path(logic, &team, &label, true);
            }
            "TEAM_GARRISON_NEAREST_BUILDING" => {
                let team = self.resolve_team(pstr(p, 0));
                self.team_garrison_nearest_building(logic, &team);
            }
            // `ScriptActions::doSkirmishFireSpecialPowerAtMostCost` (`ScriptActions.cpp:4232`): the
            // enemy is `getSkirmishEnemyPlayer()` (the current script player's enemy), the player is
            // the action's first parameter. The engine is passed in: the target search reads
            // `Player::getMpStartIndex` / `getClosestWaypointOnPath` from it, and the fire records its
            // `PLAYER_TRIGGERED/COMPLETED_SPECIAL_POWER` events on it (`logic.scripts` is empty while a
            // script action runs).
            "SKIRMISH_FIRE_SPECIAL_POWER_AT_MOST_COST" => {
                let player = self.player_from_param(pstr(p, 0));
                let enemy = self.current().and_then(|c| self.player_enemy.get(&c).copied());
                *self.special_power_action_counts.entry(pstr(p, 1).to_owned()).or_default() += 1;
                if let Some(player) = player {
                    let power = pstr(p, 1).to_owned();
                    logic.ai_fire_special_power_at_most_cost(player, enemy, &power, self);
                }
            }
            "SKIRMISH_PERFORM_COMMANDBUTTON_ON_MOST_VALUABLE_OBJECT" => self.warn(name),

            // The nearest enemy-or-neutral unmanned object (`doTeamCaptureNearestUnownedFactionUnit`).
            "TEAM_CAPTURE_NEAREST_UNOWNED_FACTION_UNIT" => {
                let team = self.resolve_team(pstr(p, 0));
                self.team_capture_nearest_unowned(logic, &team);
            }
            // `doTeamUseCommandButtonAbility` (ScriptActions.cpp:4463): every member, no target.
            "TEAM_USE_COMMANDBUTTON_ABILITY" => {
                let (team, ability) = (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned());
                self.team_use_command_button_ability(logic, &team, &ability);
            }
            // `doTeamUseCommandButtonAbilityOnNamed` (ScriptActions.cpp:4495).
            "TEAM_USE_COMMANDBUTTON_ABILITY_ON_NAMED" => {
                let (team, ability, unit) =
                    (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned(), pstr(p, 2).to_owned());
                self.team_use_command_button_ability_on_named(logic, &team, &ability, &unit);
            }
            // `doTeamUseCommandButtonOnNamed` (ScriptActions.cpp:5550).
            "TEAM_ALL_USE_COMMANDBUTTON_ON_NAMED" => {
                let (team, ability, unit) =
                    (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned(), pstr(p, 2).to_owned());
                self.team_use_command_button_on_named(logic, &team, &ability, &unit);
            }
            // `doTeamUseCommandButtonOnNearest*` (ScriptActions.cpp:5591-5945): the group centre and
            // the `PartitionFilterValidCommandButtonTarget` filter pick the target; see command_button.rs.
            "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_ENEMY_UNIT" | "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_GARRISONED_BUILDING"
            | "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_KINDOF" | "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_ENEMY_BUILDING"
            | "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_ENEMY_BUILDING_CLASS" | "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_OBJECTTYPE" => {
                use super::command_button::TargetScope;
                let team = self.resolve_team(pstr(p, 0));
                let ability = pstr(p, 1).to_owned();
                let scope = match name {
                    "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_GARRISONED_BUILDING" => TargetScope::GarrisonedBuilding,
                    "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_KINDOF" => TargetScope::KindOf(pint(p, 2)),
                    "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_ENEMY_BUILDING" => TargetScope::EnemyBuilding,
                    "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_ENEMY_BUILDING_CLASS" => {
                        TargetScope::EnemyBuildingClass(pint(p, 2))
                    }
                    "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_OBJECTTYPE" => TargetScope::ObjectType(pstr(p, 2).to_owned()),
                    _ => TargetScope::EnemyUnit,
                };
                self.team_use_command_button_nearest(logic, &team, &ability, &scope);
            }
            // `doTeamPartialUseCommandButton` (ScriptActions.cpp:5949): (percentage, team, button).
            "TEAM_PARTIAL_USE_COMMANDBUTTON" => {
                let (percentage, team, ability) = (preal(p, 0), self.resolve_team(pstr(p, 1)), pstr(p, 2).to_owned());
                self.team_partial_use_command_button(logic, &team, percentage, &ability);
            }
            // `doTeamUseCommandButtonAbilityAtWaypoint` (ScriptActions.cpp:4533).
            "TEAM_USE_COMMANDBUTTON_ABILITY_AT_WAYPOINT" => {
                let (team, ability, waypoint) =
                    (self.resolve_team(pstr(p, 0)), pstr(p, 1).to_owned(), pstr(p, 2).to_owned());
                self.team_use_command_button_ability_at_waypoint(logic, &team, &ability, &waypoint);
            }
            // Load the team's non-transport members into its `KINDOF_TRANSPORT` members
            // (`ScriptActions::doLoadAllTransports`, distributed by transport slot count).
            "TEAM_LOAD_TRANSPORTS" => {
                // `ScriptActions::doLoadAllTransports` (`ScriptActions.cpp:1473-1520`): members with
                // `KINDOF_TRANSPORT` and a contain module are the spaces (`getContainMax`); every other
                // member is an entry sized by `getTransportSlotCount`. Both go to `PartitionSolver`.
                use crate::template::kind_of;
                let team = self.resolve_team(pstr(p, 0));
                let Some(player) = self.team_owner(&team) else { return };
                let ids = self.team_members(logic, &team);
                let mut units: Vec<(crate::object::ObjectId, u32)> = Vec::new();
                let mut spaces: Vec<(crate::object::ObjectId, u32)> = Vec::new();
                for id in ids {
                    let Some(o) = logic.find_object(id) else { continue };
                    if o.template.is_kind_of(kind_of::TRANSPORT) {
                        if let Some(c) = o.template.contain.as_ref() {
                            let max = if c.capacity < 0 { u32::MAX } else { c.capacity as u32 };
                            spaces.push((id, max));
                        }
                    } else {
                        units.push((id, o.template.transport_slot_count));
                    }
                }
                // `chooseLocomotorSet(LOCOMOTORSET_NORMAL)` then `aiEnter(trans, CMD_FROM_SCRIPT)` for
                // each solution entry, in solution order (ScriptActions.cpp:1515-1525); units without
                // an AI are skipped (`getAIUpdateInterface`).
                for (unit, trans) in crate::script::team::partition_fast(&units, &spaces) {
                    if logic.find_object(unit).is_some_and(|o| o.ai_handle.is_some()) {
                        logic.ai_choose_locomotor_set(unit, crate::template::LocomotorSet::Normal);
                        logic.command_enter_from(player, &[unit], trans, crate::ai_states::CommandSource::Script);
                    }
                }
            }

            "TEAM_DELETE" | "TEAM_DELETE_LIVING" => self.warn(name),

            // ---- named units (`unit_actions.rs`) ----------------------------------------------
            "NAMED_ATTACK_NAMED" => self.named_attack_named(logic, pstr(p, 0), pstr(p, 1)),
            "NAMED_DELETE" => self.named_delete(logic, pstr(p, 0)),
            "NAMED_KILL" => self.named_kill(logic, pstr(p, 0)),
            "NAMED_SET_HELD" => self.named_set_held(logic, pstr(p, 0), pint(p, 1) != 0),
            "NAMED_EXIT_ALL" => self.named_exit_all(logic, pstr(p, 0)),
            "MOVE_NAMED_UNIT_TO" => self.named_move_to_waypoint(logic, pstr(p, 0), pstr(p, 1)),
            "NAMED_FOLLOW_WAYPOINTS" => self.named_follow_waypoints(logic, pstr(p, 0), pstr(p, 1)),
            // `doTeamFollowWaypoints(team, label, asTeam)`: the as-team flag does not change the walk
            // (see `team_follow_waypoints`).
            "TEAM_FOLLOW_WAYPOINTS" => {
                let team = self.resolve_team(pstr(p, 0));
                self.team_follow_waypoints(logic, &team, pstr(p, 1));
            }
            "TEAM_PANIC" => {
                let team = self.resolve_team(pstr(p, 0));
                self.team_panic(logic, &team, pstr(p, 1));
            }

            // ---- created units and reinforcements (`reinforce.rs`) ----------------------------
            "CREATE_NAMED_ON_TEAM_AT_WAYPOINT" => {
                self.create_unit_on_team_at(logic, pstr(p, 0), pstr(p, 1), pstr(p, 2), pstr(p, 3));
            }
            "CREATE_UNNAMED_ON_TEAM_AT_WAYPOINT" => {
                self.create_unit_on_team_at(logic, "", pstr(p, 0), pstr(p, 1), pstr(p, 2));
            }
            "CREATE_REINFORCEMENT_TEAM" => self.create_reinforcements(logic, pstr(p, 0), pstr(p, 1)),

            // Sequential scripts and their frame waits (`sequential.rs`).
            _ => {
                if !self.execute_sequential_action(logic, name, p) {
                    self.warn(name);
                }
            }
        }
    }

    fn current(&self) -> Option<crate::object::PlayerIndex> {
        self.current_player()
    }

    fn build_specific(&mut self, logic: &mut GameLogic, template: &str) {
        if let Some(pl) = self.current() {
            logic.ai_build_specific(pl, template);
        }
    }

    fn build_base_structure(&mut self, logic: &mut GameLogic, template: &str, flank: bool) {
        if let Some(pl) = self.current() {
            // The engine is out of `GameLogic` while scripts run, so hand it in for the approach-path lookup
            // (`TheTerrainLogic->getClosestWaypointOnPath` + `Player::getMpStartIndex`, AISkirmishPlayer.cpp:612-625).
            logic.ai_build_base_structure(pl, template, flank, self);
        }
    }

    fn build_base_defense(&mut self, logic: &mut GameLogic, flank: bool) {
        if let Some(pl) = self.current() {
            logic.ai_build_base_defense(pl, flank, self);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use zr_map::scripts::{Action, Condition, OrCondition, ParamValue, Parameter, Script, ScriptList};

    use crate::logic::GameLogic;
    use crate::math::Coord3D;
    use crate::script::CommandButtonUpgrade;
    use crate::{ObjectTemplate, TemplateSource, TerrainLogic};

    /// A factory whose command set holds a player-upgrade button (`Cmd_X` -> `Upgrade_X`) and an object-upgrade
    /// button (`Cmd_Y` -> `Upgrade_Y`), owned by an AI player that lists it as built (`m_player->getBuildList()`,
    /// `AIPlayer.cpp:1819`).
    fn upgrade_world() -> (GameLogic, crate::object::ObjectId) {
        struct Sets;
        impl TemplateSource for Sets {
            fn find_template(&self, _: &str) -> Option<Arc<ObjectTemplate>> {
                None
            }
            fn can_build_upgrade(&self, builder: &str, _: &str) -> Option<bool> {
                Some(builder == "Plant")
            }
        }
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        g.add_player(0, 1000);
        g.set_upgrade_info(
            [
                ("Upgrade_X".to_string(), crate::upgrade::UpgradeInfo { cost: 100, build_time_frames: 10 }),
                ("Upgrade_Y".to_string(), crate::upgrade::UpgradeInfo { cost: 100, build_time_frames: 10 }),
            ]
            .into_iter()
            .collect(),
        );
        g.set_template_source(Arc::new(Sets));
        g.scripts_mut().load_command_button_upgrades([
            ("Cmd_X".to_owned(), CommandButtonUpgrade { upgrade: "Upgrade_X".into(), object: false }),
            ("Cmd_Y".to_owned(), CommandButtonUpgrade { upgrade: "Upgrade_Y".into(), object: true }),
        ]);
        let mut t = ObjectTemplate::new("Plant");
        t.production = Some(crate::production::ProductionTemplate {
            max_queue_entries: 5,
            quantity_modifiers: vec![],
            exit: crate::production::ProductionExit::default(),
            ..Default::default()
        });
        let plant = g.create_object(&Arc::new(t), Coord3D::new(50.0, 50.0, 0.0), 0.0, 0);
        g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
        g.ai_players.get_mut(&0).unwrap().built = vec![Some(plant)];
        (g, plant)
    }

    /// Runs one `AI_PLAYER_BUILD_UPGRADE` action for player 0 through the script engine (which is out of
    /// `GameLogic` while it runs, as in `script_engine_update`) and lets the frame end.
    fn run_build_upgrade(g: &mut GameLogic, upgrade: &str) {
        let param = |s: &str| Parameter { kind: 0, value: ParamValue::Plain { int: 0, real: 0.0, string: s.to_owned() } };
        let script = Script {
            version: 2,
            name: "build_upgrade".into(),
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
            actions: vec![Action {
                version: 2,
                type_id: 0,
                internal_name: Some("AI_PLAYER_BUILD_UPGRADE".into()),
                params: vec![param("<This Player>"), param(upgrade)],
            }],
            actions_false: Vec::new(),
        };
        g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![script], groups: Vec::new() })]);
        g.run_frames(2);
    }

    /// `AIPlayer::recruitSpecificAITeam` (AIPlayer.cpp:2572, :2638): every recruit gets `aiMoveToPosition(&m_homeLocation,
    /// CMD_FROM_AI)`, and `m_homeLocation` is zero when the team has no home, so a recruit of a team without a home is
    /// still sent to the origin. Revert check: move only when a home waypoint exists (the old `if let Some(home)`) and
    /// the recruit stays `Idle`, so the assert fails.
    #[test]
    fn recruit_team_moves_each_recruit_to_the_home_or_the_origin() {
        struct Store(Vec<Arc<ObjectTemplate>>);
        impl TemplateSource for Store {
            fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
                self.0.iter().find(|t| t.name == name).cloned()
            }
        }
        let mut truck = ObjectTemplate::new("SupplyTruck");
        truck.max_health = 100.0;
        truck.initial_health = 100.0;
        truck.geometry.major_radius = 5.0;
        truck.locomotor = Some(crate::template::LocomotorParams::from_ini(crate::template::LocomotorKind::Treads, 60.0, 100.0, 100.0, 360.0));
        let mut g = GameLogic::new(TerrainLogic::flat(200, 200, 0), 1);
        g.add_player(0, 1000);
        g.set_template_source(Arc::new(Store(vec![Arc::new(truck.clone())])));
        g.scripts_mut().set_player_name("p0", 0);
        g.scripts_mut().load_default_team_protos(&[("teamP0", "p0")]);
        g.init_from_sides(&[("teamP0", 0)]);
        // A recruiting team of higher priority than the player's default team (whose units are always recruitable).
        let recruits = zr_map::Dict(vec![
            ("teamName".into(), zr_map::DictValue::Ascii("Recruits".into())),
            ("teamOwner".into(), zr_map::DictValue::Ascii("p0".into())),
            ("teamIsSingleton".into(), zr_map::DictValue::Bool(false)),
            ("teamProductionPriority".into(), zr_map::DictValue::Int(50)),
            ("teamUnitType1".into(), zr_map::DictValue::Ascii("SupplyTruck".into())),
            ("teamUnitMinCount1".into(), zr_map::DictValue::Int(1)),
            ("teamUnitMaxCount1".into(), zr_map::DictValue::Int(1)),
        ]);
        g.scripts_mut().load_teams(&[recruits]);
        let free = g.create_object(&Arc::new(truck), Coord3D::new(60.0, 60.0, 0.0), 0.0, 0);
        g.set_object_team(free, 0);
        let param = |s: &str| Parameter { kind: 0, value: ParamValue::Plain { int: 0, real: 0.0, string: s.to_owned() } };
        let script = Script {
            version: 2,
            name: "recruit".into(),
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
            actions: vec![Action {
                version: 2,
                type_id: 0,
                internal_name: Some("RECRUIT_TEAM".into()),
                params: vec![param("Recruits"), Parameter { kind: 0, value: ParamValue::Plain { int: 0, real: 0.0, string: String::new() } }],
            }],
            actions_false: Vec::new(),
        };
        g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![script], groups: Vec::new() })]);
        g.run_frames(2);
        let o = g.find_object(free).expect("the truck");
        assert_eq!(o.ai.machine.current, crate::ai_states::AiStateType::MoveTo, "the recruit is sent home");
        assert_eq!((o.ai.machine.goal_position.x, o.ai.machine.goal_position.y), (0.0, 0.0), "to the origin: the team has no home");
    }

    /// `AIPlayer::buildUpgrade` (AIPlayer.cpp:1786-1794): an `UPGRADE_TYPE_OBJECT` request is refused ("is an object,
    /// not a player upgrade"), so `AI_PLAYER_BUILD_UPGRADE` (`ScriptActions.cpp:1131`) queues nothing for
    /// `Upgrade_Y`; the player upgrade `Upgrade_X` from the same factory is queued. The upgrade type is read from
    /// the command-button table, which the script action has to do itself: the engine is out of `GameLogic` while
    /// scripts run (`script_engine_update` takes it), so the AI's own lookup sees an empty table.
    #[test]
    fn ai_player_build_upgrade_ignores_an_object_upgrade() {
        let (mut g, plant) = upgrade_world();
        run_build_upgrade(&mut g, "Upgrade_Y");
        let queue = &g.find_object(plant).unwrap().production.as_ref().unwrap().queue;
        assert!(queue.is_empty(), "an object upgrade is not a player upgrade: {queue:?}");
        assert_eq!(g.player(0).unwrap().money, 1000, "nothing is paid");
        run_build_upgrade(&mut g, "Upgrade_X");
        let queue = &g.find_object(plant).unwrap().production.as_ref().unwrap().queue;
        assert_eq!(queue.len(), 1, "the player upgrade is queued");
        assert_eq!(g.player(0).unwrap().money, 900, "and paid for");
    }

    /// A world for `SKIRMISH_BUILD_STRUCTURE_FRONT`: player 0's AI base is the Plant at (100,100), and the map has
    /// the approach paths `Center1` (north of the base) and `Center2` (east of it). `player_start[0] = 1`, so the
    /// action must aim at `Center2` (`Player::getMpStartIndex() + 1`, AISkirmishPlayer.cpp:616-625).
    fn base_world(start: i32) -> GameLogic {
        struct Store(Vec<Arc<ObjectTemplate>>);
        impl TemplateSource for Store {
            fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
                self.0.iter().find(|t| t.name == name).cloned()
            }
        }
        let mut plant = ObjectTemplate::new("Plant");
        plant.kind_of = crate::template::kind_of::STRUCTURE | crate::template::kind_of::IMMOBILE;
        plant.geometry.major_radius = 8.0;
        let mut defense = ObjectTemplate::new("Defense");
        defense.kind_of = crate::template::kind_of::STRUCTURE | crate::template::kind_of::IMMOBILE;
        defense.geometry.major_radius = 8.0;
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        g.set_template_source(Arc::new(Store(vec![Arc::new(plant), Arc::new(defense)])));
        g.scripts_mut().set_player_meta(0, "Side", start);
        g.scripts_mut().set_waypoint_paths(
            [
                (1, "Center1Waypoint".to_owned(), Coord3D::new(100.0, 300.0, 0.0), vec!["Center1".to_owned()]),
                (2, "Center2Waypoint".to_owned(), Coord3D::new(300.0, 100.0, 0.0), vec!["Center2".to_owned()]),
            ],
            [],
        );
        g.add_ai_player(
            0,
            crate::skirmish_ai::SkirmishAiConfig {
                build_list: vec![crate::skirmish_ai::AiBuildOrder {
                    template: "Plant".into(),
                    location: (100.0, 100.0),
                    angle: 0.0,
                    initially_built: true,
                    rebuilds: 1,
                    automatically_build: true,
                }],
                base_defense_extra_distance: 40.0,
                ..Default::default()
            },
        );
        g
    }

    /// Runs one `SKIRMISH_BUILD_STRUCTURE_FRONT[template]` action for player 0 through the script engine (out of
    /// `GameLogic` while it runs) and returns the priority build order the action prepended.
    fn run_build_structure_front(g: &mut GameLogic, template: &str) -> (f32, f32) {
        let param = Parameter { kind: 0, value: ParamValue::Plain { int: 0, real: 0.0, string: template.to_owned() } };
        let script = Script {
            version: 2,
            name: "build_front".into(),
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
            actions: vec![Action {
                version: 2,
                type_id: 0,
                internal_name: Some("SKIRMISH_BUILD_STRUCTURE_FRONT".into()),
                params: vec![param],
            }],
            actions_false: Vec::new(),
        };
        g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![script], groups: Vec::new() })]);
        g.run_frames(2);
        g.ai_players.get(&0).unwrap().config.build_list[0].location
    }

    /// `AISkirmishPlayer::buildAIBaseDefenseStructure` (`AISkirmishPlayer.cpp:599-700`) aims the defense at the
    /// closest waypoint of the player's own approach path: `Center<Player::getMpStartIndex() + 1>`, looked up
    /// through `TheTerrainLogic->getClosestWaypointOnPath`. The path data lives in the script engine, which is out
    /// of `GameLogic` while a script action runs, so it has to be handed in: reading `logic.scripts` there found
    /// no path at all and every defense landed off-path (AN3 seed 1: the GLA tunnel network at (881,1802) instead
    /// of the C++'s (1816,1745)).
    #[test]
    fn build_structure_front_uses_the_players_own_approach_path() {
        // start 1 -> Center2 (east of the base at (100,100)); d = base_radius + 40.
        let mut g = base_world(1);
        let pos = run_build_structure_front(&mut g, "Defense");
        let base_radius = g.ai_players.get(&0).unwrap().base_radius;
        assert!(pos.0 > 100.0 + base_radius + 39.0 && pos.0 < 100.0 + base_radius + 41.0, "at {pos:?}");
        assert!((pos.1 - 100.0).abs() < 0.01, "the Center2 path points east: {pos:?}");
    }

    /// The other start index picks `Center1` (north), which is the label bug this guards: a stale or empty
    /// `player_start` map falls back to `<label>1` on both sides.
    #[test]
    fn build_structure_front_follows_the_start_index() {
        let mut g = base_world(0);
        let pos = run_build_structure_front(&mut g, "Defense");
        let base_radius = g.ai_players.get(&0).unwrap().base_radius;
        assert!(pos.1 > 100.0 + base_radius + 39.0 && pos.1 < 100.0 + base_radius + 41.0, "at {pos:?}");
        assert!((pos.0 - 100.0).abs() < 0.01, "the Center1 path points north: {pos:?}");
    }

    /// A world for `SKIRMISH_FIRE_SPECIAL_POWER_AT_MOST_COST` with a cluster-mine power. Player 0's AI base is
    /// the Plant at (100,100); `player_start[0] = 1`, so the power must mine the `Center2` approach path (east of
    /// the base) and never `Center1` (north). The power's effect creates a `MineMarker` at the location it fires
    /// at, which is the cluster-mine ring point the action computed.
    fn cluster_mine_world() -> GameLogic {
        struct Store(Vec<Arc<ObjectTemplate>>);
        impl TemplateSource for Store {
            fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
                self.0.iter().find(|t| t.name == name).cloned()
            }
        }
        let mut plant = ObjectTemplate::new("Plant");
        plant.kind_of = crate::template::kind_of::STRUCTURE | crate::template::kind_of::IMMOBILE;
        plant.geometry.major_radius = 8.0;
        let marker = Arc::new(ObjectTemplate::new("MineMarker"));
        let mut source = ObjectTemplate::new("MineLayer");
        source.special_powers = vec![Arc::new(crate::special_power::SimSpecialPower {
            power: "ClusterMines".into(),
            effect: crate::special_power::SpecialPowerEffect::Baikonur { detonation_object: Some(marker.clone()) },
            starts_paused: false,
            power_module: true,
        })];
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        g.add_player(1, 1000);
        g.set_template_source(Arc::new(Store(vec![Arc::new(plant), marker, Arc::new(source)])));
        g.scripts_mut().set_player_meta(0, "Side", 1);
        g.scripts_mut().set_player_enemy(0, 1);
        g.scripts_mut().set_waypoint_paths(
            [
                (
                    1,
                    "Center1Waypoint".to_owned(),
                    Coord3D::new(100.0, 300.0, 0.0),
                    vec!["Center1".to_owned(), "Flank1".to_owned(), "Backdoor1".to_owned()],
                ),
                (
                    2,
                    "Center2Waypoint".to_owned(),
                    Coord3D::new(300.0, 100.0, 0.0),
                    vec!["Center2".to_owned(), "Flank2".to_owned(), "Backdoor2".to_owned()],
                ),
            ],
            [],
        );
        g.set_special_powers(
            [(
                "ClusterMines".to_string(),
                Arc::new(crate::special_power::SimSpecialPowerTemplate {
                    name: "ClusterMines".into(),
                    kind: "SPECIAL_CLUSTER_MINES".into(),
                    reload_time: 0,
                    required_science: None,
                    shared_synced_timer: false,
                    public_timer: false,
                    radius_cursor_radius: 0.0,
                }),
            )]
            .into_iter()
            .collect(),
        );
        g.add_ai_player(
            0,
            crate::skirmish_ai::SkirmishAiConfig {
                build_list: vec![crate::skirmish_ai::AiBuildOrder {
                    template: "Plant".into(),
                    location: (100.0, 100.0),
                    angle: 0.0,
                    initially_built: true,
                    rebuilds: 1,
                    automatically_build: true,
                }],
                ..Default::default()
            },
        );
        let t = g.find_template("MineLayer").unwrap();
        g.create_object(&t, Coord3D::new(100.0, 120.0, 0.0), 0.0, 0);
        g
    }

    fn string_param(s: &str) -> Parameter {
        Parameter { kind: 0, value: ParamValue::Plain { int: 0, real: 0.0, string: s.to_owned() } }
    }

    fn int_param(v: i32) -> Parameter {
        Parameter { kind: 0, value: ParamValue::Plain { int: v, real: 0.0, string: String::new() } }
    }

    fn simple_script(name: &str, conditions: Vec<Condition>, actions: Vec<Action>) -> Script {
        Script {
            version: 2,
            name: name.into(),
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
            or_conditions: vec![OrCondition { conditions }],
            actions,
            actions_false: Vec::new(),
        }
    }

    fn true_condition() -> Condition {
        Condition { version: 4, type_id: 0, internal_name: Some("CONDITION_TRUE".into()), params: Vec::new() }
    }

    fn action(name: &str, params: Vec<Parameter>) -> Action {
        Action { version: 2, type_id: 0, internal_name: Some(name.into()), params }
    }

    /// `AISkirmishPlayer::computeSuperweaponTarget`'s cluster-mine branch (`AISkirmishPlayer.cpp:1134-1175`)
    /// mines the entrances to *our own* base: it builds `Center/Flank/Backdoor<Player::getMpStartIndex()+1>`
    /// and asks `TheTerrainLogic->getClosestWaypointOnPath`. Both tables live in the script engine, which is out
    /// of `GameLogic` while a script action runs, so the action has to hand its engine in (lane 144's pattern).
    /// Reading `logic.scripts` found no start index (`<label>1`) and no path, so every cluster mine was dropped
    /// on the enemy base centre instead of the player's own approach path.
    #[test]
    fn skirmish_fire_special_power_mines_its_own_approach_path() {
        let mut g = cluster_mine_world();
        let base_radius = g.ai_player(0).unwrap().base_radius;
        assert!(base_radius > 0.0);
        g.scripts_mut().load(vec![(
            0,
            ScriptList {
                scripts: vec![simple_script(
                    "mines",
                    vec![true_condition()],
                    vec![action("SKIRMISH_FIRE_SPECIAL_POWER_AT_MOST_COST", vec![string_param("<This Player>"), string_param("ClusterMines")])],
                )],
                groups: Vec::new(),
            },
        )]);
        g.run_frames(2);
        let marker = g.objects().find(|o| o.template.name == "MineMarker").expect("the power fired and created its marker");
        // The ring point is the base centre plus `base_radius` towards the `Center2` waypoint (east):
        // (100,100) -> (300,100). The unfixed read fell back to the enemy base centre (none: (0,0)).
        assert!((marker.position.x - (100.0 + base_radius)).abs() < 0.01, "mines east of the base, at {:?}", marker.position);
        assert!((marker.position.y - 100.0).abs() < 0.01, "not on the Center1 path (north): {:?}", marker.position);
    }

    /// `ScriptEngine::notifyOfTriggeredSpecialPower` / `notifyOfCompletedSpecialPower`
    /// (`ScriptEngine.cpp:7220-7240`) push into the *singleton* engine's lists, and `isSpecialPowerTriggered` /
    /// `isSpecialPowerComplete` (`:7361`, `:7411`) read them back -- including for a power a script action of the
    /// same pass fired. `GameLogic::scripts` is empty while an action runs, so the event has to be recorded on
    /// the engine handed down: with the write through `self.scripts` the event was dropped on restore and a
    /// `PLAYER_TRIGGERED/COMPLETED_SPECIAL_POWER` condition later in the same pass never saw it.
    #[test]
    fn a_script_fired_special_power_is_seen_by_a_later_condition_in_the_same_pass() {
        let mut g = cluster_mine_world();
        let condition = |name: &str| Condition {
            version: 4,
            type_id: 0,
            internal_name: Some(name.into()),
            params: vec![string_param("<This Player>"), string_param("ClusterMines")],
        };
        g.scripts_mut().load(vec![(
            0,
            ScriptList {
                scripts: vec![
                    simple_script(
                        "fire",
                        vec![true_condition()],
                        vec![action("SKIRMISH_FIRE_SPECIAL_POWER_AT_MOST_COST", vec![string_param("<This Player>"), string_param("ClusterMines")])],
                    ),
                    // Both conditions run in the same `ScriptEngine::update` pass, after the action above.
                    simple_script(
                        "triggered",
                        vec![condition("PLAYER_TRIGGERED_SPECIAL_POWER")],
                        vec![action("PLAYER_GIVE_MONEY", vec![string_param("<This Player>"), int_param(100)])],
                    ),
                    simple_script(
                        "completed",
                        vec![condition("PLAYER_COMPLETED_SPECIAL_POWER")],
                        vec![action("PLAYER_GIVE_MONEY", vec![string_param("<This Player>"), int_param(10)])],
                    ),
                ],
                groups: Vec::new(),
            },
        )]);
        g.run_frames(2);
        assert_eq!(g.player(0).unwrap().money, 1110, "the triggered and completed events stayed in the engine");
    }

    /// `AI_PLAYER_BUILD_TYPE_NEAREST_TEAM` (`ScriptActions.cpp:1112-1122`) must resolve `<This Team>`
    /// to the calling team (`ScriptEngine::getTeamNamed`, ScriptEngine.cpp:5958-5962) and place the
    /// order at its **first** member's position (`Team::getEstimateTeamPosition`, Team.cpp:2228-2240;
    /// `Object::setTeam` prepends, Object.cpp:896-898). The team's spot is blocked, so the wiggle
    /// (`AIPlayer.cpp:2206-2278`) walks to the second ring's first cell (180, 180).
    ///
    /// Revert check: read the raw team parameter (the old "build at the base" stand-in) and no order
    /// is added, because `build_specific` finds no entry in the build list.
    #[test]
    fn the_nearest_team_action_resolves_this_team_and_its_first_member() {
        struct Store(Vec<Arc<ObjectTemplate>>);
        impl TemplateSource for Store {
            fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
                self.0.iter().find(|t| t.name == name).cloned()
            }
        }
        let mut trap = ObjectTemplate::new("Trap");
        trap.kind_of = crate::template::kind_of::STRUCTURE | crate::template::kind_of::IMMOBILE;
        trap.geometry.shape = crate::template::GeometryShape::Cylinder;
        trap.geometry.major_radius = 4.0;
        trap.geometry.minor_radius = 4.0;
        let mut blocker = ObjectTemplate::new("Block");
        blocker.kind_of = crate::template::kind_of::STRUCTURE | crate::template::kind_of::IMMOBILE;
        blocker.geometry.shape = crate::template::GeometryShape::Cylinder;
        blocker.geometry.major_radius = 12.0;
        blocker.geometry.minor_radius = 12.0;
        let mut unit = ObjectTemplate::new("Unit");
        unit.kind_of = crate::template::kind_of::INFANTRY;
        unit.max_health = 100.0;
        unit.initial_health = 100.0;
        unit.geometry.major_radius = 3.0;
        unit.geometry.minor_radius = 3.0;
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        g.set_template_source(Arc::new(Store(vec![Arc::new(trap), Arc::new(blocker), Arc::new(unit)])));
        g.add_ai_player(0, crate::skirmish_ai::SkirmishAiConfig::default());
        g.scripts_mut().set_player_name("p0", 0);
        g.scripts_mut().load_teams(&[zr_map::Dict(vec![
            ("teamName".into(), zr_map::DictValue::Ascii("GuardTeam".into())),
            ("teamOwner".into(), zr_map::DictValue::Ascii("p0".into())),
            ("teamIsSingleton".into(), zr_map::DictValue::Bool(false)),
        ])]);
        let key = g.scripts_mut().create_team("guardteam").expect("the team instance");
        let unit_t = g.find_template("Unit").expect("the unit template");
        let u = g.create_object(&unit_t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        g.set_object_team_key(u, &key);
        let block_t = g.find_template("Block").expect("the blocker template");
        g.create_object(&block_t, Coord3D::new(200.0, 200.0, 0.0), 0.0, 0);
        let a = action(
            "AI_PLAYER_BUILD_TYPE_NEAREST_TEAM",
            vec![string_param("<This Player>"), string_param("Trap"), string_param("<This Team>")],
        );
        // The engine is out of `GameLogic` while a script action runs (`script_engine_update` takes it).
        let mut engine = std::mem::take(&mut g.scripts);
        engine.current_player = Some(0);
        engine.current_team = Some(key);
        engine.execute_action(&mut g, &a);
        g.scripts = engine;
        let ai = g.ai_player(0).expect("the AI player");
        assert_eq!(ai.config.build_list.len(), 1, "<This Team> resolved and the order was placed");
        let order = &ai.config.build_list[0];
        assert_eq!(order.template, "Trap");
        assert_eq!((order.location.0, order.location.1), (180.0, 180.0), "placed at the first member's wiggle cell");
    }
}
