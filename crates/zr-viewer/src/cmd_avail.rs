//! `ControlBar::getCommandAvailability` and the control-bar build tooltip.
//!
//! The engine's `ControlBar::updateContextCommand` (`ControlBar/ControlBarCommand.cpp:693`) asks
//! `getCommandAvailability` (`:1008`) for every populated command window and then hides it
//! (`COMMAND_HIDDEN`), disables it grayscale (`COMMAND_RESTRICTED`), disables it with a clock
//! (`COMMAND_NOT_READY`), disables it dimmed but still coloured (`COMMAND_CANT_AFFORD`) or leaves it
//! enabled (`COMMAND_AVAILABLE`), checking it for check-like commands (`COMMAND_ACTIVE`).
//!
//! This module ports that evaluation against the sim's public state and assembles the
//! `ControlBarPopupDescription.wnd` tooltip (`GUICallbacks/ControlBarPopupDescription.cpp:244`):
//! the button's `TextLabel` name, `TOOLTIP:Cost` cost, `DescriptLabel` and the unsatisfied
//! prerequisites as `CONTROLBAR:Requirements`.
//!
//! The viewer only reads sim state; nothing here mutates the simulation.

use zr_sim::object::Object;
use zr_sim::production::{Prerequisite, ProductionKind};
use zr_sim::template::kind_of;
use zr_sim::weapon::WeaponStatus;
use zr_sim::DisabledType;
use zr_ui::command::{command_option as opt, CommandButton, CommandKind};

use crate::sim_view::SimRes;

/// `CommandAvailability` (`Include/GameClient/ControlBar.h:628`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CommandAvailability {
    /// Disabled, drawn grayscale (`W3DGadgetPushButtonImageDrawOne`, `DRAW_IMAGE_GRAYSCALE`).
    #[default]
    Restricted,
    Available,
    /// Check-like command is on (`GadgetCheckLikeButtonSetVisualCheck`).
    Active,
    /// Not drawn at all (`win->winHide(TRUE)`).
    Hidden,
    /// Disabled but kept coloured; the special-power clock overlay is drawn separately.
    NotReady,
    /// Disabled and dimmed with `0xff909090` (the `WIN_STATUS_ALWAYS_COLOR` path).
    CantAfford,
}

impl CommandAvailability {
    /// Whether a click should reach the command (`GadgetButton` only sends clicks while enabled).
    pub fn enabled(self) -> bool {
        matches!(self, CommandAvailability::Available | CommandAvailability::Active)
    }
}

/// `ControlBar::getCommandAvailability` (`ControlBarCommand.cpp:1008`) for one selected object.
///
/// `force_disabled_evaluation` is the recursive call the engine makes for a disabled object; it is
/// kept so the disabled branch returns `COMMAND_HIDDEN` when the command would be hidden anyway.
pub fn command_availability(sim: &SimRes, btn: &CommandButton, obj: Option<&Object>) -> CommandAvailability {
    command_availability_inner(sim, btn, obj, false)
}

