//! Shell / skirmish front-end: the map list, player slots and options from
//! `Menus/SkirmishGameOptionsMenu.wnd` (`SkirmishGameOptionsMenu.cpp`, `GameInfo`,
//! `SkirmishGameInfo`, `GUIUtil.cpp`).
//!
//! [`ShellData`] is loaded once from the install (map list, playable sides, multiplayer colours,
//! starting-cash choices, the parsed `.wnd` and its decoded images); [`ShellConfig`] is the live
//! selection. The menu widgets live in [`crate::menu`]. Starting a game builds a [`GameConfig`]
//! and hands it to `main`'s `run_build` through [`PendingGame`].

use std::collections::HashMap;

use bevy::prelude::*;
use zr_assets::Vfs;
use zr_game::{AiDifficulty, PlayerSetup};
use zr_ini::IniDb;
use zr_text::GameText;
use zr_ui::image::ImageDb;
use zr_ui::wnd::{status, WndFile};

use crate::convert;

/// Top-level shell screen.
#[derive(States, Default, Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub enum ShellState {
    /// The `Window\Menus\MainMenu.wnd` start screen (Skirmish / Options / Exit).
    #[default]
    MainMenu,
    /// The `Window\Menus\SkirmishGameOptionsMenu.wnd` setup screen.
    Setup,
    /// The load screen (`Menus/MultiplayerLoadScreen.wnd`); the map/world is built in stages here.
    Loading,
    InGame,
    Score,
    /// Transient: tear the game down, then enter [`ShellState::Loading`] with a fresh [`PendingGame`].
    Restart,
}

/// Registers the shell data loader (built once, on the first [`ShellState::MainMenu`] entry). The
/// main menu needs the same data (text, faction labels, images) as the setup screen, so it is loaded
/// before either is shown.
pub fn add_systems(app: &mut App) {
    app.add_systems(OnEnter(ShellState::MainMenu), setup_shell_data.run_if(shell_data_absent));
    app.add_systems(OnEnter(ShellState::Setup), setup_shell_data.run_if(shell_data_absent));
    // Gadget piece / label state follows the pointer (`gadget.rs`); a no-op when no gadget exists.
    app.add_systems(Update, (crate::gadget::update_visuals, crate::gadget::update_sliders));
}

fn shell_data_absent(data: Option<Res<ShellData>>) -> bool {
    data.is_none()
}

/// One selectable map (a `MapCache` block; `MapUtil.cpp` / `INIMapCache.cpp`).
#[derive(Clone, Debug)]
pub struct MapEntry {
    /// VFS path or `maps\name\name.map` (`Game::map_path` accepts either).
    pub path: String,
    /// Localized `nameLookupTag` / `displayName`, with ` (n)` for multiplayer maps.
    pub display: String,
    pub num_players: usize,
    pub multiplayer: bool,
    /// Decoded preview (`.tga` next to the map), when present.
    pub preview: Option<Handle<Image>>,
    /// `extentMin` / `extentMax` (`Region3D` X/Y), for placing start-position markers.
    pub extent_min: [f32; 2],
    pub extent_max: [f32; 2],
    /// `Player_1_Start`..`Player_N_Start` (X/Y), truncated to `num_players`.
    pub starts: Vec<[f32; 2]>,
}

/// A playable `PlayerTemplate` side (USA / China / GLA and the generals).
#[derive(Clone, Debug)]
pub struct FactionEntry {
    pub template: String,
    pub label: String,
    /// `PlayerTemplate::getGeneralFeatures()` (`Features` label), or `GUI:PlayerObserver`.
    pub features: String,
    /// `LoadScreen.cpp`'s `LocalGeneralPortrait` mapped image: a `GeneralPersona`'s
    /// `BioPortraitLarge`, else the faction logo (`SAFactionLogoLg_US`, ...). Empty when none.
    pub portrait: String,
    /// The general's `BioNameString` label when a persona matches, else [`Self::display_name`].
    pub general_name: String,
}

/// One `MultiplayerColor` definition.
#[derive(Clone, Debug)]
pub struct ColorEntry {
    pub label: String,
    pub rgb: [u8; 3],
}

/// A decoded `.wnd` mapped image: texture plus its UV sub-rect (atlas pages).
#[derive(Clone)]
pub struct ImageEntry {
    pub handle: Handle<Image>,
    pub rect: Option<Rect>,
}

/// The shell's loaded data (no VFS / INI kept alive).
#[derive(Resource)]
pub struct ShellData {
    pub text: GameText,
    pub maps: Vec<MapEntry>,
    pub factions: Vec<FactionEntry>,
    pub colors: Vec<ColorEntry>,
    pub cash_choices: Vec<u32>,
    /// The parsed `SkirmishGameOptionsMenu.wnd`, for the background.
    pub wnd: Option<WndFile>,
    /// The parsed `Menus/MainMenu.wnd` (start screen).
    pub main_menu_wnd: Option<WndFile>,
    /// The parsed `Menus/MultiplayerLoadScreen.wnd` (skirmish load screen).
    pub load_wnd: Option<WndFile>,
    /// The parsed `Menus/QuitMenu.wnd` (the in-game pause menu).
    pub quit_wnd: Option<WndFile>,
    /// The parsed `Menus/ScoreScreen.wnd` (the end-of-game score screen).
    pub score_wnd: Option<WndFile>,
    /// The parsed `Menus/OptionsMenu.wnd` (the options screen, `OptionsMenu.cpp`).
    pub options_wnd: Option<WndFile>,
    /// Mapped-image name -> decoded texture (all three layouts share it).
    pub images: HashMap<String, ImageEntry>,
    /// `.wnd` font faces by lower-case family name (`load_system_fonts`).
    pub fonts: HashMap<String, Handle<Font>>,
}

