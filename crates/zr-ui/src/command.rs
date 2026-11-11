//! `CommandButton` and `CommandSet` INI (`GameClient/GUI/ControlBar/ControlBar.cpp`,
//! `CommandButton::s_commandButtonFieldParseTable` / `CommandSet::m_commandSetFieldParseTable`,
//! and the enums in `Include/GameClient/ControlBar.h`).
//!
//! A `CommandButton` names one action (construct, unit build, upgrade, special power, ...); a
//! `CommandSet` maps the 18 command-bar slots (`1`..`18`) to button names.

use std::collections::BTreeMap;

use zr_ini::IniDb;

/// `CommandOption` bits (`ControlBar.h`).
pub mod command_option {
    pub const NEED_TARGET_ENEMY_OBJECT: u32 = 0x0000_0001;
    pub const NEED_TARGET_NEUTRAL_OBJECT: u32 = 0x0000_0002;
    pub const NEED_TARGET_ALLY_OBJECT: u32 = 0x0000_0004;
    pub const NEED_TARGET_PRISONER: u32 = 0x0000_0008;
    pub const ALLOW_SHRUBBERY_TARGET: u32 = 0x0000_0010;
    pub const NEED_TARGET_POS: u32 = 0x0000_0020;
    pub const NEED_UPGRADE: u32 = 0x0000_0040;
    pub const NEED_SPECIAL_POWER_SCIENCE: u32 = 0x0000_0080;
    pub const OK_FOR_MULTI_SELECT: u32 = 0x0000_0100;
    pub const CONTEXTMODE_COMMAND: u32 = 0x0000_0200;
    pub const CHECK_LIKE: u32 = 0x0000_0400;
    pub const ALLOW_MINE_TARGET: u32 = 0x0000_0800;
    pub const ATTACK_OBJECTS_POSITION: u32 = 0x0000_1000;
    pub const OPTION_ONE: u32 = 0x0000_2000;
    pub const OPTION_TWO: u32 = 0x0000_4000;
    pub const OPTION_THREE: u32 = 0x0000_8000;
    pub const NOT_QUEUEABLE: u32 = 0x0001_0000;
    pub const SINGLE_USE_COMMAND: u32 = 0x0002_0000;
    pub const COMMAND_FIRED_BY_SCRIPT: u32 = 0x0004_0000;
    pub const SCRIPT_ONLY: u32 = 0x0008_0000;
    pub const IGNORES_UNDERPOWERED: u32 = 0x0010_0000;
    pub const USES_MINE_CLEARING_WEAPONSET: u32 = 0x0020_0000;
    pub const CAN_USE_WAYPOINTS: u32 = 0x0040_0000;
    pub const MUST_BE_STOPPED: u32 = 0x0080_0000;

    /// `TheCommandOptionNames`, in bit order (the reserved bit uses the engine's placeholder).
    pub const NAMES: &[&str] = &[
        "NEED_TARGET_ENEMY_OBJECT",
        "NEED_TARGET_NEUTRAL_OBJECT",
        "NEED_TARGET_ALLY_OBJECT",
        "NEED_TARGET_PRISONER",
        "ALLOW_SHRUBBERY_TARGET",
        "NEED_TARGET_POS",
        "NEED_UPGRADE",
        "NEED_SPECIAL_POWER_SCIENCE",
        "OK_FOR_MULTI_SELECT",
        "CONTEXTMODE_COMMAND",
        "CHECK_LIKE",
        "ALLOW_MINE_TARGET",
        "ATTACK_OBJECTS_POSITION",
        "OPTION_ONE",
        "OPTION_TWO",
        "OPTION_THREE",
        "NOT_QUEUEABLE",
        "SINGLE_USE_COMMAND",
        "COMMAND_FIRED_BY_SCRIPT",
        "SCRIPT_ONLY",
        "IGNORES_UNDERPOWERED",
        "USES_MINE_CLEARING_WEAPONSET",
        "CAN_USE_WAYPOINTS",
        "MUST_BE_STOPPED",
    ];
}

/// `CommandButtonMappedBorderType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderType {
    #[default]
    None,
    Build,
    Upgrade,
    Action,
    System,
}

impl BorderType {
    pub fn parse(s: &str) -> BorderType {
        match s.trim().to_ascii_uppercase().as_str() {
            "BUILD" => BorderType::Build,
            "UPGRADE" => BorderType::Upgrade,
            "ACTION" => BorderType::Action,
            "SYSTEM" => BorderType::System,
            _ => BorderType::None,
        }
    }
}

