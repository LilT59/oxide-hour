//! Team command-button script actions (`ScriptActions.cpp`): `TEAM_USE_COMMANDBUTTON_ABILITY`,
//! `TEAM_USE_COMMANDBUTTON_ABILITY_ON_NAMED`, `TEAM_ALL_USE_COMMANDBUTTON_ON_NAMED`,
//! `TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_{ENEMY_UNIT,GARRISONED_BUILDING,KINDOF,ENEMY_BUILDING,
//! ENEMY_BUILDING_CLASS,OBJECTTYPE}`. `TEAM_PARTIAL_USE_COMMANDBUTTON` is handled by the caller.
//!
//! The target actions follow `doTeamUseCommandButtonOnNearest*` (`ScriptActions.cpp:5591-5845`,
//! `:5847-5945`): the source is the first member that owns the power
//! (`AIGroup::getSpecialPowerSourceObject`, `AIGroup.cpp:3240`), the search centre is the group centre
//! (`AIGroup::getCenter`, `AIGroup.cpp:325`), and the target is the nearest object (centre to centre,
//! `PartitionManager::getClosestObject`, `PartitionManager.cpp:3536`) that passes the script's
//! `PartitionFilter`s and `CommandButton::isValidToUseOn` (`ControlBar.cpp:637`). The command then
//! goes to every member (`AIGroup::groupDoCommandButtonAtObject`, `AIGroup.cpp:3019`).
//!
//! Port limits (documented, not silently approximated):
//! * The sim knows a command button only as `CommandButtonHunt` (`load_command_buttons`). Only
//!   object-target special-power buttons (`SpecialPower`) reach a power. Other buttons, and special
//!   powers that take no object target, do nothing here.
//! * The no-target team action (`TEAM_USE_COMMANDBUTTON_ABILITY`) also runs the upgrade buttons
//!   (`command_button_upgrades`) and the other `doCommandButton` buttons (`command_button_actions`:
//!   STOP, SWITCH_WEAPON, FIRE_WEAPON, UNIT_BUILD, DOZER_CONSTRUCT, HACK_INTERNET, SELL). The partial
//!   and target actions do not run those other buttons (their `isValidToUseOn` is not modelled).
//! * The validity tests are `GameLogic::can_do_special_power_at_object` / `_at_location` /
//!   `_no_target` (`enter_actions.rs`, the `ActionManager` ports). The sim keeps no `KINDOF_CLIFF_JUMPER`
//!   bit and no `RailroadBehavior` flag, so the disguise test does not exclude cliff jumpers or trains;
//!   the charge producer-ID tests are not modelled (see that module).
//! * Script fires are forced (`CMD_FROM_SCRIPT`): the science and readiness tests of
//!   `Object::doSpecialPower` are skipped (`script_do_special_power`), as in the engine.
//! * Ties between objects at exactly the same distance go to the lowest object id. The engine
//!   takes the first one in its cell walk order, which the sim does not reproduce. The engine also
//!   stops after the first cell ring that holds a candidate (`FASTER_GCO`), which the sim does not
//!   replicate; the nearest candidate is returned.
//! * `PartitionFilterSameMapStatus` (`isOffMap`) is not modelled: the sim has no off-map objects.
//! * `PartitionFilterThing` compares template names; the engine compares `isEquivalentTo`
//!   (final override), which the sim does not model.

use crate::ai_states::{AiStateType, CommandSource};
use crate::combat::AttackTarget;
use crate::damage::Relationship;
use crate::disabled::DisabledType;
use crate::enter_actions::has_power_module;
use crate::logic::GameLogic;
use crate::math::Coord3D;
use crate::object::{Object, ObjectId, PlayerIndex};
use crate::special_power::SpecialPowerTarget;
use crate::template::kind_of;

use super::{CommandButtonAction, CommandButtonPower, CommandButtonUpgrade, ScriptEngine};

/// `REALLY_FAR` (`ScriptActions.cpp:118`): `100000 * MAP_XY_FACTOR`, with `MAP_XY_FACTOR` 10
/// (`MapObject.h:57`).
const REALLY_FAR: f32 = 100_000.0 * 10.0;

/// The FIRE_WEAPON branches of `Object::doCommandButton*` (Object.cpp:5538-5546, 5677-5704, 5798-5811): the member's
/// `setWeaponLock(slot, LOCKED_TEMPORARILY)` then `ai->aiAttackPosition` / `aiAttackObject(target, maxShots, cmdSource)`.
/// The lock is the object's `WeaponSet::temporary_lock`, which the attack does not release; a permanent switch still
/// wins (`choose_weapon`). The caller checks the AI.
fn fire_weapon_order(logic: &mut GameLogic, id: ObjectId, target: AttackTarget, slot: usize, max_shots: i32) {
    logic.set_weapon_lock(id, slot, crate::weapon::WeaponLockType::Temporarily);
    logic.end_attack(id);
    logic.ai_start_attack(id, target, false, false, Some((max_shots, slot)), AiStateType::Idle);
}

/// The object filter of one team command-button action (`ScriptActions.cpp:5591-5945`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TargetScope {
    /// `doTeamUseCommandButtonOnNearestEnemy`: `PartitionFilterPlayerAffiliation(ALLOW_ENEMIES)`.
    EnemyUnit,
    /// `doTeamUseCommandButtonOnGarrisonedBuilding`: an enemy structure that is garrisonable.
    GarrisonedBuilding,
    /// `doTeamUseCommandButtonOnNearestKindof(kindofBit)`: an enemy object of that `KindOf` index.
    KindOf(i32),
    /// `doTeamUseCommandButtonOnNearestBuilding`: an enemy structure.
    EnemyBuilding,
    /// `doTeamUseCommandButtonOnNearestBuildingClass(kindofBit)`: an enemy structure of that `KindOf` index.
    EnemyBuildingClass(i32),
    /// `doTeamUseCommandButtonOnObjectType(objectType)`: an enemy or neutral object of a template,
    /// or of an object list (`OBJECTLIST_ADDOBJECTTYPE`).
    ObjectType(String),
}

/// The fixed filters of one candidate test: affiliation, structure, garrisonable, `KindOf` index
/// and template name. The command-button validity test is added by the caller.
struct Filter<'a> {
    /// `ALLOW_ENEMIES`.
    enemies: bool,
    /// `ALLOW_NEUTRAL`.
    neutral: bool,
    /// `PartitionFilterAcceptByKindOf(KINDOF_STRUCTURE)`.
    structure: bool,
    /// `PartitionFilterGarrisonable(true)`.
    garrisonable: bool,
    /// `PartitionFilterAcceptByKindOf(MAKE_KINDOF_MASK(kindofBit))`: the script's `KindOf` index.
    kind_of_index: Option<i32>,
    /// `PartitionFilterThing(template, true)`.
    template: Option<&'a str>,
}

impl ScriptEngine {
    /// `TheControlBar->findCommandButton(ability)` for a special-power button: its power and target
    /// options (`CommandButtonPower`). Other buttons are not modelled (see the module docs).
    fn button_power(&self, ability: &str) -> Option<CommandButtonPower> {
        self.command_button_powers.get(&ability.to_ascii_lowercase()).cloned()
    }

    /// `AIGroup::getSpecialPowerSourceObject` (`AIGroup.cpp:3240`): the first member, in team order,
    /// whose object owns the power.
    fn power_source(logic: &GameLogic, ids: &[ObjectId], power: &str) -> Option<ObjectId> {
        ids.iter().copied().find(|id| logic.find_object(*id).is_some_and(|o| has_power_module(o, power)))
    }

    /// `AIGroup::getCenter` (`AIGroup.cpp:325`): the mean position of the members that are not
    /// `DISABLED_HELD` riders (`:335`) and have an AI. With no AI member, the mean of the non-held
    /// members (`:350`). `None` when no member counts, as the engine's division then gives no target.
    fn group_centre(logic: &GameLogic, ids: &[ObjectId]) -> Option<Coord3D> {
        for needs_ai in [true, false] {
            let (mut n, mut x, mut y, mut z) = (0u32, 0.0f32, 0.0f32, 0.0f32);
            for o in ids.iter().filter_map(|id| logic.find_object(*id)) {
                if o.is_disabled_by_type(DisabledType::Held) || (needs_ai && o.ai_handle.is_none()) {
                    continue;
                }
                x += o.position.x;
                y += o.position.y;
                z += o.position.z;
                n += 1;
            }
            if n > 0 {
                let n = n as f32;
                return Some(Coord3D::new(x / n, y / n, z / n));
            }
        }
        None
    }