/// A player slot's state (`GameSlot::getState`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState {
    Human,
    Open,
    Closed,
    EasyAi,
    NormalAi,
    HardAi,
}

/// The name a human player shows in the player lists. The engine's default player name is the
/// machine name (`DESKTOP-…` in retail's setup screen and score screen); `Player` without it.
pub fn human_name() -> String {
    std::env::var("COMPUTERNAME").ok().filter(|n| !n.is_empty()).unwrap_or_else(|| "Player".into())
}

/// An AI player's name in the player lists: its army name (`GUI:EasyAI` / `GUI:MediumAI` /
/// `GUI:HardAI`, "Medium Army" for a Normal AI in retail). The English name stands in when the
/// string table lacks the label.
pub fn ai_army_label(text: &GameText, difficulty: AiDifficulty) -> String {
    let (label, fallback) = match difficulty {
        AiDifficulty::Easy => ("GUI:EasyAI", "Easy Army"),
        AiDifficulty::Normal => ("GUI:MediumAI", "Medium Army"),
        AiDifficulty::Hard => ("GUI:HardAI", "Hard Army"),
    };
    text.get(label).map_or_else(|| fallback.to_owned(), str::to_owned)
}

impl SlotState {
    pub fn is_ai(self) -> bool {
        matches!(self, SlotState::EasyAi | SlotState::NormalAi | SlotState::HardAi)
    }
    pub fn difficulty(self) -> AiDifficulty {
        match self {
            SlotState::EasyAi => AiDifficulty::Easy,
            SlotState::HardAi => AiDifficulty::Hard,
            _ => AiDifficulty::Normal,
        }
    }
    /// `UpdateSlotList`: a slot is played when it is the local human or an AI.
    pub fn is_active(self) -> bool {
        self == SlotState::Human || self.is_ai()
    }
}

/// One row of the setup screen.
#[derive(Clone, Debug)]
pub struct Slot {
    pub state: SlotState,
    /// Index into [`ShellData::factions`].
    pub faction: usize,
    /// `MultiplayerColor` index, or `-1` for the faction colour.
    pub color: i32,
    /// `0` = no team, `1..=4` group allies.
    pub team: u8,
    /// Chosen start position, or `-1` for automatic.
    pub start: i32,
}

/// The live setup selection (`SkirmishGameInfo`).
#[derive(Resource, Clone, Debug)]
pub struct ShellConfig {
    pub map: usize,
    pub slots: Vec<Slot>,
    pub starting_cash: usize,
    pub superweapons: bool,
    /// The map-select overlay is open.
    pub show_map_list: bool,
    /// First map row shown in the overlay.
    pub map_scroll: usize,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            map: 0,
            slots: (0..8)
                .map(|i| Slot {
                    state: if i == 0 { SlotState::Human } else if i == 1 { SlotState::NormalAi } else { SlotState::Open },
                    faction: 0,
                    // Retail defaults every slot to a random colour (the first combo entry).
                    color: -1,
                    team: 0,
                    start: -1,
                })
                .collect(),
            starting_cash: 0,
            superweapons: false,
            show_map_list: false,
            map_scroll: 0,
        }
    }
}

/// A pending "start the game" request (map + slots + options).
#[derive(Resource, Clone, Debug)]
pub struct PendingGame(pub GameConfig);

/// What `main::run_build` needs to start a skirmish.
#[derive(Clone, Debug)]
pub struct GameConfig {
    pub map: String,
    pub slots: Vec<PlayerSetup>,
    pub starting_cash: u32,
    pub superweapon_restriction: u8,
    /// Logic RNG seed (`GameLogic`); recorded in replays for exact playback.
    pub seed: u32,
}

impl GameConfig {
    /// Rebuilds a config from a replay header (`--replay`).
    pub fn from_replay_header(h: &zr_game::replay::ReplayHeader) -> GameConfig {
        GameConfig {
            map: h.map.clone(),
            slots: h.slots.clone(),
            starting_cash: h.starting_cash,
            superweapon_restriction: h.superweapon_restriction,
            seed: h.seed,
        }
    }
}

impl ShellConfig {
    /// Builds a [`GameConfig`] from the selection (`SkirmishGameInfo` -> `startNewGame`).
    pub fn to_game(&self, data: &ShellData) -> GameConfig {
        let cash = data.cash_choices.get(self.starting_cash).copied().unwrap_or(0);
        let mut used = [false; 8];
        let mut next_start = 0u8;
        let mut slots = Vec::new();
        for (i, s) in self.slots.iter().enumerate() {
            if !s.state.is_active() {
                continue;
            }
            let faction = data.factions.get(s.faction).map(|f| f.template.clone()).unwrap_or_else(|| "FactionAmerica".to_owned());
            let start = if (0..8).contains(&s.start) && !used[s.start as usize] {
                used[s.start as usize] = true;
                s.start as u8
            } else {
                while next_start < 8 && used[next_start as usize] {
                    next_start += 1;
                }
                let p = next_start.min(7);
                used[p as usize] = true;
                p
            };
            let mut setup = if s.state == SlotState::Human {
                PlayerSetup::new(&faction, start)
            } else {
                PlayerSetup::ai(&faction, start).with_difficulty(s.state.difficulty())
            };
            setup = setup.with_team(s.team).with_color(s.color).with_cash(cash);
            let _ = i;
            slots.push(setup);
        }
        GameConfig {
            map: data.maps.get(self.map).map(|m| m.path.clone()).unwrap_or_else(|| "Alpine Assault".to_owned()),
            slots,
            starting_cash: cash,
            superweapon_restriction: u8::from(self.superweapons),
            seed: 1,
        }
    }
}