/// `GUICommandType` (`TheGuiCommandNames`), with an `Unknown` fallback for names not built here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandKind {
    None,
    DozerConstruct,
    DozerConstructCancel,
    UnitBuild,
    CancelUnitBuild,
    PlayerUpgrade,
    ObjectUpgrade,
    CancelUpgrade,
    AttackMove,
    Guard,
    GuardWithoutPursuit,
    GuardFlyingUnitsOnly,
    Stop,
    Waypoints,
    ExitContainer,
    Evacuate,
    ExecuteRailedTransport,
    BeaconDelete,
    SetRallyPoint,
    Sell,
    FireWeapon,
    SpecialPower,
    PurchaseScience,
    HackInternet,
    ToggleOvercharge,
    CombatDrop,
    SwitchWeapon,
    HijackVehicle,
    ConvertToCarBomb,
    SabotageBuilding,
    PlaceBeacon,
    SpecialPowerFromShortcut,
    SpecialPowerConstruct,
    SpecialPowerConstructFromShortcut,
    SelectAllUnitsOfType,
    Unknown(String),
}

impl CommandKind {
    /// `CommandButton::parseCommand` name lookup (case-insensitive); unknown names are kept.
    pub fn parse(s: &str) -> CommandKind {
        let u = s.trim().to_ascii_uppercase();
        match u.as_str() {
            "NONE" => CommandKind::None,
            "DOZER_CONSTRUCT" => CommandKind::DozerConstruct,
            "DOZER_CONSTRUCT_CANCEL" => CommandKind::DozerConstructCancel,
            "UNIT_BUILD" => CommandKind::UnitBuild,
            "CANCEL_UNIT_BUILD" => CommandKind::CancelUnitBuild,
            "PLAYER_UPGRADE" => CommandKind::PlayerUpgrade,
            "OBJECT_UPGRADE" => CommandKind::ObjectUpgrade,
            "CANCEL_UPGRADE" => CommandKind::CancelUpgrade,
            "ATTACK_MOVE" => CommandKind::AttackMove,
            "GUARD" => CommandKind::Guard,
            "GUARD_WITHOUT_PURSUIT" => CommandKind::GuardWithoutPursuit,
            "GUARD_FLYING_UNITS_ONLY" => CommandKind::GuardFlyingUnitsOnly,
            "STOP" => CommandKind::Stop,
            "WAYPOINTS" => CommandKind::Waypoints,
            "EXIT_CONTAINER" => CommandKind::ExitContainer,
            "EVACUATE" => CommandKind::Evacuate,
            "EXECUTE_RAILED_TRANSPORT" => CommandKind::ExecuteRailedTransport,
            "BEACON_DELETE" => CommandKind::BeaconDelete,
            "SET_RALLY_POINT" => CommandKind::SetRallyPoint,
            "SELL" => CommandKind::Sell,
            "FIRE_WEAPON" => CommandKind::FireWeapon,
            "SPECIAL_POWER" => CommandKind::SpecialPower,
            "PURCHASE_SCIENCE" => CommandKind::PurchaseScience,
            "HACK_INTERNET" => CommandKind::HackInternet,
            "TOGGLE_OVERCHARGE" => CommandKind::ToggleOvercharge,
            "COMBATDROP" => CommandKind::CombatDrop,
            "SWITCH_WEAPON" => CommandKind::SwitchWeapon,
            "HIJACK_VEHICLE" => CommandKind::HijackVehicle,
            "CONVERT_TO_CARBOMB" => CommandKind::ConvertToCarBomb,
            "SABOTAGE_BUILDING" => CommandKind::SabotageBuilding,
            "PLACE_BEACON" => CommandKind::PlaceBeacon,
            "SPECIAL_POWER_FROM_SHORTCUT" | "SPECIAL_POWER_FROM_COMMAND_CENTER" => {
                CommandKind::SpecialPowerFromShortcut
            }
            "SPECIAL_POWER_CONSTRUCT" => CommandKind::SpecialPowerConstruct,
            "SPECIAL_POWER_CONSTRUCT_FROM_SHORTCUT" => CommandKind::SpecialPowerConstructFromShortcut,
            "SELECT_ALL_UNITS_OF_TYPE" => CommandKind::SelectAllUnitsOfType,
            _ => CommandKind::Unknown(u),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            CommandKind::None => "NONE",
            CommandKind::DozerConstruct => "DOZER_CONSTRUCT",
            CommandKind::DozerConstructCancel => "DOZER_CONSTRUCT_CANCEL",
            CommandKind::UnitBuild => "UNIT_BUILD",
            CommandKind::CancelUnitBuild => "CANCEL_UNIT_BUILD",
            CommandKind::PlayerUpgrade => "PLAYER_UPGRADE",
            CommandKind::ObjectUpgrade => "OBJECT_UPGRADE",
            CommandKind::CancelUpgrade => "CANCEL_UPGRADE",
            CommandKind::AttackMove => "ATTACK_MOVE",
            CommandKind::Guard => "GUARD",
            CommandKind::GuardWithoutPursuit => "GUARD_WITHOUT_PURSUIT",
            CommandKind::GuardFlyingUnitsOnly => "GUARD_FLYING_UNITS_ONLY",
            CommandKind::Stop => "STOP",
            CommandKind::Waypoints => "WAYPOINTS",
            CommandKind::ExitContainer => "EXIT_CONTAINER",
            CommandKind::Evacuate => "EVACUATE",
            CommandKind::ExecuteRailedTransport => "EXECUTE_RAILED_TRANSPORT",
            CommandKind::BeaconDelete => "BEACON_DELETE",
            CommandKind::SetRallyPoint => "SET_RALLY_POINT",
            CommandKind::Sell => "SELL",
            CommandKind::FireWeapon => "FIRE_WEAPON",
            CommandKind::SpecialPower => "SPECIAL_POWER",
            CommandKind::PurchaseScience => "PURCHASE_SCIENCE",
            CommandKind::HackInternet => "HACK_INTERNET",
            CommandKind::ToggleOvercharge => "TOGGLE_OVERCHARGE",
            CommandKind::CombatDrop => "COMBATDROP",
            CommandKind::SwitchWeapon => "SWITCH_WEAPON",
            CommandKind::HijackVehicle => "HIJACK_VEHICLE",
            CommandKind::ConvertToCarBomb => "CONVERT_TO_CARBOMB",
            CommandKind::SabotageBuilding => "SABOTAGE_BUILDING",
            CommandKind::PlaceBeacon => "PLACE_BEACON",
            CommandKind::SpecialPowerFromShortcut => "SPECIAL_POWER_FROM_SHORTCUT",
            CommandKind::SpecialPowerConstruct => "SPECIAL_POWER_CONSTRUCT",
            CommandKind::SpecialPowerConstructFromShortcut => "SPECIAL_POWER_CONSTRUCT_FROM_SHORTCUT",
            CommandKind::SelectAllUnitsOfType => "SELECT_ALL_UNITS_OF_TYPE",
            CommandKind::Unknown(s) => s,
        }
    }
}