fn command_availability_inner(sim: &SimRes, btn: &CommandButton, obj: Option<&Object>, force_disabled: bool) -> CommandAvailability {
    let logic = sim.game.logic();
    let Some(obj) = obj else { return CommandAvailability::Hidden };
    let player = obj.player;

    // `obj->testScriptStatusBit(OBJECT_STATUS_SCRIPT_DISABLED/UNPOWERED)`.
    if obj.is_disabled_by_type(DisabledType::ScriptDisabled) || obj.is_disabled_by_type(DisabledType::ScriptUnderpowered) {
        return CommandAvailability::Hidden;
    }
    // Unmanned vehicles hide every command (`Object::isDisabledByType(DISABLED_UNMANNED)`).
    if obj.is_disabled_by_type(DisabledType::Unmanned) {
        return CommandAvailability::Hidden;
    }
    // `MUST_BE_STOPPED`: a moving unit cannot use this button.
    if btn.options & opt::MUST_BE_STOPPED != 0 && obj.ai.is_moving() {
        return CommandAvailability::Restricted;
    }
    // Other disabled objects grey out, except the command types the engine lets through.
    if disabled_blocks(obj, btn) && !force_disabled {
        if command_availability_inner(sim, btn, Some(obj), true) == CommandAvailability::Hidden {
            return CommandAvailability::Hidden;
        }
        return CommandAvailability::Restricted;
    }
    // `NEED_UPGRADE`: a player upgrade must be complete, or an object upgrade must be owned.
    if btn.options & opt::NEED_UPGRADE != 0 {
        if let Some(up) = btn.upgrade.as_deref() {
            if !upgrade_present(sim, obj, player, up) {
                return CommandAvailability::Restricted;
            }
        }
    }

    let queue_maxed = obj.production.as_ref().is_some_and(queue_maxed);
    // `NOT_QUEUEABLE`: cannot queue while anything is being produced.
    if obj.production.as_ref().is_some_and(|p| !p.queue.is_empty()) && btn.options & opt::NOT_QUEUEABLE != 0 {
        return CommandAvailability::Restricted;
    }

    match &btn.command {
        CommandKind::DozerConstruct => {
            let thing = btn.object.as_deref();
            if let Some(name) = thing {
                if buildable_hidden(sim, player, name) {
                    return CommandAvailability::Hidden;
                }
            }
            // Only a dozer/worker can construct.
            if !obj.template.is_kind_of(kind_of::DOZER) {
                return CommandAvailability::Restricted;
            }
            // Already building something (`dozerAI->isTaskPending(DOZER_TASK_BUILD)`).
            if obj.build_target.is_some() {
                return CommandAvailability::Restricted;
            }
            let template = thing.and_then(|n| logic.find_template(n));
            if let Some(t) = &template {
                if !prerequisites_satisfied(sim, player, t) {
                    return CommandAvailability::Restricted;
                }
                if !can_afford(sim, player, t.build_cost) {
                    return CommandAvailability::Restricted;
                }
            }
            CommandAvailability::Available
        }
        CommandKind::Sell => {
            if obj.is_disabled_by_type(DisabledType::Subdued) {
                return CommandAvailability::Restricted;
            }
            CommandAvailability::Available
        }
        CommandKind::UnitBuild => {
            let template = btn.object.as_deref().and_then(|n| logic.find_template(n));
            if let Some(name) = btn.object.as_deref() {
                if buildable_hidden(sim, player, name) {
                    return CommandAvailability::Hidden;
                }
            }
            if queue_maxed {
                return CommandAvailability::Restricted;
            }
            // `Player::canBuild` only checks prerequisites.
            if let Some(t) = &template {
                if !prerequisites_satisfied(sim, player, t) {
                    return CommandAvailability::Restricted;
                }
                if !can_afford(sim, player, t.build_cost) {
                    return CommandAvailability::Restricted;
                }
            }
            CommandAvailability::Available
        }
        CommandKind::PlayerUpgrade | CommandKind::ObjectUpgrade => {
            if queue_maxed {
                return CommandAvailability::Restricted;
            }
            if let Some(up) = btn.upgrade.as_deref() {
                let player_upgrade = btn.command == CommandKind::PlayerUpgrade;
                let already = if player_upgrade {
                    player_upgrades(sim, player, up) || upgrade_in_production(obj, up)
                } else {
                    object_has_upgrade(obj, up) || upgrade_in_production(obj, up)
                };
                // The engine reports `COMMAND_CANT_AFFORD` for an already-owned/in-queue upgrade.
                if already {
                    return CommandAvailability::CantAfford;
                }
                if !can_afford(sim, player, upgrade_cost(sim, up)) {
                    return CommandAvailability::Restricted;
                }
            }
            for science in &btn.science {
                if !sim.game.logic().player_has_science(player, science) {
                    return CommandAvailability::Restricted;
                }
            }
            CommandAvailability::Available
        }
        CommandKind::FireWeapon => {
            let slot = crate::hud::fire_slot(btn);
            let Some(w) = obj.combat.weapons.weapon(slot) else {
                // A mine-clearing button without the mine weapon set is available; the weapon set
                // switches when the order is given (`ControlBarCommand.cpp:1338`).
                if btn.options & opt::USES_MINE_CLEARING_WEAPONSET != 0 && obj.weapon_set_flags & zr_sim::weapon::WEAPONSET_MINE_CLEARING_DETAIL == 0 {
                    return CommandAvailability::Available;
                }
                return CommandAvailability::Restricted;
            };
            // A zero clip reload time is always shown available.
            if w.template.clip_reload_time == 0 {
                return CommandAvailability::Available;
            }
            if w.status(sim.game.frame()) != WeaponStatus::ReadyToFire {
                return CommandAvailability::NotReady;
            }
            CommandAvailability::Available
        }
        CommandKind::Guard | CommandKind::GuardWithoutPursuit | CommandKind::GuardFlyingUnitsOnly => CommandAvailability::Available,
        CommandKind::CombatDrop => {
            if obj.contained.iter().filter(|&&id| sim.game.logic().find_object(id).is_some_and(|o| o.template.is_kind_of(kind_of::CAN_RAPPEL))).count() == 0 {
                CommandAvailability::Restricted
            } else {
                CommandAvailability::Available
            }
        }
        CommandKind::ExitContainer => {
            if obj.is_disabled_by_type(DisabledType::Subdued) {
                CommandAvailability::Restricted
            } else {
                CommandAvailability::Available
            }
        }
        CommandKind::Evacuate => {
            if obj.contained.is_empty() || obj.is_disabled_by_type(DisabledType::Subdued) {
                CommandAvailability::Restricted
            } else {
                CommandAvailability::Available
            }
        }
        CommandKind::SpecialPower | CommandKind::SpecialPowerFromShortcut | CommandKind::SpecialPowerConstruct | CommandKind::SpecialPowerConstructFromShortcut => {
            let Some(power) = btn.special_power.as_deref() else { return CommandAvailability::Restricted };
            // `SpecialPowerModuleInterface::isReady`: the cooldown wipe is drawn separately. A paused
            // module (`StartsPaused`, before its upgrade) is not ready.
            if !sim.game.logic().special_power_is_ready(obj.id, power) {
                return CommandAvailability::NotReady;
            }
            // `SpecialAbilityUpdate::isPowerCurrentlyInUse`.
            if obj.ability.as_ref().is_some_and(|a| a.power == power) {
                return CommandAvailability::Restricted;
            }
            CommandAvailability::Available
        }
        CommandKind::ToggleOvercharge => {
            if obj.behavior.overcharge {
                CommandAvailability::Active
            } else {
                CommandAvailability::Available
            }
        }
        CommandKind::SwitchWeapon => CommandAvailability::Available,
        // `ControlBarCommand.cpp:1498`: restricted while the hacker is hacking, packing or unpacking
        // (`HackInternetAIInterface::isHackingPackingOrUnpacking`); the sim's `is_busy` is that state.
        CommandKind::HackInternet => {
            if zr_sim::hack_internet::is_busy(obj) {
                CommandAvailability::Restricted
            } else {
                CommandAvailability::Available
            }
        }
        CommandKind::Stop => CommandAvailability::Available,
        CommandKind::SelectAllUnitsOfType => CommandAvailability::Available,
        // Every other command type falls through to available in the engine.
        _ => CommandAvailability::Available,
    }
}

