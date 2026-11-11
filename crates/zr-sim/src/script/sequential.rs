//! Sequential scripts: `SequentialScript` and `ScriptEngine::evaluateAndProgressAllSequentialScripts`
//! (`ScriptEngine.cpp:7733-8110`), plus the `*_EXECUTE_SEQUENTIAL_SCRIPT` / `*_STOP_SEQUENTIAL_SCRIPT` /
//! `*_FOR_FRAMECOUNT` actions of `ScriptActions.cpp` (`doTeamStartSequentialScript` etc.).
//!
//! A sequential script runs the *actions* of a script one at a time on a team (or a unit). After an
//! action the script waits until the team's AI group is idle (or a frame timer set by a
//! `*_FOR_FRAMECOUNT` action runs out), then runs the next action; the script's conditions are never
//! evaluated. Each team/unit has a queue (`m_nextScriptInSequence` chain) of such scripts.

use std::collections::VecDeque;

use super::{command_power_matches, pint, pstr, ScriptEngine};
use crate::ai_states::{AiStateType, CommandSource};
use crate::logic::GameLogic;
use crate::object::ObjectId;

/// `MAX_SPIN_COUNT` (`ScriptEngine.cpp:125`): re-evaluations of one entry in a frame before it is skipped.
const MAX_SPIN_COUNT: i32 = 20;

/// A script's location: `(side, group, index)` as in [`ScriptEngine::find_script_by_name`].
type ScriptRef = (usize, Option<usize>, usize);

/// `SequentialScript` (`ScriptEngine.h`).
#[derive(Clone, Debug)]
pub struct SequentialScript {
    /// Unique id, so the evaluator can find its entry again after an action edited the list.
    id: u32,
    /// `m_teamToExecOn`: the team instance key.
    team: Option<String>,
    /// `m_objectID` (0 when the script runs on a team).
    object: ObjectId,
    /// `m_scriptToExecuteSequentially`.
    script: ScriptRef,
    /// `m_currentInstruction` (`START_INSTRUCTION` = -1 until the first action runs).
    current_instruction: i32,
    /// `m_timesToLoop`: 0 = once, -1 = forever, n = n more times.
    times_to_loop: i32,
    /// `m_framesToWait`: -1 waits for idle.
    frames_to_wait: i32,
    /// `m_dontAdvanceInstruction`: a wait action that has not been satisfied yet re-runs next frame.
    dont_advance: bool,
}

impl SequentialScript {
    fn matches(&self, other: &SequentialScript) -> bool {
        (other.object.0 != 0 && other.object == self.object) || (other.team.is_some() && other.team == self.team)
    }
}

impl GameLogic {
    /// `AIUpdateInterface::isIdle` (`AIUpdate.cpp:3120`): the main machine is in `AI_IDLE`; guard,
    /// hunt and move states are not idle.
    pub(crate) fn ai_is_idle(&self, o: &crate::object::Object) -> bool {
        o.ai.machine.current == AiStateType::Idle
    }
}

impl ScriptEngine {
    /// `AIGroup::isIdle` for a team: every AI-bearing member is idle or effectively dead.
    pub(crate) fn team_group_idle(&self, logic: &GameLogic, key: &str) -> bool {
        let Some(t) = self.teams.get(key) else { return true };
        t.members.iter().all(|id| match logic.find_object(*id) {
            None => true,
            // `AIGroup::isIdle` (`AIGroup.cpp:3173`): `ai->isIdle() || obj->isEffectivelyDead()`.
            Some(o) => o.ai_handle.is_none() || o.is_dying() || logic.ai_is_idle(o),
        })
    }

    /// `AIGroup::isGroupAiDead`: every member (an empty team too) is effectively dead.
    fn team_group_dead(&self, logic: &GameLogic, key: &str) -> bool {
        let Some(t) = self.teams.get(key) else { return true };
        t.members.iter().all(|id| logic.find_object(*id).is_none_or(|o| o.is_dying()))
    }