/// Startup: load the shell data (install -> VFS -> INI -> maps/factions/colours + `.wnd` images).
/// The `.wnd` font faces the port can draw with: the Windows font files (`%WINDIR%\Fonts`), keyed by
/// the lower-case family name the `.wnd` `FONT` line uses. Bevy's family lookup does not reach these
/// system fonts, so they are loaded as font assets.
fn load_system_fonts(fonts: &mut Assets<Font>) -> HashMap<String, Handle<Font>> {
    const FILES: &[(&str, &str)] = &[("arial", "arial.ttf"), ("times new roman", "times.ttf"), ("courier new", "cour.ttf")];
    let dir = std::path::PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".to_owned())).join("Fonts");
    let mut out = HashMap::new();
    for (family, file) in FILES {
        match std::fs::read(dir.join(file)) {
            Ok(bytes) => {
                out.insert((*family).to_owned(), fonts.add(Font::from_bytes(bytes)));
            }
            Err(e) => warn!("shell: font {family} not loaded: {e}"),
        }
    }
    out
}

pub fn setup_shell_data(mut commands: Commands, mut assets: ResMut<Assets<Image>>, mut font_assets: ResMut<Assets<Font>>) {
    let Some(install) = zr_assets::Installs::detect().zero_hour else {
        warn!("shell: no Zero Hour install; setup screen disabled");
        commands.insert_resource(ShellData::empty());
        commands.insert_resource(ShellConfig::default());
        return;
    };
    let Ok(vfs) = Vfs::zero_hour(&install) else {
        warn!("shell: could not mount Zero Hour VFS");
        commands.insert_resource(ShellData::empty());
        commands.insert_resource(ShellConfig::default());
        return;
    };
    let ini = zr_ini::load_zero_hour(&vfs).db;
    let language = zr_ini::detect_language(&vfs);
    let text = zr_text::load(&vfs, &language).unwrap_or_default();
    let image_db = ImageDb::from_ini(&ini);
    let fonts = load_system_fonts(&mut font_assets);

    // `MapCache.ini` holds the map list metadata (extent, waypoints, display name); the engine
    // loads it per map directory (`MapCache::loadMapsFromMapCacheINI`), not as a `Data\INI` file.
    let mut map_ini = IniDb::new();
    for p in ["maps\\mapcache.ini", "data\\ini\\mapcache.ini"] {
        if let Some(bytes) = vfs.read(p) {
            if let Ok(blocks) = zr_ini::parse(p, &String::from_utf8_lossy(&bytes)) {
                map_ini.extend(blocks);
            }
        }
    }
    let maps = load_maps(&vfs, &map_ini, &text, &mut assets);
    let challenge_generals = load_challenge_generals(&vfs);
    let factions = load_factions(&ini, &text, &challenge_generals);
    let colors = load_colors(&ini, &text);
    let (cash_choices, default_cash) = load_cash(&ini);

    // The setup layout; the shell pushes `Menus/SkirmishGameOptionsMenu.wnd`. The start screen and
    // the skirmish load screen (`GameLogic::getLoadScreen` returns `MultiPlayerLoadScreen` for
    // `GAME_SKIRMISH`) are loaded alongside it so all three share the decoded image cache.
    let wnd = vfs
        .read(r"Window\Menus\SkirmishGameOptionsMenu.wnd")
        .and_then(|b| zr_ui::parse(&b).ok());
    let main_menu_wnd = vfs
        .read(r"Window\Menus\MainMenu.wnd")
        .and_then(|b| zr_ui::parse(&b).ok());
    let load_wnd = vfs
        .read(r"Window\Menus\MultiplayerLoadScreen.wnd")
        .and_then(|b| zr_ui::parse(&b).ok());
    let quit_wnd = vfs.read(r"Window\Menus\QuitMenu.wnd").and_then(|b| zr_ui::parse(&b).ok());
    let score_wnd = vfs.read(r"Window\Menus\ScoreScreen.wnd").and_then(|b| zr_ui::parse(&b).ok());
    let options_wnd = vfs.read(r"Window\Menus\OptionsMenu.wnd").and_then(|b| zr_ui::parse(&b).ok());
    let mut images = HashMap::new();
    let mut failed = std::collections::HashSet::new();
    // Every draw set, not only the enabled one: combo boxes, sliders and check boxes draw from
    // their own `COMBOBOX*` / `SLIDER*` / check-box sets.
    for w in [&wnd, &main_menu_wnd, &load_wnd, &quit_wnd, &score_wnd, &options_wnd].into_iter().flatten() {
        for win in w.windows.iter().flat_map(|r| r.walk()) {
            for name in win.draw_data.image_names() {
                if !images.contains_key(name) {
                    if let Some(e) = decode_mapped(&vfs, &image_db, &language, &mut assets, &mut failed, name) {
                        images.insert(name.to_owned(), e);
                    }
                }
            }
        }
    }
    // Faction portraits and the load screen's `PlayerStart` marker (drawn by `loading.rs`).
    for name in factions.iter().map(|f| f.portrait.clone()).filter(|p| !p.is_empty()).chain(["PlayerStart".to_owned()]) {
        if let std::collections::hash_map::Entry::Vacant(slot) = images.entry(name.clone()) {
            if let Some(e) = decode_mapped(&vfs, &image_db, &language, &mut assets, &mut failed, &name) {
                slot.insert(e);
            }
        }
    }
    info!(
        "shell: {} maps, {} factions, {} colours, {} cash choices; SkirmishGameOptionsMenu.wnd {}, MainMenu.wnd {}, MultiplayerLoadScreen.wnd {}, {} images",
        maps.len(),
        factions.len(),
        colors.len(),
        cash_choices.len(),
        if wnd.is_some() { "loaded" } else { "missing" },
        if main_menu_wnd.is_some() { "loaded" } else { "missing" },
        if load_wnd.is_some() { "loaded" } else { "missing" },
        images.len(),
    );

    let mut config = ShellConfig::default();
    // Default to Alpine Assault (the viewer's historical default), else a multiplayer map with a
    // preview (like `getDefaultMap`).
    if let Some(i) = maps.iter().position(|m| m.path.to_ascii_lowercase().contains("alpine")) {
        config.map = i;
    } else if let Some(i) = maps.iter().position(|m| m.multiplayer && m.num_players >= 2 && m.preview.is_some()) {
        config.map = i;
    }
    if let Some(i) = cash_choices.iter().position(|&c| c == default_cash) {
        config.starting_cash = i;
    }
    commands.insert_resource(ShellData {
        text,
        maps,
        factions,
        colors,
        cash_choices,
        wnd,
        main_menu_wnd,
        load_wnd,
        quit_wnd,
        score_wnd,
        options_wnd,
        images,
        fonts,
    });
    commands.insert_resource(config);
}