/// The disabled-object branch: the command types the engine always lets through.
fn disabled_blocks(obj: &Object, btn: &CommandButton) -> bool {
    let mut disabled = obj.is_disabled();
    if disabled
        && btn.options & opt::IGNORES_UNDERPOWERED != 0
        && obj.is_disabled_by_type(DisabledType::Underpowered)
        && obj.disabled_flags().count_ones() == 1
    {
        disabled = false;
    }
    if !disabled {
        return false;
    }
    !matches!(
        btn.command,
        CommandKind::Sell
            | CommandKind::Evacuate
            | CommandKind::ExitContainer
            | CommandKind::BeaconDelete
            | CommandKind::SetRallyPoint
            | CommandKind::Stop
            | CommandKind::SwitchWeapon
    )
}

/// `ProductionPrerequisite::isSatisfied` for the sim's `Object` OR groups (`ProductionPrerequisite.cpp`).
pub fn prerequisites_satisfied(sim: &SimRes, player: u8, unit: &zr_sim::template::ObjectTemplate) -> bool {
    for p in &unit.prerequisites {
        if let Prerequisite::AnyOfObjects(names) = p {
            let owned = names.iter().any(|n| sim.game.logic().objects().any(|o| !o.is_destroyed() && o.player == player && o.template.name == *n));
            if !owned {
                return false;
            }
        }
    }
    true
}