    /// `ScriptEngine::appendSequentialScript`: queue behind the script already running on the same
    /// object/team, else start a new queue.
    fn append_sequential_script(&mut self, mut s: SequentialScript) {
        self.next_sequential_id += 1;
        s.id = self.next_sequential_id;
        s.current_instruction = -1;
        for entry in &mut self.sequential {
            if entry.front().is_some_and(|head| head.matches(&s)) {
                entry.push_back(s);
                return;
            }
        }
        self.sequential.push(VecDeque::from([s]));
    }

    fn new_sequential(&self, team: Option<String>, object: ObjectId, script: ScriptRef, loop_val: i32) -> SequentialScript {
        SequentialScript {
            id: 0,
            team,
            object,
            script,
            current_instruction: -1,
            times_to_loop: loop_val,
            frames_to_wait: -1,
            dont_advance: false,
        }
    }

    /// `ScriptActions::doTeamStartSequentialScript`: idle the team so the script starts, then queue it.
    pub(super) fn start_team_sequential_script(&mut self, logic: &mut GameLogic, team: &str, script: &str, loop_val: i32) {
        let key = self.resolve_team(team);
        if !self.teams.contains_key(&key) {
            return;
        }
        let Some(script) = self.find_script_by_name(script) else { return };
        // `groupIdle(CMD_FROM_SCRIPT)`.
        for id in self.team_members(logic, &key) {
            if logic.find_object(id).is_some_and(|o| o.ai_handle.is_some()) {
                logic.ai_idle(id, CommandSource::Script);
            }
        }
        let s = self.new_sequential(Some(key), ObjectId(0), script, loop_val);
        self.append_sequential_script(s);
    }

    /// `ScriptActions::doUnitStartSequentialScript`.
    pub(super) fn start_unit_sequential_script(&mut self, unit: &str, script: &str, loop_val: i32) {
        let Some(id) = self.named_object(unit) else { return };
        let Some(script) = self.find_script_by_name(script) else { return };
        let s = self.new_sequential(None, id, script, loop_val);
        self.append_sequential_script(s);
    }

    /// `ScriptActions::doTeamStopSequentialScript` -> `notifyOfTeamDestruction`: drop every queued script.
    pub(super) fn stop_team_sequential_script(&mut self, team: &str) {
        let key = self.resolve_team(team);
        self.notify_of_team_destruction(&key);
    }

    /// `ScriptEngine::notifyOfTeamDestruction` (ScriptEngine.cpp:7816), called from `Team::~Team`:
    /// every sequential script running on the destroyed team is cleaned up.
    pub(crate) fn notify_of_team_destruction(&mut self, key: &str) {
        self.sequential.retain(|e| e.front().is_none_or(|h| h.team.as_deref() != Some(key)));
    }

    /// `ScriptActions::doUnitStopSequentialScript` -> `removeAllSequentialScripts(Object*)`.
    pub(super) fn stop_unit_sequential_script(&mut self, unit: &str) {
        let Some(id) = self.named_object(unit) else { return };
        self.sequential.retain(|e| e.front().is_none_or(|h| h.object != id));
    }

    /// `ScriptEngine::setSequentialTimer(Team*, frames)`: the first queue running on that team.
    pub(super) fn set_sequential_timer_team(&mut self, key: &str, frames: i32) {
        if let Some(h) = self.sequential.iter_mut().filter_map(|e| e.front_mut()).find(|h| h.team.as_deref() == Some(key)) {
            h.frames_to_wait = frames;
        }
    }

    /// `ScriptEngine::setSequentialTimer(Object*, frames)`.
    pub(super) fn set_sequential_timer_object(&mut self, id: ObjectId, frames: i32) {
        if let Some(h) = self.sequential.iter_mut().filter_map(|e| e.front_mut()).find(|h| h.object == id) {
            h.frames_to_wait = frames;
        }
    }

    /// `doTeamIdleForFramecount` (also the retail target of `TEAM_GUARD_FOR_FRAMECOUNT`, which
    /// dispatches to it in `ScriptActions::executeAction`): idle the team and wait `frames`.
    pub(super) fn team_idle_for_framecount(&mut self, logic: &mut GameLogic, team: &str, frames: i32) {
        let key = self.resolve_team(team);
        if !self.teams.contains_key(&key) {
            return;
        }
        for id in self.team_members(logic, &key) {
            if logic.find_object(id).is_some_and(|o| o.ai_handle.is_some()) {
                logic.ai_idle(id, CommandSource::Script);
            }
        }
        self.set_sequential_timer_team(&key, frames);
    }

