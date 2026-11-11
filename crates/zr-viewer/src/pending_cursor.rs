//! Pending command cursors and radius rings: `InGameUI::createCommandHint`'s `MOUSEMODE_GUI_COMMAND`
//! branch and `InGameUI::setRadiusCursor` (`GeneralsMD/Code/GameEngine/Source/GameClient/InGameUI.cpp:2854`
//! and `:1423`).
//!
//! A command button that needs a click target puts retail into `MOUSEMODE_GUI_COMMAND` with the button
//! as `m_pendingGUICommand` (`Core/.../ControlBar/ControlBarCommandProcessing.cpp:224`). While it is
//! pending, the cursor under the pointer comes from the button and the radius ring from its
//! `RadiusCursorType`:
//! - a context command or a special power (`isContextCommand()` or `GUI_COMMAND_SPECIAL_POWER*`,
//!   `InGameUI.cpp:3071`) shows `CursorName` when the hovered target is valid and `InvalidCursorName`
//!   otherwise (`MSG_VALID_GUICOMMAND_HINT` / `MSG_INVALID_GUICOMMAND_HINT`, `:3081-3088`, posted by
//!   `CommandXlat::handleGuiCommand`, `Core/.../MessageStream/CommandXlat.cpp:1749-1756`);
//! - any other target-needing button shows `CursorName` (`:3104-3111`).
//!
//! An empty or unknown name is `Mouse::CROSS`, the system crosshair (`:3091`, `:3098`, `:3107`;
//! `Mouse::getCursorIndex` returns `INVALID_MOUSE_CURSOR`, `Mouse.cpp:1265`).
//!
//! Validity is the test the click uses too (`handleGuiCommand`'s `currentlyValid`,
//! `CommandXlat.cpp:1658`), so the cursor and the click agree. The sim does not expose every
//! `ActionManager` check; each approximation is noted on its function.

use zr_sim::{Coord3D, GameLogic, ObjectId};
use zr_ui::command::{command_option as opt, CommandButton, CommandKind};

use crate::cursor::{mouse_cursor_name, CursorRequest, MouseCursorName};
use crate::placement::{object_target_allowed, EnterKind, Order};
use crate::sim_view::SimRes;

/// The `CommandButton` fields a pending command reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingSpec {
    /// `CursorName` (`CommandButton::getCursorName`), empty when the button has none.
    pub cursor_name: String,
    /// `InvalidCursorName`.
    pub invalid_cursor_name: String,
    /// `RadiusCursorType`, empty when the button has no radius cursor.
    pub radius_cursor: String,
    /// True for a context command or a special power: the cursor follows the validity hint
    /// (`InGameUI.cpp:3071`). False: the cursor is always `CursorName`.
    pub hint_split: bool,
}

impl PendingSpec {
    /// The spec for a command button. `hint_split` mirrors `isContextCommand() || GUI_COMMAND_SPECIAL_POWER*`.
    pub fn from_button(btn: &CommandButton) -> Self {
        PendingSpec {
            cursor_name: btn.cursor_name.clone().unwrap_or_default(),
            invalid_cursor_name: btn.invalid_cursor_name.clone().unwrap_or_default(),
            radius_cursor: btn.radius_cursor.clone().unwrap_or_default(),
            hint_split: btn.options & opt::CONTEXTMODE_COMMAND != 0 || matches!(btn.command, CommandKind::SpecialPower | CommandKind::SpecialPowerFromShortcut),
        }
    }
}

/// `InGameUI::createCommandHint`'s cursor for a pending command, given the hint (`valid`). A plain
/// command ignores `valid`. An unresolved name is `Mouse::CROSS`.
pub fn cursor_request(spec: &PendingSpec, valid: bool) -> CursorRequest {
    let name = if spec.hint_split && !valid { &spec.invalid_cursor_name } else { &spec.cursor_name };
    match mouse_cursor_name(name) {
        Some(n) => CursorRequest::Named(n),
        None => CursorRequest::Cross,
    }
}