/// `Player::canAffordBuild` reduced to the sim's raw `build_cost` (`ProductionUpdate::queueCreateUnit`).
pub fn can_afford(sim: &SimRes, player: u8, cost: i32) -> bool {
    sim.game.logic().player_money(player) >= cost.max(0) as u32
}

/// Whether `ControlBar::processCommandUI`'s money check would refuse this command for its owner
/// (`CANMAKE_NO_MONEY` / `canAffordBuild`). Used to play the insufficient-funds feedback when a
/// greyed build button is clicked (`ControlBarCommandProcessing.cpp:242/400`).
pub fn refuses_for_money(sim: &SimRes, btn: &CommandButton, obj: Option<&Object>) -> bool {
    let Some(obj) = obj else { return false };
    let player = obj.player;
    match &btn.command {
        CommandKind::UnitBuild | CommandKind::DozerConstruct => btn
            .object
            .as_deref()
            .and_then(|n| sim.game.logic().find_template(n))
            .is_some_and(|t| !can_afford(sim, player, t.build_cost)),
        CommandKind::PlayerUpgrade | CommandKind::ObjectUpgrade => btn
            .upgrade
            .as_deref()
            .is_some_and(|u| !can_afford(sim, player, upgrade_cost(sim, u))),
        _ => false,
    }
}

/// `ThingTemplate::getBuildable()`: hidden when `No`, and for a human when `Only_By_AI`.
fn buildable_hidden(sim: &SimRes, player: u8, name: &str) -> bool {
    match sim.game.data().templates().thing(name).map(|t| t.buildable) {
        Some(zr_templates::BuildableStatus::No) => true,
        Some(zr_templates::BuildableStatus::OnlyByAi) => sim.game.player(player).is_some_and(|p| !p.is_ai),
        _ => false,
    }
}

/// `player->hasUpgradeComplete(upgradeT)`.
fn player_upgrades(sim: &SimRes, player: u8, name: &str) -> bool {
    sim.game.logic().player_upgrades(player).any(|u| u.eq_ignore_ascii_case(name))
}

/// `obj->hasUpgrade(upgradeT)`.
fn object_has_upgrade(obj: &Object, name: &str) -> bool {
    obj.upgrades.iter().any(|u| u.eq_ignore_ascii_case(name))
}

/// `ProductionUpdate::isUpgradeInQueue(upgradeT)`.
fn upgrade_in_production(obj: &Object, name: &str) -> bool {
    obj.production
        .as_ref()
        .is_some_and(|p| p.queue.iter().any(|e| matches!(&e.kind, ProductionKind::Upgrade(u) if u.eq_ignore_ascii_case(name))))
}

/// The `NEED_UPGRADE` gate: player or object upgrade by the template's `UpgradeType`.
fn upgrade_present(sim: &SimRes, obj: &Object, player: u8, name: &str) -> bool {
    match sim.game.data().templates().upgrade(name).map(|u| u.upgrade_type) {
        Some(zr_templates::UpgradeType::Object) => object_has_upgrade(obj, name),
        _ => player_upgrades(sim, player, name),
    }
}

/// `pu->getProductionCount() == MAX_BUILD_QUEUE_BUTTONS` (the sim uses the template's `MaxQueueEntries`).
fn queue_maxed(p: &zr_sim::production::ProductionState) -> bool {
    // `ProductionTemplate::max_queue_entries` defaults to `MAX_BUILD_QUEUE_BUTTONS` (9); the state
    // does not carry it, so use the engine constant (`ControlBar.h:418`).
    p.queue.len() >= 9
}

fn upgrade_cost(sim: &SimRes, name: &str) -> i32 {
    sim.game.data().templates().upgrade(name).map_or(0, |u| u.build_cost)
}