    /// `doTeamSpinForFramecount`.
    pub(super) fn team_spin_for_framecount(&mut self, team: &str, frames: i32) {
        let key = self.resolve_team(team);
        if self.teams.contains_key(&key) {
            self.set_sequential_timer_team(&key, frames);
        }
    }

    /// `doUnitIdleForFramecount`.
    pub(super) fn unit_idle_for_framecount(&mut self, logic: &mut GameLogic, unit: &str, frames: i32) {
        let Some(id) = self.named_object(unit) else { return };
        if logic.find_object(id).is_none_or(|o| o.ai_handle.is_none()) {
            return;
        }
        logic.ai_idle(id, CommandSource::Script);
        self.set_sequential_timer_object(id, frames);
    }

    /// `doUnitGuardForFramecount`: guard the current position, then wait `frames`.
    pub(super) fn unit_guard_for_framecount(&mut self, logic: &mut GameLogic, unit: &str, frames: i32) {
        let Some(id) = self.named_object(unit) else { return };
        let Some((player, pos)) = logic.find_object(id).filter(|o| o.ai_handle.is_some()).map(|o| (o.player, o.position)) else {
            return;
        };
        logic.command_guard(player, &[id], crate::orders::GuardTarget::Point(pos), crate::orders::GuardMode::Normal);
        self.set_sequential_timer_object(id, frames);
    }

    /// `ScriptConditions::evaluateTeamIsContained` (`ScriptConditions.cpp:2011`). The retail
    /// "still exiting" test is `isContained && ...` after `!isContained`, so it never fires.
    fn team_is_contained(&self, logic: &GameLogic, team: &str, all_contained: bool) -> bool {
        let key = self.resolve_team(team);
        let Some(t) = self.teams.get(&key) else { return false };
        let mut any_considered = false;
        for id in &t.members {
            let Some(o) = logic.find_object(*id) else { continue };
            if o.contained_by.is_some() {
                if !all_contained {
                    return true;
                }
            } else if all_contained {
                return false;
            }
            any_considered = true;
        }
        any_considered && all_contained
    }

    /// `ScriptConditions::evaluateSkirmishCommandButtonIsReady` (`ScriptConditions.cpp:2285`). The
    /// command-button table is not ported, so a button maps to the member's special power matched by
    /// name (as `team_use_command_button` does); members without one are skipped, as in the original
    /// for buttons that are neither a special power nor an upgrade.
    fn command_button_ready(&self, logic: &GameLogic, team: &str, button: &str, all_ready: bool) -> bool {
        let key = self.resolve_team(team);
        let Some(t) = self.teams.get(&key) else { return false };
        for id in &t.members {
            let Some(o) = logic.find_object(*id) else { continue };
            let power = o
                .template
                .special_powers
                .iter()
                .map(|sp| sp.power.clone())
                .chain(o.template.special_abilities.iter().map(|a| a.power.clone()))
                .find(|p| command_power_matches(p, button));
            let Some(power) = power else { continue };
            if logic.special_power_is_ready(*id, &power) {
                if !all_ready {
                    return true;
                }
            } else if all_ready {
                return false;
            }
        }
        all_ready
    }

    /// `cleanupSequentialScript(it, false)`: drop the head; the queue's next script (if any) becomes
    /// the head, else the entry goes away.
    fn cleanup_sequential_head(&mut self, it: usize) {
        if let Some(e) = self.sequential.get_mut(it) {
            e.pop_front();
            if e.is_empty() {
                self.sequential.remove(it);
            }
        }
    }

