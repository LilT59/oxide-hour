//! Script condition evaluation, phase 1 (`ScriptConditions.cpp` + the `FLAG`/`COUNTER`/
//! `TIMER_EXPIRED` cases in `ScriptEngine.cpp`).
//!
//! Implemented conditions are listed in [`ScriptEngine::evaluate_condition`]; every other type is
//! a no-op that returns false and increments [`ScriptEngine::warnings`].

use zr_map::scripts::Condition;

use super::{comparison, pint, preal, pstr, ScriptEngine};
use crate::logic::GameLogic;
use crate::object::PlayerIndex;

/// How a `TEAM_*_AREA` condition tests its members.
#[derive(Clone, Copy)]
enum AreaTest {
    AnyInside,
    AllInside,
    NoneInside,
    SomeOutside,
}

/// The bounding-box centre of a polygon trigger (the engine's `getCenterPoint`).
fn trigger_center(t: &zr_map::PolygonTrigger) -> (f32, f32) {
    let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for p in &t.points {
        lo = (lo.0.min(p[0] as f32), lo.1.min(p[1] as f32));
        hi = (hi.0.max(p[0] as f32), hi.1.max(p[1] as f32));
    }
    ((lo.0 + hi.0) * 0.5, (lo.1 + hi.1) * 0.5)
}

/// `PolygonTrigger::getRadius` (`PolygonTrigger.cpp:283-295`): `sqrt(halfHeight^2 + halfWidth^2)` over the
/// integer bounds, where `halfHeight = (hi.y + lo.y) / 2` (the centre y, not half the height). The original
/// code computes it this way, so it is kept.
fn polygon_radius(t: &zr_map::PolygonTrigger) -> f32 {
    let (mut lo, mut hi) = ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN));
    for p in &t.points {
        lo = (lo.0.min(p[0]), lo.1.min(p[1]));
        hi = (hi.0.max(p[0]), hi.1.max(p[1]));
    }
    if t.points.is_empty() {
        return 0.0;
    }
    let half_w = (hi.0 - lo.0) as f32 / 2.0;
    let half_h = (hi.1 + lo.1) as f32 / 2.0;
    (half_h * half_h + half_w * half_w).sqrt()
}

