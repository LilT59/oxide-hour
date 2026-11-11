//! Shared, immutable game data: typed templates plus the skirmish-relevant slices of the INI
//! (`PlayerTemplate`, `MultiplayerStartingMoneyChoice`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use zr_assets::Vfs;
use zr_ini::{Block, IniDb};
use zr_templates::{Templates, Warning};
use zr_text::GameText;

use crate::adapter::TemplateStore;

/// `MAX_MP_STARTING_UNITS` (PlayerTemplate.h).
pub const MAX_MP_STARTING_UNITS: usize = 10;

/// `PlayerTemplate` (Common/RTS/PlayerTemplate.cpp): the fields the skirmish start reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerTemplate {
    /// `FactionAmerica`, ...
    pub name: String,
    pub side: String,
    /// `StartMoney` (0 in all shipped templates: the skirmish starting cash applies instead).
    pub start_money: u32,
    pub starting_building: String,
    /// `StartingUnit0..9`, empty slots dropped (slot order kept).
    pub starting_units: Vec<String>,
    pub is_observer: bool,
    /// `PurchaseScienceCommandSetRank1/3/8` (`ControlBar::populatePurchaseScience`).
    pub purchase_science_rank1: String,
    pub purchase_science_rank3: String,
    pub purchase_science_rank8: String,
    /// `SpecialPowerShortcutCommandSet` / `SpecialPowerShortcutWinName` / `...ButtonCount`.
    pub special_power_shortcut_set: String,
    pub special_power_shortcut_win: String,
    pub special_power_shortcut_buttons: u32,
    /// `IntrinsicSciences`: granted at game start.
    pub intrinsic_sciences: Vec<String>,
}

impl PlayerTemplate {
    fn from_block(b: &Block) -> Option<Self> {
        let name = b.name()?.to_owned();
        let get = |k: &str| b.value(k).map(|v| v.trim().to_owned()).unwrap_or_default();
        let starting_units = (0..MAX_MP_STARTING_UNITS)
            .map(|i| get(&format!("StartingUnit{i}")))
            .filter(|s| !s.is_empty())
            .collect();
        let intrinsic_sciences =
            b.value("IntrinsicSciences").map(|v| v.split_whitespace().map(str::to_owned).collect()).unwrap_or_default();
        Some(Self {
            name,
            side: get("Side"),
            start_money: get("StartMoney").parse().unwrap_or(0),
            starting_building: get("StartingBuilding"),
            starting_units,
            is_observer: get("IsObserver").eq_ignore_ascii_case("yes"),
            purchase_science_rank1: get("PurchaseScienceCommandSetRank1"),
            purchase_science_rank3: get("PurchaseScienceCommandSetRank3"),
            purchase_science_rank8: get("PurchaseScienceCommandSetRank8"),
            special_power_shortcut_set: get("SpecialPowerShortcutCommandSet"),
            special_power_shortcut_win: get("SpecialPowerShortcutWinName"),
            special_power_shortcut_buttons: get("SpecialPowerShortcutButtonCount").parse().unwrap_or(0),
            intrinsic_sciences,
        })
    }
}