    /// `ScriptEngine::evaluateAndProgressAllSequentialScripts` (`ScriptEngine.cpp:7886`), run at the end
    /// of `ScriptEngine::update` after the team states.
    pub(super) fn evaluate_and_progress_all_sequential_scripts(&mut self, logic: &mut GameLogic) {
        let mut it = 0usize;
        let mut curr_index = 0usize;
        let mut prev_index = usize::MAX;
        let mut spin_count = 0;
        while it < self.sequential.len() {
            if curr_index == prev_index {
                spin_count += 1;
            } else {
                spin_count = 0;
            }
            if spin_count > MAX_SPIN_COUNT {
                it += 1;
                curr_index += 1;
                continue;
            }
            prev_index = curr_index;
            let mut it_advanced = false;

            let Some(head) = self.sequential[it].front().cloned() else {
                self.sequential.remove(it);
                continue;
            };
            let team = head.team.clone().filter(|k| self.teams.contains_key(k));
            let obj = if head.object.0 != 0 { logic.find_object(head.object).map(|o| (o.player, o.ai_handle.is_some())) } else { None };
            if obj.is_none() && team.is_none() {
                self.cleanup_sequential_head(it);
                continue;
            }
            self.current_player = match (&obj, &team) {
                (Some((p, _)), _) => Some(*p),
                (None, Some(k)) => self.team_owner(k),
                _ => None,
            };
            let obj_ai = obj.is_some_and(|(_, ai)| ai);

            if obj_ai || team.is_some() {
                let idle_now = |s: &ScriptEngine, logic: &GameLogic| {
                    (obj_ai && logic.find_object(head.object).is_some_and(|o| logic.ai_is_idle(o)))
                        || team.as_deref().is_some_and(|k| s.team_group_idle(logic, k))
                };
                if head.frames_to_wait == 0 || (head.frames_to_wait < 0 && idle_now(self, logic)) {
                    // Time to progress to the next task.
                    let instruction = {
                        let h = self.sequential[it].front_mut().expect("head");
                        if h.dont_advance {
                            h.dont_advance = false;
                        } else {
                            h.current_instruction += 1;
                        }
                        h.current_instruction
                    };
                    let (side, group, index) = head.script;
                    let action = self.script_state(side, group, index).def.actions.get(instruction.max(0) as usize).cloned();
                    if let Some(action) = action {
                        self.sequential[it].front_mut().expect("head").frames_to_wait = -1;
                        let saved_team = self.current_team.take();
                        self.current_team = team.clone();
                        let p = &action.params;
                        let mut dont_advance = false;
                        match action.name().unwrap_or("") {
                            "SKIRMISH_WAIT_FOR_COMMANDBUTTON_AVAILABLE_ALL" => {
                                dont_advance = !self.command_button_ready(logic, pstr(p, 1), pstr(p, 2), true);
                            }
                            "SKIRMISH_WAIT_FOR_COMMANDBUTTON_AVAILABLE_PARTIAL" => {
                                dont_advance = !self.command_button_ready(logic, pstr(p, 1), pstr(p, 2), false);
                            }
                            "TEAM_WAIT_FOR_NOT_CONTAINED_ALL" => {
                                dont_advance = self.team_is_contained(logic, pstr(p, 0), true);
                            }
                            "TEAM_WAIT_FOR_NOT_CONTAINED_PARTIAL" => {
                                dont_advance = self.team_is_contained(logic, pstr(p, 0), false);
                            }
                            _ => self.execute_action(logic, &action),
                        }
                        self.current_team = saved_team;
                        // Retail orders take effect immediately; the sim queues commands until
                        // `process_command_list`, so apply them before testing the group's idle state.
                        logic.flush_command_list();
                        // The action may have stopped this script (`TEAM_STOP_SEQUENTIAL_SCRIPT`).
                        let Some(pos) = self.sequential.iter().position(|e| e.front().is_some_and(|h| h.id == head.id)) else {
                            continue;
                        };
                        it = pos;
                        if dont_advance {
                            self.sequential[it].front_mut().expect("head").dont_advance = true;
                            it += 1;
                            curr_index += 1;
                            continue;
                        }
                        let obj_idle_now = obj_ai && logic.find_object(head.object).is_some_and(|o| logic.ai_is_idle(o));
                        if obj_idle_now || team.as_deref().is_some_and(|k| self.team_group_idle(logic, k)) {
                            // Pretend we already advanced, to allow several checks per frame.
                            it_advanced = true;
                        }
                        if it_advanced {
                            if obj.is_some() && logic.find_object(head.object).is_none_or(|o| o.is_dying()) {
                                self.sequential.remove(it);
                                continue;
                            }
                            if team.as_deref().is_some_and(|k| self.team_group_dead(logic, k)) {
                                self.sequential.remove(it);
                                continue;
                            }
                        }
                    } else {
                        let h = self.sequential[it].front_mut().expect("head");
                        if h.times_to_loop != 0 {
                            if h.times_to_loop != -1 {
                                h.times_to_loop -= 1;
                            }
                            h.frames_to_wait = -1;
                            let again = h.clone();
                            self.append_sequential_script(again);
                        }
                        self.cleanup_sequential_head(it);
                        it_advanced = true;
                    }
                } else if head.frames_to_wait > 0 {
                    self.sequential[it].front_mut().expect("head").frames_to_wait -= 1;
                }
            }

            if !it_advanced {
                it += 1;
                curr_index += 1;
            }
        }
        self.current_player = None;
    }