impl ScriptEngine {
    /// `ScriptEngine::evaluateCondition` + `ScriptConditions::evaluateCondition`.
    pub(super) fn evaluate_condition(&mut self, logic: &GameLogic, c: &Condition) -> bool {
        let name = c.name().unwrap_or("");
        let p = &c.params;
        match name {
            "CONDITION_TRUE" => true,
            "CONDITION_FALSE" => false,
            "FLAG" => {
                let want = pint(p, 1) != 0;
                self.get_flag(pstr(p, 0)) == want
            }
            "COUNTER" => {
                let v = self.counter(pstr(p, 0));
                comparison::apply(pint(p, 1), v, pint(p, 2))
            }
            "TIMER_EXPIRED" => self.timer_expired(pstr(p, 0)),
            "PLAYER_HAS_CREDITS" => {
                // Note: the original compares the credit parameter to the player's money with the
                // parameter on the *left* (`evaluatePlayerHasCredits`), so LESS_THAN means "has more".
                let Some(pl) = self.player_from_param(pstr(p, 2)) else { return false };
                let money = logic.player(pl).map_or(0, |x| x.money) as i32;
                comparison::apply(pint(p, 1), pint(p, 0), money)
            }
            "PLAYER_HAS_OBJECT_COMPARISON" => {
                let Some(pl) = self.player_from_param(pstr(p, 0)) else { return false };
                let count = self.count_owned(logic, pl, pstr(p, 3)) as i32;
                comparison::apply(pint(p, 1), count, pint(p, 2))
            }
            "BUILT_BY_PLAYER" => {
                let Some(pl) = self.player_from_param(pstr(p, 1)) else { return false };
                self.count_owned(logic, pl, pstr(p, 0)) != 0
            }
            // `evaluateNamedUnitExists` / `evaluateNamedUnitDestroyed` (`ScriptConditions.cpp:306-330`):
            // an existing object counts as alive only while `!isEffectivelyDead()`; a name that never
            // existed is not destroyed (`:301`), and a name whose object is gone was destroyed.
            // `isEffectivelyDead` is the port's `Object::is_dying()` (health 0 or the die pipeline).
            "NAMED_NOT_DESTROYED" => self
                .named_object(pstr(p, 0))
                .is_some_and(|id| logic.find_object(id).is_some_and(|o| !o.is_dying())),
            "NAMED_DESTROYED" => self
                .named_object(pstr(p, 0))
                .is_some_and(|id| logic.find_object(id).is_none_or(|o| o.is_dying())),
            "START_POSITION_IS" => {
                let Some(pl) = self.player_from_param(pstr(p, 0)) else { return false };
                self.player_start.get(&pl).copied().unwrap_or(-1) == pint(p, 1) - 1
            }
            "TEAM_HAS_UNITS" => self.team_has_units(logic, pstr(p, 0)),
            "TEAM_DESTROYED" => self.team_created(pstr(p, 0)) && !self.team_has_units(logic, pstr(p, 0)),
            "SKIRMISH_PLAYER_FACTION" => {
                let Some(pl) = self.player_from_param(pstr(p, 0)) else { return false };
                self.player_sides.get(&pl).map(String::as_str) == Some(pstr(p, 1))
            }
            "PLAYER_HAS_POWER" => {
                self.player_from_param(pstr(p, 0)).is_some_and(|pl| logic.has_sufficient_power(pl))
            }
            "PLAYER_HAS_NO_POWER" => {
                self.player_from_param(pstr(p, 0)).is_some_and(|pl| !logic.has_sufficient_power(pl))
            }
            "PLAYER_ALL_DESTROYED" => self.player_from_param(pstr(p, 0)).is_some_and(|pl| {
                !logic.objects().any(|o| o.player == pl && o.is_kind_of(crate::template::kind_of::STRUCTURE))
            }),
            "PLAYER_LOST_OBJECT_TYPE" => {
                let Some(pl) = self.player_from_param(pstr(p, 0)) else { return false };
                self.count_owned(logic, pl, pstr(p, 1)) == 0
            }
            // `ScriptConditions::evaluateUpgradeFromUnitComplete` (ScriptConditions.cpp:1535-1555): consumes the first
            // completed-upgrade event of that name, from the named unit when one is given (`isUpgradeComplete` with
            // `removeFromList = TRUE`). A named unit that is not known is FALSE.
            "PLAYER_BUILT_UPGRADE" | "PLAYER_BUILT_UPGRADE_FROM_NAMED" => {
                let Some(pl) = self.player_from_param(pstr(p, 0)) else { return false };
                let source = if name.ends_with("FROM_NAMED") {
                    match self.named_object(pstr(p, 2)) {
                        Some(id) => Some(id),
                        None => return false,
                    }
                } else {
                    None
                };
                Self::consume_special_power_event(&mut self.completed_upgrades, pl, pstr(p, 1), source)
            }
            "PLAYER_ACQUIRED_SCIENCE" => {
                let Some(pl) = self.player_from_param(pstr(p, 0)) else { return false };
                logic.sciences.get(&pl).is_some_and(|s| s.sciences.contains(pstr(p, 1)))
            }
            "PLAYER_POWER_COMPARE_PERCENT" => {
                let Some(pl) = self.player_from_param(pstr(p, 0)) else { return false };
                let ratio = logic.energy_supply_ratio(pl);
                let test = pint(p, 2) as f32 / 100.0;
                comparison::apply_f(pint(p, 1), ratio, test)
            }
            // No music/audio/video subsystem: the track is always "done".
            "MUSIC_TRACK_HAS_COMPLETED" | "HAS_FINISHED_SPEECH" | "HAS_FINISHED_AUDIO" | "HAS_FINISHED_VIDEO" => true,

            // ---- named-object health (`ScriptConditions.cpp::evaluateUnitHealth`) ----------
            "UNIT_HEALTH" => {
                // `<This Object>` resolves through `getUnitNamed`'s calling/condition object; the
                // sim binds neither for generic team scripts, so it resolves to no object (false),
                // matching the engine's generic-script evaluation.
                let Some(id) = self.named_object(pstr(p, 0)) else { return false };
                let Some(obj) = logic.find_object(id) else { return false };
                let initial = obj.template.initial_health;
                if initial <= 0.0 {
                    return false;
                }
                let cur = obj.health();
                let percent = ((cur * 100.0 + initial / 2.0) / initial) as i32;
                comparison::apply(pint(p, 1), percent, pint(p, 2))
            }
            // `AIPlayer::isSupplySourceSafe` (`evaluateSkirmishSupplySourceSafe`).
            "SUPPLY_SOURCE_SAFE" => self
                .player_from_param(pstr(p, 0))
                .is_some_and(|pl| logic.supply_source_safe(pl, pint(p, 1))),
            // `SpecialPowerStore` event lists (`isSpecialPowerTriggered` / `isSpecialPowerComplete`).
            "PLAYER_TRIGGERED_SPECIAL_POWER" | "PLAYER_TRIGGERED_SPECIAL_POWER_FROM_NAMED" => {
                let Some(pl) = self.player_from_param(pstr(p, 0)) else { return false };
                let source = if name.ends_with("FROM_NAMED") {
                    match self.named_object(pstr(p, 2)) {
                        Some(id) => Some(id),
                        None => return false,
                    }
                } else {
                    None
                };
                Self::consume_special_power_event(&mut self.triggered_special_powers, pl, pstr(p, 1), source)
            }
            "PLAYER_COMPLETED_SPECIAL_POWER" | "PLAYER_COMPLETED_SPECIAL_POWER_FROM_NAMED" => {
                let Some(pl) = self.player_from_param(pstr(p, 0)) else { return false };
                let source = if name.ends_with("FROM_NAMED") {
                    match self.named_object(pstr(p, 2)) {
                        Some(id) => Some(id),
                        None => return false,
                    }
                } else {
                    None
                };
                Self::consume_special_power_event(&mut self.completed_special_powers, pl, pstr(p, 1), source)
            }
            // `evaluateTeamHasObjectStatus`: the sim derives the statuses it models from object
            // state instead of storing an `ObjectStatusMaskType` (see `object_has_status`).
            "TEAM_SOME_HAVE_OBJECT_STATUS" => {
                self.team_has_object_status(logic, pstr(p, 0), pstr(p, 1), false)
            }
            "TEAM_ALL_HAS_OBJECT_STATUS" => {
                self.team_has_object_status(logic, pstr(p, 0), pstr(p, 1), true)
            }

            // ---- multiplayer victory / defeat (`MultiplayerScripts.scb`) ----------------
            "MULTIPLAYER_PLAYER_DEFEAT" => {
                self.current_player.is_some_and(|p| !logic.player_alive(p))
            }
            "MULTIPLAYER_ALLIED_DEFEAT" => {
                self.current_player.is_some_and(|p| !logic.player_alive(p))
            }
            "MULTIPLAYER_ALLIED_VICTORY" => logic.check_for_game_end().winner == self.current_player,

            // ---- trigger areas ---------------------------------------------------------
            "TEAM_INSIDE_AREA_PARTIALLY" | "TEAM_ENTERED_AREA_PARTIALLY" => {
                self.team_in_area(logic, pstr(p, 0), pstr(p, 1), AreaTest::AnyInside)
            }
            "TEAM_INSIDE_AREA_ENTIRELY" | "TEAM_ENTERED_AREA_ENTIRELY" => {
                self.team_in_area(logic, pstr(p, 0), pstr(p, 1), AreaTest::AllInside)
            }
            "TEAM_OUTSIDE_AREA_ENTIRELY" | "TEAM_EXITED_AREA_ENTIRELY" => {
                self.team_in_area(logic, pstr(p, 0), pstr(p, 1), AreaTest::NoneInside)
            }
            "TEAM_EXITED_AREA_PARTIALLY" => self.team_in_area(logic, pstr(p, 0), pstr(p, 1), AreaTest::SomeOutside),
            "NAMED_INSIDE_AREA" | "NAMED_ENTERED_AREA" => self.named_in_area(logic, pstr(p, 0), pstr(p, 1)),
            "NAMED_OUTSIDE_AREA" | "NAMED_EXITED_AREA" => !self.named_in_area(logic, pstr(p, 0), pstr(p, 1)),
            "PLAYER_HAS_COMPARISON_UNIT_TYPE_IN_TRIGGER_AREA" => {
                let Some(pl) = self.player_from_param(pstr(p, 0)) else { return false };
                let Some(trig) = self.trigger_area(pstr(p, 4)).cloned() else { return false };
                let count = self.count_owned_in_area(logic, pl, pstr(p, 3), &trig) as i32;
                comparison::apply(pint(p, 1), count, pint(p, 2))
            }
            "SKIRMISH_VALUE_IN_AREA" => {
                let Some(pl) = self.player_from_param(pstr(p, 0)) else { return false };
                let Some(trig) = self.trigger_area(pstr(p, 3)).cloned() else { return false };
                let value: i32 = logic
                    .objects()
                    .filter(|o| o.player == pl && trig.contains(o.position.x as i32, o.position.y as i32))
                    .map(|o| o.template.build_cost.max(0))
                    .sum();
                comparison::apply(pint(p, 1), value, pint(p, 2))
            }
            "SKIRMISH_SPECIAL_POWER_READY" => self.special_power_ready(logic, pstr(p, 0), pstr(p, 1)),
            "SKIRMISH_TECH_BUILDING_WITHIN_DISTANCE" => {
                let Some(pl) = self.player_from_param(pstr(p, 0)) else { return false };
                let Some(trig) = self.trigger_area(pstr(p, 2)).cloned() else { return false };
                let (cx, cy) = trigger_center(&trig);
                let radius = preal(p, 1) + polygon_radius(&trig);
                // `evaluateSkirmishPlayerTechBuildingWithinDistancePerimeter` (`ScriptConditions.cpp:2249`):
                // a `KINDOF_TECH_BUILDING` that is not the player's and not an ally's (`PartitionFilterPlayer`
                // and `PartitionFilterPlayerAffiliation(ALLOW_ALLIES, false)`), any within the 2D radius.
                // The result is kept for good by the caller (`ScriptEngine::sticky_conditions`).
                // `getClosestObject(&center, radius, FROM_CENTER_2D)` compares squared distances and is strict
                // (`distCalcProc_CenterAndCenter_2D`, `PartitionManager.cpp:788-802`: `abDistSqr < maxDistSqr`).
                let max_dist_sqr = radius * radius;
                logic.objects().any(|o| {
                    let (dx, dy) = (o.position.x - cx, o.position.y - cy);
                    o.player != pl
                        && o.is_kind_of(crate::template::kind_of::TECH_BUILDING)
                        && logic.relationship(pl, o.player) != crate::damage::Relationship::Allies
                        && dx * dx + dy * dy < max_dist_sqr
                })
            }
            "SKIRMISH_UNOWNED_FACTION_UNIT_EXISTS" => {
                // `evaluateSkirmishUnownedFactionUnitComparison` (`ScriptConditions.cpp:2323`): members of the
                // neutral player's (`getNeutralPlayer` = `m_players[0]`) teams that are `DISABLED_UNMANNED`.
                // Map-placed neutral structures are not unmanned units, so they do not count.
                let count = logic.objects().filter(|o| o.player == 0 && o.unmanned()).count() as i32;
                comparison::apply(pint(p, 1), count, pint(p, 2))
            }
            "SKIRMISH_PLAYER_HAS_PREREQUISITE_TO_BUILD" => {
                let Some(pl) = self.player_from_param(pstr(p, 0)) else { return false };
                logic.find_template(pstr(p, 1)).is_some_and(|t| logic.prerequisites_satisfied(pl, &t))
            }
            _ => {
                self.warn(name);
                false
            }
        }
    }