    /// The filter set of a [`TargetScope`] (`ScriptActions.cpp:5591-5945`).
    fn scope_filter(scope: &TargetScope) -> Filter<'_> {
        let mut f = Filter {
            enemies: true,
            neutral: false,
            structure: false,
            garrisonable: false,
            kind_of_index: None,
            template: None,
        };
        match scope {
            TargetScope::EnemyUnit => {}
            TargetScope::GarrisonedBuilding => {
                f.structure = true;
                f.garrisonable = true;
            }
            TargetScope::KindOf(index) => f.kind_of_index = Some(*index),
            TargetScope::EnemyBuilding => f.structure = true,
            TargetScope::EnemyBuildingClass(index) => {
                f.structure = true;
                f.kind_of_index = Some(*index);
            }
            TargetScope::ObjectType(_) => f.neutral = true,
        }
        f
    }

    /// The filters that always apply: `PartitionFilterPlayerAffiliation(player, flags, true)`
    /// (`PartitionFilterPlayerAffiliation::allow`, `PartitionManager.cpp:5176`, which also accepts the
    /// player's own objects), `PartitionFilterAcceptByKindOf`, `PartitionFilterGarrisonable` and
    /// `PartitionFilterThing`.
    fn filter_accepts(&self, logic: &GameLogic, owner: PlayerIndex, o: &Object, f: &Filter<'_>) -> bool {
        let affiliated = match logic.relationship(owner, o.player) {
            Relationship::Enemies => f.enemies,
            Relationship::Neutral => f.neutral,
            Relationship::Allies => false,
        };
        if !(affiliated || o.player == owner) {
            return false;
        }
        if let Some(name) = f.template {
            if !o.template.name.eq_ignore_ascii_case(name) {
                return false;
            }
        }
        if f.structure && !o.is_kind_of(kind_of::STRUCTURE) {
            return false;
        }
        if let Some(index) = f.kind_of_index {
            // `MAKE_KINDOF_MASK(index)` against the object's KindOf: the template's KindOf list
            // (`ScriptEngine::setPriorityKind` builds the same index -> template table).
            let names = self.kind_of_templates.get(&index).map_or(&[][..], Vec::as_slice);
            if !names.iter().any(|n| n.eq_ignore_ascii_case(&o.template.name)) {
                return false;
            }
        }
        // `PartitionFilterGarrisonable(true)`: a container that reports `isGarrisonable`.
        if f.garrisonable && !o.template.contain.as_ref().is_some_and(|c| c.is_garrison()) {
            return false;
        }
        true
    }

    /// `CommandButton::isValidToUseOn(source, target, nullptr, CMD_FROM_SCRIPT)` (`ControlBar.cpp:637`)
    /// for a target object. An object-target button goes to `canDoSpecialPowerAtObject`
    /// (`ControlBar.cpp:681`). A location-target button goes to `canDoSpecialPowerAtLocation` with the
    /// target's position (`ControlBar.cpp:667-672`, `:686`). Any other button is `canDoSpecialPower`,
    /// which ignores the target (`ControlBar.cpp:689`).
    fn button_valid_for_object(&self, logic: &GameLogic, button: &CommandButtonPower, source: ObjectId, target: ObjectId) -> bool {
        if button.need_object_target {
            return logic.can_do_special_power_at_object(source, target, &button.power);
        }
        if button.need_target_pos {
            let Some(pos) = logic.find_object(target).map(|o| o.position) else { return false };
            return logic.can_do_special_power_at_location(source, &pos, &button.power);
        }
        logic.can_do_special_power_no_target(source, &button.power)
    }

    /// `CommandButton::isValidToUseOn(source, nullptr, nullptr, CMD_FROM_SCRIPT)` (`ControlBar.cpp:637`):
    /// with neither a target object nor a location, an object-target or location-target button is
    /// refused (`ControlBar.cpp:656-675`); any other button is `canDoSpecialPower`.
    fn button_valid_no_target(&self, logic: &GameLogic, button: &CommandButtonPower, source: ObjectId) -> bool {
        if button.need_object_target || button.need_target_pos {
            return false;
        }
        logic.can_do_special_power_no_target(source, &button.power)
    }

    /// The nearest object to `centre` that passes `accept` (`PartitionManager::getClosestObject`
    /// with `REALLY_FAR` and `FROM_CENTER_2D`): squared centre-to-centre distance, a strict `<`
    /// (`distCalcProc_CenterAndCenter_2D`, `PartitionManager.cpp:775`), so the first candidate in id
    /// order wins an exact tie. Contained objects are not registered with the partition manager
    /// (`OpenContain.cpp:257`), so they are skipped; so are destroyed and dying objects, which every
    /// filter rejects in the engine anyway.
    fn nearest(logic: &GameLogic, centre: &Coord3D, accept: impl Fn(&Object) -> bool) -> Option<(f32, ObjectId)> {
        let mut best: Option<(f32, ObjectId)> = None;
        for o in logic.objects() {
            if o.destroyed || o.is_dying() || o.contained_by.is_some() {
                continue;
            }
            let (dx, dy) = (o.position.x - centre.x, o.position.y - centre.y);
            let d = dx * dx + dy * dy;
            if d >= REALLY_FAR * REALLY_FAR || best.is_some_and(|(bd, _)| d >= bd) {
                continue;
            }
            if accept(o) {
                best = Some((d, o.id));
            }
        }
        best
    }

    /// The target of a team's `doTeamUseCommandButtonOnNearest*` action, or `None`.
    fn nearest_command_target(
        &self,
        logic: &GameLogic,
        owner: PlayerIndex,
        source: ObjectId,
        button: &CommandButtonPower,
        centre: &Coord3D,
        scope: &TargetScope,
    ) -> Option<ObjectId> {
        let valid = |o: &Object| self.button_valid_for_object(logic, button, source, o.id);
        if let TargetScope::ObjectType(object_type) = scope {
            // `ThingFactory::findTemplate(objectType)` first: a single template (`PartitionFilterThing`).
            if logic.find_template(object_type).is_some() {
                let f = Filter { template: Some(object_type.as_str()), ..Self::scope_filter(scope) };
                return Self::nearest(logic, centre, |o| self.filter_accepts(logic, owner, o, &f) && valid(o)).map(|(_, id)| id);
            }
            // Otherwise the object list: the nearest of its templates, the first listed one on exact ties
            // (`dist < closestDist`, `ScriptActions.cpp:5926`).
            let mut best: Option<(f32, ObjectId)> = None;
            for entry in self.object_lists.get(object_type).into_iter().flatten() {
                if logic.find_template(entry).is_none() {
                    continue;
                }
                let f = Filter { template: Some(entry.as_str()), ..Self::scope_filter(scope) };
                if let Some((d, id)) = Self::nearest(logic, centre, |o| self.filter_accepts(logic, owner, o, &f) && valid(o)) {
                    if best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, id));
                    }
                }
            }
            return best.map(|(_, id)| id);
        }
        let f = Self::scope_filter(scope);
        Self::nearest(logic, centre, |o| self.filter_accepts(logic, owner, o, &f) && valid(o)).map(|(_, id)| id)
    }

    /// `AIGroup::groupDoCommandButtonAtObject` for a special power: every member fires the power at
    /// `target` (`Object::doCommandButtonAtObject` -> `doSpecialPowerAtObject`, `CMD_FROM_SCRIPT`, so
    /// forced: `Object.cpp:5665`). Members without the power module, and disabled members, do nothing.
    /// `engine` is the script engine the action runs on; the fire's `PLAYER_TRIGGERED/COMPLETED_SPECIAL_POWER`
    /// events belong on it (`GameLogic::scripts` is empty while a script runs).
    fn group_power_at(
        logic: &mut GameLogic,
        engine: &mut ScriptEngine,
        owner: PlayerIndex,
        ids: &[ObjectId],
        power: &str,
        target: SpecialPowerTarget,
    ) {
        for id in ids {
            logic.script_do_special_power(owner, *id, power, target, engine);
        }
    }

    /// `ScriptActions::doTeamUseCommandButtonOnNearest*` (`ScriptActions.cpp:5591-5945`): the nearest
    /// valid target for the team's button, then the command to every member.
    pub(crate) fn team_use_command_button_nearest(&mut self, logic: &mut GameLogic, team: &str, ability: &str, scope: &TargetScope) {
        let Some(owner) = self.team_owner(team) else { return };
        let ids = self.team_members(logic, team);
        let Some(button) = self.button_power(ability) else { return };
        let Some(source) = Self::power_source(logic, &ids, &button.power) else { return };
        let Some(centre) = Self::group_centre(logic, &ids) else { return };
        let Some(target) = self.nearest_command_target(logic, owner, source, &button, &centre, scope) else { return };
        Self::group_power_at(logic, self, owner, &ids, &button.power, SpecialPowerTarget::Object(target));
    }

    /// `ScriptActions::doTeamUseCommandButtonOnNamed` (`ScriptActions.cpp:5550`): the team's button
    /// at the named unit, when `isValidToUseOn` allows it.
    pub(crate) fn team_use_command_button_on_named(&mut self, logic: &mut GameLogic, team: &str, ability: &str, unit: &str) {
        let Some(owner) = self.team_owner(team) else { return };
        let ids = self.team_members(logic, team);
        let Some(button) = self.button_power(ability) else { return };
        let Some(source) = Self::power_source(logic, &ids, &button.power) else { return };
        let Some(target) = self.named_object(unit) else { return };
        if !self.button_valid_for_object(logic, &button, source, target) {
            return;
        }
        Self::group_power_at(logic, self, owner, &ids, &button.power, SpecialPowerTarget::Object(target));
    }

    /// `ScriptActions::doTeamUseCommandButtonAbility` (`ScriptActions.cpp:4463`): every member uses the
    /// button with no target (`AIGroup::groupDoCommandButton`, `AIGroup.cpp:2961`, -> `Object::doCommandButton`
    /// for each member in team order). A special-power button fires its power; an upgrade button queues its
    /// upgrade at each member that can make it.
    pub(crate) fn team_use_command_button_ability(&mut self, logic: &mut GameLogic, team: &str, ability: &str) {
        let Some(owner) = self.team_owner(team) else { return };
        let ids = self.team_members(logic, team);
        if let Some(button) = self.button_power(ability) {
            Self::group_power_at(logic, self, owner, &ids, &button.power, SpecialPowerTarget::None);
            return;
        }
        let key = ability.to_ascii_lowercase();
        if let Some(upgrade) = self.command_button_upgrades.get(&key).cloned() {
            for id in ids {
                Self::do_command_button_upgrade(logic, id, &upgrade);
            }
            return;
        }
        // Every other button runs `Object::doCommandButton` for each member, in team order (`groupDoCommandButton`).
        let Some(action) = self.command_button_actions.get(&key).cloned() else { return };
        for id in ids {
            Self::do_command_button_action(logic, id, &action);
        }
    }

    /// `Object::doCommandButton` (Object.cpp:5504) for one member and a non-power, non-upgrade button. A disabled
    /// object does nothing (`isDisabled`, Object.cpp:5506). Each branch needs what the engine checks there: an AI
    /// (`ai`), a production interface (`pu`), and the target flags of a fire-weapon button. The fire, switch and
    /// stop branches run as the object's own orders with the script's source (`CMD_FROM_SCRIPT`).
    fn do_command_button_action(logic: &mut GameLogic, id: ObjectId, action: &CommandButtonAction) {
        let Some(o) = logic.find_object(id) else { return };
        if o.is_disabled() {
            return;
        }
        let (player, has_ai, has_production) = (o.player, o.ai_handle.is_some(), o.production.is_some());
        match action {
            CommandButtonAction::Stop => {
                if has_ai {
                    logic.ai_idle(id, CommandSource::Script);
                }
            }
            // `setWeaponLock(weaponSlot, LOCKED_PERMANENTLY)` (Object.cpp:5534); the member's own lock, as
            // `AIGroup::setWeaponLockForGroup` applies it one unit at a time.
            CommandButtonAction::SwitchWeapon { slot } => logic.command_switch_weapons(player, &[id], *slot),
            // `setWeaponLock(slot, LOCKED_TEMPORARILY)` and `aiAttackPosition(getPosition(), maxShots, cmdSource)`
            // (Object.cpp:5541-5546). A button that needs a target does nothing here (the DEBUG_CRASH branch).
            CommandButtonAction::FireWeapon { slot, max_shots, targets } => {
                if has_ai && !targets.needs_object_target() && !targets.position {
                    let pos = logic.find_object(id).map(|o| o.position).unwrap_or(Coord3D::ZERO);
                    fire_weapon_order(logic, id, AttackTarget::Ground(pos), *slot, *max_shots);
                }
            }
            // `queueCreateUnit(tt, requestUniqueUnitID())` (Object.cpp:5577-5583), for `UNIT_BUILD` and `DOZER_CONSTRUCT`.
            CommandButtonAction::UnitBuild { thing } | CommandButtonAction::DozerConstruct { thing } => {
                if has_production {
                    logic.queue_unit(player, id, thing);
                }
            }
            // `ai->aiHackInternet(cmdSource)` (Object.cpp:5587-5593): the hack order, without the weapon-lock
            // release of `MSG_INTERNET_HACK` (`hack_internet.rs`).
            CommandButtonAction::HackInternet => {
                if has_ai {
                    logic.ai_hack_internet(id);
                }
            }
            // `TheBuildAssistant->sellObject(this)` (Object.cpp:5596-5598).
            CommandButtonAction::Sell => logic.command_sell(player, id),
        }
    }

    /// The `PLAYER_UPGRADE` / `OBJECT_UPGRADE` branch of `Object::doCommandButton` (`Object.cpp:5504`, `:5556-5583`)
    /// for one member. A disabled object does nothing (`Object::doCommandButton` checks `isDisabled()` first).
    /// `Object::canProduceUpgrade` (`Object.cpp:6218`) needs the button in the object's command set, and
    /// `ProductionUpdate::queueUpgrade` (`ProductionUpdate.cpp:257`) charges the object's controlling player.
    /// As in `AIPlayer::buildUpgrade`'s `canUpgradeHere`, a source with no command-set data counts as no button.
    fn do_command_button_upgrade(logic: &mut GameLogic, id: ObjectId, button: &CommandButtonUpgrade) {
        let Some(o) = logic.find_object(id) else { return };
        if o.is_disabled() {
            return;
        }
        let (player, template) = (o.player, o.template.name.clone());
        let can_produce = logic.template_source.as_ref().and_then(|s| s.can_build_upgrade(&template, &button.upgrade)) == Some(true);
        if can_produce {
            // The object-upgrade gate (Object.cpp:5566) and `queueUpgrade`'s own checks are in `queue_upgrade_typed`.
            logic.queue_upgrade_typed(player, id, &button.upgrade, button.object);
        }
    }

    /// `CommandButton::isValidToUseOn(obj, nullptr, nullptr, CMD_FROM_SCRIPT)` for an upgrade button
    /// (ControlBar.cpp:640-655): the object has a production interface, its own queue holds no upgrade, and it is
    /// affected by the upgrade and does not have it.
    fn upgrade_button_valid(logic: &GameLogic, id: ObjectId, upgrade: &str) -> bool {
        let Some(st) = logic.find_object(id).and_then(|o| o.production.as_ref()) else { return false };
        if st.queue.iter().any(|e| matches!(e.kind, crate::production::ProductionKind::Upgrade(_))) {
            return false;
        }
        logic.affected_by_upgrade(id, upgrade) && !logic.has_upgrade(id, upgrade)
    }

    /// `ScriptActions::doTeamUseCommandButtonAbilityOnNamed` (`ScriptActions.cpp:4495`): the team's
    /// button at the named unit; no `isValidToUseOn` check on this path.
    pub(crate) fn team_use_command_button_ability_on_named(&mut self, logic: &mut GameLogic, team: &str, ability: &str, unit: &str) {
        let Some(owner) = self.team_owner(team) else { return };
        let ids = self.team_members(logic, team);
        if let Some(button) = self.button_power(ability) {
            let Some(target) = self.named_object(unit) else { return };
            Self::group_power_at(logic, self, owner, &ids, &button.power, SpecialPowerTarget::Object(target));
            return;
        }
        // A non-power button: `groupDoCommandButtonAtObject` (AIGroup.cpp:3019) runs `Object::doCommandButtonAtObject`
        // for each member, with no validity test on this path.
        let Some(action) = self.command_button_actions.get(&ability.to_ascii_lowercase()).cloned() else { return };
        let Some(target) = self.named_object(unit) else { return };
        for id in ids {
            Self::do_command_button_action_at_object(logic, id, &action, target);
        }
    }

    /// `Object::doCommandButtonAtObject` (Object.cpp:5634) for one member and a non-power, non-upgrade button, with the
    /// object `target`. A disabled object does nothing (Object.cpp:5636).
    fn do_command_button_action_at_object(logic: &mut GameLogic, id: ObjectId, action: &CommandButtonAction, target: ObjectId) {
        let Some(o) = logic.find_object(id) else { return };
        if o.is_disabled() {
            return;
        }
        let (has_ai, source_player) = (o.ai_handle.is_some(), o.player);
        match action {
            // `ai->aiIdle(cmdSource)` (Object.cpp:5670-5673).
            CommandButtonAction::Stop => {
                if has_ai {
                    logic.ai_idle(id, CommandSource::Script);
                }
            }
            // Object.cpp:5677-5704: an object-target fire needs an AI, a target object and `isValidObjectTarget`; the lock
            // is temporary, and `ATTACK_OBJECTS_POSITION` attacks the object's position instead of the object. A button
            // without an object target does nothing here (the DEBUG_CRASH branch).
            CommandButtonAction::FireWeapon { slot, max_shots, targets } => {
                if !has_ai || !targets.needs_object_target() {
                    return;
                }
                let Some(victim) = logic.find_object(target) else { return };
                let relationship = logic.relationship(source_player, victim.player);
                let valid = match relationship {
                    Relationship::Enemies => targets.enemy,
                    Relationship::Neutral => targets.neutral,
                    Relationship::Allies => targets.ally,
                };
                if !valid {
                    return;
                }
                let attack = if targets.attack_objects_position {
                    AttackTarget::Ground(victim.position)
                } else {
                    AttackTarget::Object(target)
                };
                fire_weapon_order(logic, id, attack, *slot, *max_shots);
            }
            // Every other command type does nothing at an object (the case list from Object.cpp:5723 and the default).
            _ => {}
        }
    }

    /// `Object::doCommandButtonAtPosition` (Object.cpp:5759) for one member and a non-power, non-upgrade button, at `pos`.
    /// A disabled object does nothing (Object.cpp:5761).
    fn do_command_button_action_at_position(logic: &mut GameLogic, id: ObjectId, action: &CommandButtonAction, pos: Coord3D) {
        let Some(o) = logic.find_object(id) else { return };
        if o.is_disabled() {
            return;
        }
        let has_ai = o.ai_handle.is_some();
        match action {
            // `ai->aiIdle(cmdSource)` (Object.cpp:5786-5789).
            CommandButtonAction::Stop => {
                if has_ai {
                    logic.ai_idle(id, CommandSource::Script);
                }
            }
            // `setWeaponLock(slot, LOCKED_TEMPORARILY)` and `aiAttackPosition(pos, maxShots)` for a `NEED_TARGET_POS`
            // button (Object.cpp:5798-5811); otherwise the DEBUG_CRASH branch does nothing.
            CommandButtonAction::FireWeapon { slot, max_shots, targets } if has_ai && targets.position => {
                fire_weapon_order(logic, id, AttackTarget::Ground(pos), *slot, *max_shots);
            }
            // `DOZER_CONSTRUCT` calls `TheBuildAssistant->buildObjectNow` (Object.cpp:5794-5795), which the sim does not
            // port: it does nothing here. The other types do nothing at a position.
            _ => {}
        }
    }

    /// `ScriptActions::doTeamPartialUseCommandButton` (`ScriptActions.cpp:5949-5983`): the members for
    /// which the button is valid with no target, in team order; the first `(Int)(percentage / 100 *
    /// count)` of them use it with no target (`obj->doCommandButton`, CMD_FROM_SCRIPT). The engine's
    /// `CEIL` is commented out, so the count truncates toward zero.
    pub(crate) fn team_partial_use_command_button(&mut self, logic: &mut GameLogic, team: &str, percentage: f32, ability: &str) {
        let Some(owner) = self.team_owner(team) else { return };
        let ids = self.team_members(logic, team);
        if let Some(button) = self.button_power(ability) {
            let valid: Vec<ObjectId> = {
                let l: &GameLogic = logic;
                ids.into_iter().filter(|id| self.button_valid_no_target(l, &button, *id)).collect()
            };
            let num = (percentage / 100.0 * valid.len() as f32) as i32;
            for (count, id) in valid.into_iter().enumerate() {
                if count as i32 >= num {
                    return;
                }
                // `Object::doCommandButton` (Object.cpp:5504): the special power with no target, forced by the script.
                logic.script_do_special_power(owner, id, &button.power, SpecialPowerTarget::None, self);
            }
            return;
        }
        // An upgrade button: the same count, over the members `CommandButton::isValidToUseOn` accepts
        // (`upgrade_button_valid`), each then running `Object::doCommandButton`'s upgrade branch.
        let Some(upgrade) = self.command_button_upgrades.get(&ability.to_ascii_lowercase()).cloned() else { return };
        let valid: Vec<ObjectId> = ids.into_iter().filter(|id| Self::upgrade_button_valid(logic, *id, &upgrade.upgrade)).collect();
        let num = (percentage / 100.0 * valid.len() as f32) as i32;
        for (count, id) in valid.into_iter().enumerate() {
            if count as i32 >= num {
                return;
            }
            Self::do_command_button_upgrade(logic, id, &upgrade);
        }
    }

    /// `ScriptActions::doTeamUseCommandButtonAbilityAtWaypoint` (`ScriptActions.cpp:4533`): every
    /// member fires the special power at the waypoint's position (`Object::doCommandButtonAtPosition`,
    /// Object.cpp:5769, `doSpecialPowerAtLocation` with an invalid angle). No validity test on this
    /// path. An unknown waypoint or button does nothing. Waypoint names match case-insensitively
    /// (the engine's `NameKeyType` lookup).
    pub(crate) fn team_use_command_button_ability_at_waypoint(&mut self, logic: &mut GameLogic, team: &str, ability: &str, waypoint: &str) {
        let Some(owner) = self.team_owner(team) else { return };
        let ids = self.team_members(logic, team);
        let Some(pos) = self
            .waypoints
            .get(waypoint)
            .or_else(|| self.waypoints.iter().find(|(k, _)| k.eq_ignore_ascii_case(waypoint)).map(|(_, v)| v))
            .copied()
        else {
            return;
        };
        if let Some(button) = self.button_power(ability) {
            for id in ids {
                logic.script_do_special_power_at_location(owner, id, &button.power, pos, self);
            }
            return;
        }
        // A non-power button: `groupDoCommandButtonAtPosition` (AIGroup.cpp:2981) runs `Object::doCommandButtonAtPosition`
        // for each member.
        let Some(action) = self.command_button_actions.get(&ability.to_ascii_lowercase()).cloned() else { return };
        for id in ids {
            Self::do_command_button_action_at_position(logic, id, &action, pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use zr_map::scripts::{Action, Condition, OrCondition, ParamValue, Parameter, Script, ScriptList};
    use zr_map::{Dict, DictValue};

    use crate::combat::AttackTarget;
    use crate::damage::Relationship;
    use crate::disabled::DisabledType;
    use crate::logic::GameLogic;
    use crate::math::Coord3D;
    use crate::object::{ObjectId, PlayerIndex};
    use crate::object::status;
    use crate::script::{CommandButtonAction, CommandButtonPower, CommandButtonUpgrade, FireTargets, ScriptEngine, TeamInstance};
    use crate::special_power::{AbilityState, PackingState, SimSpecialPower, SimSpecialPowerTemplate, SpecialAbilityTemplate, SpecialPowerEffect, SpecialPowerTarget};
    use crate::template::{kind_of, LocomotorKind, LocomotorParams};
    use crate::{ObjectTemplate, TemplateSource, TerrainLogic};

    struct Store(Vec<Arc<ObjectTemplate>>);
    impl TemplateSource for Store {
        fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
            self.0.iter().find(|t| t.name == name).cloned()
        }
    }

    /// A capturer-like infantry: one object-target power `P` (a cash hack for 100).
    fn hacker() -> ObjectTemplate {
        let mut t = ObjectTemplate::new("Hacker");
        t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 100.0, 100.0, 360.0));
        t.special_powers = vec![Arc::new(SimSpecialPower {
            power: "P".into(),
            effect: SpecialPowerEffect::CashHack { amount: 100 },
            starts_paused: false,
            power_module: true,
        })];
        t
    }

    /// An enemy-capturable building that is also a cash generator, so a cash hack can take it.
    fn building() -> ObjectTemplate {
        let mut t = ObjectTemplate::new("Bldg");
        t.kind_of = kind_of::STRUCTURE | kind_of::IMMOBILE | kind_of::CAPTURABLE | kind_of::CASH_GENERATOR;
        t.geometry.major_radius = 10.0;
        t.max_health = 500.0;
        t.initial_health = 500.0;
        t
    }

    fn tank() -> ObjectTemplate {
        let mut t = ObjectTemplate::new("Tank");
        t.kind_of = kind_of::VEHICLE | kind_of::SELECTABLE;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Treads, 30.0, 60.0, 120.0, 120.0));
        t
    }

    /// A demolition unit for power `P`: the `SpecialAbility` power module and its `SpecialAbilityUpdate`
    /// with one charge slot, so the charge branch of `canDoSpecialPowerAtObject` applies.
    fn charger() -> ObjectTemplate {
        let module = SpecialAbilityTemplate {
            power: "P".into(),
            start_ability_range: 10_000_000.0,
            ability_abort_range: 10_000_000.0,
            approach_requires_los: true,
            preparation_frames: 0,
            pack_time: 0,
            unpack_time: 0,
            pack_unpack_variation: 0.0,
            skip_packing_with_no_target: false,
            award_xp_for_triggering: 0,
            skill_points_for_triggering: -1,
            effect_duration: 0,
            effect_value: 0,
            special_object: None,
            max_special_objects: 1,
            need_to_face: false,
            flee_range_after_completion: 0.0,
            starts_paused: false,
            power_module: true,
        };
        let update = SpecialAbilityTemplate { power_module: false, ..module.clone() };
        let mut t = ObjectTemplate::new("Charger");
        t.kind_of = kind_of::INFANTRY | kind_of::SELECTABLE;
        t.max_health = 100.0;
        t.initial_health = 100.0;
        t.locomotor = Some(LocomotorParams::from_ini(LocomotorKind::Legs, 20.0, 100.0, 100.0, 360.0));
        t.special_abilities = vec![Arc::new(module), Arc::new(update)];
        t
    }

    /// An object-target button (`Cmd_Hack`) for power `P` of the given engine kind.
    fn world(kind: &str) -> GameLogic {
        world_with(kind, true, false)
    }

    /// Players 0 (the team), 1 and 2 (enemies of 0). Power `P` has the given engine kind; the button
    /// `Cmd_Hack` has the given target options.
    fn world_with(kind: &str, need_object_target: bool, need_target_pos: bool) -> GameLogic {
        world_full(kind, need_object_target, need_target_pos, None, 0)
    }

    /// As [`world_with`], with power `P` requiring `science` (if any) and reloading in `reload` frames.
    fn world_full(kind: &str, need_object_target: bool, need_target_pos: bool, science: Option<&str>, reload: u32) -> GameLogic {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        for p in 0..3 {
            g.add_player(p, 1000);
        }
        g.set_relationship_symmetric(0, 1, Relationship::Enemies);
        g.set_relationship_symmetric(0, 2, Relationship::Enemies);
        let mut powers = BTreeMap::new();
        powers.insert(
            "P".to_string(),
            Arc::new(SimSpecialPowerTemplate {
                name: "P".into(),
                kind: kind.into(),
                reload_time: reload,
                required_science: science.map(str::to_owned),
                shared_synced_timer: false,
                public_timer: false,
                radius_cursor_radius: 0.0,
            }),
        );
        g.set_special_powers(powers);
        g.set_template_source(Arc::new(Store(vec![Arc::new(hacker()), Arc::new(building()), Arc::new(tank()), Arc::new(charger())])));
        g.scripts_mut().load_command_button_powers([(
            "Cmd_Hack".to_owned(),
            CommandButtonPower { power: "P".into(), need_object_target, need_target_pos },
        )]);
        g
    }

    /// Creates a team `MyTeam` (owner `<This Player>`) with `members`.
    fn team(g: &mut GameLogic, members: &[ObjectId]) {
        let d = Dict(vec![
            ("teamName".into(), DictValue::Ascii("MyTeam".into())),
            ("teamOwner".into(), DictValue::Ascii("<This Player>".into())),
            ("teamIsAIRecruitable".into(), DictValue::Bool(true)),
            ("teamProductionPriority".into(), DictValue::Int(100)),
            ("teamProductionPrioritySuccessIncrease".into(), DictValue::Int(20)),
            ("teamProductionPriorityFailureDecrease".into(), DictValue::Int(5)),
        ]);
        g.scripts_mut().load_teams(&[d]);
        let mut t = TeamInstance::new("myteam");
        t.created = true;
        t.members = members.to_vec();
        g.scripts_mut().teams.insert("myteam".to_owned(), t);
    }

    /// Runs one script action once for player 0 (the team's owner), then lets the command land.
    fn run_action(g: &mut GameLogic, action_name: &str, params: Vec<Parameter>) {
        let script = Script {
            version: 2,
            name: "command_button".into(),
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
            actions: vec![Action { version: 2, type_id: 0, internal_name: Some(action_name.into()), params }],
            actions_false: Vec::new(),
        };
        g.scripts_mut().load(vec![(0, ScriptList { scripts: vec![script], groups: Vec::new() })]);
        g.run_frames(2);
    }

    fn team_param() -> Parameter {
        Parameter { kind: 3, value: ParamValue::Plain { int: 0, real: 0.0, string: "MyTeam".into() } }
    }
    fn str_param(s: &str) -> Parameter {
        Parameter { kind: 0, value: ParamValue::Plain { int: 0, real: 0.0, string: s.to_owned() } }
    }

    fn spawn(g: &mut GameLogic, name: &str, pos: (f32, f32), player: PlayerIndex) -> ObjectId {
        let t = g.find_template(name).unwrap();
        g.create_object(&t, Coord3D::new(pos.0, pos.1, 0.0), 0.0, player)
    }

    #[test]
    fn nearest_enemy_building_skips_structures_under_construction() {
        let mut g = world("SPECIAL_CASH_HACK");
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        let site = g.find_template("Bldg").unwrap();
        g.create_object_under_construction(&site, Coord3D::new(120.0, 100.0, 0.0), 0.0, 1);
        spawn(&mut g, "Bldg", (160.0, 100.0), 2);
        team(&mut g, &[hacker]);
        run_action(
            &mut g,
            "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_ENEMY_BUILDING",
            vec![team_param(), str_param("Cmd_Hack")],
        );
        // The nearer site is under construction and so fails `isValidToUseOn`; the completed building
        // further out is the target.
        assert_eq!(g.player_money(2), 900, "the completed enemy building lost 100");
        assert_eq!(g.player_money(1), 1000, "the site under construction is not a target");
        assert_eq!(g.player_money(0), 1100, "the hack paid the hacker");
    }

    #[test]
    fn nearest_enemy_unit_ignores_own_units() {
        let mut g = world("SPECIAL_MISSILE_DEFENDER_LASER_GUIDED_MISSILES");
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        // An own tank nearer than the enemy tank: `isValidToUseOn` rejects it (not an enemy).
        spawn(&mut g, "Tank", (112.0, 100.0), 0);
        spawn(&mut g, "Tank", (130.0, 100.0), 1);
        team(&mut g, &[hacker]);
        run_action(&mut g, "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_ENEMY_UNIT", vec![team_param(), str_param("Cmd_Hack")]);
        assert_eq!(g.player_money(1), 900, "the enemy tank was the target");
        assert_eq!(g.player_money(0), 1100);
    }

    #[test]
    fn object_type_uses_the_listed_template_that_is_a_valid_target() {
        let mut g = world("SPECIAL_CASH_HACK");
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        // An enemy tank is nearer, but a cash hack can only take a structure.
        spawn(&mut g, "Tank", (110.0, 100.0), 1);
        spawn(&mut g, "Bldg", (140.0, 100.0), 2);
        team(&mut g, &[hacker]);
        run_action(
            &mut g,
            "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_OBJECTTYPE",
            vec![team_param(), str_param("Cmd_Hack"), str_param("Bldg")],
        );
        assert_eq!(g.player_money(2), 900);
        assert_eq!(g.player_money(1), 1000);
    }

    #[test]
    fn target_is_the_nearest_to_the_group_centre_not_to_the_first_member() {
        let mut g = world("SPECIAL_CASH_HACK");
        // Two hackers at x=100 and x=110: the centre is x=105.
        let a = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        let b = spawn(&mut g, "Hacker", (110.0, 100.0), 0);
        // The building at x=96 (player 1) is 4 from the first member and 9 from the centre; the one at
        // x=112 (player 2) is 12 from the first member and 7 from the centre. The centre wins.
        spawn(&mut g, "Bldg", (96.0, 100.0), 1);
        spawn(&mut g, "Bldg", (112.0, 100.0), 2);
        team(&mut g, &[a, b]);
        run_action(&mut g, "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_ENEMY_BUILDING", vec![team_param(), str_param("Cmd_Hack")]);
        // Both hackers take 100 each from the chosen building.
        assert_eq!(g.player_money(2), 800);
        assert_eq!(g.player_money(1), 1000);
    }

    #[test]
    fn group_centre_skips_held_riders_and_counts_ai_members() {
        let mut g = world("SPECIAL_CASH_HACK");
        let a = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        let b = spawn(&mut g, "Hacker", (110.0, 100.0), 0);
        let rider = spawn(&mut g, "Hacker", (300.0, 300.0), 0);
        g.set_disabled(rider, DisabledType::Held);
        let c = ScriptEngine::group_centre(&g, &[a, b, rider]).expect("two counted members");
        assert!((c.x - 105.0).abs() < 1e-4 && (c.y - 100.0).abs() < 1e-4, "centre {c:?}");
    }

    #[test]
    fn named_target_needs_a_valid_target() {
        let mut g = world("SPECIAL_CASH_HACK");
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        let site = g.find_template("Bldg").unwrap();
        let under = g.create_object_under_construction(&site, Coord3D::new(120.0, 100.0, 0.0), 0.0, 1);
        let done = spawn(&mut g, "Bldg", (140.0, 100.0), 2);
        g.scripts_mut().add_named_object("Site", under);
        g.scripts_mut().add_named_object("Done", done);
        team(&mut g, &[hacker]);
        run_action(&mut g, "TEAM_ALL_USE_COMMANDBUTTON_ON_NAMED", vec![team_param(), str_param("Cmd_Hack"), str_param("Site")]);
        assert_eq!(g.player_money(1), 1000, "a structure under construction is refused");
        run_action(&mut g, "TEAM_ALL_USE_COMMANDBUTTON_ON_NAMED", vec![team_param(), str_param("Cmd_Hack"), str_param("Done")]);
        assert_eq!(g.player_money(2), 900);
    }

    fn real_param(v: f32) -> Parameter {
        Parameter { kind: 1, value: ParamValue::Plain { int: 0, real: v, string: String::new() } }
    }

    /// Spawns `n` hackers in a row for player 0 and makes them the team `MyTeam`.
    fn squad(g: &mut GameLogic, n: usize) {
        let ids: Vec<ObjectId> = (0..n).map(|i| spawn(g, "Hacker", (100.0 + 10.0 * i as f32, 100.0), 0)).collect();
        team(g, &ids);
    }

    /// `doTeamPartialUseCommandButton` (ScriptActions.cpp:5949-5983): 50% of three valid members is
    /// `(Int)(0.5 * 3) = 1` use with no target; the no-target cash hack pays 100 to the owner.
    #[test]
    fn partial_uses_the_truncated_share_of_valid_members() {
        let mut g = world_with("SPECIAL_CIA_INTELLIGENCE", false, false);
        squad(&mut g, 3);
        run_action(&mut g, "TEAM_PARTIAL_USE_COMMANDBUTTON", vec![real_param(50.0), team_param(), str_param("Cmd_Hack")]);
        assert_eq!(g.player_money(0), 1100, "one of the three members fired");
    }

    #[test]
    fn partial_at_full_percentage_fires_every_valid_member() {
        let mut g = world_with("SPECIAL_CIA_INTELLIGENCE", false, false);
        squad(&mut g, 3);
        run_action(&mut g, "TEAM_PARTIAL_USE_COMMANDBUTTON", vec![real_param(100.0), team_param(), str_param("Cmd_Hack")]);
        assert_eq!(g.player_money(0), 1300, "all three members fired");
    }

    /// An object-target button is never valid with no target (`isValidToUseOn` with a null target).
    #[test]
    fn partial_skips_object_target_buttons() {
        let mut g = world("SPECIAL_CASH_HACK");
        squad(&mut g, 3);
        run_action(&mut g, "TEAM_PARTIAL_USE_COMMANDBUTTON", vec![real_param(100.0), team_param(), str_param("Cmd_Hack")]);
        assert_eq!(g.player_money(0), 1000);
    }

    /// `doTeamUseCommandButtonAbility` (Object.cpp:5504): every member fires the no-target power, with
    /// no `canDoSpecialPower` test, so a cash hack that partial refuses still fires.
    #[test]
    fn ability_fires_a_no_target_power_from_every_member() {
        let mut g = world_with("SPECIAL_CASH_HACK", false, false);
        squad(&mut g, 2);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_Hack")]);
        assert_eq!(g.player_money(0), 1200);
    }

    /// `doTeamUseCommandButtonAbilityAtWaypoint` (ScriptActions.cpp:4533): the power fires at the
    /// waypoint position from every member; the waypoint name matches case-insensitively.
    #[test]
    fn ability_at_waypoint_fires_at_the_position_for_every_member() {
        let mut g = world_with("SPECIAL_CASH_HACK", false, true);
        squad(&mut g, 2);
        g.scripts_mut().set_waypoints([("Spot".to_owned(), Coord3D::new(150.0, 150.0, 0.0))]);
        run_action(
            &mut g,
            "TEAM_USE_COMMANDBUTTON_ABILITY_AT_WAYPOINT",
            vec![team_param(), str_param("Cmd_Hack"), str_param("SPOT")],
        );
        assert_eq!(g.player_money(0), 1200);
    }

    #[test]
    fn ability_at_waypoint_needs_a_known_waypoint() {
        let mut g = world_with("SPECIAL_CASH_HACK", false, true);
        squad(&mut g, 2);
        g.scripts_mut().set_waypoints([("Spot".to_owned(), Coord3D::new(150.0, 150.0, 0.0))]);
        run_action(
            &mut g,
            "TEAM_USE_COMMANDBUTTON_ABILITY_AT_WAYPOINT",
            vec![team_param(), str_param("Cmd_Hack"), str_param("Nowhere")],
        );
        assert_eq!(g.player_money(0), 1000);
    }

    /// `ActionManager::canCaptureBuilding` (ActionManager.cpp:1059-1081): a sold building, and a
    /// stealthed enemy that is neither detected nor disguised, cannot be captured.
    #[test]
    fn capture_refuses_sold_and_undetected_stealthed_buildings() {
        let mut g = world("SPECIAL_INFANTRY_CAPTURE_BUILDING");
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        let bldg = spawn(&mut g, "Bldg", (120.0, 100.0), 1);
        assert!(g.can_capture_building(hacker, bldg), "an enemy building is capturable");
        g.objects.get_mut(&bldg).unwrap().status_bits = status::SOLD;
        assert!(!g.can_capture_building(hacker, bldg), "a sold building is refused");
        g.objects.get_mut(&bldg).unwrap().status_bits = status::STEALTHED;
        assert!(!g.can_capture_building(hacker, bldg), "stealthed and undetected is refused");
        g.objects.get_mut(&bldg).unwrap().status_bits = status::STEALTHED | status::DETECTED;
        assert!(g.can_capture_building(hacker, bldg), "a detected stealthed building is capturable");
    }

    /// `Object::doCommandButton` with `CMD_FROM_SCRIPT` is forced (Object.cpp:5518): the science test of
    /// `canUseSpecialPower` (Object.cpp:5431) is skipped. The player's own fire still makes it.
    #[test]
    fn script_fire_skips_the_science_test_a_player_fire_does_not() {
        let mut g = world_full("SPECIAL_CASH_HACK", false, false, Some("SCIENCE_TEST"), 0);
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        assert!(!g.command_do_special_power(0, hacker, "P", SpecialPowerTarget::None), "the player's fire needs the science");
        assert_eq!(g.player_money(0), 1000);
        team(&mut g, &[hacker]);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_Hack")]);
        assert_eq!(g.player_money(0), 1100, "the script's fire is forced past the science test");
    }

    /// The forced fire also skips the readiness test (the object layer makes none, Object.cpp:5424-5438),
    /// and it restarts the cooldown. A player's fire waits for the module.
    #[test]
    fn script_fire_ignores_the_cooldown_a_player_fire_waits_for() {
        let mut g = world_full("SPECIAL_CASH_HACK", false, false, None, 1000);
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        assert!(!g.command_do_special_power(0, hacker, "P", SpecialPowerTarget::None), "the module recharges for 1000 frames");
        team(&mut g, &[hacker]);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_Hack")]);
        assert_eq!(g.player_money(0), 1100, "the script fired during the cooldown");
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_Hack")]);
        assert_eq!(g.player_money(0), 1200, "the next script fire is not refused either");
    }

    /// `canDoSpecialPowerAtLocation` (ActionManager.cpp:1566): a map-only power is valid on the map and not
    /// off it, and a point is validated on x and y only (`isPointOnMap`, :1463).
    #[test]
    fn map_only_location_power_needs_a_point_on_the_map() {
        let mut g = world_with("SPECIAL_SPY_SATELLITE", false, true);
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        assert!(g.can_do_special_power_at_location(hacker, &Coord3D::new(200.0, 200.0, 0.0), "P"));
        assert!(!g.can_do_special_power_at_location(hacker, &Coord3D::new(-50.0, 200.0, 0.0), "P"), "off the map");
        assert!(!g.can_do_special_power_at_location(hacker, &Coord3D::new(200.0, 1.0e6, 0.0), "P"), "far off the map");
        // `SPECIAL_LAUNCH_BAIKONUR_ROCKET` is `return TRUE` anywhere (:1567).
        let mut g = world_with("SPECIAL_LAUNCH_BAIKONUR_ROCKET", false, true);
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        assert!(g.can_do_special_power_at_location(hacker, &Coord3D::new(-50.0, 1.0e6, 0.0), "P"));
    }

    /// A location-target button on the team's nearest enemy unit: the target's position is the location
    /// (`ControlBar.cpp:667-672`, `:686`), so the button is valid there. It was refused before.
    #[test]
    fn nearest_enemy_unit_fires_a_location_power_at_its_position() {
        let mut g = world_with("SPECIAL_SPY_SATELLITE", false, true);
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        spawn(&mut g, "Tank", (130.0, 100.0), 1);
        team(&mut g, &[hacker]);
        run_action(&mut g, "TEAM_ALL_USE_COMMANDBUTTON_ON_NEAREST_ENEMY_UNIT", vec![team_param(), str_param("Cmd_Hack")]);
        assert_eq!(g.power_fire_counts.get("P"), Some(&1), "the enemy tank's position was a valid location");
    }

    /// `canDisableVehicleViaHacking` with `checkSourceRequirements` false (ActionManager.cpp:1107): an
    /// enemy vehicle is a target unless it is stealthed and neither detected nor disguised.
    #[test]
    fn disable_vehicle_hack_targets_an_enemy_vehicle_only() {
        let mut g = world("SPECIAL_BLACKLOTUS_DISABLE_VEHICLE_HACK");
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        let enemy_tank = spawn(&mut g, "Tank", (120.0, 100.0), 1);
        let own_tank = spawn(&mut g, "Tank", (120.0, 120.0), 0);
        let bldg = spawn(&mut g, "Bldg", (140.0, 100.0), 1);
        assert!(g.can_do_special_power_at_object(hacker, enemy_tank, "P"), "an enemy vehicle is hacked");
        assert!(!g.can_do_special_power_at_object(hacker, own_tank, "P"), "an own vehicle is not");
        assert!(!g.can_do_special_power_at_object(hacker, bldg, "P"), "a structure is not a vehicle");
        g.objects.get_mut(&enemy_tank).unwrap().status_bits = status::STEALTHED;
        assert!(!g.can_do_special_power_at_object(hacker, enemy_tank, "P"), "stealthed and undetected");
        g.objects.get_mut(&enemy_tank).unwrap().status_bits = status::STEALTHED | status::DETECTED;
        assert!(g.can_do_special_power_at_object(hacker, enemy_tank, "P"), "a detected stealthed vehicle is hacked");
    }

    /// `canStealCashViaHacking` (ActionManager.cpp:1231): the module must be fully ready whatever the
    /// caller passed (:1252-1257), and the target must be an enemy cash generator (:1280).
    #[test]
    fn steal_cash_hack_needs_a_ready_module_and_a_cash_generator() {
        let mut g = world_with("SPECIAL_BLACKLOTUS_STEAL_CASH_HACK", true, false);
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        let bldg = spawn(&mut g, "Bldg", (120.0, 100.0), 1);
        let mut recharging = world_full("SPECIAL_BLACKLOTUS_STEAL_CASH_HACK", true, false, None, 1000);
        let hacker2 = spawn(&mut recharging, "Hacker", (100.0, 100.0), 0);
        let bldg2 = spawn(&mut recharging, "Bldg", (120.0, 100.0), 1);
        assert!(!recharging.can_steal_cash_via_hacking(hacker2, bldg2, "P"), "the module is still recharging");
        assert!(!recharging.can_do_special_power_at_object(hacker2, bldg2, "P"), "and so the object query refuses too");
        assert!(g.can_steal_cash_via_hacking(hacker, bldg, "P"), "a ready module hacks an enemy cash generator");
        let tank = spawn(&mut g, "Tank", (130.0, 100.0), 1);
        assert!(!g.can_steal_cash_via_hacking(hacker, tank, "P"), "a vehicle has no cash");
    }

    /// `canMakeObjectDefector` (ActionManager.cpp:967) through `SPECIAL_DEFECTOR` (:1761): an enemy unit
    /// defects, a building does not, and an own unit is not an enemy.
    #[test]
    fn defector_targets_an_enemy_unit_not_a_building() {
        let mut g = world("SPECIAL_DEFECTOR");
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        let enemy_tank = spawn(&mut g, "Tank", (120.0, 100.0), 1);
        let own_tank = spawn(&mut g, "Tank", (120.0, 120.0), 0);
        let bldg = spawn(&mut g, "Bldg", (140.0, 100.0), 1);
        assert!(g.can_do_special_power_at_object(hacker, enemy_tank, "P"));
        assert!(!g.can_do_special_power_at_object(hacker, own_tank, "P"));
        assert!(!g.can_do_special_power_at_object(hacker, bldg, "P"), "buildings do not defect");
    }

    /// `SPECIAL_DISGUISE_AS_VEHICLE` (ActionManager.cpp:1740): a vehicle that is not an aircraft or a boat.
    /// The railroad and cliff-jumper exclusions are not modelled (see `enter_actions.rs`).
    #[test]
    fn disguise_targets_a_vehicle_not_a_building() {
        let mut g = world("SPECIAL_DISGUISE_AS_VEHICLE");
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        let enemy_tank = spawn(&mut g, "Tank", (120.0, 100.0), 1);
        let bldg = spawn(&mut g, "Bldg", (140.0, 100.0), 1);
        assert!(g.can_do_special_power_at_object(hacker, enemy_tank, "P"));
        assert!(!g.can_do_special_power_at_object(hacker, bldg, "P"));
    }

    /// The charge branch (ActionManager.cpp:1828-1877): a structure or vehicle target, from a unit with a
    /// `SpecialAbilityUpdate` for the power that has a free charge slot. The producer tests are not modelled.
    #[test]
    fn remote_charges_need_an_update_and_a_free_slot() {
        let mut g = world_with("SPECIAL_REMOTE_CHARGES", true, false);
        let demo = spawn(&mut g, "Charger", (100.0, 100.0), 0);
        let bldg = spawn(&mut g, "Bldg", (120.0, 100.0), 1);
        let tank = spawn(&mut g, "Tank", (130.0, 100.0), 1);
        assert!(g.can_do_special_power_at_object(demo, bldg, "P"), "a free slot places a charge on a structure");
        assert!(g.can_do_special_power_at_object(demo, tank, "P"), "and on a vehicle");
        // The hacker has the power module but no `SpecialAbilityUpdate`: `findSpecialAbilityUpdate` is null.
        let hacker = spawn(&mut g, "Hacker", (100.0, 120.0), 0);
        assert!(!g.can_do_special_power_at_object(hacker, bldg, "P"), "no update module, no charge");
        // One charge already placed fills the single slot (`getSpecialObjectCount() < getSpecialObjectMax()`).
        g.objects.get_mut(&demo).unwrap().ability = Some(AbilityState {
            power: "P".into(),
            target_obj: None,
            target_pos: Coord3D::ZERO,
            prep_frames: 0,
            packing: PackingState::None,
            anim_frames: 0,
            within_start_range: false,
            facing_initiated: false,
            facing_complete: false,
            no_target_command: false,
            special_objects: vec![bldg],
        });
        assert!(!g.can_do_special_power_at_object(demo, tank, "P"), "the slot is full");
    }

    /// A factory with a production interface, named `name` (the command set decides who may build).
    fn factory(name: &str) -> ObjectTemplate {
        let mut t = ObjectTemplate::new(name);
        t.kind_of = kind_of::STRUCTURE | kind_of::SELECTABLE;
        t.max_health = 500.0;
        t.initial_health = 500.0;
        t.production = Some(crate::production::ProductionTemplate {
            max_queue_entries: 5,
            quantity_modifiers: vec![],
            exit: crate::production::ProductionExit::default(),
            ..Default::default()
        });
        t
    }

    /// `Plant`, a factory whose object upgrade `Upgrade_Y` it is affected by (a module triggered by it).
    fn plant() -> ObjectTemplate {
        let mut t = factory("Plant");
        t.upgrade_modules = vec![Arc::new(crate::upgrade::SimUpgradeModule {
            triggered_by: vec!["Upgrade_Y".into()],
            conflicts_with: vec![],
            removes_upgrades: vec![],
            requires_all_triggers: false,
            effect: crate::upgrade::UpgradeEffect::PowerPlant,
        })];
        t
    }

    /// Templates with command sets: `Plant` holds the player-upgrade button of `Upgrade_X`; both `Plant` and
    /// `Foundry` hold the object-upgrade button of `Upgrade_Y`.
    struct Builds(Vec<Arc<ObjectTemplate>>);
    impl TemplateSource for Builds {
        fn find_template(&self, name: &str) -> Option<Arc<ObjectTemplate>> {
            self.0.iter().find(|t| t.name == name).cloned()
        }
        fn can_build_upgrade(&self, builder: &str, upgrade: &str) -> Option<bool> {
            Some(match upgrade {
                "Upgrade_X" => builder == "Plant",
                "Upgrade_Y" => builder == "Plant" || builder == "Foundry",
                _ => false,
            })
        }
    }

    /// Player 0 with `Upgrade_X` (a player upgrade, cost 500) behind `Cmd_UpgradeX` and `Upgrade_Y` (an object
    /// upgrade, cost 500) behind `Cmd_UpgradeY`, and team `MyTeam` of `members` (template, position). Returns the
    /// world and the members' ids in team order.
    fn upgrade_world(members: &[(&str, (f32, f32))]) -> (GameLogic, Vec<ObjectId>) {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        g.set_upgrade_info(
            [
                ("Upgrade_X".to_string(), crate::upgrade::UpgradeInfo { cost: 500, build_time_frames: 100 }),
                ("Upgrade_Y".to_string(), crate::upgrade::UpgradeInfo { cost: 500, build_time_frames: 100 }),
            ]
            .into_iter()
            .collect(),
        );
        g.set_template_source(Arc::new(Builds(vec![Arc::new(plant()), Arc::new(factory("Foundry")), Arc::new(tank())])));
        g.scripts_mut().load_command_button_upgrades([
            ("Cmd_UpgradeX".to_owned(), CommandButtonUpgrade { upgrade: "Upgrade_X".into(), object: false }),
            ("Cmd_UpgradeY".to_owned(), CommandButtonUpgrade { upgrade: "Upgrade_Y".into(), object: true }),
        ]);
        let ids: Vec<ObjectId> = members.iter().map(|(name, pos)| spawn(&mut g, name, *pos, 0)).collect();
        team(&mut g, &ids);
        (g, ids)
    }

    fn queue_len(g: &GameLogic, id: ObjectId) -> usize {
        g.find_object(id).and_then(|o| o.production.as_ref()).map_or(0, |p| p.queue.len())
    }

    /// `doTeamUseCommandButtonAbility` (ScriptActions.cpp:4463) -> `Object::doCommandButton` (Object.cpp:5504)
    /// for each member: the factory whose command set holds the button queues the upgrade and the player
    /// pays for it; the tank has no production interface and does nothing.
    #[test]
    fn team_upgrade_button_queues_the_upgrade_at_the_factory() {
        let (mut g, ids) = upgrade_world(&[("Plant", (100.0, 100.0)), ("Tank", (130.0, 100.0))]);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_UpgradeX")]);
        let queue = &g.find_object(ids[0]).unwrap().production.as_ref().unwrap().queue;
        assert_eq!(queue.len(), 1);
        assert!(matches!(&queue[0].kind, crate::production::ProductionKind::Upgrade(n) if n == "Upgrade_X"));
        assert_eq!(g.player(0).unwrap().money, 500, "the cost is withdrawn from the controlling player");
    }

    /// `ProductionUpdate::queueUpgrade` (ProductionUpdate.cpp:288-290): a player upgrade is produced at one
    /// factory at a time, so the second factory of the team refuses it and the cost is taken once.
    #[test]
    fn team_upgrade_button_queues_a_player_upgrade_once() {
        let (mut g, ids) = upgrade_world(&[("Plant", (100.0, 100.0)), ("Plant", (200.0, 100.0))]);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_UpgradeX")]);
        assert_eq!(queue_len(&g, ids[0]), 1, "the first factory in team order takes it");
        assert_eq!(queue_len(&g, ids[1]), 0, "the second is refused while the player produces it");
        assert_eq!(g.player(0).unwrap().money, 500, "paid once");
    }

    /// `Object::canProduceUpgrade` (Object.cpp:6218): a factory whose command set holds no button for the
    /// upgrade does not queue it.
    #[test]
    fn team_upgrade_button_needs_the_button_in_the_command_set() {
        let (mut g, ids) = upgrade_world(&[("Foundry", (100.0, 100.0))]);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_UpgradeX")]);
        assert_eq!(queue_len(&g, ids[0]), 0);
        assert_eq!(g.player(0).unwrap().money, 1000, "nothing is paid");
    }

    /// `Object::doCommandButton` returns first when the object is disabled (`isDisabled`, Object.cpp:5504).
    #[test]
    fn team_upgrade_button_skips_a_disabled_member() {
        let (mut g, ids) = upgrade_world(&[("Plant", (100.0, 100.0))]);
        g.set_disabled(ids[0], DisabledType::Emp);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_UpgradeX")]);
        assert_eq!(queue_len(&g, ids[0]), 0);
        assert_eq!(g.player(0).unwrap().money, 1000);
    }

    /// An object upgrade (`Upgrade_Y`, `UPGRADE_TYPE_OBJECT`) is queued on the factory with no player money check
    /// (`queueUpgrade`, ProductionUpdate.cpp:266-274), and when it finishes the object gets it, not the player
    /// (`us->giveUpgrade`, ProductionUpdate.cpp:900-906).
    #[test]
    fn team_object_upgrade_button_gives_the_upgrade_to_the_factory() {
        let (mut g, ids) = upgrade_world(&[("Plant", (100.0, 100.0))]);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_UpgradeY")]);
        let queue = &g.find_object(ids[0]).unwrap().production.as_ref().unwrap().queue;
        assert_eq!(queue.len(), 1);
        assert!(queue[0].object_upgrade);
        assert_eq!(g.player(0).unwrap().money, 500);
        g.run_frames(110);
        assert!(g.has_upgrade(ids[0], "Upgrade_Y"), "the factory has the object upgrade");
        assert!(!g.player_upgrades(0).any(|u| u == "Upgrade_Y"), "and the player does not");
        // The factory has it now, so the button is refused again (`hasUpgrade`, Object.cpp:5566).
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_UpgradeY")]);
        assert_eq!(queue_len(&g, ids[0]), 0);
        assert_eq!(g.player(0).unwrap().money, 500, "nothing is paid again");
    }

    /// `Object::doCommandButton` (Object.cpp:5566) and `queueUpgrade` (ProductionUpdate.cpp:271-274): an object
    /// upgrade is refused for an object it does not affect. The `Foundry` has no module for `Upgrade_Y`.
    #[test]
    fn team_object_upgrade_button_needs_an_affected_object() {
        let (mut g, ids) = upgrade_world(&[("Foundry", (100.0, 100.0))]);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_UpgradeY")]);
        assert_eq!(queue_len(&g, ids[0]), 0);
        assert_eq!(g.player(0).unwrap().money, 1000);
    }

    /// `doTeamPartialUseCommandButton` (ScriptActions.cpp:5949) takes the members `isValidToUseOn` accepts: an
    /// object whose own queue already holds an upgrade is not valid (ControlBar.cpp:644-650), so the other
    /// factory alone takes the object upgrade.
    #[test]
    fn partial_upgrade_button_skips_a_factory_already_producing_an_upgrade() {
        let (mut g, ids) = upgrade_world(&[("Plant", (100.0, 100.0)), ("Plant", (200.0, 100.0))]);
        g.objects.get_mut(&ids[0]).unwrap().production.as_mut().unwrap().queue.push(crate::production::ProductionEntry {
            kind: crate::production::ProductionKind::Upgrade("Upgrade_X".into()),
            quantity_total: 0,
            quantity_produced: 0,
            frames: 0,
            door: Some(0),
            object_upgrade: false,
        });
        run_action(&mut g, "TEAM_PARTIAL_USE_COMMANDBUTTON", vec![real_param(100.0), team_param(), str_param("Cmd_UpgradeY")]);
        assert_eq!(queue_len(&g, ids[0]), 1, "the busy factory keeps only its own entry");
        assert_eq!(queue_len(&g, ids[1]), 1, "the idle one takes the object upgrade");
        assert_eq!(g.player(0).unwrap().money, 500);
    }

    /// `Command::QueueUpgrade` from the HUD (`hud.rs`): the sim takes the object type from the upgrade (`Type = OBJECT`,
    /// via its button, ProductionUpdate.cpp:266), so the entry is an object upgrade and it completes on the factory.
    #[test]
    fn player_queue_command_queues_an_object_upgrade_at_the_factory() {
        let (mut g, ids) = upgrade_world(&[("Plant", (100.0, 100.0))]);
        g.submit(0, crate::command::Command::QueueUpgrade { building: ids[0], upgrade: "Upgrade_Y".into() });
        g.run_frames(2);
        let queue = &g.find_object(ids[0]).unwrap().production.as_ref().unwrap().queue;
        assert_eq!(queue.len(), 1, "queued by the command");
        assert!(queue[0].object_upgrade, "an object upgrade, from the upgrade's type");
        g.run_frames(110);
        assert!(g.has_upgrade(ids[0], "Upgrade_Y"), "the factory has it");
        assert!(!g.player_upgrades(0).any(|u| u == "Upgrade_Y"), "the player does not");
    }

    /// `Object::canProduceUpgrade` (Object.cpp:6218) on the command path (`ProductionUpdate::queueUpgrade`:281): a
    /// factory whose command set holds no button for the upgrade refuses it and takes no cost.
    #[test]
    fn player_queue_command_needs_the_button_in_the_command_set() {
        let (mut g, ids) = upgrade_world(&[("Foundry", (100.0, 100.0))]);
        g.submit(0, crate::command::Command::QueueUpgrade { building: ids[0], upgrade: "Upgrade_X".into() });
        g.run_frames(2);
        assert_eq!(queue_len(&g, ids[0]), 0);
        assert_eq!(g.player(0).unwrap().money, 1000, "nothing is paid");
    }

    /// `ProductionUpdate::queueUpgrade` (:292-296): the queue counts every entry, so a full factory refuses an upgrade
    /// and takes no cost.
    #[test]
    fn a_full_factory_refuses_an_upgrade_and_takes_no_cost() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        g.set_upgrade_info(
            [
                ("Upgrade_X".to_string(), crate::upgrade::UpgradeInfo { cost: 500, build_time_frames: 100 }),
                ("Upgrade_Y".to_string(), crate::upgrade::UpgradeInfo { cost: 500, build_time_frames: 100 }),
            ]
            .into_iter()
            .collect(),
        );
        let mut small = plant();
        small.production.as_mut().unwrap().max_queue_entries = 1;
        g.set_template_source(Arc::new(Builds(vec![Arc::new(small)])));
        let id = spawn(&mut g, "Plant", (100.0, 100.0), 0);
        assert!(g.queue_upgrade(0, id, "Upgrade_X"));
        assert!(!g.queue_upgrade(0, id, "Upgrade_Y"), "the one-entry queue is full");
        assert_eq!(queue_len(&g, id), 1);
        assert_eq!(g.player(0).unwrap().money, 500, "only the first upgrade is paid");
    }

    /// `Object::affectedByUpgrade` (Object.cpp:4505) with `wouldUpgrade` (UpgradeModule.cpp:124-130): a module that ran
    /// on the player's owned upgrade (Player.cpp:3143 runs it on every owned object) has executed, so the object is no
    /// longer affected by that upgrade.
    #[test]
    fn an_owned_upgrade_does_not_leave_its_module_affected() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let mut t = factory("Plant");
        t.upgrade_modules = vec![Arc::new(crate::upgrade::SimUpgradeModule {
            triggered_by: vec!["Upgrade_X".into()],
            conflicts_with: vec![],
            removes_upgrades: vec![],
            requires_all_triggers: false,
            effect: crate::upgrade::UpgradeEffect::PowerPlant,
        })];
        let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        assert!(g.affected_by_upgrade(id, "Upgrade_X"), "not owned yet");
        g.apply_player_upgrade(0, "Upgrade_X");
        assert!(!g.affected_by_upgrade(id, "Upgrade_X"), "owned, so the module has executed");
    }

    /// `RequiresAllTriggers` (UpgradeModule.cpp:124-150, `testUpgradeConditions`): the module needs every trigger.
    /// One held trigger neither activates it nor makes the object unaffected by the other.
    #[test]
    fn requires_all_triggers_needs_every_upgrade() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let mut t = factory("Plant");
        t.upgrade_modules = vec![Arc::new(crate::upgrade::SimUpgradeModule {
            triggered_by: vec!["Upgrade_A".into(), "Upgrade_B".into()],
            conflicts_with: vec![],
            removes_upgrades: vec![],
            requires_all_triggers: true,
            effect: crate::upgrade::UpgradeEffect::PowerPlant,
        })];
        let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        g.apply_player_upgrade(0, "Upgrade_A");
        assert!(!g.find_object(id).unwrap().behavior.power_plant_upgraded, "A alone does not activate it");
        assert!(g.affected_by_upgrade(id, "Upgrade_B"), "with A held, B would complete the set");
        g.apply_player_upgrade(0, "Upgrade_B");
        assert!(g.find_object(id).unwrap().behavior.power_plant_upgraded, "both together activate it");
    }

    /// `ProductionUpdate.cpp:858-861` and `PLAYER_BUILT_UPGRADE` (ScriptConditions.cpp:1535-1555): an object upgrade's
    /// completion is an event the condition consumes once, as a player upgrade's is.
    #[test]
    fn a_completed_upgrade_is_a_one_shot_script_event() {
        let (mut g, _ids) = upgrade_world(&[("Plant", (100.0, 100.0))]);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_UpgradeY")]);
        g.run_frames(110);
        let cond = zr_map::scripts::Condition {
            version: 4,
            type_id: 0,
            internal_name: Some("PLAYER_BUILT_UPGRADE".into()),
            params: vec![str_param("<This Player>"), str_param("Upgrade_Y")],
        };
        let mut engine = g.scripts().clone();
        engine.current_player = Some(0);
        assert!(engine.evaluate_condition(&g, &cond), "the completion is an event");
        assert!(!engine.evaluate_condition(&g, &cond), "and it is consumed when read");
    }

    /// `Object::doCommandButton` (Object.cpp:5504) for an unknown name: `findUpgrade` finds nothing, so nothing is queued
    /// and nothing is paid.
    #[test]
    fn an_unknown_upgrade_is_not_queued() {
        let (mut g, ids) = upgrade_world(&[("Plant", (100.0, 100.0))]);
        assert!(!g.queue_upgrade(0, ids[0], "Upgrade_Nope"));
        assert_eq!(queue_len(&g, ids[0]), 0);
        assert_eq!(g.player(0).unwrap().money, 1000);
    }

    /// `LocomotorSetUpgrade::upgradeImplementation` (LocomotorSetUpgrade.cpp:54-56): the locomotor upgrade is set only on
    /// an object with an AI.
    #[test]
    fn a_locomotor_set_upgrade_needs_an_ai() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let mut t = factory("Plant");
        t.upgrade_modules = vec![Arc::new(crate::upgrade::SimUpgradeModule {
            triggered_by: vec!["Upgrade_L".into()],
            conflicts_with: vec![],
            removes_upgrades: vec![],
            requires_all_triggers: false,
            effect: crate::upgrade::UpgradeEffect::Locomotor { flag_bit: 0 },
        })];
        let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        g.give_upgrade(id, "Upgrade_L");
        assert!(!g.find_object(id).unwrap().loco_upgraded, "no AI, no locomotor upgrade");
    }

    /// `Object::doCommandButton` UNIT_BUILD / DOZER_CONSTRUCT (Object.cpp:5577-5583): the factory queues the unit, and a
    /// disabled factory does nothing (`isDisabled`, Object.cpp:5504).
    #[test]
    fn a_team_unit_build_button_queues_the_unit_at_the_factory() {
        let (mut g, _ids) = upgrade_world(&[("Plant", (100.0, 100.0))]);
        g.scripts_mut().load_command_button_actions([("Cmd_BuildTank".to_owned(), CommandButtonAction::UnitBuild { thing: "Tank".into() })]);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_BuildTank")]);
        // A tank has no build time, so it leaves the queue in the frame it is queued: count the tank instead.
        assert_eq!(g.objects().filter(|o| o.player == 0 && o.template.name == "Tank").count(), 1, "the factory built one tank");
    }

    #[test]
    fn a_disabled_factory_skips_the_team_unit_build_button() {
        let (mut g, ids) = upgrade_world(&[("Plant", (100.0, 100.0))]);
        g.scripts_mut().load_command_button_actions([("Cmd_BuildTank".to_owned(), CommandButtonAction::UnitBuild { thing: "Tank".into() })]);
        g.set_disabled(ids[0], DisabledType::Emp);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_BuildTank")]);
        assert_eq!(queue_len(&g, ids[0]), 0, "a disabled factory takes no order (Object.cpp:5504)");
        assert_eq!(g.player(0).unwrap().money, 1000);
    }

    /// `Object::doCommandButton` SELL (Object.cpp:5596) -> `BuildAssistant::sellObject` (BuildAssistant.cpp:1580): the
    /// sale refunds the factory's queued upgrade before the building goes.
    #[test]
    fn a_team_sell_button_refunds_the_factory_queue() {
        let (mut g, ids) = upgrade_world(&[("Plant", (100.0, 100.0))]);
        g.scripts_mut().load_command_button_actions([("Cmd_Sell".to_owned(), CommandButtonAction::Sell)]);
        assert!(g.queue_upgrade(0, ids[0], "Upgrade_X"));
        assert_eq!(g.player(0).unwrap().money, 500);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_Sell")]);
        assert_eq!(g.player(0).unwrap().money, 1000, "the queued upgrade is refunded (the factory's build cost is 0)");
    }

    /// `BuildAssistant::sellObject` (BuildAssistant.cpp:1580) on its own: the queue is refunded at the start of the sale.
    #[test]
    fn selling_a_factory_refunds_its_queue() {
        let (mut g, ids) = upgrade_world(&[("Plant", (100.0, 100.0))]);
        assert!(g.queue_upgrade(0, ids[0], "Upgrade_X"));
        g.command_sell(0, ids[0]);
        assert_eq!(g.player(0).unwrap().money, 1000);
    }

    /// `ScriptActions::doTeamPartialUseCommandButton` (ScriptActions.cpp:5949): `isValidToUseOn` (ControlBar.cpp:637) is false
    /// for a button with no special power and no upgrade (`canDoSpecialPower` with a null template finds no module,
    /// ActionManager.cpp:1885), so no member is chosen and nothing is built. A guard on the port's no-op for these buttons.
    #[test]
    fn a_partial_unit_build_button_selects_no_member() {
        let mut g = action_world();
        let factory_id = spawn(&mut g, "Plant", (100.0, 100.0), 0);
        team(&mut g, &[factory_id]);
        g.scripts_mut().load_command_button_actions([("Cmd_BuildTank".to_owned(), CommandButtonAction::UnitBuild { thing: "Tank".into() })]);
        run_action(&mut g, "TEAM_PARTIAL_USE_COMMANDBUTTON", vec![real_param(100.0), team_param(), str_param("Cmd_BuildTank")]);
        assert_eq!(queue_len(&g, factory_id), 0, "no member is chosen");
        assert_eq!(g.objects().filter(|o| o.player == 0 && o.template.name == "Tank").count(), 0);
    }

    /// `CONDITION AIRBORNE_TARGET` (ScriptConditions.cpp, `isAirborneTarget`): the status bit `doLocomotor` sets
    /// (AIUpdate.cpp:2294-2300), not the locomotor's height.
    #[test]
    fn the_airborne_target_condition_reads_the_status_bit() {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        g.add_player(0, 1000);
        let id = g.create_object(&Arc::new(ObjectTemplate::new("Plain")), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        let o = g.find_object(id).unwrap();
        assert!(!super::super::conditions::object_has_status(&g, o, "AIRBORNE_TARGET"));
        g.objects.get_mut(&id).unwrap().status_bits |= crate::object::status::AIRBORNE_TARGET;
        let o = g.find_object(id).unwrap();
        assert!(super::super::conditions::object_has_status(&g, o, "AIRBORNE_TARGET"));
    }

    /// A world of players 0-2: 0 and 1 are enemies, 0 and 2 are allies. Templates: `Plant`, `Foundry`, and an armed
    /// `Tank` (a cannon, so an attack order survives its state's entry, as a unit with a weapon does).
    fn action_world() -> GameLogic {
        let mut g = GameLogic::new(TerrainLogic::flat(400, 400, 0), 1);
        for p in 0..3 {
            g.add_player(p, 1000);
        }
        g.set_relationship_symmetric(0, 1, Relationship::Enemies);
        g.set_relationship_symmetric(0, 2, Relationship::Allies);
        let mut cannon = crate::weapon::WeaponTemplate::new("Cannon");
        cannon.primary_damage = 10.0;
        cannon.attack_range = 100.0;
        cannon.min_delay_between_shots = 5;
        cannon.max_delay_between_shots = 5;
        let mut armed = tank();
        let mut set = crate::weapon::WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(cannon));
        armed.combat.weapon_set = Some(set);
        g.set_template_source(Arc::new(Builds(vec![Arc::new(plant()), Arc::new(factory("Foundry")), Arc::new(armed)])));
        g
    }

    /// `ScriptActions::doTeamUseCommandButtonAbilityOnNamed` (ScriptActions.cpp:4495) -> `Object::doCommandButtonAtObject`
    /// (Object.cpp:5677): an object-target fire attacks the named object only when `isValidObjectTarget`
    /// (ControlBar.cpp:609) accepts its relationship, and the attack holds the temporary lock on the slot.
    #[test]
    fn a_team_fire_button_attacks_a_named_enemy_in_its_target_mask() {
        let mut g = action_world();
        let shooter = spawn(&mut g, "Tank", (100.0, 100.0), 0);
        let enemy = spawn(&mut g, "Tank", (140.0, 100.0), 1);
        let ally = spawn(&mut g, "Tank", (160.0, 100.0), 2);
        g.scripts_mut().add_named_object("Enemy", enemy);
        g.scripts_mut().add_named_object("Ally", ally);
        team(&mut g, &[shooter]);
        let fire = CommandButtonAction::FireWeapon {
            slot: 0,
            max_shots: 3,
            targets: FireTargets { enemy: true, ..Default::default() },
        };
        // The member branch is called directly: an attack order set by a script action is dropped within the frames the
        // action runs (this tank's cannon does not hold it), so the frames would hide what the branch did.
        ScriptEngine::do_command_button_action_at_object(&mut g, shooter, &fire, ally);
        assert!(g.find_object(shooter).unwrap().combat.attack.is_none(), "an ally is not an enemy target");
        ScriptEngine::do_command_button_action_at_object(&mut g, shooter, &fire, enemy);
        let attack = g.find_object(shooter).unwrap().combat.attack.as_ref().expect("the enemy is attacked");
        assert!(matches!(attack.target, AttackTarget::Object(v) if v == enemy), "the attack is on the enemy object");
        assert_eq!(g.find_object(shooter).unwrap().combat.weapons.temporary_lock, Some(0), "the temporary lock is on the button's slot");
    }

    /// `ScriptActions::doTeamUseCommandButtonAbilityAtWaypoint` (ScriptActions.cpp:4533) -> `Object::doCommandButtonAtPosition`
    /// (Object.cpp:5798-5811): a position fire attacks the waypoint, and an object-only fire button does nothing there.
    #[test]
    fn a_team_fire_button_at_a_waypoint_attacks_the_position_only_if_it_takes_one() {
        let mut g = action_world();
        let shooter = spawn(&mut g, "Tank", (100.0, 100.0), 0);
        g.scripts_mut().set_waypoints([("Spot".to_owned(), Coord3D::new(200.0, 200.0, 0.0))]);
        team(&mut g, &[shooter]);
        let enemy_only = CommandButtonAction::FireWeapon { slot: 0, max_shots: 3, targets: FireTargets { enemy: true, ..Default::default() } };
        let at_position = CommandButtonAction::FireWeapon { slot: 0, max_shots: 3, targets: FireTargets { position: true, ..Default::default() } };
        let spot = Coord3D::new(200.0, 200.0, 0.0);
        ScriptEngine::do_command_button_action_at_position(&mut g, shooter, &enemy_only, spot);
        assert!(g.find_object(shooter).unwrap().combat.attack.is_none(), "an object-target button does nothing at a position");
        ScriptEngine::do_command_button_action_at_position(&mut g, shooter, &at_position, spot);
        let attack = g.find_object(shooter).unwrap().combat.attack.as_ref().expect("the position is attacked");
        assert!(
            matches!(attack.target, AttackTarget::Ground(p) if p.x == 200.0 && p.y == 200.0),
            "the attack is on the waypoint"
        );
    }

    /// `Object::doCommandButton` HACK_INTERNET (Object.cpp:5587-5593) -> `aiHackInternet`: the script path does not run the
    /// weapon-lock release of `MSG_INTERNET_HACK` (`onInternetHack`, GameLogicDispatch.cpp:1559). A hacker that is already
    /// hacking ignores the order (`hack_order`), so its temporary lock survives the button.
    #[test]
    fn a_team_hack_button_keeps_the_temporary_weapon_lock() {
        use crate::combat::{AttackOrder, AttackTarget};
        use crate::hack_internet::{HackPhase, HackState};
        let mut g = action_world();
        let mut t = hacker();
        t.hack_internet = Some(Default::default());
        // The temporary lock lives on the weapon set, so the hacker needs a weapon in slot 0 to hold it.
        let mut set = crate::weapon::WeaponSetTemplate::default();
        set.weapons[0] = Some(Arc::new(crate::weapon::WeaponTemplate::new("Gun")));
        t.combat.weapon_set = Some(set);
        let id = g.create_object(&Arc::new(t), Coord3D::new(100.0, 100.0, 0.0), 0.0, 0);
        {
            let o = g.objects.get_mut(&id).unwrap();
            o.hack_internet = Some(HackState { phase: HackPhase::Hacking, frames_remaining: 1000, pending: None });
            o.combat.attack = Some(AttackOrder::with_limit(AttackTarget::Ground(Coord3D::new(150.0, 100.0, 0.0)), 3, 0));
            assert!(o.combat.weapons.set_weapon_lock(0, crate::weapon::WeaponLockType::Temporarily));
        }
        team(&mut g, &[id]);
        g.scripts_mut().load_command_button_actions([("Cmd_Hack".to_owned(), CommandButtonAction::HackInternet)]);
        run_action(&mut g, "TEAM_USE_COMMANDBUTTON_ABILITY", vec![team_param(), str_param("Cmd_Hack")]);
        assert!(g.find_object(id).unwrap().combat.attack.is_some(), "the attack is still there");
        assert_eq!(g.find_object(id).unwrap().combat.weapons.temporary_lock, Some(0), "the script hack does not release the temporary lock");
    }

    /// `TEAM_USE_COMMANDBUTTON_ABILITY` -> `AIGroup::groupDoCommandButton` -> `Object::doCommandButton` -> a special
    /// power (`ScriptActions.cpp:4463`). `SpecialPowerStore::onSpecialPowerTriggered` /
    /// `onSpecialPowerCompleted` push into the engine's `m_triggeredSpecialPowers` / `m_finishedSpecialPowers`
    /// (`ScriptEngine.cpp:7220-7240`), so a `PLAYER_TRIGGERED/COMPLETED_SPECIAL_POWER` condition later in the same
    /// pass must see the fire of its own pass. The write through `GameLogic::scripts` landed in the empty engine the
    /// take site holds and was dropped on restore.
    #[test]
    fn a_team_command_button_power_is_seen_by_a_later_condition_in_the_same_pass() {
        let mut g = world_with("SPECIAL_CASH_HACK", false, false);
        let hacker = spawn(&mut g, "Hacker", (100.0, 100.0), 0);
        team(&mut g, &[hacker]);
        let sp = |s: &str| Parameter { kind: 3, value: ParamValue::Plain { int: 0, real: 0.0, string: s.to_owned() } };
        let cond = |name: &str| Condition {
            version: 4,
            type_id: 0,
            internal_name: Some(name.to_owned()),
            params: vec![str_param("<This Player>"), str_param("P")],
        };
        let give = |v: i32| Action {
            version: 2,
            type_id: 0,
            internal_name: Some("PLAYER_GIVE_MONEY".to_owned()),
            params: vec![str_param("<This Player>"), Parameter { kind: 0, value: ParamValue::Plain { int: v, real: 0.0, string: String::new() } }],
        };
        let script = |name: &str, conditions: Vec<Condition>, actions: Vec<Action>| Script {
            version: 2,
            name: name.to_owned(),
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
        };
        let true_cond = Condition { version: 4, type_id: 0, internal_name: Some("CONDITION_TRUE".to_owned()), params: Vec::new() };
        g.scripts_mut().load(vec![(
            0,
            ScriptList {
                scripts: vec![
                    script(
                        "fire",
                        vec![true_cond],
                        vec![Action {
                            version: 2,
                            type_id: 0,
                            internal_name: Some("TEAM_USE_COMMANDBUTTON_ABILITY".to_owned()),
                            params: vec![sp("MyTeam"), str_param("Cmd_Hack")],
                        }],
                    ),
                    script("triggered", vec![cond("PLAYER_TRIGGERED_SPECIAL_POWER")], vec![give(555)]),
                    script("completed", vec![cond("PLAYER_COMPLETED_SPECIAL_POWER")], vec![give(555)]),
                ],
                groups: Vec::new(),
            },
        )]);
        g.run_frames(2);
        // The cash hack pays 100 and both conditions fire once each.
        assert_eq!(g.player_money(0), 1000 + 100 + 555 + 555);
    }
}