impl ShellData {
    fn empty() -> ShellData {
        ShellData {
            text: GameText::default(),
            maps: Vec::new(),
            factions: vec![FactionEntry {
                template: "FactionAmerica".into(),
                label: "USA".into(),
                features: "GUI:PlayerObserver".into(),
                portrait: String::new(),
                general_name: "USA".into(),
            }],
            colors: vec![ColorEntry { label: "Gold".into(), rgb: [255, 220, 0] }],
            cash_choices: vec![10000],
            wnd: None,
            main_menu_wnd: None,
            load_wnd: None,
            quit_wnd: None,
            score_wnd: None,
            options_wnd: None,
            images: HashMap::new(),
            fonts: HashMap::new(),
        }
    }
}

/// `MapCache` blocks (official maps) with a VFS scan fallback for loose/extra maps.
fn load_maps(vfs: &Vfs, map_ini: &IniDb, text: &GameText, assets: &mut Assets<Image>) -> Vec<MapEntry> {
    let mut maps: Vec<MapEntry> = Vec::new();
    let cache_blocks = map_ini.of_kind("MapCache").count();
    info!("shell: {cache_blocks} MapCache blocks");
    for b in map_ini.of_kind("MapCache") {
        let Some(raw) = b.name() else { continue };
        // The block name is quoted-printable (`INI::parseMapCacheDefinition`).
        let path = decode_quoted_printable(raw).trim().to_owned();
        if !path.to_ascii_lowercase().ends_with(".map") {
            continue;
        }
        let num_players = b.value("numPlayers").and_then(|v| v.trim().parse().ok()).unwrap_or(2);
        let multiplayer = b
            .value("isMultiplayer")
            .is_none_or(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "yes" | "true" | "1"));
        let lookup = b.value("nameLookupTag").map(|s| s.trim()).filter(|s| !s.is_empty());
        let display_ascii = b.value("displayName").map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        let name = path.rsplit(['\\', '/']).next().unwrap_or(&path).trim_end_matches(".map").to_owned();
        let mut display = lookup
            .and_then(|t| text.get(t).map(str::to_owned))
            .or(display_ascii)
            .unwrap_or_else(|| name.clone());
        if num_players >= 2 {
            display = format!("{display} ({num_players})");
        }
        let preview = load_preview(vfs, &path, assets);
        let extent_min = b.value("extentMin").and_then(parse_coord2d).unwrap_or([0.0, 0.0]);
        let extent_max = b.value("extentMax").and_then(parse_coord2d).unwrap_or([0.0, 0.0]);
        let mut starts: Vec<[f32; 2]> = (1..=8).filter_map(|i| b.value(&format!("Player_{i}_Start")).and_then(parse_coord2d)).collect();
        starts.truncate(num_players.max(0) as usize);

        maps.push(MapEntry { path, display, num_players: num_players as usize, multiplayer, preview, extent_min, extent_max, starts });
    }
    // Fallback / additions: archived `maps\*.map` files not in the cache.
    if maps.len() < 2 {
        let mut seen: std::collections::HashSet<String> = maps.iter().map(|m| m.path.to_ascii_lowercase()).collect();
        let mut extra: Vec<String> = Vec::new();
        for (path, _) in vfs.files() {
            let p = path.to_ascii_lowercase();
            if p.starts_with("maps\\") && p.ends_with(".map") && !seen.contains(&p) {
                seen.insert(p.clone());
                extra.push(path.to_owned());
            }
        }
        extra.sort();
        for path in extra {
            let name = path.rsplit('\\').next().unwrap_or(&path).trim_end_matches(".map").to_owned();
            let preview = load_preview(vfs, &path, assets);
            maps.push(MapEntry {
                path,
                display: name.clone(),
                num_players: 2,
                multiplayer: true,
                preview,
                extent_min: [0.0, 0.0],
                extent_max: [0.0, 0.0],
                starts: Vec::new(),
            });
        }
    }
    maps.sort_by_key(|m| m.display.to_ascii_lowercase());
    let with_preview = maps.iter().filter(|m| m.preview.is_some()).count();
    info!("shell: {} of {} maps have a preview image", with_preview, maps.len());
    maps
}