    /// `evaluateTeamHasObjectStatus`: `entireTeam` requires every member, else any member. The sim
    /// derives statuses from object state rather than storing a full `ObjectStatusMaskType`.
    fn team_has_object_status(&mut self, logic: &GameLogic, team: &str, status: &str, entire_team: bool) -> bool {
        let key = self.resolve_team(team).to_ascii_lowercase();
        let Some(t) = self.teams.get(&key) else { return false };
        for id in &t.members {
            // The engine returns false when it hits a null member.
            let Some(o) = logic.find_object(*id) else { return false };
            let has = object_has_status(logic, o, status);
            if entire_team && !has {
                return false;
            }
            if !entire_team && has {
                return true;
            }
        }
        entire_team
    }

    /// `TEAM_*_AREA` tests over a team's live members.
    fn team_in_area(&mut self, logic: &GameLogic, team: &str, area: &str, test: AreaTest) -> bool {
        let key = self.resolve_team(team).to_ascii_lowercase();
        let Some(trig) = self.trigger_area(area).cloned() else { return false };
        let Some(t) = self.teams.get(&key) else { return false };
        let inside = t
            .members
            .iter()
            .filter(|id| logic.find_object(**id).is_some_and(|o| trig.contains(o.position.x as i32, o.position.y as i32)))
            .count();
        let total = t.members.iter().filter(|id| logic.find_object(**id).is_some()).count();
        match test {
            AreaTest::AnyInside => inside > 0,
            AreaTest::AllInside => total > 0 && inside == total,
            AreaTest::NoneInside => inside == 0,
            AreaTest::SomeOutside => inside < total,
        }
    }