/// The cursor a left click on a pending `FIRE_WEAPON` clears the mode for. A plain command always
/// completes on a click (`doFireWeaponCommand` returns `COMMAND_COMPLETE`, `GUICommandTranslator.cpp:112`),
/// so the mode clears. A context command clears only when its hint was valid (`CommandXlat.cpp:1708`,
/// `setGUICommand(nullptr)` after a valid DO), and an invalid click keeps the mode for the next one.
pub fn fire_weapon_click_clears(hint_split: bool, valid: bool) -> bool {
    !hint_split || valid
}

/// The weapon radii a `RadiusCursorType` can use (`Weapon::getContinueAttackRange`,
/// `Weapon::getScatterRadius() + getScatterTargetScalar()`, `InGameUI.cpp:1467-1479`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponRadii {
    pub continue_attack_range: f32,
    pub scatter_area: f32,
}

/// The ring radius for a pending command (`InGameUI::setRadiusCursor`, `InGameUI.cpp:1423-1516`).
/// `power_radius` is `SpecialPowerTemplate::RadiusCursorRadius` for a special power (every special-power
/// cursor type uses it, `:1505-1509`). Otherwise `weapon` gives the radius for the weapon cursor types.
/// `GUARD_AREA` (`AIGuardMachine::getStdGuardRange`, `:1482`) is not exposed by the sim, so it has no
/// ring here. A radius of zero is no ring (`:1513`).
pub fn radius_cursor_radius(radius_cursor: &str, power_radius: Option<f32>, weapon: Option<WeaponRadii>) -> Option<f32> {
    if radius_cursor.is_empty() {
        return None;
    }
    let radius = match power_radius {
        Some(r) => r,
        None => match radius_cursor {
            "CLEARMINES" | "ATTACK_CONTINUE_AREA" => weapon?.continue_attack_range,
            "ATTACK_SCATTER_AREA" => weapon?.scatter_area,
            _ => return None,
        },
    };
    (radius > 0.0).then_some(radius)
}

/// `InGameUI::canSelectedObjectsDoSpecialPower` (`InGameUI.cpp:4804`, `SELECTION_ANY`). An object-target
/// power needs an object; every power needs a ready source (`SpecialPowerStore::canUseSpecialPower`,
/// `special_power_ready_at`). Approximation: science, disabled state and location legality are not checked.
pub fn special_power_valid(logic: &GameLogic, selected: &[ObjectId], frame: u32, power: &str, need_obj: bool, object: Option<ObjectId>) -> bool {
    if need_obj && object.is_none() {
        return false;
    }
    // A paused module (`StartsPaused`, not yet unpaused) is never ready (`SpecialPowerModule::isReady`).
    selected.iter().any(|&u| logic.special_power_ready_at(u, power) <= frame && !logic.special_power_paused(u, power))
}

/// `InGameUI::canSelectedObjectsEffectivelyUseWeapon` (`InGameUI.cpp:4928`) over `ActionManager::canFireWeapon`,
/// `canFireWeaponAtLocation` and `canFireWeaponAtObject` (`ActionManager.cpp:1975-2040`). An object target
/// needs the weapon in `slot` to be able to attack it (range included, the button's relationship bits
/// checked by the caller). Approximation: the estimated-damage test (`estimateWeaponDamage != 0`) is skipped.
pub fn fire_weapon_valid(logic: &GameLogic, selected: &[ObjectId], slot: usize, need_obj: bool, object: Option<ObjectId>) -> bool {
    let has_weapon = |u: ObjectId| logic.find_object(u).is_some_and(|o| o.combat.weapons.weapon(slot).is_some());
    if need_obj {
        let Some(target) = object else { return false };
        return selected.iter().any(|&u| {
            has_weapon(u) && crate::context_cmd::attack_result_in_slot(logic, u, target, true, slot) != crate::context_cmd::AttackResult::NotPossible
        });
    }
    selected.iter().any(|&u| has_weapon(u))
}

/// `CommandXlat::handleGuiCommand`'s `FIRE_WEAPON` / `COMBATDROP` tests (`CommandXlat.cpp:1698-1703`,
/// `:1741`): a position drop is always valid (`!obj ? TRUE`); an object needs `canEnterObject` with
/// `COMBATDROP_INTO` (`InGameUI.cpp:4730`). Approximation: the sim has no `COMBATDROP_INTO` mode, so
/// the ordinary enter check stands in for it.
pub fn combat_drop_valid(logic: &GameLogic, selected: &[ObjectId], object: Option<ObjectId>) -> bool {
    match object {
        None => true,
        Some(target) => selected.iter().any(|&u| logic.can_enter_object(u, target, false)),
    }
}