/// Everything loaded once from the install and shared by every [`crate::Game`].
///
/// A map with its own `map.ini` / `map.str` gets a derived copy from [`GameData::for_map`]; the
/// shared instance is never mutated.
pub struct GameData {
    pub store: TemplateStore,
    /// `CommandButton` hunt table for `TEAM_HUNT_WITH_COMMAND_BUTTON`, by name (installed into the
    /// script engine by the game loader).
    pub command_buttons: BTreeMap<String, zr_sim::script::CommandButtonHunt>,
    /// Special-power `CommandButton`s with their power and target options, for the team
    /// command-button actions (installed by the game loader).
    pub command_button_powers: BTreeMap<String, zr_sim::script::CommandButtonPower>,
    /// `PLAYER_UPGRADE` / `OBJECT_UPGRADE` `CommandButton`s and the upgrade each one queues
    /// (`CommandButton::getUpgradeTemplate`), by name (installed by the game loader; the team command-button
    /// actions queue them, `ScriptActions.cpp:4463` -> `Object::doCommandButton`).
    pub command_button_upgrades: BTreeMap<String, zr_sim::script::CommandButtonUpgrade>,
    /// The other `CommandButton`s the team command-button actions run on a member, a target or a position
    /// (`CommandButtonAction`, `Object::doCommandButton*`), by name (installed by the game loader).
    pub command_button_actions: BTreeMap<String, zr_sim::script::CommandButtonAction>,
    /// In first-definition order (the original's `getNthPlayerTemplate` order).
    pub player_templates: Vec<PlayerTemplate>,
    /// `MultiplayerSettings::getDefaultStartingMoney`: the `MultiplayerStartingMoneyChoice` marked
    /// `Default = Yes` (10000 in Zero Hour).
    pub default_starting_cash: u32,
    /// Template-building warnings (0 on the shipped data).
    pub warnings: Vec<Warning>,
    /// `TheGameText`: the main string table, plus the map's `map.str` on a [`for_map`](Self::for_map) copy.
    /// Empty when the install has no string file (see `text_error`).
    pub text: Arc<GameText>,
    pub text_error: Option<String>,
    /// `map.ini` / `solo.ini` files overlaid on a [`for_map`](Self::for_map) copy (empty on the shared data).
    pub map_ini_files: Vec<String>,
    /// Parse failures of those files.
    pub map_ini_failures: Vec<String>,
    /// `GameData` `UnitDamagedThreshold` (body `BODY_DAMAGED` below this health ratio).
    pub unit_damaged_threshold: f32,
    /// `GameData` `UnitReallyDamagedThreshold` (`BODY_REALLYDAMAGED` below this ratio).
    pub unit_really_damaged_threshold: f32,
    /// `GameData` `MovementPenaltyDamageState` (`GlobalData.cpp:411`; engine default `BODY_REALLYDAMAGED`, `:1032`).
    pub movement_penalty_damage_state: zr_sim::BodyDamageType,
    /// `GameData` `PartitionCellSize`: shroud/partition grid edge in world units (retail 40).
    pub partition_cell_size: f32,
    /// `GameData` `ShroudColor` (`R:r G:g B:b`), the tint of the shroud overlay (retail white).
    pub shroud_color: [u8; 3],
    /// `GameData` `ClearAlpha`: overlay alpha for a fully visible cell (retail 255).
    pub clear_alpha: u8,
    /// `GameData` `FogAlpha`: overlay alpha for a fogged (seen, unwatched) cell (retail 127).
    pub fog_alpha: u8,
    /// `GameData` `ShroudAlpha`: overlay alpha for a never-seen cell (retail 0, i.e. black).
    pub shroud_alpha: u8,
    /// `MultiplayerSettings` `UseShroud` (`MultiplayerSettings::isShroudInMultiplayer`). Retail
    /// `Multiplayer.ini` says `No`: `GameLogic::startNewGame` then reveals the map for every
    /// occupied slot (`revealMapForPlayer`), so the start is fogged, not shrouded (`GameLogic.cpp:1814`).
    pub use_shroud_in_multiplayer: bool,
    /// `GameData` `BaseRegenHealthPercentPerSecond` (`BaseRegenerateUpdate`).
    pub base_regen_health_percent_per_second: f32,
    /// `GameData` `BaseRegenDelay`, frames.
    pub base_regen_delay: u32,
    /// `GameData` `MaxTunnelCapacity`: units one player's tunnel network can hold (retail 10).
    pub max_tunnel_capacity: i32,
    /// The base INI database, kept so a map's overrides can be overlaid and rolled back
    /// (`GameLogic::loadMapINI`). `None` for [`from_ini`](Self::from_ini) and derived copies.
    base_db: Option<Mutex<IniDb>>,
}