    fn named_in_area(&self, logic: &GameLogic, name: &str, area: &str) -> bool {
        let (Some(id), Some(trig)) = (self.named_object(name), self.trigger_area(area)) else { return false };
        logic.find_object(id).is_some_and(|o| trig.contains(o.position.x as i32, o.position.y as i32))
    }

    fn count_owned_in_area(&self, logic: &GameLogic, player: PlayerIndex, type_name: &str, trig: &zr_map::PolygonTrigger) -> usize {
        if let Some(list) = self.object_lists.get(type_name) {
            return list.iter().map(|t| self.count_owned_in_area(logic, player, t, trig)).sum();
        }
        logic
            .objects()
            .filter(|o| {
                o.player == player
                    && o.template.name == type_name
                    && trig.contains(o.position.x as i32, o.position.y as i32)
                    // "dead objects will not be considered, except crates ... they are 'dead' cause they have
                    // no body and health, but are a class of object we want to trigger this stuff"
                    // (`ScriptConditions.cpp:494-497`).
                    && (!(o.is_dying() || o.is_kind_of(crate::template::kind_of::INERT))
                        || o.is_kind_of(crate::template::kind_of::CRATE))
            })
            .count()
    }

    /// `SKIRMISH_SPECIAL_POWER_READY`: the player owns a unit whose power is off cooldown.
    fn special_power_ready(&self, logic: &GameLogic, player_name: &str, power: &str) -> bool {
        let Some(player) = self.player_from_param(player_name) else { return false };
        if power.is_empty() || !logic.special_powers.contains_key(power) {
            return false;
        }
        // `ScriptConditions.cpp:1925`: an object under construction or disabled is skipped. Then
        // `canUseSpecialPower` (the module exists and the required science is owned, `:1927`), then
        // `isReady()` (a shared timer, or unpaused and past its ready frame, `:1931`).
        logic.objects().any(|o| {
            o.player == player
                && !o.under_construction
                && !o.is_disabled()
                && logic.special_power_can_use(o.id, power)
                && logic.special_power_is_ready(o.id, power)
        })
    }