/// `getMapPreviewImage`: the map path with `.map` -> `.tga`.
fn load_preview(vfs: &Vfs, map_path: &str, assets: &mut Assets<Image>) -> Option<Handle<Image>> {
    let tga = format!("{}.tga", map_path.trim_end_matches(".map").trim_end_matches(".MAP"));
    match zr_w3d::load_texture(vfs, &tga) {
        Ok(rgba) => Some(assets.add(convert::texture_image(&rgba, convert::TextureUse::Ui))),
        Err(_) => None,
    }
}

/// Playable sides: `PlayerTemplate`s with a starting building, deduped by `Side` (generals have
/// their own side), labelled `SIDE:<side>` (`GUIUtil.cpp::PopulatePlayerTemplateComboBox`).
///
/// `general` is the `GeneralPersona` table (`Data\INI\ChallengeMode.ini`) keyed by `PlayerTemplate`,
/// giving the load screen's `LocalGeneralPortrait`/`LocalGeneralName` (`LoadScreen.cpp:1278`).
fn load_factions(ini: &IniDb, text: &GameText, general: &HashMap<String, (String, String)>) -> Vec<FactionEntry> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for b in ini.of_kind("PlayerTemplate") {
        let Some(name) = b.name() else { continue };
        let get = |k: &str| b.value(k).map(|v| v.trim()).unwrap_or("");
        if get("StartingBuilding").is_empty() || get("PlayableSide").eq_ignore_ascii_case("no") {
            continue;
        }
        let side = get("Side").to_owned();
        if side.is_empty() || !seen.insert(side.to_ascii_lowercase()) {
            continue;
        }
        let label = text
            .get(&format!("SIDE:{side}"))
            .map(str::to_owned)
            .or_else(|| text.get(&format!("GUI:{side}")).map(str::to_owned))
            .unwrap_or_else(|| side.clone());
        // `PlayerTemplate::getDisplayName()` / `getGeneralFeatures()` (`PlayerTemplate.cpp:73/116`).
        let display_name = text.get(get("DisplayName")).map(str::to_owned).unwrap_or_else(|| name.to_owned());
        let features = text.get(get("Features")).map(str::to_owned).unwrap_or_else(|| "GUI:PlayerObserver".to_owned());
        // The load screen's portrait: a persona's `BioPortraitLarge`, else the faction logo.
        let persona = general.get(name);
        let portrait = persona.map(|p| p.0.clone()).unwrap_or_else(|| faction_logo(name).to_owned());
        let general_name = persona.and_then(|p| text.get(&p.1).map(str::to_owned)).unwrap_or_else(|| display_name.clone());
        out.push(FactionEntry { template: name.to_owned(), label, features, portrait, general_name });
    }
    if out.is_empty() {
        out.push(FactionEntry {
            template: "FactionAmerica".into(),
            label: "USA".into(),
            features: "GUI:PlayerObserver".into(),
            portrait: "SAFactionLogoLg_US".into(),
            general_name: "USA".into(),
        });
    }
    out
}

/// The hardcoded original-faction logos (`LoadScreen.cpp:1291`).
fn faction_logo(template: &str) -> &'static str {
    match template {
        "FactionAmerica" => "SAFactionLogoLg_US",
        "FactionGLA" => "SUFactionLogoLg_GLA",
        "FactionChina" => "SNFactionLogoLg_China",
        _ => "",
    }
}

/// `Data\INI\ChallengeMode.ini`'s `GeneralPersona` blocks -> `PlayerTemplate` -> (`BioPortraitLarge`,
/// `BioNameString`). `ChallengeGenerals::parseGeneralPersona` (`ChallengeGenerals.cpp:62`).
fn load_challenge_generals(vfs: &Vfs) -> HashMap<String, (String, String)> {
    let mut out = HashMap::new();
    let Some(bytes) = vfs.read("Data\\INI\\ChallengeMode.ini") else { return out };
    let Ok(blocks) = zr_ini::parse("ChallengeMode.ini", &String::from_utf8_lossy(&bytes)) else { return out };
    for b in blocks.iter().filter(|b| b.kind == "GeneralPersona") {
        let template = b.value("PlayerTemplate").map(|v| v.trim().to_owned()).unwrap_or_default();
        if template.is_empty() {
            continue;
        }
        let portrait = b.value("BioPortraitLarge").map(|v| v.trim().to_owned()).unwrap_or_default();
        let name = b.value("BioNameString").map(|v| v.trim().to_owned()).unwrap_or_default();
        out.insert(template, (portrait, name));
    }
    out
}