impl GameData {
    /// Parse the whole INI database and the string table out of `vfs` and build the typed templates.
    pub fn load(vfs: &Vfs) -> Self {
        let loaded = zr_ini::load_zero_hour(vfs);
        let mut data = Self::from_ini(&loaded.db);
        // Resolve each garrison's `FIREPOINT` bones from the W3D art before any template is used.
        let fire_points = crate::firepoints::garrison_fire_points(&loaded.db, vfs, data.templates());
        data.store.set_fire_points(fire_points);
        // Resolve each object's `WeaponLaunchBone` launch positions from the W3D art.
        let launch_bones = crate::launch_bones::launch_bones(&loaded.db, vfs, data.templates());
        data.store.set_launch_bones(launch_bones);
        // Resolve each dock's `DockStart`/`DockAction`/`DockEnd`/`DockWaiting` bones.
        let dock_bones = crate::dock_bones::dock_bones(&loaded.db, vfs, data.templates());
        data.store.set_dock_bones(dock_bones);
        // Resolve each airfield's parking-space / runway bones.
        let parking_bones = crate::parking_bones::parking_bones(&loaded.db, vfs, data.templates());
        data.store.set_parking_bones(parking_bones);
        let spawn_bones = crate::spawn_bones::spawn_bones(&loaded.db, vfs, data.templates());
        data.store.set_spawn_bones(spawn_bones);
        match zr_text::load(vfs, &zr_text::detect_language(vfs)) {
            Ok(t) => data.text = Arc::new(t),
            Err(e) => data.text_error = Some(e.to_string()),
        }
        data.base_db = Some(Mutex::new(loaded.db));
        data
    }

    /// Build from an INI database (no strings, and no per-map overrides possible).
    pub fn from_ini(db: &IniDb) -> Self {
        Self::from_ini_overlay(db, usize::MAX)
    }

    /// [`from_ini`](Self::from_ini) where blocks from load-order position `first_override` on are a
    /// map's `CREATE_OVERRIDES` load ([`Templates::from_ini_overlay`]).
    pub fn from_ini_overlay(db: &IniDb, first_override: usize) -> Self {
        let (templates, warnings) = Templates::from_ini_overlay(db, first_override);
        let player_templates = db.of_kind("PlayerTemplate").filter_map(PlayerTemplate::from_block).collect();
        let mut default_starting_cash = 0;
        for b in db.iter().filter(|b| b.kind == "MultiplayerStartingMoneyChoice") {
            let is_default = b.value("Default").is_some_and(|v| v.trim().eq_ignore_ascii_case("yes"));
            if is_default {
                default_starting_cash = b.value("Value").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
            }
        }
        // `GameData` body-damage thresholds (`GlobalData.cpp`); defaults match the engine ctor.
        let (mut unit_damaged_threshold, mut unit_really_damaged_threshold) = (0.5f32, 0.1f32);
        let mut movement_penalty_damage_state = zr_sim::BodyDamageType::ReallyDamaged;
        // Shroud / partition fields (`GlobalData.cpp`); defaults match the engine ctor.
        let mut partition_cell_size = 40.0f32;
        let mut shroud_color = [255u8, 255, 255];
        let (mut clear_alpha, mut fog_alpha, mut shroud_alpha) = (255u8, 127u8, 0u8);
        let (mut base_regen_health_percent_per_second, mut base_regen_delay) = (0.0f32, 0u32);
        let mut max_tunnel_capacity = 10i32;
        // `MultiplayerSettings::m_isShroudInMultiplayer` (engine default TRUE; retail INI `No`).
        let mut use_shroud_in_multiplayer = true;
        for b in db.iter().filter(|b| b.kind == "MultiplayerSettings") {
            if let Some(v) = b.value("UseShroud") {
                use_shroud_in_multiplayer = v.trim().eq_ignore_ascii_case("yes");
            }
        }
        for b in db.iter().filter(|b| b.kind == "GameData") {
            if let Some(v) = b.value("UnitDamagedThreshold").and_then(|v| v.trim().parse().ok()) {
                unit_damaged_threshold = v;
            }
            if let Some(v) = b.value("UnitReallyDamagedThreshold").and_then(|v| v.trim().parse().ok()) {
                unit_really_damaged_threshold = v;
            }
            if let Some(v) = b.value("MovementPenaltyDamageState").and_then(parse_body_damage_type) {
                movement_penalty_damage_state = v;
            }
            if let Some(v) = b.value("PartitionCellSize").and_then(|v| v.trim().parse().ok()) {
                partition_cell_size = v;
            }
            if let Some(v) = b.value("ShroudColor") {
                shroud_color = parse_rgb(v).unwrap_or(shroud_color);
            }
            if let Some(v) = b.value("ClearAlpha").and_then(|v| v.trim().parse().ok()) {
                clear_alpha = v;
            }
            if let Some(v) = b.value("FogAlpha").and_then(|v| v.trim().parse().ok()) {
                fog_alpha = v;
            }
            if let Some(v) = b.value("ShroudAlpha").and_then(|v| v.trim().parse().ok()) {
                shroud_alpha = v;
            }
            if let Some(v) = b.value("BaseRegenHealthPercentPerSecond") {
                base_regen_health_percent_per_second = v.trim().trim_end_matches('%').parse::<f32>().map_or(0.0, |p| p / 100.0);
            }
            if let Some(v) = b.value("BaseRegenDelay") {
                if let Ok(ms) = v.trim().parse::<f32>() {
                    base_regen_delay = (ms * 30.0 / 1000.0).ceil() as u32;
                }
            }
            if let Some(v) = b.value("MaxTunnelCapacity").and_then(|v| v.trim().parse().ok()) {
                max_tunnel_capacity = v;
            }
        }
        let store = TemplateStore::new(templates);
        store.set_unit_builds(unit_build_sets(db));
        store.set_upgrade_builds(upgrade_build_sets(db));
        let command_buttons = command_button_hunts(db);
        let command_button_powers = command_button_powers(db);
        let command_button_upgrades = command_button_upgrades(db);
        let command_button_actions = command_button_actions(db);
        Self {
            store,
            command_buttons,
            command_button_powers,
            command_button_upgrades,
            command_button_actions,
            player_templates,
            default_starting_cash,
            warnings,
            text: Arc::new(GameText::default()),
            text_error: None,
            map_ini_files: Vec::new(),
            map_ini_failures: Vec::new(),
            unit_damaged_threshold,
            unit_really_damaged_threshold,
            movement_penalty_damage_state,
            partition_cell_size,
            shroud_color,
            clear_alpha,
            fog_alpha,
            shroud_alpha,
            use_shroud_in_multiplayer,
            base_regen_health_percent_per_second,
            base_regen_delay,
            max_tunnel_capacity,
            base_db: None,
        }
    }