/// One `CommandButton` definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandButton {
    pub name: String,
    pub command: CommandKind,
    pub options: u32,
    pub object: Option<String>,
    pub upgrade: Option<String>,
    pub special_power: Option<String>,
    pub weapon_slot: Option<String>,
    pub max_shots_to_fire: i32,
    pub science: Vec<String>,
    pub text_label: Option<String>,
    pub description_label: Option<String>,
    pub purchased_label: Option<String>,
    pub conflicting_label: Option<String>,
    pub button_image: Option<String>,
    pub cursor_name: Option<String>,
    pub invalid_cursor_name: Option<String>,
    pub border_type: BorderType,
    pub radius_cursor: Option<String>,
    pub unit_specific_sound: Option<String>,
}

impl CommandButton {
    fn parse(name: &str, b: &zr_ini::Block) -> CommandButton {
        let s = |k: &str| b.value(k).map(crate::value::trimmed).filter(|v| !v.is_empty());
        CommandButton {
            name: name.to_owned(),
            command: b.value("Command").map(CommandKind::parse).unwrap_or(CommandKind::None),
            options: b.value("Options").map_or(0, |v| crate::value::bits(v, command_option::NAMES)),
            object: s("Object"),
            upgrade: s("Upgrade"),
            special_power: s("SpecialPower"),
            weapon_slot: s("WeaponSlot"),
            max_shots_to_fire: b.value("MaxShotsToFire").map_or(0x7fff_ffff, crate::value::int),
            science: b.value("Science").map(crate::value::words).unwrap_or_default(),
            text_label: s("TextLabel"),
            description_label: s("DescriptLabel"),
            purchased_label: s("PurchasedLabel"),
            conflicting_label: s("ConflictingLabel"),
            button_image: s("ButtonImage"),
            cursor_name: s("CursorName"),
            invalid_cursor_name: s("InvalidCursorName"),
            border_type: b.value("ButtonBorderType").map_or(BorderType::None, BorderType::parse),
            radius_cursor: s("RadiusCursorType"),
            unit_specific_sound: s("UnitSpecificSound"),
        }
    }
}

/// `MAX_COMMANDS_PER_SET`: 18 command-bar slots.
pub const COMMAND_SET_SLOTS: usize = 18;

/// One `CommandSet`: slot index 0..17 -> command button name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandSet {
    pub name: String,
    pub slots: [Option<String>; COMMAND_SET_SLOTS],
}