    /// Live objects of `type_name` owned by `player`; expands a registered object-type list. Objects
    /// still under construction are not counted: every caller of C++ `Player::countObjectsByThingTemplate`
    /// uses its default `ignoreUnderConstruction = TRUE` (`Player.h:477`, `Team.cpp:1615` skips
    /// `OBJECT_STATUS_UNDER_CONSTRUCTION`). That covers `BUILT_BY_PLAYER` (`ScriptConditions.cpp:898`),
    /// `PLAYER_HAS_OBJECT_COMPARISON` (`:1809`) and `PLAYER_LOST_OBJECT_TYPE` (`:2701`).
    /// `ignoreDead` is also TRUE for them (`Player.h:476`), and `Team::countObjectsByThingTemplate`
    /// skips `isEffectivelyDead()` (`Team.cpp:1628-1629`) — the port's `Object::is_dying()` (health 0
    /// or the die pipeline; a bodyless object carries `EFFECTIVELY_DEAD` from its `InactiveBody`
    /// constructor, `InactiveBody.cpp:47-50`).
    pub(crate) fn count_owned(&self, logic: &GameLogic, player: PlayerIndex, type_name: &str) -> usize {
        if let Some(list) = self.object_lists.get(type_name) {
            return list.iter().map(|t| self.count_owned(logic, player, t)).sum();
        }
        if type_name.is_empty() {
            return 0;
        }
        logic
            .objects()
            .filter(|o| o.player == player && o.template.name == type_name && !o.under_construction && !o.is_dying())
            .count()
    }