/// `MultiplayerColor` definitions, in file order (`MultiplayerSettings.cpp`).
fn load_colors(ini: &IniDb, text: &GameText) -> Vec<ColorEntry> {
    let mut out = Vec::new();
    for b in ini.of_kind("MultiplayerColor") {
        let label = b
            .value("TooltipName")
            .and_then(|t| text.get(t.trim()).map(str::to_owned))
            .or_else(|| b.name().map(str::to_owned))
            .unwrap_or_else(|| format!("Color {}", out.len()));
        let rgb = b.value("RGBColor").and_then(parse_rgb).unwrap_or([255, 255, 255]);
        out.push(ColorEntry { label, rgb });
    }
    if out.is_empty() {
        out.push(ColorEntry { label: "Gold".into(), rgb: [255, 220, 0] });
    }
    out
}

/// `MultiplayerStartingMoneyChoice` values, plus the `Default = Yes` one (`GameInfo.cpp` /
/// `MultiplayerSettings::getDefaultStartingMoney`); falls back to the project's 10000.
fn load_cash(ini: &IniDb) -> (Vec<u32>, u32) {
    let mut out: Vec<u32> = Vec::new();
    let mut default = None;
    // The blocks are unnamed, so `of_kind` would collapse them to one; iterate the raw list.
    for b in ini.iter().filter(|b| b.kind == "MultiplayerStartingMoneyChoice") {
        if let Some(v) = b.value("Value").and_then(|v| v.trim().parse::<u32>().ok()) {
            if !out.contains(&v) {
                out.push(v);
            }
            if b.value("Default").is_some_and(|d| d.trim().eq_ignore_ascii_case("yes")) {
                default = Some(v);
            }
        }
    }
    if out.is_empty() {
        out.push(10000);
    }
    out.sort_unstable();
    (out, default.unwrap_or(10000))
}