/// `WeaponSlotType` name -> slot index (`WeaponSlotTypeNames`).
pub(crate) fn weapon_slot_index(slot: Option<&str>) -> Option<usize> {
    Some(match slot?.trim().to_ascii_uppercase().as_str() {
        "PRIMARY" => 0,
        "SECONDARY" => 1,
        "TERTIARY" => 2,
        _ => return None,
    })
}

/// The `ControlBarPopupDescription.wnd` tooltip content for one command button
/// (`ControlBar::populateBuildTooltipLayout`, `ControlBarPopupDescription.cpp:244`).
#[derive(Clone, Default)]
pub struct Tooltip {
    pub name: Option<String>,
    pub cost: Option<String>,
    pub description: Option<String>,
    pub requires: Option<String>,
    /// A `TOOLTIP:TooltipNotEnoughMoneyToBuild`-style reason line.
    pub reason: Option<String>,
}

/// Assembles the tooltip for `btn` on `obj`. `name`/`description` stay as localisation labels; the
/// caller resolves them through `GameText` (the sim text table).
pub fn build_tooltip(sim: &SimRes, btn: &CommandButton, obj: Option<&Object>) -> Tooltip {
    let mut tip = Tooltip { name: btn.text_label.clone(), description: btn.description_label.clone(), ..Default::default() };
    let Some(obj) = obj else { return tip };
    let player = obj.player;
    let text = sim.game.text();

    let template = btn.object.as_deref().and_then(|n| sim.game.logic().find_template(n));
    let is_upgrade = matches!(btn.command, CommandKind::PlayerUpgrade | CommandKind::ObjectUpgrade);

    if let Some(t) = &template {
        if btn.command == CommandKind::UnitBuild || btn.command == CommandKind::DozerConstruct {
            if t.build_cost > 0 {
                if let Some(fmt) = text.get("TOOLTIP:Cost") {
                    tip.cost = Some(format_one(fmt, &t.build_cost.to_string()));
                }
            }
            // `ProductionPrerequisite::getRequiresList`: unsatisfied OR groups by display name.
            let mut missing: Vec<String> = Vec::new();
            for p in &t.prerequisites {
                if let Prerequisite::AnyOfObjects(names) = p {
                    if !names.iter().any(|n| sim.game.logic().objects().any(|o| !o.is_destroyed() && o.player == player && o.template.name == *n)) {
                        missing.push(or_requirement_list(sim, names));
                    }
                }
            }
            if !missing.is_empty() {
                let list = missing.join(", ");
                if let Some(fmt) = text.get("CONTROLBAR:Requirements") {
                    tip.requires = Some(format_one(fmt, &list));
                } else {
                    tip.requires = Some(format!("Requires: {list}"));
                }
            }
            if !can_afford(sim, player, t.build_cost) {
                tip.reason = text.get("TOOLTIP:TooltipNotEnoughMoneyToBuild").map(str::to_owned);
            } else if obj.production.as_ref().is_some_and(queue_maxed) {
                tip.reason = text.get("TOOLTIP:TooltipCannotPurchaseBecauseQueueFull").map(str::to_owned);
            }
        }
    }

    if is_upgrade {
        if let Some(up) = btn.upgrade.as_deref() {
            let cost = upgrade_cost(sim, up);
            if cost > 0 {
                if let Some(fmt) = text.get("TOOLTIP:Cost") {
                    tip.cost = Some(format_one(fmt, &cost.to_string()));
                }
            }
            if !can_afford(sim, player, cost) {
                tip.reason = text.get("TOOLTIP:TooltipNotEnoughMoneyToBuild").map(str::to_owned);
            } else if obj.production.as_ref().is_some_and(queue_maxed) {
                tip.reason = text.get("TOOLTIP:TooltipCannotPurchaseBecauseQueueFull").map(str::to_owned);
            }
            // A missing prerequisite science is shown as the generals-promotion requirement.
            if btn.science.iter().any(|s| !sim.game.logic().player_has_science(player, s)) {
                if let Some(fmt) = text.get("CONTROLBAR:Requirements") {
                    let promotion = text.get("CONTROLBAR:GeneralsPromotion").unwrap_or("General's Promotion");
                    tip.requires = Some(format_one(fmt, promotion));
                }
            }
        }
    }

    tip
}