    fn team_created(&self, name: &str) -> bool {
        self.teams.contains_key(&self.resolve_team(name).to_ascii_lowercase())
    }

    fn team_has_units(&self, logic: &GameLogic, name: &str) -> bool {
        self.teams
            .get(&self.resolve_team(name).to_ascii_lowercase())
            .is_some_and(|t| t.members.iter().any(|id| logic.find_object(*id).is_some()))
    }
}

/// `Object::getStatusBits().testForAny(statusMask)`, reduced to the statuses the sim models.
/// The engine keeps an `ObjectStatusMaskType`; the sim does not maintain the full bit set, so the
/// statuses that map onto simulation state are derived here. Unmodelled names are false.
pub(super) fn object_has_status(logic: &GameLogic, o: &crate::object::Object, name: &str) -> bool {
    use crate::template::kind_of;
    let now = logic.frame();
    match name {
        "DESTROYED" => o.is_destroyed(),
        "UNDER_CONSTRUCTION" => o.under_construction,
        "UNMANNED" => o.unmanned(),
        "IS_FIRING_WEAPON" | "IS_ATTACKING" | "IS_AIMING_WEAPON" => o.combat.attack.is_some(),
        "AFLAME" => o.behavior.aflame_until > now,
        "BURNED" => o.behavior.burned_until > now,
        "STEALTHED" => logic.is_stealthed(o.id),
        "DETECTED" => o.behavior.detected_until > now,
        "UNDERGOING_REPAIR" => o.repair_target.is_some(),
        "CAN_ATTACK" => o.is_kind_of(kind_of::CAN_ATTACK),
        // `OBJECT_STATUS_AIRBORNE_TARGET`, set by `doLocomotor` (AIUpdate.cpp:2294-2300); not the locomotor's height.
        "AIRBORNE_TARGET" => o.status_bits & crate::object::status::AIRBORNE_TARGET != 0,
        _ => false,
    }
}