/// `handleGuiCommand`'s enter-type test (`CommandXlat.cpp:1672-1680`): `canSelectedObjectsDoAction` for
/// hijack, car bomb or sabotage, `SELECTION_ANY`. No object is never valid.
pub fn enter_valid(logic: &GameLogic, selected: &[ObjectId], kind: EnterKind, object: Option<ObjectId>) -> bool {
    let Some(target) = object else { return false };
    selected.iter().any(|&u| match kind {
        EnterKind::HijackVehicle => logic.can_hijack_vehicle(u, target),
        EnterKind::ConvertToCarBomb => logic.can_convert_object_to_carbomb(u, target),
        EnterKind::SabotageBuilding => logic.can_sabotage_building(u, target),
    })
}

/// The ring/target forced point of a pending order (screenshot `--place-pos`), if it has one.
pub fn order_forced(order: &Order) -> Option<Coord3D> {
    match order {
        Order::Place { forced, .. }
        | Order::AttackMove { forced, .. }
        | Order::Guard { forced, .. }
        | Order::SpecialPower { forced, .. }
        | Order::SpecialPowerConstruct { forced, .. }
        | Order::CombatDrop { forced, .. } => *forced,
        Order::Rally { .. } | Order::EnterTarget { .. } | Order::FireWeapon { .. } => None,
    }
}

/// The cursor for a pending order whose pointer is over `object` (the object under the pointer, before
/// the button's relationship filter). Placement orders (`Place`, `SpecialPowerConstruct`) keep the arrow:
/// retail's `Build` / `InvalidBuild` cursors have no `Mouse.ini` block in this install (see the LOG entry),
/// so the Win32 path has no art for them.
pub(crate) fn pending_request(order: &Order, sim: &SimRes, object: Option<ObjectId>) -> CursorRequest {
    let logic = sim.game.logic();
    let local = sim.local;
    let frame = sim.game.frame();
    let sel = &sim.selected;
    let relation = |options: u32| object.filter(|&t| object_target_allowed(sim, local, t, options));
    let (spec, valid) = match order {
        Order::Rally { spec, .. } | Order::AttackMove { spec, .. } | Order::Guard { spec, .. } => (spec, true),
        Order::SpecialPower { power, need_obj, options, spec, .. } => {
            let obj = if *need_obj { relation(*options) } else { None };
            (spec, special_power_valid(logic, sel, frame, power, *need_obj, obj))
        }
        Order::FireWeapon { slot, need_obj, options, spec, .. } => {
            let obj = if *need_obj { relation(*options) } else { None };
            (spec, fire_weapon_valid(logic, sel, *slot, *need_obj, obj))
        }
        Order::EnterTarget { kind, options, spec, .. } => (spec, enter_valid(logic, sel, *kind, relation(*options))),
        Order::CombatDrop { options, spec, .. } => (spec, combat_drop_valid(logic, sel, relation(*options))),
        Order::Place { .. } | Order::SpecialPowerConstruct { .. } => return CursorRequest::from(MouseCursorName::Arrow),
    };
    cursor_request(spec, valid)
}

/// The ring radius for a pending order, if its button has a radius cursor that resolves.
pub(crate) fn pending_radius(order: &Order, sim: &SimRes) -> Option<f32> {
    match order {
        Order::SpecialPower { radius, spec, .. } => radius_cursor_radius(&spec.radius_cursor, Some(*radius), None),
        Order::FireWeapon { slot, spec, .. } => radius_cursor_radius(&spec.radius_cursor, None, weapon_radii(sim, *slot)),
        _ => None,
    }
}