/// `QuotedPrintableToAsciiString`: `_XX` hex escapes (`_5C` -> `\`, `_20` -> space, `_2E` -> `.`).
fn decode_quoted_printable(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'_' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `X:1 Y:2` -> `[1, 2]` (`INI::parseCoord3D`'s X/Y).
fn parse_coord2d(s: &str) -> Option<[f32; 2]> {
    let mut out = [0.0f32; 2];
    let mut any = false;
    for t in s.split_whitespace() {
        let Some((k, v)) = t.split_once(':') else { continue };
        match k.to_ascii_uppercase().as_str() {
            "X" => {
                out[0] = v.trim().parse().ok()?;
                any = true;
            }
            "Y" => {
                out[1] = v.trim().parse().ok()?;
                any = true;
            }
            _ => {}
        }
    }
    any.then_some(out)
}

fn parse_rgb(s: &str) -> Option<[u8; 3]> {
    let mut c = [None; 3];
    for t in s.split_whitespace() {
        let (k, v) = t.split_once(':')?;
        let i = match k.to_ascii_uppercase().as_str() {
            "R" => 0,
            "G" => 1,
            "B" => 2,
            _ => continue,
        };
        c[i] = Some(v.trim().parse::<i32>().ok()?.clamp(0, 255) as u8);
    }
    Some([c[0]?, c[1]?, c[2]?])
}

/// Resolves a mapped image to a decoded texture (same search order as the control bar).
fn decode_mapped(
    vfs: &Vfs,
    images: &ImageDb,
    language: &str,
    assets: &mut Assets<Image>,
    failed: &mut std::collections::HashSet<String>,
    name: &str,
) -> Option<ImageEntry> {
    if failed.contains(name) {
        return None;
    }
    let exists = |p: &str| vfs.read_archived(p).is_some();
    let path = match images.resolve(name, language, &exists) {
        zr_ui::ImageResolution::Found { path, .. } => path,
        _ => {
            failed.insert(name.to_owned());
            return None;
        }
    };
    let rect = images.get(name).map(|i| Rect::new(i.left as f32, i.top as f32, i.right as f32, i.bottom as f32));
    match zr_w3d::load_texture(vfs, &path) {
        Ok(rgba) => Some(ImageEntry { handle: assets.add(convert::texture_image(&rgba, convert::TextureUse::Ui)), rect }),
        Err(_) => {
            failed.insert(name.to_owned());
            None
        }
    }
}

/// Scale factor from a `.wnd`'s creation resolution to the window.
pub fn wnd_scale(wnd: &WndFile, window: Option<&Window>) -> (f32, f32) {
    let (win_w, win_h) = window.map_or((1600.0, 900.0), |w| (w.width(), w.height()));
    let (cw, ch) = wnd
        .windows
        .first()
        .map(|w| w.rect.creation_resolution)
        .filter(|&(x, y)| x > 0 && y > 0)
        .unwrap_or((800, 600));
    (win_w / cw as f32, win_h / ch as f32)
}

/// Scaled rect `(x, y, w, h)`.
pub fn scaled(rect: zr_ui::ScreenRect, sx: f32, sy: f32) -> (f32, f32, f32, f32) {
    (
        rect.upper_left.0 as f32 * sx,
        rect.upper_left.1 as f32 * sy,
        rect.width() as f32 * sx,
        rect.height() as f32 * sy,
    )
}

/// Renders a `.wnd` file as a static background (panels, images and labels).
#[allow(clippy::too_many_arguments)]
pub fn render_wnd_background(
    commands: &mut Commands,
    root: Entity,
    wnd: &WndFile,
    images: &HashMap<String, ImageEntry>,
    text: &GameText,
    window: Option<&Window>,
    skip_replaced: bool,
) {
    render_wnd_filtered(commands, root, wnd, images, text, window, skip_replaced, &|_| false);
}

/// Like [`render_wnd_background`], but `hide` can suppress named windows (used by the main menu to
/// show one dropdown panel at a time, matching `MainMenuInit`'s `initialHide` + the transition
/// handler; `GameWindow::winHide`).
#[allow(clippy::too_many_arguments)]
pub fn render_wnd_filtered(
    commands: &mut Commands,
    root: Entity,
    wnd: &WndFile,
    images: &HashMap<String, ImageEntry>,
    text: &GameText,
    window: Option<&Window>,
    skip_replaced: bool,
    hide: &dyn Fn(&str) -> bool,
) {
    let (sx, sy) = wnd_scale(wnd, window);
    for w in &wnd.windows {
        spawn_wnd(commands, root, w, sx, sy, images, text, skip_replaced, true, hide);
    }
}


#[allow(clippy::too_many_arguments)]
fn spawn_wnd(
    commands: &mut Commands,
    root: Entity,
    w: &zr_ui::Window,
    sx: f32,
    sy: f32,
    images: &HashMap<String, ImageEntry>,
    text: &GameText,
    skip_replaced: bool,
    shown: bool,
    hide: &dyn Fn(&str) -> bool,
) {
    let name = w.name.as_deref().unwrap_or("");
    let hidden = w.status & status::HIDDEN != 0 || hide(name);
    let shown = shown && !hidden;
    // `DRAWCALLBACK = W3DNoDraw`: the window paints nothing (the engine's no-op draw callback);
    // `MainMenu.wnd:MainMenuParent` uses it so the shell map shows behind the menu art. Children
    // still draw.
    let draw = shown && w.draw_callback.as_deref() != Some("W3DNoDraw");
    let (x, y, width, height) = scaled(w.rect, sx, sy);
    let slider = w.style & (zr_ui::wnd::style::VERTSLIDER | zr_ui::wnd::style::HORZSLIDER) != 0;
    if draw && width > 0.0 && height > 0.0 && !slider {
        // Windows the menu draws itself (widgets + labels) are not drawn from the `.wnd`, so their
        // static text/background does not duplicate ours.
        let skip = skip_replaced
            && (replaced_widget(name)
                || w.text.as_deref().is_some_and(|l| l.to_ascii_lowercase().contains("speed"))
                || name.to_ascii_lowercase().contains("speed"));
        if !skip {
            let node = Node {
                position_type: PositionType::Absolute,
                left: Val::Px(x),
                top: Val::Px(y),
                width: Val::Px(width),
                height: Val::Px(height),
                ..default()
            };
            let mut entity = commands.spawn((node, ChildOf(root)));
            // The special-power shortcut bar hides the frame of an unavailable slot.
            if let Some(n) = name.rsplit_once("ButtonParent").and_then(|(_, n)| n.parse::<usize>().ok()) {
                entity.insert(crate::promotions::ShortcutFrame(n.saturating_sub(1)));
            }
            // `STATICTEXT` windows are labels and button/entry gadgets get their colour from an
            // image: the engine draws no fill for them, so their `.wnd` placeholder colour (often
            // opaque red) is skipped.
            const NO_FILL: u32 = zr_ui::wnd::style::STATICTEXT
                | zr_ui::wnd::style::PUSHBUTTON
                | zr_ui::wnd::style::CHECKBOX
                | zr_ui::wnd::style::RADIOBUTTON
                | zr_ui::wnd::style::ENTRYFIELD
                | zr_ui::wnd::style::COMBOBOX;
            let no_fill = w.style & NO_FILL != 0;
            if let Some(d) = w.draw_data.enabled.first() {
                // `W3DGadget`: when the window has `WIN_STATUS_IMAGE` the engine draws the image
                // instead of the colour fill, so a transparent-centred frame image (e.g.
                // `MainMenuRuler`) lets the shell map show through instead of a black fill.
                let image = d.image.as_ref().and_then(|n| images.get(n));
                if image.is_none() && d.color.a > 0 && !no_fill {
                    entity.insert(BackgroundColor(rgba(d.color)));
                }
                if let Some(entry) = image {
                    // The engine stretches every mapped image into its window rect
                    // (`W3DGameWindowManager` blits the draw data over the scaled window);
                    // `NodeImageMode::Auto` instead keeps the source aspect ratio, which made
                    // the load screen's panel art 4:3 while `WinMapPreview`/the bars used the
                    // raw 800x600 -> screen transform and poked past the frame.
                    entity.insert(ImageNode { image: entry.handle.clone(), rect: entry.rect, image_mode: NodeImageMode::Stretch, ..default() });
                }
            }
            if let Some(label) = &w.text {
                if let Some(s) = text.get(label) {
                    let size = w.font.as_ref().map_or(12.0, |f| f.size as f32 * sy).clamp(8.0, 28.0);
                    let color = w.text_color.map_or(Color::srgb(0.95, 0.95, 0.85), |c| rgba(c.enabled.color));
                    commands.spawn((
                        Text::new(s.to_owned()),
                        TextFont { font_size: FontSize::Px(size), ..default() },
                        TextColor(color),
                        TextLayout { justify: Justify::Center, ..default() },
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(x),
                            top: Val::Px(y + height * 0.5 - size * 0.6),
                            width: Val::Px(width),
                            ..default()
                        },
                        ChildOf(root),
                    ));
                }
            }
        }
    }
    for c in &w.children {
        spawn_wnd(commands, root, c, sx, sy, images, text, skip_replaced, shown, hide);
    }
}

pub fn rgba(c: zr_ui::Rgba) -> Color {
    Color::srgba(c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0, c.a as f32 / 255.0)
}

/// `.wnd` windows the menu replaces with its own widgets (so the background renderer skips them and
/// their static text does not double up with ours). `StaticTextGameSpeed` is dropped because the
/// sim runs at a fixed 30 Hz and the slider has no control here.
pub fn replaced_widget(name: &str) -> bool {
    const SUFFIXES: &[&str] = &[
        "ButtonStart",
        "ButtonBack",
        "ButtonSelectMap",
        "ButtonReset",
        "CheckboxLimitSuperweapons",
        "ComboBoxStartingCash",
        "TextEntryMapDisplay",
        "TextEntryPlayerName",
        "StaticTextGameSpeed",
        // Promotions panel close button (overlaid by `promotions.rs`).
        "ButtonExit",
    ];
    const PREFIXES: &[&str] = &[
        "ComboBoxPlayer",
        "ComboBoxColor",
        "ComboBoxPlayerTemplate",
        "ComboBoxTeam",
        "ButtonMapStartPosition",
        // Promotions panel / special-power shortcut bar (overlaid by `promotions.rs`).
        "ButtonRank",
        "ButtonCommand",
    ];
    SUFFIXES.iter().any(|s| name.ends_with(s)) || PREFIXES.iter().any(|p| name.contains(p))
}

/// `MultiplayerColor` table as plain RGB, for the viewer's team-colour override.
pub fn color_table(data: &ShellData) -> Vec<[u8; 3]> {
    data.colors.iter().map(|c| c.rgb).collect()
}

/// Labels for the shell's team combo (`Team:0` .. `Team:4`).
pub fn team_labels(text: &GameText) -> Vec<String> {
    (0..=4)
        .map(|i| text.get(&format!("Team:{i}")).map(str::to_owned).unwrap_or_else(|| format!("Team {i}")))
        .collect()
}

/// Cash label (`GUI:StartingMoneyFormat`), substituting the engine's `%d`/`%u` placeholder.
pub fn cash_label(text: &GameText, amount: u32) -> String {
    let fmt = text.get("GUI:StartingMoneyFormat").unwrap_or("$%d");
    let mut out = String::new();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            if let Some(&n) = chars.peek() {
                if matches!(n, 'd' | 'u' | 'i' | 's') {
                    chars.next();
                    out.push_str(&amount.to_string());
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// Map display name by index (used by the load/score screens).
pub fn map_display(data: &ShellData, index: usize) -> String {
    data.maps.get(index).map(|m| m.display.clone()).unwrap_or_default()
}

/// Faction label by index.
pub fn faction_label(data: &ShellData, index: usize) -> String {
    data.factions.get(index).map(|f| f.label.clone()).unwrap_or_default()
}

/// Colour name by index (`-1` = random / the faction colour, like retail's first combo entry).
pub fn color_label(data: &ShellData, index: i32) -> String {
    if index < 0 {
        return data.text.get("GUI:Random").unwrap_or("Random").to_owned();
    }
    data.colors.get(index as usize).map(|c| c.label.clone()).unwrap_or_else(|| "?".to_owned())
}

/// A `.wnd` window's scaled rect, matched by the trailing `name` (the file prefix is ignored, like
/// the engine's `NameKeyType` window lookup).
pub fn wnd_rect(wnd: &WndFile, window: Option<&Window>, name: &str) -> Option<(f32, f32, f32, f32)> {
    let (sx, sy) = wnd_scale(wnd, window);
    wnd.windows
        .iter()
        .flat_map(|r| r.walk())
        .find(|w| w.name.as_deref().is_some_and(|n| n.ends_with(name)))
        .map(|w| scaled(w.rect, sx, sy))
}

/// The `FactionEntry` for a faction template name, if loaded.
pub fn faction_entry<'a>(data: &'a ShellData, template: &str) -> Option<&'a FactionEntry> {
    data.factions.iter().find(|f| f.template.eq_ignore_ascii_case(template))
}

/// The `PlayerTemplate` display label for a faction template name (fell back to the raw name), for
/// the load screen's side column (`GameSlot::getApparentPlayerTemplateDisplayName`).
pub fn faction_label_for(data: &ShellData, template: &str) -> String {
    data.factions
        .iter()
        .find(|f| f.template.eq_ignore_ascii_case(template))
        .map(|f| f.label.clone())
        .unwrap_or_else(|| template.to_owned())
}

/// The `MultiplayerColor` RGB for a slot's colour index (`-1` = the random/faction colour, mapped to
/// the given fallback).
pub fn color_rgb(data: &ShellData, index: i32, fallback: [u8; 3]) -> [u8; 3] {
    if index < 0 {
        return fallback;
    }
    data.colors.get(index as usize).map(|c| c.rgb).unwrap_or(fallback)
}