    /// The data a game on `map_path` must use. Ports `GameLogic::loadMapINI`: overlay the map's
    /// `map.ini` and `solo.ini` (create-override semantics) before templates are used, and install
    /// its `map.str` into the text table. When the map has neither (or this data carries no INI
    /// database) the shared data is returned as is. Otherwise the INI is overlaid, templates are
    /// rebuilt, and the overlay is rolled back (the engine drops overrides on reset), so the shared
    /// state is unchanged.
    pub fn for_map(self: &Arc<Self>, vfs: &Vfs, map_path: &str) -> Arc<GameData> {
        let Some(base) = &self.base_db else { return self.clone() };
        let mut db = base.lock().unwrap_or_else(|e| e.into_inner());
        let overlay = zr_ini::load_map_ini(&mut db, vfs, map_path);
        let mut text_error = None;
        let map_table = overlay.map_str.as_deref().and_then(|p| match zr_text::load_map_strings(vfs, p)? {
            Ok(t) => Some(t),
            Err(e) => {
                text_error = Some(e.to_string());
                None
            }
        });
        if overlay.loaded.is_empty() && overlay.failures.is_empty() && map_table.is_none() {
            return self.clone();
        }
        // Templates are rebuilt from the overlaid database (also when only strings were added,
        // since `TemplateStore` owns its templates), then the overlay is rolled back.
        let mut d = Self::from_ini_overlay(&db, overlay.checkpoint);
        // Resolve garrison fire points against the overlaid database before rolling it back.
        let fire_points = crate::firepoints::garrison_fire_points(&db, vfs, d.templates());
        d.store.set_fire_points(fire_points);
        let launch_bones = crate::launch_bones::launch_bones(&db, vfs, d.templates());
        d.store.set_launch_bones(launch_bones);
        let dock_bones = crate::dock_bones::dock_bones(&db, vfs, d.templates());
        d.store.set_dock_bones(dock_bones);
        let parking_bones = crate::parking_bones::parking_bones(&db, vfs, d.templates());
        d.store.set_parking_bones(parking_bones);
        let spawn_bones = crate::spawn_bones::spawn_bones(&db, vfs, d.templates());
        d.store.set_spawn_bones(spawn_bones);
        db.rollback(overlay.checkpoint);
        drop(db);
        let mut text = (*self.text).clone();
        if let Some(t) = map_table {
            text.set_map_strings(t);
        }
        d.text = Arc::new(text);
        d.text_error = self.text_error.clone().or(text_error);
        d.map_ini_files = overlay.loaded;
        d.map_ini_failures = overlay.failures.iter().map(|e| e.to_string()).collect();
        Arc::new(d)
    }