/// The radii of the first selected unit's weapon in `slot` (`setRadiusCursor` uses the first selected
/// drawable, `InGameUI.cpp:1447`), read from the weapon template by name.
fn weapon_radii(sim: &SimRes, slot: usize) -> Option<WeaponRadii> {
    let first = *sim.selected.first()?;
    let obj = sim.game.logic().find_object(first)?;
    let name = &obj.combat.weapons.weapon(slot)?.template.name;
    let t = sim.game.data().templates().weapon(name)?;
    Some(WeaponRadii { continue_attack_range: t.continue_attack_range, scatter_area: t.scatter_radius + t.scatter_target_scalar })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(cursor: &str, invalid: &str, split: bool) -> PendingSpec {
        PendingSpec { cursor_name: cursor.into(), invalid_cursor_name: invalid.into(), radius_cursor: String::new(), hint_split: split }
    }

    #[test]
    fn context_command_shows_cursor_when_valid_and_invalid_cursor_otherwise() {
        // `Command_BattleshipFire` / `Command_ChinaTankECMDisableVehicle`: Target / GenericInvalid.
        let s = spec("Target", "GenericInvalid", true);
        assert_eq!(cursor_request(&s, true), CursorRequest::Named("Target"));
        assert_eq!(cursor_request(&s, false), CursorRequest::Named("GenericInvalid"));
    }

    #[test]
    fn plain_target_command_ignores_validity() {
        // `Command_AttackMove` (`AttackMove`) is a plain NEED_TARGET button, not a context command.
        let s = spec("AttackMove", "GenericInvalid", false);
        assert_eq!(cursor_request(&s, false), CursorRequest::Named("AttackMove"));
        assert_eq!(cursor_request(&s, true), CursorRequest::Named("AttackMove"));
    }

    #[test]
    fn empty_or_unknown_names_fall_back_to_cross() {
        // `Command_Guard` has no CursorName; `Mouse::getCursorIndex("")` is INVALID, so retail shows CROSS.
        assert_eq!(cursor_request(&spec("", "GenericInvalid", false), true), CursorRequest::Cross);
        assert_eq!(cursor_request(&spec("NoSuchCursor", "", false), true), CursorRequest::Cross);
        // A split command with an empty invalid name also shows CROSS when invalid.
        assert_eq!(cursor_request(&spec("Hack", "", true), false), CursorRequest::Cross);
    }

    #[test]
    fn cursor_names_resolve_ignoring_case() {
        assert_eq!(cursor_request(&spec("target", "", false), true), CursorRequest::Named("Target"));
        assert_eq!(cursor_request(&spec("ENTERAGGRESSIVE", "", false), true), CursorRequest::Named("EnterAggressive"));
    }

    #[test]
    fn special_power_radius_comes_from_the_power_and_zero_means_no_ring() {
        assert_eq!(radius_cursor_radius("DAISYCUTTER", Some(120.0), None), Some(120.0));
        assert_eq!(radius_cursor_radius("DAISYCUTTER", Some(0.0), None), None);
        // No RadiusCursorType: no ring even when the power has a radius.
        assert_eq!(radius_cursor_radius("", Some(120.0), None), None);
    }

    #[test]
    fn weapon_radius_types_use_the_weapon_and_guard_has_none() {
        let w = WeaponRadii { continue_attack_range: 15.0, scatter_area: 12.0 };
        // `Command_DisarmMinesAtPosition` (CLEARMINES) and `Command_AmericaVehicleComancheFireRocketPods` (ATTACK_SCATTER_AREA).
        assert_eq!(radius_cursor_radius("CLEARMINES", None, Some(w)), Some(15.0));
        assert_eq!(radius_cursor_radius("ATTACK_SCATTER_AREA", None, Some(w)), Some(12.0));
        assert_eq!(radius_cursor_radius("CLEARMINES", None, None), None);
        // `Command_Guard` (GUARD_AREA): the guard range is not exposed by the sim.
        assert_eq!(radius_cursor_radius("GUARD_AREA", None, Some(w)), None);
    }

    #[test]
    fn fire_weapon_click_selection() {
        // Plain FIRE_WEAPON always completes on a click, valid or not.
        assert!(fire_weapon_click_clears(false, false));
        assert!(fire_weapon_click_clears(false, true));
        // A context FIRE_WEAPON only clears (and fires) when its hint was valid.
        assert!(!fire_weapon_click_clears(true, false));
        assert!(fire_weapon_click_clears(true, true));
    }
}