/// The display-name list of an OR prerequisite group, joined with `CONTROLBAR:OrRequirement`.
fn or_requirement_list(sim: &SimRes, names: &[String]) -> String {
    let or = sim.game.text().get("CONTROLBAR:OrRequirement").unwrap_or("or");
    names
        .iter()
        .map(|n| sim.game.display_name(n).unwrap_or(n).to_owned())
        .collect::<Vec<_>>()
        .join(&format!(" {or} "))
}

/// The engine's `UnicodeString::format` for a single placeholder (the first conversion of
/// `%[flags][width][.precision]conv`), e.g. `%d`, `%s`, `%.0f`. `%%` stays a literal `%`.
pub fn format_one(text: &str, value: &str) -> String {
    let mut out = String::new();
    let mut replaced = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            // `%%` is a literal percent, before any conversion is considered.
            if chars.peek() == Some(&'%') {
                chars.next();
                out.push('%');
                continue;
            }
            if !replaced {
                // Consume an optional `[-+0-9.]*` run, then the conversion character.
                let mut spec = String::from("%");
                while let Some(&n) = chars.peek() {
                    if !(n.is_ascii_digit() || matches!(n, '.' | '-' | '+')) {
                        break;
                    }
                    spec.push(n);
                    chars.next();
                }
                if let Some(&conv) = chars.peek() {
                    if matches!(conv, 's' | 'd' | 'u' | 'i' | 'f' | 'x') {
                        chars.next();
                        out.push_str(value);
                        replaced = true;
                        continue;
                    }
                }
                // Not a conversion sequence: keep what we consumed literally.
                out.push_str(&spec);
                continue;
            }
        }
        out.push(c);
    }
    if replaced {
        out
    } else {
        format!("{text} {value}")
    }
}

/// The hovered/forced slot's tooltip text (`name`, cost, description, `Requires: ...`, reason).
pub fn tooltip_text(sim: &SimRes, info: &crate::hud::SlotInfo) -> String {
    let resolve = |label: Option<&String>| label.map(|l| sim.game.text().get(l).map(str::to_owned).unwrap_or_else(|| l.clone()));
    let mut lines: Vec<String> = Vec::new();
    for line in [
        resolve(info.label.as_ref()),
        info.cost.clone(),
        resolve(info.tooltip.as_ref()),
        info.requires.clone(),
        info.reason.clone(),
    ]
    .into_iter()
    .flatten()
    {
        if !line.is_empty() {
            lines.push(line.replace('&', ""));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_one_substitutes_a_single_placeholder() {
        assert_eq!(format_one("Cost: %d", "1500"), "Cost: 1500");
        assert_eq!(format_one("Requires: %s", "Barracks"), "Requires: Barracks");
        assert_eq!(format_one("%s", "X"), "X");
    }

    #[test]
    fn format_one_appends_without_a_placeholder() {
        assert_eq!(format_one("Cost", "1500"), "Cost 1500");
    }

    #[test]
    fn format_one_handles_precision_and_literal_percent() {
        assert_eq!(format_one("Building: %.0f%%", "42"), "Building: 42%");
        assert_eq!(format_one("Building: %d%%", "7"), "Building: 7%");
    }

    #[test]
    fn weapon_slot_names_map_to_indices() {
        assert_eq!(weapon_slot_index(Some("PRIMARY")), Some(0));
        assert_eq!(weapon_slot_index(Some("secondary")), Some(1));
        assert_eq!(weapon_slot_index(Some("TERTIARY")), Some(2));
        assert_eq!(weapon_slot_index(Some("BOGUS")), None);
        assert_eq!(weapon_slot_index(None), None);
    }

    #[test]
    fn availability_enabled_only_for_available_and_active() {
        assert!(CommandAvailability::Available.enabled());
        assert!(CommandAvailability::Active.enabled());
        for a in [CommandAvailability::Restricted, CommandAvailability::Hidden, CommandAvailability::NotReady, CommandAvailability::CantAfford] {
            assert!(!a.enabled());
        }
    }
}