impl CommandSet {
    /// The command button name in slot `i` (0-based), if any.
    pub fn slot(&self, i: usize) -> Option<&str> {
        self.slots.get(i).and_then(|s| s.as_deref())
    }
}

/// Every `CommandButton` and `CommandSet`, with name lookup.
#[derive(Debug, Clone, Default)]
pub struct CommandDb {
    buttons: Vec<CommandButton>,
    button_index: BTreeMap<String, usize>,
    sets: Vec<CommandSet>,
    set_index: BTreeMap<String, usize>,
}

impl CommandDb {
    /// Builds the database from the merged [`IniDb`] (`parseCommandButtonDefinition` /
    /// `parseCommandSetDefinition`).
    pub fn from_ini(db: &IniDb) -> CommandDb {
        let mut out = CommandDb::default();
        for b in db.of_kind("CommandButton") {
            let name = b.name().unwrap_or("").to_owned();
            let Some(resolved) = db.resolved("CommandButton", &name) else { continue };
            let button = CommandButton::parse(&name, &resolved);
            out.button_index.insert(name, out.buttons.len());
            out.buttons.push(button);
        }
        for b in db.of_kind("CommandSet") {
            let name = b.name().unwrap_or("").to_owned();
            let Some(resolved) = db.resolved("CommandSet", &name) else { continue };
            let mut set = CommandSet { name: name.clone(), ..Default::default() };
            for i in 0..COMMAND_SET_SLOTS {
                if let Some(v) = resolved.value(&(i + 1).to_string()) {
                    let v = crate::value::trimmed(v);
                    if !v.is_empty() {
                        set.slots[i] = Some(v);
                    }
                }
            }
            out.set_index.insert(name, out.sets.len());
            out.sets.push(set);
        }
        out
    }

    /// `ControlBar::findCommandButton` (exact name).
    pub fn button(&self, name: &str) -> Option<&CommandButton> {
        self.button_index.get(name).map(|&i| &self.buttons[i])
    }

    /// `ControlBar::findCommandSet` (exact name).
    pub fn set(&self, name: &str) -> Option<&CommandSet> {
        self.set_index.get(name).map(|&i| &self.sets[i])
    }

    pub fn buttons(&self) -> &[CommandButton] {
        &self.buttons
    }

    pub fn sets(&self) -> &[CommandSet] {
        &self.sets
    }

    pub fn len(&self) -> (usize, usize) {
        (self.buttons.len(), self.sets.len())
    }

    pub fn is_empty(&self) -> bool {
        self.buttons.is_empty() && self.sets.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_db(ini: &str) -> IniDb {
        let mut db = IniDb::new();
        db.extend(zr_ini::parse("test.ini", ini).unwrap());
        db
    }

    #[test]
    fn parses_button_fields() {
        let db = parse_db(
            r#"
CommandButton Command_ConstructAmericaDozer
  Command = UNIT_BUILD
  Object = AmericaVehicleDozer
  TextLabel = CONTROLBAR:ConstructAmericaDozer
  ButtonImage = SACDozer
  ButtonBorderType = BUILD
  Options = NEED_TARGET_POS OPTION_ONE
  RadiusCursorType = DAISYCUTTER
End
"#,
        );
        let c = CommandDb::from_ini(&db);
        let b = c.button("Command_ConstructAmericaDozer").unwrap();
        assert_eq!(b.command, CommandKind::UnitBuild);
        assert_eq!(b.object.as_deref(), Some("AmericaVehicleDozer"));
        assert_eq!(b.button_image.as_deref(), Some("SACDozer"));
        assert_eq!(b.border_type, BorderType::Build);
        assert_eq!(b.options, command_option::NEED_TARGET_POS | command_option::OPTION_ONE);
        assert_eq!(b.radius_cursor.as_deref(), Some("DAISYCUTTER"));
    }

    #[test]
    fn parses_command_set_slots() {
        let db = parse_db(
            r#"
CommandButton A
  Command = STOP
End
CommandButton B
  Command = GUARD
End
CommandSet GenericCommandSet
  11 = A
  14 = B
End
"#,
        );
        let c = CommandDb::from_ini(&db);
        let set = c.set("GenericCommandSet").unwrap();
        assert_eq!(set.slot(10).map(str::to_owned), Some("A".to_owned()));
        assert_eq!(set.slot(13).map(str::to_owned), Some("B".to_owned()));
        assert_eq!(set.slot(0), None);
        assert_eq!(c.len(), (2, 1));
    }

    #[test]
    fn unknown_command_is_preserved() {
        assert_eq!(CommandKind::parse("SOMETHING_NEW"), CommandKind::Unknown("SOMETHING_NEW".into()));
    }
}