    pub fn templates(&self) -> &Templates {
        self.store.templates()
    }

    /// The merged INI database, when this data owns one (`load`; derived `for_map` copies do not).
    /// Used to build client-side map geometry (bridges) that is defined by INI blocks.
    pub fn ini_db(&self) -> Option<std::sync::MutexGuard<'_, IniDb>> {
        self.base_db.as_ref().map(|m| m.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn player_template(&self, name: &str) -> Option<&PlayerTemplate> {
        self.player_templates.iter().find(|p| p.name == name)
    }

    /// The localised display name of a template: its `DisplayName` label looked up in the main
    /// table, then the map's. `None` when the template has no `DisplayName` or the label is missing.
    pub fn display_name(&self, template: &str) -> Option<&str> {
        let label = self.templates().thing(template)?.display_name.as_deref()?;
        self.text.get(label)
    }
}

/// `INI::parseIndexList` over `TheBodyDamageTypeNames` (`BodyModule.cpp`/`.h:61-69`): a `BodyDamageType` name.
fn parse_body_damage_type(s: &str) -> Option<zr_sim::BodyDamageType> {
    use zr_sim::BodyDamageType::*;
    match s.trim().to_ascii_uppercase().as_str() {
        "PRISTINE" => Some(Pristine),
        "DAMAGED" => Some(Damaged),
        "REALLYDAMAGED" => Some(ReallyDamaged),
        "RUBBLE" => Some(Rubble),
        _ => None,
    }
}

/// `INI::parseRGBColor`: `R:255 G:255 B:255`.
fn parse_rgb(s: &str) -> Option<[u8; 3]> {
    let mut out = [0u8; 3];
    for (i, key) in ["R:", "G:", "B:"].iter().enumerate() {
        let start = s.find(key)? + key.len();
        let rest = &s[start..];
        let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        out[i] = rest[..end].parse().ok()?;
    }
    Some(out)
}

/// Every `CommandButton` as the hunt `ScriptActions::doTeamHuntWithCommandButton` needs
/// (`ScriptActions.cpp:2073-2215`): the weapon and enter command types, and a `SPECIAL_POWER` button
/// only when it takes an object target (`COMMAND_OPTION_NEED_OBJECT_TARGET`).
fn command_button_hunts(db: &IniDb) -> BTreeMap<String, zr_sim::script::CommandButtonHunt> {
    use zr_sim::enter_actions::EnterKind;
    use zr_sim::script::CommandButtonHunt;
    use zr_ui::{command_option, CommandDb, CommandKind};
    const NEED_OBJECT_TARGET: u32 = command_option::NEED_TARGET_ENEMY_OBJECT
        | command_option::NEED_TARGET_NEUTRAL_OBJECT
        | command_option::NEED_TARGET_ALLY_OBJECT;
    CommandDb::from_ini(db)
        .buttons()
        .iter()
        .map(|b| {
            let hunt = match &b.command {
                CommandKind::FireWeapon | CommandKind::SwitchWeapon => CommandButtonHunt::Weapon,
                CommandKind::SpecialPower => match &b.special_power {
                    Some(power) if b.options & NEED_OBJECT_TARGET != 0 => CommandButtonHunt::SpecialPower { power: power.clone() },
                    _ => CommandButtonHunt::Refused,
                },
                CommandKind::HijackVehicle => CommandButtonHunt::Enter(EnterKind::HijackVehicle),
                CommandKind::ConvertToCarBomb => CommandButtonHunt::Enter(EnterKind::ConvertToCarBomb),
                CommandKind::SabotageBuilding => CommandButtonHunt::Enter(EnterKind::SabotageBuilding),
                _ => CommandButtonHunt::Refused,
            };
            (b.name.clone(), hunt)
        })
        .collect()
}

/// Every special-power `CommandButton` with its power and target options (`ControlBar.h`:
/// `COMMAND_OPTION_NEED_OBJECT_TARGET` is the enemy, neutral and ally object bits; `NEED_TARGET_POS`
/// is its own bit). The team command-button actions read these (`ScriptActions.cpp:4463-5983`).
fn command_button_powers(db: &IniDb) -> BTreeMap<String, zr_sim::script::CommandButtonPower> {
    use zr_sim::script::CommandButtonPower;
    use zr_ui::{command_option, CommandDb, CommandKind};
    const NEED_OBJECT_TARGET: u32 = command_option::NEED_TARGET_ENEMY_OBJECT
        | command_option::NEED_TARGET_NEUTRAL_OBJECT
        | command_option::NEED_TARGET_ALLY_OBJECT;
    CommandDb::from_ini(db)
        .buttons()
        .iter()
        .filter(|b| matches!(b.command, CommandKind::SpecialPower))
        .filter_map(|b| {
            let power = b.special_power.clone()?;
            Some((
                b.name.clone(),
                CommandButtonPower {
                    power,
                    need_object_target: b.options & NEED_OBJECT_TARGET != 0,
                    need_target_pos: b.options & command_option::NEED_TARGET_POS != 0,
                },
            ))
        })
        .collect()
}

/// Every `PLAYER_UPGRADE` / `OBJECT_UPGRADE` `CommandButton` with the upgrade it queues (its `Upgrade` field) and
/// whether that upgrade is an object upgrade, the table the team command-button actions read (`ScriptActions.cpp:4463`,
/// `Object::doCommandButton` `Object.cpp:5556`). A button whose upgrade has no `Upgrade` section has no entry
/// (`UpgradeCenter::findUpgrade` returns null).
fn command_button_upgrades(db: &IniDb) -> BTreeMap<String, zr_sim::script::CommandButtonUpgrade> {
    use zr_sim::script::CommandButtonUpgrade;
    use zr_ui::{CommandDb, CommandKind};
    CommandDb::from_ini(db)
        .buttons()
        .iter()
        .filter(|b| matches!(b.command, CommandKind::PlayerUpgrade | CommandKind::ObjectUpgrade))
        .filter_map(|b| {
            let upgrade = b.upgrade.clone()?;
            let object = upgrade_is_object(db, &upgrade)?;
            Some((b.name.clone(), CommandButtonUpgrade { upgrade, object }))
        })
        .collect()
}

/// The `CommandButton`s the team command-button actions run for a member, a target or a position, with the fields each
/// `Object::doCommandButton*` branch reads (Object.cpp:5504-5830): the `WeaponSlot` (default PRIMARY, ControlBar.cpp:568),
/// `MaxShotsToFire` (default 0x7fffffff, :569), the `NEED_TARGET_*` / `ATTACK_OBJECTS_POSITION` options, and the `Object`
/// template of a unit build (`getThingTemplate`). Special-power and upgrade buttons have their own tables; the command
/// types not listed here do nothing in these branches.
fn command_button_actions(db: &IniDb) -> BTreeMap<String, zr_sim::script::CommandButtonAction> {
    use zr_sim::script::{CommandButtonAction, FireTargets};
    use zr_templates::{NameEnum, WeaponSlot};
    use zr_ui::{command_option, CommandDb, CommandKind};
    // `WeaponSlot` names -> `WeaponSlotType` index (PRIMARY 0, SECONDARY 1, TERTIARY 2); an absent field is PRIMARY.
    let slot = |name: Option<&str>| name.and_then(|n| WeaponSlot::NAMES.iter().position(|s| s.eq_ignore_ascii_case(n))).unwrap_or(0);
    CommandDb::from_ini(db)
        .buttons()
        .iter()
        .filter_map(|b| {
            let action = match &b.command {
                CommandKind::Stop => CommandButtonAction::Stop,
                CommandKind::Sell => CommandButtonAction::Sell,
                CommandKind::HackInternet => CommandButtonAction::HackInternet,
                CommandKind::SwitchWeapon => CommandButtonAction::SwitchWeapon { slot: slot(b.weapon_slot.as_deref()) },
                CommandKind::FireWeapon => CommandButtonAction::FireWeapon {
                    slot: slot(b.weapon_slot.as_deref()),
                    max_shots: b.max_shots_to_fire,
                    targets: FireTargets {
                        enemy: b.options & command_option::NEED_TARGET_ENEMY_OBJECT != 0,
                        neutral: b.options & command_option::NEED_TARGET_NEUTRAL_OBJECT != 0,
                        ally: b.options & command_option::NEED_TARGET_ALLY_OBJECT != 0,
                        position: b.options & command_option::NEED_TARGET_POS != 0,
                        attack_objects_position: b.options & command_option::ATTACK_OBJECTS_POSITION != 0,
                    },
                },
                CommandKind::UnitBuild => CommandButtonAction::UnitBuild { thing: b.object.clone()? },
                CommandKind::DozerConstruct => CommandButtonAction::DozerConstruct { thing: b.object.clone()? },
                _ => return None,
            };
            Some((b.name.clone(), action))
        })
        .collect()
}

/// `UpgradeTemplate::m_type` (`Type = PLAYER` is the default, `OBJECT` is `UPGRADE_TYPE_OBJECT`): `Some(true)` for an
/// object upgrade, `None` when the upgrade has no `Upgrade` section.
fn upgrade_is_object(db: &IniDb, name: &str) -> Option<bool> {
    let r = db.resolved("Upgrade", name)?;
    Some(r.value("Type").is_some_and(|t| t.trim().eq_ignore_ascii_case("OBJECT")))
}

/// `CommandSet` name -> the thing names its `UNIT_BUILD` / `DOZER_CONSTRUCT` buttons make
/// (`BuildAssistant::isPossibleToMakeUnit` scans exactly these, `BuildAssistant.cpp:1249`).
fn unit_build_sets(db: &IniDb) -> BTreeMap<String, BTreeSet<String>> {
    let mut buttons: BTreeMap<String, String> = BTreeMap::new();
    for b in db.of_kind("CommandButton") {
        let name = b.name().unwrap_or("").to_owned();
        let Some(r) = db.resolved("CommandButton", &name) else { continue };
        let is_build = r.value("Command").is_some_and(|c| {
            let c = c.trim();
            c.eq_ignore_ascii_case("UNIT_BUILD") || c.eq_ignore_ascii_case("DOZER_CONSTRUCT")
        });
        if let (true, Some(obj)) = (is_build, r.value("Object")) {
            buttons.insert(name, obj.trim().to_owned());
        }
    }
    let mut sets = BTreeMap::new();
    for b in db.of_kind("CommandSet") {
        let name = b.name().unwrap_or("").to_owned();
        let Some(r) = db.resolved("CommandSet", &name) else { continue };
        let mut made = BTreeSet::new();
        for i in 1..=18 {
            if let Some(obj) = r.value(&i.to_string()).and_then(|v| buttons.get(v.trim())) {
                made.insert(obj.clone());
            }
        }
        sets.insert(name, made);
    }
    sets
}

/// `CommandSet` name -> the upgrade names its `PLAYER_UPGRADE` / `OBJECT_UPGRADE` buttons carry
/// (`CommandButton::getUpgradeTemplate`, the button's `Upgrade` field). `AIPlayer::buildUpgrade`
/// queues an upgrade only at a factory whose set holds the button (`AIPlayer.cpp:1822-1836`). Names are
/// lowercased, as the script upgrade names are compared case-insensitively.
fn upgrade_build_sets(db: &IniDb) -> BTreeMap<String, BTreeSet<String>> {
    let mut buttons: BTreeMap<String, String> = BTreeMap::new();
    for b in db.of_kind("CommandButton") {
        let name = b.name().unwrap_or("").to_owned();
        let Some(r) = db.resolved("CommandButton", &name) else { continue };
        let is_upgrade = r.value("Command").is_some_and(|c| {
            let c = c.trim();
            c.eq_ignore_ascii_case("PLAYER_UPGRADE") || c.eq_ignore_ascii_case("OBJECT_UPGRADE")
        });
        if let (true, Some(up)) = (is_upgrade, r.value("Upgrade")) {
            buttons.insert(name, up.trim().to_ascii_lowercase());
        }
    }
    let mut sets = BTreeMap::new();
    for b in db.of_kind("CommandSet") {
        let name = b.name().unwrap_or("").to_owned();
        let Some(r) = db.resolved("CommandSet", &name) else { continue };
        let mut made = BTreeSet::new();
        for i in 1..=18 {
            if let Some(up) = r.value(&i.to_string()).and_then(|v| buttons.get(v.trim())) {
                made.insert(up.clone());
            }
        }
        sets.insert(name, made);
    }
    sets
}