    /// Hash the sequential-script queues.
    pub(super) fn crc_sequential(&self, c: &mut crate::crc::Crc) {
        c.add_u32(self.sequential.len() as u32);
        for e in &self.sequential {
            c.add_u32(e.len() as u32);
            for s in e {
                c.add_str(s.team.as_deref().unwrap_or(""));
                c.add_u32(s.object.0);
                c.add_u32(s.script.0 as u32);
                c.add_u32(s.script.1.map_or(u32::MAX, |g| g as u32));
                c.add_u32(s.script.2 as u32);
                c.add_u32(s.current_instruction as u32);
                c.add_u32(s.times_to_loop as u32);
                c.add_u32(s.frames_to_wait as u32);
                c.add_bool(s.dont_advance);
            }
        }
    }

    /// Dispatch of the sequential-script action names; false when `name` is not one of them.
    pub(super) fn execute_sequential_action(&mut self, logic: &mut GameLogic, name: &str, p: &[zr_map::scripts::Parameter]) -> bool {
        match name {
            "UNIT_EXECUTE_SEQUENTIAL_SCRIPT" => self.start_unit_sequential_script(pstr(p, 0), pstr(p, 1), 0),
            "UNIT_EXECUTE_SEQUENTIAL_SCRIPT_LOOPING" => self.start_unit_sequential_script(pstr(p, 0), pstr(p, 1), pint(p, 2) - 1),
            "UNIT_STOP_SEQUENTIAL_SCRIPT" => self.stop_unit_sequential_script(pstr(p, 0)),
            "TEAM_EXECUTE_SEQUENTIAL_SCRIPT" => self.start_team_sequential_script(logic, pstr(p, 0), pstr(p, 1), 0),
            "TEAM_EXECUTE_SEQUENTIAL_SCRIPT_LOOPING" => self.start_team_sequential_script(logic, pstr(p, 0), pstr(p, 1), pint(p, 2) - 1),
            "TEAM_STOP_SEQUENTIAL_SCRIPT" => self.stop_team_sequential_script(pstr(p, 0)),
            "UNIT_GUARD_FOR_FRAMECOUNT" => self.unit_guard_for_framecount(logic, pstr(p, 0), pint(p, 1)),
            "UNIT_IDLE_FOR_FRAMECOUNT" => self.unit_idle_for_framecount(logic, pstr(p, 0), pint(p, 1)),
            // Retail dispatches `TEAM_GUARD_FOR_FRAMECOUNT` to `doTeamIdleForFramecount` too.
            "TEAM_GUARD_FOR_FRAMECOUNT" | "TEAM_IDLE_FOR_FRAMECOUNT" => self.team_idle_for_framecount(logic, pstr(p, 0), pint(p, 1)),
            "TEAM_SPIN_FOR_FRAMECOUNT" => self.team_spin_for_framecount(pstr(p, 0), pint(p, 1)),
            // Only meaningful inside a sequential script (handled by the evaluator); retail asserts.
            "SKIRMISH_WAIT_FOR_COMMANDBUTTON_AVAILABLE_ALL"
            | "SKIRMISH_WAIT_FOR_COMMANDBUTTON_AVAILABLE_PARTIAL"
            | "TEAM_WAIT_FOR_NOT_CONTAINED_ALL"
            | "TEAM_WAIT_FOR_NOT_CONTAINED_PARTIAL" => {}
            _ => return false,
        }
        true
    }
}
